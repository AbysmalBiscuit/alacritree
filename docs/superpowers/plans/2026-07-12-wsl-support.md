# WSL Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Full feature parity for WSL-resident projects in alacritree — sessions, git status, worktree create/delete, diff panes, PR status — plus WSL shells for Windows-fs projects, per the approved spec at `docs/superpowers/specs/2026-07-12-wsl-support-design.md`.

**Architecture:** One new module `wsl.rs` is the only place that knows WSL exists: distro enumeration (registry, `wsl -l -q` fallback), Windows↔Linux path translation, and `wsl.exe` command construction. Everything else dispatches on `wsl::Location` — the `Windows` arm is today's code untouched; the `Wsl` arm runs real git *inside* the distro via one batched `wsl.exe --exec sh -c` round trip per operation and parses stable porcelain formats. Canonical identity stays the Windows `PathBuf` (UNC for WSL paths) everywhere; Linux paths exist only at the wsl.exe boundary.

**Tech Stack:** Rust (edition 2024, MSRV 1.85), egui/eframe 0.31, alacritty_terminal, git2 (Windows arm only), `winreg` (new, Windows-only dep).

## Global Constraints

- Only the `alacritree/` crate changes. `alacritty*` crates are read-only vendored deps.
- Work happens on branch `feat/wsl-support` in its own worktree off `master` (use superpowers:using-git-worktrees at execution start). PR target: mathix420/alacritree.
- **Never commit** anything under `docs/superpowers/`, `docs/specs/`, `docs/plans/`, or `.worktrees/` — they are in `.git/info/exclude`. Stage files explicitly; never `git add -A`.
- Conventional Commits, imperative, subject ≤ 50 chars (72 hard limit), lowercase after colon.
- `cargo fmt` before every commit (rustfmt enforced via `rustfmt.toml`).
- Comments explain *why*, are timeless, never narrate the change. Match the file-header style of `state.rs`/`config.rs`.
- Zero behavior change on non-Windows builds and for Windows users without WSL: `wsl::distros()` empty + `classify` never returns `Wsl` ⇒ every new code path dormant. No `#[cfg(windows)]` litter at call sites — the stubs live inside `wsl.rs`.
- Never panic on WSL failures: log-and-degrade (empty status, pseudo-worktree, `last_error` toast).
- New config: exactly one option, `[ui.wsl] automount_root` (default `/mnt`) in `alacritree.toml`.
- Test convention: `#[cfg(test)] mod tests` at the bottom of each file (see `pr_status.rs`). Tests touching Windows path prefixes (`\\wsl$\…`, `C:\…`) are `#[cfg(windows)]` — `Path` only parses those prefixes on Windows. Tests that would invoke a real distro are `#[ignore]` (run manually with `cargo test -p alacritree -- --ignored`).
- Verify loop: `cargo check -p alacritree` (fast), `cargo test -p alacritree` (full).

---

### Task 1: `wsl.rs` — Location model and path translation

The pure core: classify a Windows path as Windows-fs or WSL-fs, and translate both directions. No process spawning yet.

**Files:**
- Create: `alacritree/src/wsl.rs`
- Modify: `alacritree/src/main.rs` (add `mod wsl;` to the module list, alphabetical: after `mod terminal_view;`, before `mod worktree;`... actually alphabetical order puts it between `terminal_view` and `worktree`)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces (used by every later task):
  - `pub enum Location { Windows(PathBuf), Wsl { distro: String, linux_path: String } }`
  - `pub fn classify(path: &Path) -> Location`
  - `pub fn linux_to_windows(linux: &str, distro: &str) -> PathBuf` (uses configured automount root)
  - `pub fn windows_to_linux(path: &Path) -> Option<String>` (uses configured automount root)
  - `pub fn set_automount_root(root: String)` (called once from `main`, Task 3)

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/wsl.rs` containing only the module doc, the test module below, and add `mod wsl;` to `main.rs`:

```rust
//! WSL awareness: distro enumeration, Windows ↔ Linux path translation, and
//! `wsl.exe` command construction.  The only module that knows WSL exists —
//! everything else dispatches on `Location` or hands this module argv to
//! wrap.  On non-Windows builds (and Windows without WSL) `distros()` is
//! empty and `classify` never returns `Wsl`, so all WSL code paths are
//! dormant without cfg-gating at call sites.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[cfg(windows)]
    #[test]
    fn classifies_wsl_localhost_unc() {
        let loc = classify(Path::new(r"\\wsl.localhost\kali-linux\home\lev\proj"));
        assert_eq!(loc, Location::Wsl {
            distro: "kali-linux".to_string(),
            linux_path: "/home/lev/proj".to_string(),
        });
    }

    #[cfg(windows)]
    #[test]
    fn classifies_wsl_dollar_unc() {
        let loc = classify(Path::new(r"\\wsl$\Ubuntu\srv"));
        assert_eq!(loc, Location::Wsl {
            distro: "Ubuntu".to_string(),
            linux_path: "/srv".to_string(),
        });
    }

    #[cfg(windows)]
    #[test]
    fn classifies_distro_root() {
        let loc = classify(Path::new(r"\\wsl.localhost\kali-linux"));
        assert_eq!(loc, Location::Wsl {
            distro: "kali-linux".to_string(),
            linux_path: "/".to_string(),
        });
    }

    #[cfg(windows)]
    #[test]
    fn classifies_drive_and_non_wsl_unc_as_windows() {
        assert!(matches!(classify(Path::new(r"C:\Users\Lev")), Location::Windows(_)));
        assert!(matches!(classify(Path::new(r"\\server\share\x")), Location::Windows(_)));
    }

    #[test]
    fn linux_home_path_maps_to_unc() {
        let p = linux_to_windows_with("/home/lev/proj", "kali-linux", "/mnt");
        assert_eq!(p, PathBuf::from(r"\\wsl.localhost\kali-linux\home\lev\proj"));
    }

    #[test]
    fn linux_automount_path_maps_to_drive() {
        let p = linux_to_windows_with("/mnt/c/Users/Lev", "kali-linux", "/mnt");
        assert_eq!(p, PathBuf::from(r"C:\Users\Lev"));
        let p = linux_to_windows_with("/drives/d/x", "kali-linux", "/drives");
        assert_eq!(p, PathBuf::from(r"D:\x"));
    }

    #[test]
    fn automount_prefix_must_be_a_whole_segment() {
        // "/mnta/…" must not match root "/mnt", and a multi-char segment
        // after the root is a directory, not a drive letter.
        let p = linux_to_windows_with("/mnta/c/x", "kali", "/mnt");
        assert_eq!(p, PathBuf::from(r"\\wsl.localhost\kali\mnta\c\x"));
        let p = linux_to_windows_with("/mnt/cd/x", "kali", "/mnt");
        assert_eq!(p, PathBuf::from(r"\\wsl.localhost\kali\mnt\cd\x"));
    }

    #[cfg(windows)]
    #[test]
    fn drive_path_maps_to_automount() {
        assert_eq!(
            windows_to_linux_with(Path::new(r"C:\Users\Lev"), "/mnt").as_deref(),
            Some("/mnt/c/Users/Lev")
        );
        assert_eq!(
            windows_to_linux_with(Path::new(r"D:\x y\z"), "/drives").as_deref(),
            Some("/drives/d/x y/z")
        );
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_maps_back_to_linux() {
        assert_eq!(
            windows_to_linux_with(Path::new(r"\\wsl.localhost\kali-linux\home\lev"), "/mnt")
                .as_deref(),
            Some("/home/lev")
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree wsl::`
Expected: COMPILE ERROR — `classify`, `Location`, `linux_to_windows_with`, `windows_to_linux_with` not found.

- [ ] **Step 3: Implement**

Add above the test module in `wsl.rs`:

```rust
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;

/// Where a path physically lives.  `linux_path` is the path as seen from
/// inside the distro, always with forward slashes and a leading `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Windows(PathBuf),
    Wsl { distro: String, linux_path: String },
}

/// The distro-side directory Windows drives are mounted under.  Set once at
/// startup from `[ui.wsl] automount_root`; `/mnt` is WSL's default.
static AUTOMOUNT_ROOT: OnceLock<String> = OnceLock::new();

pub fn set_automount_root(root: String) {
    let _ = AUTOMOUNT_ROOT.set(root);
}

fn automount_root() -> &'static str {
    AUTOMOUNT_ROOT.get().map(String::as_str).unwrap_or("/mnt")
}

/// Classify by UNC prefix: `\\wsl$\<distro>\…` and `\\wsl.localhost\<distro>\…`
/// (and their `\\?\UNC\…` verbatim forms) are WSL; everything else is Windows.
pub fn classify(path: &Path) -> Location {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Location::Windows(path.to_path_buf());
    };
    let (server, share) = match prefix.kind() {
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => (server, share),
        _ => return Location::Windows(path.to_path_buf()),
    };
    let server = server.to_string_lossy();
    if !server.eq_ignore_ascii_case("wsl$") && !server.eq_ignore_ascii_case("wsl.localhost") {
        return Location::Windows(path.to_path_buf());
    }
    let mut linux_path = String::new();
    for component in components {
        if let Component::Normal(segment) = component {
            linux_path.push('/');
            linux_path.push_str(&segment.to_string_lossy());
        }
    }
    if linux_path.is_empty() {
        linux_path.push('/');
    }
    Location::Wsl { distro: share.to_string_lossy().into_owned(), linux_path }
}

/// Translate a Linux path reported by git inside `distro` to the Windows
/// path the rest of the app uses: `<automount_root>/<drive>/…` becomes a
/// drive path, anything else a `\\wsl.localhost\<distro>\…` UNC path.
pub fn linux_to_windows(linux: &str, distro: &str) -> PathBuf {
    linux_to_windows_with(linux, distro, automount_root())
}

