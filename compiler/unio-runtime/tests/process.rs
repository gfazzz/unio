//! std.process.run: no shell, captured output, status, and the time limit (S-6).

#![cfg(unix)]

use unio_runtime::{call, Outcome, Runtime, Unwind, Value};

fn run(program: &str, arguments: &[&str], timeout: i64) -> Outcome {
    let mut rt = Runtime::new(vec![]);
    let arguments = Value::list(arguments.iter().map(|a| Value::text(*a)).collect());
    call(
        &mut rt,
        "std.process.run",
        &mut [Value::text(program), arguments, Value::Int(timeout)],
    )
}

fn kind(outcome: Outcome) -> String {
    match outcome {
        Err(Unwind::Error(e)) => e
            .field("kind")
            .and_then(|k| k.variant_name())
            .unwrap()
            .to_string(),
        other => panic!("expected a ProcessError, got {other:?}"),
    }
}

#[test]
fn arguments_reach_the_program_without_a_shell() {
    let output = run("echo", &["$HOME;", "ёж", "a b"], 10).unwrap();
    assert_eq!(
        output.field("stdout"),
        Some(&Value::text("$HOME; ёж a b\n"))
    );
    assert_eq!(output.field("status"), Some(&Value::Int(0)));
}

#[test]
fn status_and_standard_error_are_reported() {
    let output = run("sh", &["-c", "echo oops >&2; exit 3"], 10).unwrap();
    assert_eq!(output.field("status"), Some(&Value::Int(3)));
    assert_eq!(output.field("stderr"), Some(&Value::text("oops\n")));
}

