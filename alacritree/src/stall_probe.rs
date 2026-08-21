//! Diagnostic instrumentation for the large-write PTY stall.  Never merge.
//!
//! A terminal fed one 64 MiB write freezes periodically; fed the same bytes as
//! 5 KiB writes it does not.  Earlier passes ruled out the paint path (lock
//! held under a millisecond, occupancy at two percent), the read buffer
//! (backlog zero through a stall), and a lost repaint (wakeups stop exactly
//! when frames do).  What is left is the reader thread itself: through a stall
//! it issues no reads at all, and the gap between two reads reaches half a
//! second.
//!
//! That gap has two possible occupants — a parse holding the terminal, or a
//! poll that nothing woke — and the per-second summary cannot tell them apart,
//! because during a stall the painter is not running to record a wait either.
//! So a watchdog samples the terminal lock from outside both threads: locked
//! through the stall means the parse, free through the stall means the poll.
//! It also reports whether the last drain came up empty, which separates "the
//! console gave us nothing" from "we left bytes in the pipe and slept anyway".
//!
//! Enabled by `ALACRITREE_STALL_PROBE=1`, silent otherwise.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty::PTY_READ_WRITE_TOKEN;
use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, Poller};

use crate::session::EventProxy;

/// Mirrors `alacritty_terminal::event_loop::READ_BUFFER_SIZE`, which is
/// private.  The reader's slice is this minus whatever is unparsed, so a wrong
/// value here shows up as a constant offset in the backlog.
const READ_BUFFER_SIZE: usize = 0x10_0000;

const SUMMARY_EVERY: Duration = Duration::from_secs(1);

/// How long without a read counts as a stall worth a line of its own.
const STALL_AFTER: Duration = Duration::from_millis(100);

const WATCHDOG_TICK: Duration = Duration::from_millis(5);

static MAX_BACKLOG: AtomicUsize = AtomicUsize::new(0);
static READS: AtomicUsize = AtomicUsize::new(0);
static FRAMES: AtomicUsize = AtomicUsize::new(0);
static MAX_WAIT_US: AtomicU64 = AtomicU64::new(0);
static MAX_HOLD_US: AtomicU64 = AtomicU64::new(0);
static SUM_HOLD_US: AtomicU64 = AtomicU64::new(0);
static MAX_GAP_US: AtomicU64 = AtomicU64::new(0);
static LAST_FRAME_US: AtomicU64 = AtomicU64::new(0);
static UPDATES: AtomicUsize = AtomicUsize::new(0);
static MAX_UPDATE_US: AtomicU64 = AtomicU64::new(0);
static MAX_UPDATE_GAP_US: AtomicU64 = AtomicU64::new(0);
static LAST_UPDATE_US: AtomicU64 = AtomicU64::new(0);
static WAKEUPS: AtomicUsize = AtomicUsize::new(0);
static MAX_WAKEUP_GAP_US: AtomicU64 = AtomicU64::new(0);
static LAST_WAKEUP_US: AtomicU64 = AtomicU64::new(0);
static MAX_READ_GAP_US: AtomicU64 = AtomicU64::new(0);
static MAX_READ_US: AtomicU64 = AtomicU64::new(0);
static LAST_READ_US: AtomicU64 = AtomicU64::new(0);
static DRAINS: AtomicUsize = AtomicUsize::new(0);
static MAX_DRAIN: AtomicUsize = AtomicUsize::new(0);
static SUM_DRAIN: AtomicUsize = AtomicUsize::new(0);
static LAST_DRAIN: AtomicUsize = AtomicUsize::new(0);
static LAST_DRAIN_HIT_EMPTY: AtomicBool = AtomicBool::new(true);
static POKES: AtomicUsize = AtomicUsize::new(0);
static POKES_WITH_DATA: AtomicUsize = AtomicUsize::new(0);

/// The poller the read loop sleeps in, so the watchdog can wake it from
/// outside.
static POLLER: OnceLock<Weak<Poller>> = OnceLock::new();

/// Called from the PTY wrapper's registration, the only place holding the
/// poller the read loop waits on.
pub fn set_poller(poller: &Arc<Poller>) {
    if !enabled() {
        return;
    }
    let _ = POLLER.set(Arc::downgrade(poller));
}

