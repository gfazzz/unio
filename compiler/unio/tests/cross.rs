//! `unio build --target` (table 6.1, R-8; docs/runtime.md 8.4): the four targets by their
//! triples, the pairs of machines that build for each other, and a clear refusal, with no file
//! written, for every pair that cannot.
//!
//! Which pairs work: every machine builds for itself; Linux on x86-64 and Linux on AArch64
//! build for each other with the GNU cross C compiler of the other target and the runtime
//! library built for it. An executable for macOS needs Apple's linker and SDK, and one for
//! Windows needs link.exe with the libraries of MSVC and the Windows SDK, so they are built on
//! their own systems only; Linux is built for from Linux only.
//!
//! The test that links and runs for the other Linux target needs the cross C compiler, the
//! runtime library of that target, and an emulator of it (qemu-user); CI installs them in the
//! steps that set `UNIO_TEST_CROSS=1`, and without that variable the test notes that it did not
//! run.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TRIPLES: [&str; 4] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

const PROGRAM: &str = "\
import std.args
from std.text import join

public function main():
    let words = args.list()
    print(\"{words.length()} arguments: {join(words, \" \")}\")
    var total = 0
    for i in range(0, 1000):
        total = total + i * i
    print(\"total {total}\")
";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn all(output: &Output) -> String {
    text(&output.stdout) + &text(&output.stderr)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-cross-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("unio.toml"),
        "[package]\nname = \"cross\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(dir.join("hello.unio"), PROGRAM).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn unio(dir: &Path, args: &[&str], envs: &[(String, String)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().unwrap()
}

/// The triple of the machine these tests run on, when unio builds for it.
fn host() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(TRIPLES[0])
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(TRIPLES[1])
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(TRIPLES[2])
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some(TRIPLES[3])
    } else {
        None
    }
}

fn linux(triple: &str) -> bool {
    triple.ends_with("-linux-gnu")
}

/// Whether this machine builds for the target: itself, and Linux for Linux.
fn builds_for(host: &str, target: &str) -> bool {
    host == target || (linux(host) && linux(target))
}

/// The other Linux target of a Linux host.
fn other_linux(host: &str) -> Option<&'static str> {
    match host {
        "x86_64-unknown-linux-gnu" => Some(TRIPLES[1]),
        "aarch64-unknown-linux-gnu" => Some(TRIPLES[0]),
        _ => None,
    }
}

fn variable_part(triple: &str) -> String {
    triple.replace('-', "_")
}

#[test]
fn an_unknown_target_is_a_usage_error_naming_the_four() {
    let dir = scratch("unknown");
    for args in [
        vec!["build", "--target", "sparc-sun-solaris", "hello.unio"],
        vec!["build", "--target=x86_64-linux", "hello.unio"],
        vec!["build", "hello.unio", "--target"],
    ] {
        let output = unio(&dir, &args, &[]);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", all(&output));
    }
    let output = unio(&dir, &["build", "--target", "mips", "hello.unio"], &[]);
    for triple in TRIPLES {
        assert!(text(&output.stderr).contains(triple), "{}", all(&output));
    }
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    let _ = fs::remove_dir_all(&dir);
}

