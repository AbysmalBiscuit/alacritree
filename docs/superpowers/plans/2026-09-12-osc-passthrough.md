# OSC passthrough Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support the OSC sequences that need no cursor position, meaning OSC 52 read, 7, 9;9, 9, 777, 9;4 and 22, without forking `vte` or editing a vendored crate.

**Architecture:** A PTY reader wrapper copies each chunk to a per-session thread running a second `vte::Parser` whose `Perform` implements only `osc_dispatch`. Recognised sequences reach the UI thread through a channel drained beside the existing terminal events. OSC 52 read needs no tap at all: it is a config value `Term` already enforces plus one match arm.

**Tech Stack:** Rust edition 2024, `alacritty_terminal`, `vte` 0.15, `egui`/`eframe`, `gethostname`.

**Spec:** `docs/superpowers/specs/2026-09-12-osc-passthrough-design.md`

## Global Constraints

- Workspace MSRV is 1.85, edition 2024.
- `alacritty/`, `alacritty_terminal/`, `alacritty_config/`, `alacritty_config_derive/` are read-only. Every change lands in `alacritree/`.
- Every new `[vt]` key defaults to `false`, which is today's behaviour. `[terminal] osc52` defaults to `OnlyCopy`, which is upstream's default.
- Run commands through devkit, not cargo directly: `devrun task check`, `devrun task test`, `devrun task fmt`, `devrun task clippy`. `fmt` runs nightly rustfmt because ten of the fifteen options in `rustfmt.toml` are nightly-only.
- This checkout is shared with other agents. Claim every file before editing it: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"`.
- Commit messages are Conventional Commits, imperative, subject under 72 characters, and end with the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- After any change to a `Raw*` config type, regenerate the schema with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema` and leave `config_schema` and `schema_defaults` passing.
- Comments explain why, never what. No PR or task references, no change-relative phrasing.

---

### Task 1: Config surface

Adds the `[vt]` table and the `[terminal] osc52` key, and wires the latter into `TermConfig`. Nothing reads the `[vt]` values yet; later tasks consume them.

**Files:**
- Modify: `alacritree/src/config.rs`
- Modify: `alacritree/src/session.rs` (`term_config`)
- Modify: `schema/alacritree-config.json` (generated, do not hand-edit)
- Test: in-module `#[cfg(test)]` in `alacritree/src/config.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Config::vt: VtConfig` with fields `report_cwd: bool`, `notify: bool`, `progress: bool`, `pointer_shape: bool`. `Config::osc52: alacritty_terminal::term::Osc52`. `VtConfig::any_enabled(&self) -> bool`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)]` module in `alacritree/src/config.rs`, beside the existing `ui_from_toml` helper:

```rust
fn vt_from_toml(input: &str) -> VtConfig {
    let value: toml::Value = toml::from_str(input).expect("valid toml");
    let raw: RawConfig = value.try_into().expect("valid config");
    raw.into_config().vt
}

#[test]
fn vt_keys_are_all_off_unless_asked_otherwise() {
    let vt = vt_from_toml("");
    assert!(!vt.report_cwd);
    assert!(!vt.notify);
    assert!(!vt.progress);
    assert!(!vt.pointer_shape);
    assert!(!vt.any_enabled());
}

#[test]
fn vt_keys_parse_and_report_enabled() {
    let vt = vt_from_toml("[vt]\nreport_cwd = true");
    assert!(vt.report_cwd);
    assert!(!vt.notify);
    assert!(vt.any_enabled());
}

#[test]
fn osc52_defaults_to_copy_only_and_parses() {
    let value: toml::Value = toml::from_str("").expect("valid toml");
    let raw: RawConfig = value.try_into().expect("valid config");
    assert_eq!(raw.into_config().osc52, Osc52::OnlyCopy);

    let value: toml::Value =
        toml::from_str("[terminal]\nosc52 = \"CopyPaste\"").expect("valid toml");
    let raw: RawConfig = value.try_into().expect("valid config");
    assert_eq!(raw.into_config().osc52, Osc52::CopyPaste);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devrun task test -- config::tests::vt_keys_are_all_off_unless_asked_otherwise`
Expected: FAIL to compile, `cannot find type VtConfig in this scope`.

- [ ] **Step 3: Add the config types**

In `alacritree/src/config.rs`, add the resolved type near the other resolved config structs:

```rust
/// Sequences alacritree reads off the PTY byte stream rather than through
/// `Term`.  Each is off by default: turning one on starts a parser thread
/// per session and changes what the UI shows.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct VtConfig {
    pub report_cwd: bool,
    pub notify: bool,
    pub progress: bool,
    pub pointer_shape: bool,
}

impl VtConfig {
    /// Whether any sequence is wanted.  A session with none skips the tap
    /// thread entirely.
    pub fn any_enabled(&self) -> bool {
        self.report_cwd || self.notify || self.progress || self.pointer_shape
    }
}
```

Add the raw type beside `RawDebug`:

```rust
/// alacritree-only, so it belongs in `alacritree.toml`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(default)]
struct RawVt {
    /// Track the shell's working directory from OSC 7 and OSC 9;9, and show
    /// it in the session row's hover text.  A sibling session then starts
    /// where the shell is rather than at the workspace root.
    report_cwd: bool,
    /// Surface OSC 9 and OSC 777 desktop notifications.  The text reaches
    /// the sidebar row either way; `[ui] notifications` decides whether a
    /// desktop toast fires.
    notify: bool,
    /// Draw the ConEmu progress report (OSC 9;4) as a bar across the
    /// session's sidebar row.
    progress: bool,
    /// Let an application choose the mouse cursor over the grid with OSC 22.
    pointer_shape: bool,
}
```

Add `osc52` to `RawTerminal`, which currently holds only `shell`:

```rust
    /// Whether an application may use OSC 52 to write the clipboard, read
    /// it, both, or neither.  Upstream alacritty's key and upstream's
    /// default, so it belongs in the shared `alacritty.toml`.  Reading is
    /// refused by default: an application that can read the clipboard can
    /// read whatever was last copied anywhere else.
    #[serde(default)]
    osc52: SerdeOsc52,
```

`Osc52` does not implement `Deserialize` for a bare TOML string, so mirror upstream's newtype from `alacritty/src/config/terminal.rs`:

```rust
#[derive(Debug, Default, JsonSchema)]
#[serde(transparent)]
struct SerdeOsc52(#[schemars(with = "String")] Osc52);

impl<'de> Deserialize<'de> for SerdeOsc52 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Osc52::deserialize(toml::Value::String(value))
            .map(SerdeOsc52)
            .map_err(serde::de::Error::custom)
    }
}
```

Add `vt: RawVt` to `RawConfig`, `vt: VtConfig` and `osc52: Osc52` to `Config`, and populate both in `into_config`.

Import `Osc52` at the top of the file: `use alacritty_terminal::term::Osc52;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `devrun task test -- config::tests::vt_`
Expected: PASS, three tests.

- [ ] **Step 5: Wire the value into `TermConfig`**

In `alacritree/src/session.rs`, `term_config` currently relies on `..TermConfig::default()` leaving `osc52` at `OnlyCopy`. Make it explicit:

```rust
        // `Term` refuses an OSC 52 read before an event is ever emitted
        // unless this carries the user's choice.  Upstream's default refuses
        // it, and so does ours.
        osc52: config.osc52,
