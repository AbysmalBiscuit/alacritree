//! Reproduce cargo's progress-line writes without cargo's CPU load.
//!
//! Modelled on cargo 0.98.0 (`src/cargo/util/progress.rs`,
//! `crates/cargo-util-terminal/src/shell.rs`), because the details that decide
//! whether a reader can catch a half-drawn line are all in how cargo writes
//! rather than in what it draws:
//!
//! * Every fragment is its own write.  Cargo writes to `io::stderr()`, which is
//!   unbuffered, so a redraw reaches the terminal as the header, then the bar,
//!   then the carriage return, as separate syscalls.
//! * A redraw ends in `\r`, parking the cursor at column 0 rather than after
//!   the text.
//! * The bar is space-padded to a fixed width instead of erased, so a steady
//!   run of ticks never blanks the line.
//! * Erasing for a status line is platform-split.  Unix emits `ESC [ K`, three
//!   bytes.  Windows has no such path: `default_err_erase_line` writes one
//!   terminal-width run of spaces followed by `\r`, so every `Compiling` line
//!   is preceded by a write that genuinely blanks the bottom row.
//! * Ticks are throttled to one per 100 ms after an initial 500 ms, and a tick
//!   whose rendered line is unchanged writes nothing at all.
//!
//! `--erase el` forces the Unix sequence on Windows.  If that alone settles the
//! flicker, the trigger is the full-width blank write, not the tick rate.
//!
//! Usage: `cargo run -p alacritree --example flicker_repro -- [OPTIONS]`
//! It depends only on `std`, so `rustc -O flicker_repro.rs` builds it anywhere.

use std::io::Write;
use std::time::{Duration, Instant};

const NAMES: &[&str] = &[
    "serde", "syn", "quote", "proc-macro2", "libc", "unicode-ident", "cfg-if", "memchr",
    "once_cell", "bitflags", "itoa", "ryu", "log", "regex-syntax", "aho-corasick", "thiserror",
    "smallvec", "parking_lot", "rayon-core", "crossbeam-epoch", "windows-sys", "getrandom",
];

/// Cargo caps the bar at 50 columns however wide the terminal is.
const MAX_PRINT: usize = 50;
/// Width cargo reserves for the right-justified status header.
const HEADER_WIDTH: usize = 15;

#[derive(Clone, Copy, PartialEq)]
enum Erase {
    /// `ESC [ K`, what cargo emits everywhere except Windows.
    El,
    /// A terminal-width run of spaces then `\r`, what cargo emits on Windows.
    Spaces,
}

struct Options {
    width: usize,
    tick_ms: u64,
    /// Packages finishing per second, each one a status line above the bar.
    scroll: f64,
    load: usize,
    secs: u64,
    erase: Erase,
    /// Emit each redraw as one write instead of cargo's several.
    buffered: bool,
    /// Ask the console for its width once per tick, the way cargo does.
    query_width: bool,
}

impl Default for Options {
    fn default() -> Self {
        let erase = if cfg!(windows) { Erase::Spaces } else { Erase::El };
        Self { width: 120, tick_ms: 100, scroll: 3.0, load: 0, secs: 30, erase, buffered: false, query_width: false }
    }
}

