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
        runtime_library();
        return true;
    }
    eprintln!("skipped: native executables are made for Linux on x86-64 only so far");
    false
}

/// `unio build` links with `libunio.a` next to the compiler. `cargo test` builds only the rlib
/// of the crate, so the static library is brought up to date here once, with the profile of
/// the tests: a stale one would refuse the images of this compiler (code 104).
fn runtime_library() {
    static BUILT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        let exe = Path::new(env!("CARGO_BIN_EXE_unio"));
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["build", "-p", "unio", "--lib"]);
        if exe.parent().and_then(|d| d.file_name()) == Some("release".as_ref()) {
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

/// The fast paths of the code generator: lines stored only before calls, the depth of the
/// stack in a register, operations without an overflow check where the bounds of a counted
/// loop or of a tested parameter keep them in range, and leading `if c: return v` clauses
/// decided at the call. Each keeps the faults, traces, and exit codes of `unio run`: a fault
/// deep in recursion, overflow of `i * i` in a loop whose bounds do not keep it in range, `%`
/// and `/` by negative constants and by -1 at the minimum, a fault after the body of a `while`
/// (the line of the last statement that ran), a fault in a handler, and the depth limit
/// reached by a call whose leading clause would return at once.
#[test]
fn fast_paths_keep_faults_and_traces_exact() {
    if !supported() {
        return;
    }
    let dir = scratch("paths");
    let program = dir.join("paths.unio");
    fs::write(
        &program,
        r#"import std.args

function fall(n as Int) returns Int:
    if n < 1: return 0
    let step = 100 / (n - 25)
    return fall(n - 1) + step

function edge(n as Int) returns Int:
    if n >= 9999: return n
    return edge(n + 1)

function over(n as Int) returns Int:
    if n >= 10000: return n
    return over(n + 1)

function sink(n as Int) returns Int:
    if n < 0: return 0
    return sink(n + 1) + 1

function check(n as Int) returns Bool:
    return 10 / (n - 3) != 7

function squares(start as Int) returns Int:
    var total = 0
    for i in range(3037000490, 3037000510):
        total = total + i * i % 7
    return total + start

function divisions() returns Int:
    var total = 0
    for i in range(-7, 8):
        print("{i / 2} {i % 2} {i / -2} {i % -2} {i % 3} {i / -3} {i % -1} {i / -1} {i * i % 5}")
        total = total + i * i
    for i in range(0, 20):
        print("{i % 7} {i / 7} {i % 8} {i / 8} {i / 1} {i * 1000003 % 1000000007} {i / 3}")
    for i in range(9223372036854775800, 9223372036854775807):
        print("{i % 1000000007} {i / 10} {i % 9223372036854775807} {i / 4611686018427387904}")
    for i in range(-9223372036854775807, -9223372036854775805):
        let m = i - 1
        print("{m % -1} {m % 7} {m % -7} {m / 7} {m / -7}")
        print("{m / -1}")
    return total

function negative(n as Int) returns Int:
    if n > -3: return n * 2
    if n == -5: return 1 / (n + 5)
    return negative(n + 1) - 1

error Bad:
    code as Int

function risky(n as Int) returns Int fails with Bad:
    if n > 2: fail Bad(code = n)
    return n

public function main() returns Int:
    let which = args.list()[0]
    if which == "fall": return fall(40)
    if which == "edge": return edge(1) % 256
    if which == "over": return over(1)
    if which == "sink": return sink(0)
    if which == "squares": return squares(0)
    if which == "divisions": return divisions()
    if which == "negative": return negative(-4) + negative(-9)
    if which == "handled":
        let v = try risky(5) on Bad as e:
            print("caught {e.code}")
            return fall(30)
        return v
    if which == "while":
        var n = 0
        while check(n):
            n = n + 1
            print("{n}")
    return 0
"#,
    )
    .unwrap();
    // Each case and the exit code it must end with, so that a case cannot pass by not
    // reaching its fault.
    let expected = [
        ("fall", 101),
        ("edge", 15),
        ("over", 102),
        ("sink", 102),
        ("squares", 101),
        ("divisions", 101),
        ("negative", 101),
        ("handled", 101),
        ("while", 101),
    ];
    let cases: Vec<Case> = expected
        .iter()
        .map(|(which, _)| case(program.clone(), &[which], &dir))
        .collect();
    let differences = compare(&cases, &dir);
    assert!(differences.is_empty(), "{}", differences.join("\n\n"));
    let exe = dir.join("bin").join("paths");
    for (c, (which, code)) in cases.iter().zip(expected) {
        let found = native(&exe, c);
        assert_eq!(
            found.status.code(),
            Some(code),
            "{which}: {}",
            text(&found.stderr)
        );
    }
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

/// Types of one name in two modules: the executable finds methods, implementations, handlers
/// and variants by the identity of the type, as the interpreter does.
#[test]
fn types_of_one_name_in_two_modules_match_the_interpreter() {
    if !supported() {
        return;
    }
    let dir = scratch("same-names");
    let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs/same_names/main.unio");
    let exe = build(&program, &dir, true);
    let output = native(&exe, &case(program.clone(), &[], &dir));
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "trees node 1\ngraphs node 2\ntrees label 1\ngraphs label 2\ncaught graphs.Problem Problem(reason = g)\nown trees.Problem\nother graphs.Problem\ntrees inner 3\ngraphs leaf\ngraphs inner 4\n"
    );
    let differences = compare(&[case(program, &[], &dir)], &dir);
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

/// A package of two modules with a manifest, written into `dir`: `main.unio` imports `shapes`
/// and ends with a division by zero inside `shapes` when it gets the argument `fault`.
fn write_package(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir.join("geometry")).unwrap();
    fs::write(
        dir.join("unio.toml"),
        "[package]\nname = \"portable\"\nversion = \"0.1.0\"\n\n[permissions]\nfiles.read = [\"$CWD/open/**\"]\n",
    )
    .unwrap();
    fs::write(
        dir.join("geometry/shapes.unio"),
        "public struct Square:\n    public side as Int\n\npublic function area(s as Square) returns Int:\n    return s.side * s.side\n\npublic function ratio(a as Int, b as Int) returns Int:\n    return a / b\n",
    )
    .unwrap();
    let main = dir.join("main.unio");
    fs::write(
        &main,
        "import std.args\nimport std.files\nimport geometry.shapes\n\nfunction show(name as Text):\n    let text = try files.read_text(Path(name)) on IoError as e:\n        print(\"denied {name}: {e.reason}\")\n        return\n    print(\"read {name}: {text}\")\n\npublic function main() returns Int:\n    let given = args.list()\n    if given.length() > 0 and given[0] == \"fault\":\n        return shapes.ratio(1, given.length() - 1)\n    print(\"area {shapes.area(shapes.Square(side = 3))}\")\n    show(\"open/a.txt\")\n    show(\"closed/secret.txt\")\n    return 0\n",
    )
    .unwrap();
    main
}

/// `unio build` run from `cwd` with its own HOME and TMPDIR.
#[track_caller]
fn build_in(cwd: &Path, program: &str, out: &Path, home: &Path) -> Output {
    let temp = home.join("tmp");
    fs::create_dir_all(&temp).unwrap();
    Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(cwd)
        .env("HOME", home)
        .env("TMPDIR", &temp)
        .env_remove("UNIO_POLICY")
        .args(["build", "--release", program, "-o"])
        .arg(out)
        .output()
        .unwrap()
}

/// C-4 across places: the same project copied into two directories and built there, with a
/// different home and temporary directory, gives the same bytes; the image holds no path of
/// the machine that built it.
#[test]
fn builds_in_two_directories_are_byte_for_byte_the_same() {
    if !supported() {
        return;
    }
    let dir = scratch("portable-build");
    let mut built = Vec::new();
    for name in ["first-place", "second/deeper/place"] {
        let project = dir.join(name).join("project");
        write_package(&project);
        let home = dir.join(name).join("home");
        let out = dir.join(name).join("program");
        let output = build_in(&project, "main.unio", &out, &home);
        assert!(output.status.success(), "{}", text(&output.stderr));
        let bytes = fs::read(&out).unwrap();
        let place = project.to_string_lossy().into_owned();
        assert!(
            !bytes.windows(place.len()).any(|w| w == place.as_bytes()),
            "the executable holds the directory of its build {place}"
        );
        built.push(bytes);
    }
    assert!(
        built[0] == built[1],
        "builds of one project in two directories differ"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Traces name files relative to the root of their package, natively and in the interpreter.
#[test]
fn traces_show_paths_relative_to_the_package() {
    if !supported() {
        return;
    }
    let dir = scratch("relative-traces");
    let project = dir.join("project");
    let main = write_package(&project);
    let exe = build(&main, &dir, true);
    let expected = "fault: division by zero\n    at geometry.shapes.ratio (geometry/shapes.unio:8)\n    at main.main (main.unio:14)\n";
    let found = Command::new(&exe)
        .current_dir(&project)
        .arg("fault")
        .output()
        .unwrap();
    assert_eq!(found.status.code(), Some(101));
    assert_eq!(text(&found.stderr), expected);
    let interpreted = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(&dir)
        .args(["run", "project/main.unio", "--", "fault"])
        .output()
        .unwrap();
    assert_eq!(interpreted.status.code(), Some(101));
    assert_eq!(text(&interpreted.stderr), expected);
    // A module of the standard library is named by its logical root.
    let library = dir.join("library.unio");
    fs::write(
        &library,
        "public function main():\n    let items as List[Int] = []\n    print(\"{items.first() or 0}\")\n    print(\"{[1, 2][5]}\")\n",
    )
    .unwrap();
    let exe = build(&library, &dir, true);
    let found = Command::new(&exe).current_dir(&dir).output().unwrap();
    let err = text(&found.stderr);
    assert!(
        err.contains("(library.unio:4)") && !err.contains(&*dir.to_string_lossy()),
        "{err}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// An executable moved to another directory, with its project removed, still runs: `$CWD` is
/// the working directory of the run, so it reads what its manifest permits there and nothing
/// else. A manifest with a relative entry, which only has a meaning next to the manifest, is
/// refused by the build (docs/runtime.md 8.4).
#[test]
fn a_moved_executable_runs_with_the_permissions_of_its_manifest() {
    if !supported() {
        return;
    }
    let dir = scratch("moved");
    let project = dir.join("project");
    let main = write_package(&project);
    let exe = build(&main, &dir, true);
    let installed = dir.join("installed").join("bin");
    fs::create_dir_all(&installed).unwrap();
    let moved = installed.join("portable");
    fs::rename(&exe, &moved).unwrap();
    fs::remove_dir_all(&project).unwrap();
    let work = dir.join("work");
    fs::create_dir_all(work.join("open")).unwrap();
    fs::create_dir_all(work.join("closed")).unwrap();
    fs::write(work.join("open/a.txt"), "alpha").unwrap();
    fs::write(work.join("closed/secret.txt"), "SECRET").unwrap();
    let output = Command::new(&moved)
        .current_dir(&work)
        .env_remove("UNIO_POLICY")
        .output()
        .unwrap();
    let out = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(out.starts_with("area 9\nread open/a.txt: alpha\n"), "{out}");
    assert!(out.contains("denied closed/secret.txt"), "{out}");
    assert!(!out.contains("SECRET"), "{out}");

    let relative = dir.join("relative");
    let main = write_package(&relative);
    fs::write(
        relative.join("unio.toml"),
        "[permissions]\nfiles.read = [\"data/**\"]\n",
    )
    .unwrap();
    let out = dir.join("relative-program");
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .args(["build"])
        .arg(&main)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let err = text(&output.stderr);
    assert!(err.contains("'data/**'") && err.contains("$CWD"), "{err}");
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&dir);
}

/// An executable whose program image is of another format, or was made by another compiler,
/// is refused before anything runs, with code 104 and a message saying why.
#[test]
fn an_incompatible_image_is_refused() {
    if !supported() {
        return;
    }
    let dir = scratch("incompatible");
    let program = dir.join("hello.unio");
    fs::write(&program, "public function main():\n    print(\"hello\")\n").unwrap();
    let exe = build(&program, &dir, true);
    let bytes = fs::read(&exe).unwrap();
    let magic = b"UNIOPRG\0";
    // The image: the mark, four bytes of the format, and the identity of the compiler. The
    // mark alone also occurs in the machine code that compares it.
    let at = (0..bytes.len().saturating_sub(magic.len() + 10))
        .find(|&i| {
            bytes[i..].starts_with(magic) && bytes[i + magic.len() + 5..].starts_with(b"unio ")
        })
        .expect("the executable holds a program image");
    let run = |bytes: &[u8], name: &str| {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Command::new(&path).current_dir(&dir).output().unwrap()
    };
    let fine = run(&bytes, "fine");
    assert_eq!(text(&fine.stdout), "hello\n");
    // The format version follows the magic.
    let mut other_format = bytes.clone();
    other_format[at + magic.len()] ^= 0x7f;
    let output = run(&other_format, "other-format");
    assert_eq!(output.status.code(), Some(104));
    assert_eq!(text(&output.stdout), "");
    let err = text(&output.stderr);
    assert!(err.contains("format"), "{err}");
    // Then the identity of the compiler that made it.
    let mut other_compiler = bytes.clone();
    let id = at + magic.len() + 4 + 1;
    other_compiler[id] ^= 0x20;
    let output = run(&other_compiler, "other-compiler");
    assert_eq!(output.status.code(), Some(104));
    let err = text(&output.stderr);
    assert!(err.contains("another compiler"), "{err}");
    // A damaged image.
    let mut damaged = bytes;
    for b in damaged[at + 64..at + 256].iter_mut() {
        *b = 0xff;
    }
    let output = run(&damaged, "damaged");
    assert_eq!(output.status.code(), Some(104));
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

// ---------------------------------------------------------------- the reserve for release (C-17)

/// Runs `main.unio` of a reserve package under `unio run` and as an executable built from it,
/// both under the package's policy and with the heap filled by the test hook when `free` is
/// given; the target file is reset before each run. Returns both outputs.
fn run_both(dir: &Path, free: Option<usize>) -> (Output, Output) {
    let exe = build(&dir.join("main.unio"), dir, true);
    let prepare = |command: &mut Command| {
        command
            .current_dir(dir)
            .env("UNIO_POLICY", dir.join("policy.toml"));
        match free {
            Some(free) => command.env("UNIO_TEST_HEAP_FREE", free.to_string()),
            None => command.env_remove("UNIO_TEST_HEAP_FREE"),
        };
    };
    let reset = || {
        for name in common::temporaries(dir) {
            fs::remove_file(dir.join("open").join(name)).unwrap();
        }
        fs::write(dir.join("open/target.txt"), "old content").unwrap();
    };
    reset();
    let mut interpreter = Command::new(env!("CARGO_BIN_EXE_unio"));
    interpreter.args(["run", "main.unio"]);
    prepare(&mut interpreter);
    let interpreted = interpreter.output().unwrap();
    reset();
    let mut native = Command::new(&exe);
    prepare(&mut native);
    let found = native.output().unwrap();
    for (what, a, b) in [
        ("stdout", &interpreted.stdout, &found.stdout),
        ("stderr", &interpreted.stderr, &found.stderr),
    ] {
        assert_eq!(text(a), text(b), "{what} of unio run and of the executable");
    }
    assert_eq!(interpreted.status.code(), found.status.code());
    (interpreted, found)
}

#[test]
fn the_executable_releases_from_the_reserve_after_a_fault_on_a_full_heap() {
    if !supported() {
        return;
    }
    let dir = common::reserve_package(
        "unio-native",
        "release",
        "public function main() returns Int:
    try:
        using first = Log(name = \"first\"):
            using file = try files.open_atomic(Path(\"open/target.txt\")):
                try file.write(\"new content\")
                using second = Faulty(name = \"second\"):
                    using third = Log(name = \"third\"):
                        return divide(1, 0)
    on IoError as e:
        print(\"error {e.reason}\")
    return 0
",
    );
    // The hook fills the heap of the executable: without the reserve, allocating fails.
    let full = common::reserve_package(
        "unio-native",
        "full",
        "public function main():\n    let words = [\"w {i}\" for i in range(0, 100)]\n    print(\"{words.length()}\")\n",
    );
    let (_, filled) = run_both(&full, Some(0));
    assert_eq!(filled.status.code(), Some(102));
    let (_, found) = run_both(&dir, Some(0));
    let err = text(&found.stderr);
    assert_eq!(found.status.code(), Some(101), "{err}");
    assert_eq!(
        text(&found.stdout),
        "released third: 2000 words, last third 1999\nreleasing second: 2000 words\nreleased first: 2000 words, last first 1999\n"
    );
    assert_eq!(common::temporaries(&dir), Vec::<String>::new());
    common::assert_target_unchanged(&dir);
    assert!(
        err.starts_with("fault: index 2005 is outside 0..2000\n"),
        "{err}"
    );
    assert!(
        err.contains("fault: division by zero\n    at main.divide ("),
        "{err}"
    );
    assert!(err.ends_with("main.unio:39)\n"), "{err}");
    assert!(!err.contains("exhausted"), "{err}");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&full);
}

#[test]
fn the_executable_does_not_release_on_exhaustion() {
    if !supported() {
        return;
    }
    // A release that exhausts the reserve on a full heap: code 102, the rest is skipped.
    let dir = common::reserve_package(
        "unio-native",
        "exhausted",
        "public function main() returns Int:
    try:
        using first = Log(name = \"first\"):
            using file = try files.open_atomic(Path(\"open/target.txt\")):
                try file.write(\"new content\")
                using hungry = Hungry(name = \"hungry\"):
                    return divide(1, 0)
    on IoError as e:
        print(\"error {e.reason}\")
    return 0
",
    );
    let (_, found) = run_both(&dir, Some(0));
    assert_eq!(found.status.code(), Some(102));
    assert_eq!(
        text(&found.stderr),
        "fault: memory exhausted; the program ends with code 102\n"
    );
    assert_eq!(text(&found.stdout), "releasing hungry\n");
    assert_eq!(common::temporaries(&dir).len(), 1);
    common::assert_target_unchanged(&dir);
    // A program that really fills the heap under the limit of 8 MiB: no release runs.
    let filled = common::reserve_package(
        "unio-native",
        "filled",
        "public function main():
    try:
        using file = try files.open_atomic(Path(\"open/target.txt\")):
            try file.write(\"new content\")
            using log = Log(name = \"log\"):
                var all as List[List[Int]] = []
                while true:
                    all.append([n for n in range(0, 10000)])
    on IoError as e:
        print(\"error {e.reason}\")
",
    );
    let (_, found) = run_both(&filled, None);
    assert_eq!(found.status.code(), Some(102));
    assert_eq!(text(&found.stdout), "");
    assert_eq!(common::temporaries(&filled).len(), 1);
    common::assert_target_unchanged(&filled);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&filled);
}
