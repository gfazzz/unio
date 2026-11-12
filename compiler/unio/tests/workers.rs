//! The hostile projects of SB-15 for the workers of the tools (SB-10, SB-11; docs/runtime.md
//! 9.7): projects that try to read files outside the package, to write outside the temporary
//! directory of the worker, to use the network, to start a process, to read the environment of
//! the tool, or to exhaust its memory or time.
//!
//! The compiler already refuses some of these by its own logic (E0053 for an import through a
//! link). The rules of the operating system are shown with the probes of `UNIO_WORKER_PROBE`:
//! the worker tries each operation with plain system calls after its sandbox applies, past the
//! compiler, and reports whether it was allowed.
//!
//! On Windows the worker has a Job Object only (memory, one process); the file system and the
//! network are not restricted there yet, so only the limits, the environment, and processes
//! are tested there.

// The helpers of the file-system tests are not used on Windows.
#![cfg_attr(windows, allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const SECRET: &str = "SECRET-OF-THE-OPERATOR";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// A temporary directory with a package `pkg/`, a directory `outside/` next to it, and a home
/// directory `home/` with a key in `.ssh/`, all holding the secret.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let dir = std::env::temp_dir().join(format!("unio-workers-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::create_dir_all(dir.join("outside")).unwrap();
        fs::create_dir_all(dir.join("home/.ssh")).unwrap();
        fs::write(dir.join("pkg/unio.toml"), "[package]\nname = \"hostile\"\n").unwrap();
        fs::write(
            dir.join("pkg/main.unio"),
            "public function main():\n    print(\"hello\")\n",
        )
        .unwrap();
        fs::write(
            dir.join("outside/secret.unio"),
            format!("public function leak() returns Text:\n    return \"{SECRET}\"\n"),
        )
        .unwrap();
        fs::write(dir.join("home/.ssh/id_rsa"), SECRET).unwrap();
        Project {
            root: fs::canonicalize(dir).unwrap(),
        }
    }

    fn pkg(&self) -> PathBuf {
        self.root.join("pkg")
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// `unio` in the package, with the home directory of the project and without a policy.
    fn unio(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        self.command(args, envs).output().unwrap()
    }

    fn command(&self, args: &[&str], envs: &[(&str, &str)]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
        command
            .current_dir(self.pkg())
            .env_remove("UNIO_POLICY")
            .env_remove("UNIO_WORKER_PROBE")
            .env("HOME", self.path("home"))
            .args(args);
        if cfg!(windows) {
            // The operator allows the weaker sandbox through the external policy (SB-5, SB-10);
            // the refusal without it is tested on its own below.
            let policy = self.path("weaker.toml");
            fs::write(&policy, "allow_weaker = true\n").unwrap();
            command.env("UNIO_POLICY", policy);
        }
        for (name, value) in envs {
            command.env(name, value);
        }
        command
    }

    /// `unio check main.unio` with the probes.
    fn probe(&self, probes: &[String]) -> Output {
        self.unio(
            &["check", "main.unio"],
            &[("UNIO_WORKER_PROBE", &probes.join(";"))],
        )
    }

    fn policy(&self, body: &str) -> PathBuf {
        let path = self.path("policy.toml");
        // On Windows the operator must allow the weaker sandbox (SB-5, SB-10).
        let weaker = if cfg!(windows) {
            "allow_weaker = true\n"
        } else {
            ""
        };
        fs::write(&path, format!("{weaker}{body}")).unwrap();
        path
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn all(output: &Output) -> String {
    text(&output.stdout) + &text(&output.stderr)
}

/// The result the worker reported for a probe: `allowed ...` or `denied (...)`.
#[track_caller]
fn probe_result(output: &Output, operation: &str) -> String {
    let prefix = format!("unio worker probe: {operation}: ");
    let err = text(&output.stderr);
    match err.lines().find_map(|l| l.strip_prefix(prefix.as_str())) {
        Some(result) => result.to_string(),
        None => panic!("no result for the probe {operation}:\n{}", all(output)),
    }
}

#[track_caller]
fn assert_denied(output: &Output, operations: &[String]) {
    for operation in operations {
        let result = probe_result(output, operation);
        assert!(
            result.starts_with("denied"),
            "{operation} was {result}:\n{}",
            all(output)
        );
    }
    assert!(
        !all(output).contains(SECRET),
        "the secret leaked:\n{}",
        all(output)
    );
}

#[track_caller]
fn assert_allowed(output: &Output, operations: &[String]) {
    for operation in operations {
        let result = probe_result(output, operation);
        assert!(
            result.starts_with("allowed"),
            "{operation} was {result}:\n{}",
            all(output)
        );
    }
}

fn op(name: &str, path: &Path) -> String {
    format!("{name}={}", path.display())
}

/// (a) A source that is a link to a file outside the package: the import is refused by the
/// compiler (E0053), and the operating system refuses to read the file through the link, so
/// naming the link directly does not reveal it either.
#[cfg(unix)]
#[test]
fn a_linked_source_outside_the_package_cannot_be_read() {
    let p = Project::new("link");
    std::os::unix::fs::symlink(p.path("outside/secret.unio"), p.pkg().join("lib.unio")).unwrap();
    fs::write(
        p.pkg().join("main.unio"),
        "import lib\n\npublic function main():\n    print(lib.leak())\n",
    )
    .unwrap();
    let output = p.unio(&["check", "main.unio"], &[]);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(text(&output.stdout).contains("E0053"), "{}", all(&output));
    assert!(!all(&output).contains(SECRET), "{}", all(&output));
    let output = p.unio(&["check", "lib.unio"], &[]);
    assert_ne!(output.status.code(), Some(0), "{}", all(&output));
    assert!(!all(&output).contains(SECRET), "{}", all(&output));
    let output = p.unio(&["outline", "lib.unio"], &[]);
    assert!(!all(&output).contains(SECRET), "{}", all(&output));
    let output = p.probe(&[
        op("read", &p.pkg().join("lib.unio")),
        op("read", &p.path("outside/secret.unio")),
        "read=../outside/secret.unio".into(),
        op("list", &p.path("outside")),
        op("read", &p.pkg().join("main.unio")),
    ]);
    assert_denied(
        &output,
        &[
            op("read", &p.pkg().join("lib.unio")),
            op("read", &p.path("outside/secret.unio")),
            "read=../outside/secret.unio".into(),
            op("list", &p.path("outside")),
        ],
    );
    assert_allowed(&output, &[op("read", &p.pkg().join("main.unio"))]);
}

/// (b) A manifest that is a link to a key of the operator, and the key by its path: neither can
/// be read, and nothing of the key reaches the output.
#[cfg(unix)]
#[test]
fn files_of_the_home_directory_cannot_be_read() {
    let p = Project::new("home");
    fs::remove_file(p.pkg().join("unio.toml")).unwrap();
    std::os::unix::fs::symlink(p.path("home/.ssh/id_rsa"), p.pkg().join("unio.toml")).unwrap();
    let output = p.unio(&["check", "main.unio"], &[]);
    assert!(!all(&output).contains(SECRET), "{}", all(&output));
    let key = p.path("home/.ssh/id_rsa");
    let output = p.probe(&[
        op("read", &key),
        op("read", &p.pkg().join("unio.toml")),
        op("list", &p.path("home")),
        "read=/etc/passwd".into(),
    ]);
    assert_denied(
        &output,
        &[
            op("read", &key),
            op("read", &p.pkg().join("unio.toml")),
            op("list", &p.path("home")),
            "read=/etc/passwd".into(),
        ],
    );
}

/// (c) The worker writes nothing but its temporary directory: not the package, not a `target/`
/// that is a link to elsewhere, not the home directory.
#[cfg(unix)]
#[test]
fn the_worker_writes_only_its_temporary_directory() {
    let p = Project::new("write");
    std::os::unix::fs::symlink(p.path("outside"), p.pkg().join("target")).unwrap();
    let denied = vec![
        op("write", &p.pkg().join("new.unio")),
        op("write", &p.pkg().join("main.unio")),
        op("mkdir", &p.pkg().join("build")),
        op("write", &p.pkg().join("target/escaped.txt")),
        op("write", &p.path("outside/escaped.txt")),
        op("write", &p.path("home/.bashrc")),
        op("remove", &p.pkg().join("main.unio")),
    ];
    let mut probes = denied.clone();
    probes.push("write-temp".into());
    let output = p.probe(&probes);
    assert_denied(&output, &denied);
    assert_allowed(&output, &["write-temp".into()]);
    assert!(!p.path("outside/escaped.txt").exists());
    assert!(!p.path("home/.bashrc").exists());
    assert!(p.pkg().join("main.unio").exists());
}

/// (c) `unio fmt` replaces exactly the files it was given, through the writer process; the
/// worker cannot write them itself, nor any other file, and a given link is not followed.
#[cfg(unix)]
#[test]
fn fmt_writes_only_the_given_files_through_the_writer() {
    let p = Project::new("fmt");
    let ugly = "public function main():\n    let x = 1+2\n    print(\"{x}\")\n";
    fs::write(p.pkg().join("a.unio"), ugly).unwrap();
    fs::write(p.pkg().join("b.unio"), ugly).unwrap();
    std::os::unix::fs::symlink(p.path("outside/secret.unio"), p.pkg().join("link.unio")).unwrap();
    let probes = [
        op("write", &p.pkg().join("a.unio")),
        op("write", &p.pkg().join("b.unio")),
    ];
    let output = p.unio(
        &["fmt", "a.unio"],
        &[("UNIO_WORKER_PROBE", &probes.join(";"))],
    );
    assert_denied(&output, &probes);
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    let formatted = fs::read_to_string(p.pkg().join("a.unio")).unwrap();
    assert!(formatted.contains("1 + 2"), "{formatted}");
    assert_eq!(fs::read_to_string(p.pkg().join("b.unio")).unwrap(), ugly);
    // A given link is read through the sandbox, which refuses its target outside the package.
    let before = fs::read_to_string(p.path("outside/secret.unio")).unwrap();
    let output = p.unio(&["fmt", "link.unio"], &[]);
    assert_ne!(output.status.code(), Some(0), "{}", all(&output));
    assert!(!all(&output).contains(SECRET), "{}", all(&output));
    assert_eq!(
        fs::read_to_string(p.path("outside/secret.unio")).unwrap(),
        before
    );
}

/// (d) No network: neither TCP to a listening port of this machine nor a UDP socket.
#[cfg(unix)]
#[test]
fn the_worker_has_no_network() {
    let p = Project::new("net");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let probes = vec![format!("connect={address}"), "udp".to_string()];
    let output = p.probe(&probes);
    assert_denied(&output, &probes);
    assert!(
        listener.accept().is_err(),
        "the worker reached the listener"
    );
}

/// (e) The worker starts no process, by exec or by fork, during `unio check` and the compile
/// steps of `unio test` and `unio run`.
#[cfg(unix)]
#[test]
fn the_worker_cannot_start_a_process() {
    let p = Project::new("exec");
    let marker = p.path("outside/ran");
    let script = p.path("outside/run.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\necho ran > {}\n", marker.display()),
    )
    .unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(
        &mut fs::metadata(&script).unwrap().permissions(),
        0o755,
    );
    fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let probes = vec![
        "exec=/bin/sh".to_string(),
        op("exec", &script),
        "fork".to_string(),
    ];
    let output = p.probe(&probes);
    assert_denied(&output, &probes);
    fs::write(
        p.pkg().join("main.unio"),
        "public function main():\n    print(\"hello\")\n\ntest \"runs\":\n    assert 1 == 1\n",
    )
    .unwrap();
    for command in [&["test", "main.unio"][..], &["run", "main.unio"][..]] {
        let mut args = command.to_vec();
        if cfg!(windows) {
            args.insert(1, "--allow-weaker-sandbox");
        }
        let output = p.unio(&args, &[("UNIO_WORKER_PROBE", &probes.join(";"))]);
        assert_denied(&output, &probes);
        assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    }
    assert!(!marker.exists());
}

/// On Windows the worker cannot restrict files and the network, so the tools refuse with code
/// 103 before they read the project, unless the operator allows a weaker sandbox by the flag or
/// the external policy; a project cannot allow it in its manifest (SB-5, SB-10).
#[cfg(windows)]
#[test]
fn the_tools_refuse_a_weaker_sandbox_on_windows_unless_the_operator_allows_it() {
    let p = Project::new("refuse");
    fs::write(
        p.pkg().join("unio.toml"),
        "[package]\nname = \"refuse\"\nversion = \"0.1.0\"\nallow_weaker = true\n",
    )
    .unwrap();
    for args in [
        &["check", "main.unio"][..],
        &["fmt", "--check", "main.unio"][..],
    ] {
        let mut command = p.command(args, &[("UNIO_WORKER_PROBE", "marker")]);
        command.env_remove("UNIO_POLICY");
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(103), "{}", all(&output));
        assert!(
            text(&output.stderr).contains("--allow-weaker-sandbox"),
            "{}",
            all(&output)
        );
        let mut allowed: Vec<&str> = args.to_vec();
        allowed.insert(1, "--allow-weaker-sandbox");
        let mut command = p.command(&allowed, &[]);
        command.env_remove("UNIO_POLICY");
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", all(&output));
        assert!(
            text(&output.stderr).contains("weaker sandbox"),
            "{}",
            all(&output)
        );
    }
}

