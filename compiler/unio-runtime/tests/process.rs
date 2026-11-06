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
