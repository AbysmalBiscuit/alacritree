//! Ablation switches and synthetic keystrokes for the load-latency
//! diagnosis.
//!
//! Passive timings say where a frame's time went; they cannot say what caused
//! it.  Removing one mechanism and re-measuring can, which is what settled
//! the last investigation of this kind.  Everything here is off unless an
//! environment variable asks for it, so a normal run pays one atomic read per
//! guarded site.
//!
//! Environment rather than config, matching `ALACRITREE_FRAME_LOG`: a switch
//! then costs no schema regeneration and no edit to the config the render
//! track is also changing.
//!
//! ```text
//! ALACRITREE_ABLATE=sidebars,gitpoll   drop both from the frame
//! ALACRITREE_ABLATE=repaint=8          coalesce output-driven repaints to 8ms
//! ALACRITREE_SYNTH_KEYS=50             type one character every 50ms
//! ```

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Switches {
    grid: bool,
    sidebars: bool,
    jobs: bool,
    git_poll: bool,
    /// Shortest gap between two repaints asked for by arriving output.
    repaint: Option<Duration>,
}

fn switches() -> &'static Switches {
    static SWITCHES: OnceLock<Switches> = OnceLock::new();
    SWITCHES.get_or_init(|| {
        let Some(raw) = std::env::var_os("ALACRITREE_ABLATE") else {
            return Switches::default();
        };
        let raw = raw.to_string_lossy().into_owned();
        let mut switches = Switches::default();
        for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let (name, value) = token.split_once('=').unwrap_or((token, ""));
            match name {
                "grid" => switches.grid = true,
                "sidebars" => switches.sidebars = true,
                // Every background job, the poller included: the switch names
                // the class, so pausing it must not leave one running.
                "jobs" => {
                    switches.jobs = true;
                    switches.git_poll = true;
                },
                "gitpoll" => switches.git_poll = true,
                "repaint" => {
                    switches.repaint = value.parse().ok().map(Duration::from_millis);
                },
                other => log::warn!("unknown ablation {other:?}"),
            }
        }
        log::info!("ablating: {raw}");
        switches
    })
}

/// Whether the terminal grid should be left unpainted.
pub fn skip_grid() -> bool {
    switches().grid
}

/// Whether both sidebars and the git panel should be left out of the frame.
pub fn skip_sidebars() -> bool {
    switches().sidebars
}

/// Whether background jobs other than the git poller should stay unspawned.
pub fn pause_jobs() -> bool {
    switches().jobs
}

/// Whether the git status poller should stay unspawned.
pub fn pause_git_poll() -> bool {
    switches().git_poll
}

/// When the last output-driven repaint was asked for, as nanoseconds since
/// [`epoch`].  Written from the PTY threads, which own no shared state
/// beyond this.
static LAST_OUTPUT_REPAINT: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Ask for a frame on behalf of output that just arrived.
///
/// Unablated this is `request_repaint`.  Under `repaint=<ms>` a frame that
/// would land inside the interval is deferred to its end instead, so
/// streaming output stops setting the frame rate while input-driven repaints
/// keep going straight through.
pub fn request_output_repaint(ctx: &egui::Context) {
    let Some(interval) = switches().repaint else {
        ctx.request_repaint();
        return;
    };

    let now = epoch().elapsed().as_nanos() as u64;
    let last = LAST_OUTPUT_REPAINT.load(Ordering::Relaxed);
    let since = Duration::from_nanos(now.saturating_sub(last));
    if last == 0 || since >= interval {
        LAST_OUTPUT_REPAINT.store(now, Ordering::Relaxed);
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(interval - since);
    }
}

/// A synthetic typist, so a run can produce enough keystrokes for a p99.
///
/// The characters go into the raw input queue rather than through IPC,
/// because the wait for a frame to run before the byte reaches the PTY is the
/// part that hurts and IPC skips it.  The terminal has to hold focus for the
/// session to receive them.
pub struct SynthKeys {
    interval: Duration,
    next: Instant,
}

impl SynthKeys {
    /// A typist if `ALACRITREE_SYNTH_KEYS` names an interval in milliseconds,
    /// otherwise nothing.  `ALACRITREE_SYNTH_DELAY` holds it back for that many
    /// seconds first.
    ///
    /// The delay is what separates typing from typing-at-a-shell-that-is-still-
    /// starting.  Under load the second takes seconds, and a typist that begins
    /// with the window measures it as though it were the round trip.
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("ALACRITREE_SYNTH_KEYS").ok()?;
        let millis: u64 = raw.trim().parse().ok()?;
        let interval = Duration::from_millis(millis.max(1));
        let delay = std::env::var("ALACRITREE_SYNTH_DELAY")
            .ok()
            .and_then(|raw| raw.trim().parse().ok())
            .map_or(interval, Duration::from_secs);
        log::info!("synthesizing a keystroke every {interval:?}, starting in {delay:?}");
        Some(Self { interval, next: Instant::now() + delay })
    }

    /// Add this frame's keystroke, if one is due.
    ///
    /// The cadence is held by asking for the frame that carries the next one:
    /// under load nothing else would wake the loop on time, and a typist that
    /// only fires when something else already did would measure the frames
    /// that were going to happen anyway.
    pub fn inject(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let now = Instant::now();
        if now >= self.next {
            raw.events.push(egui::Event::Text("a".into()));
            crate::frame_log::typist_late(now - self.next);
            self.next = now + self.interval;
        }
        ctx.request_repaint_after(self.next.saturating_duration_since(now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typist that fired this frame must not fire again until the interval
    /// has passed, or the sample measures a burst rather than typing.
    #[test]
    fn a_keystroke_is_injected_once_per_interval() {
        let ctx = egui::Context::default();
        let mut keys = SynthKeys { interval: Duration::from_millis(50), next: Instant::now() };
        let mut raw = egui::RawInput::default();

        keys.inject(&ctx, &mut raw);
        keys.inject(&ctx, &mut raw);

        assert_eq!(raw.events.len(), 1);
    }

    /// Printable input reaches the terminal as text, not as a key press:
    /// `input.rs` prefers `Text` so dead keys and IME behave.
    #[test]
    fn the_injected_keystroke_is_text() {
        let ctx = egui::Context::default();
        let mut keys = SynthKeys { interval: Duration::from_millis(1), next: Instant::now() };
        let mut raw = egui::RawInput::default();

        keys.inject(&ctx, &mut raw);

        assert!(matches!(raw.events.first(), Some(egui::Event::Text(_))));
    }
}