fn linux_to_windows_with(linux: &str, distro: &str, automount_root: &str) -> PathBuf {
    let root = automount_root.trim_end_matches('/');
    if let Some(rest) = linux.strip_prefix(root) {
        // The root must end at a segment boundary — "/mnta/…" is not under "/mnt".
        if rest.starts_with('/') {
            let mut segments = rest.split('/').filter(|s| !s.is_empty());
            if let Some(first) = segments.next() {
                let mut chars = first.chars();
                if let (Some(letter), None) = (chars.next(), chars.next()) {
                    if letter.is_ascii_alphabetic() {
                        let mut out = format!("{}:\\", letter.to_ascii_uppercase());
                        out.push_str(&segments.collect::<Vec<_>>().join("\\"));
                        return PathBuf::from(out);
                    }
                }
            }
        }
    }
    let mut out = format!(r"\\wsl.localhost\{distro}");
    for segment in linux.split('/').filter(|s| !s.is_empty()) {
        out.push('\\');
        out.push_str(segment);
    }
    PathBuf::from(out)
}

/// Translate a Windows path to what git inside a distro can resolve:
/// WSL UNC paths strip to their Linux part; drive paths map under the
/// automount root; anything else (non-WSL UNC shares) is untranslatable.
pub fn windows_to_linux(path: &Path) -> Option<String> {
    windows_to_linux_with(path, automount_root())
}

