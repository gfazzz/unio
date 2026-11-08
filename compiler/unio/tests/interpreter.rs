//! The interpreter of sprint S5 (docs/runtime.md, plan section 12): every `test` block of the
//! repository runs and passes under `unio test`, the reference utilities work end to end on
//! small inputs under `unio run`, and faults end programs with their codes (C-14, C-15, C-17,
//! C-18).

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{repo_root, unio_files};

fn unio() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command.current_dir(repo_root());
    allow_weaker_on_windows(&mut command);
    command
}

/// Windows has no sandbox of the operating system yet (SB-4), so `unio run` and `unio test`
/// refuse to start there (SB-5); the tests act as the operator and allow the weaker sandbox
/// through a policy.
fn allow_weaker_on_windows(command: &mut Command) {
    if cfg!(windows) {
        let policy = std::env::temp_dir().join(format!("unio-weaker-{}.toml", std::process::id()));
        fs::write(&policy, "allow_weaker = true\n").unwrap();
        command.env("UNIO_POLICY", policy);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-interpreter-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The files whose `test` blocks the exit criterion of S5 covers.
fn tested_sources() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in [
        "stdlib",
        "examples",
        "docs/examples",
        "docs/ru/examples",
        "tools",
    ] {
        files.extend(unio_files(&root.join(dir)));
    }
    files
}

#[test]
fn every_test_block_of_the_repository_runs_and_passes() {
    let files = tested_sources();
    let blocks: usize = files
        .iter()
        .map(|f| {
            common::read(f)
                .lines()
                .filter(|l| l.starts_with("test \""))
                .count()
        })
        .sum();
    assert!(blocks > 100, "only {blocks} test blocks found");
    let output = unio()
        .arg("test")
        .arg("--json")
        .args(&files)
        .output()
        .unwrap();
    let json = text(&output.stdout);
    let passed = json.matches("\"status\": \"passed\"").count();
    let failed: Vec<&str> = json
        .lines()
        .filter(|l| !l.contains("\"status\": \"passed\""))
        .filter(|l| l.contains("\"status\""))
        .collect();
    assert!(failed.is_empty(), "failed tests:\n{}", failed.join("\n"));
    assert_eq!(passed, blocks, "{json}");
    assert!(output.status.success());
}

#[test]
fn unio_test_reports_a_failing_test_and_a_test_that_does_not_compile() {
    let dir = scratch("failing");
    let file = dir.join("t.unio");
    fs::write(
        &file,
        "test \"passes\":\n    assert 1 + 1 == 2\n\ntest \"fails\":\n    let items = [1]\n    assert items.length() == 2\n\ntest \"does not compile\":\n    assert missing == 1\n",
    )
    .unwrap();
    let output = unio().arg("test").arg(&file).output().unwrap();
    let out = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{out}");
    assert!(out.contains("ok   "), "{out}");
    assert!(
        out.contains("assertion failed: items.length() == 2"),
        "{out}"
    );
    assert!(out.contains("the test does not compile"), "{out}");
    assert!(out.contains("1 passed, 2 failed"), "{out}");
    assert!(
        !text(&output.stderr).contains("without the sandbox"),
        "SB-1: tests run in the sandbox"
    );
    let filtered = unio().args(["test", "passes"]).arg(&file).output().unwrap();
    assert!(text(&filtered.stdout).contains("1 passed, 0 failed"));
    assert!(filtered.status.success());
    let _ = fs::remove_dir_all(&dir);
}

/// Runs a program with arguments in a directory and returns its output.
fn run(program: &Path, args: &[&str], dir: &Path, env: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(dir)
        .arg("run")
        .arg(program)
        .arg("--")
        .args(args);
    allow_weaker_on_windows(&mut command);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().unwrap()
}

#[track_caller]
fn expect(output: &Output, code: i32, stdout: &str) {
    let out = text(&output.stdout).replace('\\', "/");
    let err = text(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    assert_eq!(out, stdout, "stderr:\n{err}");
    assert!(!err.contains("without the sandbox"), "{err}");
}

#[test]
fn reference_utilities_work_end_to_end() {
    let root = repo_root();
    let example = |name: &str| root.join("examples").join(format!("{name}.unio"));
    let dir = scratch("utilities");
    fs::write(dir.join("a.txt"), "one two\nthree\n").unwrap();
    fs::write(dir.join("b.txt"), "alpha\nBeta gamma\n").unwrap();
    fs::write(
        dir.join("d.csv"),
        "name,amount,qty\nx,1.5,2\n\"y, z\",2.5,\n",
    )
    .unwrap();
    fs::write(
        dir.join("d.json"),
        "{\"users\": [{\"name\": \"Ann\", \"age\": 30}], \"ok\": true}",
    )
    .unwrap();
    fs::write(
        dir.join("app.log"),
        "2026-11-02T10:00:00Z INFO start\n2026-11-02T10:01:00Z WARN disk low\n2026-11-02T10:02:00Z ERROR disk full\nplain line\n",
    )
    .unwrap();

    let wc = run(&example("wc"), &["a.txt", "b.txt"], &dir, &[]);
    expect(&wc, 0, "2 3 14 a.txt\n2 3 17 b.txt\n4 6 31 total\n");
    expect(&run(&example("wc"), &[], &dir, &[]), 2, "");

    let grep = example("grep_lite");
    let found = run(
        &grep,
        &["--ignore-case", "beta", "a.txt", "b.txt"],
        &dir,
        &[],
    );
    expect(&found, 0, "b.txt:2: Beta gamma\n");
    expect(&run(&grep, &["zeta", "a.txt"], &dir, &[]), 1, "");

    let query = example("json_query");
    expect(
        &run(&query, &["users.0.name", "d.json"], &dir, &[]),
        0,
        "\"Ann\"\n",
    );
    expect(&run(&query, &["users.5", "d.json"], &dir, &[]), 3, "");

    let csv = example("csv_sum");
    let sums = run(&csv, &["d.csv", "amount", "qty"], &dir, &[]);
    expect(&sums, 0, "amount: 4.0 (2 values)\nqty: 2.0 (1 values)\n");

    let tail = example("log_tail");
    let warnings = run(&tail, &["--level", "WARN", "app.log"], &dir, &[]);
    expect(
        &warnings,
        0,
        "2026-11-02T10:01:00Z WARN disk low\n2026-11-02T10:02:00Z ERROR disk full\n",
    );
    expect(
        &run(&tail, &["--lines", "1", "app.log"], &dir, &[]),
        0,
        "plain line\n",
    );

    let rename = example("rename");
    let plan = run(&rename, &["--dry-run", ".txt", ".md", "a.txt"], &dir, &[]);
    expect(&plan, 0, "a.txt -> ./a.md\n");
    let conflict = run(
        &rename,
        &["--dry-run", "a", "b", "a.txt", "b.txt"],
        &dir,
        &[],
    );
    expect(&conflict, 1, "");
    assert!(dir.join("a.txt").exists() && !dir.join("a.md").exists());

    // The task list keeps its tasks where TODO_FILE says; its package permits that variable
    // and the files of the directory (SB-2).
    let package = dir.join("todo");
    fs::create_dir_all(&package).unwrap();
    let todo = package.join("todo.unio");
    fs::copy(root.join("docs/examples/todo.unio"), &todo).unwrap();
    fs::write(
        package.join("unio.toml"),
        "[permissions]\nfiles.read = [\"$CWD/**\"]\nfiles.write = [\"$CWD/**\"]\nenv = [\"TODO_FILE\"]\n",
    )
    .unwrap();
    let store = dir.join("todo.json");
    let env = [("TODO_FILE", store.as_path())];
    expect(
        &run(&todo, &["add", "buy bread"], &dir, &env),
        0,
        "Added: buy bread\n",
    );
    expect(
        &run(&todo, &["add", "milk"], &dir, &env),
        0,
        "Added: milk\n",
    );
    expect(
        &run(&todo, &["done", "1"], &dir, &env),
        0,
        "Done: buy bread\n",
    );
    let list = run(&todo, &["list"], &dir, &env);
    expect(&list, 0, "[x] 1. buy bread\n[ ] 2. milk\n");
    let bad = run(&todo, &["done", "x"], &dir, &env);
    expect(&bad, 2, "");
    assert!(text(&bad.stderr).contains("Error: task number must be an integer: x"));
    assert!(common::read(&store).contains("\"title\": \"milk\""));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn faults_release_resources_and_end_with_their_codes() {
    let dir = scratch("faults");
    let fault = dir.join("fault.unio");
    fs::write(
        &fault,
        "struct Log:\n    name as Text\n\nextend Log with Resource:\n    function release(var self):\n        print(\"released {self.name}\")\n\nfunction open(name as Text) returns Log:\n    return Log(name = name)\n\nfunction divide(a as Int, b as Int) returns Int:\n    return a / b\n\npublic function main() returns Int:\n    using first = open(\"first\"):\n        using second = open(\"second\"):\n            return divide(1, 0)\n",
    )
    .unwrap();
    let output = run(&fault, &[], &dir, &[]);
    // C-17: open `using` blocks are released in reverse order before the program ends.
    expect(&output, 101, "released second\nreleased first\n");
    let err = text(&output.stderr);
    assert!(err.contains("fault: division by zero"), "{err}");
    // C-14: the trace names functions, files and lines.
    assert!(
        err.contains("at fault.divide (") && err.contains("fault.unio:12)"),
        "{err}"
    );
    assert!(
        err.contains("at fault.main (") && err.contains("fault.unio:17)"),
        "{err}"
    );

    let code = dir.join("code.unio");
    fs::write(
        &code,
        "import std.args\n\npublic function main() returns Int:\n    return args.list()[0].to_int() or 0\n",
    )
    .unwrap();
    expect(&run(&code, &["7"], &dir, &[]), 7, "");
    // C-15: an exit code outside 0–100 is a fault.
    let outside = run(&code, &["101"], &dir, &[]);
    expect(&outside, 101, "");
    assert!(text(&outside.stderr).contains("main returned 101"));

    // C-18: recursion deeper than the stack limit ends with code 102.
    let deep = dir.join("deep.unio");
    fs::write(
        &deep,
        "function down(n as Int) returns Int:\n    return down(n + 1) + 1\n\npublic function main() returns Int:\n    return down(0)\n",
    )
    .unwrap();
    expect(&run(&deep, &[], &dir, &[]), 102, "");

    // A program that does not compile does not run.
    let broken = dir.join("broken.unio");
    fs::write(
        &broken,
        "public function main() returns Int:\n    return missing\n",
    )
    .unwrap();
    let output = run(&broken, &[], &dir, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("E0056"));
    let _ = fs::remove_dir_all(&dir);
}

/// What `tests/programs/same_names/main.unio` prints: every call reaches the method, the
/// implementation, the handler and the variant of its own module's type.
const SAME_NAMES_OUTPUT: &str = "trees node 1\ngraphs node 2\ntrees label 1\ngraphs label 2\ncaught graphs.Problem Problem(reason = g)\nown trees.Problem\nother graphs.Problem\ntrees inner 3\ngraphs leaf\ngraphs inner 4\n";

/// Two modules each declare `Node`, `Problem` and `Mark` with methods of the same names and an
/// implementation of one interface: a type is known by its module and name at run time, not by
/// its short name, so neither module's methods replace the other's.
#[test]
fn types_of_one_name_in_two_modules_keep_their_own_methods() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs/same_names");
    let main = dir.join("main.unio");
    expect(&run(&main, &[], &dir, &[]), 0, SAME_NAMES_OUTPUT);
    let output = unio().arg("test").arg(&main).output().unwrap();
    let out = text(&output.stdout);
    assert!(out.contains("1 passed, 0 failed"), "{out}");
    assert!(output.status.success(), "{out}");
}

/// S-17: the run-time conversion never sets a private field, even in a program whose check
/// reported E0046 and was run anyway; decoding fails with DecodeError instead (gap 16).
#[test]
fn decoding_never_fills_a_private_field() {
    let dir = scratch("decode-private");
    let file = dir.join("account.unio");
    fs::write(
        &file,
        "import std.json as json\n\n\
         struct Account:\n    public owner as Text\n    balance as Int\n\n\
         test \"a private field is not decoded\":\n    \
         let value = try json.parse(\"\\{\\\"owner\\\":\\\"Ann\\\",\\\"balance\\\":5\\}\") on ParseError as e:\n        \
         assert_fail(e.to_text())\n    \
         try:\n        \
         let account = try json.decode[Account](value)\n        \
         assert_fail(\"a private field was decoded\")\n    \
         on DecodeError as e: assert e.message.contains(\"private\")\n",
    )
    .unwrap();
    let results = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let program = unio::load_checked(
                std::slice::from_ref(&file),
                unio::effects::Profile::Application,
            );
            assert!(
                program.diagnostics.iter().any(|(_, d)| d.code == "E0046"),
                "the check reports E0046"
            );
            let module = program.modules.iter().position(|m| m.report).unwrap();
            let interp = unio::interp::Interp::new(program);
            let tests: Vec<usize> = interp.program.modules[module]
                .analysis
                .file
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| matches!(item, unio::ast::Item::Test(_)))
                .map(|(i, _)| i)
                .collect();
            tests
                .into_iter()
                .map(|i| {
                    let r = unio::interp::run_test(&interp, module, i);
                    (r.passed, r.message)
                })
                .collect::<Vec<_>>()
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].0, "{}", results[0].1);
}

/// S-7: an external function outside the standard library stops `unio run` at the check
/// with E0086, before anything runs; it never reaches a fault at the call (gap 17).
#[test]
fn an_external_function_of_a_program_is_rejected_before_it_runs() {
    let dir = scratch("external");
    let file = dir.join("poke.unio");
    fs::write(
        &file,
        "unsafe module\n\nexternal function poke(address as Int) returns Int\n\n\
         function main():\n    print(\"{poke(1)}\")\n",
    )
    .unwrap();
    let output = run(&file, &[], &dir, &[]);
    let err = text(&output.stderr);
    assert_ne!(output.status.code(), Some(101), "{err}");
    assert_ne!(output.status.code(), Some(0), "{err}");
    assert!(err.contains("E0086"), "{err}");
    assert!(
        !err.contains("the runtime has no external function"),
        "{err}"
    );
}
