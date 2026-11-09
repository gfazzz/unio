//! RFC 0005: the effects of function values are carried by the calls of the values. Every
//! path a function value can travel (parameters, returns, locals, fields, collections,
//! patterns, constants, captures, interfaces, recursion) is checked by `unio check` and
//! `unio symbol`, and a run shows that the effect happens at the call and only there.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use unio::effects::Profile;

/// A fresh package directory with the given files.
fn package(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-effects-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        fs::write(dir.join(file), text).unwrap();
    }
    dir
}

/// The diagnostics of a file as (code, line, English message).
fn codes(dir: &Path, file: &str, profile: Profile) -> Vec<(String, usize, String)> {
    unio::check_files_with(&[dir.join(file)], profile)
        .iter()
        .flat_map(|a| {
            a.diagnostics.iter().map(|d| {
                (
                    d.code.to_string(),
                    d.span.start.line,
                    d.message.get(unio::Lang::En).to_string(),
                )
            })
        })
        .collect()
}

fn effects(dir: &Path, name: &str) -> Vec<&'static str> {
    let reports = unio::symbol(name, dir);
    assert_eq!(reports.len(), 1, "{name}: expected one declaration");
    reports[0].effects.clone()
}

/// Checks the package without errors and that each function has `files.read` or not.
#[track_caller]
fn assert_reads(dir: &Path, file: &str, reads: &[&str], pure: &[&str]) {
    let found = codes(dir, file, Profile::Application);
    assert!(found.is_empty(), "{found:?}");
    for name in reads {
        let e = effects(dir, name);
        assert!(e.contains(&"files.read"), "{name} should read: {e:?}");
    }
    for name in pure {
        let e = effects(dir, name);
        assert!(!e.contains(&"files.read"), "{name} should not read: {e:?}");
    }
}

const READER: &str = "Function[(Path) -> Text fails with IoError]";

fn source(body: &str) -> String {
    format!(
        "import std.files as files\n\nfunction pure_text(p as Path) returns Text fails with IoError:\n    return \"x\"\n\n{}",
        body.replace("READER", READER)
    )
}

