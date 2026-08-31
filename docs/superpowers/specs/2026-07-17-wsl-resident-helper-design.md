# WSL Resident Helper — Design

Status: approved in brainstorming 2026-07-17. Untracked; never commit.

## Problem

Two independent pains share one root cause: the Windows side has no live view
into a WSL distro.

1. **Dead focus keys in WSL sessions.** The Windows process probe cannot see
   past `wsl.exe`, so `is_wsl_boundary_name` currently assumes a cooperating
   nav TUI is always running and forwards every FocusLeft/FocusRight into the
   distro. In a bare shell or Claude Code session nothing hands focus back —
   the keys appear completely dead.
2. **Per-query `wsl.exe` spawn cost.** Every WSL operation (`run_batch`: git
   status, dirty counts, project discovery, worktree ops, `$HOME` lookup;
   plus `discover_delta` and `gh pr view` for WSL repos) pays a one-shot
   `wsl.exe` spawn: ~400 ms warm, seconds while the VM cold-boots.

## Solution overview

One resident POSIX-sh process per distro, spawned lazily by alacritree and
spoken to over its stdio pipe. It serves three things:

- **RUN** — the existing `run_batch` scripts, verbatim, without a spawn.
- **PROBE** — the foreground process of a WSL session (the same
  tpgid-from-`/proc` check the native Linux build performs), replacing the
  assume-it-cooperates rule for focus passthrough.
- **hello capabilities** — login-shell-resolved paths for `git`, `delta`,
  `gh`, answering `discover_delta` without a spawn.

Nothing is installed inside the distro. The helper script is a string
constant compiled into the binary and passed as an argument to
`wsl.exe --exec sh -c`. A future compiled (musl) helper can replace the
script behind the same wire protocol if sh ever runs out of road; nothing on
the Windows side would change.

## Module layout

- `alacritree/src/wsl_helper.rs` (new) — helper script constants, wire
  protocol codec, `HelperClient`, per-distro client registry, probe cache.
- `alacritree/src/wsl.rs` — `run_batch` goes resident-first;
  `shell_invocation` learns the shim; `discover_delta` consults capabilities
  first. Signatures unchanged for all other callers.
- `alacritree/src/session.rs` — WSL sessions carry `(distro, probe_key)`;
  `nav_tui_running` consults the probe cache for them; the
  `is_wsl_boundary_name` ⇒ `nav_tui = true` rule is deleted (the function
  stays for boundary *detection*).
- `alacritree/src/pr_status.rs` — the WSL branch of `query_gh` becomes a
  batch script through the resident-first path.
- `alacritree/src/config.rs` — new top-level `[wsl]` section:
  `resident_helper` (bool, default `true`), `automount_root` (moved from
  `[ui.wsl]`, old location honored as deprecated fallback).

## Wire protocol

Text frames on the request side, length-prefixed binary on the response side.
All request-side variable content is base64 (standard alphabet, no wrap) so
requests are always single lines; `base64` is assumed present (coreutils and
busybox both ship it).

**Hello** (helper → client, once, first line):

```
hello<TAB>1<TAB>b64(git path or "")<TAB>b64(delta path or "")<TAB>b64(gh path or "")<TAB>b64(runtime dir)
```

`1` is the protocol version; a client seeing an unknown version marks the
helper unusable and falls back to one-shot spawns. Tool paths are resolved
through the user's login shell (`getent passwd` shell, `-lc 'command -v …'`)
so per-user install dirs like `~/.cargo/bin` are honored — same trick as
today's `discover_delta`.

**Requests** (client → helper, one line each):

```
<id><TAB>RUN<TAB>b64(script)<TAB>b64(arg1)<TAB>…
<id><TAB>PROBE<TAB><probe key>
```

`id` is a per-client monotonically increasing integer.

**Responses** (helper → client):

```
<id><TAB><exit code><TAB><payload byte count>\n
<payload bytes, raw>
```