fn windows_to_linux_with(path: &Path, automount_root: &str) -> Option<String> {
    if let Location::Wsl { linux_path, .. } = classify(path) {
        return Some(linux_path);
    }
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return None;
    };
    let drive = match prefix.kind() {
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
        _ => return None,
    };
    let root = automount_root.trim_end_matches('/');
    let mut out = format!("{root}/{}", (drive as char).to_ascii_lowercase());
    for component in components {
        if let Component::Normal(segment) = component {
            out.push('/');
            out.push_str(&segment.to_string_lossy());
        }
    }
    Some(out)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree wsl::`
Expected: all Task-1 tests PASS. Also run `cargo check -p alacritree` — clean (unused-fn warnings for `linux_to_windows`/`windows_to_linux`/`set_automount_root` are acceptable at this point only if the compiler emits them; if so, silence by the fact later tasks consume them — do NOT add `#[allow(dead_code)]`; if the warning blocks a clean check, it's fine to leave the warning until Task 3 wires them).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/wsl.rs alacritree/src/main.rs
git commit -m "feat(wsl): add location model and path translation"
```

---

### Task 2: `wsl.rs` — distro enumeration and command builders

**Files:**
- Modify: `alacritree/Cargo.toml` (windows-only `winreg` dep)
- Modify: `alacritree/src/wsl.rs`
- Test: `wsl.rs` tests module

**Interfaces:**
- Consumes: `crate::command_ext::CommandExt` (`hide_console()`).
- Produces:
  - `pub struct WslDistro { pub name: String, pub is_default: bool }`
  - `pub fn distros() -> Vec<WslDistro>` (empty on non-Windows / no WSL)
  - `pub fn command(distro: &str, cd: Option<&Path>) -> Command` — `wsl.exe -d <d> [--cd <dir>] --exec`, console hidden, `WSL_UTF8=1`; caller appends argv
  - `pub fn shell_invocation(distro: &str, workdir: &Path) -> (String, Vec<String>)` — program+args for a session shell

- [ ] **Step 1: Add the dependency**

In `alacritree/Cargo.toml`, after the `[target.'cfg(unix)'.dependencies]` block add:

```toml
[target.'cfg(windows)'.dependencies]
# Distro enumeration reads HKCU\...\Lxss directly (Windows Terminal's
# approach) — no process spawn at startup, and it identifies the default.
winreg = "0.55"
```

- [ ] **Step 2: Write the failing tests**

Append inside `mod tests` in `wsl.rs`:

```rust
    #[test]
    fn parses_utf8_distro_list() {
        let out = b"kali-linux\nUbuntu\ndocker-desktop\n";
        let distros = parse_distro_list(out);
        assert_eq!(distros.len(), 2);
        assert_eq!(distros[0], WslDistro { name: "kali-linux".to_string(), is_default: true });
        assert_eq!(distros[1], WslDistro { name: "Ubuntu".to_string(), is_default: false });
    }

    #[test]
    fn parses_utf16_distro_list() {
        // wsl.exe older than 0.64.0 ignores WSL_UTF8 and emits UTF-16LE.
        let text = "kali-linux\r\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let distros = parse_distro_list(&bytes);
        assert_eq!(distros, vec![WslDistro { name: "kali-linux".to_string(), is_default: true }]);
    }

    #[test]
    fn command_builds_expected_argv() {
        let cmd = command("kali-linux", Some(Path::new(r"\\wsl.localhost\kali-linux\home")));
        let args: Vec<String> =
            cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(cmd.get_program().to_string_lossy(), "wsl.exe");
        assert_eq!(args, vec!["-d", "kali-linux", "--cd", r"\\wsl.localhost\kali-linux\home", "--exec"]);
    }

    #[test]
    fn shell_invocation_has_no_exec() {
        let (program, args) = shell_invocation("kali-linux", Path::new(r"C:\proj"));
        assert_eq!(program, "wsl.exe");
        assert_eq!(args, vec!["-d", "kali-linux", "--cd", r"C:\proj"]);
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p alacritree wsl::`
Expected: COMPILE ERROR — `WslDistro`, `parse_distro_list`, `command`, `shell_invocation` not found.

- [ ] **Step 4: Implement**

Add to `wsl.rs` (below the translation code, above tests). Imports to extend at the top: `use std::process::{Command, Stdio};` and `use crate::command_ext::CommandExt;`.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslDistro {
    pub name: String,
    pub is_default: bool,
}

/// Docker/Rancher register utility distros the user never shells into.
fn is_utility_distro(name: &str) -> bool {
    name.starts_with("docker-desktop") || name.starts_with("rancher-desktop")
}

/// Registered distros, default first-classed.  Registry is the primary
/// source (no process spawn, knows the default); `wsl -l -q` is the
/// fallback when the key is unreadable.  Empty means WSL features stay
/// dormant.
#[cfg(windows)]
pub fn distros() -> Vec<WslDistro> {
    match registry_distros() {
        Some(list) if !list.is_empty() => list,
        _ => cli_distros(),
    }
}

#[cfg(not(windows))]
pub fn distros() -> Vec<WslDistro> {
    Vec::new()
}

#[cfg(windows)]
fn registry_distros() -> Option<Vec<WslDistro>> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let lxss = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Lxss")
        .ok()?;
    let default_guid: String = lxss.get_value("DefaultDistribution").unwrap_or_default();
    let mut out = Vec::new();
    for guid in lxss.enum_keys().flatten() {
        let Ok(subkey) = lxss.open_subkey(&guid) else { continue };
        let Ok(name) = subkey.get_value::<String, _>("DistributionName") else { continue };
        if is_utility_distro(&name) {
            continue;
        }
        out.push(WslDistro { is_default: guid == default_guid, name });
    }
    Some(out)
}

#[cfg(windows)]
fn cli_distros() -> Vec<WslDistro> {
    let output = command_bare()
        .args(["-l", "-q"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => parse_distro_list(&o.stdout),
        _ => Vec::new(),
    }
}

/// `wsl -l -q` lists the default distro first.  Output is UTF-8 when
/// WSL_UTF8=1 is honored (WSL 0.64.0+); older versions emit UTF-16LE,
/// detected by the NUL bytes ASCII names acquire in that encoding.
fn parse_distro_list(stdout: &[u8]) -> Vec<WslDistro> {
    let text = if stdout.contains(&0) {
        let units: Vec<u16> =
            stdout.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(stdout).into_owned()
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_utility_distro(line))
        .enumerate()
        .map(|(i, name)| WslDistro { name: name.to_string(), is_default: i == 0 })
        .collect()
}

/// `wsl.exe -d <distro> [--cd <dir>] --exec` with the console window
/// suppressed and wsl.exe's own messages forced to UTF-8 (they are UTF-16LE
/// otherwise; the relayed Linux byte stream is unaffected).  Callers append
/// the argv to run — `--exec` passes it verbatim to the process, skipping
/// the user's shell and rc files (per-invocation rc sourcing is a known
/// latency trap).  `--cd` natively accepts Windows, UNC, and Linux paths.
pub fn command(distro: &str, cd: Option<&Path>) -> Command {
    let mut cmd = command_bare();
    cmd.arg("-d").arg(distro);
    if let Some(dir) = cd {
        cmd.arg("--cd").arg(dir);
    }
    cmd.arg("--exec");
    cmd
}

fn command_bare() -> Command {
    let mut cmd = Command::new("wsl.exe");
    cmd.hide_console().env("WSL_UTF8", "1");
    cmd
}

/// Program + args for a session whose shell runs inside `distro`.  No
/// `--exec`: wsl.exe launches the distro's own default login shell, which
/// is the contract — we never guess shells.
pub fn shell_invocation(distro: &str, workdir: &Path) -> (String, Vec<String>) {
    (
        "wsl.exe".to_string(),
        vec![
            "-d".to_string(),
            distro.to_string(),
            "--cd".to_string(),
            workdir.to_string_lossy().into_owned(),
        ],
    )
}
```

Note: `Stdio` and `CommandExt` are only used inside `#[cfg(windows)]` functions plus `command()`/`command_bare()` which are unconditional — keep `command`/`command_bare`/`shell_invocation`/`parse_distro_list` unconditional (they compile everywhere; `wsl.exe` merely fails to spawn on Unix, and classify never routes there).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alacritree wsl::`
Expected: PASS (Task 1 + Task 2 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add alacritree/Cargo.toml Cargo.lock alacritree/src/wsl.rs
git commit -m "feat(wsl): enumerate distros and build wsl.exe commands"
```

---

### Task 3: config — `[ui.wsl] automount_root`

**Files:**
- Modify: `alacritree/src/config.rs`
- Modify: `alacritree/src/main.rs`
- Test: `config.rs` tests module (new)

**Interfaces:**
- Consumes: `wsl::set_automount_root` (Task 1).
- Produces: `Config.wsl_automount_root: String` (normalized, default `"/mnt"`).

- [ ] **Step 1: Write the failing test**

`config.rs` has no test module yet; add at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automount_root_defaults_and_normalizes() {
        let raw: RawConfig = toml::from_str("").unwrap();
        assert_eq!(raw.into_config().wsl_automount_root, "/mnt");

        let raw: RawConfig =
            toml::from_str("[ui.wsl]\nautomount_root = \"/drives/\"").unwrap();
        assert_eq!(raw.into_config().wsl_automount_root, "/drives");

        // Nonsense values fall back rather than corrupting every translation.
        let raw: RawConfig = toml::from_str("[ui.wsl]\nautomount_root = \"mnt\"").unwrap();
        assert_eq!(raw.into_config().wsl_automount_root, "/mnt");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alacritree config::`
Expected: COMPILE ERROR — no field `wsl_automount_root`, no `[ui.wsl]` in `RawUi`.

- [ ] **Step 3: Implement**

1. `Config` struct: add field `pub wsl_automount_root: String,` and to `Default for Config`: `wsl_automount_root: "/mnt".to_string(),`.
2. `RawUi`: add field `wsl: RawUiWsl,` and below it:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawUiWsl {
    /// Distro-side mount point for Windows drives, mirroring wsl.conf's
    /// `[automount] root`.  Only used for paths *we* translate (git output
    /// from inside a distro); `wsl.exe --cd` translates with the distro's
    /// real mount table regardless of this value.
    automount_root: Option<String>,
}
```

3. In `RawConfig::into_config`, before the final `Config { … }` literal:

```rust
        // ---- WSL ----
        let wsl_automount_root = self
            .ui
            .wsl
            .automount_root
            .map(|r| r.trim_end_matches('/').to_string())
            .filter(|r| r.starts_with('/') && r.len() > 1)
            .unwrap_or_else(|| "/mnt".to_string());
```

and add `wsl_automount_root,` to the `Config { … }` literal.

4. `main.rs`, right after `let config = config::load();`:

```rust
    wsl::set_automount_root(config.wsl_automount_root.clone());
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree config::` then `cargo check -p alacritree`
Expected: PASS, clean check.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/config.rs alacritree/src/main.rs
git commit -m "feat(config): add [ui.wsl] automount_root option"
```

---

### Task 4: `wsl.rs` — batched script runner

**Files:**
- Modify: `alacritree/src/wsl.rs`
- Test: `wsl.rs` tests module

**Interfaces:**
- Consumes: `command()` (Task 2).
- Produces:
  - `pub const SECTION_SEP: &[u8] = b"\n@@ALACRITREE@@\n";`
  - `pub fn run_batch(distro: &str, script: &str, args: &[&str]) -> Result<Vec<u8>, String>` — runs `script` via `sh -c` inside the distro, `args` bound to `$1..`; returns raw stdout
  - `pub fn split_sections(stdout: &[u8]) -> Vec<&[u8]>` — byte-wise split on `SECTION_SEP`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn splits_sections_preserving_nuls() {
        let mut input = Vec::new();
        input.extend_from_slice(b"yes");
        input.extend_from_slice(SECTION_SEP);
        input.extend_from_slice(b"a\0b\0\0c\0");
        input.extend_from_slice(SECTION_SEP);
        input.extend_from_slice(b"tail");
        let sections = split_sections(&input);
        assert_eq!(sections, vec![&b"yes"[..], &b"a\0b\0\0c\0"[..], &b"tail"[..]]);
    }

    #[test]
    fn split_handles_empty_and_missing_sections() {
        assert_eq!(split_sections(b""), vec![&b""[..]]);
        let mut input = Vec::new();
        input.extend_from_slice(SECTION_SEP);
        input.extend_from_slice(SECTION_SEP);
        assert_eq!(split_sections(&input), vec![&b""[..], &b""[..], &b""[..]]);
    }

    /// Live round trip against the default distro.  Requires WSL; run
    /// manually: `cargo test -p alacritree wsl:: -- --ignored`
    #[test]
    #[ignore]
    fn run_batch_round_trips() {
        let distro = distros().into_iter().find(|d| d.is_default).expect("a default distro");
        let out = run_batch(&distro.name, r#"printf '%s' "$1""#, &["hello"]).unwrap();
        assert_eq!(out, b"hello");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree wsl::`
Expected: COMPILE ERROR — `SECTION_SEP`, `split_sections`, `run_batch` not found.

- [ ] **Step 3: Implement**

```rust
/// Separates the outputs of the individual commands a batch script runs.
/// Scripts emit it between sections via `sep() { printf '\n@@ALACRITREE@@\n'; }`;
/// NUL-delimited porcelain payloads pass through untouched because the
/// separator is matched as raw bytes, and the leading newline absorbs the
/// section's own trailing newline when it has one.
pub const SECTION_SEP: &[u8] = b"\n@@ALACRITREE@@\n";

/// Run `script` through `sh -c` inside `distro`, with `args` bound to
/// `$1..`.  One wsl.exe round trip (~400 ms warm on a dev machine, seconds
/// while the VM cold-boots) — callers batch every query for a repo into a
/// single script and must never call this on the UI thread.
pub fn run_batch(distro: &str, script: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = command(distro, None)
        .arg("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run wsl.exe: {e}"))?;
    // Scripts guard individual commands with `2>/dev/null || true`-style
    // fallbacks; a hard failure with no stdout means wsl.exe itself refused
    // (deregistered distro, WSL not installed).
    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() { "wsl.exe failed".to_string() } else { stderr });
    }
    Ok(output.stdout)
}

/// Split batched stdout on `SECTION_SEP`.  Always returns at least one
/// section; a script with N separators yields N+1.
pub fn split_sections(stdout: &[u8]) -> Vec<&[u8]> {
    let mut sections = Vec::new();
    let mut rest = stdout;
    while let Some(pos) = rest.windows(SECTION_SEP.len()).position(|w| w == SECTION_SEP) {
        sections.push(&rest[..pos]);
        rest = &rest[pos + SECTION_SEP.len()..];
    }
    sections.push(rest);
    sections
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree wsl::`
Expected: PASS (ignored test skipped). Optionally verify live: `cargo test -p alacritree wsl:: -- --ignored` on the dev machine — PASS against `kali-linux`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/wsl.rs
git commit -m "feat(wsl): add batched sh script runner"
```

---

### Task 5: `projects.rs` — WSL discovery arm

**Files:**
- Modify: `alacritree/src/projects.rs`
- Test: `projects.rs` tests module (new)

**Interfaces:**
- Consumes: `wsl::{classify, Location, run_batch, split_sections, linux_to_windows}`.
- Produces:
  - `Project::discover(root)` transparently handles WSL roots (signature unchanged)
  - `Project::placeholder(root: PathBuf) -> Project` — pseudo-worktree entry, public (Task 6 uses it for pre-discovery display)
  - internal: `parse_worktree_list_z(bytes) -> Vec<WorktreeRecord>`, `default_branch_from_batch(...)`

- [ ] **Step 1: Write the failing tests**

Add at the bottom of `projects.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_list_porcelain_z() {
        let bytes = b"worktree /home/lev/proj\0HEAD 1234567890abcdef\0branch refs/heads/main\0\0\
worktree /home/lev/wt/feat-x\0HEAD fedcba0987654321\0branch refs/heads/feat-x\0\0\
worktree /home/lev/wt/tmp\0HEAD 0011223344556677\0detached\0\0";
        let records = parse_worktree_list_z(bytes);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].path, "/home/lev/proj");
        assert_eq!(records[0].branch.as_deref(), Some("main"));
        assert_eq!(records[1].branch.as_deref(), Some("feat-x"));
        assert_eq!(records[2].branch, None);
        assert_eq!(records[2].head.as_deref(), Some("0011223344556677"));
    }

    #[test]
    fn worktree_paths_with_spaces_survive() {
        let bytes = b"worktree /home/lev/my proj\0HEAD abc\0branch refs/heads/main\0\0";
        let records = parse_worktree_list_z(bytes);
        assert_eq!(records[0].path, "/home/lev/my proj");
    }

    #[test]
    fn default_branch_priority_matches_git2_arm() {
        // origin/HEAD wins.
        assert_eq!(
            default_branch_from_batch("refs/remotes/origin/dev\n", "main\nmaster", "master"),
            Some("dev".to_string())
        );
        // Then common names in priority order, regardless of listing order.
        assert_eq!(
            default_branch_from_batch("", "develop\nmain", ""),
            Some("main".to_string())
        );
        // init.defaultBranch is last (already existence-verified by the script).
        assert_eq!(default_branch_from_batch("", "", "trunk2"), Some("trunk2".to_string()));
        assert_eq!(default_branch_from_batch("", "", ""), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree projects::`
Expected: COMPILE ERROR — `parse_worktree_list_z`, `default_branch_from_batch` not found.

- [ ] **Step 3: Implement**

In `projects.rs`, add `use crate::wsl;` to imports. Replace `discover`'s body with a dispatch, extract the current non-git fallback into `placeholder`, and add the WSL arm:

```rust
impl Project {
    /// Non-git roots get a single pseudo-worktree pointing at themselves so
    /// the user can still spawn a shell there from the sidebar.
    pub fn discover(root: PathBuf) -> Self {
        let name = display_name(&root);
        match wsl::classify(&root) {
            wsl::Location::Wsl { distro, linux_path } => {
                Self::discover_wsl(root, name, &distro, &linux_path)
            },
            wsl::Location::Windows(_) => match Repository::open(&root) {
                Ok(repo) => Self::from_repo(root, name, &repo),
                Err(_) => Self::placeholder(root),
            },
        }
    }

    /// Pseudo-worktree entry: what non-git roots get permanently, and what a
    /// WSL project shows until background discovery fills in worktrees.
    pub fn placeholder(root: PathBuf) -> Self {
        let name = display_name(&root);
        Project {
            worktrees: vec![Worktree {
                name: name.clone(),
                path: root.clone(),
                branch: None,
                is_main: true,
            }],
            root,
            name,
            default_branch: None,
            expanded: true,
        }
    }
```

Add the free function (replacing the inline `name` computation, which `from_repo` callers keep receiving):

```rust
fn display_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}
```

Add the WSL arm inside `impl Project`:

```rust
    /// One wsl.exe round trip answers everything discovery needs; sections
    /// are split on `wsl::SECTION_SEP`.  Any failure degrades to the same
    /// pseudo-worktree a non-git folder gets.
    fn discover_wsl(root: PathBuf, name: String, distro: &str, linux_path: &str) -> Self {
        let stdout = match wsl::run_batch(distro, DISCOVER_SCRIPT, &[linux_path]) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("WSL discovery failed for {}: {e}", root.display());
                return Self::placeholder(root);
            },
        };
        let sections = wsl::split_sections(&stdout);
        let text = |i: usize| {
            sections.get(i).map(|s| String::from_utf8_lossy(s).trim().to_string()).unwrap_or_default()
        };

        if text(0) != "yes" {
            return Self::placeholder(root);
        }

        let records = parse_worktree_list_z(sections.get(1).copied().unwrap_or_default());
        let worktrees: Vec<Worktree> = records
            .iter()
            .enumerate()
            .map(|(i, rec)| {
                let path = wsl::linux_to_windows(&rec.path, distro);
                // Same rendering as the git2 arm: branch name, or the short
                // OID when detached.
                let branch = rec
                    .branch
                    .clone()
                    .or_else(|| rec.head.as_ref().map(|h| h.chars().take(7).collect()));
                let wt_name = if i == 0 {
                    "main".to_string()
                } else {
                    display_name(&path)
                };
                Worktree { name: wt_name, path, branch, is_main: i == 0 }
            })
            .collect();
        if worktrees.is_empty() {
            return Self::placeholder(root);
        }

        Project {
            default_branch: default_branch_from_batch(&text(2), &text(3), &text(4)),
            worktrees,
            root,
            name,
            expanded: true,
        }
    }
