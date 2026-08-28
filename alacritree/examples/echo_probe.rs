//! Headless echo and spawn latency probe.
//!
//! Throwaway instrumentation for the load-latency diagnosis.  It drives the
//! same PTY stack the app does — `alacritty_terminal::tty`, `EventLoop`, and
//! on Windows the rearming wrapper — with no egui, no sidebars, and no git
//! status, so what it measures is the child round trip alone.  Whatever the
//! app adds on top is the difference between this and `frame_log`'s `echo`.
//!
//! Several shells run at once and take keystrokes in turn, because absolute
//! timings on this machine swing far enough between runs that only arms
//! measured inside one process can be compared.
//!
//! ```text
//! echo_probe --shell nu.exe --shell cmd.exe --load 16 --keys 40 --spawns 5
//! ```

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};

#[cfg(windows)]
#[path = "../src/pty_rearm.rs"]
mod pty_rearm;

/// Output stops arriving for this long once the child has finished answering.
///
/// Raise it for a loaded run: a starved child can pause mid-answer for longer
/// than an idle one takes to finish, and a window too narrow then closes the
/// settle early and bills the leftovers to the next keystroke.
static QUIET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(150);

fn quiet() -> Duration {
    Duration::from_millis(QUIET.load(std::sync::atomic::Ordering::Relaxed))
}

/// A shell's own startup pauses for longer than a keystroke's answer does —
/// nushell's config shells out to `mise` — so declaring the prompt ready needs
/// a wider window than declaring a keystroke answered.
const STARTUP_QUIET: Duration = Duration::from_millis(600);

/// Keystrokes discarded before the sample opens.  The first ones after a
/// spawn race whatever the shell is still doing to its prompt.
const WARMUP_KEYS: usize = 3;

/// A round trip longer than this is recorded as a timeout rather than waited
/// on, so one wedged arm cannot stall the sweep.
const PATIENCE: Duration = Duration::from_secs(20);

const COLS: u16 = 120;
const LINES: u16 = 40;

struct Size;

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        LINES as usize
    }

    fn screen_lines(&self) -> usize {
        LINES as usize
    }

    fn columns(&self) -> usize {
        COLS as usize
    }
}

/// Timestamps every event the PTY thread posts.  Which event it was does not
/// matter: any of them means bytes came back from the child.
#[derive(Clone)]
struct Tap(Sender<Instant>);

impl EventListener for Tap {
    fn send_event(&self, event: TermEvent) {
        if std::env::var_os("ECHO_PROBE_TRACE").is_some() {
            eprintln!("[DEBUG-7c1a] event {event:?}");
        }
        let _ = self.0.send(Instant::now());
    }
}

/// How long a spawn took to say anything, and how long until it stopped.
struct Spawned {
    first: Duration,
    ready: Duration,
}

struct Arm {
    label: String,
    program: String,
    args: Vec<String>,
    /// A line run once at the settled prompt, before the sample opens.
    setup: Option<String>,
    firsts: Vec<Duration>,
    readys: Vec<Duration>,
    echoes: Vec<Duration>,
    timeouts: usize,
}