```

- [ ] **Step 6: Regenerate the schema and verify**

```bash
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
devrun task test -- schema_defaults
devrun task check
```

Expected: `config_schema` and `schema_defaults` both PASS. If `schema_defaults` requires an allowlist entry, justify it in `alacritree/tests/schema-defaults-allowlist.txt` against omission behaviour.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/config.rs alacritree/src/session.rs schema/alacritree-config.json
git commit -m "$(cat <<'EOF'
feat(config): add the [vt] table and terminal.osc52

The sequences alacritree reads off the byte stream each need a key whose
default is today's behaviour, and OSC 52 read needs the upstream key that
Term already enforces.  Nothing consumes the [vt] values yet.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: OSC 52 read

Answers a clipboard read request. No tap involvement.

**Files:**
- Modify: `alacritree/src/session.rs` (`DrainOutcome`, `apply_term_event`)
- Modify: `alacritree/src/app.rs` (beside the existing `outcome.clipboard` loop)
- Test: in-module `#[cfg(test)]` in `alacritree/src/session.rs`

**Interfaces:**
- Consumes: `Config::osc52` from Task 1.
- Produces: `DrainOutcome::clipboard_reads: Vec<(Target, ClipboardFormatter)>` where `ClipboardFormatter = Arc<dyn Fn(&str) -> String + Send + Sync>`.

- [ ] **Step 1: Write the failing test**

The existing `osc52_copy_is_carried_out_to_the_clipboard` at `alacritree/src/session.rs:2273` is the model. Add beside it:

```rust
#[test]
fn osc52_read_is_carried_out_for_the_caller_to_answer() {
    let mut title = String::new();
    let mut exit_status = None;
    let mut outcome = DrainOutcome::default();
    let event = TermEvent::ClipboardLoad(
        ClipboardType::Clipboard,
        Arc::new(|text: &str| format!("reply:{text}")),
    );

    apply_term_event(event, &mut title, false, &mut exit_status, &mut outcome);

    assert_eq!(outcome.clipboard_reads.len(), 1);
    let (target, format) = &outcome.clipboard_reads[0];
    assert_eq!(*target, Target::Clipboard);
    assert_eq!(format("hello"), "reply:hello");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `devrun task test -- osc52_read_is_carried_out_for_the_caller_to_answer`
Expected: FAIL, `no field clipboard_reads on type DrainOutcome`.

- [ ] **Step 3: Carry the read out of the drain**

In `alacritree/src/session.rs`, add to `DrainOutcome`:

```rust
    /// OSC 52 read requests, answered by the caller for the same reason
    /// copied text is written there: the drain runs once per frame for every
    /// session and stays free of OS clipboard access.
    pub clipboard_reads: Vec<(Target, ClipboardFormatter)>,
```

with the alias beside it:

```rust
/// Builds the whole reply sequence from the clipboard's contents, carrying
/// whatever prefix and terminator the request arrived with.
pub type ClipboardFormatter = Arc<dyn Fn(&str) -> String + Send + Sync + 'static>;
```

Add the arm to `apply_term_event`, beside the existing `ClipboardStore` arm:

```rust
        // OSC 52 read.  `Term` only emits this once the config allows it, so
        // reaching here means the user opted in.
        TermEvent::ClipboardLoad(ty, format) => {
            outcome.clipboard_reads.push((clipboard_target(ty), format))
        },
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `devrun task test -- osc52_read_is_carried_out_for_the_caller_to_answer`
Expected: PASS.

- [ ] **Step 5: Answer the request in the frame loop**

In `alacritree/src/app.rs`, immediately after the existing loop that writes `outcome.clipboard`:

```rust
            // Upstream alacritty answers a read only for the focused window
            // (`alacritty/src/event.rs`, ClipboardLoad).  A background
            // alacritree session is what a background alacritty window would
            // be, so it takes both conditions.
            if focused && Some(idx) == visible_idx {
                for (target, format) in &outcome.clipboard_reads {
                    if let Some(text) = clipboard::read(*target) {
                        self.sessions[idx].write(format(&text).into_bytes());
                    }
                }
            }
```

Place it before the attention early-out, since that block `continue`s.

- [ ] **Step 6: Verify the build and full suite**

```bash
devrun task check
devrun task test
devrun task clippy
```

Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/session.rs alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(vt): answer OSC 52 clipboard reads when configured

Term dropped the read before an event existed and the drain dropped the
event into its catch-all.  The formatter is carried out to the frame loop
so the drain keeps its distance from OS clipboard access, and the reply
goes out only for a focused, visible session, matching upstream.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: The classification seam

Every protocol decision as one pure function. No threads, no PTY, no egui context beyond the `CursorIcon` enum.

**Files:**
- Create: `alacritree/src/osc_tap.rs`
- Modify: `alacritree/src/main.rs` (add `mod osc_tap;`)
- Modify: `alacritree/Cargo.toml` (add `gethostname`)
- Test: in-module `#[cfg(test)]` in `alacritree/src/osc_tap.rs`

