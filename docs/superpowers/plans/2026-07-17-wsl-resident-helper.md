# WSL Resident Helper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One resident POSIX-sh process per WSL distro, spoken to over stdio, that answers foreground-TUI probes (fixing dead FocusLeft/FocusRight in WSL sessions), runs the existing `run_batch` scripts without a per-call `wsl.exe` spawn, and reports tool paths (`git`/`delta`/`gh`) from its hello line.

**Architecture:** A new `wsl_helper.rs` module owns the wire protocol, the helper/shim script constants, a per-distro `HelperClient` registry, and a probe cache fed by a 1 s poller thread. `wsl::run_batch`, `wsl::discover_delta`, and `pr_status::query_gh`'s WSL branch become resident-first with one-shot fallback. WSL sessions spawn through a shim that publishes the shell PID for probing; `Session::nav_tui_running` consults the probe cache instead of the deleted "wsl.exe ⇒ assume TUI cooperates" rule.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), `std::process` pipes, POSIX sh (dash/busybox-ash compatible), `base64` crate (new dependency), egui/eframe app context.

**Spec:** `docs/superpowers/specs/2026-07-17-wsl-resident-helper-design.md` (untracked — never commit it, nor this plan).

**Workspace:** Execute in a fresh worktree `alacritree-worktrees/feat/wsl-resident-helper` branched off `session-display-and-focus` (`ed98d820`, PR #103's branch — the helper builds on its focus/probe code and becomes its own PR after the user merges the stack down; do not push or open a PR unprompted). All file paths below are relative to that worktree root. Line numbers were captured on `integration/all-features` at `cb9a6eaf` and may drift on this base; anchor on the named symbols, not the numbers.

## Global Constraints

- Only the `alacritree/` crate changes. `alacritty/`, `alacritty_terminal/`, `alacritty_config*` are read-only vendored code.
- Helper and shim scripts are POSIX sh only — no bashisms; they must run under dash and busybox ash. `base64` (coreutils/busybox) is assumed present.
- Wire protocol version is the string `"1"`. A client seeing any other version marks the helper unusable (one-shot fallback).
- Timeouts, verbatim from the spec: **60 s** for RUN, **2 s** for PROBE, **30 s** respawn cooldown after helper death, **1 s** probe poll cadence.
- Fallback safety rule: fall back to a one-shot `wsl.exe` spawn **only** when the transport failed *before the request was written*. A request that was sent but got no reply returns `Err` — never silently re-run (batch scripts have side effects, e.g. worktree add).
- Probe unknown (helper down, VM cold, unshimmed session, stale pidfile) means **not a TUI — FocusLeft/FocusRight move panel focus**.
- Probe keys are globally unique across alacritree instances: `<windows pid>-<counter>`. Pidfile GC removes only entries whose PID is dead.
- Config: new top-level `[wsl]` section — `resident_helper = true` (default), `automount_root = "/mnt"` (moved from `[ui.wsl]`; old location still honored as deprecated fallback, `[wsl]` wins when both are set). `resident_helper = false` restores today's one-shot behavior exactly, with probes reporting unknown.
- `client()` / `run_batch` / anything touching the helper pipe never runs on the UI thread. The UI thread only reads the probe cache (`foreground_comm`, non-blocking).
- Never run the built GUI and never kill running `alacritree.exe` processes. The release exe is file-locked by running instances — the user rebuilds.
- `cargo fmt` before every commit. Test suite: `cargo test -p alacritree` (green at the base commit; must stay green).
- Commits: Conventional Commits, imperative, lowercase after the colon, subject ≤ 72 chars, each ending with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. Stage files explicitly — never `git add -A` or `git add .`.
- Comments explain *why*, never *what*; no PR/task/change-relative phrasing. Match the file-header comment style of `wsl.rs` / `session.rs`.
- Never commit anything under `docs/superpowers/`.

## Codebase primer (for implementers with zero context)

- `alacritree/src/wsl.rs` — existing WSL subsystem. Key items you build on:
  - `pub fn command(distro: &str, cd: Option<&Path>) -> Command` (line ~263): `wsl.exe -d <d> [--cd <dir>] --exec`, hidden console, `WSL_UTF8=1`.
  - `pub fn run_batch(distro, script, args) -> Result<Vec<u8>, String>` (~305): one-shot `sh -c <script> sh <args...>`; error only when the process hard-fails with empty stdout.
  - `pub fn discover_delta(distro) -> Option<String>` (~334): login-shell `command -v delta`; a miss is never cached.
  - `pub fn shell_invocation(distro, workdir)` (~282): `wsl.exe -d <d> --cd <dir>` with **no** `--exec` (distro's default login shell).
  - `pub fn distros() -> Vec<WslDistro>` (~187): cached registry/CLI enumeration; `WslDistro { name, is_default }`.
  - `Location::{Windows(PathBuf), Wsl { distro, linux_path }}` from `classify(path)`.
- `alacritree/src/session.rs` — `Session::spawn` → `spawn_with` creates the PTY; `shell_pid` captured; `process_probe()` (~778) caches `(agent_glyph, foreground_job, nav_tui)` at `AGENT_CACHE_TTL` (1 s); `is_nav_tui_name` (~192) matches `nvim`/`vim`/`tmux` prefixes; `is_wsl_boundary_name` (~203) currently forces `nav_tui = true` for any `wsl*` image name inside `windows_process_probe::probe` (~517) — that clause is the bug this feature fixes.
- `alacritree/src/app.rs` — `resolve_shell` (~809) decides ConfigShell / WslDistro / Profile via `shell_decision` (~3177); `wsl_shell` (~3156) builds the WSL argv; `profile_shell` (~3209); `spawn_session_with_shell` (~745); `spawn_profile_session` (~792); `wsl_delta_path` (~3016) backgrounds `discover_delta`; `focus_move` (~225) already takes a `tui_running: bool` and needs no change.
- `alacritree/src/pr_status.rs` — `query_gh` (~158): Windows repo → Windows `gh`; WSL repo → `wsl::command` + `gh`.
- `alacritree/src/config.rs` — `RawUi` (~997) holds `wsl: RawUiWsl` (`automount_root`); `RawConfig::into_config` (~1270) normalizes it into `Config::wsl_automount_root`; `Config::default` sets `"/mnt"` (~453).
- `alacritree/src/main.rs` — module list (~3-36); `wsl::set_automount_root(...)` call (~88).

## File structure

- **Create** `alacritree/src/wsl_helper.rs` — wire protocol codec, helper/shim script constants, profile-argv wrapper, `HelperClient`, per-distro registry, probe cache + poller. Single new file: everything that knows the resident protocol exists lives here, mirroring how `wsl.rs` is the only module that knows WSL exists.
- **Modify** `alacritree/src/wsl.rs` — `run_batch` resident-first; `discover_delta` capability-first.
- **Modify** `alacritree/src/session.rs` — `Session` carries `Option<WslProbe>`; probe registration/unregistration; `nav_tui` from the probe cache for WSL sessions; delete the boundary-assume rule.
- **Modify** `alacritree/src/app.rs` — shim invocation for WSL/auto sessions, wrap for parseable `wsl.exe` profiles and a parseable `[terminal.shell] program = "wsl.exe"`.
- **Modify** `alacritree/src/pr_status.rs` — WSL branch of `query_gh` becomes a batch script.
- **Modify** `alacritree/src/config.rs`, `alacritree/src/main.rs`, `alacritree/Cargo.toml`.

---

### Task 1: Wire-protocol codec

**Files:**
- Create: `alacritree/src/wsl_helper.rs`
- Modify: `alacritree/src/main.rs` (module list)
- Modify: `alacritree/Cargo.toml` (add `base64`)
- Test: inline `#[cfg(test)] mod tests` in `wsl_helper.rs`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces (later tasks rely on these exact names):
  - `pub const PROTOCOL_VERSION: &str = "1"`
  - `pub struct Capabilities { pub git: Option<String>, pub delta: Option<String>, pub gh: Option<String>, pub runtime_dir: String }`
  - `pub fn parse_hello(line: &str) -> Option<Capabilities>`
  - `pub fn encode_run(id: u64, script: &str, args: &[&str]) -> String`
  - `pub fn encode_probe(id: u64, key: &str) -> String`
  - `pub struct Frame { pub id: u64, pub exit: i32, pub payload: Vec<u8> }`
  - `pub struct FrameReader` with `pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, String>`

- [ ] **Step 1: Add the base64 dependency**

In `alacritree/Cargo.toml`, under `[dependencies]`, add:

```toml
base64 = "0.22"
```

- [ ] **Step 2: Register the module**

In `alacritree/src/main.rs`, the module list is alphabetized (`mod app;` … `mod wsl;`). Add after `mod wsl;`:

```rust
mod wsl_helper;
```

Create `alacritree/src/wsl_helper.rs` with only a module doc for now:

```rust
//! Resident WSL helper: one long-lived `sh` per distro, spoken to over its
//! stdio pipe, serving the batch scripts (`RUN`), the foreground-process
//! probe (`PROBE`), and tool paths (the hello line) without a per-call
//! `wsl.exe` spawn.  The wire protocol is the seam a future compiled helper
//! would slot behind; nothing outside this module knows it exists.
```

- [ ] **Step 3: Write the failing codec tests**

Append to `wsl_helper.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_request_encodes_base64_fields() {
        let line = encode_run(7, r#"printf %s "$1""#, &["hello"]);
        assert_eq!(line, "7\tRUN\tcHJpbnRmICVzICIkMSI=\taGVsbG8=\n");
    }

    #[test]
    fn empty_arg_encodes_as_dash() {
        // Tab is IFS whitespace in sh, so an empty field would be collapsed
        // away by the dispatcher's field splitting.
        let line = encode_run(1, "s", &["", "x"]);
        assert_eq!(line, "1\tRUN\tcw==\t-\teA==\n");
    }

    #[test]
    fn probe_request_is_plain() {
        assert_eq!(encode_probe(3, "1234-7"), "3\tPROBE\t1234-7\n");
    }

    #[test]
    fn parses_hello_with_missing_tools() {
        // git and runtime dir present, delta and gh absent (empty fields).
        let line = "hello\t1\tL3Vzci9iaW4vZ2l0\t\t\tL3J1bi91c2VyLzEwMDAvYWxhY3JpdHJlZQ==\n";
        let caps = parse_hello(line).unwrap();
        assert_eq!(caps.git.as_deref(), Some("/usr/bin/git"));
        assert_eq!(caps.delta, None);
        assert_eq!(caps.gh, None);
        assert_eq!(caps.runtime_dir, "/run/user/1000/alacritree");
    }

    #[test]
    fn rejects_unknown_hello_version() {
        assert!(parse_hello("hello\t2\t\t\t\t\n").is_none());
        assert!(parse_hello("goodbye\t1\t\t\t\t\n").is_none());
        assert!(parse_hello("hello\t1\t\t\n").is_none());
    }

    #[test]
    fn reassembles_frames_across_split_reads() {
        let mut stream = Vec::new();
        stream.extend_from_slice(b"4\t0\t5\nhello");
        stream.extend_from_slice(b"9\t1\t0\n");
        let mut reader = FrameReader::default();
        let mut frames = Vec::new();
        // Byte-at-a-time is the worst case a pipe can deliver.
        for byte in stream {
            frames.extend(reader.push(&[byte]).unwrap());
        }
        assert_eq!(
            frames,
            vec![
                Frame { id: 4, exit: 0, payload: b"hello".to_vec() },
                Frame { id: 9, exit: 1, payload: Vec::new() },
            ]
        );
    }

    #[test]
    fn payload_bytes_are_binary_safe() {
        // NUL-delimited git porcelain, tabs, and newlines all pass through:
        // the header's byte count is the only framing.
        let payload = b"a\0b\tc\nd";
        let mut stream = format!("1\t0\t{}\n", payload.len()).into_bytes();
        stream.extend_from_slice(payload);
        let frames = FrameReader::default().push(&stream).unwrap();
        assert_eq!(frames, vec![Frame { id: 1, exit: 0, payload: payload.to_vec() }]);
    }

    #[test]
    fn malformed_header_is_a_protocol_error() {
        assert!(FrameReader::default().push(b"not a header\n").is_err());
        assert!(FrameReader::default().push(b"1\t0\n").is_err());
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p alacritree wsl_helper::`
Expected: **compile error** — `encode_run`, `parse_hello`, `FrameReader`, `Frame` not found. A compile failure of the test module is the RED here.

- [ ] **Step 5: Implement the codec**

Add above the test module:

```rust
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

/// Bumped only when the request/response framing changes incompatibly; a
/// client seeing any other version treats the helper as unusable and stays
/// on one-shot spawns.
pub const PROTOCOL_VERSION: &str = "1";

/// Login-shell-resolved tool paths and the distro-side runtime dir, from
/// the helper's hello line.  `None` means the tool wasn't on the login
/// shell's PATH at helper start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub git: Option<String>,
    pub delta: Option<String>,
    pub gh: Option<String>,
    pub runtime_dir: String,
}

/// A request-side payload field: base64, or `-` for the empty payload.
/// Tab is IFS whitespace in sh, so an empty field would be collapsed away
/// by the dispatcher's field splitting; base64 can never produce a bare
/// `-`, so the encodings stay disjoint.
fn encode_field(payload: &str) -> String {
    if payload.is_empty() { "-".to_string() } else { B64.encode(payload) }
}

pub fn encode_run(id: u64, script: &str, args: &[&str]) -> String {
    let mut line = format!("{id}\tRUN\t{}", encode_field(script));
    for arg in args {
        line.push('\t');
        line.push_str(&encode_field(arg));
    }
    line.push('\n');
    line
}

pub fn encode_probe(id: u64, key: &str) -> String {
    format!("{id}\tPROBE\t{key}\n")
}

pub fn parse_hello(line: &str) -> Option<Capabilities> {
    // Strip only line terminators — trim_end() would also eat the tab
    // before a legitimately empty trailing field.
    let mut fields = line.trim_end_matches(['\r', '\n']).split('\t');
    if fields.next()? != "hello" || fields.next()? != PROTOCOL_VERSION {
        return None;
    }
    let mut decode = || -> Option<String> {
        let raw = B64.decode(fields.next()?).ok()?;
        Some(String::from_utf8_lossy(&raw).trim().to_string())
    };
    let git = decode()?;
    let delta = decode()?;
    let gh = decode()?;
    let runtime_dir = decode()?;
    let some = |s: String| (!s.is_empty()).then_some(s);
    Some(Capabilities { git: some(git), delta: some(delta), gh: some(gh), runtime_dir })
}

/// One response off the helper's stdout: `<id>\t<exit>\t<len>\n` followed
/// by exactly `len` raw payload bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u64,
    pub exit: i32,
    pub payload: Vec<u8>,
}

/// Incremental response parser fed arbitrary read chunks; complete frames
/// come out as they close.  A malformed header is unrecoverable (the byte
/// count is the only framing, so there is no resync point) and surfaces as
/// an error for the caller to tear the client down on.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, String> {
        self.buf.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let Some(newline) = self.buf.iter().position(|&b| b == b'\n') else {
                return Ok(frames);
            };
            let Some((id, exit, len)) = parse_header(&self.buf[..newline]) else {
                return Err(format!(
                    "malformed helper frame header: {:?}",
                    String::from_utf8_lossy(&self.buf[..newline])
                ));
            };
            let frame_end = newline + 1 + len;
            if self.buf.len() < frame_end {
                return Ok(frames);
            }
            frames.push(Frame { id, exit, payload: self.buf[newline + 1..frame_end].to_vec() });
            self.buf.drain(..frame_end);
        }
    }
}

fn parse_header(line: &[u8]) -> Option<(u64, i32, usize)> {
    let text = std::str::from_utf8(line).ok()?;
    let mut fields = text.trim_end_matches('\r').split('\t');
    // `wc -c` output may carry leading blanks on some implementations.
    let id = fields.next()?.trim().parse().ok()?;
    let exit = fields.next()?.trim().parse().ok()?;
    let len = fields.next()?.trim().parse().ok()?;
    fields.next().is_none().then_some((id, exit, len))
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p alacritree wsl_helper::`
Expected: 8 tests PASS. `cargo check -p alacritree` may emit dead-code warnings for the codec items until Tasks 3-7 wire them — expected; do not add `allow` attributes, just note it in the task report.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/wsl_helper.rs alacritree/src/main.rs alacritree/Cargo.toml Cargo.lock
git commit -m "feat(wsl): add resident-helper wire protocol codec" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Helper script, session shim, and profile-argv wrapper

**Files:**
- Modify: `alacritree/src/wsl_helper.rs`
- Test: inline tests in the same file

**Interfaces:**
- Consumes: `SHIM_SCRIPT` is self-contained; nothing from Task 1 at runtime.
- Produces:
  - `pub(crate) const HELPER_SCRIPT: &str`
  - `pub(crate) const SHIM_SCRIPT: &str`
  - `pub fn shim_invocation(distro: &str, workdir: &Path, probe_key: &str) -> (String, Vec<String>)`
  - `pub fn wrap_profile_argv(program: &str, args: &[String], probe_key: &str) -> Option<(Vec<String>, Option<String>)>` — rewritten argv plus the explicit distro (`None` = default distro)

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module:

```rust
use std::path::Path;

#[test]
fn shim_invocation_builds_expected_argv() {
    let (program, args) = shim_invocation("kali-linux", Path::new(r"C:\proj"), "1234-1");
    assert_eq!(program, "wsl.exe");
    assert_eq!(
        args,
        vec![
            "-d",
            "kali-linux",
            "--cd",
            r"C:\proj",
            "--exec",
            "sh",
            "-c",
            SHIM_SCRIPT,
            "sh",
            "1234-1",
        ]
    );
}

#[test]
fn wraps_bare_wsl_profile_for_default_distro() {
    let (args, distro) = wrap_profile_argv("wsl.exe", &[], "1234-2").unwrap();
    assert_eq!(distro, None);
    assert_eq!(args, vec!["--exec", "sh", "-c", SHIM_SCRIPT, "sh", "1234-2"]);
}

#[test]
fn wraps_distro_and_cd_flags() {
    let profile_args: Vec<String> =
        ["-d", "kali-linux", "--cd", "/home"].iter().map(|s| s.to_string()).collect();
    let (args, distro) = wrap_profile_argv(r"C:\Windows\System32\wsl.exe", &profile_args, "9-9").unwrap();
    assert_eq!(distro.as_deref(), Some("kali-linux"));
    assert_eq!(
        args,
        vec!["-d", "kali-linux", "--cd", "/home", "--exec", "sh", "-c", SHIM_SCRIPT, "sh", "9-9"]
    );
}

#[test]
fn refuses_unparseable_profiles() {
    let to_vec = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // A positional command, an unknown flag, or a dangling value-flag may
    // not be a plain login shell — leave it alone (probes as unknown).
    assert!(wrap_profile_argv("wsl.exe", &to_vec(&["bash"]), "k").is_none());
    assert!(wrap_profile_argv("wsl.exe", &to_vec(&["-d", "kali", "htop"]), "k").is_none());
    assert!(wrap_profile_argv("wsl.exe", &to_vec(&["--exec", "sh"]), "k").is_none());
    assert!(wrap_profile_argv("wsl.exe", &to_vec(&["-d"]), "k").is_none());
    assert!(wrap_profile_argv("pwsh.exe", &[], "k").is_none());
    assert!(wrap_profile_argv("wslhost.exe", &[], "k").is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree wsl_helper::`
Expected: compile error — `shim_invocation`, `wrap_profile_argv`, `SHIM_SCRIPT` not found.

- [ ] **Step 3: Implement the scripts and wrapper**

Add to `wsl_helper.rs` (after the codec, before the client code Task 3 will add). Note the raw-string delimiter is `r##`…`##` because the scripts contain `#` next to `"`:

```rust
use std::path::Path;

/// The distro-side helper, passed verbatim as the single argument of
/// `wsl.exe --exec sh -c`.  POSIX sh only — dash and busybox ash both run
/// it.  Shape: capability hello, dead-pidfile GC, a background writer that
/// owns stdout, then the request dispatcher on stdin.  Responses all leave
/// through the writer, whose FIFO completion lines are far under PIPE_BUF,
/// so concurrent jobs never interleave frames.  Commentary lives here, not
/// in the script, so every byte shipped into the distro earns its keep.
///
/// Empty request fields arrive as `-` (see `encode_field`); decoded args
/// lose trailing newlines to command substitution, which no current caller
/// passes.  Stdin EOF ends the dispatcher; the EXIT trap removes the temp
/// dir and `kill 0` takes the writer and any in-flight jobs down with the
/// process group, so a job can never deadlock on the deleted FIFO.
pub(crate) const HELPER_SCRIPT: &str = r##"
set -u
b64() { printf %s "$1" | base64 | tr -d '\n'; }
s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)
[ -x "$s" ] || s=${SHELL:-/bin/sh}
caps=$("$s" -lc 'command -v git || echo; command -v delta || echo; command -v gh || echo' 2>/dev/null)
rt=${XDG_RUNTIME_DIR:-/tmp}/alacritree
printf 'hello\t1\t%s\t%s\t%s\t%s\n' \
  "$(b64 "$(printf %s "$caps" | sed -n 1p)")" \
  "$(b64 "$(printf %s "$caps" | sed -n 2p)")" \
  "$(b64 "$(printf %s "$caps" | sed -n 3p)")" \
  "$(b64 "$rt")"
mkdir -p "$rt" 2>/dev/null
for f in "$rt"/session-*.pid; do
  [ -e "$f" ] || continue
  p=$(cat "$f" 2>/dev/null)
  case $p in ''|*[!0-9]*) rm -f "$f"; continue;; esac
  [ -d "/proc/$p" ] || rm -f "$f"
done
t=$(mktemp -d) || exit 1
mkfifo "$t/done" || exit 1
trap 'rm -rf "$t"; kill 0 2>/dev/null' EXIT
(
  exec 3<>"$t/done"
  while read -r id code <&3; do
    out="$t/$id.out"
    n=$(wc -c < "$out" 2>/dev/null) || n=0
    printf '%s\t%s\t%s\n' "$id" "$code" "${n:-0}"
    cat "$out" 2>/dev/null
    rm -f "$out"
  done
) &
TAB=$(printf '\t')
while IFS=$TAB read -r id kind rest; do
  case $kind in
  RUN)
    (
      script=
      set --
      first=1
      line=$rest
      while [ -n "$line" ]; do
        case $line in
        *"$TAB"*) field=${line%%"$TAB"*}; line=${line#*"$TAB"} ;;
        *) field=$line; line= ;;
        esac
        if [ "$field" = - ]; then dec=; else dec=$(printf %s "$field" | base64 -d 2>/dev/null); fi
        if [ "$first" = 1 ]; then script=$dec; first=0; else set -- "$@" "$dec"; fi
      done
      sh -c "$script" sh "$@" > "$t/$id.out" 2>/dev/null
      printf '%s %s\n' "$id" "$?" >> "$t/done"
    ) &
    ;;
  PROBE)
    comm=
    p=$(cat "$rt/session-$rest.pid" 2>/dev/null)
    case $p in ''|*[!0-9]*) p= ;; esac
    if [ -n "$p" ] && [ -d "/proc/$p" ]; then
      stat=$(cat "/proc/$p/stat" 2>/dev/null)
      after=${stat##*')'}
      set -- $after
      tpgid=${6:-}
      case $tpgid in ''|*[!0-9]*) tpgid= ;; esac
      [ -n "$tpgid" ] && comm=$(cat "/proc/$tpgid/comm" 2>/dev/null)
    fi
    printf %s "$comm" > "$t/$id.out"
    printf '%s 0\n' "$id" >> "$t/done"
    ;;
  esac
done
"##;

/// Login-shell shim for shimmed WSL sessions: publish the shell's PID under
/// the probe key, then become the user's login shell.  `exec` makes the
/// pidfile PID *be* the shell, so the helper's tpgid walk starts from the
/// right place.  wsl.exe's own no-`--exec` launch would start the login
/// shell too but gives no way to learn its PID; re-resolving through
/// `getent` is the documented divergence, with `/bin/sh` only as a last
/// resort.  Single line: it travels through ConPTY command-line quoting.
pub(crate) const SHIM_SCRIPT: &str = r##"d=${XDG_RUNTIME_DIR:-/tmp}/alacritree; mkdir -p "$d" 2>/dev/null && printf %s $$ > "$d/session-$1.pid"; s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7); [ -x "$s" ] || s=/bin/sh; exec "$s" -l"##;

/// argv for a session alacritree constructs itself (`ShellChoice::Wsl`,
/// auto-by-location): the shim with the probe key as `$1`.
pub fn shim_invocation(distro: &str, workdir: &Path, probe_key: &str) -> (String, Vec<String>) {
    (
        "wsl.exe".to_string(),
        vec![
            "-d".to_string(),
            distro.to_string(),
            "--cd".to_string(),
            workdir.to_string_lossy().into_owned(),
            "--exec".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            SHIM_SCRIPT.to_string(),
            "sh".to_string(),
            probe_key.to_string(),
        ],
    )
}

/// Probe-key shim for a `[[ui.profiles]]` entry that launches wsl.exe.
/// Only argv this parser fully understands is wrapped: any mix of
/// `-d`/`--distribution <distro>` and `--cd <dir>`, nothing else.  An
/// unknown flag or a positional command may not be a plain login shell —
/// it runs unmodified and simply probes as unknown.  Returns the rewritten
/// argv plus the explicit distro (`None` = the default distro; the caller
/// resolves it, since only `wsl::distros` knows which that is).
pub fn wrap_profile_argv(
    program: &str,
    args: &[String],
    probe_key: &str,
) -> Option<(Vec<String>, Option<String>)> {
    let stem = Path::new(program).file_stem()?.to_str()?;
    if !stem.eq_ignore_ascii_case("wsl") {
        return None;
    }
    let mut distro = None;
    let mut wrapped = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-d" | "--distribution" => {
                let name = it.next()?;
                distro = Some(name.clone());
                wrapped.push(arg.clone());
                wrapped.push(name.clone());
            },
            "--cd" => {
                let dir = it.next()?;
                wrapped.push(arg.clone());
                wrapped.push(dir.clone());
            },
            _ => return None,
        }
    }
    wrapped.extend([
        "--exec".to_string(),
        "sh".to_string(),
        "-c".to_string(),
        SHIM_SCRIPT.to_string(),
        "sh".to_string(),
        probe_key.to_string(),
    ]);
    Some((wrapped, distro))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree wsl_helper::`
Expected: all Task 1 + Task 2 tests PASS (13 total).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/wsl_helper.rs
git commit -m "feat(wsl): add resident helper and session shim scripts" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: HelperClient, registry, and lifecycle

**Files:**
- Modify: `alacritree/src/wsl_helper.rs`
- Test: inline tests + one `#[ignore]`d live test

**Interfaces:**
- Consumes: Task 1 codec, Task 2 `HELPER_SCRIPT`; `crate::wsl::command`, `crate::wsl::distros`.
- Produces:
  - `pub enum TransportError { NotWritten(String), NoReply(String) }` (derives `Debug`)
  - `pub struct HelperClient` with `pub fn run(&self, script: &str, args: &[&str]) -> Result<(i32, Vec<u8>), TransportError>`, `pub fn probe(&self, key: &str) -> Result<Option<String>, TransportError>`, `pub fn capabilities(&self) -> Option<&Capabilities>`
  - `pub fn client(distro: &str) -> Option<Arc<HelperClient>>` — **never call on the UI thread**
  - `pub fn set_enabled(enabled: bool)` / `pub fn enabled() -> bool` (default **true**)
  - `pub fn try_run(distro: &str, script: &str, args: &[&str]) -> Option<Result<Vec<u8>, String>>`
  - `pub fn capability_delta(distro: &str) -> Option<String>` / `pub fn capability_gh(distro: &str) -> Option<String>`

- [ ] **Step 1: Write the failing live round-trip test**

Append to the `tests` module:

```rust
/// Live round trip against the default distro.  Requires WSL; run
/// manually: `cargo test -p alacritree wsl_helper:: -- --ignored`
#[test]
#[ignore]
fn helper_round_trips() {
    use std::time::{Duration, Instant};

    let distro =
        crate::wsl::distros().into_iter().find(|d| d.is_default).expect("a default distro");
    // Cold VM boot can take a while; the client comes up asynchronously.
    let deadline = Instant::now() + Duration::from_secs(120);
    let client = loop {
        if let Some(c) = client(&distro.name) {
            break c;
        }
        assert!(Instant::now() < deadline, "helper never became ready");
        std::thread::sleep(Duration::from_millis(200));
    };

    let caps = client.capabilities().expect("capabilities after ready");
    assert!(caps.git.is_some(), "test distros are expected to have git");
    assert!(caps.runtime_dir.ends_with("/alacritree"));

    let (exit, out) = client.run(r#"printf '%s' "$1""#, &["hello"]).expect("run");
    assert_eq!((exit, out.as_slice()), (0, &b"hello"[..]));

    // Empty args survive the `-` field encoding.
    let (_, out) = client.run(r#"printf '[%s][%s]' "$1" "$2""#, &["", "x"]).expect("run");
    assert_eq!(out, b"[][x]");

    // Payloads are binary-safe end to end.
    let (_, out) = client.run(r#"printf 'a\0b'"#, &[]).expect("run");
    assert_eq!(out, b"a\0b");

    // Concurrent jobs multiplex on one pipe without cross-talk.
    let slow = std::thread::spawn({
        let client = client.clone();
        move || client.run("sleep 1; printf slow", &[]).expect("slow run")
    });
    let (_, fast) = client.run("printf fast", &[]).expect("fast run");
    assert_eq!(fast, b"fast");
    assert_eq!(slow.join().unwrap().1, b"slow");

    // An unregistered probe key resolves to "no foreground comm".
    assert_eq!(client.probe("999999-999999").expect("probe"), None);
}
```

- [ ] **Step 2: Verify it fails to compile**

Run: `cargo test -p alacritree wsl_helper:: --no-run`
Expected: compile error — `client`, `HelperClient` not found.

- [ ] **Step 3: Implement the client and registry**

Add to `wsl_helper.rs`:

```rust
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, mpsc};
use std::time::{Duration, Instant};

use crate::wsl;

/// Batch scripts can legitimately run long (worktree add on a cold cache);
/// probes are two `/proc` reads and only ever gate a keypress decision.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// A broken distro must not cause a spawn storm.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(30);

static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Why a request produced no result — the distinction the fallback rule
/// keys on.  `NotWritten` never reached the helper and is safe to re-run
/// as a one-shot; `NoReply` was written and may have executed (batch
/// scripts have side effects), so it must surface as an error, never a
/// silent retry.
#[derive(Debug)]
pub enum TransportError {
    NotWritten(String),
    NoReply(String),
}

pub struct HelperClient {
    distro: String,
    stdin: Mutex<Option<std::process::ChildStdin>>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Frame>>>,
    next_id: AtomicU64,
    capabilities: OnceLock<Capabilities>,
    down: AtomicBool,
}

fn lock<'a, T>(mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl HelperClient {
    /// Spawn the helper for `distro`.  Returns once the process launch is
    /// attempted; readiness (the hello line) arrives asynchronously on the
    /// reader thread.  Failures leave the client marked down so the
    /// registry's cooldown sees them like any other death.
    fn spawn(distro: &str) -> Arc<Self> {
        let client = Arc::new(Self {
            distro: distro.to_string(),
            stdin: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            capabilities: OnceLock::new(),
            down: AtomicBool::new(false),
        });
        let mut child = match wsl::command(distro, None)
            .arg("sh")
            .arg("-c")
            .arg(HELPER_SCRIPT)
            .arg("sh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                client.mark_down(&format!("failed to spawn: {e}"));
                return client;
            },
        };
        *lock(&client.stdin) = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout piped above");
        let reader = client.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("wsl-helper-{distro}"))
            .spawn(move || {
                reader.read_loop(stdout);
                // Stdin is closed by mark_down; reap so a dead helper never
                // lingers as a zombie in the process table.
                let _ = child.wait();
            });
        if let Err(e) = spawned {
            client.mark_down(&format!("failed to start reader thread: {e}"));
        }
        client
    }

    fn read_loop(&self, stdout: std::process::ChildStdout) {
        let mut reader = std::io::BufReader::new(stdout);
        let mut hello = String::new();
        match reader.read_line(&mut hello) {
            Ok(n) if n > 0 => {},
            _ => return self.mark_down("exited before hello"),
        }
        let Some(caps) = parse_hello(&hello) else {
            return self.mark_down("unusable hello (unknown protocol version?)");
        };
        let _ = self.capabilities.set(caps);
        let mut frames = FrameReader::default();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => return self.mark_down("closed its pipe"),
                Err(e) => return self.mark_down(&format!("read failed: {e}")),
                Ok(n) => match frames.push(&chunk[..n]) {
                    Ok(done) => {
                        for frame in done {
                            if let Some(tx) = lock(&self.pending).remove(&frame.id) {
                                let _ = tx.send(frame);
                            }
                        }
                    },
                    Err(e) => return self.mark_down(&e),
                },
            }
        }
    }

    fn mark_down(&self, why: &str) {
        if !self.down.swap(true, Ordering::AcqRel) {
            log::warn!(
                "wsl helper for {}: {why}; falling back to one-shot spawns",
                self.distro
            );
        }
        // Closing stdin EOFs the helper, which cleans up and exits.
        *lock(&self.stdin) = None;
        // Waiters whose request was already written see the hangup as a
        // dropped sender — NoReply, never a retry.
        lock(&self.pending).clear();
    }

    fn is_down(&self) -> bool {
        self.down.load(Ordering::Acquire)
    }

    fn is_ready(&self) -> bool {
        !self.is_down() && self.capabilities.get().is_some()
    }

    pub fn capabilities(&self) -> Option<&Capabilities> {
        self.capabilities.get()
    }

    fn request(&self, id: u64, line: String, timeout: Duration) -> Result<Frame, TransportError> {
        if !self.is_ready() {
            return Err(TransportError::NotWritten("helper not ready".to_string()));
        }
        let (tx, rx) = mpsc::channel();
        lock(&self.pending).insert(id, tx);
        let write = {
            let mut guard = lock(&self.stdin);
            match guard.as_mut() {
                None => Err("helper stdin closed".to_string()),
                Some(stdin) => stdin
                    .write_all(line.as_bytes())
                    .and_then(|()| stdin.flush())
                    .map_err(|e| e.to_string()),
            }
        };
        if let Err(e) = write {
            lock(&self.pending).remove(&id);
            // A partial line has no terminating newline, so the dispatcher
            // can never have run it — NotWritten is safe.
            self.mark_down(&format!("write failed: {e}"));
            return Err(TransportError::NotWritten(e));
        }
        match rx.recv_timeout(timeout) {
            Ok(frame) => Ok(frame),
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err(TransportError::NoReply(format!(
                    "no reply from the {} helper",
                    self.distro
                )))
            },
        }
    }

    pub fn run(&self, script: &str, args: &[&str]) -> Result<(i32, Vec<u8>), TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = self.request(id, encode_run(id, script, args), RUN_TIMEOUT)?;
        Ok((frame.exit, frame.payload))
    }

    pub fn probe(&self, key: &str) -> Result<Option<String>, TransportError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = self.request(id, encode_probe(id, key), PROBE_TIMEOUT)?;
        let comm = String::from_utf8_lossy(&frame.payload).trim().to_string();
        Ok((!comm.is_empty()).then_some(comm))
    }
}

enum Slot {
    Live(Arc<HelperClient>),
    Cooldown(Instant),
}

fn registry() -> &'static Mutex<HashMap<String, Slot>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The ready client for `distro`, spawning one when none exists.  `None`
/// while disabled, still starting, or cooling down after a death — callers
/// fall back to one-shot spawns, which pay the same cold-boot cost the
/// helper would.  Spawning happens under the registry lock but is only a
/// process launch; the slow part (the hello) lands on the reader thread.
/// Never call on the UI thread — same rule as `wsl::run_batch`.
pub fn client(distro: &str) -> Option<Arc<HelperClient>> {
    if !enabled() || !cfg!(windows) {
        return None;
    }
    let mut reg = lock(registry());
    match reg.get(distro) {
        Some(Slot::Live(c)) if c.is_ready() => return Some(c.clone()),
        Some(Slot::Live(c)) if !c.is_down() => return None,
        Some(Slot::Live(_)) => {
            reg.insert(distro.to_string(), Slot::Cooldown(Instant::now()));
            return None;
        },
        Some(Slot::Cooldown(since)) if since.elapsed() < RESPAWN_COOLDOWN => return None,
        _ => {},
    }
    reg.insert(distro.to_string(), Slot::Live(HelperClient::spawn(distro)));
    None
}

/// Resident-first transport for `wsl::run_batch`.  `None` = helper
/// unavailable before anything was sent (fall back to a one-shot spawn);
/// `Some(Err)` = sent but unanswered, which must not be retried;
/// `Some(Ok)` = script stdout, one-shot-compatible.
pub fn try_run(distro: &str, script: &str, args: &[&str]) -> Option<Result<Vec<u8>, String>> {
    let client = client(distro)?;
    match client.run(script, args) {
        Ok((exit, stdout)) => {
            // Mirror one-shot semantics: guarded scripts always emit their
            // sections, so hard failure with silence means the script
            // itself refused.
            if exit != 0 && stdout.is_empty() {
                Some(Err(format!("wsl helper script exited {exit}")))
            } else {
                Some(Ok(stdout))
            }
        },
        Err(TransportError::NotWritten(_)) => None,
        Err(TransportError::NoReply(e)) => Some(Err(e)),
    }
}

pub fn capability_delta(distro: &str) -> Option<String> {
    client(distro)?.capabilities()?.delta.clone()
}

pub fn capability_gh(distro: &str) -> Option<String> {
    client(distro)?.capabilities()?.gh.clone()
}
```

- [ ] **Step 4: Verify compilation and unit tests**

Run: `cargo test -p alacritree wsl_helper::`
Expected: the 13 codec/script tests PASS; `helper_round_trips` is ignored.

- [ ] **Step 5: Run the live test (skip if no WSL on the build machine)**

Run: `cargo test -p alacritree wsl_helper::helper_round_trips -- --ignored`
Expected: PASS (allow ~30 s+ on a cold VM). If this machine has no WSL, note that in the task report and rely on the user's WSL kali lab later — do not delete or weaken the test.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/wsl_helper.rs
git commit -m "feat(wsl): add resident helper client and registry" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Probe cache, poller, and probe keys

**Files:**
- Modify: `alacritree/src/wsl_helper.rs`
- Test: inline tests

**Interfaces:**
- Consumes: Task 3 `client()` and `HelperClient::probe`.
- Produces:
  - `pub struct WslProbe { pub distro: String, pub key: String }` (derives `Debug, Clone`)
  - `pub fn new_probe_key() -> String`
  - `pub fn register_probe(distro: &str, key: &str)`
  - `pub fn unregister_probe(distro: &str, key: &str)`
  - `pub fn foreground_comm(distro: &str, key: &str) -> Option<String>` — non-blocking, UI-thread-safe

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn probe_cache_lifecycle() {
    // An inert distro name: even if the poller ticks mid-test, `client()`
    // cools down on the failed spawn instead of touching a real distro.
    const D: &str = "no-such-distro";
    // Unknown key: unknown comm — the caller treats that as "no TUI".
    assert_eq!(foreground_comm(D, "test-77-1"), None);
    register_probe(D, "test-77-1");
    // Registered but not yet polled: still unknown, not a panic or a block.
    assert_eq!(foreground_comm(D, "test-77-1"), None);
    set_cached_comm(D, "test-77-1", Some("nvim".to_string()));
    assert_eq!(foreground_comm(D, "test-77-1").as_deref(), Some("nvim"));
    unregister_probe(D, "test-77-1");
    assert_eq!(foreground_comm(D, "test-77-1"), None);
}

#[test]
fn probe_keys_are_pid_namespaced_and_unique() {
    let a = new_probe_key();
    let b = new_probe_key();
    assert_ne!(a, b);
    let prefix = format!("{}-", std::process::id());
    assert!(a.starts_with(&prefix), "{a} should start with {prefix}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree wsl_helper::`
Expected: compile error — `register_probe`, `foreground_comm`, `new_probe_key`, `set_cached_comm` not found.

- [ ] **Step 3: Implement the probe registry and poller**

```rust
/// Identity of a shimmed WSL session for the foreground probe.
#[derive(Debug, Clone)]
pub struct WslProbe {
    pub distro: String,
    pub key: String,
}

const PROBE_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Last-known foreground `comm` per registered `(distro, probe key)`.
/// Written only by the poller thread (and tests); read from the UI thread.
fn probe_cache() -> &'static Mutex<HashMap<(String, String), Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A probe key unique across alacritree instances: the pidfile dir inside
/// each distro is shared, so the Windows pid namespaces the per-instance
/// counter.
pub fn new_probe_key() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

pub fn register_probe(distro: &str, key: &str) {
    lock(probe_cache()).insert((distro.to_string(), key.to_string()), None);
    ensure_poller();
}

pub fn unregister_probe(distro: &str, key: &str) {
    lock(probe_cache()).remove(&(distro.to_string(), key.to_string()));
}

/// Cached foreground `comm` for a shimmed WSL session — never blocks and
/// never touches the pipe, so it is safe on the UI thread.  `None` means
/// unknown (helper down, key unregistered, or an idle shell at the last
/// poll); callers must treat unknown as "no TUI".
pub fn foreground_comm(distro: &str, key: &str) -> Option<String> {
    lock(probe_cache()).get(&(distro.to_string(), key.to_string()))?.clone()
}

#[cfg(test)]
fn set_cached_comm(distro: &str, key: &str, comm: Option<String>) {
    lock(probe_cache()).insert((distro.to_string(), key.to_string()), comm);
}

/// One process-wide poller refreshes every registered key at the agent
/// cadence.  Requests leave this thread, so a slow helper delays freshness,
/// never the UI.  Polling a distro also (re)spawns its helper through
/// `client()`, so an open WSL session keeps nudging a cooled-down helper
/// back up.  The key list is snapshotted before any pipe I/O so the cache
/// lock is never held across a request.
fn ensure_poller() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new().name("wsl-helper-probe".to_string()).spawn(
            || loop {
                std::thread::sleep(PROBE_POLL_INTERVAL);
                let keys: Vec<(String, String)> =
                    lock(probe_cache()).keys().cloned().collect();
                for entry in keys {
                    let comm =
                        client(&entry.0).and_then(|c| c.probe(&entry.1).ok()).flatten();
                    if let Some(slot) = lock(probe_cache()).get_mut(&entry) {
                        *slot = comm;
                    }
                }
            },
        );
        if let Err(e) = spawned {
            log::warn!("wsl probe poller failed to start: {e}");
        }
    });
}
```

Note: the poller does not `request_repaint` — the cached value only matters when a FocusLeft/FocusRight key arrives, and a keypress already triggers a frame.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree wsl_helper::`
Expected: all PASS (15 non-ignored). The lifecycle test never touches a real distro: the poller's first tick comes after a 1 s sleep, the key is unregistered immediately, and the inert distro name means a stray tick only burns one failed spawn into cooldown.

- [ ] **Step 5: Extend the live test**

In `helper_round_trips`, after the unregistered-probe assertion, add a shimmed-session round trip. A session spawned with pipes has no controlling tty inside the distro, so its tpgid is `-1` and the probe legitimately answers "unknown" — the tty half of the probe (nvim ⇒ passthrough) is only observable under ConPTY and stays with the manual smoke checklist. What this test *can* verify live is the shim's pidfile publication and that probing it neither errors nor lies:

```rust
    use std::process::{Command, Stdio};

    // The shim publishes its pid, then execs the login shell; piped stdin
    // (held open) keeps that shell alive for the duration of the test.
    let key = new_probe_key();
    let (program, args) = shim_invocation(&distro.name, Path::new(r"C:\"), &key);
    let mut child = Command::new(program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn shimmed session");
    std::thread::sleep(Duration::from_secs(3));

    // The pidfile names a live, numeric pid...
    let (exit, out) = client
        .run(
            r#"cat "${XDG_RUNTIME_DIR:-/tmp}/alacritree/session-$1.pid" 2>/dev/null"#,
            &[&key],
        )
        .expect("read pidfile");
    assert_eq!(exit, 0, "pidfile should exist for a shimmed session");
    let pid = String::from_utf8_lossy(&out);
    assert!(!pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit()), "pid: {pid:?}");

    // ...and probing it completes without a transport error.  No tty under
    // pipes, so the comm is unknown — which must read as "no TUI".
    assert_eq!(client.probe(&key).expect("probe shimmed session"), None);

    let _ = child.kill();
    let _ = child.wait();
```

Run: `cargo test -p alacritree wsl_helper::helper_round_trips -- --ignored`
Expected: PASS (skip with a note if the build machine has no WSL).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/wsl_helper.rs
git commit -m "feat(wsl): poll foreground comm for shimmed sessions" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: run_batch and discover_delta go resident-first

**Files:**
- Modify: `alacritree/src/wsl.rs` (`run_batch` ~305, `discover_delta` ~334)

**Interfaces:**
- Consumes: `wsl_helper::try_run`, `wsl_helper::capability_delta`.
- Produces: unchanged public signatures — `run_batch(distro, script, args) -> Result<Vec<u8>, String>`, `discover_delta(distro) -> Option<String>`. All existing callers (git_status.rs, projects.rs, worktree.rs, app.rs) accelerate with zero call-site changes.

- [ ] **Step 1: Modify `run_batch`**

Insert at the top of the function body, and extend the doc comment's first paragraph with the resident-first sentence:

```rust
/// Run `script` through `sh -c` inside `distro`, with `args` bound to
/// `$1..`.  Rides the resident helper's pipe when it is up; otherwise one
/// wsl.exe round trip (~400 ms warm on a dev machine, seconds while the VM
/// cold-boots) — callers batch every query for a repo into a single script
/// and must never call this on the UI thread.
pub fn run_batch(distro: &str, script: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    // A request the helper may have executed is never re-run as a one-shot
    // (batch scripts have side effects); only a transport that failed
    // before the write falls through to the spawn below.
    if let Some(result) = crate::wsl_helper::try_run(distro, script, args) {
        return result;
    }
    let output = command(distro, None)
        // ... existing body unchanged ...
```

- [ ] **Step 2: Modify `discover_delta`**

Insert at the top of the function body and note the capability path in the doc comment:

```rust
pub fn discover_delta(distro: &str) -> Option<String> {
    // The helper's hello already resolved delta through the login shell; a
    // missing capability is not a cached miss — fall through and re-check
    // live so a mid-session install is still picked up.
    if let Some(path) = crate::wsl_helper::capability_delta(distro) {
        return Some(path);
    }
    let script = /* ... existing body unchanged ... */
```

- [ ] **Step 3: Verify the whole suite**

Run: `cargo test -p alacritree`
Expected: all tests PASS (no behavior change on machines where `client()` yields `None`; the unit suite never brings a helper up).

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add alacritree/src/wsl.rs
git commit -m "feat(wsl): route run_batch and delta discovery through the helper" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: PR status rides the resident path

**Files:**
- Modify: `alacritree/src/pr_status.rs` (`query_gh` ~158)

**Interfaces:**
- Consumes: `wsl::run_batch` (Task 5 resident-first), `wsl_helper::capability_gh`, `wsl::Location`.
- Produces: `query_gh(path, branch) -> Option<PrInfo>` — signature and routing unchanged: Windows repo → Windows `gh`, WSL repo → the distro's `gh`. Only the WSL transport changes.

- [ ] **Step 1: Rewrite `query_gh`**

Replace the whole function (the `Command`-building match plus the shared tail) with per-arm execution — the WSL arm can no longer share the Windows arm's `Command` plumbing:

```rust
/// Ask `gh` for the PR associated with `branch` in `path`.  Returns `None`
/// on any failure mode (no `gh`, not authenticated, no PR, non-GitHub
/// remote, ...).  The branch is passed as a positional selector so the
/// answer is tied to that specific branch rather than whatever ref happens
/// to be checked out in the worktree.
fn query_gh(path: &Path, branch: &str) -> Option<PrInfo> {
    const PR_JSON_FIELDS: &str = "number,baseRefName,url,state,isDraft";
    match wsl::classify(path) {
        wsl::Location::Windows(p) => {
            let output = Command::new("gh")
                .hide_console()
                .current_dir(p)
                .args(["pr", "view", branch, "--json", PR_JSON_FIELDS])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .stdin(Stdio::null())
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            parse_gh_output(&output.stdout)
        },
        // `gh` must be installed and authenticated *inside* the distro; any
        // failure falls back to the default branch, same as a missing
        // Windows gh.  The batch script rides the resident helper when it
        // is up (a one-shot spawn otherwise); the capability path from the
        // helper's hello honors per-user install dirs that the default
        // `--exec` PATH lacks.
        wsl::Location::Wsl { distro, linux_path } => {
            let gh = crate::wsl_helper::capability_gh(&distro)
                .unwrap_or_else(|| "gh".to_string());
            let script = r#"cd "$1" && exec "$2" pr view "$3" --json "$4""#;
            let stdout = wsl::run_batch(
                &distro,
                script,
                &[&linux_path, &gh, branch, PR_JSON_FIELDS],
            )
            .ok()?;
            parse_gh_output(&stdout)
        },
    }
}
```

Remove the now-unused `wsl::command` import path if nothing else in the file uses it (check with `rg "wsl::command" alacritree/src/pr_status.rs`).

- [ ] **Step 2: Verify the suite**

Run: `cargo test -p alacritree pr_status::`
Expected: existing `parses_gh_json` / `rejects_empty_output` / `parses_pr_states` / `missing_state_fields_default_to_open` tests PASS unchanged (they exercise `parse_gh_output`, which did not move).

Run: `cargo test -p alacritree`
Expected: full suite PASS.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add alacritree/src/pr_status.rs
git commit -m "feat(pr): query gh for wsl repos through the resident helper" -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Sessions — shim spawn, probe consumption, boundary rule deletion

This is the task that actually fixes the dead FocusLeft/FocusRight keys.

**Files:**
- Modify: `alacritree/src/session.rs` (`Session` struct ~82, `spawn`/`spawn_command`/`spawn_with` ~538-673, `process_probe` ~778, `windows_process_probe::probe` ~517, `is_wsl_boundary_name` ~197-205, `Drop` ~848)
- Modify: `alacritree/src/app.rs` (`resolve_shell` ~809, `spawn_session_with_shell` ~745, `spawn_profile_session` ~792, `wsl_shell` ~3156, `profile_shell` ~3209)
- Test: inline tests in both files

**Interfaces:**
- Consumes: `wsl_helper::{WslProbe, new_probe_key, register_probe, unregister_probe, foreground_comm, shim_invocation, wrap_profile_argv, enabled}` and `wsl::distros`.
- Produces:
  - `Session::spawn(ctx, config, working_directory, size, cell_size, shell_override: Option<Shell>, wsl_probe: Option<WslProbe>)` — one new trailing parameter
  - `session.rs` private fn `wsl_nav_tui(comm: Option<&str>) -> bool`
  - `app.rs` private fns `wsl_session_shell(distro: &str, workdir: &Path) -> (Option<Shell>, Option<WslProbe>)`, `shimmed_wsl_argv(program: &str, args: &[String]) -> Option<(Shell, WslProbe)>`, `profile_session_shell(profile: &crate::config::Profile) -> (Option<Shell>, Option<WslProbe>)`, and `config_session_shell(config: &crate::config::Config) -> (Option<Shell>, Option<WslProbe>)`
  - `resolve_shell` returns `(Option<Shell>, Option<WslProbe>)`

- [ ] **Step 1: Write the failing probe-decision tests (session.rs)**

In the `session.rs` test module, add:

```rust
#[test]
fn wsl_nav_tui_needs_a_known_cooperating_comm() {
    assert!(wsl_nav_tui(Some("nvim")));
    assert!(wsl_nav_tui(Some("vim")));
    assert!(wsl_nav_tui(Some("tmux: client")));
    // A shell, an agent, or an unknown probe must move panel focus —
    // losing passthrough beats losing the keys.
    assert!(!wsl_nav_tui(Some("bash")));
    assert!(!wsl_nav_tui(Some("claude")));
    assert!(!wsl_nav_tui(None));
}
```

Also locate the existing tests that assert the boundary rule (`rg -n "is_wsl_boundary_name" alacritree/src/session.rs`). Any test asserting that a `wsl*` image name yields `nav_tui == true` must be **inverted**: a wsl.exe in the descendant tree no longer implies a cooperating TUI. Update those assertions now so they fail against the current code.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree session::`
Expected: compile error on `wsl_nav_tui`; the inverted boundary tests FAIL against the still-present rule.

- [ ] **Step 3: Implement the session.rs changes**

1. Add the import: `use crate::wsl_helper::{self, WslProbe};`

2. Add the decision helper near `is_nav_tui_name`:

```rust
/// FocusLeft/FocusRight passthrough decision for a shimmed WSL session:
/// the helper's cached foreground `comm`, matched like the native Linux
/// probe.  Unknown means no TUI — the keys move panel focus.
#[cfg(any(test, windows))]
fn wsl_nav_tui(comm: Option<&str>) -> bool {
    comm.is_some_and(is_nav_tui_name)
}
```

3. Delete `is_wsl_boundary_name` (~197-205) and its `use` inside `windows_process_probe` (~478), and in `windows_process_probe::probe` change:

```rust
let nav_tui = names.iter().any(|n| is_nav_tui_name(n) || is_wsl_boundary_name(n));
```
to
```rust
let nav_tui = names.iter().any(|n| is_nav_tui_name(n));
```

(The spec reserved the function for "boundary detection", but the only detection site left — recognizing a wsl.exe profile program — needs an exact `wsl` stem match, not a `wsl*` prefix that would also swallow `wslhost`/`wslrelay`; that lives in `wrap_profile_argv`. See Unresolved Questions.)

4. Extend `Session`:

```rust
    /// Set for shimmed WSL sessions: the distro plus the probe key its
    /// shim published, unregistered again on drop.  The Windows process
    /// table ends at wsl.exe, so this is the only live view inside.
    wsl_probe: Option<WslProbe>,
```

5. Thread the parameter: `Session::spawn` gains `wsl_probe: Option<WslProbe>` after `shell_override` and passes it to `spawn_with`; `spawn_command` passes `None`; `spawn_with` gains the parameter, registers before returning, and stores it:

```rust
        if let Some(probe) = &wsl_probe {
            wsl_helper::register_probe(&probe.distro, &probe.key);
        }
        Ok(Self {
            // ... existing fields ...
            wsl_probe,
            // ...
        })
```

6. `Drop` unregisters first:

```rust
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(probe) = &self.wsl_probe {
            wsl_helper::unregister_probe(&probe.distro, &probe.key);
        }
        self.shutdown();
    }
}
```

7. In `process_probe`, replace the `nav_tui` line:

```rust
        let nav_tui = match &self.wsl_probe {
            Some(probe) => {
                wsl_nav_tui(wsl_helper::foreground_comm(&probe.distro, &probe.key).as_deref())
            },
            None => self.shell_pid.is_some_and(foreground_nav_tui),
        };
```

(The cache read is non-blocking, so the 1 s `AGENT_CACHE_TTL` wrapper keeps working unchanged around it.)

- [ ] **Step 4: Implement the app.rs changes**

1. Add imports: `use crate::wsl_helper::{self, WslProbe};` (adjust to the file's existing import style).

2. Replace `wsl_shell` usage with a probe-aware pair next to it (keep `wsl_shell` itself — the disabled path still uses it):

```rust
/// Shimmed when the resident helper is on; the plain wsl.exe login-shell
/// launch (and an unknown probe) otherwise.
fn wsl_session_shell(distro: &str, workdir: &Path) -> (Option<Shell>, Option<WslProbe>) {
    if !wsl_helper::enabled() {
        return (Some(wsl_shell(distro, workdir)), None);
    }
    let key = wsl_helper::new_probe_key();
    let (program, args) = wsl_helper::shim_invocation(distro, workdir, &key);
    (Some(Shell::new(program, args)), Some(WslProbe { distro: distro.to_string(), key }))
}

/// The probe shim for any user-supplied wsl.exe argv (profile or
/// `[terminal.shell]`): `Some` only when the argv is fully understood and
/// a distro name is known — the probe registry needs one, so a wrapped
/// default-distro launch resolves it via enumeration.  Anything exotic
/// runs unmodified and probes as unknown.
fn shimmed_wsl_argv(program: &str, args: &[String]) -> Option<(Shell, WslProbe)> {
    if !wsl_helper::enabled() {
        return None;
    }
    let key = wsl_helper::new_probe_key();
    let (args, distro) = wsl_helper::wrap_profile_argv(program, args, &key)?;
    let distro =
        distro.or_else(|| wsl::distros().into_iter().find(|d| d.is_default).map(|d| d.name))?;
    Some((Shell::new(program.to_string(), args), WslProbe { distro, key }))
}

fn profile_session_shell(profile: &crate::config::Profile) -> (Option<Shell>, Option<WslProbe>) {
    match shimmed_wsl_argv(&profile.program, &profile.args) {
        Some((shell, probe)) => (Some(shell), Some(probe)),
        None => (Some(profile_shell(profile)), None),
    }
}

/// `[terminal.shell] program = "wsl.exe"` gets the same shim as a wsl.exe
/// profile; any other config shell (or none) spawns unchanged through
/// `Session::spawn`'s own config-shell default.
fn config_session_shell(config: &crate::config::Config) -> (Option<Shell>, Option<WslProbe>) {
    match &config.shell {
        Some(s) => match shimmed_wsl_argv(&s.program, &s.args) {
            Some((shell, probe)) => (Some(shell), Some(probe)),
            None => (None, None),
        },
        None => (None, None),
    }
}
```

3. `resolve_shell` returns the pair:

```rust
    fn resolve_shell(&self, workspace: &WorkspaceKey) -> (Option<Shell>, Option<WslProbe>) {
        // ... existing choice/location/known resolution unchanged ...
        match shell_decision(
            choice.as_ref(),
            location_distro.as_deref(),
            &known,
            &self.config.profiles,
            self.config.default_profile.as_deref(),
        ) {
            ShellDecision::ConfigShell => config_session_shell(&self.config),
            // A WSL decision only arises from a workspace path (override or
            // location), never from the home tab.
            ShellDecision::WslDistro(distro) => match path {
                Some(p) => wsl_session_shell(&distro, p),
                None => (None, None),
            },
            ShellDecision::Profile(name) => match self.config.profile(&name) {
                Some(profile) => profile_session_shell(profile),
                None => (None, None),
            },
        }
    }
```

4. Update the callers. `spawn_session_with_shell` gains the probe parameter and passes it to `Session::spawn`:

```rust
    fn spawn_session_with_shell(
        &mut self,
        ctx: &Context,
        working_directory: WorkspaceKey,
        shell: Option<Shell>,
        wsl_probe: Option<WslProbe>,
    ) -> std::io::Result<SessionId> {
        let session = Session::spawn(
            ctx.clone(),
            &self.config,
            working_directory.clone(),
            TermSize::new(80, 24),
            (8.0, 16.0),
            shell,
            wsl_probe,
        )?;
        // ... rest unchanged ...
```

In `spawn_session` (~741): `let (shell, wsl_probe) = self.resolve_shell(&working_directory); self.spawn_session_with_shell(ctx, working_directory, shell, wsl_probe)`.

In `spawn_profile_session` (~792): `let (shell, wsl_probe) = profile_session_shell(profile); ... self.spawn_session_with_shell(ctx, ws, shell, wsl_probe)`.

5. Sweep for other callers: `rg -n "resolve_shell|spawn_session_with_shell|Session::spawn\b" alacritree/src` and update every site to the new signatures (diff panes and any IPC-driven spawn pass `None` for the probe).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p alacritree`
Expected: full suite PASS, including the new `wsl_nav_tui_needs_a_known_cooperating_comm` and the inverted boundary assertions. `cargo check -p alacritree` clean.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/src/session.rs alacritree/src/app.rs
git commit -m "feat(session): probe wsl foreground via the resident helper" -m "The Windows process probe cannot see past wsl.exe, so the boundary rule
assumed a cooperating TUI and forwarded every focus key into the distro,
where a bare shell or agent swallowed them.  Shimmed WSL sessions now
publish their shell pid for the helper's tpgid probe; an unknown probe
moves panel focus instead of forwarding." -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Config — top-level `[wsl]` section

**Files:**
- Modify: `alacritree/src/config.rs` (`RawUiWsl` ~920, `RawConfig` root struct, `into_config` WSL block ~1270, `Config` struct ~42 and `Default` ~453)
- Modify: `alacritree/src/main.rs` (~88)
- Test: inline tests in `config.rs`

**Interfaces:**
- Consumes: `wsl_helper::set_enabled` (Task 3).
- Produces: `Config::wsl_resident_helper: bool` (default `true`); `Config::wsl_automount_root` now sourced `[wsl]`-first.

- [ ] **Step 1: Write the failing tests**

Next to the existing `automount_root_defaults_and_normalizes` test (~1391), add:

```rust
#[test]
fn wsl_section_wins_over_deprecated_ui_location() {
    let raw: RawConfig = toml::from_str("[wsl]\nautomount_root = \"/drives\"").unwrap();
    assert_eq!(raw.into_config().wsl_automount_root, "/drives");

    let both = "[wsl]\nautomount_root = \"/new\"\n[ui.wsl]\nautomount_root = \"/old\"";
    let raw: RawConfig = toml::from_str(both).unwrap();
    assert_eq!(raw.into_config().wsl_automount_root, "/new");

    // Existing configs keep working through the deprecated location.
    let raw: RawConfig = toml::from_str("[ui.wsl]\nautomount_root = \"/old\"").unwrap();
    assert_eq!(raw.into_config().wsl_automount_root, "/old");
}

#[test]
fn resident_helper_defaults_on() {
    let raw: RawConfig = toml::from_str("").unwrap();
    assert!(raw.into_config().wsl_resident_helper);

    let raw: RawConfig = toml::from_str("[wsl]\nresident_helper = false").unwrap();
    assert!(!raw.into_config().wsl_resident_helper);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alacritree config::`
Expected: compile error — `RawConfig` has no field `wsl`, `Config` has no `wsl_resident_helper`.

- [ ] **Step 3: Implement**

1. New raw struct next to `RawUiWsl`:

```rust
/// Top-level `[wsl]`: platform-integration options.  Lives outside `[ui]`
/// because nothing here is presentation — it governs how the app talks to
/// distros.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawWsl {
    /// Keep a resident helper process per distro for foreground probes,
    /// batched git queries, and tool discovery.  `false` restores one-shot
    /// wsl.exe spawns everywhere; WSL sessions then always report "no
    /// TUI", so FocusLeft/FocusRight always move panel focus.
    resident_helper: Option<bool>,
    /// Distro-side mount point for Windows drives, mirroring wsl.conf's
    /// `[automount] root`.  Only used for paths *we* translate (git output
    /// from inside a distro); `wsl.exe --cd` translates with the distro's
    /// real mount table regardless of this value.
    automount_root: Option<String>,
}
```

2. Mark the old location deprecated — extend `RawUiWsl`'s field doc:

```rust
    /// Deprecated location: `[wsl] automount_root` supersedes this and wins
    /// when both are set; kept so existing configs keep working.
    automount_root: Option<String>,
```

3. Add `wsl: RawWsl,` to the `RawConfig` root struct (find it with `rg -n "struct RawConfig" alacritree/src/config.rs`; it already `#[serde(default)]`s its sections — match that pattern).

4. In `into_config`, replace the `---- WSL ----` block:

```rust
        // ---- WSL ----
        // `[wsl]` supersedes the deprecated `[ui.wsl]` location.
        let wsl_automount_root = self
            .wsl
            .automount_root
            .or(self.ui.wsl.automount_root)
            .map(|r| r.trim_end_matches('/').to_string())
            .filter(|r| r.starts_with('/') && r.len() > 1)
            .unwrap_or_else(|| "/mnt".to_string());
        let wsl_resident_helper = self.wsl.resident_helper.unwrap_or(true);
```

and add `wsl_resident_helper,` to the `Config { ... }` construction.

5. `Config` struct: add `pub wsl_resident_helper: bool,` next to `wsl_automount_root` (~42), and `wsl_resident_helper: true,` in the `Default` impl (~453).

6. `main.rs`, after the `set_automount_root` line:

```rust
    wsl_helper::set_enabled(config.wsl_resident_helper);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alacritree`
Expected: full suite PASS, including both new config tests and the untouched `automount_root_defaults_and_normalizes`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs alacritree/src/main.rs
git commit -m "feat(config): add top-level [wsl] section" -m "Moves automount_root out of [ui.wsl] (still honored, deprecated) and
adds resident_helper, default on, whose off state restores one-shot
wsl.exe spawns with probes reporting unknown." -m "Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Final verification

- [ ] `cargo test -p alacritree` — full suite green.
- [ ] `cargo build -p alacritree --release` — builds clean (do **not** run the GUI, do not touch running `alacritree.exe` instances; the user swaps the binary and verifies live).
- [ ] If WSL is available: `cargo test -p alacritree wsl_helper:: -- --ignored` green.
- [ ] Manual smoke checklist for the user (put in the final report, not automated):
  1. Rebuild + restart alacritree, open a WSL-profile session: Ctrl+Left/Right now move panel focus in a bare shell and inside Claude Code.
  2. Run `nvim` in the WSL session: within ~1 s the keys pass through; quit nvim: within ~1 s they move focus again.
  3. Git sidebar in a WSL repo refreshes noticeably faster after the first query (resident path).
  4. `wsl --shutdown` mid-session: features degrade to one-shot (log line "falling back to one-shot spawns"), recover within 30 s of the VM coming back on next use.
  5. Set `[wsl] resident_helper = false`, restart: keys always move focus in WSL sessions; no helper process inside the distro (`ps aux | rg alacritree`-shaped check finds none — the helper is `sh`, so check for the mktemp dir instead: no `/tmp/tmp.*/done` FIFO owned by an sh with an alacritree parent).

## Resolved questions (answered 2026-07-17)

1. **Base branch:** `session-display-and-focus` (`ed98d820`) — a separate `feat/wsl-resident-helper` branch stacked on PR #103's work; the user merges the PR stack down first, then this becomes its own PR. (#103 is still open upstream; master carries only through #102/#105–#107.)
2. **`is_wsl_boundary_name` deletion:** approved — no caller remains and the behavior change (unknown foreground ⇒ keys move focus) is the feature.
3. **Config-shell wsl.exe:** shimmed too, same strict-parse rule as profiles — `config_session_shell` in Task 7.
4. **Live tests:** fine to run against the default distro during implementation; nothing destructive (`wsl --shutdown`, distro modification, killing sessions are all off-limits).
