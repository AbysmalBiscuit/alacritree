//! Sample the terminal grid while a program runs, to separate "the grid lost
//! the line" from "the painter did".
//!
//! A recording of the window answers neither question on its own: a row missing
//! from a frame may have been missing from the grid the painter read, or may
//! have been in it and dropped on the way to the screen.  Driving the real
//! `tty` and `EventLoop` path and reading `Term` under the same lock the
//! painter takes puts a number on the first half, leaving whatever gap remains
//! to the second.
//!
//! Sampling in process is the point.  Asking a running instance through
//! `session read-screen` spawns a process per sample, which bounds the rate to
//! tens of samples over a whole build and adds load to the thing being
//! measured.  Here a sample is a lock and a scan.
//!
//! On Windows the pty is wrapped the way `Session::spawn_with` wraps it.  That
//! reader defers parsing whenever it cannot reach the terminal lock, so a probe
//! without the wrapper would measure a different reader from the one that ships.
//!
//! Usage: `cargo run -p alacritree --example grid_probe -- [OPTIONS] <program> [args…]`

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};

#[cfg(windows)]
#[path = "../src/pty_rearm.rs"]
mod pty_rearm;

/// One observation of the grid.
struct Sample {
    at: Duration,
    /// Whether any row on screen held the text being watched for.
    present: bool,
    cursor_column: usize,
}

struct Options {
    log: Option<String>,
    /// Text that marks the row being watched, e.g. cargo's `Building`.
    needle: String,
    interval: Duration,
    columns: usize,
    screen_lines: usize,
    max: Duration,
    program: String,
    args: Vec<String>,
}