**Interfaces:**
- Consumes: `VtConfig` from Task 1.
- Produces:
  - `enum OscEvent { Cwd(Option<String>), Notify(String), Progress(Progress), PointerShape(egui::CursorIcon) }`
  - `enum Progress { Clear, Set(u8), Error(u8), Indeterminate, Paused(u8) }`
  - `enum ShellPlatform { Unix, Windows }`
  - `struct TapPolicy { vt: VtConfig, hostname: String, shell: ShellPlatform }`
  - `fn classify(params: &[&[u8]], policy: &TapPolicy) -> Option<OscEvent>`
  - `fn local_hostname() -> &'static str`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/osc_tap.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn policy(shell: ShellPlatform) -> TapPolicy {
        TapPolicy {
            vt: VtConfig {
                report_cwd: true,
                notify: true,
                progress: true,
                pointer_shape: true,
            },
            hostname: "LEVPC".to_string(),
            shell,
        }
    }

    fn osc(payload: &[&str], shell: ShellPlatform) -> Option<OscEvent> {
        let owned: Vec<&[u8]> = payload.iter().map(|s| s.as_bytes()).collect();
        classify(&owned, &policy(shell))
    }

    #[test]
    fn osc7_accepts_every_spelling_of_a_local_host() {
        for host in ["", "localhost", "levpc", "LEVPC"] {
            assert_eq!(
                osc(&["7", &format!("file://{host}/home/dev/src")], ShellPlatform::Unix),
                Some(OscEvent::Cwd(Some("/home/dev/src".into()))),
                "host {host:?}",
            );
        }
    }

    #[test]
    fn osc7_rejects_a_foreign_host_and_a_foreign_scheme() {
        assert_eq!(osc(&["7", "file://remote/home/dev"], ShellPlatform::Unix), None);
        assert_eq!(osc(&["7", "http://localhost/home/dev"], ShellPlatform::Unix), None);
    }

    #[test]
    fn osc7_decodes_percent_escapes() {
        assert_eq!(
            osc(&["7", "file:///home/dev/my%20src"], ShellPlatform::Unix),
            Some(OscEvent::Cwd(Some("/home/dev/my src".into()))),
        );
    }

    #[test]
    fn osc7_strips_the_slash_before_a_drive_letter() {
        assert_eq!(
            osc(&["7", "file:///C:/src"], ShellPlatform::Windows),
            Some(OscEvent::Cwd(Some("C:/src".into()))),
        );
    }

    #[test]
    fn osc7_treats_an_empty_payload_as_a_reset() {
        assert_eq!(osc(&["7", ""], ShellPlatform::Unix), Some(OscEvent::Cwd(None)));
    }

    #[test]
    fn a_unc_path_is_refused_by_both_routes() {
        assert_eq!(osc(&["9", "9", r"\\evil\share"], ShellPlatform::Windows), None);
        assert_eq!(osc(&["7", "file:////evil/share"], ShellPlatform::Windows), None);
    }

    #[test]
    fn an_unrooted_path_is_refused() {
        for path in ["src/thing", "~/src", "C:thing"] {
            assert_eq!(
                osc(&["9", "9", path], ShellPlatform::Windows),
                None,
                "path {path:?}",
            );
        }
    }

    #[test]
    fn osc9_9_strips_one_pair_of_quotes_and_keeps_the_remainder() {
        assert_eq!(
            osc(&["9", "9", "\"D:/src\""], ShellPlatform::Windows),
            Some(OscEvent::Cwd(Some("D:/src".into()))),
        );
        // vte split the path on its semicolon; rejoining is what keeps it.
        assert_eq!(
            osc(&["9", "9", "D:/a", "b"], ShellPlatform::Windows),
            Some(OscEvent::Cwd(Some("D:/a;b".into()))),
        );
    }

    #[test]
    fn osc9_tells_a_conemu_subcommand_from_a_notification() {
        assert_eq!(
            osc(&["9", "9 items remaining"], ShellPlatform::Unix),
            Some(OscEvent::Notify("9 items remaining".into())),
        );
        assert_eq!(
            osc(&["9", "build finished"], ShellPlatform::Unix),
            Some(OscEvent::Notify("build finished".into())),
        );
    }

    #[test]
    fn osc777_takes_the_body_after_the_title() {
        assert_eq!(
            osc(&["777", "notify", "cargo", "build failed"], ShellPlatform::Unix),
            Some(OscEvent::Notify("build failed".into())),
        );
    }

    #[test]
    fn osc9_4_follows_windows_terminals_bounds() {
        let unix = ShellPlatform::Unix;
        assert_eq!(osc(&["9", "4", "0"], unix), Some(OscEvent::Progress(Progress::Clear)));
        assert_eq!(osc(&["9", "4", "1", "50"], unix), Some(OscEvent::Progress(Progress::Set(50))));
        assert_eq!(osc(&["9", "4", "1", "900"], unix), Some(OscEvent::Progress(Progress::Set(100))));
        assert_eq!(osc(&["9", "4", "3"], unix), Some(OscEvent::Progress(Progress::Indeterminate)));
        assert_eq!(osc(&["9", "4", ""], unix), Some(OscEvent::Progress(Progress::Clear)));
        assert_eq!(osc(&["9", "4", "5"], unix), None);
    }

    #[test]
    fn osc22_maps_known_names_and_ignores_the_rest() {
        assert_eq!(
            osc(&["22", "pointer"], ShellPlatform::Unix),
            Some(OscEvent::PointerShape(egui::CursorIcon::PointingHand)),
        );
        assert_eq!(osc(&["22", "no-such-cursor"], ShellPlatform::Unix), None);
    }

    #[test]
    fn a_disabled_key_silences_its_sequence() {
        let mut p = policy(ShellPlatform::Unix);
        p.vt.report_cwd = false;
        let owned: Vec<&[u8]> = vec![b"7", b"file:///home/dev"];
        assert_eq!(classify(&owned, &p), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `mod osc_tap;` to `alacritree/src/main.rs` beside the other module declarations, then:

Run: `devrun task test -- osc_tap`
Expected: FAIL to compile, `cannot find type TapPolicy in this scope`.

- [ ] **Step 3: Write the implementation**

Add above the test module in `alacritree/src/osc_tap.rs`:

```rust
//! The OSC sequences `vte`'s `ansi` layer recognises and drops, read off a
//! copy of the PTY byte stream.
//!
//! Everything here is a pure decision about bytes.  The thread that feeds it
//! and the session that consumes its output live elsewhere, so every rule
//! below is testable without a PTY.

use std::sync::OnceLock;

use crate::config::VtConfig;

/// What the shell on the other end of the PTY spells a path like.  A WSL
/// session on Windows is `Unix`: the payload is a Linux path, and the
/// translation to a Windows one happens later, against the session's distro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPlatform {
    Unix,
    Windows,
}

/// The ConEmu progress states, already bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Clear,
    Set(u8),
    Error(u8),
    Indeterminate,
    Paused(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    /// `None` clears the reported directory.  The path is still the shell's
    /// own spelling: WSL translation needs the session's distro.
    Cwd(Option<String>),
    Notify(String),
    Progress(Progress),
    PointerShape(egui::CursorIcon),
}

pub struct TapPolicy {
    pub vt: VtConfig,
    pub hostname: String,
    pub shell: ShellPlatform,
}

/// This machine's name, resolved once.  An empty string when the lookup
/// fails, which leaves only an empty host and `localhost` counting as local.
pub fn local_hostname() -> &'static str {
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME.get_or_init(|| gethostname::gethostname().to_string_lossy().into_owned())
}

pub fn classify(params: &[&[u8]], policy: &TapPolicy) -> Option<OscEvent> {
    match params.first().copied()? {
        b"7" => {
            if !policy.vt.report_cwd {
                return None;
            }
            let payload = rejoin(params, 1)?;
            if payload.is_empty() {
                return Some(OscEvent::Cwd(None));
            }
            let path = file_url_path(&payload, &policy.hostname)?;
            Some(OscEvent::Cwd(Some(rooted(&path, policy.shell)?)))
        },
        b"9" => classify_osc9(params, policy),
        b"777" => {
            if !policy.vt.notify {
                return None;
            }
            // `777;notify;<title>;<body>`; the body may hold semicolons.
            if params.get(1).copied()? != b"notify" {
                return None;
            }
            let body = rejoin(params, 3).filter(|b| !b.is_empty())?;
            Some(OscEvent::Notify(body))
        },
        b"22" => {
            if !policy.vt.pointer_shape {
                return None;
            }
            cursor_icon(&rejoin(params, 1)?).map(OscEvent::PointerShape)
        },
        _ => None,
    }
}

/// OSC 9 carries two unrelated protocols.  ghostty's `osc9.zig` resolves
/// them by requiring a ConEmu subcommand to be a digit run followed by a
/// separator, so `9;9 items remaining` stays a notification.
fn classify_osc9(params: &[&[u8]], policy: &TapPolicy) -> Option<OscEvent> {
    let has_separator = params.len() > 2;
    match (params.get(1).copied(), has_separator) {
        (Some(b"4"), _) => {
            if !policy.vt.progress {
                return None;
            }
            progress(params).map(OscEvent::Progress)
        },
        (Some(b"9"), true) => {
            if !policy.vt.report_cwd {
                return None;
            }
            let raw = rejoin(params, 2)?;
            let path = unquote(&raw);
            Some(OscEvent::Cwd(Some(rooted(path, policy.shell)?)))
        },
        _ => {
            if !policy.vt.notify {
                return None;
            }
            let body = rejoin(params, 1).filter(|b| !b.is_empty())?;
            Some(OscEvent::Notify(body))
        },
    }
}

/// Windows Terminal's `DoConEmuAction` sets these bounds: a state above the
/// highest defined one rejects the whole sequence, progress above a hundred
/// clamps, and an absent or empty state means zero.
fn progress(params: &[&[u8]]) -> Option<Progress> {
    let field = |i: usize| -> Option<u8> {
        let raw = params.get(i).copied()?;
        if raw.is_empty() {
            return Some(0);
        }
        std::str::from_utf8(raw).ok()?.parse::<u32>().ok().map(|n| n.min(100) as u8)
    };
    let state = field(2).unwrap_or(0);
    let value = field(3).unwrap_or(0);
    match state {
        0 => Some(Progress::Clear),
        1 => Some(Progress::Set(value)),
        2 => Some(Progress::Error(value)),
        3 => Some(Progress::Indeterminate),
        4 => Some(Progress::Paused(value)),
        _ => None,
    }
}

/// vte splits OSC parameters on semicolons, so any payload that may legally
/// contain one is put back together.
fn rejoin(params: &[&[u8]], from: usize) -> Option<String> {
    let parts = params.get(from..)?;
    let joined =
        parts.iter().map(|p| String::from_utf8_lossy(p)).collect::<Vec<_>>().join(";");
    Some(joined)
}

/// ConEmu documents the 9;9 payload as a quoted string.  Windows Terminal
/// strips the quotes when both are present and parses the bare string when
/// they are not, and says in a comment that ConEmu does the same.
fn unquote(raw: &str) -> &str {
    raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(raw)
}

/// The host check filters shells that honestly name a remote host, which is
/// the common accident.  It stops nobody writing bytes with intent, who
/// simply writes `localhost`, so nothing downstream may lean on it.
fn file_url_path(url: &str, hostname: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let local = host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || (!hostname.is_empty() && host.eq_ignore_ascii_case(hostname));
    if !local {
        return None;
    }
    let decoded = percent_decode(path)?;
    // `/C:/src` is how a Windows path travels in a file URL.
    let trimmed = match decoded.strip_prefix('/') {
        Some(r) if is_drive_rooted(r) => r.to_string(),
        _ => decoded,
    };
    Some(trimmed)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = (hex[0] as char).to_digit(16)?;
            let lo = (hex[1] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn is_drive_rooted(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(c), Some(':'), Some('/' | '\\')) if c.is_ascii_alphabetic()
    )
}

/// The guard the rest of the design rests on.  A path beginning `\\` or `//`
/// is UNC on Windows: touching it with `is_dir` opens an SMB connection to a
/// host the payload named, and the payload need not come from a remote shell
/// at all, since `cat` of a downloaded file reaches the PTY the same way.
/// Anything not rooted for the shell's own platform is dropped with it.
fn rooted(path: &str, shell: ShellPlatform) -> Option<String> {
    if path.starts_with("\\\\") || path.starts_with("//") {
        return None;
    }
    let ok = match shell {
        ShellPlatform::Unix => path.starts_with('/'),
        ShellPlatform::Windows => is_drive_rooted(path),
    };
    ok.then(|| path.to_string())
}

/// The xterm pointer names alacritree has a cursor for.  An unknown name
/// leaves the current shape alone: not understanding a request is not a
/// request to reset.
fn cursor_icon(name: &str) -> Option<egui::CursorIcon> {
    Some(match name {
        "default" | "left_ptr" | "arrow" => egui::CursorIcon::Default,
        "pointer" | "hand" | "hand2" => egui::CursorIcon::PointingHand,
        "text" | "xterm" | "ibeam" => egui::CursorIcon::Text,
        "crosshair" | "cross" => egui::CursorIcon::Crosshair,
        "wait" | "watch" => egui::CursorIcon::Wait,
        "progress" => egui::CursorIcon::Progress,
        "help" | "question_arrow" => egui::CursorIcon::Help,
        "move" | "fleur" => egui::CursorIcon::Move,
        "not-allowed" | "crossed_circle" => egui::CursorIcon::NotAllowed,
        _ => return None,
    })
}
```

Add to `alacritree/Cargo.toml` under `[dependencies]`:

```toml
# This machine's name, for the OSC 7 host check.  Already in the lock file
# through the notification backends, so it costs no new compilation.
gethostname = "1"
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `devrun task test -- osc_tap`
Expected: PASS, thirteen tests.

- [ ] **Step 5: Lint and format**

```bash
devrun task fmt
devrun task clippy
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/osc_tap.rs alacritree/src/main.rs alacritree/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(vt): add the OSC classification seam

One pure function turns vte's raw OSC parameters into an event, so every
protocol decision is testable without a PTY or a thread.  Carries the host
check, the rooted-path guard that refuses UNC payloads, and the ConEmu
versus iTerm2 split on OSC 9.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: The tee

The PTY wrapper. Its job is to copy bytes and hand them off without disturbing them.

**Files:**
- Create: `alacritree/src/pty_tee.rs`
- Modify: `alacritree/src/main.rs` (add `mod pty_tee;`)
- Test: in-module `#[cfg(test)]` in `alacritree/src/pty_tee.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `struct Chunk { pub bytes: Vec<u8>, pub gap_before: bool }`
  - `struct TapHandle` with `fn new(tx: SyncSender<Chunk>, pool: Receiver<Vec<u8>>) -> Self` and `fn offer(&mut self, filled: &[u8])`
  - `struct TeePty<P>` with `fn new(inner: P, tap: Option<TapHandle>) -> Self`
  - `struct TeeReader<R>` with `fn new(inner: R, tap: Option<TapHandle>) -> Self`
  - `const QUEUE_DEPTH: usize`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/pty_tee.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::sync::mpsc;

    use super::*;

    /// A reader that hands out short reads, the way a PTY does.
    struct Choppy {
        data: Vec<u8>,
        at: usize,
        step: usize,
    }

    impl Read for Choppy {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let end = (self.at + self.step).min(self.data.len());
            let n = (end - self.at).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    fn drain_all(mut reader: impl Read) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 64];
        loop {
            match reader.read(&mut buf).unwrap() {
                0 => return out,
                n => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    #[test]
    fn every_byte_reaches_the_reader_unchanged() {
        let data: Vec<u8> = (0..10_000u32).map(|n| (n % 251) as u8).collect();
        let (tx, rx) = mpsc::sync_channel(QUEUE_DEPTH);
        let (_pool_tx, pool_rx) = mpsc::channel();
        let reader = TeeReader::new(
            Choppy { data: data.clone(), at: 0, step: 7 },
            Some(TapHandle::new(tx, pool_rx)),
        );

        assert_eq!(drain_all(reader), data);
        let seen: Vec<u8> = rx.try_iter().flat_map(|c| c.bytes).collect();
        assert_eq!(seen, data);
    }

    #[test]
    fn a_full_queue_drops_a_chunk_and_flags_the_gap() {
        let (tx, rx) = mpsc::sync_channel(1);
        let (_pool_tx, pool_rx) = mpsc::channel();
        let mut tap = TapHandle::new(tx, pool_rx);

        tap.offer(b"first");
        tap.offer(b"lost");
        tap.offer(b"after");

        let first = rx.recv().unwrap();
        assert_eq!(first.bytes, b"first");
        assert!(!first.gap_before);

        let after = rx.recv().unwrap();
        assert_eq!(after.bytes, b"after");
        assert!(after.gap_before, "the drop has to be announced");
    }

    #[test]
    fn a_returned_buffer_is_reused_instead_of_allocated() {
        let (tx, rx) = mpsc::sync_channel(QUEUE_DEPTH);
        let (pool_tx, pool_rx) = mpsc::channel();
        let mut tap = TapHandle::new(tx, pool_rx);

        tap.offer(b"first");
        let mut recycled = rx.recv().unwrap().bytes;
        recycled.clear();
        recycled.reserve(4096);
        let addr = recycled.as_ptr();
        pool_tx.send(recycled).unwrap();

        tap.offer(b"second");
        let next = rx.recv().unwrap();
        assert_eq!(next.bytes, b"second");
        assert_eq!(next.bytes.as_ptr(), addr, "the pooled allocation was not reused");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `mod pty_tee;` to `alacritree/src/main.rs`, then:

Run: `devrun task test -- pty_tee`
Expected: FAIL to compile, `cannot find type TeeReader in this scope`.

- [ ] **Step 3: Write the implementation**

Add above the test module in `alacritree/src/pty_tee.rs`:

```rust
//! Copies the PTY byte stream on its way through, so a second parser can see
//! the OSC sequences `vte`'s `ansi` layer drops.
//!
//! The copy is all this layer does.  Parsing on the read thread costs up to
//! the terminal's own parse again once escape sequences arrive every few
//! bytes, which is exactly when a TUI is repainting, so the bytes go to a
//! thread of their own instead.  Buffers cycle through a return channel
//! rather than being allocated per read.

use std::io::{self, Read};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};

/// How many reads may be in flight before the tap starts dropping them.
/// Deep enough to absorb a burst, shallow enough that a tap thread which
/// cannot keep up costs bounded memory rather than growing memory.
pub const QUEUE_DEPTH: usize = 64;

/// One read's worth of bytes on its way to the tap.
pub struct Chunk {
    pub bytes: Vec<u8>,
    /// Set when the queue was full and a read was dropped before this one.
    /// A hole in the stream makes framing meaningless, so the tap resets.
    pub gap_before: bool,
}

/// The read thread's half of the tap.
pub struct TapHandle {
    tx: SyncSender<Chunk>,
    pool: Receiver<Vec<u8>>,
    dropped: bool,
}

impl TapHandle {
    pub fn new(tx: SyncSender<Chunk>, pool: Receiver<Vec<u8>>) -> Self {
        Self { tx, pool, dropped: false }
    }

    /// Never blocks and never fails the read it was called from.
    pub fn offer(&mut self, filled: &[u8]) {
        let mut bytes = self.pool.try_recv().unwrap_or_default();
        bytes.clear();
        bytes.extend_from_slice(filled);
        let chunk = Chunk { bytes, gap_before: self.dropped };
        match self.tx.try_send(chunk) {
            Ok(()) => self.dropped = false,
            Err(TrySendError::Full(_)) => {
                if !self.dropped {
                    log::debug!("osc tap fell behind; dropping reads until it catches up");
                }
                self.dropped = true;
            },
            // The tap thread is gone.  Reads carry on without it.
            Err(TrySendError::Disconnected(_)) => self.dropped = true,
        }
    }
}

pub struct TeeReader<R> {
    inner: R,
    tap: Option<TapHandle>,
}

impl<R: Read> TeeReader<R> {
    pub fn new(inner: R, tap: Option<TapHandle>) -> Self {
        Self { inner, tap }
    }
}

impl<R: Read> Read for TeeReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            if let Some(tap) = self.tap.as_mut() {
                tap.offer(&buf[..n]);
            }
        }
        Ok(n)
    }
}
```

Then add `TeePty<P>`. `EventedReadWrite::reader` returns `&mut Self::Reader`, so `TeePty` cannot own the inner PTY and hand out a reader borrowed from it at the same time. `pty_rearm.rs` already solves this exact problem: read its `RearmingPty` struct comment, which explains the arrangement, and mirror it by taking the inner reader out at construction.

Implement `EventedReadWrite` for `TeePty<P>` with `type Reader = TeeReader<P::Reader>` and `type Writer = P::Writer`, forwarding `register`, `reregister`, `deregister` and `writer` to the inner PTY. Implement `EventedPty` by forwarding `next_child_event`, and `OnResize` by forwarding `on_resize`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `devrun task test -- pty_tee`
Expected: PASS, three tests.

- [ ] **Step 5: Verify the whole suite and lints**

```bash
devrun task check
devrun task test
devrun task clippy
devrun task fmt
```

Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/pty_tee.rs alacritree/src/main.rs
git commit -m "$(cat <<'EOF'
feat(vt): copy the PTY stream to a tap channel

A wrapper around the PTY reader hands each read to a bounded channel with
pooled buffers, so the read thread pays a copy rather than a second VT
parse.  A full queue drops the read and flags the gap, keeping a tap that
falls behind bounded in memory rather than in output.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: The tap thread and its wiring

Connects Task 3 to Task 4 and delivers events to the session.

**Files:**
- Modify: `alacritree/src/osc_tap.rs` (add the thread and its parser)
- Modify: `alacritree/src/session.rs` (spawn path, `Session` field, `drain_events`)
- Test: in-module `#[cfg(test)]` in `alacritree/src/osc_tap.rs`

**Interfaces:**
- Consumes: `classify`, `TapPolicy`, `OscEvent` from Task 3. `Chunk`, `TapHandle`, `TeePty`, `QUEUE_DEPTH` from Task 4.
- Produces:
  - `fn spawn(policy: TapPolicy, ctx: egui::Context) -> Option<(TapHandle, Receiver<OscEvent>)>`
  - `fn feed(parser: &mut vte::Parser, sink: &mut Sink, chunk: &Chunk)`
  - `const MAX_PENDING: usize`
  - `Session::osc_events: Option<Receiver<OscEvent>>`

- [ ] **Step 1: Write the failing tests**

Add to the test module in `alacritree/src/osc_tap.rs`:

```rust
    fn chunk(bytes: &[u8], gap_before: bool) -> crate::pty_tee::Chunk {
        crate::pty_tee::Chunk { bytes: bytes.to_vec(), gap_before }
    }

    #[test]
    fn a_sequence_split_across_chunks_still_dispatches() {
        let mut parser = vte::Parser::new();
        let mut sink = Sink::collecting(policy(ShellPlatform::Unix));

        feed(&mut parser, &mut sink, &chunk(b"\x1b]7;file:///home/", false));
        assert!(sink.take().is_empty(), "nothing is complete yet");

        feed(&mut parser, &mut sink, &chunk(b"dev/src\x1b\\", false));
        assert_eq!(sink.take(), vec![OscEvent::Cwd(Some("/home/dev/src".into()))]);
    }

    #[test]
    fn a_gap_discards_the_sequence_it_interrupted() {
        let mut parser = vte::Parser::new();
        let mut sink = Sink::collecting(policy(ShellPlatform::Unix));

        feed(&mut parser, &mut sink, &chunk(b"\x1b]7;file:///home/", false));
        feed(&mut parser, &mut sink, &chunk(b"dev/src\x1b\\", true));
        assert!(sink.take().is_empty(), "a hole in the stream must not be framed across");

        feed(&mut parser, &mut sink, &chunk(b"\x1b]7;file:///tmp\x1b\\", false));
        assert_eq!(sink.take(), vec![OscEvent::Cwd(Some("/tmp".into()))]);
    }

    #[test]
    fn an_unterminated_sequence_does_not_grow_without_bound() {
        let mut parser = vte::Parser::new();
        let mut sink = Sink::collecting(policy(ShellPlatform::Unix));

        feed(&mut parser, &mut sink, &chunk(b"\x1b]7;", false));
        let flood = vec![b'x'; MAX_PENDING + 1];
        feed(&mut parser, &mut sink, &chunk(&flood, false));

        // The parser reset, so a well-formed sequence still works after it.
        feed(&mut parser, &mut sink, &chunk(b"\x1b]7;file:///tmp\x1b\\", false));
        assert_eq!(sink.take(), vec![OscEvent::Cwd(Some("/tmp".into()))]);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devrun task test -- osc_tap`
Expected: FAIL to compile, `cannot find type Sink in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `alacritree/src/osc_tap.rs`:

```rust
/// How many bytes one sequence may occupy before the parser is reset.
/// `vte` buffers an OSC payload in a `Vec` with no cap under the `std`
/// feature, so an unterminated sequence would otherwise grow until memory
/// ran out.  `Term`'s own parser carries that exposure already, and the tap
/// does not add a second one.
pub const MAX_PENDING: usize = 1 << 20;

/// Where recognised events go.  A channel in production, a vector under
/// test, so the tested type is the shipped type.
enum Destination {
    Channel { tx: std::sync::mpsc::Sender<OscEvent>, ctx: egui::Context },
    Collected(Vec<OscEvent>),
}

pub struct Sink {
    policy: TapPolicy,
    destination: Destination,
    pending: usize,
}

impl Sink {
    fn channel(
        policy: TapPolicy,
        tx: std::sync::mpsc::Sender<OscEvent>,
        ctx: egui::Context,
    ) -> Self {
        Self { policy, destination: Destination::Channel { tx, ctx }, pending: 0 }
    }

    #[cfg(test)]
    fn collecting(policy: TapPolicy) -> Self {
        Self { policy, destination: Destination::Collected(Vec::new()), pending: 0 }
    }

    #[cfg(test)]
    fn take(&mut self) -> Vec<OscEvent> {
        match &mut self.destination {
            Destination::Collected(events) => std::mem::take(events),
            Destination::Channel { .. } => unreachable!("collecting sink only"),
        }
    }

    /// The repaint is what wakes the frame loop.  Without it a quiet PTY
    /// leaves the event sitting in the channel until the next keystroke,
    /// which is the contract `EventProxy::send_event` already keeps.
    fn emit(&mut self, event: OscEvent) {
        match &mut self.destination {
            Destination::Channel { tx, ctx } => {
                if tx.send(event).is_ok() {
                    ctx.request_repaint();
                }
            },
            Destination::Collected(events) => events.push(event),
        }
    }
}

impl vte::Perform for Sink {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        self.pending = 0;
        if let Some(event) = classify(params, &self.policy) {
            self.emit(event);
        }
    }
}

/// Feeds one chunk, resetting first when the stream had a hole in it.
pub fn feed(parser: &mut vte::Parser, sink: &mut Sink, chunk: &crate::pty_tee::Chunk) {
    if chunk.gap_before {
        *parser = vte::Parser::new();
        sink.pending = 0;
    }
    sink.pending = sink.pending.saturating_add(chunk.bytes.len());
    parser.advance(sink, &chunk.bytes);
    if sink.pending > MAX_PENDING {
        *parser = vte::Parser::new();
        sink.pending = 0;
    }
}

/// Starts a parser thread for one session.  `None` when no sequence is
/// wanted, and also when the thread cannot be started: a session must open
/// whether or not a diagnostic feature does.
pub fn spawn(
    policy: TapPolicy,
    ctx: egui::Context,
) -> Option<(crate::pty_tee::TapHandle, std::sync::mpsc::Receiver<OscEvent>)> {
    if !policy.vt.any_enabled() {
        return None;
    }
    let (chunk_tx, chunk_rx) = std::sync::mpsc::sync_channel(crate::pty_tee::QUEUE_DEPTH);
    let (pool_tx, pool_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = std::sync::mpsc::channel();

    let started =
        std::thread::Builder::new().name("alacritree-osc-tap".into()).spawn(move || {
            let mut parser = vte::Parser::new();
            let mut sink = Sink::channel(policy, event_tx, ctx);
            while let Ok(chunk) = chunk_rx.recv() {
                feed(&mut parser, &mut sink, &chunk);
                // Recycling is best effort; a closed pool just drops.
                let _ = pool_tx.send(chunk.bytes);
            }
        });

    match started {
        Ok(_) => Some((crate::pty_tee::TapHandle::new(chunk_tx, pool_rx), event_rx)),
        Err(e) => {
            log::debug!("osc tap thread did not start: {e}");
            None
        },
    }
}
```

In `alacritree/src/session.rs`, in the function that builds the PTY, after the existing Windows `RearmingPty` wrap:

```rust
    // A WSL session's shell spells paths the Linux way even on Windows.
    let shell_platform = if wsl_distro.is_some() || cfg!(unix) {
        osc_tap::ShellPlatform::Unix
    } else {
        osc_tap::ShellPlatform::Windows
    };
    let policy = osc_tap::TapPolicy {
        vt: config.vt,
        hostname: osc_tap::local_hostname().to_string(),
        shell: shell_platform,
    };
    let (tap, osc_events) = match osc_tap::spawn(policy, ctx.clone()) {
        Some((tap, rx)) => (Some(tap), Some(rx)),
        None => (None, None),
    };
    let pty = crate::pty_tee::TeePty::new(pty, tap);
```

Add `pub osc_events: Option<mpsc::Receiver<OscEvent>>` to `Session`, `None` for scratchpad sessions, and drain it at the end of `drain_events`:

```rust
        // Taken out of the field so the match arms in later tasks can hold
        // `&mut self` while the receiver is borrowed.
        if let Some(rx) = self.osc_events.take() {
            while let Ok(event) = rx.try_recv() {
                let _ = event;
            }
            self.osc_events = Some(rx);
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `devrun task test -- osc_tap`
Expected: PASS, sixteen tests.

- [ ] **Step 5: Verify nothing regressed**

```bash
devrun task check
devrun task test
devrun task clippy
devrun task fmt
```

Expected: all PASS. In particular the existing `session.rs` PTY tests must still pass, since every session now reads through the tee.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/osc_tap.rs alacritree/src/session.rs
git commit -m "$(cat <<'EOF'
feat(vt): parse tapped OSC sequences on their own thread

One thread per session owns a vte parser fed from the tee, turning
recognised sequences into events the frame loop drains beside the
terminal's own.  A gap resets the parser, and an unterminated sequence
past a ceiling resets it too, since vte's OSC buffer has no cap with std.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Working directory reporting

**Files:**
- Modify: `alacritree/src/session.rs` (`Session::reported_cwd`, the drain arm)
- Modify: `alacritree/src/app.rs` (row hover text, sibling spawn)
- Test: in-module `#[cfg(test)]` in `alacritree/src/session.rs`

**Interfaces:**
- Consumes: `OscEvent::Cwd` from Task 3, the drain from Task 5.
- Produces: `Session::reported_cwd: Option<PathBuf>`, `fn resolve_reported_cwd(path: &str, distro: Option<&str>) -> Option<PathBuf>`, `fn spawn_directory(reported: Option<&PathBuf>, workspace: Option<&PathBuf>) -> Option<PathBuf>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_reported_cwd_is_translated_for_a_wsl_session() {
    assert_eq!(
        resolve_reported_cwd("/home/dev/src", Some("Ubuntu")),
        Some(crate::wsl::linux_to_windows("/home/dev/src", "Ubuntu")),
    );
}

#[test]
fn a_reported_cwd_is_taken_as_given_without_a_distro() {
    assert_eq!(
        resolve_reported_cwd("/home/dev/src", None),
        Some(PathBuf::from("/home/dev/src")),
    );
}

#[test]
fn the_spawn_directory_falls_back_when_the_reported_path_is_not_here() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();

    assert_eq!(
        spawn_directory(Some(&PathBuf::from("/nowhere/at/all")), Some(&workspace)),
        Some(workspace.clone()),
    );
    assert_eq!(spawn_directory(Some(&workspace), Some(&workspace)), Some(workspace));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devrun task test -- reported_cwd`
Expected: FAIL to compile, `cannot find function resolve_reported_cwd`.

- [ ] **Step 3: Write the implementation**

In `alacritree/src/session.rs`, add to `Session`:

```rust
    /// Where the shell says it is.  Deliberately not `working_directory`,
    /// which is the sidebar's workspace key: writing a report into that
    /// would re-home a session on every `cd`.
    ///
    /// Anything that writes to the PTY can set this, local or remote, so
    /// every consumer guards at use.
    pub reported_cwd: Option<PathBuf>,
```

and beside it:

```rust
/// A WSL session's payload is a Linux path.  The distro comes from the
/// session rather than the sequence, which is why the resulting UNC path is
/// trustworthy where one built from the payload would not be.
fn resolve_reported_cwd(path: &str, distro: Option<&str>) -> Option<PathBuf> {
    Some(match distro {
        Some(distro) => crate::wsl::linux_to_windows(path, distro),
        None => PathBuf::from(path),
    })
}

/// The reported directory when it exists here, the workspace otherwise.  One
/// stat, taken when the user asks for a session rather than on every `cd`.
fn spawn_directory(reported: Option<&PathBuf>, workspace: Option<&PathBuf>) -> Option<PathBuf> {
    reported.filter(|p| p.is_dir()).or(workspace).cloned()
}
```

Handle the event in the drain loop added by Task 5:

```rust
                    OscEvent::Cwd(path) => {
                        let distro = self.wsl_distro().map(str::to_string);
                        self.reported_cwd =
                            path.and_then(|p| resolve_reported_cwd(&p, distro.as_deref()));
                    },
```

In `alacritree/src/app.rs`, add the reported path to the session row's hover text beside the existing title, and route the "new session here" action through `spawn_directory`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `devrun task test -- session::tests`
Expected: PASS.

- [ ] **Step 5: Check the steady-state assertion**

```bash
devrun task test -- steady_state
```

Expected: PASS. If the reported path reached the sidebar snapshot and made the unchanged-frame reconcile allocate, this is the test that says so. Fix by keeping the path out of the snapshot's diff key, since hover text does not drive cursor repair.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/session.rs alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(vt): track where the shell actually is

OSC 7 and OSC 9;9 land on a field of their own rather than on
working_directory, which is the sidebar's workspace key.  The row's hover
shows it, and a sibling session starts there when the path exists here.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Notifications

**Files:**
- Modify: `alacritree/src/session.rs` (`DrainOutcome`, `Session::last_notification`, the drain arm)
- Modify: `alacritree/src/app.rs` (the attention block, `notify_attention`)
- Test: in-module `#[cfg(test)]` in `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `OscEvent::Notify` from Task 3, the drain from Task 5.
- Produces: `DrainOutcome::notification: Option<String>`, `Session::last_notification: Option<String>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_visible_session_keeps_notification_text_it_gets_no_toast_for() {
    let mut app = notification_app();
    app.deliver_notification(0, "build failed", Visibility::VisibleAndFocused);

    assert_eq!(app.sessions[0].last_notification.as_deref(), Some("build failed"));
    assert_eq!(app.toasts_fired(), 0, "you are looking at it");
}

#[test]
fn two_notifications_to_a_latched_session_toast_twice() {
    let mut app = notification_app();
    app.deliver_notification(0, "build started", Visibility::Hidden);
    app.deliver_notification(0, "build failed", Visibility::Hidden);

    assert_eq!(app.toasts_fired(), 2);
    assert_eq!(app.sessions[0].last_notification.as_deref(), Some("build failed"));
}

#[test]
fn two_bells_to_a_latched_session_still_toast_once() {
    let mut app = notification_app();
    app.deliver_bell(0, Visibility::Hidden);
    app.deliver_bell(0, Visibility::Hidden);

    assert_eq!(app.toasts_fired(), 1, "the latch is what stops a bell storm");
}
```

`notification_app`, `deliver_notification`, `deliver_bell` and `toasts_fired` are test helpers to write in the same module. Model them on the existing `herdr_lifecycle_app` helper in `alacritree/src/app.rs`, and count toasts by putting a counter behind the call site rather than invoking the platform notifier.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devrun task test -- notification`
Expected: FAIL to compile, `no field last_notification`.

- [ ] **Step 3: Store the text outside the attention path**

In `alacritree/src/session.rs`, add to `DrainOutcome`:

```rust
    /// Text from OSC 9 or OSC 777.  Present means the trigger was an
    /// application asking, not a bell or a title inferred to need one.
    pub notification: Option<String>,
```

and to `Session`:

```rust
    /// The most recent notification body.  Written in the drain, ahead of
    /// every visibility test, so a session you are looking at keeps the text
    /// even though no toast fires for it.
    pub last_notification: Option<String>,
```

Handle the event in the drain loop:

```rust
                    OscEvent::Notify(body) => {
                        let body = truncate_notification(body);
                        self.last_notification = Some(body.clone());
                        outcome.notification = Some(body);
                        outcome.attention = true;
                    },
```

with, beside it:

```rust
/// Platform notifiers cap what they will show, and a runaway body would
/// otherwise travel through the channel and sit on the session for nothing.
fn truncate_notification(mut body: String) -> String {
    const LIMIT: usize = 512;
    if body.len() > LIMIT {
        let cut = body.char_indices().map(|(i, _)| i).take_while(|i| *i <= LIMIT).last();
        body.truncate(cut.unwrap_or(0));
    }
    body
}
```

- [ ] **Step 4: Let an explicit notification through the latch**

In `alacritree/src/app.rs`, change the `Fire` arm:

```rust
                AttentionVerdict::Fire => {
                    self.sessions[idx].pending_attention = None;
                    let was_attending = self.sessions[idx].needs_attention;
                    self.sessions[idx].needs_attention = true;
                    // A bell and a title transition in one idle cycle are one
                    // event, which is what the latch is for.  Two OSC 9
                    // sequences are two messages.
                    let explicit = outcome.notification.is_some();
                    if (!was_attending || explicit) && self.config.ui.notifications {
                        notify_attention(&self.sessions[idx], ctx);
                    }
                },
```

and have `notify_attention` prefer `session.last_notification` over the body it derives from the working directory.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `devrun task test -- notification`
Expected: PASS, three tests.

- [ ] **Step 6: Verify and commit**

```bash
devrun task test
devrun task clippy
devrun task fmt
git add alacritree/src/session.rs alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(vt): surface OSC 9 and OSC 777 notifications

The attention pipeline assumed every trigger was payload-free.  The body is
now stored in the drain, ahead of the visibility test that would discard
it, and an explicit notification passes the transition latch so two
messages produce two toasts where two bells still produce one.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Progress

**Files:**
- Modify: `alacritree/src/session.rs` (`Session::progress`, the drain arm)
- Modify: `alacritree/src/app.rs` (`session_row`)
- Test: in-module `#[cfg(test)]` in `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `OscEvent::Progress` and `Progress` from Task 3.
- Produces: `Session::progress: Option<Progress>`, `enum ProgressTone { Normal, Error, Paused }`, `fn progress_bar_fill(progress: Progress, width: f32) -> f32`, `fn progress_tone(progress: Progress) -> ProgressTone`.

- [ ] **Step 1: Write the failing test**

`Theme` derives only `Clone, Copy`, so a test cannot build one. The tone is returned instead of a colour and the call site maps it, which keeps the decision testable and the palette knowledge where the rest of it lives.

```rust
#[test]
fn a_progress_bar_fills_and_tones_by_state() {
    assert_eq!(progress_bar_fill(Progress::Set(50), 100.0), 50.0);
    assert_eq!(progress_bar_fill(Progress::Indeterminate, 100.0), 100.0);
    assert_eq!(progress_bar_fill(Progress::Clear, 100.0), 0.0);

    assert_eq!(progress_tone(Progress::Set(10)), ProgressTone::Normal);
    assert_eq!(progress_tone(Progress::Error(10)), ProgressTone::Error);
    assert_eq!(progress_tone(Progress::Paused(10)), ProgressTone::Paused);
    assert_eq!(progress_tone(Progress::Indeterminate), ProgressTone::Normal);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `devrun task test -- a_progress_bar_fills_and_tones_by_state`
Expected: FAIL to compile, `cannot find function progress_bar_fill`.

- [ ] **Step 3: Write the implementation**

Add `pub progress: Option<Progress>` to `Session`, set from the drain arm:

```rust
                    OscEvent::Progress(progress) => self.progress = Some(progress),
```

and in `alacritree/src/app.rs`:

```rust
/// Indeterminate fills the bar whole rather than animating: the row is a few
/// pixels tall and a moving bar there reads as noise.
fn progress_bar_fill(progress: Progress, width: f32) -> f32 {
    let fraction = match progress {
        Progress::Clear => 0.0,
        Progress::Set(p) | Progress::Error(p) | Progress::Paused(p) => f32::from(p) / 100.0,
        Progress::Indeterminate => 1.0,
    };
    width * fraction
}

/// How the bar reads, decided here so the palette lookup stays at the call
/// site with the rest of the theme's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressTone {
    Normal,
    Error,
    Paused,
}

fn progress_tone(progress: Progress) -> ProgressTone {
    match progress {
        Progress::Error(_) => ProgressTone::Error,
        Progress::Paused(_) => ProgressTone::Paused,
        _ => ProgressTone::Normal,
    }
}
```

Draw it in `session_row` as a thin filled rect along the bottom of the row rect, skipped entirely when `progress` is `None` or `Progress::Clear`. Map the tone there:

```rust
            let color = match progress_tone(progress) {
                ProgressTone::Error => theme.attention,
                ProgressTone::Paused => theme.text_muted,
                ProgressTone::Normal => theme.accent,
            };
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `devrun task test -- a_progress_bar_fills_and_tones_by_state`
Expected: PASS.

- [ ] **Step 5: Verify and commit**

```bash
devrun task test
devrun task clippy
devrun task fmt
git add alacritree/src/session.rs alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(vt): draw the ConEmu progress report in the sidebar

OSC 9;4 becomes a bar across the session's row, with error and paused
carried by colour and indeterminate filling it whole.  Needs no addition
to the baked glyph set.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Pointer shape

**Files:**
- Modify: `alacritree/src/session.rs` (`Session::pointer_shape`, the drain arm)
- Modify: `alacritree/src/terminal_view.rs`
- Test: in-module `#[cfg(test)]` in `alacritree/src/terminal_view.rs`

**Interfaces:**
- Consumes: `OscEvent::PointerShape` from Task 3.
- Produces: `Session::pointer_shape: Option<egui::CursorIcon>`, `fn grid_cursor(requested: Option<egui::CursorIcon>, over_link: bool) -> egui::CursorIcon`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_grid_cursor_prefers_what_the_application_asked_for() {
    assert_eq!(
        grid_cursor(Some(egui::CursorIcon::Crosshair), false),
        egui::CursorIcon::Crosshair,
    );
    assert_eq!(grid_cursor(None, false), egui::CursorIcon::Text);
    assert_eq!(
        grid_cursor(Some(egui::CursorIcon::Crosshair), true),
        egui::CursorIcon::PointingHand,
        "a hovered link still wins: it tells the user the click does something",
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `devrun task test -- grid_cursor`
Expected: FAIL to compile, `cannot find function grid_cursor`.

- [ ] **Step 3: Write the implementation**

Add `pub pointer_shape: Option<egui::CursorIcon>` to `Session`, set from the drain arm:

```rust
                    OscEvent::PointerShape(icon) => self.pointer_shape = Some(icon),
```

and in `alacritree/src/terminal_view.rs`:

```rust
/// A hovered link outranks the application's request, because the shape is
/// the only thing telling the user the click will do something.
fn grid_cursor(requested: Option<egui::CursorIcon>, over_link: bool) -> egui::CursorIcon {
    if over_link {
        return egui::CursorIcon::PointingHand;
    }
    requested.unwrap_or(egui::CursorIcon::Text)
}
```

Call it where the view currently sets the cursor over the grid.

- [ ] **Step 4: Run the test to verify it passes**

Run: `devrun task test -- grid_cursor`
Expected: PASS.

- [ ] **Step 5: Verify the whole suite**

```bash
devrun task check
devrun task test
devrun task clippy
devrun task fmt
```

Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/session.rs alacritree/src/terminal_view.rs
git commit -m "$(cat <<'EOF'
feat(vt): let an application choose the pointer shape

vte parses OSC 22 and calls a Handler method Term never overrides, so the
request reached an empty default.  Reading it off the tap gets it to the
egui cursor without touching a vendored crate.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Documentation and the schema's final state

**Files:**
- Modify: `AGENTS.md` (module list)
- Modify: `schema/alacritree-config.json` (generated)

- [ ] **Step 1: Add the two new modules to the architecture list**

In `AGENTS.md`, under the big-picture architecture bullets, beside the `pty_rearm.rs` entry. Match the separator the neighbouring entries use rather than this snippet's colon:

```markdown
- `pty_tee.rs`, `osc_tap.rs`: the OSC sequences `vte`'s `ansi` layer recognises and drops. `pty_tee.rs` wraps the PTY reader and copies each read into a bounded channel with pooled buffers; `osc_tap.rs` runs a second `vte::Parser` on a per-session thread and turns what it recognises into events the frame loop drains. Off unless a `[vt]` key is set, since parsing on the read thread costs up to the terminal's own parse again on escape-dense output.
```

- [ ] **Step 2: Regenerate the schema and run everything**

```bash
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
devrun task fmt
devrun task check
devrun task clippy
devrun task test
```

Expected: all PASS, and `git status` shows `schema/alacritree-config.json` either unchanged or carrying only the `[vt]` and `osc52` additions.

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md schema/alacritree-config.json
git commit -m "$(cat <<'EOF'
docs(vt): describe the OSC tap in the architecture notes

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the executor

- **Task order matters.** Tasks 3 and 4 are independent of each other and both must land before Task 5. Tasks 6 through 9 all depend on Task 5's drain loop. Task 2 depends only on Task 1.
- **The tee's byte-identity test is the one never to weaken.** Every session reads through `TeeReader` once Task 5 lands, so a dropped or duplicated byte there corrupts the grid for every pane.
- **Two claims in the spec are marked unverified** and block nothing: fish interpolating the hostname by default, and WSL defaulting its hostname to the Windows computer name. Neither changes the rooted-path rule.
- **Manual check before opening the PR.** With `[vt] report_cwd = true`, run a shell with an OSC 7 integration and confirm the row hover shows the directory. With `[vt] notify = true`, `printf '\e]9;hello\a'` should raise a toast from a hidden session and none from a visible focused one.
