//! `std.process.run` (requirement S-6): a program runs directly, never through a shell, with a
//! time limit, and both outputs are captured as UTF-8 text.
//!
//! The boundaries of a call (docs/runtime.md 7.5 and 9.5):
//!
//! - the environment of the child is built, not inherited: the variables the sandbox grants
//!   under `env` and `PATH` (and `SystemRoot` on Windows), see
//!   [`crate::sandbox::child_environment`];
//! - the child runs in a process tree of its own ([`crate::sandbox::tree`]): a new process
//!   group on Unix, which no descendant can leave on Linux (a seccomp filter denies `setsid`
//!   and `setpgid`) and whose leavers macOS finds through their parents and the marker of the
//!   Seatbelt profile; on Windows a Job Object with kill-on-close and without breakaway, which
//!   the child joins while it is still suspended. A job that cannot be set up fails the call
//!   before the child runs. When the child ends, what is left of the tree is stopped; on
//!   timeout, on output over the limit, and on exhaustion the whole tree is stopped;
//! - the deadline covers the whole call, including reading the outputs: a process that keeps a
//!   pipe open after leaving the tree cannot make the call wait longer;
//! - each captured stream holds at most [`OUTPUT_LIMIT`] bytes, and what is captured counts
//!   against the heap limit before it is stored. Between the pipes and the call the output
//!   passes through a bounded queue: a reader stops reading while its chunks are not taken, so
//!   at most [`IN_FLIGHT`] bytes are on the way, and they count against the heap limit for the
//!   whole call. Output over the limit ends the call at once and stops the tree; the readers
//!   then end at their next chunk.

use super::{int, text, texts, ExternFn};
use crate::fault::{Outcome, Unwind};
use crate::heap::{Counted, Measure};
use crate::runtime::Runtime;
use crate::sandbox::tree;
use crate::value::Value;
use std::io::{ErrorKind, Read};
use std::process::ExitStatus;
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tree::Child;

pub const BINDINGS: &[(&str, ExternFn)] = &[("std.process.run", run)];

/// The most bytes captured from each of standard output and standard error (16 MiB); more is
/// a `ProcessError` of kind `OutputTooLarge`. Declared in `stdlib/std/process.unio`.
pub const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// The size of one read from a pipe.
const CHUNK: usize = 64 * 1024;

/// The chunks the queue between the readers and the call holds.
const QUEUE: usize = 2;

/// The most bytes on the way from the pipes to the call: the chunks in the queue, plus, for
/// each of the two readers, the chunk it waits to send and the buffer it reads into.
pub const IN_FLIGHT: usize = (QUEUE + 2 * 2) * CHUNK;

/// One message of a reader: a chunk of its stream, or `None` at its end.
type Message = (usize, Option<Vec<u8>>);

/// The bounded queue from the readers of the pipes to the call.
fn queue() -> (mpsc::SyncSender<Message>, mpsc::Receiver<Message>) {
    mpsc::sync_channel(QUEUE)
}

/// What the output on the way takes of the heap, counted for the whole call.
struct InFlight;

