//! The sandbox suite of SB-8: Unio programs that attempt forbidden actions through every module
//! of the standard library that reaches the system and through child processes, each of which
//! must be denied; and the policy of SB-3 and SB-5: narrowing, warnings, limits, and failing
//! closed with code 103.
//!
//! Every program runs in its own package in a temporary directory, with `open/` granted by its
//! manifest and `closed/` holding a file the program must never see.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SECRET: &str = "SECRET-CONTENT";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// A package with a manifest, a program `main.unio`, and the files the programs try to reach.
fn package(name: &str, manifest: &str, program: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-sandbox-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("open/sub")).unwrap();
    fs::create_dir_all(dir.join("closed")).unwrap();
    fs::write(dir.join("open/a.txt"), "alpha").unwrap();
    fs::write(dir.join("open/sub/b.txt"), "beta").unwrap();
    fs::write(dir.join("closed/secret.txt"), SECRET).unwrap();
    fs::write(dir.join("unio.toml"), with_decoys(manifest)).unwrap();
    fs::write(dir.join("main.unio"), program).unwrap();
    fs::canonicalize(dir).unwrap()
}

/// The manifest with a harmless grant for every kind of permission it does not list: the
/// programs share helpers that use every kind, and the compile-time check (E0085, SB-7) would
/// otherwise reject them before the sandbox is reached. A manifest without `[permissions]`
/// stays as it is: it permits nothing, and the compiler does not check it.
fn with_decoys(manifest: &str) -> String {
    if !manifest.contains("[permissions]") {
        return manifest.to_string();
    }
    let mut out = manifest.to_string();
    for (key, decoy) in [
        ("files.read", "[\"$CWD/unio-decoy/**\"]"),
        ("files.write", "[\"$CWD/unio-decoy/**\"]"),
        ("env", "[\"UNIO_DECOY\"]"),
        ("process", "[\"unio-decoy\"]"),
    ] {
        if !manifest.lines().any(|l| l.starts_with(&format!("{key} "))) {
            out.push_str(&format!("{key} = {decoy}\n"));
        }
    }
    out
}

/// A policy file; on Windows, where the operating system part of the sandbox is missing, the
/// tests act as the operator and allow the weaker sandbox (SB-5).
fn policy(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("policy.toml");
    let weaker = if cfg!(windows) {
        "allow_weaker = true\n"
    } else {
        ""
    };
    fs::write(&path, format!("{weaker}{body}")).unwrap();
    path
}

/// Runs `main.unio` of the package in its directory, with sandbox options before the file.
fn run(dir: &Path, options: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .arg("run");
    if cfg!(windows) && !options.iter().any(|o| o.starts_with("--policy")) {
        command.arg("--allow-weaker-sandbox");
    }
    command.args(options).arg("main.unio");
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().unwrap()
}

