//! `std.process.run` (requirement S-6): a program runs directly, never through a shell, with a
//! time limit, and both outputs are captured as UTF-8 text.
//!
//! The boundaries of a call (docs/runtime.md 7.5 and 9.5):
//!
//! - the environment of the child is built, not inherited: the variables the sandbox grants
//!   under `env` and `PATH` (and `SystemRoot` on Windows), see
//!   [`crate::sandbox::child_environment`];
//! - the child runs in a process tree of its own: a new process group on Unix, which no
//!   descendant can leave on Linux (a seccomp filter denies `setsid` and `setpgid`) and whose
//!   leavers macOS finds through their parents; a Job Object with kill-on-close and without
//!   breakaway on Windows. When the child ends, what is left of the tree is stopped; on
//!   timeout, on output over the limit, and on exhaustion the whole tree is stopped;
//! - the deadline covers the whole call, including reading the outputs: a process that keeps a
//!   pipe open after leaving the tree cannot make the call wait longer;
//! - each captured stream holds at most [`OUTPUT_LIMIT`] bytes, and what is captured counts
//!   against the heap limit before it is stored.

use super::{int, text, texts, ExternFn};
use crate::fault::{Outcome, Unwind};
use crate::heap::Counted;
use crate::runtime::Runtime;
use crate::value::Value;
use std::io::{ErrorKind, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const BINDINGS: &[(&str, ExternFn)] = &[("std.process.run", run)];

/// The most bytes captured from each of standard output and standard error (16 MiB); more is
/// a `ProcessError` of kind `OutputTooLarge`. Declared in `stdlib/std/process.unio`.
pub const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// The size of one read from a pipe.
const CHUNK: usize = 64 * 1024;

fn process_error(program: &str, kind: &str, reason: impl Into<String>) -> Unwind {
    Unwind::Error(Box::new(Value::record(
        "ProcessError",
        vec![
            ("program", Value::text(program)),
            ("kind", Value::variant("ProcessErrorKind", kind, vec![])),
            ("reason", Value::text(reason.into())),
        ],
    )))
}

/// Runs the program with the arguments and waits at most timeout_seconds, which must be
/// positive. A program that does not finish in time is stopped with its process tree and the
/// call fails with TimedOut; output over the limit fails with OutputTooLarge; output that is
/// not UTF-8 fails with InvalidOutput. A non-zero status is not an error. On Unix, a program
/// ended by a signal has the status 128 plus the signal number, as in shells. Standard input is
/// empty.
fn run(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let program = text(runtime, arguments, 0)?;
    let program = program.as_str();
    let args = texts(runtime, arguments, 1)?;
    let timeout = int(runtime, arguments, 2)?;
    if timeout <= 0 {
        return Err(process_error(
            program,
            "Other",
            "timeout_seconds must be positive",
        ));
    }
    let started = Instant::now();
    let deadline = started
        .checked_add(Duration::from_secs(timeout.unsigned_abs()))
        .unwrap_or(started + Duration::from_secs(u32::MAX.into()));
    crate::sandbox::may_run(program)
        .map_err(|reason| process_error(program, "PermissionDenied", reason))?;
    let mut command = Command::new(program);
    crate::sandbox::limit_child(&mut command);
    if let Some(environment) = crate::sandbox::child_environment() {
        command.env_clear().envs(environment);
    }
    tree::own_tree(&mut command);
    crate::sandbox::keep_in_tree(&mut command);
    let child = command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let kind = match e.kind() {
                ErrorKind::NotFound => "NotFound",
                ErrorKind::PermissionDenied => "PermissionDenied",
                _ => "Other",
            };
            process_error(program, kind, e.to_string())
        })?;
    let mut running = Running::new(child);
    let (sender, receiver) = mpsc::channel::<(usize, Option<Vec<u8>>)>();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        running
            .child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        running
            .child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ];
    for (stream, pipe) in pipes.into_iter().enumerate() {
        let Some(pipe) = pipe else {
            let _ = sender.send((stream, None));
            continue;
        };
        let sender = sender.clone();
        let spawned = thread::Builder::new()
            .name("unio-process-output".into())
            .spawn(move || read_pipe(stream, pipe, sender));
        if spawned.is_err() {
            return Err(process_error(
                program,
                "Other",
                "a thread to read the output cannot start",
            ));
        }
    }
    drop(sender);
    // The captured outputs count against the heap limit like values do.
    let mut captured = [Counted::new(Vec::<u8>::new()), Counted::new(Vec::new())];
    let mut open = 2;
    let mut status: Option<ExitStatus> = None;
    loop {
        if status.is_none() {
            match running.exited() {
                Ok(Some(s)) => status = Some(s),
                Ok(None) => {}
                Err(e) => return Err(process_error(program, "Other", e.to_string())),
            }
        }
        if status.is_some() && open == 0 {
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            let reason = if status.is_some() {
                format!(
                    "the program ended, but a process it started still held its output after \
                     {timeout} s and was stopped"
                )
            } else {
                format!("the program did not finish in {timeout} s and was stopped")
            };
            return Err(process_error(program, "TimedOut", reason));
        }
        let wait = (deadline - now).min(Duration::from_millis(5));
        match receiver.recv_timeout(wait) {
            Ok((stream, Some(chunk))) => {
                let buffer = &mut captured[stream];
                if buffer.len() + chunk.len() > OUTPUT_LIMIT {
                    let name = ["standard output", "standard error"][stream];
                    return Err(process_error(
                        program,
                        "OutputTooLarge",
                        format!("{name} is longer than {OUTPUT_LIMIT} bytes"),
                    ));
                }
                if buffer.capacity() - buffer.len() < chunk.len() {
                    // Grow by doubling, but never past the limit of the stream.
                    let more = buffer
                        .len()
                        .min(OUTPUT_LIMIT - buffer.len())
                        .max(chunk.len());
                    super::reserve(runtime, buffer, more)?;
                }
                buffer.extend_from_slice(&chunk);
                buffer.recount();
                runtime.check_heap()?;
            }
            Ok((_, None)) => open -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => open = 0,
        }
    }
    let status = status.expect("the loop ends after the program ended");
    running.finished = true;
    let [out, err] = captured;
    let out = String::from_utf8(take(out))
        .map_err(|_| process_error(program, "InvalidOutput", "standard output is not UTF-8"))?;
    let err = String::from_utf8(take(err))
        .map_err(|_| process_error(program, "InvalidOutput", "standard error is not UTF-8"))?;
    let result = Value::record(
        "Output",
        vec![
            ("status", Value::Int(exit_status(&status))),
            ("stdout", Value::text(out)),
            ("stderr", Value::text(err)),
        ],
    );
    runtime.check_heap()?;
    Ok(result)
}