/// One live shell: the PTY thread, the channel its events land in, and the
/// handle keystrokes go out through.
struct Live {
    events: Receiver<Instant>,
    notifier: Notifier,
    sender: alacritty_terminal::event_loop::EventLoopSender,
    _term: Arc<FairMutex<Term<Tap>>>,
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

fn spawn(program: &str, args: &[String]) -> std::io::Result<(Live, Spawned)> {
    let (tx, events) = mpsc::channel();
    let tap = Tap(tx);

    let term = Term::new(TermConfig::default(), &Size, tap.clone());
    let term = Arc::new(FairMutex::new(term));

    let mut env = HashMap::new();
    env.insert("TERM".into(), "xterm-256color".into());

    let options = PtyOptions {
        shell: Some(Shell::new(program.to_string(), args.to_vec())),
        working_directory: None,
        drain_on_exit: false,
        env,
        #[cfg(windows)]
        escape_args: false,
    };

    let started = Instant::now();
    let pty = tty::new(
        &options,
        WindowSize { num_lines: LINES, num_cols: COLS, cell_width: 8, cell_height: 16 },
        0,
    )?;

    #[cfg(windows)]
    let pty = pty_rearm::RearmingPty::new(pty);

    let event_loop = EventLoop::new(term.clone(), tap, pty, false, false)?;
    let sender = event_loop.channel();
    event_loop.spawn();

    let live = Live { events, notifier: Notifier(sender.clone()), sender, _term: term };

    // What the user calls "the shell started" is the prompt being there, not
    // the first byte: nushell emits a title and a partial line well before
    // starship has drawn anything to type at.
    let first = live.events.recv_timeout(PATIENCE).map(|at| at - started).unwrap_or(PATIENCE);
    let mut last = started + first;
    while let Ok(at) = live.events.recv_timeout(STARTUP_QUIET) {
        last = at;
    }
    Ok((live, Spawned { first, ready: last - started }))
}

/// Drain until the child stops producing, so the next keystroke is measured
/// against a settled prompt rather than against the tail of the last one.
fn settle(live: &Live) {
    while live.events.recv_timeout(quiet()).is_ok() {}
}

/// The non-blank lines of the grid, so a run can be checked against a real
/// prompt rather than against whatever the shell happened to emit first.
fn screen(live: &Live) -> String {
    let term = live._term.lock();
    let grid = term.grid();
    let mut out = format!(
        "[cursor {:?} offset {} {}x{}]\n",
        grid.cursor.point,
        grid.display_offset(),
        grid.columns(),
        grid.screen_lines()
    );
    for line in 0..grid.screen_lines() {
        let row: String = (0..grid.columns())
            .map(|col| {
                grid[alacritty_terminal::index::Line(line as i32)]
                    [alacritty_terminal::index::Column(col)]
                .c
            })
            .collect();
        let row = row.trim_end();
        if !row.is_empty() {
            out.push_str(row);
            out.push('\n');
        }
    }
    out
}

/// Time from a keystroke reaching the PTY to the first byte coming back.
fn echo(live: &Live) -> Option<Duration> {
    let at = Instant::now();
    live.notifier.notify(b"a".to_vec());
    match live.events.recv_timeout(PATIENCE) {
        Ok(back) => Some(back - at),
        Err(RecvTimeoutError::Timeout) => None,
        Err(RecvTimeoutError::Disconnected) => None,
    }
}

/// A round trip with no child process in it.
///
/// One thread answers on a channel the way a PTY thread does, so this pays
/// the same two context switches and none of the shell.  Under load it is the
/// control that decides whether the probe is measuring starved children or
/// its own starved threads: if this degrades as much as the shells do, the
/// shell numbers say nothing about shells.
struct Control {
    ask: Sender<Instant>,
    answer: Receiver<Instant>,
}

impl Control {
    fn new() -> Self {
        let (ask, asked) = mpsc::channel::<Instant>();
        let (answered, answer) = mpsc::channel();
        std::thread::spawn(move || {
            while asked.recv().is_ok() {
                if answered.send(Instant::now()).is_err() {
                    return;
                }
            }
        });
        Self { ask, answer }
    }

    fn round_trip(&self) -> Option<Duration> {
        let at = Instant::now();
        self.ask.send(at).ok()?;
        self.answer.recv_timeout(PATIENCE).ok().map(|back| back - at)
    }
}

/// Busy loop in a child process, so the load sits outside the process under
/// test the way a build inside a terminal does.
fn burn() -> ! {
    // The parent holds the write end of this pipe.  Killing the parent hard
    // skips every destructor it owns, including the one that reaps these, so
    // a burner that does not notice on its own outlives the run and quietly
    // eats a core until someone spots it in the task list.
    std::thread::spawn(|| {
        let mut byte = [0u8; 1];
        let _ = std::io::stdin().read(&mut byte);
        std::process::exit(0);
    });

    let mut state: u64 = 1;
    loop {
        for _ in 0..4096 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(state);
    }
}

struct Burners(Vec<Child>);

impl Drop for Burners {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn burners(count: usize) -> Burners {
    let exe = std::env::current_exe().expect("current exe");
    let mut children = Vec::new();
    for _ in 0..count {
        match Command::new(&exe).arg("--burn").stdin(std::process::Stdio::piped()).spawn() {
            Ok(child) => children.push(child),
            Err(err) => eprintln!("burner failed to start: {err}"),
        }
    }
    Burners(children)
}

fn quantile(sorted: &[Duration], q: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    sorted[((sorted.len() as f64 * q) as usize).min(sorted.len() - 1)]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn report(arms: &[Arm], load: usize) {
    println!("\nload={load} burners");
    println!(
        "{:<34} {:>4} {:>9} {:>9} {:>9} {:>4} {:>10} {:>10} {:>10} {:>10}",
        "arm",
        "n",
        "echo p50",
        "p95",
        "max",
        "n",
        "1st byte",
        "ready p50",
        "ready p95",
        "ready max"
    );
    for arm in arms {
        let mut echoes = arm.echoes.clone();
        let mut firsts = arm.firsts.clone();
        let mut readys = arm.readys.clone();
        echoes.sort_unstable();
        firsts.sort_unstable();
        readys.sort_unstable();
        println!(
            "{:<34} {:>4} {:>8.1}ms {:>8.1}ms {:>8.1}ms {:>4} {:>9.1}ms {:>9.1}ms {:>9.1}ms \
             {:>9.1}ms{}",
            arm.label,
            echoes.len(),
            ms(quantile(&echoes, 0.50)),
            ms(quantile(&echoes, 0.95)),
            ms(echoes.last().copied().unwrap_or_default()),
            readys.len(),
            ms(quantile(&firsts, 0.50)),
            ms(quantile(&readys, 0.50)),
            ms(quantile(&readys, 0.95)),
            ms(readys.last().copied().unwrap_or_default()),
            if arm.timeouts > 0 { format!("  ({} timeouts)", arm.timeouts) } else { String::new() },
        );
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--burn") {
        burn();
    }

    let mut shells: Vec<(String, Option<String>)> = Vec::new();
    let mut load = 0usize;
    let mut keys = 40usize;
    let mut spawn_rounds = 3usize;
    let mut dump = false;
    let mut hold: Option<u64> = None;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--shell" => shells.push((value(), None)),
            // Changing one setting in a shell that is already running beats
            // running two configs: the arms then differ in that setting and
            // in nothing else, not even startup.
            "--setup" => {
                let line = value();
                match shells.last_mut() {
                    Some(shell) => shell.1 = Some(line),
                    None => eprintln!("--setup before any --shell"),
                }
            },
            "--load" => load = value().parse().unwrap_or(0),
            "--keys" => keys = value().parse().unwrap_or(40),
            "--spawns" => spawn_rounds = value().parse().unwrap_or(3),
            "--dump" => dump = true,
            // Load for someone else's measurement: hold the burners for this
            // many seconds and measure nothing.  The burners have to be this
            // process's children, or the pipe they watch is one nobody holds
            // and they exit the moment they start.
            "--hold" => hold = value().parse().ok(),
            "--quiet" => {
                let ms = value().parse().unwrap_or(150);
                QUIET.store(ms, std::sync::atomic::Ordering::Relaxed);
            },
            other => eprintln!("ignoring {other}"),
        }
    }
    if shells.is_empty() {
        shells.push(("cmd.exe".into(), None));
    }

