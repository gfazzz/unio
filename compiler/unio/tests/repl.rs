//! `unio repl` (T-13, docs/language-contracts.md section 5): every input is checked as a module
//! of the package with the diagnostics of `unio check`, runs in the sandbox of `unio run` with
//! the manifest and the policy, keeps its bindings by value, never runs an earlier input
//! again, and offers fixes bound to the revision of the input they were made for.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn scratch(name: &str, permissions: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-repl-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("unio.toml"),
        format!(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[permissions]\n{permissions}\n"
        ),
    )
    .unwrap();
    fs::write(dir.join("data.txt"), "secret\n").unwrap();
    dir
}

fn repl(dir: &Path, args: &[&str], lines: &[&str]) -> (String, String, Output) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .env_remove("UNIO_WORKER_PROBE")
        .arg("repl")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = lines.join("\n");
    input.push('\n');
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let out = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let err = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    (out, err, output)
}

#[test]
fn inputs_are_checked_run_once_and_keep_their_bindings_by_value() {
    let dir = scratch("session", "");
    let (out, err, output) = repl(
        &dir,
        &[],
        &[
            "1 + 2",
            "let x = 40",
            "print(\"once\")",
            "x + 2",
            "var items = [1, 2]",
            "items.append(x)",
            "items",
            "struct P:",
            "    a as Int",
            "",
            "let p = P(a = 3)",
            "p.a + x",
            "missing + 1",
            "let f = function (n as Int) returns Int: n + 1",
            ":type print(\"not run\")",
            ":type items.length()",
            "let ok = [1, 2]",
            "[1, 2][5]",
            "items",
            ":show",
        ],
    );
    assert!(output.status.success(), "{err}");
    assert!(out.contains("= 3 : Int"), "{out}");
    assert!(out.contains("= 42 : Int"), "{out}");
    // Mutations of a `var` binding are kept, and earlier inputs never run again.
    assert!(out.contains("= [1, 2, 40] : List[Int]"), "{out}");
    assert_eq!(out.matches("once").count(), 1, "{out}");
    assert!(out.contains("= 43 : Int"), "{out}");
    // The ordinary diagnostics, at the place of the input.
    assert!(
        out.contains("error[E0056]: unknown name 'missing'"),
        "{out}"
    );
    assert!(out.contains("--> input:1:1"), "{out}");
    assert!(out.contains("note: 'f' is not kept"), "{out}");
    // A type query runs nothing.
    assert!(out.contains("print(\"not run\") : nothing"), "{out}");
    assert!(!out.contains("not run\n"), "{out}");
    assert!(out.contains("items.length() : Int"), "{out}");
    // A fault ends the input; its place is a line of the input.
    assert!(err.contains("index 5 is outside 0..2"), "{err}");
    assert!(err.contains("(input:1)"), "{err}");
    assert!(out.contains("not undone"), "{out}");
    assert!(out.contains("var items as List[Int] = [1, 2, 40]"), "{out}");
    assert!(out.contains("let p as P = P(a = 3)"), "{out}");
    // The session module was never written into the package.
    assert!(!dir.join("unio_repl_session.unio").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_fix_applies_only_to_the_revision_it_was_made_for() {
    let dir = scratch("fix", "");
    let (out, _, output) = repl(
        &dir,
        &[],
        &[
            "enum C:",
            "    List",
            "    Show",
            "",
            "let c as C = List",
            ":fix 1",
            "c",
            "let d as C = List",
            "1 + 1",
            ":fix 1",
        ],
    );
    assert!(output.status.success());
    assert!(out.contains("fix 1 [r2]: write 'C.List'"), "{out}");
    // Applied at its revision: the input is checked again and runs.
    assert!(out.contains("let c as C = C.List\n"), "{out}");
    assert!(out.contains("= List : C"), "{out}");
    // After another input the fix of r5 is refused.
    assert!(
        out.contains("the fix belongs to revision r5, and the session is at r6"),
        "{out}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_manifest_and_the_policy_bound_what_an_input_may_do() {
    let read = [
        "import std.files",
        "from std.files import IoError",
        "try:",
        "    print(files.read_text(Path(\"data.txt\")))",
        "on IoError as e:",
        "    print(\"denied: {e.reason}\")",
        "",
    ];
    let dir = scratch("denied", "");
    let (out, _, _) = repl(&dir, &[], &read);
    assert!(out.contains("error[E0085]"), "{out}");
    let _ = fs::remove_dir_all(&dir);
    let dir = scratch("allowed", "files.read = [\"data.txt\"]");
    let (out, err, _) = repl(&dir, &[], &read);
    assert!(out.contains("secret"), "{out}{err}");
    // The policy narrows the manifest at run time: the sandbox refuses the read.
    fs::write(dir.join("policy.toml"), "[permissions]\nfiles.read = []\n").unwrap();
    let (out, err, _) = repl(&dir, &["--policy", "policy.toml"], &read);
    assert!(out.contains("denied: denied by the sandbox"), "{out}{err}");
    // A wall-clock limit stops an input that does not end; the session goes on.
    fs::write(dir.join("slow.toml"), "[limits]\nwall_seconds = 1\n").unwrap();
    let (out, err, _) = repl(
        &dir,
        &["--policy", "slow.toml"],
        &["var i = 0", "while true:", "    i = i + 1", "", "1 + 1"],
    );
    assert!(
        err.contains("stopped after 1 s") || err.contains("ended with code"),
        "{err}"
    );
    assert!(out.contains("= 2 : Int"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}