#[track_caller]
fn lines_of(output: &Output) -> Vec<String> {
    let out = text(&output.stdout);
    let err = text(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    out.lines().map(String::from).collect()
}

/// Checks that every attempt was denied and every granted operation worked.
#[track_caller]
fn assert_denied(output: &Output, expected: &[&str]) {
    let lines = lines_of(output);
    let all = text(&output.stdout) + &text(&output.stderr);
    assert!(!all.contains(SECRET), "the secret leaked:\n{all}");
    let breaches: Vec<&String> = lines.iter().filter(|l| l.starts_with("BREACH")).collect();
    assert!(breaches.is_empty(), "{all}");
    for name in expected {
        assert!(
            lines
                .iter()
                .any(|l| l == &format!("denied {name}") || l == &format!("granted {name}")),
            "no result for {name}:\n{all}"
        );
    }
}

const PROGRAM_HEAD: &str = "import std.env
import std.files
import std.process
from std.files import IoErrorKind
from std.process import ProcessErrorKind

function report(name as Text, denied as Bool):
    if denied:
        print(\"denied {name}\")
    else:
        print(\"BREACH {name}\")

function granted(name as Text, worked as Bool):
    if worked:
        print(\"granted {name}\")
    else:
        print(\"BREACH {name} does not work\")

function is_denied(e as IoError) returns Bool:
    return e.kind == IoErrorKind.PermissionDenied and e.reason.contains(\"denied by the sandbox\")

function read_text_denied(path as Path) returns Bool:
    try:
        print(try files.read_text(path))
        return false
    on IoError as e:
        return is_denied(e)

function read_bytes_denied(path as Path) returns Bool:
    try:
        print(\"{(try files.read_bytes(path)).length()} bytes\")
        return false
    on IoError as e:
        return is_denied(e)

function open_reader_denied(path as Path) returns Bool:
    try:
        using reader = try files.open_reader(path):
            print(\"opened {path.to_text()}\")
        return false
    on IoError as e:
        return is_denied(e)

function list_directory_denied(path as Path) returns Bool:
    try:
        for entry in try files.list_directory(path):
            print(entry.to_text())
        return false
    on IoError as e:
        return is_denied(e)

function resolve_denied(path as Path) returns Bool:
    try:
        print((try files.resolve(path)).to_text())
        return false
    on IoError as e:
        return is_denied(e)

function write_text_denied(path as Path) returns Bool:
    try:
        try files.write_text(path, \"written\")
        return false
    on IoError as e:
        return is_denied(e)

function write_bytes_denied(path as Path) returns Bool:
    try:
        try files.write_bytes(path, Bytes())
        return false
    on IoError as e:
        return is_denied(e)

function copy_denied(source as Path, target as Path) returns Bool:
    try:
        try files.copy_file(source, target)
        return false
    on IoError as e:
        return is_denied(e)

function rename_denied(source as Path, target as Path) returns Bool:
    try:
        try files.rename(source, target)
        return false
    on IoError as e:
        return is_denied(e)

function create_directory_denied(path as Path) returns Bool:
    try:
        try files.create_directory(path)
        return false
    on IoError as e:
        return is_denied(e)

function run_denied(program as Text, arguments as List[Text]) returns Bool:
    try:
        let output = try process.run(program, arguments, timeout_seconds = 10)
        print(output.stdout)
        return false
    on ProcessError as e:
        return e.kind == ProcessErrorKind.PermissionDenied

";

#[test]
fn every_module_of_the_standard_library_denies_what_the_manifest_does_not_permit() {
    let manifest = "[permissions]\nfiles.read = [\"$CWD/open/**\"]\nfiles.write = [\"$CWD/open/out.txt\"]\nenv = [\"UNIO_ALLOWED\"]\nprocess = [\"unio-listed-program\"]\n";
    let program = format!(
        "{PROGRAM_HEAD}public function main():
    let secret = Path(\"closed/secret.txt\")
    report(\"files.read_text\", read_text_denied(secret))
    report(\"files.read_text through ..\", read_text_denied(Path(\"open/../closed/secret.txt\")))
    report(\"files.read_bytes\", read_bytes_denied(secret))
    report(\"files.open_reader\", open_reader_denied(secret))
    report(\"files.list_directory\", list_directory_denied(Path(\"closed\")))
    report(\"files.list_directory of the package\", list_directory_denied(Path(\".\")))
    report(\"files.resolve\", resolve_denied(secret))
    report(\"files.exists\", not files.exists(secret))
    report(\"files.is_directory\", not files.is_directory(Path(\"closed\")))
    report(\"files.write_text\", write_text_denied(Path(\"closed/new.txt\")))
    report(\"files.write_text in a read scope\", write_text_denied(Path(\"open/a.txt\")))
    report(\"files.write_text of the manifest\", write_text_denied(Path(\"unio.toml\")))
    report(\"files.write_bytes\", write_bytes_denied(Path(\"closed/new.bin\")))
    report(\"files.copy_file out\", copy_denied(Path(\"open/a.txt\"), Path(\"closed/copy.txt\")))
    report(\"files.copy_file in\", copy_denied(secret, Path(\"open/out.txt\")))
    report(\"files.rename\", rename_denied(Path(\"open/a.txt\"), Path(\"closed/a.txt\")))
    report(\"files.rename of a secret\", rename_denied(secret, Path(\"open/out.txt\")))
    report(\"files.create_directory\", create_directory_denied(Path(\"closed/dir\")))
    report(\"env.get\", env.get(\"UNIO_SECRET\").is_none())
    report(\"process.run of an unlisted program\", run_denied(\"unio-unlisted-program\", []))
    report(\"process.run of a listed name by path\", run_denied(\"/usr/bin/unio-listed-program\", []))
    granted(\"files.read_text\", not read_text_denied(Path(\"open/a.txt\")))
    granted(\"files.exists\", files.exists(Path(\"open/sub/b.txt\")))
    granted(\"files.list_directory\", not list_directory_denied(Path(\"open\")))
    granted(\"files.write_text\", not write_text_denied(Path(\"open/out.txt\")))
    granted(\"env.get\", env.get(\"UNIO_ALLOWED\") == some(\"yes\"))
"
    );
    let dir = package("modules", manifest, &program);
    let output = run(
        &dir,
        &[],
        &[("UNIO_SECRET", SECRET), ("UNIO_ALLOWED", "yes")],
    );
    assert_denied(
        &output,
        &[
            "files.read_text",
            "files.read_text through ..",
            "files.read_bytes",
            "files.open_reader",
            "files.list_directory",
            "files.list_directory of the package",
            "files.resolve",
            "files.exists",
            "files.is_directory",
            "files.write_text",
            "files.write_text in a read scope",
            "files.write_text of the manifest",
            "files.write_bytes",
            "files.copy_file out",
            "files.copy_file in",
            "files.rename",
            "files.rename of a secret",
            "files.create_directory",
            "env.get",
            "process.run of an unlisted program",
            "process.run of a listed name by path",
        ],
    );
    // Nothing was created or moved outside the scopes, and the granted file was written.
    assert!(!dir.join("closed/new.txt").exists() && !dir.join("closed/a.txt").exists());
    assert!(dir.join("open/a.txt").exists() && dir.join("closed/secret.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.join("open/out.txt")).unwrap(),
        "written"
    );
    let leftovers: Vec<_> = fs::read_dir(dir.join("open"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("unio-tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temporary files are left: {leftovers:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn without_permissions_a_program_only_has_its_arguments_and_standard_streams() {
    let program = format!(
        "{PROGRAM_HEAD}import std.args

public function main():
    print(\"arguments {{args.list().length()}}\")
    report(\"files.read_text\", read_text_denied(Path(\"open/a.txt\")))
    report(\"files.write_text\", write_text_denied(Path(\"open/new.txt\")))
    report(\"files.exists\", not files.exists(Path(\"open/a.txt\")))
    report(\"env.get\", env.get(\"PATH\").is_none())
    report(\"process.run\", run_denied(\"git\", [\"--version\"]))
"
    );
    let dir = package("none", "[package]\nname = \"none\"\n", &program);
    let output = run(&dir, &[], &[]);
    assert_denied(
        &output,
        &[
            "files.read_text",
            "files.write_text",
            "files.exists",
            "env.get",
            "process.run",
        ],
    );
    assert!(!dir.join("open/new.txt").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_link_inside_a_scope_does_not_lead_out_of_it() {
    let program = format!(
        "{PROGRAM_HEAD}public function main():
    report(\"files.read_text through a link\", read_text_denied(Path(\"open/link\")))
    report(\"files.write_text through a link\", write_text_denied(Path(\"open/dir-link/x.txt\")))
"
    );
    let manifest =
        "[permissions]\nfiles.read = [\"$CWD/open/**\"]\nfiles.write = [\"$CWD/open/**\"]\n";
    let dir = package("link", manifest, &program);
    std::os::unix::fs::symlink(dir.join("closed/secret.txt"), dir.join("open/link")).unwrap();
    std::os::unix::fs::symlink(dir.join("closed"), dir.join("open/dir-link")).unwrap();
    let output = run(&dir, &[], &[]);
    assert_denied(
        &output,
        &[
            "files.read_text through a link",
            "files.write_text through a link",
        ],
    );
    assert!(!dir.join("closed/x.txt").exists());
    let _ = fs::remove_dir_all(&dir);
}

/// A child process is not checked by the runtime: only the operating system stops it, so this
/// is where the restrictions of SB-1 and SB-4 show. Linux and macOS enforce them; Windows does
/// not yet (docs/runtime.md 7.4).
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn child_processes_inherit_the_sandbox() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let python =
        Command::new("python3").arg("--version").output().is_ok() && cfg!(target_os = "linux");
    let program = format!(
        "{PROGRAM_HEAD}function child(name as Text, program as Text, arguments as List[Text]):
    let output = try process.run(program, arguments, timeout_seconds = 30) on ProcessError as e:
        print(\"BREACH {{name}}: the child does not start: {{e.reason}}\")
        return
    print(output.stdout)
    if output.status != 0:
        print(\"denied {{name}}\")
    else:
        print(\"BREACH {{name}}\")

public function main():
    let output = try process.run(\"cat\", [\"open/a.txt\"]) on ProcessError as e:
        print(\"BREACH cat does not start: {{e.reason}}\")
        return
    if output.stdout != \"alpha\":
        print(\"BREACH cat ended with {{output.status}}: {{output.stderr}}\")
    granted(\"process.run\", output.stdout == \"alpha\")
    child(\"a child reads a closed file\", \"cat\", [\"closed/secret.txt\"])
    child(\"a child reads through ..\", \"cat\", [\"open/../closed/secret.txt\"])
    if {python}:
        child(\"a child writes a closed file\", \"python3\", [\"-c\", \"open('closed/new.txt', 'w').write('x')\"])
        child(\"a child connects to the network\", \"python3\", [\"-c\", \"import socket; socket.create_connection(('127.0.0.1', {port}), 5)\"])
        child(\"a grandchild reads a closed file\", \"python3\", [\"-c\", \"import subprocess, sys; sys.exit(subprocess.call(['/bin/sh', '-c', 'cat closed/secret.txt']))\"])
    else:
        print(\"denied a child writes a closed file\")
        print(\"denied a child connects to the network\")
        print(\"denied a grandchild reads a closed file\")
"
    );
    let manifest =
        "[permissions]\nfiles.read = [\"$CWD/open/**\"]\nprocess = [\"cat\", \"python3\"]\n";
    let dir = package("children", manifest, &program);
    let output = run(&dir, &[], &[]);
    assert_denied(
        &output,
        &[
            "process.run",
            "a child reads a closed file",
            "a child reads through ..",
            "a child writes a closed file",
            "a child connects to the network",
            "a grandchild reads a closed file",
        ],
    );
    assert!(!dir.join("closed/new.txt").exists());
    drop(listener);
    let _ = fs::remove_dir_all(&dir);
}

/// Audit item 3, second part: a `process` permission lets a child run exactly the listed
/// programs and read their libraries, nothing else (docs/runtime.md 9.5 and 9.6). The child is
/// a shell that may run, so it tries the rest itself: running unlisted programs by path and by
/// name, running one through the dynamic loader, and reading with its own redirections a file
/// outside the read scopes and `/etc/passwd`. It still reads its permitted input.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_child_runs_and_reads_only_what_the_manifest_lists() {
    let outside = std::env::temp_dir().join(format!("unio-outside-{}.txt", std::process::id()));
    fs::write(&outside, SECRET).unwrap();
    let outside_text = outside.to_string_lossy().into_owned();
    let program = format!(
        "{PROGRAM_HEAD}function child(name as Text, script as Text):
    let output = try process.run(\"sh\", [\"-c\", script], timeout_seconds = 30) on ProcessError as e:
        print(\"BREACH {{name}}: the child does not start: {{e.reason}}\")
        return
    print(output.stdout)
    if output.status != 0:
        print(\"denied {{name}}\")
    else:
        print(\"BREACH {{name}}\")

public function main():
    let output = try process.run(\"sh\", [\"-c\", \"exec 3< open/a.txt; IFS= read -r line <&3; printf %s \\\"$line\\\"\"]) on ProcessError as e:
        print(\"BREACH sh does not start: {{e.reason}}\")
        return
    if output.stdout != \"alpha\":
        print(\"BREACH sh ended with {{output.status}}: {{output.stdout}} {{output.stderr}}\")
    granted(\"the listed program reads its input\", output.stdout == \"alpha\")
    child(\"a child runs /bin/cat\", \"/bin/cat open/a.txt\")
    child(\"a child runs cat through PATH\", \"cat open/a.txt\")
    child(\"a child runs /usr/bin/env\", \"/usr/bin/env\")
    child(\"a child runs a program through the loader\", \"for l in /lib64/ld-linux-x86-64.so.2 /lib/ld-linux-aarch64.so.1; do if [ -e $l ]; then exec $l /bin/cat open/a.txt; fi; done; exit 1\")
    child(\"a child reads a file outside the scopes\", \"exec 3< {outside_text}; IFS= read -r line <&3; printf %s \\\"$line\\\"\")
    child(\"a child reads /etc/passwd\", \"exec 3< /etc/passwd; IFS= read -r line <&3; printf %s \\\"$line\\\"\")
"
    );
    let manifest = "[permissions]\nfiles.read = [\"$CWD/open/**\"]\nprocess = [\"sh\"]\n";
    let dir = package("child-exec", manifest, &program);
    let output = run(&dir, &[], &[]);
    let _ = fs::remove_file(&outside);
    assert_denied(
        &output,
        &[
            "the listed program reads its input",
            "a child runs /bin/cat",
            "a child runs cat through PATH",
            "a child runs /usr/bin/env",
            "a child runs a program through the loader",
            "a child reads a file outside the scopes",
            "a child reads /etc/passwd",
        ],
    );
    assert!(
        !text(&output.stdout).contains("root:"),
        "{}",
        text(&output.stdout)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A descendant of a child that tries to leave the process group with a double fork into a new
/// session is stopped with the tree at the deadline and does not outlive the call (docs/runtime.md
/// 9.5). Linux only: the test finds the process by a marker in its command line in `/proc`.
#[cfg(target_os = "linux")]
#[test]
fn a_descendant_that_leaves_the_process_group_is_stopped_at_the_timeout() {
    if !Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipped: python3 is not available");
        return;
    }
    let marker = format!("unio-escape-marker-{}", std::process::id());
    let script = "import os, time\nif os.fork() == 0:\n    if os.fork() != 0:\n        os._exit(0)\n    try:\n        os.setsid()\n    except OSError:\n        pass\n    time.sleep(60)\n    os._exit(0)\ntime.sleep(60)\n";
    let program = format!(
        "{PROGRAM_HEAD}public function main():
    let output = try process.run(\"python3\", [\"-c\", \"{}\", \"{marker}\"], timeout_seconds = 2) on ProcessError as e:
        report(\"a descendant outlives the deadline\", e.kind == ProcessErrorKind.TimedOut)
        return
    print(\"BREACH the call did not time out: {{output.status}} {{output.stderr}}\")
",
        script.replace('\n', "\\n")
    );
    let manifest = "[permissions]\nprocess = [\"python3\"]\n";
    let dir = package("child-setsid", manifest, &program);
    let output = run(&dir, &[], &[]);
    assert_denied(&output, &["a descendant outlives the deadline"]);
    let alive = || {
        fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .bytes()
                    .all(|b| b.is_ascii_digit())
            })
            .any(|e| {
                fs::read(e.path().join("cmdline"))
                    .is_ok_and(|c| String::from_utf8_lossy(&c).contains(&marker))
            })
    };
    let mut still = alive();
    for _ in 0..100 {
        if !still {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        still = alive();
    }
    let _ = fs::remove_dir_all(&dir);
    assert!(
        !still,
        "a descendant that left the process group still runs"
    );
}

#[test]
fn a_policy_narrows_the_manifest_and_warns_about_extra_permissions() {
    let program = format!(
        "{PROGRAM_HEAD}public function main():
    report(\"files.read_text outside the policy\", read_text_denied(Path(\"open/a.txt\")))
    granted(\"files.read_text inside the policy\", not read_text_denied(Path(\"open/sub/b.txt\")))
    report(\"files.read_text of a policy-only path\", read_text_denied(Path(\"closed/secret.txt\")))
    report(\"env.get not in the policy\", env.get(\"UNIO_ALLOWED\").is_none())
"
    );
    let manifest = "[permissions]\nfiles.read = [\"$CWD/open/**\"]\nenv = [\"UNIO_ALLOWED\"]\n";
    let dir = package("narrow", manifest, &program);
    let file = policy(
        &dir,
        "[permissions]\nfiles.read = [\"$CWD/open/sub/**\", \"$CWD/closed/**\"]\n",
    );
    let file = file.to_string_lossy().into_owned();
    let output = run(&dir, &["--policy", &file], &[("UNIO_ALLOWED", "yes")]);
    assert_denied(
        &output,
        &[
            "files.read_text outside the policy",
            "files.read_text inside the policy",
            "files.read_text of a policy-only path",
            "env.get not in the policy",
        ],
    );
    let err = text(&output.stderr);
    assert!(
        err.contains("warning: the policy permits files.read") && err.contains("closed"),
        "SB-3: an extra permission of the policy gives a warning:\n{err}"
    );
    // UNIO_POLICY gives the same policy.
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    let again = command
        .current_dir(&dir)
        .env("UNIO_POLICY", &file)
        .env("UNIO_ALLOWED", "yes")
        .args(["run", "main.unio"])
        .output()
        .unwrap();
    assert_eq!(lines_of(&again), lines_of(&output));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_limits_of_a_policy_end_the_program_or_fail_the_operation() {
    let manifest = "[permissions]\nfiles.write = [\"$CWD/open/**\"]\nprocess = [\"cat\"]\n";
    let cases: [(&str, &str, &str); 4] = [
        (
            "heap",
            "limits.heap_bytes = 1_000_000",
            "public function main():\n    var all as List[List[Int]] = []\n    while true:\n        all.append([n for n in range(0, 10000)])\n",
        ),
        (
            "wall",
            "limits.wall_seconds = 1",
            "public function main():\n    while true:\n        pass\n",
        ),
        (
            "cpu",
            "limits.cpu_seconds = 1",
            "public function main():\n    var n = 0\n    while true:\n        n = (n + 1) % 1000\n",
        ),
        (
            "file size",
            "limits.file_size_bytes = 10",
            "import std.files\n\npublic function main():\n    try:\n        try files.write_text(Path(\"open/big.txt\"), \"0123456789abcdef\")\n        print(\"BREACH\")\n    on IoError as e:\n        print(\"denied: {e.reason}\")\n",
        ),
    ];
    for (name, limit, program) in cases {
        let dir = package(
            &format!("limit-{}", name.replace(' ', "-")),
            manifest,
            program,
        );
        let file = policy(
            &dir,
            &format!("[limits]\n{}\n", limit.trim_start_matches("limits.")),
        );
        let file = file.to_string_lossy().into_owned();
        let started = std::time::Instant::now();
        let output = run(&dir, &["--policy", &file], &[]);
        let out = text(&output.stdout);
        let err = text(&output.stderr);
        if name == "file size" {
            assert_eq!(output.status.code(), Some(0), "{out}{err}");
            assert!(
                out.contains("denied: the policy limits written files to 10 bytes"),
                "{out}{err}"
            );
            assert!(!dir.join("open/big.txt").exists());
        } else {
            assert_eq!(output.status.code(), Some(102), "{name}: {out}{err}");
            assert!(started.elapsed().as_secs() < 60, "{name} took too long");
        }
        let _ = fs::remove_dir_all(&dir);
    }
    let program = format!(
        "{PROGRAM_HEAD}public function main():\n    report(\"process.run over the limit\", run_denied(\"cat\", [\"unio.toml\"]))\n"
    );
    let dir = package("limit-processes", manifest, &program);
    let file = policy(&dir, "[limits]\nprocesses = 0\n");
    let output = run(&dir, &["--policy", &file.to_string_lossy()], &[]);
    assert_denied(&output, &["process.run over the limit"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_policy_or_manifest_that_cannot_be_used_stops_the_start_with_code_103() {
    let program = "public function main():\n    print(\"ran\")\n";
    let dir = package("closed", "[package]\nname = \"x\"\n", program);
    let missing = dir
        .join("no-such-policy.toml")
        .to_string_lossy()
        .into_owned();
    let cases: Vec<(Vec<String>, &str)> = vec![
        (vec!["--policy".into(), missing.clone()], "cannot be read"),
        (vec![format!("--policy={missing}")], "cannot be read"),
    ];
    for (options, message) in cases {
        let options: Vec<&str> = options.iter().map(String::as_str).collect();
        let output = run(&dir, &options, &[]);
        assert_eq!(output.status.code(), Some(103), "{}", text(&output.stderr));
        assert!(
            text(&output.stderr).contains(message),
            "{}",
            text(&output.stderr)
        );
        assert!(text(&output.stdout).is_empty());
    }
    for body in [
        "[limits]\nheap = 1\n",
        "[limits\n",
        "[permissions]\nnetwork = true\n",
    ] {
        let file = policy(&dir, body);
        let output = run(&dir, &["--policy", &file.to_string_lossy()], &[]);
        assert_eq!(
            output.status.code(),
            Some(103),
            "{body}: {}",
            text(&output.stderr)
        );
        assert!(
            text(&output.stderr).contains("is invalid"),
            "{}",
            text(&output.stderr)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(&dir)
        .env("UNIO_POLICY", &missing)
        .args(["run", "main.unio"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(103));
    fs::write(
        dir.join("unio.toml"),
        "[permissions]\nfiles.read = \"$CWD\"\n",
    )
    .unwrap();
    let output = run(&dir, &[], &[]);
    assert_eq!(output.status.code(), Some(103), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("unio.toml is invalid"));
    // unio test applies the same sandbox and the same rule.
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(&dir)
        .args(["test", "--policy", &missing, "main.unio"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(103));
    let _ = fs::remove_dir_all(&dir);
}

/// Without the mechanisms of the operating system the sandbox fails closed unless the operator
/// allows a weaker one, which then names what is missing (SB-5).
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[test]
fn a_missing_mechanism_fails_closed() {
    let dir = package(
        "weaker",
        "[package]\nname = \"x\"\n",
        "public function main():\n    print(\"ran\")\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(&dir)
        .env_remove("UNIO_POLICY")
        .args(["run", "main.unio"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(103));
    assert!(text(&output.stderr).contains("--allow-weaker-sandbox"));
    let output = run(&dir, &[], &[]);
    assert_eq!(text(&output.stdout), "ran\n");
    assert!(text(&output.stderr).contains("missing"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn tests_run_in_the_sandbox_of_their_package() {
    let program = format!(
        "{PROGRAM_HEAD}test \"the sandbox applies to tests\":
    assert read_text_denied(Path(\"closed/secret.txt\"))
    assert not read_text_denied(Path(\"open/a.txt\"))
"
    );
    let manifest = "[permissions]\nfiles.read = [\"$CWD/open/**\"]\n";
    let dir = package("tests", manifest, &program);
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(&dir)
        .env_remove("UNIO_POLICY")
        .arg("test");
    if cfg!(windows) {
        command.arg("--allow-weaker-sandbox");
    }
    let output = command.arg("main.unio").output().unwrap();
    let out = text(&output.stdout);
    assert!(output.status.success(), "{out}{}", text(&output.stderr));
    assert!(out.contains("1 passed, 0 failed"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

/// A child sees only the variables the manifest grants under `env`, and PATH (docs/runtime.md
/// 9.5): a secret in the environment of `unio run` does not reach it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_child_sees_only_the_granted_environment() {
    let program = format!(
        "{PROGRAM_HEAD}public function main():
    let output = try process.run(\"env\", []) on ProcessError as e:
        print(\"BREACH env does not start: {{e.reason}}\")
        return
    print(output.stdout)
"
    );
    let manifest = "[permissions]\nenv = [\"UNIO_ALLOWED\"]\nprocess = [\"env\"]\n";
    let dir = package("child-env", manifest, &program);
    let output = run(
        &dir,
        &[],
        &[("UNIO_SECRET", SECRET), ("UNIO_ALLOWED", "yes")],
    );
    let lines = lines_of(&output);
    assert!(!text(&output.stdout).contains(SECRET), "{lines:?}");
    assert!(lines.iter().any(|l| l == "UNIO_ALLOWED=yes"), "{lines:?}");
    let names: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.split_once('=').map(|(name, _)| name))
        .collect();
    assert!(
        names.iter().all(|n| ["UNIO_ALLOWED", "PATH"].contains(n)),
        "the child sees variables that are not granted: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A read whose buffer the program sizes is checked against the heap limit before the buffer
/// is allocated (C-18): the program ends with code 102 and its diagnostic, not with an abort.
#[test]
fn a_huge_read_under_a_heap_limit_ends_with_code_102() {
    let program = "import std.files

public function main():
    try:
        using reader = try files.open_reader(Path(\"open/a.txt\")):
            let data = try reader.read_bytes(max_bytes = 4_000_000_000_000_000)
            print(\"read {data.is_some()}\")
    on IoError as e:
        print(\"error {e.reason}\")
";
    let manifest = "[permissions]\nfiles.read = [\"$CWD/open/**\"]\n";
    let dir = package("huge-read", manifest, program);
    let file = policy(&dir, "[limits]\nheap_bytes = 10_000_000\n");
    let output = run(&dir, &["--policy", &file.to_string_lossy()], &[]);
    let err = text(&output.stderr);
    assert_eq!(output.status.code(), Some(102), "{err}");
    assert!(err.contains("memory exhausted"), "{err}");
    let _ = fs::remove_dir_all(&dir);
}

/// list_tree does not follow links to directories, so a cycle of links ends (std.files).
#[cfg(unix)]
#[test]
fn list_tree_does_not_follow_links_to_directories() {
    let program = "import std.files

public function main():
    let found = try files.list_tree(Path(\"open\")) on IoError as e:
        print(\"error {e.reason}\")
        return
    for entry in found:
        print(entry.to_text())
";
    let manifest = "[permissions]\nfiles.read = [\"$CWD/open/**\"]\n";
    let dir = package("tree-cycle", manifest, program);
    std::os::unix::fs::symlink(dir.join("open"), dir.join("open/sub/loop")).unwrap();
    std::os::unix::fs::symlink(dir.join("open/sub/b.txt"), dir.join("open/file-link")).unwrap();
    let output = run(&dir, &[], &[]);
    assert_eq!(
        lines_of(&output),
        [
            "open/a.txt",
            "open/file-link",
            "open/sub/b.txt",
            "open/sub/loop"
        ]
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Audit item 3: permission to write one file does not open its directory at the level of the
/// operating system. A child started through `process.run` inherits the restrictions of the
/// program, which writes no file itself: every write goes through the writer process, which
/// permits exactly the scopes (docs/runtime.md 9.2). The child can neither change a neighbouring
/// file nor create a new one, and the program still replaces its one file atomically, with and
/// without `durable`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_child_cannot_change_a_neighbour_of_a_writable_file() {
    let program = format!(
        "{PROGRAM_HEAD}function child(name as Text, script as Text):
    let output = try process.run(\"sh\", [\"-c\", script], timeout_seconds = 30) on ProcessError as e:
        print(\"BREACH {{name}}: the child does not start: {{e.reason}}\")
        return
    if output.status != 0:
        print(\"denied {{name}}\")
    else:
        print(\"BREACH {{name}}\")

function write_works(name as Text, text as Text, durable as Bool):
    try:
        try files.write_text(Path(\"open/out.txt\"), text, durable = durable)
    on IoError as e:
        if durable and e.kind == IoErrorKind.Unsupported:
            print(\"unsupported {{name}}\")
            return
        print(\"BREACH {{name}} does not work: {{e.reason}}\")
        return
    let back = try files.read_text(Path(\"open/out.txt\")) on IoError as e:
        print(\"BREACH {{name}} cannot be read back: {{e.reason}}\")
        return
    granted(name, back == text)

public function main():
    child(\"a child writes a neighbour\", \"echo changed > open/neighbour.txt\")
    child(\"a child appends to a neighbour\", \"echo changed >> open/neighbour.txt\")
    child(\"a child creates a file next to the target\", \"echo new > open/new.txt\")
    child(\"a child removes a neighbour\", \"rm -f open/neighbour.txt && test ! -e open/neighbour.txt\")
    child(\"a child renames a neighbour\", \"mv open/neighbour.txt open/moved.txt\")
    child(\"a child writes the target directly\", \"echo child > open/out.txt\")
    write_works(\"files.write_text\", \"one\", false)
    write_works(\"files.write_text durable\", \"two\", true)
    report(\"files.write_text of a neighbour\", write_text_denied(Path(\"open/neighbour.txt\")))
"
    );
    let manifest = "[permissions]\nfiles.read = [\"$CWD/open/**\"]\nfiles.write = [\"$CWD/open/out.txt\"]\nprocess = [\"sh\"]\n";
    let dir = package("neighbour", manifest, &program);
    fs::write(dir.join("open/neighbour.txt"), "neighbour").unwrap();
    let output = run(&dir, &[], &[]);
    assert_denied(
        &output,
        &[
            "a child writes a neighbour",
            "a child appends to a neighbour",
            "a child creates a file next to the target",
            "a child removes a neighbour",
            "a child renames a neighbour",
            "a child writes the target directly",
            "files.write_text",
            "files.write_text of a neighbour",
        ],
    );
    let out = text(&output.stdout);
    let durable = unio_runtime::externs::durability::supports_durability(&dir.join("open/out.txt"));
    if durable {
        assert!(out.contains("granted files.write_text durable"), "{out}");
    }
    assert_eq!(
        fs::read_to_string(dir.join("open/neighbour.txt")).unwrap(),
        "neighbour"
    );
    assert!(!dir.join("open/new.txt").exists() && !dir.join("open/moved.txt").exists());
    let expected = if durable { "two" } else { "one" };
    assert_eq!(
        fs::read_to_string(dir.join("open/out.txt")).unwrap(),
        expected
    );
    let names: Vec<String> = fs::read_dir(dir.join("open"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.contains("unio-tmp")),
        "temporary files are left: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The writer process itself, started from `unio` and driven through its protocol as a program
/// whose runtime checks were bypassed would drive it: it refuses every path outside the scopes
/// it was given, and it runs under its own restrictions (no new privileges, seccomp) with an
/// empty environment.
#[cfg(target_os = "linux")]
#[test]
fn the_writer_process_checks_the_scopes_itself() {
    use std::io::Write;
    use unio_runtime::sandbox::writer::{Failure, Handle};
    use unio_runtime::sandbox::Scope;
    let dir = package("writer-process", "[package]\nname = \"w\"\n", "");
    let target = dir.join("open/out.txt");
    let writer = Handle::start_with(
        Path::new(env!("CARGO_BIN_EXE_unio")),
        &[Scope {
            root: target.clone(),
            subtree: false,
        }],
    )
    .unwrap();
    let refused = |result: Result<(), Failure>| matches!(result, Err(Failure::Refused(_)));
    assert!(refused(writer.open(&dir.join("open/a.txt")).map(drop)));
    assert!(refused(writer.open(&dir.join("closed/x.txt")).map(drop)));
    assert!(refused(writer.make_directory(&dir.join("open/dir"))));
    assert!(refused(writer.rename(&dir.join("open/a.txt"), &target)));
    assert!(refused(writer.commit(1, &target)));
    let (id, _, mut file) = writer.open(&target).unwrap();
    file.write_all(b"written").unwrap();
    drop(file);
    writer.commit(id, &target).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "written");
    assert_eq!(fs::read_to_string(dir.join("open/a.txt")).unwrap(), "alpha");
    let pid = writer.pid().unwrap();
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    assert!(status.contains("NoNewPrivs:\t1"), "{status}");
    if status.contains("Seccomp:") {
        assert!(status.contains("Seccomp:\t2"), "{status}");
    }
    let environ = fs::read(format!("/proc/{pid}/environ")).unwrap();
    let names: Vec<String> = environ
        .split(|b| *b == 0)
        .filter(|v| !v.is_empty())
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    drop(writer);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    let _ = fs::remove_dir_all(&dir);
}

/// The writer ends with its program, even when the program is killed.
#[cfg(target_os = "linux")]
#[test]
fn the_writer_ends_with_its_program() {
    let program = "import std.files

public function main():
    try files.write_text(Path(\"open/out.txt\"), \"started\") on IoError as e:
        print(\"error {e.reason}\")
        return
    while true:
        pass
";
    let manifest = "[permissions]\nfiles.write = [\"$CWD/open/out.txt\"]\n";
    let dir = package("writer-ends", manifest, program);
    let mut child = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(&dir)
        .env_remove("UNIO_POLICY")
        .args(["run", "main.unio"])
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while !dir.join("open/out.txt").exists() {
        assert!(
            started.elapsed().as_secs() < 60 && child.try_wait().unwrap().is_none(),
            "the program did not write"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let parent = child.id().to_string();
    let writers: Vec<String> = fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let stat = fs::read_to_string(e.path().join("stat")).ok()?;
            let after = stat.rsplit_once(") ")?.1;
            let ppid = after.split(' ').nth(1)?;
            let environ = fs::read(e.path().join("environ")).ok()?;
            (ppid == parent && String::from_utf8_lossy(&environ).contains("UNIO_SANDBOX_WRITER"))
                .then(|| e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    assert_eq!(
        writers.len(),
        1,
        "one writer runs for the program: {writers:?}"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let started = std::time::Instant::now();
    while Path::new(&format!("/proc/{}/stat", writers[0])).exists()
        && !fs::read_to_string(format!("/proc/{}/stat", writers[0]))
            .unwrap_or_default()
            .contains(") Z ")
    {
        assert!(
            started.elapsed().as_secs() < 10,
            "the writer outlived its program"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(&dir);
}
