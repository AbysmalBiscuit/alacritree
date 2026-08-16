//! Whether a worktree's checkout is still on disk, for the sidebar's benefit.
//!
//! `Project::discover` owns `Worktree::prunable` and stays the only writer of
//! it: the delete flow reads that flag to choose between `git worktree remove`
//! and a prune, and `Project::apply` reads it to decide which rows survive a
//! refresh.  This cache never touches it.  It answers one question — "should
//! this row paint as gone?" — so a wrong answer costs a frame of styling and
//! can never pick a destructive branch.  Every action stats the path itself at
//! the moment it runs.
//!
//! Cost is the whole design.  Probing is a syscall per path, and on a
//! `\\wsl.localhost\` UNC path that is a 9P round trip, so probes run on a
//! worker, one batch at a time, only for rows the sidebar is drawing, and only
//! once per interval.  `wants_probe` is what the paint path asks first: on the
//! other ~89 frames of every 90 it is false and nothing here allocates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a batch of results stands before the visible rows are checked
/// again.  Matches `git_status::StatusCache`, which answers the same "did this
/// worktree change under us" question at the same human timescale.
pub const FRESH_FOR: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Present,
    Missing,
    /// The probe failed for a reason other than "not found" — a distro that
    /// did not answer, a permission error.  Distinct from `Missing` because a
    /// filesystem that cannot be reached is not a filesystem without the
    /// directory, and greying the row on that would be a lie.
    Unknown,
}

/// Whether `path` is still a worktree checkout, which is `.git`'s presence
/// rather than the directory's.  `git worktree remove` deletes the contents
/// first and only then the directory itself, so a remove that loses the last
/// step — the usual outcome on Windows, where a shell sitting in the directory
/// pins it — leaves an empty husk behind.  Git calls that worktree gone and
/// refuses to remove it twice ("validation failed: '<path>/.git' does not
/// exist"); stat'ing the directory would call it alive.
///
/// `metadata` rather than `exists` so the difference between "not there" and
/// "could not tell" survives: `exists` folds every error into `false`.
pub fn probe(path: &Path) -> Liveness {
    match std::fs::metadata(path.join(".git")) {
        Ok(_) => Liveness::Present,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Liveness::Missing,
        Err(_) => Liveness::Unknown,
    }
}

/// Probe results keyed by worktree path, plus when the next batch is due.
/// Entries live only as long as the sidebar keeps drawing their path, so a
/// project the user removes does not leave its worktrees behind.
#[derive(Default)]
pub struct LivenessCache {
    states: HashMap<PathBuf, Liveness>,
    /// `None` until the first batch lands, which is what makes the first
    /// painted frame probe rather than wait out an interval.
    next_probe: Option<Instant>,
}

impl LivenessCache {
    /// Whether `path` is gone, or `None` when no definite answer exists and
    /// the row should keep discovery's word.  A definite answer overrides that
    /// word in *both* directions: a checkout restored under a path discovery
    /// last saw as pruned has to lose the grey, or this fixes one stale
    /// direction and leaves its mirror image behind.
    pub fn missing(&self, path: &Path) -> Option<bool> {
        match self.states.get(path)? {
            Liveness::Present => Some(false),
            Liveness::Missing => Some(true),
            Liveness::Unknown => None,
        }
    }

    /// Whether the interval has elapsed.  The sidebar asks this *before* it
    /// starts collecting the paths it draws, so a steady frame does no work
    /// and makes no allocation on this path at all.
    pub fn wants_probe(&self, now: Instant) -> bool {
        self.next_probe.is_none_or(|due| now >= due)
    }

    /// Take the batch to probe, forgetting every path the sidebar no longer
    /// draws.  All visible paths go in together: they are checked on one
    /// worker, so splitting them by individual freshness would buy nothing.
    pub fn batch(&mut self, visible: &[PathBuf]) -> Vec<PathBuf> {
        self.states.retain(|path, _| visible.contains(path));
        visible.to_vec()
    }

    /// An `Unknown` result keeps whatever the last definite answer was: a
    /// distro that stops answering must not silently grey out every row it
    /// owns.  The interval restarts either way, so an unreachable path is
    /// retried on the tick rather than on every frame.
    pub fn adopt(&mut self, results: Vec<(PathBuf, Liveness)>, now: Instant) {
        for (path, state) in results {
            match state {
                Liveness::Unknown => {
                    self.states.entry(path).or_insert(Liveness::Unknown);
                },
                definite => {
                    self.states.insert(path, definite);
                },
            }
        }
        self.next_probe = Some(now + FRESH_FOR);
    }