#[test]
fn passing_through_layers_named_arguments_defaults_and_lambdas() {
    let app = source(
        "\
function h(f as READER, p as Path) returns Text fails with IoError:
    return try f(p)

function g(f as READER, p as Path) returns Text fails with IoError:
    return try h(f, p)

function f1(f as READER, p as Path) returns Text fails with IoError:
    return try g(f, p)

function outer() returns Text fails with IoError:
    return try f1(files.read_text, Path(\"a\"))

function outer_pure() returns Text fails with IoError:
    return try f1(pure_text, Path(\"a\"))

function named(p as Path, f as READER) returns Text fails with IoError:
    return try f(p)

function by_name() returns Text fails with IoError:
    return try named(f = files.read_text, p = Path(\"a\"))

function chain(f as READER) returns Text fails with IoError:
    return try named(Path(\"a\"), f = f)

function by_name_chain() returns Text fails with IoError:
    return try chain(f = files.read_text)

function only_named(named f as READER) returns Text fails with IoError:
    return try f(Path(\"a\"))

function by_only_named() returns Text fails with IoError:
    return try only_named(f = files.read_text)

function defaulted(p as Path, f as READER = files.read_text) returns Text fails with IoError:
    return try f(p)

function uses_default() returns Text fails with IoError:
    return try defaulted(Path(\"a\"))

function overrides_default() returns Text fails with IoError:
    return try defaulted(Path(\"a\"), pure_text)

function wraps(f as READER) returns Text fails with IoError:
    return try h(function (p as Path) returns Text fails with IoError: try f(p), Path(\"a\"))

function wraps_read() returns Text fails with IoError:
    return try wraps(files.read_text)

function wraps_pure() returns Text fails with IoError:
    return try wraps(pure_text)
",
    );
    let dir = package("passing", &[("app.unio", &app)]);
    assert_reads(
        &dir,
        "app.unio",
        &[
            "app.outer",
            "app.by_name",
            "app.by_name_chain",
            "app.by_only_named",
            "app.uses_default",
            "app.wraps_read",
        ],
        &[
            "app.h",
            "app.g",
            "app.f1",
            "app.outer_pure",
            "app.named",
            "app.chain",
            "app.only_named",
            "app.defaulted",
            "app.overrides_default",
            "app.wraps",
            "app.wraps_pure",
        ],
    );
    for (name, parameter) in [
        ("app.h", "f"),
        ("app.g", "f"),
        ("app.f1", "f"),
        ("app.chain", "f"),
        ("app.defaulted", "f"),
        ("app.wraps", "f"),
    ] {
        assert_eq!(
            unio::symbol(name, &dir)[0].argument_effects,
            [parameter],
            "{name}"
        );
    }
    // The chain names the call that passes the value and the value.
    fs::write(dir.join("unio.toml"), MANIFEST_NOTHING).unwrap();
    let found = codes(&dir, "app.unio", Profile::Application);
    assert_eq!(found[0].0, "E0085", "{found:?}");
    assert!(
        found[0]
            .2
            .contains("app.outer -> app.f1 (argument f) -> std.files.read_text"),
        "{}",
        found[0].2
    );
    let _ = fs::remove_dir_all(&dir);
}

const MANIFEST_NOTHING: &str = "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\n";

#[test]
fn returning_functions_and_lambdas() {
    let app = source(
        "\
function reader() returns READER:
    return files.read_text

function use_reader() returns Text fails with IoError:
    return try reader()(Path(\"a\"))

function use_reader_local() returns Text fails with IoError:
    let r = reader()
    return try r(Path(\"a\"))

function choose(fast as Bool) returns READER:
    if fast:
        return pure_text
    return files.read_text

function use_choose() returns Text fails with IoError:
    let r = choose(true)
    return try r(Path(\"a\"))

function choose_pure(fast as Bool) returns READER:
    return pure_text if fast else pure_text

function use_choose_pure() returns Text fails with IoError:
    return try choose_pure(true)(Path(\"a\"))

function wrap(f as READER) returns READER:
    return function (p as Path) returns Text fails with IoError: try f(p)

function use_wrap() returns Text fails with IoError:
    let w = wrap(files.read_text)
    return try w(Path(\"a\"))

function level3() returns READER:
    return files.read_text

function level2() returns Function[() -> READER]:
    return level3

function level1() returns Function[() -> Function[() -> READER]]:
    return level2

function use_levels() returns Text fails with IoError:
    return try level1()()()(Path(\"a\"))

function never_calls() returns Int:
    let r = reader()
    let w = wrap(files.read_text)
    return 1
",
    );
    let dir = package("returning", &[("app.unio", &app)]);
    assert_reads(
        &dir,
        "app.unio",
        &[
            "app.use_reader",
            "app.use_reader_local",
            "app.use_choose",
            "app.use_wrap",
            "app.use_levels",
        ],
        &[
            "app.reader",
            "app.choose",
            "app.choose_pure",
            "app.use_choose_pure",
            "app.wrap",
            "app.level1",
            "app.level2",
            "app.level3",
            "app.never_calls",
        ],
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn storing_in_locals_fields_collections_options_constants_and_captures() {
    let other = "\
import std.files as files

public struct Box:
    public var run as Function[(Path) -> Text fails with IoError]

public function fill(var b as Box):
    b.run = files.read_text

public let READ = files.read_text
";
    let app = source(
        "\
import other

function loop_local() returns Text fails with IoError:
    var f = pure_text
    var out = \"\"
    for i in [1, 2]:
        out = try f(Path(\"a\"))
        f = files.read_text
    return out

function field_call() returns Text fails with IoError:
    var b = other.Box(run = pure_text)
    return try b.run(Path(\"a\"))

function list_index() returns Text fails with IoError:
    let fs = [files.read_text]
    return try fs[0](Path(\"a\"))

function list_iteration() returns Text fails with IoError:
    var out = \"\"
    for f in [files.read_text]:
        out = try f(Path(\"a\"))
    return out

function list_get() returns Text fails with IoError:
    let fs = [files.read_text]
    match fs.get(0):
        case some(f): return try f(Path(\"a\"))
        case none: return \"\"

function map_get() returns Text fails with IoError:
    let m = {\"r\": files.read_text}
    match m.get(\"r\"):
        case some(f): return try f(Path(\"a\"))
        case none: return \"\"

function option_call(o as Option[READER]) returns Text fails with IoError:
    match o:
        case some(f): return try f(Path(\"a\"))
        case none: return \"\"

function constant_call() returns Text fails with IoError:
    return try other.READ(Path(\"a\"))

function constant_local() returns Text fails with IoError:
    let r = other.READ
    return try r(Path(\"a\"))

function captured_struct() returns Text fails with IoError:
    let b = other.Box(run = pure_text)
    let call = function (p as Path) returns Text fails with IoError: try b.run(p)
    return try call(Path(\"a\"))

function captured_local() returns Text fails with IoError:
    let r = files.read_text
    let call = function (p as Path) returns Text fails with IoError: try r(p)
    return try call(Path(\"a\"))

function never_called() returns Int:
    let r = files.read_text
    let kept = [r]
    let boxed = other.Box(run = r)
    let call = function (p as Path) returns Text fails with IoError: try r(p)
    return kept.length()
",
    );
    let dir = package("storing", &[("other.unio", other), ("app.unio", &app)]);
    assert_reads(
        &dir,
        "app.unio",
        &[
            "app.loop_local",
            "app.field_call",
            "app.list_index",
            "app.list_iteration",
            "app.list_get",
            "app.map_get",
            "app.option_call",
            "app.constant_call",
            "app.constant_local",
            "app.captured_struct",
            "app.captured_local",
        ],
        &["app.never_called", "other.fill"],
    );
    // The capturing lambda allocates where it is made, though it is never called.
    assert_eq!(effects(&dir, "app.never_called"), ["allocates"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn calling_through_interfaces_methods_loops_using_and_handlers() {
    let app = source(
        "\
interface Loader:
    function load(self, p as Path) returns Text fails with IoError

struct Stored:
    step as READER

extend Stored with Loader:
    function load(self, p as Path) returns Text fails with IoError:
        return try self.step(p)

struct Plain:
    n as Int

extend Plain with Loader:
    function load(self, p as Path) returns Text fails with IoError:
        return \"x\"

extend Plain:
    function run(self, f as READER) returns Text fails with IoError:
        return try f(Path(\"a\"))

interface Visitor:
    function visit(self, f as READER) returns Text fails with IoError

extend Plain with Visitor:
    function visit(self, f as READER) returns Text fails with IoError:
        return try self.run(f)

function make() returns Stored:
    return Stored(step = files.read_text)

function via_interface(l as Loader) returns Text fails with IoError:
    return try l.load(Path(\"a\"))

function via_plain(l as Plain) returns Text fails with IoError:
    return try l.load(Path(\"a\"))

function method_read(r as Plain) returns Text fails with IoError:
    return try r.run(files.read_text)

function method_pure(r as Plain) returns Text fails with IoError:
    return try r.run(pure_text)

function visit_read(v as Visitor) returns Text fails with IoError:
    return try v.visit(files.read_text)

function visit_pure(v as Visitor) returns Text fails with IoError:
    return try v.visit(pure_text)

struct Handle:
    var n as Int

extend Handle with Resource:
    function release(var self):
        self.n = 0

function in_for(paths as List[Path]) returns Int:
    let r = files.read_text
    var n = 0
    for p in paths:
        let t = try r(p) on IoError as e:
            return 0
        n = n + t.length()
    return n

function in_using() returns Int:
    let r = files.read_text
    using h = Handle(n = 1):
        let t = try r(Path(\"a\")) on IoError as e:
            return 0
        return t.length()

function in_handler() returns Int:
    let r = files.read_text
    let t = try pure_text(Path(\"a\")) on IoError as e:
        let u = try r(Path(\"b\")) on IoError as e2:
            return 0
        return u.length()
    return t.length()

function in_try_block() returns Int:
    let r = files.read_text
    try:
        let t = r(Path(\"a\"))
        return t.length()
    on IoError as e:
        return 0
",
    );
    let dir = package("calling", &[("app.unio", &app)]);
    assert_reads(
        &dir,
        "app.unio",
        &[
            "app.Loader.load",
            "app.Stored.load",
            "app.via_interface",
            "app.method_read",
            "app.visit_read",
            "app.in_for",
            "app.in_using",
            "app.in_handler",
            "app.in_try_block",
        ],
        &[
            "app.make",
            "app.via_plain",
            "app.Plain.run",
            "app.Visitor.visit",
            "app.method_pure",
            "app.visit_pure",
        ],
    );
    assert_eq!(
        unio::symbol("app.Visitor.visit", &dir)[0].argument_effects,
        ["f"]
    );
    let _ = fs::remove_dir_all(&dir);
    // Unio has no method values: a method is not a value of a function type.
    let methods = source(
        "\
struct S:
    n as Int

extend S:
    function get(self) returns Int:
        return self.n

function value(s as S) returns Int:
    let m = s.get
    return 0
",
    );
    let dir = package("method_value", &[("app.unio", &methods)]);
    let found = codes(&dir, "app.unio", Profile::Application);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "E0056", "{found:?}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn recursion_reaches_a_fixpoint_with_the_effect() {
    let app = source(
        "\
function repeat(n as Int, f as READER) returns Int fails with IoError:
    if n == 0:
        let t = try f(Path(\"a\"))
        return t.length()
    return try repeat(n - 1, f)

function use_repeat() returns Int fails with IoError:
    return try repeat(3, files.read_text)

function use_repeat_pure() returns Int fails with IoError:
    return try repeat(3, pure_text)

function ping(n as Int, f as READER) returns Int fails with IoError:
    if n == 0:
        let t = try f(Path(\"a\"))
        return t.length()
    return try pong(n - 1, f)

function pong(n as Int, f as READER) returns Int fails with IoError:
    return try ping(n, f)

function use_ping() returns Int fails with IoError:
    return try ping(4, files.read_text)

function use_pong_pure() returns Int fails with IoError:
    return try pong(4, pure_text)

function depth0(n as Int) returns Int fails with IoError:
    return try depth1(n, pure_text)

function depth1(n as Int, f as READER) returns Int fails with IoError:
    let t = try f(Path(\"a\"))
    if n == 0:
        return t.length()
    return try depth2(n, f)

function depth2(n as Int, f as READER) returns Int fails with IoError:
    return try depth1(n - 1, files.read_text)

function a(n as Int) returns READER:
    if n == 0:
        return pure_text
    return b(n - 1)

function b(n as Int) returns READER:
    if n == 1:
        return files.read_text
    return a(n - 1)

function use_a() returns Text fails with IoError:
    return try a(5)(Path(\"x\"))

struct Node:
    step as READER
    rest as List[Node]

function walk(node as Node) returns Int fails with IoError:
    let t = try node.step(Path(\"a\"))
    var n = t.length()
    for child in node.rest:
        n = n + try walk(child)
    return n

function tree() returns Node:
    return Node(step = pure_text, rest = [Node(step = files.read_text, rest = [])])
",
    );
    let dir = package("recursion", &[("app.unio", &app)]);
    assert_reads(
        &dir,
        "app.unio",
        &[
            "app.use_repeat",
            "app.use_ping",
            "app.depth0",
            "app.depth1",
            "app.depth2",
            "app.use_a",
            "app.walk",
        ],
        &[
            "app.repeat",
            "app.use_repeat_pure",
            "app.ping",
            "app.pong",
            "app.use_pong_pure",
            "app.a",
            "app.b",
            "app.tree",
        ],
    );
    for name in ["app.repeat", "app.ping", "app.pong", "app.depth1"] {
        assert_eq!(
            unio::symbol(name, &dir)[0].argument_effects,
            ["f"],
            "{name}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn values_that_are_not_called_and_pure_values_have_no_effect() {
    let app = "\
function double(x as Int) returns Int:
    return x * 2

function apply(f as Function[(Int) -> Int], x as Int) returns Int:
    return f(x)

function stored() returns Int:
    let f = double
    let g = function (x as Int) returns Int: x + 1
    return 1

function pure_higher_order() returns Int:
    return apply(double, 1) + apply(function (x as Int) returns Int: x + 1, 2)

function capturing(n as Int) returns Int:
    let add = function (x as Int) returns Int: x + n
    return n
";
    let dir = package("negatives", &[("core.unio", app)]);
    assert!(codes(&dir, "core.unio", Profile::Application).is_empty());
    assert!(effects(&dir, "core.stored").is_empty());
    assert!(effects(&dir, "core.pure_higher_order").is_empty());
    assert_eq!(effects(&dir, "core.capturing"), ["allocates"]);
    let found = codes(&dir, "core.unio", Profile::Core);
    let lines: Vec<(&str, usize)> = found.iter().map(|f| (f.0.as_str(), f.1)).collect();
    assert_eq!(lines, [("E0084", 15)], "{found:?}");
    assert!(
        found[0]
            .2
            .contains("a lambda that captures values on line 16"),
        "{}",
        found[0].2
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The conservative cases under the core profile: a call through an estimate is E0087 at the
/// call when one of its possible values needs more than core, and accepted when none does.
/// The types are of the package (`Meters`), so that no function value of the standard library
/// may have them.
const CORE: &str = "\
struct Meters:
    n as Int

function double(m as Meters) returns Int:
    return m.n * 2

function triple(m as Meters) returns Int:
    return m.n * 3

struct Box:
    run as Function[(Meters) -> Int]

function keep(f as Function[(Meters) -> Int]) returns Box:
    return Box(run = f)

function call_box(b as Box) returns Int:
    return b.run(Meters(n = 1))

function use_keep() returns Int:
    return call_box(keep(double))

function call_option(o as Option[Function[(Meters) -> Int]]) returns Int:
    match o:
        case some(f): return f(Meters(n = 1))
        case none: return 0

function use_option() returns Int:
    return call_option(some(double))

function pick(b as Box) returns Function[(Meters) -> Int]:
    return b.run

function call_pick(b as Box) returns Int:
    return pick(b)(Meters(n = 2))

function level2() returns Function[(Meters) -> Int]:
    return triple

function level1() returns Function[() -> Function[(Meters) -> Int]]:
    return level2

function call_levels() returns Int:
    return level1()()(Meters(n = 3))

function id[T](x as T) returns T:
    return x

function call_generic() returns Int:
    return id(double)(Meters(n = 4))

function apply(f as Function[(Meters) -> Int], m as Meters) returns Int:
    return f(m)

function call_apply() returns Int:
    return apply(double, Meters(n = 1))
";

#[test]
fn the_core_profile_rejects_only_estimates_that_may_leave_core() {
    let dir = package("core_pure", &[("core.unio", CORE)]);
    let found = codes(&dir, "core.unio", Profile::Core);
    assert!(found.is_empty(), "{found:?}");
    let _ = fs::remove_dir_all(&dir);

    let mixed = format!(
        "{CORE}
function label(m as Meters) returns Int:
    return \"{{m.n}}\".length()

function use_label() returns Int:
    return apply(label, Meters(n = 1))

function keep_label() returns Box:
    return keep(label)
"
    );
    let dir = package("core_mixed", &[("core.unio", &mixed)]);
    assert!(codes(&dir, "core.unio", Profile::Application).is_empty());
    let found = codes(&dir, "core.unio", Profile::Core);
    let lines: Vec<(&str, usize)> = found.iter().map(|f| (f.0.as_str(), f.1)).collect();
    assert_eq!(
        lines,
        [
            // The field holds `label` through the parameter `keep` stores.
            ("E0087", 17),
            // A caller is reported at its own call, not again at the estimated call.
            ("E0087", 20),
            // A pattern binding: any value of the type.
            ("E0087", 24),
            ("E0087", 28),
            // The values `pick` returns are read from a field.
            ("E0087", 34),
            // The result of a call through a value, and a generic result.
            ("E0087", 43),
            ("E0087", 49),
            // A named function and a parameter are exact: E0084, not E0087.
            ("E0084", 57),
            ("E0084", 60),
        ],
        "{found:?}"
    );
    assert!(
        found[0]
            .2
            .contains("the values stored in the field core.Box.run")
            && found[0].2.contains("core.label"),
        "{}",
        found[0].2
    );
    assert!(
        found[2]
            .2
            .contains("any function value of type Function[(Meters) -> Int]"),
        "{}",
        found[2].2
    );
    assert!(
        found[8]
            .2
            .contains("core.use_label -> core.apply (argument f) -> core.label"),
        "{}",
        found[8].2
    );
    // `keep_label` stores `label` but never calls it: no error of its own.
    assert!(
        !found.iter().any(|f| f.2.contains("'keep_label'")),
        "{found:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A run: the effect of a function value happens where it is called, through every path,
/// and a value that is only made does nothing; the check derives the same.
#[test]
fn a_run_writes_only_where_the_value_is_called() {
    let program = "\
import std.files as files

function mark(path as Path) fails with IoError:
    try files.write_text(path, \"x\")

function skip(path as Path) fails with IoError:
    return

function h(f as Function[(Path) fails with IoError], path as Path) fails with IoError:
    try f(path)

function g(f as Function[(Path) fails with IoError], path as Path) fails with IoError:
    try h(f, path)

function layers(f as Function[(Path) fails with IoError], path as Path) fails with IoError:
    try g(f, path)

function later(f as Function[(Path) fails with IoError]) returns Function[(Path) fails with IoError]:
    return function (path as Path) fails with IoError: try f(path)

struct Task:
    run as Function[(Path) fails with IoError]

interface Job:
    function start(self, path as Path) fails with IoError

extend Task with Job:
    function start(self, path as Path) fails with IoError:
        try self.run(path)

function start(job as Job, path as Path) fails with IoError:
    try job.start(path)

public function main() returns Int:
    let kept = mark
    let never = later(mark)
    let boxed = Task(run = mark)
    try:
        try layers(mark, Path(\"out/layers.txt\"))
        try layers(skip, Path(\"out/skipped.txt\"))
        try later(mark)(Path(\"out/returned.txt\"))
        let steps = [skip, mark]
        try steps[1](Path(\"out/list.txt\"))
        for step in steps:
            try step(Path(\"out/loop.txt\"))
        match steps.get(1):
            case some(step): try step(Path(\"out/option.txt\"))
            case none: return 2
        try start(Task(run = mark), Path(\"out/job.txt\"))
    on IoError as e:
        return 1
    return 0
";
    let manifest = "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\nfiles.write = [\"$CWD/out/**\"]\n";
    let dir = package("run", &[("main.unio", program), ("unio.toml", manifest)]);
    fs::create_dir_all(dir.join("out")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    assert!(codes(&dir, "main.unio", Profile::Application).is_empty());
    assert!(effects(&dir, "main.main").contains(&"files.write"));
    assert!(effects(&dir, "main.layers").is_empty());
    assert!(effects(&dir, "main.later").contains(&"allocates"));
    assert!(!effects(&dir, "main.later").contains(&"files.write"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(&dir)
        .env_remove("UNIO_POLICY")
        .arg("run");
    if cfg!(windows) {
        command.arg("--allow-weaker-sandbox");
    }
    let output = command.arg("main.unio").output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut written: Vec<String> = fs::read_dir(dir.join("out"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        [
            "job.txt",
            "layers.txt",
            "list.txt",
            "loop.txt",
            "option.txt",
            "returned.txt"
        ]
    );

    // Without the permission the check rejects the first call that carries the value.
    fs::write(dir.join("unio.toml"), MANIFEST_NOTHING).unwrap();
    let found = codes(&dir, "main.unio", Profile::Application);
    assert!(
        found.iter().all(|f| f.0 == "E0085") && !found.is_empty(),
        "{found:?}"
    );
    // A program that only makes and keeps the value is accepted with no permission.
    let keeps = "\
import std.files as files

struct Task:
    run as Function[(Path, Text, Bool) fails with IoError]

public function main() returns Int:
    let kept = files.write_text
    let boxed = Task(run = files.write_text)
    let all = [files.write_text]
    return all.length()
";
    fs::write(dir.join("main.unio"), keeps).unwrap();
    assert!(codes(&dir, "main.unio", Profile::Application).is_empty());
    let _ = fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ the tests in the analysis

/// A hook that ordinary code calls through a field, and a function that keeps a value in one.
/// The program itself only ever stores `quiet`.
const HOOKS: &str = "\
import std.files as files

public struct Spot:
    public path as Path

public struct Hook:
    public run as Function[(Spot) fails with IoError]

public function keep(f as Function[(Spot) fails with IoError]) returns Hook:
    return Hook(run = f)

public function fire(h as Hook, spot as Spot) fails with IoError:
    try h.run(spot)

public function fire_option(o as Option[Function[(Spot) fails with IoError]], spot as Spot) fails with IoError:
    match o:
        case some(f): try f(spot)
        case none: return

public function quiet(spot as Spot) fails with IoError:
    return
";

const MANIFEST_OUT: &str =
    "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\nfiles.write = [\"$CWD/out/**\"]\n";

/// Runs the compiler in the directory; the exit code and what it printed.
fn unio_in(dir: &Path, args: &[&str], stdlib: Option<&Path>) -> (Option<i32>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command.current_dir(dir).env_remove("UNIO_POLICY");
    if let Some(root) = stdlib {
        command.env("UNIO_STDLIB", root);
    }
    command.arg(args[0]);
    if cfg!(windows) {
        // Programs and the workers of the tools need the operator's permission for the weaker
        // sandbox on Windows (SB-5, SB-10).
        command.arg("--allow-weaker-sandbox");
    }
    let output = command.args(&args[1..]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.code(), text)
}

/// A test block that is not run does not add its function values to the program: `unio
/// check` and `unio run` see only what the program stores (RFC 0005, tests in the analysis).
#[test]
fn values_made_in_tests_that_are_not_run_do_not_reach_the_program() {
    let program = format!(
        "{HOOKS}
public function main() returns Int:
    try:
        try fire(Hook(run = quiet), Spot(path = Path(\"out/main.txt\")))
        try fire_option(some(quiet), Spot(path = Path(\"out/main.txt\")))
    on IoError as e:
        return 1
    return 0

test \"a writer through a field and through any value of its type\":
    let h = Hook(run = function (s as Spot) fails with IoError: try files.write_text(s.path, \"x\"))
    let o = some(function (s as Spot) fails with IoError: try files.write_text(s.path, \"y\"))
    assert true
"
    );
    let dir = package(
        "unrun",
        &[("main.unio", &program), ("unio.toml", MANIFEST_NOTHING)],
    );
    let dir = fs::canonicalize(dir).unwrap();
    let found = codes(&dir, "main.unio", Profile::Application);
    assert!(found.is_empty(), "{found:?}");
    assert!(!effects(&dir, "main.fire").contains(&"files.write"));
    assert!(!effects(&dir, "main.fire_option").contains(&"files.write"));
    let (code, text) = unio_in(&dir, &["run", "main.unio"], None);
    assert_eq!(code, Some(0), "{text}");
    // Under the core profile the estimated call sees no value of a test either.
    let core = "\
struct Meters:
    n as Int

function call_option(o as Option[Function[(Meters) -> Int]]) returns Int:
    match o:
        case some(f): return f(Meters(n = 1))
        case none: return 0

function double(m as Meters) returns Int:
    return m.n * 2

function use_option() returns Int:
    return call_option(some(double))

test \"a value that allocates, made in a test\":
    assert call_option(some(function (m as Meters) returns Int: \"{m.n}\".length())) == 1
";
    fs::write(dir.join("core.unio"), core).unwrap();
    let found = codes(&dir, "core.unio", Profile::Core);
    assert!(found.is_empty(), "{found:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// The standard library's private test helper `positive` (an `(Int) -> Int` value named only
/// in tests of alloc/builtin/list.unio) is not a possible target of a user's estimated call.
/// Functions are numbered before lambdas, so while it was a target the chain of E0087 named
/// it first. Ordinary functions of the library still count, test helpers among them: the
/// lambda in `sort_singleton_with_faulting_key` is made by an ordinary function.
#[test]
fn the_tests_of_the_library_do_not_reach_a_user_package() {
    let core = "\
function call_option(o as Option[Function[(Int) -> Int]]) returns Int:
    match o:
        case some(f): return f(1)
        case none: return 0

function inc(x as Int) returns Int:
    return x + 1

function use_option() returns Int:
    return call_option(some(inc))
";
    let dir = package("positive", &[("core.unio", core)]);
    let found = codes(&dir, "core.unio", Profile::Core);
    // Neither a value made in the library's tests nor a helper that only its tests use is
    // a call target of the user's program, so the core profile accepts the package.
    assert!(found.is_empty(), "{found:?}");
    let _ = fs::remove_dir_all(&dir);
}

const WRITER_TESTS: &str = "
test \"a kept writer inside out\":
    let h = keep(function (s as Spot) fails with IoError: try files.write_text(s.path, \"x\"))
    try:
        try fire(h, Spot(path = Path(\"out/test.txt\")))
    on IoError as e: assert_fail(\"the write inside out failed\")

test \"a kept writer outside out\":
    let h = keep(function (s as Spot) fails with IoError: try files.write_text(s.path, \"x\"))
    try:
        try fire(h, Spot(path = Path(\"elsewhere.txt\")))
        assert_fail(\"the sandbox let the write through\")
    on IoError as e: assert true

test \"a pure hook\":
    assert true
";

/// When the test runs, its values count: the effect they bring into ordinary code is checked
/// against the manifest (E0085, nothing runs), and with the permission the sandbox still
/// denies what the manifest does not list. Only the tests that run are analysed.
#[test]
fn values_made_in_tests_that_run_are_checked() {
    let program = format!(
        "{HOOKS}
public function main() returns Int:
    try fire(keep(quiet), Spot(path = Path(\"out/main.txt\"))) on IoError as e:
        return 1
    return 0
{WRITER_TESTS}"
    );
    let dir = package(
        "tested",
        &[("main.unio", &program), ("unio.toml", MANIFEST_NOTHING)],
    );
    fs::create_dir_all(dir.join("out")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    assert!(codes(&dir, "main.unio", Profile::Application).is_empty());
    let (code, text) = unio_in(&dir, &["test", "main.unio"], None);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("E0085") && text.contains("files.write"),
        "{text}"
    );
    assert!(text.contains("main.fire"), "{text}");
    assert!(!dir.join("out/test.txt").exists());
    // A filter that leaves out the writers leaves their values out of the analysis too.
    let (code, text) = unio_in(&dir, &["test", "pure", "main.unio"], None);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");

    // With the permission the tests run, and the sandbox keeps its limits.
    fs::write(dir.join("unio.toml"), MANIFEST_OUT).unwrap();
    let (code, text) = unio_in(&dir, &["test", "main.unio"], None);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("3 passed, 0 failed"), "{text}");
    assert!(dir.join("out/test.txt").exists());
    assert!(!dir.join("elsewhere.txt").exists());
    let _ = fs::remove_dir_all(&dir);
}

/// The tests of an imported module are not the user's tests: they count only when that module
/// is itself the target of `unio test`.
#[test]
fn the_tests_of_an_imported_module_count_only_as_targets() {
    let lib = format!(
        "{HOOKS}
test \"the library keeps a writer\":
    let h = Hook(run = function (s as Spot) fails with IoError: try files.write_text(s.path, \"x\"))
    let k = keep(function (s as Spot) fails with IoError: try files.write_text(s.path, \"y\"))
    assert true
"
    );
    let main = "\
import std.files as files
import lib

public function main() returns Int:
    try lib.fire(lib.keep(lib.quiet), lib.Spot(path = Path(\"out/main.txt\"))) on IoError as e:
        return 1
    return 0

test \"the user's hook\":
    try:
        try lib.fire(lib.Hook(run = lib.quiet), lib.Spot(path = Path(\"out/user.txt\")))
    on IoError as e: assert_fail(\"a pure hook failed\")
";
    let dir = package(
        "imported",
        &[
            ("lib.unio", &lib),
            ("main.unio", main),
            ("unio.toml", MANIFEST_NOTHING),
        ],
    );
    let dir = fs::canonicalize(dir).unwrap();
    assert!(codes(&dir, "main.unio", Profile::Application).is_empty());
    let (code, text) = unio_in(&dir, &["test", "main.unio"], None);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");
    let (code, text) = unio_in(&dir, &["test", "lib.unio"], None);
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("E0085"), "{text}");
    let _ = fs::remove_dir_all(&dir);
}

/// The tests of the standard library run and are analysed when its modules are the targets,
/// and stay out of a user package's analysis otherwise. A probe copy of the library, chosen
/// by UNIO_STDLIB, has a module whose test keeps a writer.
#[test]
fn the_tests_of_the_library_run_and_count_as_targets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = fs::canonicalize(root).unwrap();
    let (code, text) = unio_in(&root, &["test", "stdlib/alloc/builtin/list.unio"], None);
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("fallible mapping stops at the first error"),
        "{text}"
    );

    let probe = std::env::temp_dir().join(format!("unio-effects-{}-probe", std::process::id()));
    let _ = fs::remove_dir_all(&probe);
    copy_dir(&root.join("stdlib"), &probe.join("stdlib"));
    let stdlib = probe.join("stdlib");
    // Inside the library the error of `std.files` is named through its module.
    let hooks = HOOKS.replace("IoError", "files.IoError");
    let module = format!(
        "{hooks}
test \"the probe keeps a writer\":
    let h = Hook(run = function (s as Spot) fails with files.IoError: try files.write_text(s.path, \"x\"))
    let k = keep(function (s as Spot) fails with files.IoError: try files.write_text(s.path, \"y\"))
    try:
        try fire(Hook(run = quiet), Spot(path = Path(\"probe.txt\")))
    on files.IoError as e: assert_fail(\"a pure hook failed\")
"
    );
    fs::write(stdlib.join("std/probe.unio"), module).unwrap();
    let user = "\
import std.files as files
import std.probe as probe

public function main() returns Int:
    try probe.fire(probe.keep(probe.quiet), probe.Spot(path = Path(\"out/main.txt\"))) on IoError as e:
        return 1
    return 0

test \"the user's hook\":
    try:
        try probe.fire(probe.Hook(run = probe.quiet), probe.Spot(path = Path(\"out/user.txt\")))
    on IoError as e: assert_fail(\"a pure hook failed\")
";
    let app = probe.join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("main.unio"), user).unwrap();
    fs::write(app.join("unio.toml"), MANIFEST_NOTHING).unwrap();
    let (code, text) = unio_in(&app, &["check", "main.unio"], Some(&stdlib));
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = unio_in(&app, &["test", "main.unio"], Some(&stdlib));
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");
    let (code, text) = unio_in(&stdlib, &["test", "std/probe.unio"], Some(&stdlib));
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("ok   std/probe.unio: the probe keeps a writer"),
        "{text}"
    );
    // The analysis of the probe as a target counts the writer for `fire`; a check does not.
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_effects_in_a_child", "--nocapture"])
        .env("UNIO_STDLIB", &stdlib)
        .env("UNIO_PROBE_MODULE", stdlib.join("std/probe.unio"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("tested: files.write; checked: none"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&probe);
}

/// Run by `the_tests_of_the_library_run_and_count_as_targets` in a process of its own, so that
/// UNIO_STDLIB names the probe copy; does nothing otherwise.
#[test]
fn probe_effects_in_a_child() {
    let Some(module) = std::env::var_os("UNIO_PROBE_MODULE") else {
        return;
    };
    let file = PathBuf::from(module);
    let writes = |program: &unio::program::Program| {
        let derived = unio::effects::derive(program);
        let fire = program
            .funcs
            .iter()
            .position(|f| f.name == "fire" && program.modules[f.module].name == "std.probe")
            .expect("std.probe.fire");
        if derived.effect_names(fire).contains(&"files.write") {
            "files.write"
        } else {
            "none"
        }
    };
    let (tested, checked) = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let tested = writes(&unio::load_for_test(std::slice::from_ref(&file), None));
            let checked = writes(&unio::load_checked(&[file], Profile::Application));
            (tested, checked)
        })
        .unwrap()
        .join()
        .unwrap();
    println!("tested: {tested}; checked: {checked}");
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}