#[test]
fn a_program_that_does_not_finish_in_time_is_stopped() {
    let started = std::time::Instant::now();
    assert_eq!(kind(run("sleep", &["30"], 1)), "TimedOut");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn missing_programs_invalid_output_and_bad_limits() {
    assert_eq!(kind(run("unio-no-such-program", &[], 5)), "NotFound");
    assert_eq!(kind(run("printf", &["\\377"], 5)), "InvalidOutput");
    assert_eq!(kind(run("echo", &[], 0)), "Other");
}

/// Whether a process is gone: not running, or a zombie its new parent has not reaped yet.
fn gone(pid: &str) -> bool {
    for _ in 0..200 {
        let state = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout).trim().to_string();
        if state.is_empty() || state.starts_with('Z') {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[test]
fn a_grandchild_that_holds_the_output_does_not_hold_the_call() {
    // The child starts a grandchild that keeps standard output open and exits at once.
    let started = std::time::Instant::now();
    let output = run("sh", &["-c", "sleep 30 & echo $!"], 5).unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the call waited for the grandchild: {:?}",
        started.elapsed()
    );
    assert_eq!(output.field("status"), Some(&Value::Int(0)));
    let Some(Value::Text(pid)) = output.field("stdout") else {
        panic!("{output:?}")
    };
    assert!(
        gone(pid.as_str()),
        "the grandchild {} still runs",
        pid.as_str()
    );
}

#[test]
fn a_timeout_stops_the_whole_process_tree() {
    let dir = std::env::temp_dir().join(format!("unio-process-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pid");
    let script = format!("sleep 30 & echo $! > '{}'; sleep 30", pid_file.display());
    let started = std::time::Instant::now();
    assert_eq!(kind(run("sh", &["-c", &script], 1)), "TimedOut");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(gone(&pid), "the grandchild {pid} still runs");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn output_over_the_limit_is_an_error() {
    // 16 MiB is the documented limit of each captured stream.
    let over = (16 * 1024 * 1024 + 1).to_string();
    assert_eq!(
        kind(run("head", &["-c", &over, "/dev/zero"], 30)),
        "OutputTooLarge"
    );
    let script = format!("head -c {over} /dev/zero >&2");
    assert_eq!(kind(run("sh", &["-c", &script], 30)), "OutputTooLarge");
    let at = (16 * 1024 * 1024).to_string();
    let output = run("head", &["-c", &at, "/dev/zero"], 30).unwrap();
    assert!(
        matches!(output.field("stdout"), Some(Value::Text(t)) if t.as_str().len() == 16 * 1024 * 1024)
    );
}

#[test]
fn captured_output_counts_against_the_heap_limit() {
    let mut rt = Runtime::new(vec![]);
    rt.set_heap_limit(Some(unio_runtime::reserve::RESERVE_BYTES + (1 << 20)));
    let arguments = Value::list(vec![
        Value::text("-c"),
        Value::text("4000000"),
        Value::text("/dev/zero"),
    ]);
    let outcome = call(
        &mut rt,
        "std.process.run",
        &mut [Value::text("head"), arguments, Value::Int(30)],
    );
    assert!(
        matches!(
            outcome,
            Err(Unwind::Exhausted(unio_runtime::Exhaustion::Heap))
        ),
        "{outcome:?}"
    );
}

/// A child that writes far more than the limit on both streams, as fast as it can: the call
/// ends with OutputTooLarge once a stream passes the limit, and stops the child, instead of
/// reading on.
#[test]
fn a_child_that_writes_far_more_than_the_limit_is_stopped_at_it() {
    let started = std::time::Instant::now();
    let script = "head -c 4294967296 /dev/zero >&2 & head -c 4294967296 /dev/zero; wait";
    assert_eq!(kind(run("sh", &["-c", script], 120)), "OutputTooLarge");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
}

/// The output on its way from the pipes to the call is bounded and counts against the heap
/// limit for the whole call: a heap with less room than that bound is exhausted before the
/// child starts.
#[test]
fn the_output_on_its_way_counts_against_the_heap_limit() {
    let mut rt = Runtime::new(vec![]);
    rt.set_heap_limit(Some(
        unio_runtime::reserve::RESERVE_BYTES + unio_runtime::externs::process::IN_FLIGHT / 2,
    ));
    let outcome = call(
        &mut rt,
        "std.process.run",
        &mut [Value::text("true"), Value::list(vec![]), Value::Int(30)],
    );
    assert!(
        matches!(
            outcome,
            Err(Unwind::Exhausted(unio_runtime::Exhaustion::Heap))
        ),
        "{outcome:?}"
    );
}

/// Whether python3 can run here; the tests of leaving the process group need `os.setsid`.
fn python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A descendant that calls `setsid` to leave the process group of the child is still stopped at
/// the deadline (docs/runtime.md 9.5). The grandchild writes its number first and then tries to
/// leave; on Linux the attempt fails, on macOS the runtime finds it through its parent.
#[test]
fn a_descendant_that_calls_setsid_is_stopped_at_the_timeout() {
    if !python() {
        eprintln!("skipped: python3 is not available");
        return;
    }
    let dir = std::env::temp_dir().join(format!("unio-process-setsid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pid");
    let script = "import os, sys, time
if os.fork() == 0:
    try:
        os.setsid()
    except OSError:
        pass
    with open(sys.argv[1] + '.tmp', 'w') as f:
        f.write(str(os.getpid()))
    os.rename(sys.argv[1] + '.tmp', sys.argv[1])
    time.sleep(60)
    os._exit(0)
time.sleep(60)
";
    let path = pid_file.to_string_lossy().into_owned();
    assert_eq!(kind(run("python3", &["-c", script, &path], 2)), "TimedOut");
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(
        gone(&pid),
        "the grandchild {pid} left the group and still runs"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A double fork into a new session: the grandchild exits at once, so the great-grandchild is
/// orphaned before it calls `setsid`. Linux denies `setsid` and `setpgid` to every descendant
/// of a child, so it stays in the group and is stopped with it. On macOS such an orphan is
/// found only by the marker of the Seatbelt profile of a program (`sandbox/tree.rs`), and this
/// test calls the runtime without a sandbox; `tests/sandbox.rs` of the tool checks it there
/// (`a_double_fork_into_a_new_session_is_stopped_with_the_tree`).
#[cfg(target_os = "linux")]
#[test]
fn a_double_fork_into_a_new_session_does_not_escape_the_tree() {
    if !python() {
        eprintln!("skipped: python3 is not available");
        return;
    }
    let dir = std::env::temp_dir().join(format!("unio-process-daemon-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pid");
    let script = "import os, sys, time
if os.fork() == 0:
    if os.fork() != 0:
        os._exit(0)
    left = 'left'
    try:
        os.setsid()
    except OSError:
        left = 'stayed'
    try:
        os.setpgid(0, 0)
    except OSError:
        pass
    with open(sys.argv[1] + '.tmp', 'w') as f:
        f.write(str(os.getpid()) + ' ' + left)
    os.rename(sys.argv[1] + '.tmp', sys.argv[1])
    time.sleep(60)
    os._exit(0)
time.sleep(60)
";
    let path = pid_file.to_string_lossy().into_owned();
    assert_eq!(kind(run("python3", &["-c", script, &path], 2)), "TimedOut");
    let record = std::fs::read_to_string(&pid_file).unwrap();
    let (pid, left) = record.split_once(' ').unwrap();
    assert_eq!(left, "stayed", "setsid is not denied to a descendant");
    assert!(gone(pid), "the orphaned descendant {pid} still runs");
    let _ = std::fs::remove_dir_all(&dir);
}
