//! Record what a pseudoconsole sends, with the time each read arrived.
//!
//! ConPTY does not forward a child's bytes.  It renders the child's output into
//! its own buffer and emits a diff of that buffer, so a client sees conpty's
//! reconstruction rather than what the program wrote.  When a redrawing program
//! flickers, the question is whether the gap is in what conpty sent or in what
//! we did with it, and only the received stream answers it.
//!
//! Usage: `cargo run -p alacritree --example conpty_stream -- <log> <program> [args…]`

#[cfg(windows)]
use std::io::{Read, Write};
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use alacritty_terminal::tty::{self, EventedReadWrite, Options as PtyOptions, Shell};

#[cfg(not(windows))]
fn main() {
    eprintln!("conpty_stream records what a pseudoconsole re-encodes; elsewhere the pty");
    eprintln!("forwards the child's bytes unchanged and there is nothing to compare against.");
}

#[cfg(windows)]
fn main() {
    let mut args = std::env::args().skip(1);
    let log = args.next().expect("a log path");
    let program = args.next().expect("a program to run");
    let rest: Vec<String> = args.collect();

    harden_dll_search_path();

    let options = PtyOptions {
        shell: Some(Shell::new(program, rest)),
        working_directory: None,
        drain_on_exit: true,
        env: std::collections::HashMap::new(),
        escape_args: false,
    };
    let size = alacritty_terminal::event::WindowSize {
        num_lines: 40,
        num_cols: 140,
        cell_width: 8,
        cell_height: 16,
    };
    let mut pty = tty::new(&options, size, 0).expect("open a pseudoconsole");

    let mut out = std::io::BufWriter::new(std::fs::File::create(&log).expect("create the log"));
    let start = Instant::now();
    let mut buf = [0u8; 65536];
    let mut idle = 0u32;

    // A build runs far longer than the behaviour under study needs; the cap
    // ends the recording while the child keeps going, and the child dies with
    // the console when the probe drops it.
    let limit = std::env::var("CONPTY_STREAM_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(Duration::from_secs(45), Duration::from_secs);

    loop {
        if start.elapsed() > limit {
            break;
        }
        match pty.reader().read(&mut buf) {
            // Windows reports "nothing readable yet" as a zero-length read
            // rather than as `WouldBlock`, so this is idleness, not EOF.
            Ok(0) => {
                idle += 1;
                if idle > 10_000 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            },
            Ok(got) => {
                idle = 0;
                writeln!(out, "{}\t{}\t{}", start.elapsed().as_micros(), got, escape(&buf[..got]))
                    .expect("write a record");
            },
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                idle += 1;
                // The child is done when nothing has arrived for a while and
                // its watcher has fired; 10 s of silence ends the run either
                // way so a hung build cannot wedge the probe.
                if idle > 10_000 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            },
            Err(_) => break,
        }
    }
    out.flush().expect("flush the log");
    eprintln!("wrote {log}");
}

/// Hex-encode a chunk so one read is one line.  Control bytes are the whole
/// point of the recording, so the format has to carry them without a quoting
/// rule that could itself be misread.
#[cfg(windows)]
fn escape(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The same DLL search hardening `main` does: without it a foreign
/// `conpty.dll` on PATH hosts the console and the probe measures that
/// implementation instead of the one the app uses.
#[cfg(windows)]
fn harden_dll_search_path() {
    use windows_sys::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, SetDefaultDllDirectories,
    };
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) };
}
