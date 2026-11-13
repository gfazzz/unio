//! S-12: the `core` runtime makes no call to the operating system (docs/runtime.md 11).
//!
//! A program that passes `unio check --profile=core` is built with `unio build` and run with
//! the test hook `UNIO_TEST_CORE_SECCOMP=1`: right before `main` the executable installs, on
//! every thread, a seccomp filter that ends the process with SIGSYS at any system call except a
//! write to standard output or error, the memory calls of the allocator, a futex, the calls
//! that end a thread, and exit. The core programs end normally, with their exit code, and with
//! a fault and its trace on standard error; a program of the `std` layer that touches a file is
//! ended by the filter, which shows that the filter is in force. The static half of the check,
//! for every platform, is `the_core_layer_reaches_the_system_only_through_hooks`
//! (`compiler/unio-runtime/tests/bindings.rs`). Linux on x86-64 and AArch64 only: seccomp
//! exists only there.

#![cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

mod common;

use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SIGSYS: i32 = 31;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `unio build` links with `libunio.a` next to the compiler, which `cargo test` does not build;
/// as in `tests/native.rs`, it is brought up to date once with the profile of the tests.
fn runtime_library() {
    static BUILT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        let exe = Path::new(env!("CARGO_BIN_EXE_unio"));
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["build", "-p", "unio", "--lib"]);
        if exe.parent().unwrap().file_name() == Some("release".as_ref()) {
            command.arg("--release");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "building libunio.a failed:\n{}",
            text(&output.stderr)
        );
    });
}

fn package(name: &str, manifest: &str, program: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-layers-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("unio.toml"), manifest).unwrap();
    fs::write(dir.join("main.unio"), program).unwrap();
    dir
}

fn unio(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command.current_dir(dir).env_remove("UNIO_POLICY");
    command
}

/// Builds `main.unio` of the package and runs it under the filter of the core layer.
fn build_and_run_core_only(dir: &Path) -> Output {
    runtime_library();
    let exe = dir.join("main-bin");
    let output = unio(dir)
        .args(["build", "main.unio", "-o"])
        .arg(&exe)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    Command::new(&exe)
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .env("UNIO_TEST_CORE_SECCOMP", "1")
        .output()
        .unwrap()
}

#[track_caller]
fn assert_core(dir: &Path) {
    let output = unio(dir)
        .args(["check", "--profile=core", "main.unio"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "not a core program: {}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

const MANIFEST: &str = "[package]\nname = \"layers\"\nversion = \"0.1.0\"\n\n[permissions]\n";

const CORE_PROGRAM: &str = "function fibonacci(n as Int) returns Int:
    if n < 2: return n
    return fibonacci(n - 1) + fibonacci(n - 2)

function largest(values as Range) returns Int:
    var best = 0
    for value in values:
        best = max(best, value % 7)
    return best

public function main() returns Int:
    let n = fibonacci(20)
    assert n.checked_multiply(MAXIMUM_INT) == none
    assert (n.checked_add(1) or 0) == 6766
    assert 7.0.total_order() > 2.0.total_order()
    return n % 100 + largest(range(0, 1000)) - 6
";

const FAULTING_PROGRAM: &str = "function grow(n as Int) returns Int:
    return n * 2 + grow(n + 1) if n < 70 else n * MAXIMUM_INT

public function main():
    let total = grow(1)
    assert total > 0
";

/// A core program runs to its end under the filter: entry, calls, checked arithmetic and the
/// exit code make no system call.
#[test]
fn a_core_program_makes_no_system_call() {
    let dir = package("core", MANIFEST, CORE_PROGRAM);
    assert_core(&dir);
    let output = build_and_run_core_only(&dir);
    assert_eq!(
        output.status.code(),
        Some(65),
        "status {:?}, signal {:?}: {}",
        output.status,
        output.status.signal(),
        text(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A fault of a core program is reported through the hook of standard error, with its trace,
/// and ends the program with code 101 under the filter.
#[test]
fn a_fault_of_a_core_program_is_reported_without_other_system_calls() {
    let dir = package("fault", MANIFEST, FAULTING_PROGRAM);
    assert_core(&dir);
    let output = build_and_run_core_only(&dir);
    let err = text(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(101),
        "{:?}: {err}",
        output.status
    );
    assert!(err.contains("fault: "), "{err}");
    assert!(err.contains("at main.grow (main.unio:2)"), "{err}");
}

/// The control: a program of the `std` layer that asks the file system is ended by the filter
/// with SIGSYS, so the filter is in force when the core programs run.
#[test]
fn the_filter_ends_a_program_that_calls_the_system() {
    let manifest = format!("{MANIFEST}files.read = [\"$CWD/**\"]\n");
    let program = "import std.files

public function main():
    print(\"before\")
    let found = files.exists(Path(\"unio.toml\"))
    print(\"{found}\")
";
    let dir = package("control", &manifest, program);
    let output = build_and_run_core_only(&dir);
    assert_eq!(
        output.status.signal(),
        Some(SIGSYS),
        "status {:?}: {}{}",
        output.status,
        text(&output.stdout),
        text(&output.stderr)
    );
    // Standard output is allowed: what was printed before the call is there.
    assert!(text(&output.stdout).starts_with("before"), "{output:?}");
    let _ = fs::remove_dir_all(&dir);
}