/// (e) On Windows the Job Object of the worker allows one process: the worker itself.
#[cfg(windows)]
#[test]
fn the_worker_cannot_start_a_process_on_windows() {
    let p = Project::new("exec");
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let probes = vec![format!("exec={system}\\System32\\cmd.exe")];
    let output = p.probe(&probes);
    assert_denied(&output, &probes);
}

/// (f) A worker that runs longer than the time limit of the policy is stopped, and the tool
/// reports it with code 102 instead of hanging.
#[test]
fn a_worker_over_the_time_limit_is_stopped() {
    let p = Project::new("time");
    let policy = p.policy("[limits]\nwall_seconds = 1\n");
    let started = Instant::now();
    let output = p.unio(
        &["check", "--policy", &policy.to_string_lossy(), "main.unio"],
        &[("UNIO_WORKER_PROBE", "spin")],
    );
    let elapsed = started.elapsed();
    assert_eq!(output.status.code(), Some(102), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("time limit of 1 s"),
        "{}",
        all(&output)
    );
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
}

/// (f) The compile steps of `unio run` and `unio test` are workers under the same time limits
/// of the operator's policy, given by `--policy` or by `UNIO_POLICY`: a compile step that does
/// not end is stopped at the limit of the policy with code 102, not at the default of ten
/// minutes, and the program does not run.
#[test]
fn the_compile_steps_of_run_and_test_keep_the_time_limit_of_the_policy() {
    let p = Project::new("compile-time");
    let policy = p.policy("[limits]\nwall_seconds = 1\n");
    let policy = policy.to_string_lossy().into_owned();
    for command in ["run", "test"] {
        for by_variable in [false, true] {
            let mut args = vec![command];
            if !by_variable {
                args.extend(["--policy", &policy]);
            }
            args.push("main.unio");
            let mut unio = p.command(&args, &[("UNIO_WORKER_PROBE", "spin")]);
            if by_variable {
                unio.env("UNIO_POLICY", &policy);
            }
            let started = Instant::now();
            let output = unio.output().unwrap();
            let elapsed = started.elapsed();
            let case = format!("{command}, policy by variable: {by_variable}");
            assert_eq!(output.status.code(), Some(102), "{case}: {}", all(&output));
            assert!(
                text(&output.stderr).contains("time limit of 1 s"),
                "{case}: {}",
                all(&output)
            );
            assert!(!text(&output.stdout).contains("hello"), "{case}");
            assert!(elapsed < Duration::from_secs(20), "{case}: {elapsed:?}");
        }
    }
    // Without the probe the same policy runs the program.
    let output = p.unio(&["run", "--policy", &policy, "main.unio"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    assert_eq!(text(&output.stdout), "hello\n");
}

/// (f) The memory limit of the policy narrows the compile steps of `unio run` and `unio test`
/// too, down to the floor that compiling needs: above it a compile step that takes more is
/// stopped with code 102; a heap limit below it is the program's alone, so the program runs.
#[test]
fn the_compile_steps_of_run_and_test_keep_the_memory_limit_of_the_policy() {
    let p = Project::new("compile-memory");
    let policy = p.policy("[limits]\nheap_bytes = 536870912\nwall_seconds = 60\n");
    let policy = policy.to_string_lossy().into_owned();
    for command in ["run", "test"] {
        let output = p.unio(
            &[command, "--policy", &policy, "main.unio"],
            &[("UNIO_WORKER_PROBE", "alloc=1073741824")],
        );
        assert_eq!(
            output.status.code(),
            Some(102),
            "{command}: {}",
            all(&output)
        );
        assert!(
            text(&output.stderr).contains("memory limit"),
            "{command}: {}",
            all(&output)
        );
        assert!(!text(&output.stdout).contains("hello"), "{command}");
    }
    let small = p.policy("[limits]\nheap_bytes = 2_097_152\n");
    let output = p.unio(
        &["run", "--policy", &small.to_string_lossy(), "main.unio"],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    assert_eq!(text(&output.stdout), "hello\n");
}

/// (f) A worker that uses more memory than the limit of the policy is stopped by the operating
/// system (Linux: the address space; macOS: the footprint the tool watches; Windows: the Job
/// Object), and the tool reports it with code 102.
#[test]
fn a_worker_over_the_memory_limit_is_stopped() {
    let p = Project::new("memory");
    let policy = p.policy("[limits]\nheap_bytes = 536870912\nwall_seconds = 60\n");
    let output = p.unio(
        &["check", "--policy", &policy.to_string_lossy(), "main.unio"],
        &[("UNIO_WORKER_PROBE", "alloc=4294967296")],
    );
    assert_eq!(output.status.code(), Some(102), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("memory limit"),
        "{}",
        all(&output)
    );
}

/// (f) Pathological sources: deep nesting is refused by the parser's bound (E0013) at once; a
/// huge generated file under a small memory limit stops the worker by the limit, with a
/// diagnostic, within the time limit.
#[test]
fn pathological_sources_end_with_a_diagnostic() {
    let p = Project::new("huge");
    let deep = format!(
        "public function main():\n    let x = {}1{}\n",
        "(".repeat(100_000),
        ")".repeat(100_000)
    );
    fs::write(p.pkg().join("deep.unio"), deep).unwrap();
    let output = p.unio(&["check", "deep.unio"], &[]);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(text(&output.stdout).contains("E0013"), "{}", all(&output));
    let mut huge = String::from("public function main():\n");
    let line = "    let value = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]\n";
    while huge.len() < 48 << 20 {
        huge.push_str(line);
    }
    fs::write(p.pkg().join("huge.unio"), huge).unwrap();
    let policy = p.policy("[limits]\nheap_bytes = 268435456\nwall_seconds = 30\n");
    let started = Instant::now();
    let output = p.unio(
        &["check", "--policy", &policy.to_string_lossy(), "huge.unio"],
        &[],
    );
    assert_eq!(output.status.code(), Some(102), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("the worker of the tool was stopped"),
        "{}",
        all(&output)
    );
    assert!(started.elapsed() < Duration::from_secs(60));
}

/// (g) The worker sees no variable of the environment of the tool but those that select the
/// language of messages: a secret in the environment does not reach it.
#[test]
fn the_worker_sees_only_the_language_variables() {
    let p = Project::new("env");
    let output = p.unio(
        &["check", "main.unio"],
        &[
            ("UNIO_WORKER_PROBE", "env"),
            ("UNIO_TEST_SECRET", SECRET),
            ("LANG", "C.UTF-8"),
        ],
    );
    let result = probe_result(&output, "env");
    let names: Vec<&str> = result
        .trim_start_matches("allowed [")
        .trim_end_matches(']')
        .split(',')
        .filter(|n| !n.is_empty())
        .collect();
    let permitted = [
        "UNIO_LANG",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_MESSAGES",
        "SystemRoot",
    ];
    assert!(
        names.iter().all(|n| permitted.contains(n)),
        "the worker sees {names:?}"
    );
    assert!(names.contains(&"LANG"), "{names:?}");
    assert!(!all(&output).contains(SECRET));
}

/// (g) The same, as the system shows it: the environment of the running worker process in
/// `/proc/<pid>/environ` has no secret of the tool.
#[cfg(target_os = "linux")]
#[test]
fn the_environment_of_the_worker_process_has_no_secret() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let p = Project::new("environ");
    let mut child = p
        .command(
            &["check", "main.unio"],
            &[
                ("UNIO_WORKER_PROBE", "pid;sleep=3"),
                ("UNIO_TEST_SECRET", SECRET),
            ],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let pid = loop {
        let line = lines.next().expect("the probe of the pid").unwrap();
        if let Some(pid) = line.strip_prefix("unio worker probe: pid: allowed ") {
            break pid.trim().to_string();
        }
    };
    let environ = fs::read(format!("/proc/{pid}/environ")).unwrap();
    let tool = fs::read(format!("/proc/{}/environ", child.id())).unwrap();
    assert!(
        String::from_utf8_lossy(&tool).contains(SECRET),
        "the tool itself has the secret"
    );
    assert!(
        !String::from_utf8_lossy(&environ).contains(SECRET),
        "the worker has the secret"
    );
    for _ in lines {}
    assert!(child.wait().unwrap().success());
}

/// `unio build`: the compile step runs in the worker and cannot write the output path; the
/// executable reaches the path the operator chose, and nothing else outside the package or its
/// temporary directory is written. Where executables are made and the operating system
/// restricts the file system of the workers: Linux on x86-64 and AArch64, and macOS on AArch64,
/// where it also runs the link step under its Seatbelt profile.
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(target_os = "macos", target_arch = "aarch64"),
))]
#[test]
fn build_writes_the_output_only_through_the_tool() {
    let exe = Path::new(env!("CARGO_BIN_EXE_unio"));
    if !exe.with_file_name("libunio.a").is_file() {
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
    let p = Project::new("build");
    let out = p.path("outside/hello");
    let probes = vec![op("write", &out), op("write", &p.pkg().join("hello"))];
    let output = p.unio(
        &["build", "main.unio", "-o", &out.to_string_lossy()],
        &[("UNIO_WORKER_PROBE", &probes.join(";"))],
    );
    assert_denied(&output, &probes);
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    let ran = Command::new(&out).output().unwrap();
    assert_eq!(text(&ran.stdout), "hello\n");
    assert!(!p.pkg().join("hello").exists());
}

/// Builds `libunio.a` next to the tool when it is missing, for the tests of `unio build`.
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(target_os = "macos", target_arch = "aarch64"),
))]
fn ensure_runtime_library() {
    let exe = Path::new(env!("CARGO_BIN_EXE_unio"));
    if !exe.with_file_name("libunio.a").is_file() {
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

/// The processes whose command line names `marker` and that have not ended (zombies aside).
#[cfg(unix)]
fn processes_naming(marker: &str) -> Vec<String> {
    let listed = Command::new("ps")
        .args(["-A", "-o", "pid=,stat=,args="])
        .output()
        .unwrap();
    text(&listed.stdout)
        .lines()
        .filter(|l| l.contains(marker))
        .filter(|l| {
            let mut words = l.split_whitespace();
            let _pid = words.next();
            !words.next().is_some_and(|stat| stat.starts_with('Z'))
        })
        .map(str::to_string)
        .collect()
}

/// (f) The link step over the time limit ends with its whole process tree (docs/runtime.md
/// 9.7): the C compiler is a script that starts a child and both run forever. After the tool
/// reports code 102, neither the script nor its child runs. The script uses only built-ins of
/// the shell, since the link step may execute nothing but the compiler and its interpreter.
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(target_os = "macos", target_arch = "aarch64"),
))]
#[test]
fn the_link_step_over_the_time_limit_ends_with_its_process_tree() {
    use std::os::unix::fs::PermissionsExt;
    ensure_runtime_library();
    let p = Project::new("link-tree");
    let cc = p.path("outside/unio-hanging-cc");
    fs::write(
        &cc,
        "#!/bin/sh\n\
         for a in \"$@\"; do\n\
         \x20   case \"$a\" in\n\
         \x20       --version) echo 'hanging-cc 1.0'; exit 0 ;;\n\
         \x20       -print-prog-name=*) exit 1 ;;\n\
         \x20   esac\n\
         done\n\
         ( while :; do :; done ) &\n\
         while :; do :; done\n",
    )
    .unwrap();
    fs::set_permissions(&cc, fs::Permissions::from_mode(0o755)).unwrap();
    let cc_text = cc.to_string_lossy().into_owned();
    let policy = p.policy("[limits]\nwall_seconds = 3\n");
    let output = p.unio(
        &[
            "build",
            "--policy",
            &policy.to_string_lossy(),
            "main.unio",
            "-o",
            &p.path("outside/hello").to_string_lossy(),
        ],
        &[("CC", &cc_text)],
    );
    let mut left = processes_naming(&cc_text);
    for _ in 0..100 {
        if left.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        left = processes_naming(&cc_text);
    }
    // Clean up before asserting, so that a failure leaves nothing running.
    for line in &left {
        if let Some(pid) = line.split_whitespace().next() {
            let _ = Command::new("kill").args(["-9", pid]).status();
        }
    }
    assert_eq!(output.status.code(), Some(102), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("time limit of 3 s"),
        "{}",
        all(&output)
    );
    assert!(
        left.is_empty(),
        "the link step left processes running:\n{}",
        left.join("\n")
    );
}