/// Wake the read loop by hand, to find out whether a stalled loop is asleep on
/// a wakeup that never came or on a console that has gone quiet.
///
/// The read that follows says which: bytes mean they were sitting in the pipe
/// with nothing to announce them, and an empty drain means the console really
/// had nothing to give.
fn poke() -> bool {
    let Some(poller) = POLLER.get().and_then(Weak::upgrade) else {
        return false;
    };
    POKES.fetch_add(1, Ordering::Relaxed);
    poller.post(CompletionPacket::new(Event::readable(PTY_READ_WRITE_TOKEN))).is_ok()
}

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ALACRITREE_STALL_PROBE").is_some())
}

fn started() -> Instant {
    static T0: OnceLock<Instant> = OnceLock::new();
    *T0.get_or_init(Instant::now)
}

/// Called from the PTY reader with the length of the slice it was given.  The
/// loop hands out `&mut buf[unprocessed..]`, so the shortfall is the backlog.
pub fn read_slice(remaining: usize) {
    if !enabled() {
        return;
    }
    READS.fetch_add(1, Ordering::Relaxed);
    MAX_BACKLOG.fetch_max(READ_BUFFER_SIZE.saturating_sub(remaining), Ordering::Relaxed);

    // Between one read returning and the next being issued, the loop either
    // took the terminal and parsed, or failed to and came straight back.  A
    // long gap here is therefore a long `advance`, which is the suspected
    // shape of the freeze.
    let now_us = started().elapsed().as_micros() as u64;
    let prev_us = LAST_READ_US.swap(now_us, Ordering::Relaxed);
    if prev_us != 0 {
        MAX_READ_GAP_US.fetch_max(now_us.saturating_sub(prev_us), Ordering::Relaxed);
    }
}

/// Called with how long one drain of the console pipe took, and what it got.
///
/// `hit_empty` says the drain stopped because the pipe ran dry rather than
/// because it filled the staging buffer, which is the state that hands the
/// wakeup back to `piper`.
pub fn drain(took: Duration, bytes: usize, hit_empty: bool) {
    if !enabled() {
        return;
    }
    MAX_READ_US.fetch_max(took.as_micros() as u64, Ordering::Relaxed);
    DRAINS.fetch_add(1, Ordering::Relaxed);
    MAX_DRAIN.fetch_max(bytes, Ordering::Relaxed);
    SUM_DRAIN.fetch_add(bytes, Ordering::Relaxed);
    LAST_DRAIN.store(bytes, Ordering::Relaxed);
    LAST_DRAIN_HIT_EMPTY.store(hit_empty, Ordering::Relaxed);
}

/// Called from the paint path with how long it waited for the terminal and how
/// long it then held it.  The gap since the previous frame is measured here so
/// a painter that stops running entirely shows up as a gap rather than as
/// silence.
pub fn frame(waited: Duration, held: Duration) {
    if !enabled() {
        return;
    }
    let now_us = started().elapsed().as_micros() as u64;
    let prev_us = LAST_FRAME_US.swap(now_us, Ordering::Relaxed);
    let gap = Duration::from_micros(now_us.saturating_sub(prev_us));
    FRAMES.fetch_add(1, Ordering::Relaxed);
    MAX_GAP_US.fetch_max(gap.as_micros() as u64, Ordering::Relaxed);
    lock_sample(waited, held);
    maybe_summarise();
}

/// A terminal lock taken somewhere other than the grid capture, folded into
/// the same wait and hold figures without counting as a frame.
pub fn lock_sample(waited: Duration, held: Duration) {
    if !enabled() {
        return;
    }
    MAX_WAIT_US.fetch_max(waited.as_micros() as u64, Ordering::Relaxed);
    MAX_HOLD_US.fetch_max(held.as_micros() as u64, Ordering::Relaxed);
    SUM_HOLD_US.fetch_add(held.as_micros() as u64, Ordering::Relaxed);
}

/// Called from `EventProxy::send_event`, the only thing that wakes the egui
/// loop on PTY output.
///
/// This separates the two remaining explanations for a frameless 700 ms.  If
/// wakeups keep arriving while `update` does not run, the event loop is at
/// fault.  If they stop, the read thread is stuck inside `pty_read` and sending
/// nothing, and the missing frames are a consequence rather than the problem.
pub fn wakeup() {
    if !enabled() {
        return;
    }
    let now_us = started().elapsed().as_micros() as u64;
    let prev_us = LAST_WAKEUP_US.swap(now_us, Ordering::Relaxed);
    WAKEUPS.fetch_add(1, Ordering::Relaxed);
    MAX_WAKEUP_GAP_US.fetch_max(now_us.saturating_sub(prev_us), Ordering::Relaxed);
}

