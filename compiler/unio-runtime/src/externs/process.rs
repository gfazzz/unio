//! `std.process.run` (requirement S-6): a program runs directly, never through a shell, with a
//! time limit, and both outputs are captured as UTF-8 text.

use super::{int, text, texts, ExternFn};
use crate::fault::{Outcome, Unwind};
use crate::runtime::Runtime;
use crate::value::Value;
use std::io::{ErrorKind, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const BINDINGS: &[(&str, ExternFn)] = &[("std.process.run", run)];

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
/// positive. A program that does not finish in time is stopped and the call fails with
/// TimedOut; output that is not UTF-8 fails with InvalidOutput. A non-zero status is not an
/// error. On Unix, a program ended by a signal has the status 128 plus the signal number, as in
/// shells. Standard input is empty.
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
    crate::sandbox::may_run(program)
        .map_err(|reason| process_error(program, "PermissionDenied", reason))?;
    let mut command = Command::new(program);
    crate::sandbox::limit_child(&mut command);
    let mut child = command
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
    let mut stdout = child.stdout.take().expect("a piped standard output");
    let mut stderr = child.stderr.take().expect("a piped standard error");
    let out_reader = thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stdout.read_to_end(&mut data);
        data
    });
    let err_reader = thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stderr.read_to_end(&mut data);
        data
    });
    let deadline = Instant::now() + Duration::from_secs(timeout.unsigned_abs());
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err(process_error(
                    program,
                    "TimedOut",
                    format!("the program did not finish in {timeout} s and was stopped"),
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(process_error(program, "Other", e.to_string())),
        }
    };
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    let out = String::from_utf8(out)
        .map_err(|_| process_error(program, "InvalidOutput", "standard output is not UTF-8"))?;
    let err = String::from_utf8(err)
        .map_err(|_| process_error(program, "InvalidOutput", "standard error is not UTF-8"))?;
    Ok(Value::record(
        "Output",
        vec![
            ("status", Value::Int(exit_status(&status))),
            ("stdout", Value::text(out)),
            ("stderr", Value::text(err)),
        ],
    ))
}

#[cfg(unix)]
fn exit_status(status: &std::process::ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => i64::from(code),
        (None, Some(signal)) => 128 + i64::from(signal),
        (None, None) => -1,
    }
}

#[cfg(not(unix))]
fn exit_status(status: &std::process::ExitStatus) -> i64 {
    status.code().map_or(-1, i64::from)
}
