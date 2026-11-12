//! Installation without a source checkout (plan section 12, release condition 1): a copy of
//! `unio` laid out as an installation (`<prefix>/bin/unio`, `<prefix>/lib/unio/libunio.a`,
//! docs/runtime.md 8.4) checks, formats, runs, tests, and builds a program written in an empty
//! directory outside the repository, with no `UNIO_STDLIB` and no other variable of Unio or
//! Cargo, and the executable it builds runs. The standard library it uses is the one embedded in
//! the compiler (RFC 0004: trusted by origin), never a `stdlib` directory of the checkout or of
//! the project.
//!
//! CI repeats this on a machine with no checkout at all: the step "Installed unio works without
//! the source tree" of the job `install` (.github/workflows/ci.yml).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PROGRAM: &str = "\
## The program of the installation check: it greets with a sum.

import std.args

function total(values as List[Int]) returns Int:
    var sum = 0
    for value in values:
        sum = sum + value
    return sum

public function main() returns Int:
    print(\"hello {total([1, 2, 3])} {args.list().length()}\")
    return 0

test \"the total of three numbers\":
    assert total([1, 2, 3]) == 6
";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Whether `unio build` makes executables on this machine (as in tests/native.rs).
fn builds_here() -> bool {
    cfg!(any(
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(target_os = "macos", target_arch = "aarch64"),
        all(windows, target_env = "msvc", target_arch = "x86_64"),
    ))
}

/// The name of the runtime library on this system.
fn runtime_name() -> &'static str {
    if cfg!(windows) {
        "unio.lib"
    } else {
        "libunio.a"
    }
}

/// The runtime library of this compiler, brought up to date with the profile of the tests, as
/// tests/native.rs does: `cargo test` builds only the rlib of the crate.
fn runtime_library() -> PathBuf {
    let exe = Path::new(env!("CARGO_BIN_EXE_unio"));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["build", "-p", "unio", "--lib"]);
    let profile = exe.parent().unwrap();
    if profile.file_name() == Some("release".as_ref()) {
        command.arg("--release");
    }
    let above = profile.parent().unwrap();
    let name = above.file_name().unwrap().to_string_lossy();
    if name.matches('-').count() >= 2 && above.parent().unwrap().join("CACHEDIR.TAG").exists() {
        command.arg("--target").arg(&*name);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "building the runtime library failed:\n{}",
        text(&output.stderr)
    );
    profile.join(runtime_name())
}

/// A command of the installed compiler in `dir`, with the environment of a user who has no
/// checkout: no variable of Unio, Cargo, or Rust. On Windows the operator allows the weaker
/// sandbox (SB-5, SB-10), as CI does, through a policy file outside the repository.
fn installed(unio: &Path, dir: &Path, policy: &Path) -> Command {
    let mut command = Command::new(unio);
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy().to_ascii_uppercase();
        if key.starts_with("UNIO_") || key.starts_with("CARGO") || key.starts_with("RUST") {
            command.env_remove(&key);
        }
    }
    if cfg!(windows) {
        command.env("UNIO_POLICY", policy);
    }
    command.current_dir(dir);
    command
}

#[track_caller]
fn succeeds(output: &Output, what: &str) -> String {
    let out = text(&output.stdout);
    assert!(
        output.status.success(),
        "{what}: {:?}\n{out}\n{}",
        output.status.code(),
        text(&output.stderr)
    );
    out
}

#[test]
fn an_installed_unio_works_without_the_source_tree() {
    let root =
        std::env::temp_dir().join(format!("unio-install-{}-no-checkout", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let bin = root.join("prefix/bin");
    let lib = root.join("prefix/lib/unio");
    let work = root.join("work");
    for dir in [&bin, &lib, &work] {
        fs::create_dir_all(dir).unwrap();
    }
    let unio = bin.join(format!("unio{}", std::env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_unio"), &unio).unwrap();
    if builds_here() {
        fs::copy(runtime_library(), lib.join(runtime_name())).unwrap();
    }
    let policy = root.join("policy.toml");
    fs::write(&policy, "allow_weaker = true\n").unwrap();
    // A `stdlib` directory in the project is never the trusted library (RFC 0004).
    fs::create_dir_all(work.join("stdlib/core")).unwrap();
    fs::write(work.join("stdlib/core/prelude.unio"), "this is not Unio\n").unwrap();
    fs::write(work.join("hello.unio"), PROGRAM).unwrap();
    let run = |args: &[&str]| {
        installed(&unio, &work, &policy)
            .args(args)
            .output()
            .unwrap()
    };

    succeeds(&run(&["check", "hello.unio"]), "unio check");
    succeeds(&run(&["fmt", "--check", "hello.unio"]), "unio fmt --check");
    let out = succeeds(&run(&["run", "hello.unio", "--", "a", "b"]), "unio run");
    assert_eq!(out, "hello 6 2\n");
    let out = succeeds(&run(&["test", "hello.unio"]), "unio test");
    assert!(out.contains("the total of three numbers"), "{out}");
    // The library is the one embedded in the compiler, not a directory of the machine.
    let out = succeeds(&run(&["symbol", "std.args.list"]), "unio symbol");
    let embedded = Path::new("<unio>/stdlib").join("std").join("args.unio");
    assert!(
        out.contains(&format!("declared: {}:", embedded.display())),
        "{out}"
    );

    if builds_here() {
        let exe = work.join(format!("hello{}", std::env::consts::EXE_SUFFIX));
        succeeds(
            &run(&[
                "build",
                "--release",
                "hello.unio",
                "-o",
                exe.to_str().unwrap(),
            ]),
            "unio build",
        );
        let output = installed(&exe, &work, &policy)
            .args(["x"])
            .output()
            .unwrap();
        assert_eq!(succeeds(&output, "the executable"), "hello 6 1\n");
    } else {
        eprintln!("skipped unio build: no native executables are made for this machine");
    }
    let _ = fs::remove_dir_all(&root);
}