    /// How long until the next batch is due, for `request_repaint_after`.
    /// Without that wake-up the sidebar would only re-probe when something
    /// else happened to draw a frame, and a worktree deleted from an otherwise
    /// idle terminal would stay marked live indefinitely.
    pub fn wait(&self, now: Instant) -> Duration {
        self.next_probe.map_or(Duration::ZERO, |due| due.saturating_duration_since(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn a_checkout_with_its_git_link_is_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "gitdir: /somewhere/.git/worktrees/x").unwrap();

        assert_eq!(probe(dir.path()), Liveness::Present);
    }

    /// `git worktree remove` deletes the contents and only then the directory,
    /// so on Windows a shell sitting in it leaves this behind.  Git treats the
    /// worktree as gone; stat'ing the directory would not.
    #[test]
    fn the_husk_left_by_a_half_finished_remove_is_missing() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(probe(dir.path()), Liveness::Missing);
    }

    #[test]
    fn a_checkout_deleted_outright_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        drop(dir);

        assert_eq!(probe(&path), Liveness::Missing);
    }

    #[test]
    fn the_first_frame_probes_rather_than_waiting_out_an_interval() {
        assert!(LivenessCache::default().wants_probe(Instant::now()));
    }

    #[test]
    fn a_batch_holds_the_interval_shut_until_it_expires() {
        let now = Instant::now();
        let mut cache = LivenessCache::default();
        cache.adopt(vec![(p("/a"), Liveness::Present)], now);

        assert!(!cache.wants_probe(now + FRESH_FOR / 2), "the steady frame does nothing");
        assert!(cache.wants_probe(now + FRESH_FOR));
    }

    #[test]
    fn only_a_definite_answer_greys_the_row() {
        let now = Instant::now();
        let mut cache = LivenessCache::default();

        cache.adopt(vec![(p("/a"), Liveness::Present)], now);
        assert_eq!(cache.missing(&p("/a")), Some(false));

        cache.adopt(vec![(p("/a"), Liveness::Missing)], now);
        assert_eq!(cache.missing(&p("/a")), Some(true));
    }

    /// A distro that stops answering turns every path it owns `Unknown`.
    /// Letting that grey the rows would report a deleted worktree every time
    /// WSL hiccups.
    #[test]
    fn an_unknown_result_leaves_the_last_answer_standing() {
        let now = Instant::now();
        let mut cache = LivenessCache::default();
        cache.adopt(vec![(p("/a"), Liveness::Missing)], now);

        cache.adopt(vec![(p("/a"), Liveness::Unknown)], now + FRESH_FOR);

        assert_eq!(cache.missing(&p("/a")), Some(true), "the last definite answer stands");
        assert!(
            !cache.wants_probe(now + FRESH_FOR),
            "but the failed probe still restarts the tick"
        );
    }

    /// `git worktree add` on the same path brings the checkout back; the row
    /// has to lose the grey again rather than stay marked gone.
    #[test]
    fn a_path_that_reappears_goes_back_to_present() {
        let now = Instant::now();
        let mut cache = LivenessCache::default();
        cache.adopt(vec![(p("/a"), Liveness::Missing)], now);

        cache.adopt(vec![(p("/a"), Liveness::Present)], now + FRESH_FOR);

        assert_eq!(cache.missing(&p("/a")), Some(false));
    }

    #[test]
    fn a_path_the_sidebar_stopped_drawing_is_forgotten() {
        let now = Instant::now();
        let mut cache = LivenessCache::default();
        cache.adopt(vec![(p("/a"), Liveness::Missing)], now);

        assert_eq!(cache.batch(&[p("/b")]), vec![p("/b")]);

        assert_eq!(cache.missing(&p("/a")), None, "the entry went with the row");
    }

    #[test]
    fn the_wait_counts_down_to_the_next_batch() {
        let now = Instant::now();
        let mut cache = LivenessCache::default();
        cache.adopt(vec![(p("/a"), Liveness::Present)], now);

        assert_eq!(cache.wait(now + FRESH_FOR / 2), FRESH_FOR / 2);
        // An overdue tick asks for the next frame, not a negative span.
        assert_eq!(cache.wait(now + FRESH_FOR * 2), Duration::ZERO);
    }
}