```

And the free items at module level:

```rust
/// Sections: 0 repo-or-not, 1 `worktree list --porcelain -z`,
/// 2 origin/HEAD symref, 3 which common default-branch names exist,
/// 4 `init.defaultBranch` only if it names an existing branch.
const DISCOVER_SCRIPT: &str = r#"
p="$1"
sep() { printf '\n@@ALACRITREE@@\n'; }
git -C "$p" rev-parse --is-inside-work-tree >/dev/null 2>&1 && printf yes || printf no
sep
git -C "$p" worktree list --porcelain -z 2>/dev/null
sep
git -C "$p" symbolic-ref refs/remotes/origin/HEAD 2>/dev/null
sep
git -C "$p" for-each-ref --format='%(refname:short)' refs/heads/main refs/heads/master refs/heads/trunk refs/heads/develop 2>/dev/null
sep
cfg=$(git -C "$p" config init.defaultBranch 2>/dev/null)
if [ -n "$cfg" ] && git -C "$p" rev-parse --verify --quiet "refs/heads/$cfg" >/dev/null 2>&1; then printf '%s' "$cfg"; fi
"#;

/// One record from `git worktree list --porcelain -z`.  The main worktree is
/// always the first record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeRecord {
    path: String,
    head: Option<String>,
    branch: Option<String>,
}

/// Parse `git worktree list --porcelain -z`: attributes are NUL-terminated
/// `label value` lines; an empty line (two consecutive NULs) ends a record.
/// `detached`/`bare`/`locked`/`prunable` labels need no handling — a
/// detached record simply carries no `branch`.
fn parse_worktree_list_z(bytes: &[u8]) -> Vec<WorktreeRecord> {
    let mut records = Vec::new();
    let mut current: Option<WorktreeRecord> = None;
    for token in bytes.split(|&b| b == 0) {
        let token = String::from_utf8_lossy(token);
        let token = token.trim_matches('\n');
        if token.is_empty() {
            if let Some(record) = current.take() {
                records.push(record);
            }
            continue;
        }
        if let Some(path) = token.strip_prefix("worktree ") {
            if let Some(record) = current.take() {
                records.push(record);
            }
            current = Some(WorktreeRecord { path: path.to_string(), head: None, branch: None });
        } else if let Some(record) = current.as_mut() {
            if let Some(sha) = token.strip_prefix("HEAD ") {
                record.head = Some(sha.to_string());
            } else if let Some(branch) = token.strip_prefix("branch ") {
                record.branch =
                    Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).to_string());
            }
        }
    }
    if let Some(record) = current.take() {
        records.push(record);
    }
    records
}

/// Replicates `detect_default_branch`'s priority from batched output — see
/// that function for why `init.defaultBranch` comes last.
fn default_branch_from_batch(
    origin_head: &str,
    existing: &str,
    config_default: &str,
) -> Option<String> {
    if let Some(name) = origin_head.trim().strip_prefix("refs/remotes/origin/") {
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    let present: Vec<&str> = existing.lines().map(str::trim).collect();
    for candidate in ["main", "master", "trunk", "develop"] {
        if present.contains(&candidate) {
            return Some(candidate.to_string());
        }
    }
    let cfg = config_default.trim();
    (!cfg.is_empty()).then(|| cfg.to_string())
}
```

Also simplify the old `discover` non-git arm: the `Err(_)` branch now calls `Self::placeholder(root)` (delete the inline struct literal). `from_repo` is unchanged.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree projects::` then `cargo test -p alacritree`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/projects.rs
git commit -m "feat(projects): discover WSL repos via in-distro git"
```

---

### Task 6: `app.rs` — refresh WSL projects off the UI thread

`Project::discover` for a WSL root now runs wsl.exe (~400 ms warm, seconds while the VM boots). It currently runs on the UI thread at startup, on the ↻ button, and after worktree create/delete — that would freeze the window. Windows projects keep the synchronous path (git2 is fast; zero behavior change).

**Files:**
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `Project::{discover, placeholder}` (Task 5), `wsl::{classify, Location}`.
- Produces: `AlacritreeApp::refresh_project(&mut self, ctx: &Context, idx: usize)` — the ONLY way later tasks refresh a project.

- [ ] **Step 1: Add the plumbing**

1. Imports: add `use crate::wsl;`.
2. `AlacritreeApp` struct: add field

```rust
    /// In-flight background re-discoveries, keyed by project root.  WSL
    /// discovery shells out to wsl.exe and must never block paint; results
    /// are adopted in `poll_project_refreshes`.
    pending_project_refresh: HashMap<PathBuf, Receiver<Project>>,
```

and initialize `pending_project_refresh: HashMap::new(),` in the `Self { … }` literal in `new()`.

3. Add methods to `impl AlacritreeApp` (near `persist`):

```rust
    /// Windows projects re-discover synchronously (git2, fast).  WSL
    /// projects re-discover on a worker thread: wsl.exe takes ~400 ms warm
    /// and seconds while the distro VM boots.
    fn refresh_project(&mut self, ctx: &Context, idx: usize) {
        let root = self.projects[idx].root.clone();
        if matches!(wsl::classify(&root), wsl::Location::Windows(_)) {
            self.projects[idx].refresh();
            return;
        }
        if self.pending_project_refresh.contains_key(&root) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let worker_root = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Project::discover(worker_root));
            ctx.request_repaint();
        });
        self.pending_project_refresh.insert(root, rx);
    }

    /// Adopt completed background discoveries.  Only worktrees and the
    /// default branch are copied — `expanded` and the shell override are
    /// user state that survives refreshes (mirrors `Project::refresh`).
    fn poll_project_refreshes(&mut self) {
        let projects = &mut self.projects;
        self.pending_project_refresh.retain(|root, rx| match rx.try_recv() {
            Ok(fresh) => {
                if let Some(project) = projects.iter_mut().find(|p| p.root == *root) {
                    project.worktrees = fresh.worktrees;
                    project.default_branch = fresh.default_branch;
                }
                false
            },
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
    }
```

- [ ] **Step 2: Route all refresh call sites through it**

1. **Startup** (`AlacritreeApp::new`, the `persisted.projects.iter().map(…)` block around line 234): WSL roots get a placeholder instead of blocking discovery:

```rust
        let projects: Vec<Project> = persisted
            .projects
            .iter()
            .map(|p| {
                // WSL roots discover in the background after construction —
                // a cold distro takes seconds to boot and would block first
                // paint.
                let mut project = match wsl::classify(&p.root) {
                    wsl::Location::Windows(_) => Project::discover(p.root.clone()),
                    wsl::Location::Wsl { .. } => Project::placeholder(p.root.clone()),
                };
                project.expanded = p.expanded;
                project
            })
            .collect();
```

and after `let mut app = Self { … };` (before the initial `spawn_session` call):

```rust
        let wsl_indices: Vec<usize> = app
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(wsl::classify(&p.root), wsl::Location::Wsl { .. }))
            .map(|(i, _)| i)
            .collect();
        for idx in wsl_indices {
            app.refresh_project(&cc.egui_ctx, idx);
        }
```

2. **↻ button** (left-sidebar handler, `if let Some(idx) = refresh_idx { self.projects[idx].refresh(); }` around line 868): replace with `self.refresh_project(ctx, idx);` (`ctx` is in scope — the sidebar fn takes `&Context`).
3. **`run_pending_delete`** (around line 1811, `self.projects[req.project_idx].refresh();`): `run_pending_delete` has no `ctx` parameter — add one (`fn run_pending_delete(&mut self, ctx: &Context)`), update its single caller in `show_delete_dialog` (`self.run_pending_delete();` → `self.run_pending_delete(ctx);`), and replace the refresh line with `self.refresh_project(ctx, req.project_idx);`.
4. **`show_create_dialog` Done arm** (around line 1839, `self.projects[project_idx].refresh();`): replace with `self.refresh_project(ctx, project_idx);` (`ctx` already in scope).
5. **`add_project_via_dialog`** (line 416): WSL folders are pickable in the folder dialog; discovery must not block. Replace the body:

```rust
    fn add_project_via_dialog(&mut self, ctx: &Context) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            if !self.projects.iter().any(|p| p.root == path) {
                match wsl::classify(&path) {
                    wsl::Location::Windows(_) => self.projects.push(Project::discover(path)),
                    wsl::Location::Wsl { .. } => {
                        self.projects.push(Project::placeholder(path));
                        let idx = self.projects.len() - 1;
                        self.refresh_project(ctx, idx);
                    },
                }
                self.persist();
            }
        }
    }