#[derive(Copy, Clone)]
struct Size {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// The probe has no window to repaint, so an event's only use here is knowing
/// when the child is gone.
#[derive(Clone)]
struct Proxy {
    events: Arc<AtomicUsize>,
    exited: Arc<AtomicBool>,
}

impl EventListener for Proxy {
    fn send_event(&self, event: TermEvent) {
        self.events.fetch_add(1, Ordering::Relaxed);
        if matches!(event, TermEvent::ChildExit(_)) {
            self.exited.store(true, Ordering::Relaxed);
        }
    }
}

fn main() -> std::io::Result<()> {
    let Some(opts) = parse_args() else {
        eprintln!(
            "usage: grid_probe [--log PATH] [--match TEXT] [--interval-us N] [--cols N] [--rows \
             N] [--max-secs N] <program> [args…]"
        );
        std::process::exit(2);
    };

    let size = Size { columns: opts.columns, screen_lines: opts.screen_lines };
    let window_size = WindowSize {
        num_cols: opts.columns as u16,
        num_lines: opts.screen_lines as u16,
        cell_width: 8,
        cell_height: 16,
    };

    let proxy =
        Proxy { events: Arc::new(AtomicUsize::new(0)), exited: Arc::new(AtomicBool::new(false)) };
    let term = Arc::new(FairMutex::new(Term::new(TermConfig::default(), &size, proxy.clone())));

    let pty_options = PtyOptions {
        shell: Some(Shell::new(opts.program.clone(), opts.args.clone())),
        working_directory: None,
        drain_on_exit: false,
        env: Default::default(),
        // The argv here is built in code rather than typed by a user, so an
        // argument holding a space has to survive as one argument.
        #[cfg(windows)]
        escape_args: true,
    };

    let pty = tty::new(&pty_options, window_size, 0)?;
    #[cfg(windows)]
    let pty = pty_rearm::RearmingPty::new(pty);

    let event_loop = EventLoop::new(term.clone(), proxy.clone(), pty, false, false)?;
    let channel = event_loop.channel();
    let loop_handle = event_loop.spawn();

    let start = Instant::now();
    let mut samples: Vec<Sample> = Vec::new();
    while !proxy.exited.load(Ordering::Relaxed) && start.elapsed() < opts.max {
        let at = start.elapsed();
        let (present, cursor_column) = {
            let term = term.lock();
            let grid = term.grid();
            let mut present = false;
            for line in 0..grid.screen_lines() {
                let row = &grid[Line(line as i32)];
                let text: String =
                    (0..grid.columns()).map(|column| row[Column(column)].c).collect();
                if text.contains(&opts.needle) {
                    present = true;
                    break;
                }
            }
            (present, grid.cursor.point.column.0)
        };
        samples.push(Sample { at, present, cursor_column });
        std::thread::sleep(opts.interval);
    }

    let _ = channel.send(Msg::Shutdown);
    let _ = loop_handle.join();

    if let Some(path) = &opts.log {
        let mut out = String::with_capacity(samples.len() * 24);
        for sample in &samples {
            out.push_str(&format!(
                "{:.6}\t{}\t{}\n",
                sample.at.as_secs_f64(),
                u8::from(sample.present),
                sample.cursor_column
            ));
        }
        std::fs::write(path, out)?;
    }

    report(&opts, &samples, proxy.events.load(Ordering::Relaxed));
    Ok(())
}

/// One frame at 60 Hz.  A gap shorter than this cannot have had a frame of its
/// own, so it is not something a viewer could have seen.
const FRAME: Duration = Duration::from_micros(16_667);

/// The summary is what the probe is for: whoever runs it on another platform
/// should get the number without a second tool to post-process the log.
fn report(opts: &Options, samples: &[Sample], events: usize) {
    let Some(last) = samples.last() else {
        eprintln!("no samples");
        return;
    };
    let Some(first_match) = samples.iter().find(|s| s.present) else {
        eprintln!(
            "{} samples over {:.2} s, {events} terminal events, never saw {:?}",
            samples.len(),
            last.at.as_secs_f64(),
            opts.needle
        );
        return;
    };

    // Everything before the first match is the program starting up rather than
    // a gap in what it was drawing, and counting it would swamp the figure the
    // probe exists to produce.
    let observed = last.at.saturating_sub(first_match.at).as_secs_f64();
    let mut absent = 0.0;
    let mut run = 0.0;
    let mut longest_absent: f64 = 0.0;
    let mut gaps_over_a_frame = 0;
    let mut off_column = 0.0;
    let mut longest_off: f64 = 0.0;
    let mut off_run = 0.0;

    for (sample, next) in samples.iter().zip(samples.iter().skip(1)) {
        if sample.at < first_match.at {
            continue;
        }
        let dt = next.at.saturating_sub(sample.at).as_secs_f64();
        if sample.present {
            if run > FRAME.as_secs_f64() {
                gaps_over_a_frame += 1;
            }
            run = 0.0;
        } else {
            absent += dt;
            run += dt;
            longest_absent = longest_absent.max(run);
        }
        // A completed redraw ends in a carriage return, so a cursor anywhere
        // but column 0 is a write the reader caught half applied.
        if sample.cursor_column == 0 {
            off_run = 0.0;
        } else {
            off_column += dt;
            off_run += dt;
            longest_off = longest_off.max(off_run);
        }
    }

    let percent = |seconds: f64| if observed > 0.0 { 100.0 * seconds / observed } else { 0.0 };
    eprintln!(
        "{} samples over {:.2} s, {events} terminal events, first match at {:.2} s",
        samples.len(),
        last.at.as_secs_f64(),
        first_match.at.as_secs_f64()
    );
    eprintln!(
        "{:?} present {:.2}% of the {observed:.1} s after that, longest gap {:.1} ms, \
         {gaps_over_a_frame} gaps over one frame",
        opts.needle,
        100.0 - percent(absent),
        longest_absent * 1000.0
    );
    eprintln!(
        "cursor off column 0 {:.3}% of the time, longest {:.1} ms",
        percent(off_column),
        longest_off * 1000.0
    );
}

fn parse_args() -> Option<Options> {
    fn value<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>) -> Option<T> {
        args.next()?.parse().ok()
    }

    let mut log = None;
    let mut needle = String::from("Building");
    let mut interval_us = 500u64;
    let mut columns = 117usize;
    let mut screen_lines = 30usize;
    let mut max_secs = 600u64;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => log = Some(args.next()?),
            "--match" => needle = args.next()?,
            "--interval-us" => interval_us = value(&mut args)?,
            "--cols" => columns = value(&mut args)?,
            "--rows" => screen_lines = value(&mut args)?,
            "--max-secs" => max_secs = value(&mut args)?,
            // Everything from the first non-option on is the command to run,
            // options of its own included.
            _ => {
                return Some(Options {
                    log,
                    needle,
                    interval: Duration::from_micros(interval_us),
                    columns,
                    screen_lines,
                    max: Duration::from_secs(max_secs),
                    program: arg,
                    args: args.collect(),
                });
            },
        }
    }

    None
}