/// The bytes of a counted buffer; the value that receives them counts them again.
fn take(mut buffer: Counted<Vec<u8>>) -> Vec<u8> {
    let data = std::mem::take(&mut *buffer);
    buffer.recount();
    data
}

/// Reads a pipe in chunks until its end and sends them; stops when the call no longer listens.
/// A thread whose pipe a process outside the tree keeps open outlives the call, blocked in a
/// read that ends when that process closes the pipe; the call itself does not wait for it.
fn read_pipe(
    stream: usize,
    mut pipe: Box<dyn Read + Send>,
    sender: mpsc::Sender<(usize, Option<Vec<u8>>)>,
) {
    let mut chunk = vec![0u8; CHUNK];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if sender.send((stream, Some(chunk[..n].to_vec()))).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = sender.send((stream, None));
}

/// A started child and its process tree. Until the call finishes normally, dropping it stops
/// the whole tree, so that every early return (timeout, output over the limit, exhaustion)
/// leaves no process behind in the tree.
struct Running {
    child: Child,
    tree: tree::Tree,
    /// The child has ended and was reaped; the rest of its tree was stopped.
    reaped: bool,
    /// The call finished normally.
    finished: bool,
}

impl Running {
    fn new(child: Child) -> Running {
        let tree = tree::Tree::new(&child);
        Running {
            child,
            tree,
            reaped: false,
            finished: false,
        }
    }