/// Called once per eframe `update`, with how long that update took.
///
/// The grid capture only happens when the terminal view actually paints, so
/// counting updates separately distinguishes an update that ran without
/// repainting the grid from an update that never ran at all.  Summaries are
/// driven from here too: if the painter stops, `frame` stops being called, and
/// a summary keyed off it alone would go silent exactly when it matters.
pub fn update_tick(took: Duration) {
    if !enabled() {
        return;
    }
    let now_us = started().elapsed().as_micros() as u64;
    let prev_us = LAST_UPDATE_US.swap(now_us, Ordering::Relaxed);
    UPDATES.fetch_add(1, Ordering::Relaxed);
    MAX_UPDATE_US.fetch_max(took.as_micros() as u64, Ordering::Relaxed);
    MAX_UPDATE_GAP_US.fetch_max(now_us.saturating_sub(prev_us), Ordering::Relaxed);
    maybe_summarise();
}

/// Watch one session's terminal from a thread of its own, so a stall is
/// reported by something neither the reader nor the painter can block.
pub fn watch(term: &Arc<FairMutex<Term<EventProxy>>>) {
    if !enabled() {
        return;
    }
    static WATCHING: OnceLock<()> = OnceLock::new();
    if WATCHING.set(()).is_err() {
        return;
    }
    let term = Arc::downgrade(term);
    std::thread::Builder::new()
        .name("stall-probe-watchdog".into())
        .spawn(move || watchdog(term))
        .ok();
}

fn watchdog(term: Weak<FairMutex<Term<EventProxy>>>) {
    // The read this stall began after.  A stall is over once some other read
    // takes its place, which is what turns a first-hundred-milliseconds
    // snapshot into a measurement of the whole gap.
    let mut stalled_after: Option<u64> = None;

    loop {
        std::thread::sleep(WATCHDOG_TICK);
        let Some(term) = term.upgrade() else { return };

        let last_read_us = LAST_READ_US.load(Ordering::Relaxed);
        if last_read_us == 0 {
            continue;
        }

        if let Some(start_us) = stalled_after {
            if last_read_us == start_us {
                continue;
            }
            let bytes = LAST_DRAIN.load(Ordering::Relaxed);
            if bytes > 0 {
                POKES_WITH_DATA.fetch_add(1, Ordering::Relaxed);
            }
            log::warn!(
                "stall_probe: stall ended after {:.1} ms | poked read got {} KiB",
                last_read_us.saturating_sub(start_us) as f64 / 1000.0,
                bytes / 1024,
            );
            stalled_after = None;
        }

        let now_us = started().elapsed().as_micros() as u64;
        let idle_us = now_us.saturating_sub(last_read_us);
        if Duration::from_micros(idle_us) < STALL_AFTER {
            continue;
        }

        // Sampled, not held: the reader's own `try_lock_unfair` is racing this
        // one, and a probe that lingered would create the contention it is
        // here to observe.
        let locked = term.try_lock_unfair().is_none();
        drop(term);
        log::warn!(
            "stall_probe: stall at {:.1} ms | terminal {} | last drain {} KiB, pipe {} | poking",
            idle_us as f64 / 1000.0,
            if locked { "LOCKED (parsing)" } else { "free (waiting on poll)" },
            LAST_DRAIN.load(Ordering::Relaxed) / 1024,
            if LAST_DRAIN_HIT_EMPTY.load(Ordering::Relaxed) { "empty" } else { "still had bytes" },
        );
        poke();
        stalled_after = Some(last_read_us);
    }
}