```

Update its callers to pass `ctx` (the `add_project_clicked` handler in the left sidebar, and the `add_project_requested` shortcut handler in `handle_shortcuts` — grep `add_project_via_dialog(`).
6. **Poll**: at the top of `fn update(&mut self, ctx: &Context, …)` in `impl eframe::App for AlacritreeApp` (grep `fn update`), add `self.poll_project_refreshes();` as the first statement.

- [ ] **Step 3: Verify**

Run: `cargo check -p alacritree` then `cargo test -p alacritree`
Expected: clean, all tests pass. Manual smoke (dev machine): `cargo run -p alacritree`, add `\\wsl.localhost\kali-linux\home\<user>\<some-repo>` via `+` — the row appears immediately as a single pseudo-entry, then fills in worktrees within a second or two without the window freezing.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(app): refresh WSL projects off the UI thread"
```

---

### Task 7: `git_status.rs` — WSL status arm

**Files:**
- Modify: `alacritree/src/git_status.rs`
- Test: `git_status.rs` tests module (new)

**Interfaces:**
- Consumes: `wsl::{classify, Location, run_batch, split_sections}`, existing `GitStatus`/`FileChange`/`ChangeKind`/`DiffStat`/`DirtyCounts`.
- Produces: `compute(path, hint)` and `dirty_counts(path)` transparently handle WSL paths (signatures unchanged; `StatusCache` untouched — it already runs `compute` on a worker thread).

- [ ] **Step 1: Write the failing tests**

Add at the bottom of `git_status.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2_z() {
        let bytes = b"1 .M N... 100644 100644 100644 aaaa bbbb src/main.rs\0\
1 A. N... 000000 100644 100644 0000 1111 new.rs\0\
2 R. N... 100644 100644 100644 cccc dddd R100 renamed.rs\0old-name.rs\0\
u UU N... 100644 100644 100644 100644 e1 e2 e3 conflicted.rs\0\
? untracked with space.txt\0";
        let (staged, unstaged) = parse_status_v2_z(bytes);

        let staged_pairs: Vec<(&str, ChangeKind)> =
            staged.iter().map(|c| (c.path.as_str(), c.kind)).collect();
        assert_eq!(staged_pairs, vec![
            ("new.rs", ChangeKind::Added),
            ("renamed.rs", ChangeKind::Renamed),
            ("conflicted.rs", ChangeKind::Conflicted),
        ]);

        let unstaged_pairs: Vec<(&str, ChangeKind)> =
            unstaged.iter().map(|c| (c.path.as_str(), c.kind)).collect();
        assert_eq!(unstaged_pairs, vec![
            ("src/main.rs", ChangeKind::Modified),
            ("untracked with space.txt", ChangeKind::Untracked),
        ]);
    }

    #[test]
    fn parses_numstat_z() {
        // Ordinary, rename (empty path + src/dst tokens), binary (- counts).
        let bytes = b"3\t1\tsrc/lib.rs\0\
2\t0\t\0old.rs\0new.rs\0\
-\t-\tassets/icon.png\0";
        let stats = parse_numstat_z(bytes);
        assert_eq!(stats.len(), 3);
        assert_eq!((stats[0].path.as_str(), stats[0].additions, stats[0].deletions),
                   ("src/lib.rs", 3, 1));
        assert_eq!((stats[1].path.as_str(), stats[1].additions, stats[1].deletions),
                   ("new.rs", 2, 0));
        assert_eq!((stats[2].path.as_str(), stats[2].additions, stats[2].deletions),
                   ("assets/icon.png", 0, 0));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree git_status::`
Expected: COMPILE ERROR — `parse_status_v2_z`, `parse_numstat_z` not found.

- [ ] **Step 3: Implement parsers**

Add to `git_status.rs` (module level), plus `use crate::wsl;` in imports:

```rust
/// Map porcelain-v2 `XY` state chars to the sidebar's kinds.  X is the
/// index-vs-HEAD (staged) side, Y the worktree-vs-index (unstaged) side;
/// `.` means unchanged on that side.  Mirrors `staged_kind`/`unstaged_kind`.
fn staged_kind_v2(x: char) -> Option<ChangeKind> {
    match x {
        'A' => Some(ChangeKind::Added),
        'D' => Some(ChangeKind::Deleted),
        'R' | 'C' => Some(ChangeKind::Renamed),
        'M' | 'T' => Some(ChangeKind::Modified),
        _ => None,
    }
}

fn unstaged_kind_v2(y: char) -> Option<ChangeKind> {
    match y {
        'D' => Some(ChangeKind::Deleted),
        'R' | 'C' => Some(ChangeKind::Renamed),
        'M' | 'T' | 'A' => Some(ChangeKind::Modified),
        _ => None,
    }
}

/// Parse `git status --porcelain=v2 -z` into the same (staged, unstaged)
/// split the git2 arm produces.  Records are NUL-terminated; rename records
/// (`2 …`) are followed by an extra NUL-separated token holding the rename
/// source, which the sidebar doesn't show.
fn parse_status_v2_z(bytes: &[u8]) -> (Vec<FileChange>, Vec<FileChange>) {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut tokens = bytes.split(|&b| b == 0);
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(token);
        let Some((kind, rest)) = line.split_once(' ') else { continue };
        match kind {
            // `1 XY sub mH mI mW hH hI path` — path is the 8th field and may
            // contain spaces, so bound the split.
            "1" => {
                let mut fields = rest.splitn(8, ' ');
                let xy = fields.next().unwrap_or("..");
                if let Some(path) = fields.nth(6) {
                    push_xy(xy, path.to_string(), &mut staged, &mut unstaged);
                }
            },
            // `2 XY sub mH mI mW hH hI Xscore path` + NUL + origPath.
            "2" => {
                let mut fields = rest.splitn(9, ' ');
                let xy = fields.next().unwrap_or("..");
                let path = fields.nth(7).map(str::to_string);
                let _orig = tokens.next();
                if let Some(path) = path {
                    push_xy(xy, path, &mut staged, &mut unstaged);
                }
            },
            // `u XY sub m1 m2 m3 mW h1 h2 h3 path` — conflicts land in the
            // staged list, matching the git2 arm.
            "u" => {
                if let Some(path) = rest.splitn(10, ' ').nth(9) {
                    staged.push(FileChange { path: path.to_string(), kind: ChangeKind::Conflicted });
                }
            },
            "?" => unstaged
                .push(FileChange { path: rest.to_string(), kind: ChangeKind::Untracked }),
            _ => {},
        }
    }
    (staged, unstaged)
}

fn push_xy(xy: &str, path: String, staged: &mut Vec<FileChange>, unstaged: &mut Vec<FileChange>) {
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.');
    let y = chars.next().unwrap_or('.');
    if let Some(kind) = staged_kind_v2(x) {
        staged.push(FileChange { path: path.clone(), kind });
    }
    if let Some(kind) = unstaged_kind_v2(y) {
        unstaged.push(FileChange { path, kind });
    }
}

/// Parse `git diff --numstat -z`: `added TAB deleted TAB path NUL`, except
/// renames, where the path field is empty and `src NUL dst NUL` follow.
/// Binary files report `-` counts, mapped to 0 (matching the git2 arm,
/// which never sees text lines for them either).
fn parse_numstat_z(bytes: &[u8]) -> Vec<DiffStat> {
    let mut stats = Vec::new();
    let mut tokens = bytes.split(|&b| b == 0);
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(token);
        let mut fields = line.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let additions = added.parse().unwrap_or(0);
        let deletions = deleted.parse().unwrap_or(0);
        let path = if path.is_empty() {
            let _src = tokens.next();
            match tokens.next() {
                Some(dst) => String::from_utf8_lossy(dst).into_owned(),
                None => continue,
            }
        } else {
            path.to_string()
        };
        stats.push(DiffStat { path, additions, deletions });
    }
    stats
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree git_status::`
Expected: PASS.

- [ ] **Step 5: Wire the WSL arms**

1. `compute` becomes the dispatcher:

```rust
pub fn compute(path: &Path, default_branch_hint: Option<&str>) -> GitStatus {
    match wsl::classify(path) {
        wsl::Location::Wsl { distro, linux_path } => {
            compute_wsl(&distro, &linux_path, default_branch_hint)
        },
        wsl::Location::Windows(_) => match compute_inner(path, default_branch_hint) {
            Ok(s) => s,
            Err(e) => GitStatus { error: Some(e.to_string()), ..Default::default() },
        },
    }
}
```

2. Add the script and WSL compute:

```rust
/// Sections: 0 current branch (short OID when detached), 1 porcelain-v2
/// status, 2 effective default branch (the hint, or detection replicating
/// `detect_default_branch`), 3 the resolved base ref (origin-first, like
/// `resolve_base_commit`), 4 numstat against the merge base (`...` = git's
/// merge-base triple-dot, preserving `diff_against_branch` semantics).
const STATUS_SCRIPT: &str = r#"
p="$1"; hint="$2"
sep() { printf '\n@@ALACRITREE@@\n'; }
git -C "$p" symbolic-ref --short HEAD 2>/dev/null || git -C "$p" rev-parse --short=7 HEAD 2>/dev/null
sep
git -C "$p" status --porcelain=v2 -z 2>/dev/null
sep
if [ -z "$hint" ]; then
  h=$(git -C "$p" symbolic-ref refs/remotes/origin/HEAD 2>/dev/null)
  h="${h#refs/remotes/origin/}"
  if [ -z "$h" ]; then
    for c in main master trunk develop; do
      if git -C "$p" rev-parse --verify --quiet "refs/heads/$c" >/dev/null 2>&1; then h="$c"; break; fi
    done
  fi
  if [ -z "$h" ]; then
    c=$(git -C "$p" config init.defaultBranch 2>/dev/null)
    if [ -n "$c" ] && git -C "$p" rev-parse --verify --quiet "refs/heads/$c" >/dev/null 2>&1; then h="$c"; fi
  fi
  hint="$h"
fi
printf '%s' "$hint"
sep
base=""
if [ -n "$hint" ]; then
  for ref in "refs/remotes/origin/$hint" "refs/heads/$hint"; do
    if git -C "$p" rev-parse --verify --quiet "$ref" >/dev/null 2>&1; then base="$ref"; break; fi
  done
fi
printf '%s' "$base"
sep
if [ -n "$base" ]; then git -C "$p" diff --numstat -z "$base...HEAD" 2>/dev/null; fi
"#;

/// One wsl.exe round trip per refresh tick.  Runs on `spawn_compute`'s
/// worker thread, so the ~400 ms round trip never blocks paint.
fn compute_wsl(distro: &str, linux_path: &str, hint: Option<&str>) -> GitStatus {
    let stdout = match wsl::run_batch(distro, STATUS_SCRIPT, &[linux_path, hint.unwrap_or("")]) {
        Ok(s) => s,
        Err(e) => return GitStatus { error: Some(e), ..Default::default() },
    };
    let sections = wsl::split_sections(&stdout);
    let text = |i: usize| {
        sections.get(i).map(|s| String::from_utf8_lossy(s).trim().to_string()).unwrap_or_default()
    };

    let branch = Some(text(0)).filter(|s| !s.is_empty());
    if branch.is_none() {
        return GitStatus {
            error: Some(format!("could not open repository at {linux_path}")),
            ..Default::default()
        };
    }
    let (staged, unstaged) = parse_status_v2_z(sections.get(1).copied().unwrap_or_default());
    let default_branch = Some(text(2)).filter(|s| !s.is_empty());
    let default_branch_resolved = Some(text(3)).filter(|s| !s.is_empty());
    let branch_diff = if default_branch_resolved.is_some() {
        parse_numstat_z(sections.get(4).copied().unwrap_or_default())
    } else {
        Vec::new()
    };
    GitStatus {
        branch,
        default_branch,
        default_branch_resolved,
        staged,
        unstaged,
        branch_diff,
        error: None,
    }
}
```

3. `dirty_counts` dispatch — wrap the existing body:

```rust
pub fn dirty_counts(path: &Path) -> DirtyCounts {
    match wsl::classify(path) {
        wsl::Location::Wsl { distro, linux_path } => dirty_counts_wsl(&distro, &linux_path),
        wsl::Location::Windows(_) => dirty_counts_git2(path),
    }
}

fn dirty_counts_git2(path: &Path) -> DirtyCounts {
    // …existing body of dirty_counts, verbatim…
}

/// Counts from one porcelain-v2 round trip.  Called synchronously when the
/// delete modal opens — a warm wsl.exe call (~400 ms) is a tolerable
/// one-shot stall for an explicit destructive action.
fn dirty_counts_wsl(distro: &str, linux_path: &str) -> DirtyCounts {
    let Ok(stdout) =
        wsl::run_batch(distro, r#"git -C "$1" status --porcelain=v2 -z 2>/dev/null"#, &[linux_path])
    else {
        return DirtyCounts::default();
    };
    let (staged, unstaged) = parse_status_v2_z(&stdout);
    DirtyCounts {
        staged: staged.len(),
        modified: unstaged.iter().filter(|c| c.kind != ChangeKind::Untracked).count(),
        untracked: unstaged.iter().filter(|c| c.kind == ChangeKind::Untracked).count(),
    }
}
```

- [ ] **Step 6: Verify**

Run: `cargo test -p alacritree` and `cargo check -p alacritree`
Expected: PASS/clean. Manual smoke: run the app with the WSL project from Task 6 active — the right sidebar shows branch, staged/unstaged files, and `Changes vs <branch>` matching `git status` run inside the distro.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add alacritree/src/git_status.rs
git commit -m "feat(git-status): compute WSL repo status via in-distro git"
```

---

### Task 8: shell override — model and persistence

**Files:**
- Modify: `alacritree/src/wsl.rs` (`ShellChoice`)
- Modify: `alacritree/src/state.rs` (`PersistedProject.shell`)
- Modify: `alacritree/src/projects.rs` (`Project.shell_override`)
- Modify: `alacritree/src/app.rs` (load/save the field)
- Test: `wsl.rs` and `state.rs` test modules

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub enum ShellChoice { Windows, Wsl(String) }` in `wsl.rs`, with `pub fn parse(s: &str) -> Option<Self>` and `pub fn to_state_string(&self) -> String`
  - `Project.shell_override: Option<crate::wsl::ShellChoice>` (public field)
  - `PersistedProject.shell: Option<String>`

- [ ] **Step 1: Write the failing tests**

In `wsl.rs` tests:

```rust
    #[test]
    fn shell_choice_round_trips() {
        assert_eq!(ShellChoice::parse("windows"), Some(ShellChoice::Windows));
        assert_eq!(
            ShellChoice::parse("wsl:kali-linux"),
            Some(ShellChoice::Wsl("kali-linux".to_string()))
        );
        assert_eq!(ShellChoice::parse("wsl:"), None);
        assert_eq!(ShellChoice::parse("plan9"), None);
        assert_eq!(ShellChoice::Wsl("u".to_string()).to_state_string(), "wsl:u");
        assert_eq!(ShellChoice::Windows.to_state_string(), "windows");
    }
```

In `state.rs`, add a tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_field_is_optional_and_round_trips() {
        // Old state files (no `shell`) still parse.
        let old = "[[projects]]\nroot = 'C:/x'\n";
        let state: PersistedState = toml::from_str(old).unwrap();
        assert_eq!(state.projects[0].shell, None);

        let state = PersistedState {
            projects: vec![PersistedProject {
                root: PathBuf::from("C:/x"),
                expanded: true,
                shell: Some("wsl:kali-linux".to_string()),
            }],
            ..Default::default()
        };
        let text = toml::to_string_pretty(&state).unwrap();
        let back: PersistedState = toml::from_str(&text).unwrap();
        assert_eq!(back.projects[0].shell.as_deref(), Some("wsl:kali-linux"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alacritree`
Expected: COMPILE ERROR — `ShellChoice` and `shell` field missing.

- [ ] **Step 3: Implement**

1. `wsl.rs`:

```rust
/// Per-project shell override, persisted in state.toml as `"windows"` or
/// `"wsl:<distro>"`.  Absent means auto-by-location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellChoice {
    Windows,
    Wsl(String),
}

impl ShellChoice {
    pub fn parse(s: &str) -> Option<Self> {
        if s == "windows" {
            return Some(Self::Windows);
        }
        s.strip_prefix("wsl:").filter(|d| !d.is_empty()).map(|d| Self::Wsl(d.to_string()))
    }

    pub fn to_state_string(&self) -> String {
        match self {
            Self::Windows => "windows".to_string(),
            Self::Wsl(distro) => format!("wsl:{distro}"),
        }
    }
}
```

2. `state.rs`, in `PersistedProject`:

```rust
    /// Shell override: `"windows"` or `"wsl:<distro>"`.  Absent = auto by
    /// project location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
```

3. `projects.rs`: add `pub shell_override: Option<crate::wsl::ShellChoice>,` to `Project`, and `shell_override: None,` to both construction sites (`placeholder` and the two `Project { … }` literals in `from_repo`/`discover_wsl`). `refresh()` needs no change — it only copies `worktrees`/`default_branch`, so the override survives.
4. `app.rs`:
   - In `new()`'s project mapping, after `project.expanded = p.expanded;`: `project.shell_override = p.shell.as_deref().and_then(wsl::ShellChoice::parse);`
   - In `persist()`: `PersistedProject { root: p.root.clone(), expanded: p.expanded, shell: p.shell_override.as_ref().map(|c| c.to_state_string()) }`
   - In `add_project_via_dialog`, nothing — new projects have no override.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alacritree` and `cargo check -p alacritree`
Expected: PASS/clean.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/wsl.rs alacritree/src/state.rs alacritree/src/projects.rs alacritree/src/app.rs
git commit -m "feat(state): persist per-project shell override"
```

---

### Task 9: session spawn — WSL shell resolution

**Files:**
- Modify: `alacritree/src/session.rs`
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `wsl::{classify, distros, shell_invocation, ShellChoice}`, `Project.shell_override`.
- Produces: `Session::spawn(ctx, config, working_directory, size, cell_size, shell_override: Option<Shell>)` — new last parameter; `AlacritreeApp::resolve_shell(&self, workspace: &WorkspaceKey) -> Option<Shell>`.

- [ ] **Step 1: Extend `Session::spawn`**

In `session.rs`:

```rust
    pub fn spawn(
        ctx: egui::Context,
        config: &Config,
        working_directory: Option<PathBuf>,
        size: TermSize,
        cell_size: (f32, f32),
        shell_override: Option<Shell>,
    ) -> std::io::Result<Self> {
        // Overrides are argv built in code (`wsl.exe -d <distro> --cd <dir>`),
        // so their args need Windows quoting like diff-pane argv; config
        // shells stay raw to match upstream alacritty.
        let escape_args = shell_override.is_some();
        let shell = shell_override
            .or_else(|| config.shell.as_ref().map(|s| Shell::new(s.program.clone(), s.args.clone())));
        let title = working_directory
            .as_ref()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "shell".to_string());
        Self::spawn_with(
            ctx,
            config,
            working_directory,
            size,
            cell_size,
            shell,
            title,
            SessionKind::Shell,
            escape_args,
        )
    }
```

`spawn_command`: pass `true` as the new trailing `escape_args` argument to `spawn_with` (diff panes always build argv in code).

`spawn_with`: add trailing parameter `escape_args: bool`; replace the `PtyOptions` field computation:

```rust
        #[cfg(not(windows))]
        let _ = escape_args;
        let pty_options = PtyOptions {
            shell,
            working_directory: working_directory.clone(),
            drain_on_exit: false,
            env: config.env.clone(),
            // Windows has no argv: alacritty_terminal joins these args into a
            // single CreateProcess command line, quoting them only when this
            // is set.  True for argv built in code (diff panes, WSL shells),
            // where an arg with a space (delta's pager spec, UNC paths) must
            // survive as one argument; shell args from alacritty.toml stay
            // raw to match upstream alacritty.
            #[cfg(windows)]
            escape_args,
        };
```

- [ ] **Step 2: Resolution in `app.rs`**

Imports: add `use alacritty_terminal::tty::Shell;` and extend the wsl import to `use crate::wsl::{self, ShellChoice};` (adjust the Task 6 import).

Add to `impl AlacritreeApp`:

```rust
    /// Shell for a workspace: the owning project's override wins, then a WSL
    /// location auto-selects that distro's default shell, then the
    /// configured shell.  The home tab (None) always uses the configured
    /// shell.  `None` means "no override" — `Session::spawn` falls through
    /// to alacritty's config-driven shell with its OS-guaranteed fallback.
    fn resolve_shell(&self, workspace: &WorkspaceKey) -> Option<Shell> {
        let path = workspace.as_ref()?;
        let choice = self
            .projects
            .iter()
            .find(|p| p.worktrees.iter().any(|wt| &wt.path == path))
            .and_then(|p| p.shell_override.clone());
        match choice {
            Some(ShellChoice::Windows) => None,
            Some(ShellChoice::Wsl(distro)) => {
                if wsl::distros().iter().any(|d| d.name == distro) {
                    Some(wsl_shell(&distro, path))
                } else {
                    log::warn!("shell override names unknown WSL distro `{distro}`; using auto");
                    auto_wsl_shell(path)
                }
            },
            None => auto_wsl_shell(path),
        }
    }
```

Free functions near the bottom of `app.rs`:

```rust
/// WSL-resident paths get their distro's shell; everything else falls back
/// to the configured shell.
fn auto_wsl_shell(path: &Path) -> Option<Shell> {
    match wsl::classify(path) {
        wsl::Location::Wsl { distro, .. } => Some(wsl_shell(&distro, path)),
        wsl::Location::Windows(_) => None,
    }
}

fn wsl_shell(distro: &str, workdir: &Path) -> Shell {
    let (program, args) = wsl::shell_invocation(distro, workdir);
    Shell::new(program, args)
}
```

Update `spawn_session`:

```rust
        let shell = self.resolve_shell(&working_directory);
        let session = Session::spawn(
            ctx.clone(),
            &self.config,
            working_directory.clone(),
            TermSize::new(80, 24),
            (8.0, 16.0),
            shell,
        )?;
```

- [ ] **Step 3: Verify**

Run: `cargo test -p alacritree` and `cargo check -p alacritree`
Expected: PASS/clean (the compiler will catch any missed `Session::spawn` call site). Manual smoke: activate the WSL project's worktree → the session opens inside `kali-linux` at the right directory (`pwd` shows the Linux path). Home tab and Windows projects still open the configured shell.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add alacritree/src/session.rs alacritree/src/app.rs
git commit -m "feat(session): auto-select WSL shell by project location"
```

---

### Task 10: sidebar context menu — "Open in…"

**Files:**
- Modify: `alacritree/src/app.rs` (left-sidebar project loop, around lines 780–830)

**Interfaces:**
- Consumes: `wsl::distros()`, `ShellChoice`, `Project.shell_override`, `self.persist()`.
- Produces: UI only.

- [ ] **Step 1: Implement**

In the left-sidebar function, before the `SidePanel::left(…)` closure (next to the other snapshots around line 707):

```rust
        let distros = wsl::distros();
        let mut shell_override_changed = false;
```

In the project loop, the name label currently reads:

```rust
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&project.name)
                                            .color(theme.text)
                                            .strong()
                                            .small(),
                                    )
                                    .truncate(),
                                );
```

Capture the response out of the `row_with_trailing` leading closure and attach the menu. Declare `let mut name_resp: Option<egui::Response> = None;` immediately before the `row_with_trailing(…)` call, change the label statement to an assignment with click sense:

```rust
                                name_resp = Some(ui.add(
                                    egui::Label::new(
                                        RichText::new(&project.name)
                                            .color(theme.text)
                                            .strong()
                                            .small(),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                ));
```

and after the `row_with_trailing(…)` call (still inside the loop, before the `if project.expanded` block):

```rust
                        // Right-click: choose which shell this project's
                        // sessions use.  Hidden entirely when no distros are
                        // registered so non-WSL setups see zero new UI.
                        if !distros.is_empty() {
                            if let Some(resp) = name_resp {
                                resp.context_menu(|ui| {
                                    ui.label(
                                        RichText::new("Open in…").color(theme.text_muted).small(),
                                    );
                                    let mark = |selected: bool| if selected { "• " } else { "   " };
                                    let auto = project.shell_override.is_none();
                                    if ui.button(format!("{}Auto (by location)", mark(auto))).clicked()
                                    {
                                        project.shell_override = None;
                                        shell_override_changed = true;
                                        ui.close_menu();
                                    }
                                    let win = matches!(
                                        project.shell_override,
                                        Some(ShellChoice::Windows)
                                    );
                                    if ui.button(format!("{}Windows shell", mark(win))).clicked() {
                                        project.shell_override = Some(ShellChoice::Windows);
                                        shell_override_changed = true;
                                        ui.close_menu();
                                    }
                                    for distro in &distros {
                                        let selected = matches!(
                                            &project.shell_override,
                                            Some(ShellChoice::Wsl(name)) if name == &distro.name
                                        );
                                        if ui
                                            .button(format!("{}WSL ({})", mark(selected), distro.name))
                                            .clicked()
                                        {
                                            project.shell_override =
                                                Some(ShellChoice::Wsl(distro.name.clone()));
                                            shell_override_changed = true;
                                            ui.close_menu();
                                        }
                                    }
                                });
                            }
                        }
```

After the panel (next to `if expand_toggled { self.persist(); }`):

```rust
        if shell_override_changed {
            self.persist();
        }
```

Note: the override applies to sessions spawned *after* the change — existing sessions are untouched (sessions outlive workspace switches, per crate convention). No code needed for that; `resolve_shell` runs at spawn time.

- [ ] **Step 2: Verify**

Run: `cargo check -p alacritree`, `cargo test -p alacritree`
Expected: clean/PASS. Manual: right-click a project name → menu shows Auto / Windows shell / WSL (kali-linux) with the active choice marked; pick WSL on a Windows project → new session opens in the distro at `/mnt/c/...`; pick Windows shell on the WSL project → new session opens PowerShell at the UNC path; restart the app → choice persisted in `state.toml`.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): add per-project shell override menu"
```

---

### Task 11: diff panes for WSL repos

**Files:**
- Modify: `alacritree/src/app.rs` (`open_diff`, `build_diff_command`)

**Interfaces:**
- Consumes: `wsl::{classify, Location}`, existing `DiffRequest`/`DiffSource`, `Session::spawn_command`.
- Produces: `diff_args(req) -> Vec<String>` (shared), `build_wsl_diff_command(distro, workspace, req) -> (String, Vec<String>)`.

- [ ] **Step 1: Refactor and add the WSL command**

Split `build_diff_command` so both arms share the git arguments. Replace the existing function with:

```rust
/// git arguments (everything after `git`) for the requested diff — shared
/// by the Windows and WSL pane commands.
fn diff_args(req: &DiffRequest) -> Vec<String> {
    let mut args = vec!["diff".to_string()];
    match &req.source {
        DiffSource::Staged => args.push("--cached".to_string()),
        DiffSource::Worktree => {},
        // `--no-index` against /dev/null shows the untracked file as a pure
        // addition; git special-cases "/dev/null" on every platform. Exits
        // non-zero by design.
        DiffSource::Untracked => args.push("--no-index".to_string()),
        // Triple-dot diff = "from merge-base to HEAD" — matches the sidebar's
        // `Changes vs <branch>` stat semantics in git_status.rs.
        DiffSource::Branch { base } => args.push(format!("{base}...")),
    }
    args.push("--".to_string());
    if matches!(req.source, DiffSource::Untracked) {
        args.push("/dev/null".to_string());
    }
    args.push(req.file.clone());
    args
}

/// Show the clicked file's `git diff` in delta, wired in as git's pager so git
/// drives the pipe itself.  This drops the POSIX-`sh` dependency the old
/// `sh -c '… | delta'` had — which had no equivalent on Windows, so diffs never
/// opened there.  Paths/branches stay in argv, so no file name is shell-parsed.
fn build_diff_command(req: &DiffRequest) -> (String, Vec<String>) {
    let mut args =
        vec!["-c".to_string(), "core.pager=delta --paging=always".to_string()];
    args.extend(diff_args(req));
    ("git".to_string(), args)
}

/// The same diff run inside the repo's distro.  `sh -l` sources the user's
/// profile so `delta` resolves from their PATH (`--exec` alone only sees the
/// default system PATH; a missing delta prints in the pane, same failure
/// surface as Windows).  Diff arguments travel as positional parameters, so
/// no file name is shell-parsed.
fn build_wsl_diff_command(
    distro: &str,
    workspace: &Path,
    req: &DiffRequest,
) -> (String, Vec<String>) {
    let mut args = vec![
        "-d".to_string(),
        distro.to_string(),
        "--cd".to_string(),
        workspace.to_string_lossy().into_owned(),
        "--exec".to_string(),
        "sh".to_string(),
        "-lc".to_string(),
        r#"exec git -c "core.pager=delta --paging=always" "$@""#.to_string(),
        "sh".to_string(),
    ];
    args.extend(diff_args(req));
    ("wsl.exe".to_string(), args)
}
```

In `open_diff`, replace `let (program, args) = build_diff_command(&req);` with:

```rust
        let (program, args) = match wsl::classify(&workspace) {
            wsl::Location::Wsl { distro, .. } => build_wsl_diff_command(&distro, &workspace, &req),
            wsl::Location::Windows(_) => build_diff_command(&req),
        };
```

- [ ] **Step 2: Verify**

Run: `cargo check -p alacritree`, `cargo test -p alacritree`
Expected: clean/PASS. Manual: in the WSL project, click a modified file in the right sidebar → delta renders in the pane (delta installed inside kali-linux); `q` closes it; a Windows project's diff pane is unchanged.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add alacritree/src/app.rs
git commit -m "feat(diff): open WSL repo diffs via in-distro git"
```

---

### Task 12: worktree create/delete inside the distro

**Files:**
- Modify: `alacritree/src/worktree.rs`

**Interfaces:**
- Consumes: `wsl::{classify, Location, command, run_batch, linux_to_windows, windows_to_linux}`.
- Produces: `spawn_create`/`delete_worktree` signatures unchanged; internal `git_command(cwd) -> Command`, `worktree_base_dir(repo) -> Result<PathBuf, String>`, `git_path_arg(repo, path) -> Result<String, String>`.

- [ ] **Step 1: Route git through the platform layer**

Add `use crate::wsl;` to imports. Add:

```rust
/// `git` primed to run against `cwd`'s repo: `git -C <cwd>` for Windows
/// paths, the same command inside the owning distro for WSL paths.  Path
/// *arguments* for WSL repos must already be Linux paths (`git_path_arg`).
fn git_command(cwd: &Path) -> Command {
    match wsl::classify(cwd) {
        wsl::Location::Windows(path) => {
            let mut cmd = Command::new("git");
            cmd.hide_console().arg("-C").arg(path);
            cmd
        },
        wsl::Location::Wsl { distro, linux_path } => {
            let mut cmd = wsl::command(&distro, None);
            cmd.arg("git").arg("-C").arg(linux_path);
            cmd
        },
    }
}

/// The form of `path` git receives as an argument: Linux for WSL repos
/// (in-distro git can't resolve UNC paths), the Windows string otherwise.
fn git_path_arg(repo: &Path, path: &Path) -> Result<String, String> {
    match wsl::classify(repo) {
        wsl::Location::Windows(_) => {
            Ok(path.to_str().ok_or("invalid worktree path")?.to_string())
        },
        wsl::Location::Wsl { .. } => wsl::windows_to_linux(path)
            .ok_or_else(|| "worktree path is outside the distro".to_string()),
    }
}
```

Rewrite the four spawners to use `git_command(cwd)` instead of `Command::new("git").hide_console().arg("-C").arg(cwd)`:
- `run_git`: `let output = git_command(cwd).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).output()…` (rest identical)
- `has_remote`: `git_command(cwd).args(["remote", "get-url", name])…`
- `rev_parse_verify`: `git_command(cwd).args(["rev-parse", "--verify", "--quiet", name])…`
- `query_origin_head`: `git_command(cwd).args(["ls-remote", "--symref", "origin", "HEAD"])…`

- [ ] **Step 2: Worktree location inside the distro**

Replace `project_worktree_dir` with:

```rust
/// Worktrees live under `<home>/.alacritree/worktrees/<project>-<hash>/` —
/// the *distro's* home for WSL repos, so the worktree stays on the Linux
/// filesystem next to its repo instead of crossing onto 9P-mounted NTFS.
/// The path hash disambiguates same-named repos in different locations.
fn project_worktree_dir(repo: &Path) -> Result<PathBuf, String> {
    let home = match wsl::classify(repo) {
        wsl::Location::Windows(_) => {
            home::home_dir().ok_or_else(|| "could not locate home directory".to_string())?
        },
        wsl::Location::Wsl { distro, .. } => {
            let stdout = wsl::run_batch(&distro, r#"printf '%s' "$HOME""#, &[])
                .map_err(|e| format!("could not query WSL home: {e}"))?;
            let linux_home = String::from_utf8_lossy(&stdout).trim().to_string();
            if linux_home.is_empty() {
                return Err("could not determine the distro home directory".into());
            }
            wsl::linux_to_windows(&linux_home, &distro)
        },
    };
    let canonical = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let project_name = canonical
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "project".to_string());

    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    let hash = hasher.finish() as u32;

    Ok(home.join(".alacritree").join("worktrees").join(format!("{project_name}-{hash:08x}")))
}
```

(`pick_worktree_path` is unchanged — `create_dir_all`/`exists()` work over the `\\wsl.localhost\` share, as do `copy_llm_configs` and `enable_claude_terminal_bell` afterwards.)

- [ ] **Step 3: Translate the path arguments**

In `run_create`, the `worktree add` call becomes:

```rust
    send("Creating git worktree…");
    let target = pick_worktree_path(&req.project_root, &req.branch)?;
    let target_arg = git_path_arg(&req.project_root, &target)?;
    run_git(
        &req.project_root,
        &["worktree", "add", &target_arg, "-b", &req.branch, &base_ref],
    )?;
```

In `delete_worktree`, replace the `path_str` line:

```rust
    let path_arg = git_path_arg(project_root, worktree_path)?;
    let mut args: Vec<&str> = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_arg);
```

- [ ] **Step 4: Verify**

Run: `cargo check -p alacritree`, `cargo test -p alacritree`
Expected: clean/PASS. Manual (WSL project with an `origin` remote): `+` on the project → create branch `wsl-test` → progress steps stream, worktree appears in the sidebar under the distro home, clicking it opens a distro shell there; `×` → delete succeeds, branch gone (`git branch` inside the distro confirms).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add alacritree/src/worktree.rs
git commit -m "feat(worktree): create and delete worktrees in WSL repos"
```

---

### Task 13: PR status via in-distro `gh`

**Files:**
- Modify: `alacritree/src/pr_status.rs`

**Interfaces:**
- Consumes: `wsl::{classify, Location, command}`.
- Produces: `query_gh` handles WSL paths; everything else (TTL cache, `parse_gh_output`) unchanged.

- [ ] **Step 1: Implement**

Add `use crate::wsl;` to imports. Replace `query_gh`'s command construction:

```rust
fn query_gh(path: &Path, branch: &str) -> Option<PrInfo> {
    let mut cmd = match wsl::classify(path) {
        wsl::Location::Windows(p) => {
            let mut c = Command::new("gh");
            c.hide_console().current_dir(p);
            c
        },
        // `gh` must be installed and authenticated *inside* the distro; any
        // failure falls back to the default branch, same as a missing
        // Windows gh.  `--cd` accepts the UNC path natively.
        wsl::Location::Wsl { distro, .. } => {
            let mut c = wsl::command(&distro, Some(path));
            c.arg("gh");
            c
        },
    };
    let output = cmd
        .args(["pr", "view", branch, "--json", "number,baseRefName,url"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_gh_output(&output.stdout)
}
```

- [ ] **Step 2: Verify**

Run: `cargo test -p alacritree` (existing `pr_status` tests still pass), `cargo check -p alacritree`
Expected: PASS/clean. Manual: in a WSL worktree whose branch has an open PR (and `gh auth status` succeeds inside the distro), the sidebar shows the PR badge and diffs against the PR base.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add alacritree/src/pr_status.rs
git commit -m "feat(pr-status): query gh inside the owning distro"
```

---

### Task 14: full-build verification and manual E2E checklist

**Files:** none (verification only).

- [ ] **Step 1: Full automated pass**

```bash
cargo fmt --check
cargo test -p alacritree
cargo test -p alacritree -- --ignored   # live WSL round trip, dev machine only
cargo build -p alacritree --release
```

Expected: all green. If `cargo fmt --check` fails, run `cargo fmt` and amend the offending commit is NOT allowed — make a `style:` commit instead.

- [ ] **Step 2: Manual E2E checklist (release build, against `kali-linux`)**

Run `target/release/alacritree.exe` and verify each item; report results to the user for the GUI-acceptance parts:

1. Add `\\wsl.localhost\kali-linux\home\<user>\<repo>` via `+` → row appears instantly, worktrees + branches fill in without freezing.
2. Activating a worktree opens a session inside the distro at the right cwd (`pwd`).
3. Right sidebar matches `git status` inside the distro on a repo containing `+x` files (no phantom modifications) and lists `Changes vs <branch>`.
4. Worktree create → lands under the distro's `~/.alacritree/worktrees/…`, LLM configs copied; delete removes it and the branch.
5. Diff pane renders via in-distro delta; untracked-file diff works; `q` closes.
6. PR badge appears for a branch with an open PR (`gh` authed in-distro).
7. Right-click project → Open in… overrides work both directions; a Windows project overridden to WSL opens at `/mnt/c/…`; persists across restart.
8. `wsl --shutdown`, then activate the WSL workspace → first refresh takes seconds, then recovers; no crash, no error spam.
9. Windows-only regression pass: home tab shell, a Windows project's status/diff/worktree-create all behave exactly as before.
10. `[ui.wsl] automount_root` absent → everything above works with `/mnt` default (setting it is only needed on hosts with a customized wsl.conf).

- [ ] **Step 3: Report**

Summarize results; any failure loops back to its owning task. Do not push or open a PR — the user decides (per their PR rules).

---

## Self-review notes (already applied)

- Spec coverage: §1→Tasks 1,2,4; §2→Tasks 8,9,10 (+11 for pane spawn); §3→Tasks 5,7,12,13; §4→Task 6 (async discovery; status polling already active-workspace-only); §5→error paths embedded per task; §6→tests per task + Task 14 checklist; automount config→Task 3. v2 helper (§7) is spec-only, no task by design.
- Type consistency: `wsl::Location`, `ShellChoice`, `run_batch(distro, script, args)`, `command(distro, cd)`, `shell_invocation` — names identical at every consumer; `Session::spawn` gains exactly one trailing param, `spawn_with` one trailing `escape_args: bool`.
- Known accepted trade-offs (from the approved spec): `dirty_counts` on delete-click stalls ~400 ms for WSL repos (one-shot, explicit action); WSL worktree names are path basenames rather than git's internal worktree names (delete is path-based, so this is display-only).