RUN's payload is the script's stdout (stderr is discarded, matching the
guard-with-`|| true` convention the batch scripts already follow). PROBE's
payload is the foreground process's `comm` (empty when unknown). All
responses are written by a single writer process, so frames never interleave
and payloads are binary-safe (NUL-delimited porcelain passes through).

## Helper script

Started as `wsl.exe -d <distro> --exec sh -c <MAIN> sh`. Structure:

1. Emit the hello line.
2. `t=$(mktemp -d)`; `mkfifo "$t/done"`; `trap 'rm -rf "$t"' EXIT`.
3. **Writer** (background): holds the FIFO open read-write, loops reading
   `<id> <exit>` completion lines, emits the response header and streams
   `$t/<id>.out` to stdout, then deletes the temp file. Completion lines are
   well under `PIPE_BUF`, so concurrent jobs' notifications never tear.
4. **Dispatcher** (foreground): loops reading request lines from stdin.
   - RUN: decode script and args, run
     `sh -c "$script" sh "$@" > "$t/$id.out" 2>/dev/null` as a background
     job whose completion line goes to the FIFO. Backgrounding means a slow
     `git diff` never blocks a probe.
   - PROBE: resolved by the dispatcher itself (it is two `/proc` reads):
     read the pidfile for the key, confirm `/proc/<pid>` exists, take
     field 6 after the last `)` of `/proc/<pid>/stat` (tpgid — parsing
     after the last `)` is the standard defense against `comm` values
     containing spaces or parens), read `/proc/<tpgid>/comm`. The answer is
     written to `$t/<id>.out` and its completion line sent to the FIFO like
     any job — every response leaves through the single writer.
   - stdin EOF: exit; the EXIT trap cleans the temp dir, background jobs die
     with the process group.

## Session shim and probe

**Shim.** Sessions whose invocation alacritree constructs itself
(`ShellChoice::Wsl`, auto-by-location for WSL projects) spawn as:

```
wsl.exe -d <distro> --cd <workdir> --exec sh -c '<SHIM>' sh <probe key>
```

```sh
d=${XDG_RUNTIME_DIR:-/tmp}/alacritree
mkdir -p "$d" && printf %s $$ > "$d/session-$1.pid"
s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)
[ -x "$s" ] || s=/bin/sh
exec "$s" -l
```

The `exec` makes the pidfile PID *be* the shell's PID, and `"$s" -l`
preserves the "distro's own default login shell" contract (documented
divergence: wsl.exe's own launch path is replaced by an equivalent
`getent`-resolved login exec).

Profile sessions whose `program` is `wsl.exe` get the shim appended **only
when the profile argv is safely parseable**: every arg is one of
`-d/--distribution <x>`, `--cd <dir>`, with no positional command. (This
covers a bare `program = "wsl.exe"` profile — default distro.) Anything
else — nested `wsl` typed by hand, exotic flags, an explicit command — runs
unmodified and simply probes as unknown.

**Probe key.** Globally unique across alacritree instances, because the
runtime dir is shared: `<windows pid>-<session counter>`. Never the bare
per-instance session counter.

**Probe cache.** `HelperClient` owns a poller thread that refreshes the
foreground `comm` of every registered probe key at the existing 1 s agent
cadence. `Session` registers its key on spawn and unregisters on `Drop`.
`session.rs` reads the cache through a non-blocking
`wsl_helper::foreground_comm(distro, key) -> Option<String>`; the UI thread
never touches the pipe. For a WSL session, `nav_tui_running` is
`is_nav_tui_name(comm)` on the cached value; `None` (helper down, VM cold,
unshimmed session, stale pidfile) means **not a TUI — keys move panel
focus**. The lesser evil is losing passthrough, not losing the keys.

**Cleanup.** Pidfiles are validated against `/proc` on every probe, so stale
files are inert. GC (helper-side, opportunistic on start) removes only
entries whose PID is dead — safe across instances without coordination.

