//! Where does a WSL session's time to prompt go?
//!
//! Throwaway instrumentation for the load-latency diagnosis.  A new WSL shell
//! takes tens of seconds on a saturated machine while the distro itself
//! measures healthy — its CPU loop and its fork cost barely move under 64 host
//! burners — so the cost is somewhere between the two, and "somewhere" is not
//! something a fix can be aimed at.
//!
//! The launch is split into three phases that can be attributed separately:
//!
//!   host    `tty::new` returning to the first byte coming back.  Creating
//!           `wsl.exe`, its console plumbing, and entering the VM.
//!   vm      Between two timestamps the shim prints, both taken inside the
//!           distro, so no clock has to be compared with the host's.
//!   shell   What is left of the wall time: the login shell and everything its
//!           profile sources.
//!
//! Whether the host half or the VM half carries the seconds decides whether
//! anything on the Windows side can be aimed at it at all.
//!
//! ```text
//! cargo run -p alacritree --release --example wsl_phases -- --distro kali-linux --load 64
//! ```

use std::collections::HashMap;
use std::io::Read as _;
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use alacritty_terminal::event::Notify as _;
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};

#[cfg(windows)]
#[path = "../src/pty_rearm.rs"]
mod pty_rearm;

/// A shell's startup pauses for longer than any keystroke's answer, so
/// declaring the prompt ready needs a wide window.  Wider than `echo_probe`'s:
/// a starved WSL launch stalls for seconds mid-profile.
const STARTUP_QUIET: Duration = Duration::from_millis(1500);

/// A launch slower than this is recorded as a timeout rather than waited on.
const PATIENCE: Duration = Duration::from_secs(120);

const COLS: u16 = 200;
const LINES: u16 = 60;

/// Where the instrumented shim is installed inside the distro.
///
/// Installed as a file rather than passed as an argument.  The real
/// `wsl_helper::SHIM_SCRIPT` reaches the child through a ConPTY command line
/// because it carries no nested quotes; adding timestamps needs them, and a
/// script passed inline then arrives mangled and the child dies before writing
/// a byte.
const SHIM_PATH: &str = "/tmp/alacritree-phase-shim.sh";

/// Mirrors `wsl_helper::SHIM_SCRIPT` — the pidfile write, the `getent` lookup
/// for the login shell, and the `exec` into it — with a timestamp printed at
/// each boundary.
///
/// The profile is timed by running one throwaway interactive login shell
/// (`-lic`) and letting it exit, rather than by waiting for the real one to
/// draw a prompt.  A shell that sources a slow profile is *silent* while it
/// does so, so any "output stopped" heuristic calls the launch finished
/// seconds before there is anything to type at — which is exactly the trap
/// `echo_probe`'s ready figure falls into.
const INSTRUMENTED: &str = r#"printf 'PHASE1 %s\n' "$(date +%s%N)"
d=${XDG_RUNTIME_DIR:-/tmp}/alacritree
mkdir -p "$d" 2>/dev/null && printf %s $$ > "$d/session-probe.pid"
s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)
[ -x "$s" ] || s=/bin/sh
printf 'PHASE2 %s\n' "$(date +%s%N)"
"$s" -lic 'exit 0' >/dev/null 2>&1
printf 'PHASE3 %s\n' "$(date +%s%N)"
printf 'SHELL_IS %s\n' "$s"
exec "$s" -l
"#;

/// Put the shim inside the distro, once, before any launch is timed.
fn install_shim(distro: &str) {
    let mut child = Command::new("wsl.exe")
        .args(["-d", distro, "--exec", "sh", "-c", &format!("cat > {SHIM_PATH}")])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("install the shim");
    use std::io::Write as _;
    child.stdin.take().expect("shim stdin").write_all(INSTRUMENTED.as_bytes()).expect("write shim");
    let status = child.wait().expect("shim install");
    assert!(status.success(), "could not install the shim in {distro}");
}

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

/// Timestamps every event the PTY thread posts, and carries the payload of the
/// ones the child is waiting on an answer to.
///
/// A login shell asks the terminal what it supports before it draws anything,
/// and `PtyWrite` is that answer on its way back out.  A probe that drops it
/// leaves the shell blocked on a read that never completes, which reads as a
/// launch that finished in 20 ms with an empty screen.
#[derive(Clone)]
struct Tap(mpsc::Sender<(Instant, Option<Vec<u8>>)>);

impl EventListener for Tap {
    fn send_event(&self, event: TermEvent) {
        let reply = match &event {
            TermEvent::PtyWrite(text) => Some(text.clone().into_bytes()),
            _ => None,
        };
        let _ = self.0.send((Instant::now(), reply));
    }
}

