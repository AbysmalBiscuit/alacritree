//! Branch upstream state for the sidebar badge.

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
}
