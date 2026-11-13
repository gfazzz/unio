//! C-20: what holds for resources, standard output, and files at each kind of termination
//! (docs/runtime.md section 5.4). One program prints a line, writes `open/done.txt` with
//! `write_text`, opens a resource and an atomic file of `open/target.txt`, writes to the atomic
//! file, and then ends in one of the kinds of termination: normally, with a fault (101), with
//! exhaustion of the stack or of the heap (102), with a refusal of the policy (103), killed, or
//! at the wall time limit of the policy. Each test checks the exit code, standard output (the
//! lines printed before the end, and the line of `release` where it runs), and the files: the
//! complete content of `done.txt`, the target either old or new and complete, and the temporary
//! file of the atomic file removed by `release` or left behind where `release` does not run.
//! The refusal of an executable's image (104) is checked in `tests/native.rs`
//! (`an_incompatible_image_is_refused`).

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The program; `END` is replaced by the statements that end it inside the `using` blocks.
const PROGRAM: &str = "function recurse(n as Int) returns Int:
    return recurse(n + 1) + 1

function grow() returns Int:
    var items as List[Text] = []
    while true:
        items.append(\"item {items.length()}\")
    return items.length()

public function main() returns Int:
    try:
        try files.write_text(Path(\"open/done.txt\"), \"complete\")
        print(\"before\")
        using first = Log(name = \"first\"):
            using file = try files.open_atomic(Path(\"open/target.txt\")):
                try file.write(\"new content\")
                END
    on IoError as e:
        print(\"error {e.reason}\")
    return 0
";

/// The line that the `release` of `first` prints (`Log` of [`common::RESERVE_PRELUDE`]).
const RELEASED: &str = "released first: 2000 words, last first 1999\n";

/// A package of the program that ends with `end`, run with the policy `policy`.
fn package(name: &str, end: &str, policy: &str) -> PathBuf {
    let body = PROGRAM.replace("END", end);
    let dir = common::reserve_package("unio-termination", name, &body);
    let source = fs::read_to_string(dir.join("main.unio")).unwrap();
    fs::write(dir.join("main.unio"), format!("import std.time\n{source}")).unwrap();
    fs::write(dir.join("policy.toml"), policy).unwrap();
    dir
}

fn command(dir: &Path) -> Command {
    let mut unio = Command::new(env!("CARGO_BIN_EXE_unio"));
    unio.current_dir(dir)
        .env_remove("UNIO_POLICY")
        .env_remove("UNIO_TEST_HEAP_FREE")
        .args(["run", "--policy", "policy.toml", "main.unio"]);
    unio
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// The content of `open/done.txt`, written completely before the end.
#[track_caller]
fn assert_done(dir: &Path) {
    assert_eq!(common::read(&dir.join("open/done.txt")), "complete");
}

/// The target is unchanged and the temporary file of the atomic file is gone: `release` ran.
#[track_caller]
fn assert_released(dir: &Path) {
    common::assert_target_unchanged(dir);
    assert_eq!(temporaries(dir), Vec::<String>::new());
}

/// The target is unchanged and one temporary file is left behind: `release` did not run.
#[track_caller]
fn assert_not_released(dir: &Path) {
    common::assert_target_unchanged(dir);
    let left = temporaries(dir);
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].starts_with(".target.txt.unio-tmp-"), "{left:?}");
    let content = common::read(&dir.join("open").join(&left[0]));
    // Written straight to the temporary file, so complete here; S-9 allows it to stay.
    assert_eq!(content, "new content");
}

/// The temporary files of atomic files left in `open/`.
fn temporaries(dir: &Path) -> Vec<String> {
    common::temporaries(dir)
        .into_iter()
        .filter(|name| name.contains(".unio-tmp-"))
        .collect()
}

fn run(dir: &Path) -> Output {
    command(dir).output().unwrap()
}

/// Normal end: the blocks end, so `release` has run before `main` returns; every line printed
/// is on standard output; the committed atomic file replaced the target completely.
#[test]
fn normal_end_releases_resources_and_writes_everything() {
    let dir = package(
        "normal",
        "try file.commit()\n                print(\"committed\")",
        "",
    );
    let output = run(&dir);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        format!("before\ncommitted\n{RELEASED}")
    );
    assert_eq!(text(&output.stderr), "");
    assert_done(&dir);
    assert_eq!(common::read(&dir.join("open/target.txt")), "new content");
    assert_eq!(temporaries(&dir), Vec::<String>::new());
    let _ = fs::remove_dir_all(&dir);
}

/// A fault (101): the lines printed before it stay on standard output, `release` runs and its
/// line follows, the atomic file without a commit removes its temporary file and leaves the
/// target unchanged (C-17).
#[test]
fn a_fault_releases_resources_and_keeps_the_output() {
    let dir = package("fault", "return divide(1, 0)", "");
    let output = run(&dir);
    assert_eq!(output.status.code(), Some(101), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), format!("before\n{RELEASED}"));
    assert!(
        text(&output.stderr).starts_with("fault: division by zero\n"),
        "{}",
        text(&output.stderr)
    );
    assert_done(&dir);
    assert_released(&dir);
    let _ = fs::remove_dir_all(&dir);
}

/// Exhaustion of the stack (102): the lines printed before it stay on standard output, no
/// `release` runs, and the temporary file is left with the target unchanged (C-18).
#[test]
fn exhaustion_of_the_stack_keeps_the_output_and_runs_no_release() {
    let dir = package("stack", "return recurse(0)", "");
    let output = run(&dir);
    assert_eq!(output.status.code(), Some(102), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "before\n");
    assert_eq!(
        text(&output.stderr),
        "fault: stack exhausted by recursion; the program ends with code 102\n"
    );
    assert_done(&dir);
    assert_not_released(&dir);
    let _ = fs::remove_dir_all(&dir);
}

/// Exhaustion of the heap (102), at the heap limit of the policy: the same as for the stack.
#[test]
fn exhaustion_of_the_heap_keeps_the_output_and_runs_no_release() {
    let dir = package(
        "heap",
        "return grow()",
        "[limits]\nheap_bytes = 8_388_608\n",
    );
    let output = run(&dir);
    assert_eq!(output.status.code(), Some(102), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "before\n");
    assert_eq!(
        text(&output.stderr),
        "fault: memory exhausted; the program ends with code 102\n"
    );
    assert_done(&dir);
    assert_not_released(&dir);
    let _ = fs::remove_dir_all(&dir);
}

/// The wall time limit of the policy (102): the watchdog ends the process; the lines printed
/// before stay on standard output, no `release` runs, and the temporary file is left.
#[test]
fn the_wall_time_limit_keeps_the_output_and_runs_no_release() {
    let dir = package(
        "timeout",
        "print(\"waiting\")\n                time.sleep(time.seconds(60))",
        "[limits]\nwall_seconds = 1\n",
    );
    let output = run(&dir);
    assert_eq!(output.status.code(), Some(102), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "before\nwaiting\n");
    assert_eq!(
        text(&output.stderr),
        "fault: the wall time limit of the policy (1 s) is used up; the program ends with code 102\n"
    );
    assert_done(&dir);
    assert_not_released(&dir);
    let _ = fs::remove_dir_all(&dir);
}

/// A policy that cannot be applied (103) stops the start: nothing of the program runs, so
/// nothing is printed and no file is touched.
#[test]
fn a_refused_policy_runs_nothing() {
    let dir = package("policy", "return 0", "[limits]\nunknown_limit = 1\n");
    let output = run(&dir);
    assert_eq!(output.status.code(), Some(103), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "");
    assert!(!dir.join("open/done.txt").exists());
    assert_released(&dir);
    let _ = fs::remove_dir_all(&dir);
}

/// Killed by a signal (on Windows, terminated): the runtime gives no guarantee (C-19), but each
/// `print` has reached the system before it returned, so the lines printed before stay; no
/// `release` runs, the temporary file is left, and the target stays old (S-9).
#[test]
fn a_killed_program_keeps_its_printed_lines_and_runs_no_release() {
    let dir = package(
        "killed",
        "print(\"waiting\")\n                time.sleep(time.seconds(60))",
        "",
    );
    let mut child = command(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    assert_eq!(lines.next().unwrap().unwrap(), "before");
    assert_eq!(lines.next().unwrap().unwrap(), "waiting");
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9), "{status:?}");
    }
    assert!(lines.next().is_none(), "nothing after the kill");
    assert_done(&dir);
    // The writer process of the sandbox may still hold the temporary file for a moment.
    let mut tries = 0;
    while temporaries(&dir).len() != 1 && tries < 100 {
        tries += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_not_released(&dir);
    let _ = fs::remove_dir_all(&dir);
}