/// Every pair that cannot be built for stops before compiling with a message that names the
/// target and why, and writes no file; `--print-toolchain` says the same.
#[test]
fn a_pair_that_cannot_be_built_is_refused_with_a_reason_and_no_file() {
    let Some(host) = host() else {
        common::missing_prerequisite("a machine for which unio builds executables");
        return;
    };
    let dir = scratch("refused");
    for target in TRIPLES.into_iter().filter(|t| !builds_for(host, t)) {
        let out = dir.join(format!("out-{target}"));
        let output = unio(
            &dir,
            &[
                "build",
                "--target",
                target,
                "hello.unio",
                "-o",
                &out.to_string_lossy(),
            ],
            &[],
        );
        let err = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{target}: {}", all(&output));
        assert!(err.contains("cannot build for"), "{target}: {err}");
        assert!(err.contains(target), "{target}: {err}");
        let why = if target.contains("apple") {
            "macOS SDK"
        } else if target.contains("windows") {
            "Windows SDK"
        } else {
            "C library"
        };
        assert!(err.contains(why), "{target}: {err}");
        assert!(!out.exists(), "{target}: a file was written");
        let output = unio(
            &dir,
            &["build", "--print-toolchain", "--target", target],
            &[],
        );
        assert_eq!(output.status.code(), Some(1), "{target}: {}", all(&output));
        assert!(
            text(&output.stderr).contains("cannot build for"),
            "{target}: {}",
            all(&output)
        );
    }
    let left: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// The runtime library of the host, which `cargo test` builds with the crate.
fn ensure_runtime_library() {
    let exe = Path::new(env!("CARGO_BIN_EXE_unio"));
    let name = if cfg!(windows) {
        "unio.lib"
    } else {
        "libunio.a"
    };
    if !exe.with_file_name(name).is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["build", "-p", "unio", "--lib"]);
        if exe.parent().and_then(|d| d.file_name()) == Some("release".as_ref()) {
            command.arg("--release");
        }
        assert!(command.status().unwrap().success());
    }
}

