//! `alacritree crashes` — crashed and indeterminate sessions, newest first.
//!
//! `session_begin` creates an artifact on every GUI launch, not only a crash
//! one — that is what makes "header present, no exit marker, dead pid"
//! detectable at all. Most launches exit cleanly, so by default this shows
//! only what [`classify`] calls `Crashed` or `Indeterminate`; `--all` also
//! shows clean exits and still-running sessions.
//!
//! Strictly read-only.  The artifacts are per-process files that nothing
//! merges on disk; this derives the single view instead, so it can run at any
//! time without coordinating with a live instance.

use std::path::Path;

use serde_json::{Value, json};

use crate::crash_log::{Verdict, classify};
use crate::logdir::{self, ProcessId};

struct Artifact {
    name: String,
    id: ProcessId,
    bytes: Vec<u8>,
    verdict: Verdict,
}

pub fn run(as_json: bool, all: bool) -> i32 {
    let Some(dir) = logdir::log_dir() else {
        eprintln!("alacritree: no log directory on this platform");
        return 1;
    };
    let artifacts = select(collect(&dir), all);

    if as_json {
        println!("{:#}", to_json(&artifacts));
    } else {
        print_human(&artifacts);
    }
    0
}

fn collect(dir: &Path) -> Vec<Artifact> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };

    let mut artifacts: Vec<Artifact> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let id = logdir::parse_name("crash-", &name)?;
            // A file pruned by a concurrently starting instance is expected;
            // anything else read stops being reported rather than aborting the
            // whole listing.
            let bytes = std::fs::read(entry.path()).ok()?;
            let verdict = classify(&entry.path(), id.pid);
            Some(Artifact { name, id, bytes, verdict })
        })
        .collect();

    artifacts.sort_by(|a, b| {
        logdir::sort_key(&a.id).cmp(&logdir::sort_key(&b.id)).then_with(|| a.name.cmp(&b.name))
    });
    artifacts
}

/// A `Running` artifact belongs to a live process that has not crashed, so it
/// is hidden right alongside `Clean` unless `--all` asks for everything.
fn select(artifacts: Vec<Artifact>, all: bool) -> Vec<Artifact> {
    if all {
        return artifacts;
    }
    artifacts
        .into_iter()
        .filter(|a| matches!(a.verdict, Verdict::Crashed | Verdict::Indeterminate))
        .collect()
}

fn print_human(artifacts: &[Artifact]) {
    use std::io::Write;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for artifact in artifacts {
        let _ = writeln!(out, "==> {} <==", artifact.name);
        // Byte-copy: a truncated artifact may not be valid UTF-8, and refusing
        // to print a damaged crash record is the one unacceptable outcome.
        let _ = out.write_all(&artifact.bytes);
        let _ = writeln!(out);
    }
}

fn to_json(artifacts: &[Artifact]) -> Value {
    Value::Array(
        artifacts
            .iter()
            .map(|a| {
                json!({
                    "name": a.name,
                    "start": a.id.start as u64,
                    "pid": a.id.pid,
                    "contents": String::from_utf8_lossy(&a.bytes),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("seed");
    }

    #[test]
    fn artifacts_are_ordered_newest_first_then_by_ordinal() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "crash-10-1.log", "old\n");
        seed(dir.path(), "crash-20-2.log", "new-a\n");
        seed(dir.path(), "crash-20-2-1.log", "new-b\n");

        let listed = collect(dir.path());

        let names: Vec<_> = listed.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["crash-20-2.log", "crash-20-2-1.log", "crash-10-1.log"]);
    }

    /// A truncated artifact may not be valid UTF-8, and the one thing a crash
    /// reader must not do is refuse to show a damaged record.
    #[test]
    fn invalid_utf8_is_still_rendered() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("crash-1-1.log"), [0x74, 0x78, 0xff, 0x0a]).unwrap();

        let listed = collect(dir.path());

        assert_eq!(listed.len(), 1);
        assert!(!listed[0].bytes.is_empty(), "a damaged artifact was dropped");
    }

    #[test]
    fn unrelated_files_are_ignored() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "state.toml", "x");
        seed(dir.path(), "alacritree-1-1.log", "continuous");

        assert!(collect(dir.path()).is_empty());
    }

    /// Nothing has crashed is a success, not an error.
    #[test]
    fn a_missing_directory_lists_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let absent = dir.path().join("gone");

        assert!(collect(&absent).is_empty());
    }

    /// `session_begin` writes an artifact on every launch, so most artifacts on
    /// disk are clean exits — the default view must not bury the crash among
    /// them.
    #[test]
    fn a_clean_session_is_hidden_by_default_and_shown_with_all() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "crash-1-1.log", "t1 start v pid=1\nt2 exit ok\n");

        assert!(select(collect(dir.path()), false).is_empty(), "a clean session was shown");
        assert_eq!(select(collect(dir.path()), true).len(), 1, "--all did not show it");
    }

    /// A live process pins its own pid, so a header with no exit marker and a
    /// live pid is `Running`, not a crash — and `Running` is hidden by default
    /// too, since nothing has crashed yet.
    #[test]
    fn a_running_session_is_hidden_by_default_and_shown_with_all() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let body = format!("t1 start v pid={}\n", std::process::id());
        seed(dir.path(), &format!("crash-1-{}.log", std::process::id()), &body);

        assert!(select(collect(dir.path()), false).is_empty(), "a running session was shown");
        assert_eq!(select(collect(dir.path()), true).len(), 1, "--all did not show it");
    }

    #[test]
    fn a_crashed_session_is_shown_by_default() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "crash-1-1.log", "t1 start v pid=1\nt2 PANIC thread=main\n");

        assert_eq!(select(collect(dir.path()), false).len(), 1, "a crashed session was hidden");
    }

    /// A headerless artifact cannot be told apart from a clean one with any
    /// confidence, so it must err toward being shown rather than hidden.
    #[test]
    fn an_indeterminate_session_is_shown_by_default() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "crash-1-1.log", "no header here\n");

        assert_eq!(
            select(collect(dir.path()), false).len(),
            1,
            "an indeterminate session was hidden"
        );
    }

    #[test]
    fn the_json_shape_carries_the_parsed_identity() {
        let dir = tempfile::tempdir().expect("a temp dir");
        seed(dir.path(), "crash-42-7.log", "body\n");

        let value = to_json(&collect(dir.path()));

        let first = &value.as_array().expect("an array")[0];
        assert_eq!(first["name"], "crash-42-7.log");
        assert_eq!(first["start"], 42);
        assert_eq!(first["pid"], 7);
        assert_eq!(first["contents"], "body\n");
    }
}
