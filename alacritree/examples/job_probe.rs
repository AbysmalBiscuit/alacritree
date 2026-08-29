//! Can a ConPTY child be put in a job object, and does the job's priority
//! class reach the processes it starts?
//!
//! Throwaway instrumentation for the load-latency diagnosis.  Raising a shell
//! catches nothing that lives less than one scan interval, and a shell's short
//! commands are exactly what a saturated machine is slowest to start.  A job
//! object would cover them with no scanning at all — processes created by a
//! member join the job automatically — but only if ConPTY has not already
//! claimed the child for a job of its own, which is what this asks.
//!
//! Job objects are a Windows facility and CI builds every target on Linux, so
//! everything below is gated and the example is a message elsewhere.
//!
//! ```text
//! cargo run -p alacritree --release --example job_probe
//! ```

#[cfg(windows)]
use std::io::Write as _;
#[cfg(windows)]
use std::process::{Command, Stdio};

#[cfg(windows)]
use alacritty_terminal::event::WindowSize;
#[cfg(windows)]
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_PRIORITY_CLASS,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, GetPriorityClass, NORMAL_PRIORITY_CLASS, OpenProcess,
    PROCESS_QUERY_INFORMATION,
};

#[cfg(windows)]
const COLS: u16 = 120;
#[cfg(windows)]
const LINES: u16 = 40;

#[cfg(not(windows))]
fn main() {
    eprintln!("job_probe asks a question only Windows has an answer to");
}

#[cfg(windows)]
fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

#[cfg(windows)]
fn class_of(pid: u32) -> String {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid) };
    if handle.is_null() {
        return format!("unreadable ({})", last_error());
    }
    let class = unsafe { GetPriorityClass(handle) };
    unsafe { CloseHandle(handle) };
    match class {
        0x0000_8000 => "above normal".to_string(),
        0x0000_0020 => "normal".to_string(),
        other => format!("{other:#x}"),
    }
}

/// The pids the job currently holds.  A grandchild appearing here is the whole
/// point: it means coverage costs no scanning.
#[cfg(windows)]
fn job_members(job: HANDLE) -> Vec<u32> {
    // The struct carries one pid inline; the rest are read past its end, so
    // the query gets a buffer sized for a plausible tree rather than the
    // struct alone.  A pid here is a `ULONG_PTR`, so the buffer is typed as
    // one: it gives the header the alignment it wants, and the count of ids
    // that fit is then just its length less the two-`u32` header.
    const ROOM: usize = 256;
    let mut buffer = vec![0usize; ROOM];
    let bytes = std::mem::size_of_val(buffer.as_slice()) as u32;
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicProcessIdList,
            buffer.as_mut_ptr().cast(),
            bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        eprintln!("  could not list the job's processes: {}", last_error());
        return Vec::new();
    }
    let header: &JOBOBJECT_BASIC_PROCESS_ID_LIST = unsafe { &*buffer.as_ptr().cast() };
    let returned = header.NumberOfProcessIdsInList as usize;
    // `ProcessIdList` is declared as one element and continues past it.
    let list = unsafe {
        std::slice::from_raw_parts(header.ProcessIdList.as_ptr(), returned.min(ROOM - 1))
    };
    list.iter().map(|&pid| pid as u32).collect()
}

#[cfg(windows)]
fn main() {
    let shell = std::env::args().nth(1).unwrap_or_else(|| "cmd.exe".to_string());

    let pty_options = PtyOptions {
        shell: Some(Shell::new(shell.clone(), Vec::new())),
        working_directory: None,
        drain_on_exit: false,
        env: Default::default(),
        escape_args: false,
    };
    let window_size =
        WindowSize { num_lines: LINES, num_cols: COLS, cell_width: 8, cell_height: 16 };
    let pty = tty::new(&pty_options, window_size, 0).expect("open a pty");
    let handle = pty.child_watcher().raw_handle();
    let pid = pty.child_watcher().pid().map(std::num::NonZeroU32::get).unwrap_or(0);
    println!("ConPTY child: {shell} pid {pid}");

    // Whether conhost already claimed it decides everything below: a process
    // that is already in a job can still be assigned on Windows 8 and later,
    // but only by nesting, and nesting has a depth limit.
    let mut already = 0i32;
    unsafe { IsProcessInJob(handle, std::ptr::null_mut(), &mut already) };
    println!("already in a job before we touch it: {}", already != 0);

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        println!("FAIL: could not create a job object: {}", last_error());
        return;
    }

    if unsafe { AssignProcessToJobObject(job, handle) } == 0 {
        println!("FAIL: the ConPTY child refused assignment: {}", last_error());
        println!("      a job object is not an option; the scanning boost stands");
        return;
    }
    println!("OK: the ConPTY child was assigned to the job");

    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation = JOBOBJECT_BASIC_LIMIT_INFORMATION {
        LimitFlags: JOB_OBJECT_LIMIT_PRIORITY_CLASS,
        PriorityClass: ABOVE_NORMAL_PRIORITY_CLASS,
        ..unsafe { std::mem::zeroed() }
    };
    let set = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    if set == 0 {
        println!("FAIL: the job refused a priority class: {}", last_error());
        return;
    }
    println!("OK: the job carries an above-normal priority class");
    println!("  the shell now reads: {}", class_of(pid));

    // A plain child of this process, assigned the same way, answers the second
    // half: whether what a job member starts joins the job and takes the class
    // without anything having to notice it was born.
    let mut parent = Command::new("cmd.exe")
        .args(["/c", "ping -n 20 127.0.0.1 > nul"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn a stand-in");
    let parent_handle = unsafe {
        OpenProcess(
            windows_sys::Win32::System::Threading::PROCESS_SET_QUOTA
                | windows_sys::Win32::System::Threading::PROCESS_TERMINATE,
            0,
            parent.id(),
        )
    };
    if unsafe { AssignProcessToJobObject(job, parent_handle) } == 0 {
        println!("  (could not add the stand-in: {})", last_error());
    }
    std::thread::sleep(std::time::Duration::from_millis(1500));

    let members = job_members(job);
    println!("job holds {} process(es):", members.len());
    for member in &members {
        println!("  pid {member}: {}", class_of(*member));
    }

    // Blur has two candidate implementations and they differ by a whole
    // enumeration pass.  Setting the limit to normal, if it lowers members the
    // way raising them raised them, makes blur one call.  Try that before
    // dropping the limit, which is the case already known not to lower.
    limits.BasicLimitInformation.PriorityClass = NORMAL_PRIORITY_CLASS;
    let renormalised = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    println!("limit set to normal: {}", renormalised != 0);
    std::thread::sleep(std::time::Duration::from_millis(300));
    for member in &members {
        println!("  pid {member}: {}", class_of(*member));
    }

    // Focus following needs the raise to come back off, so the last question
    // is whether dropping the limit lowers what the job already covers or only
    // what it takes on next.
    limits.BasicLimitInformation.LimitFlags = 0;
    let cleared = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    println!("limit dropped: {}", cleared != 0);
    std::thread::sleep(std::time::Duration::from_millis(300));
    for member in &members {
        println!("  pid {member}: {}", class_of(*member));
    }

    let _ = parent.kill();
    let _ = parent.wait();
    let _ = std::io::stdout().flush();
    unsafe {
        CloseHandle(parent_handle);
        CloseHandle(job);
    }
}