/// One line per second, reporting the window then clearing it, so a stall shows
/// as its own row rather than as a high-water mark that never comes back down.
fn maybe_summarise() {
    static LAST: OnceLock<std::sync::Mutex<Instant>> = OnceLock::new();
    let last = LAST.get_or_init(|| std::sync::Mutex::new(started()));
    let Ok(mut last) = last.try_lock() else { return };
    let now = Instant::now();
    if now.duration_since(*last) < SUMMARY_EVERY {
        return;
    }
    let elapsed = now.duration_since(*last);
    *last = now;

    let frames = FRAMES.swap(0, Ordering::Relaxed);
    let reads = READS.swap(0, Ordering::Relaxed);
    let backlog = MAX_BACKLOG.swap(0, Ordering::Relaxed);
    let wait = MAX_WAIT_US.swap(0, Ordering::Relaxed);
    let hold = MAX_HOLD_US.swap(0, Ordering::Relaxed);
    let hold_sum = SUM_HOLD_US.swap(0, Ordering::Relaxed);
    let gap = MAX_GAP_US.swap(0, Ordering::Relaxed);
    let updates = UPDATES.swap(0, Ordering::Relaxed);
    let update_max = MAX_UPDATE_US.swap(0, Ordering::Relaxed);
    let update_gap = MAX_UPDATE_GAP_US.swap(0, Ordering::Relaxed);
    let wakeups = WAKEUPS.swap(0, Ordering::Relaxed);
    let wakeup_gap = MAX_WAKEUP_GAP_US.swap(0, Ordering::Relaxed);
    let read_gap = MAX_READ_GAP_US.swap(0, Ordering::Relaxed);
    let drain_took = MAX_READ_US.swap(0, Ordering::Relaxed);
    let drains = DRAINS.swap(0, Ordering::Relaxed);
    let drain_max = MAX_DRAIN.swap(0, Ordering::Relaxed);
    let drain_sum = SUM_DRAIN.swap(0, Ordering::Relaxed);
    let pokes = POKES.swap(0, Ordering::Relaxed);
    let pokes_with_data = POKES_WITH_DATA.swap(0, Ordering::Relaxed);

    let secs = elapsed.as_secs_f64();
    let locked_pct = (hold_sum as f64 / 1000.0) / (secs * 1000.0) * 100.0;

    log::warn!(
        "stall_probe: {:.0} fps | updates {:.0}/s (max {:.1} ms, gap {:.1} ms) | \
         wakeups {:.0}/s (gap {:.1} ms) | reads {} (gap {:.1} ms) | \
         drains {} ({} MiB, max {} KiB, slowest {:.1} ms) | backlog max {} KiB | \
         wait max {:.1} ms | hold max {:.1} ms | lock held {:.0}% | grid gap max {:.1} ms | \
         pokes {} ({} found data)",
        frames as f64 / secs,
        updates as f64 / secs,
        update_max as f64 / 1000.0,
        update_gap as f64 / 1000.0,
        wakeups as f64 / secs,
        wakeup_gap as f64 / 1000.0,
        reads,
        read_gap as f64 / 1000.0,
        drains,
        drain_sum / (1024 * 1024),
        drain_max / 1024,
        drain_took as f64 / 1000.0,
        backlog / 1024,
        wait as f64 / 1000.0,
        hold as f64 / 1000.0,
        locked_pct,
        gap as f64 / 1000.0,
        pokes,
        pokes_with_data,
    );
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::WindowSize;
    use alacritty_terminal::term::Config as TermConfig;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    use super::*;

    /// What the grid alone costs, with no PTY, no poller and no painter.
    ///
    /// The console side of the stall measured 68 MiB per second while the read
    /// buffer stayed nearly full, which puts the parse in front of the pipe as
    /// the limit.  Feeding the reproducer's payload straight into the terminal
    /// prices that half on its own: a figure near 68 says the plumbing is
    /// already at the ceiling, and a figure far above it says the loop around
    /// the parse is where the time goes.
    ///
    /// `cargo test -p alacritree --release parse_throughput -- --ignored
    /// --nocapture`
    #[test]
    #[ignore = "a benchmark, not a check"]
    fn parse_throughput() {
        const CHUNK: usize = 1024 * 1024;
        const CHUNKS: usize = 64;

        // ManyLine scrolls the grid roughly once every twenty-seven bytes;
        // LongLine writes the same volume of cells and never scrolls.  The
        // spread between them is what the scroll costs.  Running each against
        // both scrollback sizes prices the history push separately again.
        for (name, newlines) in [("manyline", true), ("longline", false)] {
            let mut seed = 1u32;
            let payload: Vec<u8> = (0..CHUNK)
                .map(|_| {
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    match (seed >> 16) % 27 {
                        26 if newlines => b'\n',
                        pick => b'a' + (pick % 26) as u8,
                    }
                })
                .collect();

            for history in [100_000usize, 10_000, 1_000, 100, 0] {
                let (proxy, _events) = crate::session::EventProxy::new(egui::Context::default());
                let config = TermConfig { scrolling_history: history, ..TermConfig::default() };
                let mut term =
                    alacritty_terminal::term::Term::new(config, &TermSize::new(158, 41), proxy);
                let mut parser = Processor::<StdSyncHandler>::new();

                let started = Instant::now();
                for _ in 0..CHUNKS {
                    parser.advance(&mut term, &payload);
                }
                let took = started.elapsed();

                let mib = (CHUNK * CHUNKS) as f64 / (1024.0 * 1024.0);
                println!(
                    "parse_throughput: {name:>8}, history {history:>5} -> {:.0} MiB/s",
                    mib / took.as_secs_f64(),
                );
            }
        }
    }

    /// Cycles this thread has actually run for.
    ///
    /// `Instant` measures wall time, so anything else scheduled on the machine
    /// lands in the number; this does not.
    fn thread_cycles() -> u64 {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::GetCurrentThread;
            use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
            let mut cycles = 0u64;
            // SAFETY: the pseudo handle is always valid and the out parameter
            // is a live local.
            unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
            cycles
        }
        #[cfg(not(windows))]
        {
            Instant::now().elapsed().as_nanos() as u64
        }
    }

    /// Price the two halves of the parse separately.
    ///
    /// `print` never emits a newline, so its only scrolls are the wrap every
    /// 158 columns; `feed` is nothing but newlines, so it scrolls every byte
    /// over rows that hold no text.  `text` is the reproducer's own mix.  Two
    /// unknowns, a per-byte print cost and a per-scroll cost, fall out of the
    /// first two, and `text` checks the model against a real payload.
    ///
    /// `cargo test -p alacritree --release parse_breakdown -- --ignored
    /// --nocapture`
    #[test]
    #[ignore = "a benchmark, not a check"]
    fn parse_breakdown() {
        const COLS: usize = 158;
        const ROWS: usize = 41;
        const CHUNK: usize = 1024 * 1024;
        let chunks: usize =
            std::env::var("BENCH_CHUNKS").ok().and_then(|v| v.parse().ok()).unwrap_or(16);

        let print: Vec<u8> = vec![b'a'; CHUNK];
        let feed: Vec<u8> = vec![b'\n'; CHUNK];
        let text: Vec<u8> =
            (0..CHUNK).map(|i| if i % 27 == 26 { b'\n' } else { b'a' + (i % 26) as u8 }).collect();
        let wide: Vec<u8> = {
            let mut v = Vec::with_capacity(CHUNK + 8);
            while v.len() < CHUNK {
                v.extend_from_slice("は".as_bytes());
            }
            v
        };

        for history in [10_000usize, 1_000, 0] {
            let only = std::env::var("BENCH_ONLY").unwrap_or_default();
            for (name, payload) in
                [("print", &print), ("feed", &feed), ("text", &text), ("wide", &wide)]
                    .into_iter()
                    .filter(|(n, _)| only.is_empty() || only == *n)
            {
                let (proxy, _events) = crate::session::EventProxy::new(egui::Context::default());
                let config = TermConfig { scrolling_history: history, ..TermConfig::default() };
                let mut term =
                    alacritty_terminal::term::Term::new(config, &TermSize::new(COLS, ROWS), proxy);
                let mut parser = Processor::<StdSyncHandler>::new();

                // Warm the ring so the history allocations land outside the timing.
                parser.advance(&mut term, payload);

                // This machine's clock moves under load, so neither wall time
                // nor a cycle count is comparable between two processes.
                // Alternating the clears inside one run puts them all under
                // the same conditions, and the fastest of several reps sheds
                // whatever else was resident.
                //
                // `base` is upstream's parse, `bulk` adds the row clear as one
                // copy, `batch` adds writing a run of printable ASCII into a
                // row at once.  Under a profiler `BENCH_PATH` pins one path, or
                // the report blends them.
                const PATHS: [(&str, bool, bool); 3] =
                    [("base", false, false), ("bulk", true, false), ("batch", true, true)];
                let only = std::env::var("BENCH_PATH").unwrap_or_default();

                let mut best = [u64::MAX; PATHS.len()];
                for _ in 0..6 {
                    for (index, (name, bulk, batch)) in PATHS.into_iter().enumerate() {
                        if !only.is_empty() && only != name {
                            continue;
                        }
                        use std::sync::atomic::Ordering::Relaxed;
                        alacritty_terminal::grid::BULK_RESET.store(bulk, Relaxed);
                        alacritty_terminal::term::BATCH_INPUT.store(batch, Relaxed);
                        let started = thread_cycles();
                        for _ in 0..chunks {
                            parser.advance(&mut term, payload);
                        }
                        best[index] = best[index].min(thread_cycles() - started);
                    }
                }

                let bytes = (CHUNK * chunks) as f64;
                let cycles = best.map(|b| b as f64 / bytes);
                println!(
                    "parse_breakdown: history {history:>5} {name:>5} -> base {:>6.2}, bulk \
                     {:>6.2} ({:>4.2}x), batch {:>6.2} ({:>4.2}x)",
                    cycles[0],
                    cycles[1],
                    cycles[0] / cycles[1],
                    cycles[2],
                    cycles[0] / cycles[2],
                );
            }
        }
    }
}
