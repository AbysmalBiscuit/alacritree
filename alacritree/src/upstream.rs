//! Branch upstream state for the sidebar badge.

use std::collections::HashMap;

/// What a branch's configured upstream is doing.  Nothing here contacts a
/// remote, so every state describes local refs only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamState {
    /// Tracks `upstream` and matches it.
    Level { upstream: String },
    /// Tracks `upstream` and differs from it.
    Diverged { upstream: String, ahead: usize, behind: usize },
    /// `branch.<name>.remote` and `.merge` name `upstream`, but no such
    /// reference exists locally.  A merged-and-deleted remote branch looks
    /// like this only once something has pruned.
    Gone { upstream: String },
    /// No upstream is configured.  Not the same as "never pushed": `git push
    /// origin <branch>` without `-u` pushes without configuring one.
    Untracked,
}

impl UpstreamState {
    pub fn upstream_name(&self) -> Option<&str> {
        match self {
            UpstreamState::Level { upstream }
            | UpstreamState::Diverged { upstream, .. }
            | UpstreamState::Gone { upstream } => Some(upstream),
            UpstreamState::Untracked => None,
        }
    }
}

/// Parse the tab-delimited output of
/// `git for-each-ref --format='%(refname:short)%09%(upstream:short)%09%(upstream:track,nobracket)'`.
/// The track field is empty for a level branch, absent-upstream branches
/// carry an empty upstream, and `gone` marks a configured upstream whose ref
/// no longer resolves. Callers must run git under `LC_ALL=C`: the track
/// vocabulary is localized.
pub fn parse_for_each_ref(bytes: &[u8]) -> HashMap<String, UpstreamState> {
    let text = String::from_utf8_lossy(bytes);
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let (Some(branch), Some(upstream), Some(track)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if branch.is_empty() {
            continue;
        }
        if let Some(state) = classify(upstream, track) {
            map.insert(branch.to_string(), state);
        }
    }
    map
}

fn classify(upstream: &str, track: &str) -> Option<UpstreamState> {
    if upstream.is_empty() {
        return Some(UpstreamState::Untracked);
    }
    let upstream = upstream.to_string();
    match track.trim() {
        "" => Some(UpstreamState::Level { upstream }),
        "gone" => Some(UpstreamState::Gone { upstream }),
        track => match parse_track_counts(track) {
            Some((ahead, behind)) => Some(UpstreamState::Diverged { upstream, ahead, behind }),
            // A track string we cannot read means a git whose vocabulary
            // changed. No entry, so the row paints nothing — better than
            // "0 ahead, 0 behind" on a branch that is neither.
            None => None,
        },
    }
}

/// `ahead 2`, `behind 3`, or `ahead 2, behind 3`. `None` when no clause
/// parsed, which is the only signal that the vocabulary was not what
/// `LC_ALL=C` promised.
fn parse_track_counts(track: &str) -> Option<(usize, usize)> {
    let mut ahead = 0;
    let mut behind = 0;
    let mut parsed = false;
    for part in track.split(',') {
        let mut words = part.split_whitespace();
        match (words.next(), words.next().and_then(|n| n.parse().ok())) {
            (Some("ahead"), Some(n)) => (ahead, parsed) = (n, true),
            (Some("behind"), Some(n)) => (behind, parsed) = (n, true),
            _ => return None,
        }
    }
    parsed.then_some((ahead, behind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_name_is_exposed_for_tracked_states_only() {
        assert_eq!(
            UpstreamState::Level { upstream: "origin/x".into() }.upstream_name(),
            Some("origin/x")
        );
        assert_eq!(
            UpstreamState::Diverged { upstream: "origin/x".into(), ahead: 1, behind: 2 }
                .upstream_name(),
            Some("origin/x")
        );
        assert_eq!(
            UpstreamState::Gone { upstream: "origin/x".into() }.upstream_name(),
            Some("origin/x")
        );
        assert_eq!(UpstreamState::Untracked.upstream_name(), None);
    }

    #[test]
    fn parses_every_upstream_state() {
        let out = b"level\torigin/level\t\n\
                    ahead\torigin/ahead\tahead 2\n\
                    behind\torigin/behind\tbehind 3\n\
                    both\torigin/both\tahead 2, behind 3\n\
                    dead\torigin/dead\tgone\n\
                    solo\t\t\n";
        let map = parse_for_each_ref(out);
        assert_eq!(map["level"], UpstreamState::Level { upstream: "origin/level".into() });
        assert_eq!(
            map["ahead"],
            UpstreamState::Diverged { upstream: "origin/ahead".into(), ahead: 2, behind: 0 }
        );
        assert_eq!(
            map["behind"],
            UpstreamState::Diverged { upstream: "origin/behind".into(), ahead: 0, behind: 3 }
        );
        assert_eq!(
            map["both"],
            UpstreamState::Diverged { upstream: "origin/both".into(), ahead: 2, behind: 3 }
        );
        assert_eq!(map["dead"], UpstreamState::Gone { upstream: "origin/dead".into() });
        assert_eq!(map["solo"], UpstreamState::Untracked);
    }

    /// `|` is legal in a ref name, which is why the format is tab-delimited —
    /// git forbids ASCII control characters in refs.
    #[test]
    fn a_branch_name_containing_a_pipe_survives() {
        let map = parse_for_each_ref(b"feat|x\torigin/feat|x\t\n");
        assert_eq!(map["feat|x"], UpstreamState::Level { upstream: "origin/feat|x".into() });
    }

    #[test]
    fn malformed_and_empty_input_yield_no_entries() {
        assert!(parse_for_each_ref(b"").is_empty());
        assert!(parse_for_each_ref(b"\n\n").is_empty());
        assert!(parse_for_each_ref(b"no-tabs-at-all\n").is_empty());
    }

    /// A track string we cannot read must produce no entry at all.  Falling
    /// through to `Diverged { ahead: 0, behind: 0 }` would paint the divergence
    /// glyph and a "0 ahead, 0 behind" tooltip on a branch that is neither.
    #[test]
    fn an_unreadable_track_string_yields_no_entry() {
        assert!(parse_for_each_ref(b"b\torigin/b\tvorne 2\n").is_empty());
        assert!(parse_for_each_ref(b"b\torigin/b\tahead many\n").is_empty());
    }
}