struct Live {
    term: Arc<FairMutex<Term<Tap>>>,
    sender: alacritty_terminal::event_loop::EventLoopSender,
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

struct Launch {
    /// `tty::new` returning to the first byte back.
    first: Duration,
    /// To the last byte before the child went quiet at its prompt.
    ready: Duration,
    /// Between the shim's two timestamps, measured inside the distro.
    vm: Option<Phases>,
    grid: String,
}

fn launch(program: &str, args: &[String]) -> std::io::Result<Launch> {
    let (tx, events): (_, Receiver<(Instant, Option<Vec<u8>>)>) = mpsc::channel();
    let tap = Tap(tx);
    let term = Arc::new(FairMutex::new(Term::new(TermConfig::default(), &Size, tap.clone())));

    let mut env = HashMap::new();
    env.insert("TERM".to_string(), "xterm-256color".to_string());

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
    let notifier = Notifier(sender.clone());
    let live = Live { term, sender };

    let answer = |reply: Option<Vec<u8>>| {
        if let Some(bytes) = reply {
            notifier.notify(bytes);
        }
    };

    let first = match events.recv_timeout(PATIENCE) {
        Ok((at, reply)) => {
            answer(reply);
            at - started
        },
        Err(_) => PATIENCE,
    };

    // Wait for the last marker to be on screen rather than for the child to
    // fall quiet: a shell sourcing a slow profile is silent for the whole time
    // it takes, and silence is what a quiescence check reads as "finished".
    let deadline = Instant::now() + PATIENCE;
    let mut ready = PATIENCE;
    while Instant::now() < deadline {
        if let Ok((at, reply)) = events.recv_timeout(Duration::from_millis(200)) {
            answer(reply);
            if screen(&live).contains("PHASE3") {
                ready = at - started;
                break;
            }
        }
    }

    Ok(Launch { first, ready, vm: phases(&live), grid: screen(&live) })
}

/// The shim's own bookkeeping and the login shell's profile, split.
///
/// Every timestamp came from one clock inside the distro, so neither figure
/// has to agree with the host's.
struct Phases {
    shim: Duration,
    profile: Duration,
    login_shell: String,
}

fn phases(live: &Live) -> Option<Phases> {
    let text = screen(live);
    let field = |tag: &str| -> Option<String> {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(tag))
            .map(|rest| rest.trim().split_whitespace().next().unwrap_or_default().to_string())
    };
    let stamp = |tag: &str| -> Option<u128> { field(tag)?.parse().ok() };
    let (one, two, three) = (stamp("PHASE1")?, stamp("PHASE2")?, stamp("PHASE3")?);
    Some(Phases {
        shim: Duration::from_nanos(two.checked_sub(one)? as u64),
        profile: Duration::from_nanos(three.checked_sub(two)? as u64),
        login_shell: field("SHELL_IS").unwrap_or_else(|| "unknown".to_string()),
    })
}

fn screen(live: &Live) -> String {
    let term = live.term.lock();
    let grid = term.grid();
    (0..grid.screen_lines())
        .map(|line| {
            let row: String = (0..grid.columns())
                .map(|col| {
                    grid[alacritty_terminal::index::Line(line as i32)]
                        [alacritty_terminal::index::Column(col)]
                    .c
                })
                .collect();
            format!("{}\n", row.trim_end())
        })
        .collect()
}

/// Busy loop in a child process, so the load sits outside everything measured.
fn burn() -> ! {
    // The parent holds the write end of this pipe.  Killing the parent hard
    // skips the destructor that reaps these, so a burner that does not notice
    // on its own outlives the run and quietly eats a core.
    std::thread::spawn(|| {
        let mut byte = [0u8; 1];
        let _ = std::io::stdin().read(&mut byte);
        std::process::exit(0);
    });
    let mut state: u64 = 1;
    loop {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
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
    Burners(
        (0..count)
            .filter_map(|_| {
                Command::new(&exe)
                    .arg("--burn")
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| eprintln!("burner failed to start: {e}"))
                    .ok()
            })
            .collect(),
    )
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--burn") {
        burn();
    }

    let mut distro = "kali-linux".to_string();
    let mut load = 0usize;
    let mut runs = 3usize;
    let mut windows_arm = false;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--distro" => distro = value(),
            "--load" => load = value().parse().unwrap_or(0),
            "--runs" => runs = value().parse().unwrap_or(3),
            // The same three phases for a Windows shell, where there is no VM
            // and the middle one is empty: the control that says whether a
            // slow host half is about WSL at all.
            "--windows" => windows_arm = true,
            other => eprintln!("unknown flag {other}"),
        }
    }

    let _burners = burners(load);
    if load > 0 {
        eprintln!("{load} burners running; warming up");
        std::thread::sleep(Duration::from_secs(5));
    }

    install_shim(&distro);
    let wsl_args: Vec<String> =
        ["-d", &distro, "--exec", "sh", SHIM_PATH].iter().map(|s| s.to_string()).collect();

    let mut arms: Vec<(&str, String, Vec<String>)> = vec![("wsl", "wsl.exe".to_string(), wsl_args)];
    if windows_arm {
        arms.push(("nu", "nu.exe".to_string(), Vec::new()));
    }

    println!("load={load} burners, {runs} run(s) per arm\n");
    for (label, program, args) in &arms {
        for run in 1..=runs {
            match launch(program, args) {
                Ok(l) => {
                    if std::env::var_os("DUMP").is_some() {
                        eprintln!(
                            "--- grid ---
{}",
                            l.grid
                        );
                    }
                    match l.vm {
                        Some(p) => println!(
                            "{label} #{run}  host_launch={:.0}ms  shim={:.0}ms  
                             profile={:.0}ms  to_prompt={:.0}ms  ({})",
                            ms(l.first),
                            ms(p.shim),
                            ms(p.profile),
                            ms(l.ready),
                            p.login_shell,
                        ),
                        None => println!(
                            "{label} #{run}  host_launch={:.0}ms  to_prompt={:.0}ms  (no markers)",
                            ms(l.first),
                            ms(l.ready),
                        ),
                    }
                },
                Err(e) => println!("{label} #{run}  failed: {e}"),
            }
        }
    }
}