## Call-site integration

- **`run_batch(distro, script, args)`** — signature unchanged. When the
  feature is enabled and the distro's client is up, requests go over the
  pipe; otherwise the existing one-shot spawn runs. All current callers
  (git status, dirty counts, discovery, worktree ops, `$HOME`) accelerate
  with zero call-site changes.
- **Fallback safety rule:** falling back to a one-shot spawn is allowed only
  when the transport failed *before the request was written* (helper down,
  spawn failed, write error). A request that was sent but got no reply
  (timeout, pipe died mid-flight) returns `Err` — never silently re-run,
  because batch scripts can have side effects (worktree add). Timeouts:
  60 s for RUN, 2 s for PROBE.
- **`discover_delta`** — answered from hello capabilities when the client is
  up; existing spawn otherwise. Miss-is-never-cached semantics preserved: a
  `""` capability triggers a live re-check on next demand (helper restart or
  one-shot).
- **PR status** — routing unchanged: Windows repo → Windows `gh`, WSL repo →
  the distro's `gh`. The WSL branch becomes a batch script
  (`cd "$1" && exec gh pr view …` with the `windows_to_linux`-translated
  path) through the resident-first path.
- **Diff rendering** — untouched. delta runs as an interactive pager inside
  its own PTY session; only its discovery step gets faster.

## Lifecycle and failure

- Clients spawn lazily on first use per distro, on a background thread; the
  UI thread never blocks on helper startup. Until the hello arrives, callers
  use one-shot spawns (which pay the same VM cold-boot the helper would).
- Helper death (EOF/broken pipe on the reader thread) marks the client down;
  everything falls back to one-shot; respawn waits a 30 s cooldown so a
  broken distro cannot cause a spawn storm.
- App exit drops the clients, closing stdin — helpers exit via EOF, traps
  clean their temp dirs. No orphan processes, no residue in the distro
  beyond dead pidfiles (GC'd on next helper start).

## Multiple instances

Safe by construction: each instance owns its helpers via private stdio
pipes — no sockets, no named rendezvous. Shared state is only the pidfile
dir, covered by the two rules above (globally unique probe keys;
dead-PID-only GC).

## Config

New top-level `[wsl]` section in `alacritree.toml` (platform integration,
not UI):

```toml
[wsl]
resident_helper = true   # default
automount_root = "/mnt"
```

`resident_helper = false` restores today's behavior exactly: one-shot
spawns everywhere, probe reports unknown — which after this change means
WSL sessions never passthrough (keys always move focus), replacing the
deleted assume-it-cooperates rule.

`automount_root` moves here from `[ui.wsl]` for section coherence; the old
`[ui.wsl]` location is still honored as a fallback (`[wsl]` wins when both
are set) and documented as deprecated in the `Raw*` structs.

## Testing

- **Protocol codec** — pure unit tests: request encoding, hello parsing
  (including unknown version), response frame reassembly from a fake byte
  stream (split reads, binary payloads with NULs), id routing to pending
  requests.
- **Probe decision** — extend the existing `session.rs` name-match tests and
  `app.rs` `focus_move` tests: cached comm `nvim` ⇒ passthrough; `None` ⇒
  focus moves; boundary-without-key ⇒ focus moves.
- **Profile argv parser** — unit tests: bare `wsl.exe`, `-d x`, `--cd y`
  wrappable; positional command or unknown flag ⇒ not wrapped.
- **Live round trips** — `#[ignore]`d tests against the default distro
  mirroring `run_batch_round_trips`: hello capabilities, RUN echo, PROBE of
  a shimmed `sleep` session. Runnable locally and on the WSL kali lab.

## Out of scope (explicit)

- Compiled (musl) helper binary — protocol is its drop-in seam; not built now.
- inotify-based status push, file watching.
- macOS anything.
- Cross-boundary `gh` fallback (Windows repos always use Windows `gh`).