/// `--target` with the host's own triple builds what `unio build` without it builds, and the
/// toolchain it prints is the host's.
#[test]
fn the_host_target_builds_the_same_executable_as_no_target() {
    let Some(host) = host() else {
        common::missing_prerequisite("a machine for which unio builds executables");
        return;
    };
    ensure_runtime_library();
    let dir = scratch("host");
    let plain = unio(&dir, &["build", "--print-toolchain"], &[]);
    if plain.status.code() != Some(0) {
        common::missing_prerequisite(&format!("the linker of this machine: {}", all(&plain)));
        let _ = fs::remove_dir_all(&dir);
        return;
    }
    let named = unio(&dir, &["build", "--print-toolchain", "--target", host], &[]);
    assert_eq!(named.status.code(), Some(0), "{}", all(&named));
    assert_eq!(text(&named.stdout), text(&plain.stdout));
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let a = dir.join(format!("a{suffix}"));
    let b = dir.join(format!("b{suffix}"));
    let output = unio(
        &dir,
        &["build", "hello.unio", "-o", &a.to_string_lossy()],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    let output = unio(
        &dir,
        &[
            "build",
            "--target",
            host,
            "hello.unio",
            "-o",
            &b.to_string_lossy(),
        ],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    let run = Command::new(&b).arg("x").output().unwrap();
    assert_eq!(run.status.code(), Some(0), "{}", all(&run));
    assert_eq!(text(&run.stdout), "1 arguments: x\ntotal 332833500\n");
    let _ = fs::remove_dir_all(&dir);
}

/// For the other Linux target, a runtime library that is missing or holds code of another
/// machine stops the build with a message that says where the library belongs and how to get
/// it; no file is written. The cross C compiler is replaced by the host's `cc`, which is only
/// asked for its version before the runtime library is looked for.
#[test]
fn a_missing_or_foreign_runtime_library_of_the_target_is_named() {
    let Some(host) = host() else {
        common::missing_prerequisite("a machine for which unio builds executables");
        return;
    };
    let Some(target) = other_linux(host) else {
        // On macOS and Windows no other target is built for.
        return;
    };
    ensure_runtime_library();
    let dir = scratch("runtime");
    let cc = (
        format!("CC_{}", variable_part(target)),
        std::env::var("CC").unwrap_or_else(|_| "cc".into()),
    );
    let runtime_variable = format!(
        "UNIO_RUNTIME_LIB_{}",
        variable_part(target).to_ascii_uppercase()
    );
    let out = dir.join("out");
    let build = |envs: &[(String, String)]| {
        unio(
            &dir,
            &[
                "build",
                "--target",
                target,
                "hello.unio",
                "-o",
                &out.to_string_lossy(),
            ],
            envs,
        )
    };
    let missing = dir.join("no-such-libunio.a");
    let output = build(&[
        cc.clone(),
        (runtime_variable.clone(), missing.to_string_lossy().into()),
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(
        text(&output.stderr).contains(&runtime_variable)
            && text(&output.stderr).contains("missing"),
        "{}",
        all(&output)
    );
    let host_runtime = Path::new(env!("CARGO_BIN_EXE_unio")).with_file_name("libunio.a");
    let output = build(&[
        cc.clone(),
        (
            runtime_variable.clone(),
            host_runtime.to_string_lossy().into(),
        ),
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("holds code for")
            && text(&output.stderr).contains("runtime of another target"),
        "{}",
        all(&output)
    );
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&dir);
}

/// The ELF machine of an executable.
fn elf_machine(path: &Path) -> u16 {
    let bytes = fs::read(path).unwrap();
    assert!(
        bytes.starts_with(b"\x7fELF"),
        "{} is not ELF",
        path.display()
    );
    u16::from_le_bytes([bytes[18], bytes[19]])
}

fn found(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Linux on x86-64 builds for Linux on AArch64 and the other way round: the executable is for
/// the other machine, records the cross toolchain, and is started by the emulator of that
/// machine. Two builds are the same bytes (C-4).
#[test]
fn linux_builds_for_the_other_linux_target_and_the_executable_runs() {
    let Some(target) = host().and_then(other_linux) else {
        return;
    };
    if std::env::var_os("UNIO_TEST_CROSS").is_none_or(|v| v != "1") {
        eprintln!(
            "skipped: building for {target} needs its cross C compiler, runtime library, and emulator; set UNIO_TEST_CROSS=1 where they are installed (the CI steps of unio build --target)"
        );
        return;
    }
    let (gcc, emulator, machine) = if target.starts_with("aarch64") {
        ("aarch64-linux-gnu-gcc", "qemu-aarch64", 183)
    } else {
        ("x86_64-linux-gnu-gcc", "qemu-x86_64", 62)
    };
    let cc = std::env::var(format!("CC_{}", variable_part(target))).unwrap_or(gcc.into());
    assert!(found(&cc), "the cross C compiler {cc} is missing");
    assert!(found(emulator), "the emulator {emulator} is missing");
    let dir = scratch("other-linux");
    let toolchain = unio(
        &dir,
        &["build", "--print-toolchain", "--target", target],
        &[],
    );
    assert_eq!(toolchain.status.code(), Some(0), "{}", all(&toolchain));
    let identity = text(&toolchain.stdout);
    assert!(
        identity.starts_with(&format!("{target}; ")) && identity.contains(gcc),
        "{identity}"
    );
    let a = dir.join("a");
    let b = dir.join("b");
    for out in [&a, &b] {
        let output = unio(
            &dir,
            &[
                "build",
                "--release",
                "--target",
                target,
                "hello.unio",
                "-o",
                &out.to_string_lossy(),
            ],
            &[],
        );
        assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    }
    assert_eq!(elf_machine(&a), machine);
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    let expected = unio(&dir, &["run", "hello.unio", "--", "one", "two"], &[]);
    assert_eq!(expected.status.code(), Some(0), "{}", all(&expected));
    // Run by the emulator, the runtime of the target starts and accepts the image of the
    // program (an image it refuses ends with 104). qemu-user does not pass the system calls of
    // Landlock and seccomp through, so the sandbox cannot be applied there and the executable
    // stops before `main` with 103 and the names of the missing mechanisms, as SB-5 asks; an
    // emulator that passes them runs the program, which then prints what `unio run` prints.
    // The sandbox of the target is checked natively by the native suite on that machine.
    let run = Command::new(emulator)
        .arg(&a)
        .args(["one", "two"])
        .current_dir(&dir)
        .env_remove("UNIO_POLICY")
        .output()
        .unwrap();
    match run.status.code() {
        Some(0) => assert_eq!(text(&run.stdout), text(&expected.stdout)),
        Some(103) => {
            let err = text(&run.stderr);
            assert!(
                err.contains("the sandbox cannot be applied")
                    && err.contains("Landlock")
                    && err.contains("seccomp"),
                "{}",
                all(&run)
            );
            assert!(text(&run.stdout).is_empty(), "{}", all(&run));
        }
        _ => panic!("the executable under {emulator}: {}", all(&run)),
    }
    let _ = fs::remove_dir_all(&dir);
}