fn main() {
    let Some(opts) = parse_args() else {
        eprintln!(
            "usage: flicker_repro [--width N] [--tick-ms N] [--scroll N] [--load N] [--secs N] \
             [--erase el|spaces] [--buffered] [--query-width]"
        );
        std::process::exit(2);
    };

    for _ in 0..opts.load {
        std::thread::spawn(|| {
            let mut x = 0u64;
            loop {
                x = std::hint::black_box(x).wrapping_mul(6364136223846793005).wrapping_add(1);
            }
        });
    }

    let total = 412usize;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(opts.secs);

    // Cargo holds the first tick back half a second, then rate-limits the rest.
    let mut next_tick = start + Duration::from_millis(500);
    let mut needs_clear = false;
    let mut last_line: Option<String> = None;
    let mut finished = 0usize;

    while Instant::now() < deadline {
        let elapsed = start.elapsed().as_secs_f64();

        // A finished package interrupts the bar: cargo erases the line it is
        // sitting on before printing anything permanent.
        let want = (elapsed * opts.scroll) as usize;
        while finished < want {
            if needs_clear {
                erase_line(opts.erase, opts.width);
                needs_clear = false;
            }
            let name = NAMES[finished % NAMES.len()];
            let mut err = std::io::stderr();
            let _ = write!(err, "\x1b[1;32m{:>12}\x1b[0m", "Compiling");
            let _ = writeln!(err, " {name} v{}.{}.0", finished % 4, finished % 9);
            finished += 1;
        }

        if Instant::now() >= next_tick {
            next_tick = Instant::now() + Duration::from_millis(opts.tick_ms);
            // Cargo re-reads the width every tick, and on Windows that is a
            // console API call rather than a VT write.  Whether conpty can stay
            // in passthrough while the child mixes the two is the question.
            if opts.query_width {
                console_width();
            }
            let done = ((elapsed / opts.secs as f64) * total as f64) as usize;
            let line = bar(opts.width, done.min(total), total, finished);

            // An unchanged line is not rewritten, so a stalled build leaves the
            // terminal completely idle rather than rewriting the same bytes.
            if last_line.as_ref() != Some(&line) {
                let mut err = std::io::stderr();
                if opts.buffered {
                    let _ = err.write_all(
                        format!("\x1b[1;32m{:>12}\x1b[0m{line}\r", "Building").as_bytes(),
                    );
                } else {
                    let _ = write!(err, "\x1b[1;32m{:>12}\x1b[0m", "Building");
                    let _ = write!(err, "{line}\r");
                }
                last_line = Some(line);
                needs_clear = true;
            }
        }

        std::thread::sleep(Duration::from_millis(2));
    }

    if needs_clear {
        erase_line(opts.erase, opts.width);
    }
}

/// Blank the row the bar occupies.  The two arms are not cosmetic: `El` moves no
/// text, while `Spaces` rewrites the whole row and so shows up downstream as a
/// real change to every cell in it.
fn erase_line(erase: Erase, width: usize) {
    let mut err = std::io::stderr();
    match erase {
        Erase::El => {
            let _ = err.write_all(b"\x1b[K");
        },
        Erase::Spaces => {
            let _ = write!(err, "{}\r", " ".repeat(width));
        },
    }
}

/// `[===>    ] 42/412: syn, quote`, padded with trailing spaces so successive
/// bars overwrite each other without needing an erase between them.
fn bar(width: usize, cur: usize, max: usize, tick: usize) -> String {
    let stats = format!(" {cur}/{max}");
    let bar_width = width.min(MAX_PRINT).saturating_sub(stats.len() + 2 + HEADER_WIDTH);
    let filled = (bar_width as f64 * (cur as f64 / max as f64)) as usize;

    let mut line = String::with_capacity(width);
    line.push('[');
    if filled > 0 {
        line.extend(std::iter::repeat_n('=', filled - 1));
        line.push(if cur == max { '=' } else { '>' });
    }
    line.extend(std::iter::repeat_n(' ', bar_width - filled));
    line.push(']');
    line.push_str(&stats);

    line.push_str(": ");
    for i in 0..=(tick % 4) {
        if i > 0 {
            line.push_str(", ");
        }
        line.push_str(NAMES[(tick + i) % NAMES.len()]);
    }

    let padded = width.saturating_sub(HEADER_WIDTH);
    while line.len() < padded {
        line.push(' ');
    }
    line.truncate(padded);
    line
}

/// `GetConsoleScreenBufferInfo` on stderr, which is what cargo's
/// `stderr_width` reaches for before every redraw.
#[cfg(windows)]
fn console_width() -> Option<usize> {
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_ERROR_HANDLE,
    };

    unsafe {
        let handle = GetStdHandle(STD_ERROR_HANDLE);
        let mut csbi: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(handle, &mut csbi) == 0 {
            return None;
        }
        Some((csbi.srWindow.Right - csbi.srWindow.Left) as usize)
    }
}

#[cfg(not(windows))]
fn console_width() -> Option<usize> {
    None
}

fn parse_args() -> Option<Options> {
    fn value<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>) -> Option<T> {
        args.next()?.parse().ok()
    }

    let mut opts = Options::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--width" => opts.width = value(&mut args)?,
            "--tick-ms" => opts.tick_ms = value(&mut args)?,
            "--scroll" => opts.scroll = value(&mut args)?,
            "--load" => opts.load = value(&mut args)?,
            "--secs" => opts.secs = value(&mut args)?,
            "--buffered" => opts.buffered = true,
            "--query-width" => opts.query_width = true,
            "--erase" => {
                opts.erase = match args.next()?.as_str() {
                    "el" => Erase::El,
                    "spaces" => Erase::Spaces,
                    _ => return None,
                }
            },
            _ => return None,
        }
    }

    Some(opts)
}
