# WSL support — design

Branch: `feat/wsl-support` (worktree off `master`, PR upstream to
mathix420/alacritree). Local-only spec; PR description carries the context.

## Problem

alacritree is Windows-native (egui GUI, ConPTY sessions) but real work
often lives inside WSL2. Today nothing in the app is WSL-aware:

- Sessions always spawn the configured Windows shell; there is no way to
  open a session inside a distro short of typing `wsl` by hand.
- Projects living in the WSL filesystem (`\\wsl.localhost\<distro>\...`)
  break the sidebar: git2 refuses the repo (libgit2 ownership check),
  status over the 9P share is both slow (~10–20x) and **wrong** (9P strips
  executable bits, so every `+x` file shows as modified when
  `core.filemode=true`; symlinks error), and worktrees created inside WSL
  record absolute Linux gitdir paths (`/home/...`) that Windows-side git2
  cannot resolve at all.
- `worktree.rs`/`pr_status.rs` shell out to Windows `git`/`gh`, which hit
  the same 9P and `safe.directory` problems against UNC paths.

Goal: full feature parity for WSL-resident projects — sessions, git
status, worktree create/delete, diff panes, PR status — plus WSL shells
for Windows-fs projects, with one new config option (`automount_root`).

## Decisions (2026-07-12)

- **Shell choice is auto-by-location with a per-project override**: a
  project under `\\wsl.localhost\<d>\...` gets distro `d`'s shell; a
  Windows-fs project gets today's behavior. A `state.toml` override
  (set from a sidebar context menu) flips either direction. No shell
  autodetection — inside WSL the distro's own default shell runs, which
  is `wsl.exe`'s behavior, not ours.
- **Git metadata for WSL repos runs real git *inside* the distro**, one
  batched `wsl.exe --exec` round trip per operation (the
  wslgit / GitHub-Desktop-proposed model). git2 over UNC is rejected on
  correctness, not just speed (filemode phantoms, unresolvable worktree
  paths). A persistent WSL-side helper is **specced as v2**, not built:
  VS Code needs its server for extension hosts and file watching — needs
  we don't have — and JetBrains' agent migration produced a long tail of
  lifecycle regressions. Measured on the dev machine: warm `wsl.exe`
  round trip ~400 ms vs ~55 ms for git inside the VM; one batched call
  per 1.5 s poll tick fits comfortably.
- **Canonical identity stays the Windows `PathBuf`** (UNC for WSL paths)
  everywhere in the app — state, project roots, session cwds. Linux
  paths exist only at the `wsl.exe` boundary.