    let mut arms: Vec<Arm> = shells
        .iter()
        .map(|(spec, setup)| {
            let mut parts = spec.split_whitespace().map(str::to_string);
            let program = parts.next().unwrap_or_default();
            let args: Vec<String> = parts.collect();
            let label = match setup {
                Some(line) => format!("{spec} | {line}"),
                None => spec.clone(),
            };
            Arm {
                label,
                program,
                args,
                setup: setup.clone(),
                firsts: Vec::new(),
                readys: Vec::new(),
                echoes: Vec::new(),
                timeouts: 0,
            }
        })
        .collect();

    let _burners = burners(load);
    if let Some(seconds) = hold {
        eprintln!("holding {load} burners for {seconds}s");
        std::thread::sleep(Duration::from_secs(seconds));
        return;
    }
    if load > 0 {
        eprintln!("warming {load} burners");
        std::thread::sleep(Duration::from_secs(2));
    }

    // Spawn cost is measured by opening and dropping a shell repeatedly; the
    // echo pass then keeps one of each open so the arms interleave.
    for round in 0..spawn_rounds {
        for arm in &mut arms {
            match spawn(&arm.program, &arm.args) {
                Ok((_live, spawned)) => {
                    arm.firsts.push(spawned.first);
                    arm.readys.push(spawned.ready);
                },
                Err(err) => eprintln!("{}: spawn failed: {err}", arm.label),
            }
        }
        eprint!("\rspawn round {}/{spawn_rounds}", round + 1);
        let _ = std::io::stderr().flush();
    }
    eprintln!();

    let mut live: Vec<Option<Live>> = Vec::new();
    for arm in &arms {
        match spawn(&arm.program, &arm.args) {
            Ok((session, _)) => {
                settle(&session);
                if let Some(line) = &arm.setup {
                    session.notifier.notify(format!("{line}\r").into_bytes());
                    settle(&session);
                }
                if dump {
                    eprintln!("--- {} grid ---\n{}", arm.label, screen(&session));
                }
                live.push(Some(session));
            },
            Err(err) => {
                eprintln!("{}: spawn failed: {err}", arm.label);
                live.push(None);
            },
        }
    }

    let control = Control::new();
    let mut control_arm = Arm {
        label: "(no child, threads only)".into(),
        program: String::new(),
        args: Vec::new(),
        setup: None,
        firsts: Vec::new(),
        readys: Vec::new(),
        echoes: Vec::new(),
        timeouts: 0,
    };

    for key in 0..keys + WARMUP_KEYS {
        // Sampled alongside the shells rather than before them, so it sees the
        // same load at the same moment.
        let control_took = control.round_trip();
        if key >= WARMUP_KEYS {
            match control_took {
                Some(took) => control_arm.echoes.push(took),
                None => control_arm.timeouts += 1,
            }
        }
        for (arm, session) in arms.iter_mut().zip(&live) {
            let Some(session) = session else { continue };
            let took = echo(session);
            settle(session);
            if key < WARMUP_KEYS {
                continue;
            }
            match took {
                Some(took) => arm.echoes.push(took),
                None => arm.timeouts += 1,
            }
        }
        eprint!("\rkey {}/{keys}", key + 1);
        let _ = std::io::stderr().flush();
    }
    eprintln!();

    if dump {
        for (arm, session) in arms.iter().zip(&live) {
            let Some(session) = session else { continue };
            eprintln!("--- {} after keys ---\n{}", arm.label, screen(session));
        }
    }

    arms.insert(0, control_arm);
    report(&arms, load);
}