    /// The status of the child once it has ended. When it ends, the processes it left in its
    /// tree are stopped before the child is reaped, so that the identity of the tree cannot be
    /// reused by an unrelated process in between.
    fn exited(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if !tree::has_exited(&mut self.child)? {
            return Ok(None);
        }
        self.tree.stop();
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(Some(status))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if !self.reaped {
            self.tree.stop();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(unix)]
mod tree {
    use std::process::{Child, Command};

    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
        fn waitid(idtype: u32, id: u32, info: *mut u8, options: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    const P_PID: u32 = 1;
    const WEXITED: i32 = 4;
    const WNOHANG: i32 = 1;
    #[cfg(target_os = "linux")]
    const WNOWAIT: i32 = 0x0100_0000;
    #[cfg(not(target_os = "linux"))]
    const WNOWAIT: i32 = 0x20;

    /// The child leads a new process group, which its descendants join; on Linux they cannot
    /// leave it (`sandbox::keep_in_tree`), on macOS they can (setsid, setpgid).
    pub fn own_tree(command: &mut Command) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    /// The process group of the child, whose number is the number of the child.
    pub struct Tree {
        group: i32,
        live: bool,
    }

    impl Tree {
        pub fn new(child: &Child) -> Tree {
            let group = i32::try_from(child.id()).unwrap_or(0);
            if group > 0 {
                super::register(group);
            }
            Tree {
                group,
                live: group > 0,
            }
        }

        /// Stops every process of the group with SIGKILL. The child is not reaped yet when this
        /// runs, so the number of the group still belongs to this tree.
        pub fn stop(&mut self) {
            if self.live {
                stop_group(self.group);
                super::unregister(self.group);
                self.live = false;
            }
        }
    }

    /// Stops a tree: on Linux its descendants cannot leave the group (the filter of
    /// `sandbox::keep_in_tree`), so SIGKILL to the group stops them all.
    #[cfg(not(target_os = "macos"))]
    fn stop_group(group: i32) {
        // SAFETY: kill with a negative number signals the process group; it has no memory
        // effects.
        unsafe {
            kill(-group, SIGKILL);
        }
    }

    /// Stops a tree on macOS, where a descendant can leave the group: the group is frozen with
    /// SIGSTOP, the descendants of the child are found through their parents and frozen in
    /// turn, so that none starts another process meanwhile, and then all of them and the group
    /// get SIGKILL. A descendant whose parent has already ended belongs to launchd and is not
    /// found (docs/runtime.md 9.5).
    #[cfg(target_os = "macos")]
    fn stop_group(group: i32) {
        const SIGSTOP: i32 = 17;
        const PROC_PPID_ONLY: u32 = 6;
        const MOST: usize = 4096;
        extern "C" {
            fn proc_listpids(kind: u32, info: u32, buffer: *mut i32, size: i32) -> i32;
        }
        // SAFETY: kill has no memory effects.
        unsafe {
            kill(-group, SIGSTOP);
        }
        let mut found = vec![group];
        let mut next = 0;
        let mut buffer = vec![0i32; 1024];
        while next < found.len() && found.len() < MOST {
            let parent = found[next];
            next += 1;
            let size = i32::try_from(buffer.len() * 4).unwrap_or(i32::MAX);
            // SAFETY: the buffer holds size bytes; the call writes at most that many and
            // returns how many it wrote.
            let bytes =
                unsafe { proc_listpids(PROC_PPID_ONLY, parent as u32, buffer.as_mut_ptr(), size) };
            let count = usize::try_from(bytes).unwrap_or(0) / 4;
            for &pid in &buffer[..count.min(buffer.len())] {
                if pid > 0 && !found.contains(&pid) {
                    // SAFETY: as above.
                    unsafe {
                        kill(pid, SIGSTOP);
                    }
                    found.push(pid);
                }
            }
        }
        // SAFETY: as above.
        unsafe {
            kill(-group, SIGKILL);
            for &pid in &found[1..] {
                kill(pid, SIGKILL);
            }
        }
    }

    /// Whether the child has ended, without reaping it (waitid with WNOWAIT).
    pub fn has_exited(child: &mut Child) -> std::io::Result<bool> {
        let mut info = [0u8; 256];
        // SAFETY: info is larger than siginfo_t on every Unix the runtime supports; waitid
        // writes at most one siginfo_t.
        let result = unsafe {
            waitid(
                P_PID,
                child.id(),
                info.as_mut_ptr(),
                WEXITED | WNOHANG | WNOWAIT,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // si_signo is the first int of siginfo_t; it stays 0 when no child has ended.
        Ok(i32::from_ne_bytes([info[0], info[1], info[2], info[3]]) != 0)
    }

    /// Stops the process groups of children that still run; for the watchdog of the time
    /// limits, which ends the process.
    pub fn stop_groups(groups: &[i32]) {
        for &group in groups {
            stop_group(group);
        }
    }
}

#[cfg(windows)]
mod tree {
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};

    type Handle = *mut std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        minimum_working_set: usize,
        maximum_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut std::ffi::c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: i32,
            info: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    /// The flags of the job: kill-on-close only. Neither JOB_OBJECT_LIMIT_BREAKAWAY_OK
    /// (0x800) nor JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK (0x1000) is set, so a process in the
    /// job cannot start a child outside it with CREATE_BREAKAWAY_FROM_JOB.
    const JOB_FLAGS: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    const _: () = assert!(JOB_FLAGS & (0x800 | 0x1000) == 0);

    /// Nothing to set before the start: the child joins its Job Object right after it starts.
    pub fn own_tree(_: &mut Command) {}

    /// A Job Object that holds the child and every process it starts; closing it stops them
    /// all (kill-on-close). A process the child starts in the moment before it joins the job
    /// is not in it.
    pub struct Tree {
        job: Handle,
    }

    impl Tree {
        pub fn new(child: &Child) -> Tree {
            // SAFETY: a new unnamed job with default security; the limits structure is the
            // documented JOBOBJECT_EXTENDED_LIMIT_INFORMATION; the process handle belongs to
            // the child, which is alive.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return Tree { job };
                }
                let mut limits = ExtendedLimits::default();
                limits.basic.flags = JOB_FLAGS;
                let set = SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&mut limits as *mut ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                );
                if set == 0 || AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                    CloseHandle(job);
                    return Tree {
                        job: std::ptr::null_mut(),
                    };
                }
                Tree { job }
            }
        }

        /// Stops every process of the job and closes it.
        pub fn stop(&mut self) {
            if !self.job.is_null() {
                // SAFETY: the job handle is owned by this tree and closed once.
                unsafe {
                    TerminateJobObject(self.job, 1);
                    CloseHandle(self.job);
                }
                self.job = std::ptr::null_mut();
            }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            self.stop();
        }
    }

