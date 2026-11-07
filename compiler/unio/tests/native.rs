//! Native code of sprint S6 (docs/runtime.md 8.4, plan section 12): `unio build` makes one
//! executable (C-5) that gives the same standard output, standard error, and exit code as
//! `unio run` for every reference program the interpreter tests run, including faults with
//! their traces (C-14, C-15, C-17, C-18); two builds of one source are byte for byte the same
//! (C-4); and a program that does not compile is not built.
//!
//! Executables are made for Linux on x86-64 only so far; elsewhere these tests are skipped
//! with a note.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::repo_root;

fn supported() -> bool {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return true;
    }
    eprintln!("skipped: native executables are made for Linux on x86-64 only so far");
    false
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-native-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds a program with `--release` into `dir`; returns the executable.
#[track_caller]
fn build(program: &Path, dir: &Path, release: bool) -> PathBuf {
    let stem = program.file_stem().unwrap().to_string_lossy().into_owned();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let out = bin.join(format!("{stem}{}", if release { "" } else { "-debug" }));
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command.current_dir(repo_root()).arg("build");
    if release {
        command.arg("--release");
    }
    let output = command.arg(program).arg("-o").arg(&out).output().unwrap();
    assert!(
        output.status.success(),
        "unio build {}:\n{}",
        program.display(),
        text(&output.stderr)
    );
    out
}

/// One run of a program: arguments, directory, and environment.
struct Case {
    program: PathBuf,
    args: Vec<String>,
    dir: PathBuf,
    env: Vec<(String, PathBuf)>,
    /// Files to reset before each of the two runs, so that both start from the same state.
    reset: Vec<(PathBuf, Option<String>)>,
}

fn case(program: PathBuf, args: &[&str], dir: &Path) -> Case {
    Case {
        program,
        args: args.iter().map(|a| a.to_string()).collect(),
        dir: dir.to_path_buf(),
        env: Vec::new(),
        reset: Vec::new(),
    }
}