- **Distro enumeration reads the registry**
  (`HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`), Windows
  Terminal's approach — no process spawn at startup, and it identifies
  the default distro. Fallback: parse `wsl.exe -l -q` with `WSL_UTF8=1`
  (wsl.exe's own output is UTF-16LE without it).

## Design

### 1. Platform layer (`wsl.rs`, new, `#[cfg(windows)]`)

The only module that knows WSL exists. Everything else consumes its
types or hands it commands to wrap.

- `WslDistro { name: String, is_default: bool }`.
  `distros() -> Vec<WslDistro>`: enumerate `Lxss` GUID subkeys
  (`DistributionName`, default from the root `DefaultDistribution`
  GUID), filter `docker-desktop*`/`rancher-desktop*`; registry
  unreadable → `wsl -l -q` fallback; both empty → WSL features dormant,
  app unchanged.
- `Location::Windows(PathBuf) | Wsl { distro: String, linux_path: String }`,
  classified from the `\\wsl$\` / `\\wsl.localhost\` prefix.
  Translations are pure functions: UNC ↔ Linux is a prefix strip;
  any drive path `X:\y` ↔ `<automount_root>/x/y` (lowercased drive
  letter — WSL automounts every fixed drive, not just `C:`). The
  automount root defaults to `/mnt` and is configurable via
  `[ui.wsl] automount_root` in `alacritree.toml` (alacritree-only
  option, so it lives under `[ui]` per crate convention; documented in
  the `Raw*` structs in `config.rs`). Linux paths reported by git
  (`/home/...`, `<root>/c/...`) translate back to UNC / drive paths
  before entering the app.
- `wsl_command(distro, argv) -> Command`: builds
  `wsl.exe -d <distro> --exec <argv>` with `hide_console()` and
  `WSL_UTF8=1` applied. `--exec` skips the user's shell and rc files
  (per-invocation rc sourcing is the known JetBrains latency trap).
  A shell-wrapped variant (`wsl.exe -d <d> --cd <dir> -- <cmdline>`)
  exists for the two places that need a pipe or cwd semantics: session
  spawn and diff panes.

### 2. Sessions and shell resolution (`app.rs`, `state.rs`)

- Resolution for a workspace: `state.toml` override
  (`shell = "windows"` / `"wsl:<distro>"`, keyed like the existing
  per-project entries) wins; else auto by the project root's
  `Location`; else today's config-driven shell. Overrides referencing a
  distro that no longer exists fall back to auto and log.
- WSL session spawn = existing `Session::spawn` with the shell set to
  `wsl.exe -d <distro> --cd <workdir>` — plain ConPTY, exactly a
  Windows Terminal WSL profile. `--cd` natively accepts Windows, UNC,
  and Linux paths (verified), so a Windows project overridden to WSL
  lands in `/mnt/c/...` with no translation on our side.
- Sidebar context menu on a project: "Open in… → Windows shell /
  WSL (<distro>)" writes the override and applies to sessions spawned
  after that; existing sessions are untouched (sessions outlive
  workspace switches, per crate convention).
- Diff panes for WSL repos: `spawn_command` with
  `wsl.exe -d <d> --cd <worktree> -- git diff <args> | delta`, running
  through the distro's default shell so `delta` resolves from the
  user's PATH. Missing delta prints inside the pane — same failure
  surface as Windows today. The existing Windows `escape_args`
  quoting rule applies to the wsl.exe argv itself.

### 3. Git metadata backend (`projects.rs`, `git_status.rs`)

Dispatch on the project root's `Location`; the `Location::Windows` arm
is the existing git2 code, untouched.

- `projects.rs` — WSL arm of `discover`/`refresh`: one batched
  `wsl.exe --exec sh -c '...'` emits, sentinel-separated:
  repo check + toplevel, current branch (`git symbolic-ref --short HEAD`
  falling back to short OID), `git worktree list --porcelain -z`, and
  default-branch probes replicating the existing priority
  (`refs/remotes/origin/HEAD` symref → `main`/`master`/`trunk`/`develop`
  existence → `init.defaultBranch` if that branch exists). Worktree
  paths translate to UNC. Non-repo WSL folders get the same
  pseudo-worktree fallback as today.
- `git_status.rs` — WSL arm of `compute` and `dirty_counts`: per 1.5 s
  refresh, **one** `wsl.exe --exec sh -c` round trip emitting
  `git status --porcelain=v2 -z` and `git diff --numstat -z` against
  the same diff target the git2 path computes today (PR base when one
  exists, else the default branch, preserving its merge-base
  semantics), parsed into the existing
  `FileChange`/`DiffStat`/`DirtyCounts` structs. Runs on the existing off-UI-thread refresh path, so the
  ~400 ms round trip never blocks paint.
- `worktree.rs` / `pr_status.rs`: already CLI shell-outs. Their
  `Command` construction routes through the platform layer — Windows
  repos build `git`/`gh` directly as today; WSL repos build
  `wsl.exe -d <d> --exec git -C <linux_path> ...` (same for `gh`).
  Progress streaming, branch validation, and the PR TTL cache are
  unchanged. PR status requires `gh` installed and authed *inside* the
  distro; the existing missing/unauthed-gh silent fallback to the
  default branch carries over.
- Parsing: porcelain v2 / `--porcelain -z` / `--numstat -z` everywhere —
  documented stable formats, NUL-delimited so renames and spaces are
  safe, and identical to what the v2 helper will emit (parsers are
  reused verbatim).

### 4. Polling and VM lifecycle

Touching a stopped distro's UNC path or running any `wsl.exe` command
boots the VM, and continuous polling keeps it alive. The existing
architecture already contains this: `StatusCache::poll` runs only while
the right sidebar renders the **active** workspace — inactive projects
never refresh on any platform and simply show their last computed
status (no stale marker today; WSL projects inherit that behavior
unchanged). So the VM is kept alive only while a WSL workspace is
active or has live sessions (whose `wsl.exe` processes pin it anyway).
No new gating logic is needed; the WSL arm must just avoid adding any
background polling of its own.

First refresh after a distro cold start can take seconds; the async
cache already tolerates slow computes (UI shows stale data until the
result lands).

### 5. Error handling

- No `wsl.exe`, no distros: WSL code paths dormant; zero behavior
  change for non-WSL users (macOS/Linux builds compile the module out).
- `wsl.exe` nonzero exit, deregistered distro, unparseable output:
  surface via the existing `last_error` toast, degrade to empty
  status / unchanged sidebar — log-and-continue, matching the `state.rs`
  philosophy. Never panic on WSL failures.
- Session spawn failure (distro deleted between click and spawn) reports
  through the existing "failed to spawn shell" path.

### 6. Testing

- Unit tests (pure, no WSL required): path classification and both
  translation directions (UNC ↔ Linux; `D:\x` as well as `C:\x` ↔
  automount paths, under `/mnt` and a non-default root), porcelain-v2
  status parsing, `--numstat -z` parsing, `worktree list --porcelain -z`
  parsing, registry-value → `WslDistro` mapping. Fixtures captured from
  real git output.
- Manual E2E checklist against `kali-linux`: add `\\wsl.localhost\
  kali-linux\home\...` project → sidebar lists worktrees with branches;
  session opens in the distro at the right cwd; status matches in-WSL
  `git status` on a repo with `+x` files and a worktree; worktree
  create/delete round trip; diff pane renders via in-distro delta; PR
  badge appears with in-distro `gh`; Windows project overridden to WSL
  opens at `/mnt/c/...`; stopped-distro first refresh recovers.

### 7. v2 — persistent stdio helper (specced only)

If ~400 ms/tick proves too heavy (very large repos, many WSL projects):

- One long-lived process per distro:
  `wsl.exe -d <d> --exec sh -c '<inline loop>'` — a single POSIX-sh
  loop, no installed artifact, no version skew, dies with the VM.
- Protocol: NUL-delimited request lines on stdin
  (`status <linux_path>`, `discover <linux_path>`, …); responses are
  length-prefixed frames whose payloads are the same porcelain formats
  v1 parses — the parsers and the `Location` dispatch point don't
  change, only the transport behind the WSL arm.
- Lifecycle: spawn on first use, restart on EOF/write failure (VM
  shutdown), kill on app exit. No ports (AF_UNIX doesn't cross the
  WSL2 boundary; hvsockets need admin) — stdio only.

## Out of scope

- Per-distro automount roots (`[automount] root` is set per distro in
  its `/etc/wsl.conf`; the config option is a single global value —
  make it a map only if someone actually runs mixed roots).
- WSL1 distros (registry `Version` 1): treated like WSL2; 9P vs DrvFs
  differences are irrelevant because git runs in-distro either way.
- Per-distro shell/args configuration (`[ui.wsl]`) — the distro's own
  default shell is the contract; add config only when a need appears.
- Distro management (install, start, stop, `--shutdown`).
- File watching / event-driven status for WSL repos (v2 helper could
  host inotify later).