    pub fn has_exited(child: &mut Child) -> std::io::Result<bool> {
        // On Windows waiting does not reuse the identity of the job: the handle is ours.
        Ok(child.try_wait()?.is_some())
    }
}

#[cfg(not(any(unix, windows)))]
mod tree {
    use std::process::{Child, Command};

    pub fn own_tree(_: &mut Command) {}

    pub struct Tree;

    impl Tree {
        pub fn new(_: &Child) -> Tree {
            Tree
        }

        pub fn stop(&mut self) {}
    }

    pub fn has_exited(child: &mut Child) -> std::io::Result<bool> {
        Ok(child.try_wait()?.is_some())
    }
}

/// The process groups of children that run now, for [`stop_children`].
#[cfg(unix)]
static GROUPS: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

#[cfg(unix)]
fn register(group: i32) {
    if let Ok(mut groups) = GROUPS.lock() {
        groups.push(group);
    }
}

#[cfg(unix)]
fn unregister(group: i32) {
    if let Ok(mut groups) = GROUPS.lock() {
        groups.retain(|&g| g != group);
    }
}

/// Stops the process trees of children that still run. The watchdog of the time limits calls
/// it before it ends the process with code 102, since a child in a process group of its own
/// would otherwise outlive the program. Windows stops them through kill-on-close of the job
/// when the process ends.
pub fn stop_children() {
    #[cfg(unix)]
    if let Ok(groups) = GROUPS.try_lock() {
        tree::stop_groups(&groups);
    }
}

#[cfg(unix)]
fn exit_status(status: &ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => i64::from(code),
        (None, Some(signal)) => 128 + i64::from(signal),
        (None, None) => -1,
    }
}

#[cfg(not(unix))]
fn exit_status(status: &ExitStatus) -> i64 {
    status.code().map_or(-1, i64::from)
}