fn reset(case: &Case) {
    for (path, contents) in &case.reset {
        match contents {
            Some(c) => fs::write(path, c).unwrap(),
            None => {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn interpreted(case: &Case) -> Output {
    reset(case);
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(&case.dir)
        .env_remove("UNIO_POLICY")
        .arg("run")
        .arg(&case.program)
        .arg("--")
        .args(&case.args);
    for (k, v) in &case.env {
        command.env(k, v);
    }
    command.output().unwrap()
}

fn native(exe: &Path, case: &Case) -> Output {
    reset(case);
    let mut command = Command::new(exe);
    command
        .current_dir(&case.dir)
        .env_remove("UNIO_POLICY")
        .args(&case.args);
    for (k, v) in &case.env {
        command.env(k, v);
    }
    command.output().unwrap()
}

/// Runs every case under the interpreter and natively and lists the differences.
fn compare(cases: &[Case], dir: &Path) -> Vec<String> {
    let mut built: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut differences = Vec::new();
    for c in cases {
        let exe = match built.iter().find(|(p, _)| *p == c.program) {
            Some((_, exe)) => exe.clone(),
            None => {
                let exe = build(&c.program, dir, true);
                built.push((c.program.clone(), exe.clone()));
                exe
            }
        };
        let expected = interpreted(c);
        assert!(
            !text(&expected.stderr).contains("error[E"),
            "{} does not compile:\n{}",
            c.program.display(),
            text(&expected.stderr)
        );
        let found = native(&exe, c);
        if expected.status.code() != found.status.code()
            || expected.stdout != found.stdout
            || expected.stderr != found.stderr
        {
            differences.push(format!(
                "{} {:?}: interpreter exit {:?}, native exit {:?}\n--- interpreter stdout\n{}--- native stdout\n{}--- interpreter stderr\n{}--- native stderr\n{}",
                c.program.display(),
                c.args,
                expected.status.code(),
                found.status.code(),
                text(&expected.stdout),
                text(&found.stdout),
                text(&expected.stderr),
                text(&found.stderr)
            ));
        }
    }
    differences
}

#[test]
fn reference_utilities_match_the_interpreter() {
    if !supported() {
        return;
    }
    let root = repo_root();
    let example = |name: &str| root.join("examples").join(format!("{name}.unio"));
    let dir = scratch("utilities");
    let work = dir.join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("a.txt"), "one two\nthree\n").unwrap();
    fs::write(work.join("b.txt"), "alpha\nBeta gamma\n").unwrap();
    fs::write(
        work.join("d.csv"),
        "name,amount,qty\nx,1.5,2\n\"y, z\",2.5,\n",
    )
    .unwrap();
    fs::write(
        work.join("d.json"),
        "{\"users\": [{\"name\": \"Ann\", \"age\": 30}], \"ok\": true}",
    )
    .unwrap();
    fs::write(
        work.join("app.log"),
        "2026-11-02T10:00:00Z INFO start\n2026-11-02T10:01:00Z WARN disk low\n2026-11-02T10:02:00Z ERROR disk full\nplain line\n",
    )
    .unwrap();
    let mut cases = vec![
        case(example("wc"), &["a.txt", "b.txt"], &work),
        case(example("wc"), &[], &work),
        case(example("wc"), &["missing.txt"], &work),
        case(
            example("grep_lite"),
            &["--ignore-case", "beta", "a.txt", "b.txt"],
            &work,
        ),
        case(example("grep_lite"), &["zeta", "a.txt"], &work),
        case(example("json_query"), &["users.0.name", "d.json"], &work),
        case(example("json_query"), &["users.5", "d.json"], &work),
        case(example("csv_sum"), &["d.csv", "amount", "qty"], &work),
        case(example("log_tail"), &["--level", "WARN", "app.log"], &work),
        case(example("log_tail"), &["--lines", "1", "app.log"], &work),
        case(
            example("rename"),
            &["--dry-run", ".txt", ".md", "a.txt"],
            &work,
        ),
        case(
            example("rename"),
            &["--dry-run", "a", "b", "a.txt", "b.txt"],
            &work,
        ),
    ];
    // Every reference program without arguments: its usage message and code.
    for entry in fs::read_dir(root.join("examples")).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "unio") {
            cases.push(case(path, &[], &work));
        }
    }
    // The task list keeps its tasks where TODO_FILE says (SB-2).
    let package = dir.join("todo");
    fs::create_dir_all(&package).unwrap();
    let todo = package.join("todo.unio");
    fs::copy(root.join("docs/examples/todo.unio"), &todo).unwrap();
    fs::write(
        package.join("unio.toml"),
        "[permissions]\nfiles.read = [\"$CWD/**\"]\nfiles.write = [\"$CWD/**\"]\nenv = [\"TODO_FILE\"]\n",
    )
    .unwrap();
    let store = work.join("todo.json");
    let before_done = "[{\"id\": 1, \"title\": \"buy bread\", \"done\": false}, {\"id\": 2, \"title\": \"milk\"}]";
    for (args, start) in [
        (&["add", "buy bread"][..], None),
        (&["list"][..], Some(before_done)),
        (&["done", "1"][..], Some(before_done)),
        (&["done", "x"][..], Some(before_done)),
    ] {
        let mut c = case(todo.clone(), args, &work);
        c.env.push(("TODO_FILE".into(), store.clone()));
        c.reset.push((store.clone(), start.map(String::from)));
        cases.push(c);
    }
    let differences = compare(&cases, &dir);
    assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    assert!(common::read(&store).contains("milk"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn faults_and_exit_codes_match_the_interpreter() {
    if !supported() {
        return;
    }
    let dir = scratch("faults");
    let fault = dir.join("fault.unio");
    fs::write(
        &fault,
        "struct Log:\n    name as Text\n\nextend Log with Resource:\n    function release(var self):\n        print(\"released {self.name}\")\n\nfunction open(name as Text) returns Log:\n    return Log(name = name)\n\nfunction divide(a as Int, b as Int) returns Int:\n    return a / b\n\npublic function main() returns Int:\n    using first = open(\"first\"):\n        using second = open(\"second\"):\n            return divide(1, 0)\n",
    )
    .unwrap();
    let code = dir.join("code.unio");
    fs::write(
        &code,
        "import std.args\n\npublic function main() returns Int:\n    return args.list()[0].to_int() or 0\n",
    )
    .unwrap();
    let deep = dir.join("deep.unio");
    fs::write(
        &deep,
        "function down(n as Int) returns Int:\n    return down(n + 1) + 1\n\npublic function main() returns Int:\n    return down(0)\n",
    )
    .unwrap();
    // Every kind of integer fault, an assertion, an error that reaches the top, a missing
    // key and an index outside the list, each chosen by the argument.
    let kinds = dir.join("kinds.unio");
    fs::write(
        &kinds,
        "import std.args\n\nerror Broken:\n    reason as Text\n\nfunction broken() returns Int fails with Broken:\n    fail Broken(reason = \"on purpose\")\n\nfunction pick(which as Text) returns Int fails with Broken:\n    let big = 9223372036854775807\n    let items = [1, 2, 3]\n    let table = {\"a\": 1}\n    if which == \"add\": return big + 1\n    if which == \"sub\": return -big - 2\n    if which == \"mul\": return big * 2\n    if which == \"div\": return 1 / (items.length() - 3)\n    if which == \"rem\": return 1 % (items.length() - 3)\n    if which == \"neg\": return -(-big - 1)\n    if which == \"pow\": return 2 ** 64\n    if which == \"index\": return items[5]\n    if which == \"key\": return table[\"b\"]\n    if which == \"assert\":\n        assert items.length() == 2\n    if which == \"error\": return try broken()\n    return 0\n\npublic function main() returns Int:\n    let which = args.list()[0]\n    let value = try pick(which) on Broken as e:\n        print(\"handled {e.reason}\")\n        return 3\n    print(\"value {value}\")\n    return 0\n",
    )
    .unwrap();
    let mut cases = vec![
        case(fault, &[], &dir),
        case(code.clone(), &["7"], &dir),
        case(code.clone(), &["101"], &dir),
        case(code, &["x"], &dir),
        case(deep, &[], &dir),
    ];
    for which in [
        "add", "sub", "mul", "div", "rem", "neg", "pow", "index", "key", "assert", "error", "none",
    ] {
        cases.push(case(kinds.clone(), &[which], &dir));
    }
    let differences = compare(&cases, &dir);
    assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn benchmarks_and_tools_match_the_interpreter() {
    if !supported() {
        return;
    }
    let root = repo_root();
    let dir = scratch("bench");
    let data = dir.join("data");
    fs::create_dir_all(&data).unwrap();
    let mut words = String::new();
    for i in 0..2000 {
        words.push_str(&format!(
            "the quick brown fox {i} мир текст jumps over the lazy dog\n"
        ));
    }
    fs::write(data.join("text.txt"), &words).unwrap();
    let mut json = String::from("[");
    for i in 0..300 {
        if i > 0 {
            json.push_str(",\n");
        }
        json.push_str(&format!(
            "{{\"id\": {i}, \"name\": \"item {i} мир\", \"note\": \"tab\\there \\\"q\\\"\", \"score\": {}, \"active\": {}, \"parent\": null, \"tags\": [\"a\", \"b\"], \"point\": {{\"x\": {i}, \"y\": -{i}}}, \"ratio\": 1.5}}",
            i * 37 - 5000,
            i % 2 == 0
        ));
    }
    json.push(']');
    fs::write(data.join("doc.txt"), &json).unwrap();
    let bench = |name: &str, file: &str| root.join("bench").join(name).join(file);
    let mut cases = vec![
        case(bench("wc", "wc.unio"), &["text.txt"], &data),
        case(bench("text", "text.unio"), &["text.txt"], &data),
        case(bench("json", "json.unio"), &["doc.txt"], &data),
        case(bench("json", "json.unio"), &["text.txt"], &data),
        case(bench("loop", "loop.unio"), &[], &data),
    ];
    // The project checkers of tools/, run from the root of the repository as CI runs them.
    let mut sources: Vec<String> = Vec::new();
    for d in ["docs/examples", "examples", "stdlib", "tools"] {
        for f in common::unio_files(&root.join(d)) {
            sources.push(
                f.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    let tool = |name: &str| root.join("tools").join(name);
    cases.push(case(tool("check_docs.unio"), &[], &root));
    cases.push(case(tool("check_grammar.unio"), &["--corpus"], &root));
    let refs: Vec<&str> = sources.iter().map(String::as_str).collect();
    cases.push(case(tool("check_grammar.unio"), &refs, &root));
    cases.push(case(tool("check_api.unio"), &[], &root));
    let differences = compare(&cases, &dir);
    assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn two_builds_are_byte_for_byte_the_same() {
    if !supported() {
        return;
    }
    let root = repo_root();
    let dir = scratch("deterministic");
    for (program, release) in [
        (root.join("bench/loop/loop.unio"), true),
        (root.join("examples/json_query.unio"), true),
        (root.join("examples/json_query.unio"), false),
    ] {
        let first = fs::read(build(&program, &dir, release)).unwrap();
        let again = fs::read(build(&program, &dir, release)).unwrap();
        assert!(
            first == again,
            "{}: two builds differ (release {release})",
            program.display()
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_program_that_does_not_compile_is_not_built() {
    if !supported() {
        return;
    }
    let dir = scratch("broken");
    let broken = dir.join("broken.unio");
    fs::write(
        &broken,
        "public function main() returns Int:\n    return missing\n",
    )
    .unwrap();
    let out = dir.join("broken");
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .arg("build")
        .arg(&broken)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("E0056"),
        "{}",
        text(&output.stderr)
    );
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_executable_applies_the_manifest_of_its_package() {
    if !supported() {
        return;
    }
    // The program may read only open/; the executable keeps that permission from the build
    // and enforces it at run time, as `unio run` does (SB-1, SB-6).
    let dir = scratch("sandbox");
    fs::create_dir_all(dir.join("open")).unwrap();
    fs::create_dir_all(dir.join("closed")).unwrap();
    fs::write(dir.join("open/a.txt"), "alpha").unwrap();
    fs::write(dir.join("closed/secret.txt"), "SECRET").unwrap();
    fs::write(
        dir.join("unio.toml"),
        "[permissions]\nfiles.read = [\"$CWD/open/**\"]\n",
    )
    .unwrap();
    let program = dir.join("main.unio");
    fs::write(
        &program,
        "import std.files\n\nfunction show(name as Text):\n    let text = try files.read_text(Path(name)) on IoError as e:\n        print(\"denied {name}: {e.reason}\")\n        return\n    print(\"read {name}: {text}\")\n\npublic function main():\n    show(\"open/a.txt\")\n    show(\"closed/secret.txt\")\n",
    )
    .unwrap();
    let cases = vec![case(program, &[], &dir)];
    let differences = compare(&cases, &dir);
    assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    let exe = dir.join("bin").join("main");
    let output = Command::new(&exe).current_dir(&dir).output().unwrap();
    let out = text(&output.stdout);
    assert!(out.contains("read open/a.txt: alpha"), "{out}");
    assert!(!out.contains("SECRET"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}