impl Measure for InFlight {
    fn measure(&self) -> usize {
        IN_FLIGHT
    }
}

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
    // The output on the way counts against the heap limit before the child starts.
    runtime.reserve_heap(IN_FLIGHT)?;
    let in_flight = Counted::new(InFlight);
    let mut child = spawn(program, &args).map_err(|e| {
        let kind = match e.kind() {
            ErrorKind::NotFound => "NotFound",
            ErrorKind::PermissionDenied => "PermissionDenied",
            _ => "Other",
        };
        process_error(program, kind, e.to_string())
    })?;
    let tree = tree::Tree::start(&mut child, tree::applied_marker().cloned()).map_err(|e| {
        process_error(
            program,
            "Other",
            format!("the process tree of the child cannot be set up: {e}"),
        )
    })?;
    let mut running = Running::new(child, tree);
    let (sender, receiver) = queue();
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
        if running.tree.over_limit() {
            let limit = crate::sandbox::process_budget().map_or(0, |(limit, _)| limit);
            return Err(process_error(
                program,
                "PermissionDenied",
                format!(
                    "denied by the sandbox: the process tree started more processes than the \
                     policy allows the program ({limit}) and was stopped"
                ),
            ));
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
    drop(in_flight);
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

/// Starts the child suspended in its tree where the system needs that (Windows), with the
/// environment the sandbox grants, standard input empty and both outputs piped.
#[cfg(not(windows))]
fn spawn(program: &str, args: &[String]) -> std::io::Result<Child> {
    let mut command = Command::new(program);
    crate::sandbox::limit_child(&mut command);
    if let Some(environment) = crate::sandbox::child_environment() {
        command.env_clear().envs(environment);
    }
    tree::prepare(&mut command);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// Windows: the program is found as `windows::find_program` finds it, through the `PATH` the
/// child receives, and created suspended in the AppContainer of the running program (none
/// without a sandbox). A batch file is not started: Windows runs it through `cmd.exe`, which
/// then reads its arguments as commands.
#[cfg(windows)]
fn spawn(program: &str, args: &[String]) -> std::io::Result<Child> {
    use crate::sandbox::windows as w;
    use std::ffi::{OsStr, OsString};
    let env: Option<Vec<(OsString, OsString)>> = crate::sandbox::child_environment().map(|vars| {
        vars.into_iter()
            .map(|(k, v)| (OsString::from(k), v))
            .collect()
    });
    let path = match &env {
        Some(vars) => vars
            .iter()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v.clone()),
        None => std::env::var_os("PATH"),
    };
    let found = w::find_program(OsStr::new(program), path.as_deref())
        .ok_or_else(|| std::io::Error::new(ErrorKind::NotFound, "the program is not found"))?;
    let extension = found
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    if matches!(extension.as_deref(), Some("bat" | "cmd")) {
        return Err(std::io::Error::other(
            "a batch file runs only through cmd.exe, which must be listed and run instead",
        ));
    }
    crate::sandbox::with_container(|container| {
        let mut command = w::Command::new(&found);
        command.args = args.iter().map(OsString::from).collect();
        command.env = env;
        command.stdin = w::Stdio::Null;
        command.stdout = w::Stdio::Piped;
        command.stderr = w::Stdio::Piped;
        command.container = container;
        w::spawn(&command)
    })
}

/// The bytes of a counted buffer; the value that receives them counts them again.
fn take(mut buffer: Counted<Vec<u8>>) -> Vec<u8> {
    let data = std::mem::take(&mut *buffer);
    buffer.recount();
    data
}

/// Reads a pipe in chunks until its end and sends them through the bounded queue, waiting
/// while it is full; stops when the call no longer listens. A thread whose pipe a process
/// outside the tree keeps open outlives the call, blocked in a read that ends when that
/// process writes or closes the pipe; the call itself does not wait for it.
fn read_pipe(stream: usize, mut pipe: Box<dyn Read + Send>, sender: mpsc::SyncSender<Message>) {
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
    fn new(child: Child, tree: tree::Tree) -> Running {
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

/// Stops the process trees of children that still run. The watchdog of the time limits calls
/// it before it ends the process with code 102, since a child in a process group of its own
/// would otherwise outlive the program. Windows stops them through kill-on-close of the job
/// when the process ends.
pub fn stop_children() {
    tree::stop_all();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A pipe that never ends on its own: it yields chunks until `total` bytes, counting them.
    struct Endless {
        read: Arc<AtomicUsize>,
        total: usize,
    }

    impl Read for Endless {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let done = self.read.load(Ordering::SeqCst);
            let n = buffer.len().min(self.total - done);
            buffer[..n].fill(b'x');
            self.read.fetch_add(n, Ordering::SeqCst);
            Ok(n)
        }
    }

    /// A reader of a pipe whose chunks nobody takes stops reading after a bounded number of
    /// bytes, so that a child that writes fast cannot grow the memory of the call past the
    /// limit of its output while the call is busy.
    #[test]
    fn a_reader_whose_chunks_are_not_taken_stops_reading() {
        let read = Arc::new(AtomicUsize::new(0));
        let pipe = Endless {
            read: read.clone(),
            total: 4 * OUTPUT_LIMIT,
        };
        let (sender, receiver) = queue();
        let reader = thread::spawn(move || read_pipe(0, Box::new(pipe), sender));
        thread::sleep(Duration::from_millis(300));
        let bytes = read.load(Ordering::SeqCst);
        assert!(
            bytes <= IN_FLIGHT,
            "{bytes} bytes were read ahead, more than {IN_FLIGHT}"
        );
        drop(receiver);
        reader.join().unwrap();
        assert!(read.load(Ordering::SeqCst) <= IN_FLIGHT + CHUNK);
    }
}
