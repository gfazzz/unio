//! `unio symbol` (C-11), content hashes (C-11a, C-11b), effects and layers (semantics 8),
//! the core profile (E0084) and the permissions of the manifest (E0085).

use std::path::{Path, PathBuf};

use unio::effects::Profile;

/// A fresh package directory with the given files.
fn package(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-symbol-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        std::fs::write(dir.join(file), text).unwrap();
    }
    dir
}

fn hash_of(dir: &Path, name: &str) -> String {
    let reports = unio::symbol(name, dir);
    assert_eq!(reports.len(), 1, "{name}: expected one declaration");
    let hash = reports[0].hash.clone();
    assert!(hash.starts_with("unio-hash-v1:sha384:"), "{hash}");
    assert_eq!(hash.len(), "unio-hash-v1:sha384:".len() + 96);
    hash
}

/// The hashes of the names in a package with one module `m.unio`.
fn hashes(case: &str, source: &str, names: &[&str]) -> Vec<String> {
    let dir = package(case, &[("m.unio", source)]);
    let found = names
        .iter()
        .map(|n| hash_of(&dir, &format!("m.{n}")))
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    found
}

const BASE: &str = "\
struct Point:
    x as Int
    y as Int

function twice(value as Int) returns Int:
    return value * 2

function scaled(point as Point) returns Point:
    return Point(x = twice(point.x), y = twice(point.y))

function even(n as Int) returns Bool:
    if n == 0: return true
    return odd(n - 1)

function odd(n as Int) returns Bool:
    if n == 0: return false
    return even(n - 1)
";

const NAMES: [&str; 5] = ["Point", "twice", "scaled", "even", "odd"];

#[test]
fn formatting_comments_and_order_do_not_change_hashes() {
    let base = hashes("base", BASE, &NAMES);
    let reformatted = "\
## A point.
struct Point:
    x as Int  # across
    y as Int

# Mutual recursion first, in the other order.
function odd(n as Int) returns Bool:
    if n == 0:
        return false
    return even(n - 1)

function even(n as Int) returns Bool:
    if n == 0: return true
    return odd(n - 1)

function scaled(point as Point) returns Point:
    return Point(
        x = twice(point.x),
        y = twice(point.y),
    )


## Doubles.
function twice(value as Int) returns Int:
    return value * 2
";
    assert_eq!(base, hashes("reformatted", reformatted, &NAMES));
}

#[test]
fn a_changed_body_changes_its_hash_and_the_hashes_of_its_users() {
    let base = hashes("base2", BASE, &NAMES);
    let changed = hashes(
        "changed",
        &BASE.replace("return value * 2", "return value * 3"),
        &NAMES,
    );
    assert_eq!(base[0], changed[0], "Point does not use twice");
    assert_ne!(base[1], changed[1], "twice changed");
    assert_ne!(base[2], changed[2], "scaled calls twice");
    assert_eq!(base[3], changed[3]);
    assert_eq!(base[4], changed[4]);
}

#[test]
fn a_changed_type_or_name_changes_the_hash() {
    let base = hashes("base3", BASE, &NAMES);
    let field = hashes(
        "field",
        &BASE.replace("    y as Int\n", "    y as Int\n    z as Int = 0\n"),
        &NAMES,
    );
    assert_ne!(base[0], field[0], "Point changed");
    assert_ne!(base[2], field[2], "scaled uses Point");
    assert_eq!(base[1], field[1]);
    let renamed = hashes(
        "renamed",
        &BASE.replace(
            "twice(value as Int) returns Int:\n    return value * 2",
            "twice(number as Int) returns Int:\n    return number * 2",
        ),
        &NAMES,
    );
    assert_ne!(base[1], renamed[1], "a parameter name is part of the hash");
}

#[test]
fn mutual_recursion_is_hashed_as_one_component() {
    let base = hashes("base4", BASE, &NAMES);
    assert_ne!(
        base[3], base[4],
        "members of one component have their own hashes"
    );
    let changed = hashes(
        "recursion",
        &BASE.replace("if n == 0: return false", "if n <= 0: return false"),
        &NAMES,
    );
    assert_ne!(base[3], changed[3], "even depends on the changed odd");
    assert_ne!(base[4], changed[4]);
}

#[test]
fn symbol_reports_signature_documentation_errors_effects_and_calls() {
    let dir = package(
        "report",
        &[(
            "app.unio",
            "\
import std.files as files

## Reads a file.
## Then counts it.
function size(path as Path) returns Int fails with IoError:
    return (try files.read_text(path)).length()

public function main() returns Int:
    return try size(Path(\"a\")) on IoError as e:
        return 1
",
        )],
    );
    let reports = unio::symbol("app.size", &dir);
    assert_eq!(reports.len(), 1);
    let r = &reports[0];
    assert_eq!(r.kind, "function");
    assert_eq!(
        r.signature,
        "function size(path as Path) returns Int fails with IoError"
    );
    assert_eq!(r.documentation, "Reads a file.\nThen counts it.");
    assert!(r.declared.ends_with("app.unio:5:10"), "{}", r.declared);
    assert_eq!(r.errors, ["IoError"]);
    assert_eq!(r.effects, ["files.read", "allocates"]);
    assert_eq!(r.layer, "std");
    assert!(
        r.calls.contains(&"std.files.read_text".to_string()),
        "{:?}",
        r.calls
    );
    assert_eq!(r.usages.len(), 1, "{:?}", r.usages);
    assert!(r.usages[0].ends_with("app.unio:9:16"), "{:?}", r.usages);
    let text = r.text();
    assert!(text.contains("effects: files.read, allocates"), "{text}");
    assert!(text.contains("hash: unio-hash-v1:sha384:"), "{text}");
    let json = r.json();
    assert!(
        json.contains("\"effects\":[\"files.read\",\"allocates\"]"),
        "{json}"
    );
    assert!(json.contains("\"hash\":\"unio-hash-v1:sha384:"), "{json}");
    let types = unio::symbol("std.files.IoError", &dir);
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].kind, "type");
    assert!(unio::symbol("app.missing", &dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

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

#[test]
fn the_core_profile_reports_functions_that_need_a_higher_layer_with_the_chain() {
    let source = "\
function double(x as Int) returns Int:
    return x * 2

function count(text as Text) returns Int:
    return words(text)

function words(text as Text) returns Int:
    return text.split_whitespace().length()
";
    let dir = package("profile", &[("core.unio", source)]);
    assert!(codes(&dir, "core.unio", Profile::Application).is_empty());
    let found = codes(&dir, "core.unio", Profile::Core);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!((found[0].0.as_str(), found[0].1), ("E0084", 4));
    assert!(
        found[0].2.contains("core.count -> core.words -> "),
        "{}",
        found[0].2
    );
    assert_eq!((found[1].0.as_str(), found[1].1), ("E0084", 7));
    let _ = std::fs::remove_dir_all(&dir);
}

const READER: &str = "\
import std.files as files

function load(path as Path) returns Text fails with IoError:
    return try files.read_text(path)

public function main() returns Int:
    let text = try load(Path(\"a.txt\")) on IoError as e:
        return 1
    return text.length()
";

/// Every external of std.files states its scope, because the scope is its effect (semantics 8.2).
#[test]
fn every_external_of_std_files_states_its_permission() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std/files.unio");
    let source = std::fs::read_to_string(root).unwrap();
    let lines: Vec<&str> = source.lines().collect();
    let mut missing = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("external function") {
            continue;
        }
        let mut doc = String::new();
        let mut j = i;
        while j > 0 && lines[j - 1].trim_start().starts_with("##") {
            j -= 1;
            doc.push_str(lines[j]);
        }
        if !doc.contains("Permission: files.") && !doc.contains("Permission: none") {
            missing.push(line.trim().to_string());
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}

#[test]
fn the_manifest_permits_effects() {
    let manifest = |permissions: &str| {
        format!("[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\n{permissions}")
    };
    let denied = package(
        "denied",
        &[
            ("app.unio", READER),
            ("unio.toml", &manifest("files.read = []\n")),
        ],
    );
    let found = codes(&denied, "app.unio", Profile::Application);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].0.as_str(), found[0].1), ("E0085", 4));
    assert!(
        found[0].2.contains("app.load -> std.files.read_text"),
        "{}",
        found[0].2
    );
    let allowed = package(
        "allowed",
        &[
            ("app.unio", READER),
            ("unio.toml", &manifest("files.read = [\"$CWD/**\"]\n")),
        ],
    );
    assert!(codes(&allowed, "app.unio", Profile::Application).is_empty());
    let unmanaged = package(
        "unmanaged",
        &[
            ("app.unio", READER),
            ("unio.toml", "[package]\nname = \"p\"\n"),
        ],
    );
    assert!(codes(&unmanaged, "app.unio", Profile::Application).is_empty());
    // files.read and files.write are separate scopes (T-6): writing needs files.write.
    let writer = "\
import std.files as files

function save(path as Path) fails with IoError:
    try files.write_text(path, \"x\")

public function main() returns Int:
    try save(Path(\"a.txt\")) on IoError as e:
        return 1
    return 0
";
    let read_only = package(
        "read_only",
        &[
            ("app.unio", writer),
            ("unio.toml", &manifest("files.read = [\"$CWD/**\"]\n")),
        ],
    );
    let found = codes(&read_only, "app.unio", Profile::Application);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].0.as_str(), found[0].1), ("E0085", 4));
    assert!(found[0].2.contains("files.write"), "{}", found[0].2);
    let writable = package(
        "writable",
        &[
            ("app.unio", writer),
            ("unio.toml", &manifest("files.write = [\"$CWD/**\"]\n")),
        ],
    );
    assert!(codes(&writable, "app.unio", Profile::Application).is_empty());
    let write_only = package(
        "write_only",
        &[
            ("app.unio", READER),
            ("unio.toml", &manifest("files.write = [\"$CWD/**\"]\n")),
        ],
    );
    let found = codes(&write_only, "app.unio", Profile::Application);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].2.contains("files.read"), "{}", found[0].2);
    for dir in [denied, allowed, unmanaged, read_only, writable, write_only] {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Allocating expressions need the layer `alloc` (semantics 8.4): under the core profile a
/// list literal, interpolation, a comprehension, a map literal, joining texts and a lambda that
/// captures a value are E0084 with the chain down to the expression; code without them is not.
#[test]
fn the_core_profile_reports_allocating_expressions() {
    let source = "\
function pair(x as Int) returns List[Int]:
    return [x, x]

function label(x as Int) returns Text:
    return \"x = {x}\"

function twice(x as Int) returns Int:
    return pair(x)[0] + x

function squares(xs as List[Int]) returns List[Int]:
    return [x * x for x in xs]

function table() returns Map[Text, Int]:
    return {\"a\": 1}

function joined(a as Text, b as Text) returns Text:
    return a + b

function adder(n as Int) returns Function[(Int) -> Int]:
    return function(x as Int) returns Int: x + n

function plain(x as Int) returns Int:
    let double = function(y as Int) returns Int: y * 2
    return double(x) + 1 if x > 0 else 0
";
    let dir = package("allocating", &[("core.unio", source)]);
    assert!(codes(&dir, "core.unio", Profile::Application).is_empty());
    let found = codes(&dir, "core.unio", Profile::Core);
    let lines: Vec<usize> = found.iter().map(|f| f.1).collect();
    assert_eq!(lines, [1, 4, 7, 10, 13, 16, 19], "{found:?}");
    assert!(found.iter().all(|f| f.0 == "E0084"), "{found:?}");
    let messages: Vec<&str> = found.iter().map(|f| f.2.as_str()).collect();
    assert!(
        messages[0].contains("core.pair -> a list literal on line 2"),
        "{}",
        messages[0]
    );
    assert!(
        messages[1].contains("a text with interpolation on line 5"),
        "{}",
        messages[1]
    );
    assert!(
        messages[2].contains("core.twice -> core.pair -> a list literal on line 2"),
        "{}",
        messages[2]
    );
    assert!(messages[3].contains("a list comprehension on line 11"));
    assert!(messages[4].contains("a map literal on line 14"));
    assert!(messages[5].contains("joining texts with + on line 17"));
    assert!(messages[6].contains("a lambda that captures values on line 20"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A module of the `core` layer of the standard library may not allocate (S-11): a list literal
/// there is E0084 at the literal, as a call of a higher layer is.
#[test]
fn a_core_module_of_the_library_may_not_allocate() {
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
    let root = std::env::temp_dir().join(format!("unio-symbol-{}-core-layer", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let copy = root.join("stdlib");
    for file in [
        "core/prelude.unio",
        "core/builtin/bool.unio",
        "core/builtin/float.unio",
        "core/builtin/int.unio",
        "core/builtin/option.unio",
    ] {
        let target = copy.join(file);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(stdlib.join(file), &target).unwrap();
    }
    let extra = copy.join("core/builtin/extra.unio");
    std::fs::write(
        &extra,
        "## Not a part of the library: a core module that allocates.\n\nfunction pair(x as Int) returns List[Int]:\n    return [x, x]\n",
    )
    .unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_unio"));
    command.env("UNIO_STDLIB", &copy).arg("check");
    let output = command
        .arg("--diagnostics=json")
        .arg(&extra)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{out}");
    assert!(out.contains("\"E0084\""), "{out}");
    assert!(out.contains("a list literal allocates"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}

const MANIFEST_READ_ONLY: &str =
    "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\nfiles.read = [\"$CWD/**\"]\n";

/// A call through an interface has the effects of every implementation (semantics 8.4): the
/// module that only calls `Sink.put` needs files.write too, with the chain through the
/// implementation, and `unio symbol` shows it.
#[test]
fn a_call_through_an_interface_has_the_effects_of_its_implementations() {
    let sinks = "\
import std.files as files

public interface Sink:
    function put(self, path as Path) fails with IoError

public struct FileSink:
    public text as Text

extend FileSink with Sink:
    function put(self, path as Path) fails with IoError:
        try files.write_text(path, self.text)
";
    let app = "\
import sinks
import std.files

function store(sink as sinks.Sink) fails with IoError:
    try sink.put(Path(\"a.txt\"))

public function main() returns Int:
    try store(sinks.FileSink(text = \"x\")) on IoError as e:
        return 1
    return 0
";
    let dir = package(
        "interface",
        &[
            ("sinks.unio", sinks),
            ("app.unio", app),
            ("unio.toml", MANIFEST_READ_ONLY),
        ],
    );
    let found = codes(&dir, "app.unio", Profile::Application);
    let in_app: Vec<_> = found.iter().filter(|f| f.2.contains("app.store")).collect();
    assert_eq!(in_app.len(), 1, "{found:?}");
    assert_eq!((in_app[0].0.as_str(), in_app[0].1), ("E0085", 5));
    assert!(
        in_app[0]
            .2
            .contains("app.store -> sinks.Sink.put -> sinks.FileSink.put -> std.files.write_text"),
        "{}",
        in_app[0].2
    );
    let reports = unio::symbol("app.store", &dir);
    assert!(
        reports[0].effects.contains(&"files.write"),
        "{:?}",
        reports[0].effects
    );
    // With the permission there is nothing to report.
    std::fs::write(
        dir.join("unio.toml"),
        MANIFEST_READ_ONLY.replace("files.read", "files.write"),
    )
    .unwrap();
    assert!(codes(&dir, "app.unio", Profile::Application).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A call through a function value has the effects of the values it may be (RFC 0005): a
/// call through a constant that holds a lambda needs what the body of the lambda needs, and a
/// call through a parameter whose default is that constant counts for the caller that relies
/// on the default, not for the function with the parameter.
#[test]
fn a_call_through_a_function_value_has_the_effects_of_the_value() {
    let actions = "\
import std.files as files

## Writes a mark into a file.
public let SAVE = function(path as Path) fails with IoError: try files.write_text(path, \"x\")
";
    let app = "\
import actions
import std.files

function save(path as Path) fails with IoError:
    try actions.SAVE(path)

function keep(path as Path, write as Function[(Path) fails with IoError] = actions.SAVE) fails with IoError:
    try write(path)

public function main() returns Int:
    try save(Path(\"a.txt\")) on IoError as e:
        return 1
    try keep(Path(\"b.txt\")) on IoError as e:
        return 1
    return 0
";
    let dir = package(
        "function_value",
        &[
            ("actions.unio", actions),
            ("app.unio", app),
            ("unio.toml", MANIFEST_READ_ONLY),
        ],
    );
    let found = codes(&dir, "app.unio", Profile::Application);
    let in_app: Vec<_> = found.iter().filter(|f| f.2.contains("app.")).collect();
    assert_eq!(in_app.len(), 1, "{found:?}");
    assert_eq!((in_app[0].0.as_str(), in_app[0].1), ("E0085", 5));
    assert!(
        in_app[0].2.contains(
            "app.save -> actions.SAVE -> the lambda on line 4 in actions.SAVE -> std.files.write_text"
        ),
        "{}",
        in_app[0].2
    );
    // The default of the parameter counts where the call relies on it.
    let keep = &unio::symbol("app.keep", &dir)[0];
    assert!(!keep.effects.contains(&"files.write"), "{:?}", keep.effects);
    assert_eq!(keep.argument_effects, ["write"]);
    let main = &unio::symbol("app.main", &dir)[0];
    assert!(main.effects.contains(&"files.write"), "{:?}", main.effects);
    let _ = std::fs::remove_dir_all(&dir);
}

const MANIFEST_NOTHING: &str = "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\n";

/// RFC 0005: making a function value does not run it. A function that only keeps a
/// reference to `files.read_text` has no `files.read`, and a package without the permission
/// that never calls the value is not E0085.
#[test]
fn a_stored_function_value_has_no_effect_until_it_is_called() {
    let app = "\
import std.files as files

function store() returns Int:
    let reader = files.read_text
    let kept = [reader]
    return kept.length()
";
    let dir = package(
        "stored_value",
        &[("app.unio", app), ("unio.toml", MANIFEST_NOTHING)],
    );
    let found = codes(&dir, "app.unio", Profile::Application);
    assert!(found.is_empty(), "{found:?}");
    let store = unio::symbol("app.store", &dir);
    assert_eq!(store[0].effects, ["allocates"], "{:?}", store[0].effects);
    let _ = std::fs::remove_dir_all(&dir);
}

const APPLY: &str = "\
import std.files as files

function apply(f as Function[(Path) -> Text fails with IoError], p as Path) returns Text fails with IoError:
    return try f(p)

function constant(p as Path) returns Text fails with IoError:
    return \"x\"

function plain() returns Text fails with IoError:
    return try apply(constant, Path(\"a\"))

function reading() returns Text fails with IoError:
    return try apply(files.read_text, Path(\"a\"))
";

/// RFC 0005: a call through a parameter is a latent effect of the function. Each call of the
/// function carries the effects of the value it passes: `reading` passes `files.read_text`
/// and needs `files.read`, `plain` passes a pure function and needs nothing, and `apply`
/// itself has no effect. E0085 shows the path through the argument.
#[test]
fn a_call_through_a_parameter_carries_the_effects_of_the_argument_to_the_caller() {
    let dir = package(
        "apply",
        &[("app.unio", APPLY), ("unio.toml", MANIFEST_NOTHING)],
    );
    let found = codes(&dir, "app.unio", Profile::Application);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].0.as_str(), found[0].1), ("E0085", 13));
    assert!(
        found[0]
            .2
            .contains("app.reading -> app.apply (argument f) -> std.files.read_text"),
        "{}",
        found[0].2
    );
    let effects = |name: &str| unio::symbol(name, &dir)[0].effects.clone();
    assert!(
        effects("app.apply").is_empty(),
        "{:?}",
        effects("app.apply")
    );
    assert!(
        effects("app.plain").is_empty(),
        "{:?}",
        effects("app.plain")
    );
    assert!(effects("app.reading").contains(&"files.read"));
    let apply = &unio::symbol("app.apply", &dir)[0];
    assert_eq!(apply.argument_effects, ["f"]);
    assert!(
        apply.text().contains("  argument effects: f\n"),
        "{}",
        apply.text()
    );
    assert!(
        apply.json().contains("\"argument_effects\":[\"f\"]"),
        "{}",
        apply.json()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// RFC 0005: the higher-order functions of the library carry the effects of the function
/// they get to their caller, not to themselves.
#[test]
fn library_higher_order_functions_carry_the_effects_of_their_argument_to_the_caller() {
    let app = "\
import std.files as files

function size(p as Path) returns Int:
    let text = try files.read_text(p) on IoError as e:
        return 0
    return text.length()

function sizes(paths as List[Path]) returns List[Int]:
    return paths.map(size)

function longest_first(paths as List[Path]) returns List[Path]:
    return paths.sorted_by(function (p as Path) returns Int: 0 - size(p))

function texts(paths as List[Path]) returns List[Text] fails with IoError:
    return try paths.try_map(files.read_text)

function lengths(words as List[Text]) returns List[Int]:
    return words.map(function (w as Text) returns Int: w.length())
";
    let dir = package("higher_order", &[("app.unio", app)]);
    assert!(codes(&dir, "app.unio", Profile::Application).is_empty());
    for name in ["app.sizes", "app.longest_first", "app.texts"] {
        let r = &unio::symbol(name, &dir)[0];
        assert!(r.effects.contains(&"files.read"), "{name}: {:?}", r.effects);
    }
    let lengths = &unio::symbol("app.lengths", &dir)[0];
    assert!(
        !lengths.effects.contains(&"files.read"),
        "{:?}",
        lengths.effects
    );
    for (name, parameter) in [
        ("List.map", "change"),
        ("List.sorted_by", "key"),
        ("List.try_map", "change"),
    ] {
        let r = &unio::symbol(name, &dir)[0];
        assert!(
            !r.effects.contains(&"files.read"),
            "{name}: {:?}",
            r.effects
        );
        assert_eq!(r.argument_effects, [parameter], "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// RFC 0005: a function value kept in a field is called with every value stored in that field
/// anywhere in the program. Under the core profile such a call whose possible values need more
/// than core is E0087 at the call; a field that only ever holds core functions is accepted.
#[test]
fn a_call_through_a_field_has_the_values_stored_in_the_field() {
    let pure = "\
struct Handler:
    run as Function[(Int) -> Int]

function double(x as Int) returns Int:
    return x * 2

function call(h as Handler) returns Int:
    return h.run(1)

function make() returns Handler:
    return Handler(run = double)
";
    let dir = package("field_pure", &[("core.unio", pure)]);
    let found = codes(&dir, "core.unio", Profile::Core);
    assert!(found.is_empty(), "{found:?}");
    let _ = std::fs::remove_dir_all(&dir);
    let mixed = format!(
        "{pure}
function label(x as Int) returns Int:
    return \"{{x}}\".length()

function make_label() returns Handler:
    return Handler(run = label)
"
    );
    let dir = package("field_mixed", &[("core.unio", &mixed)]);
    assert!(codes(&dir, "core.unio", Profile::Application).is_empty());
    let found = codes(&dir, "core.unio", Profile::Core);
    let lines: Vec<(&str, usize)> = found.iter().map(|f| (f.0.as_str(), f.1)).collect();
    assert_eq!(lines, [("E0087", 8), ("E0084", 13)], "{found:?}");
    assert!(
        found[0].2.contains("core.Handler.run") && found[0].2.contains("core.label"),
        "{}",
        found[0].2
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// RFC 0005: making a lambda that captures a value allocates where it is made; the body of
/// a lambda counts where the lambda is called, so a function that only returns a lambda whose
/// body allocates does not allocate, and the function that calls the result does.
#[test]
fn a_lambda_body_counts_where_it_is_called_and_a_capture_where_it_is_made() {
    let source = "\
function maker() returns Function[(Int) -> List[Int]]:
    return function (x as Int) returns List[Int]: [x]

function user() returns Int:
    return maker()(1)[0]

function adder(n as Int) returns Function[(Int) -> Int]:
    return function (x as Int) returns Int: x + n
";
    let dir = package("lambda_body", &[("core.unio", source)]);
    let found = codes(&dir, "core.unio", Profile::Core);
    let lines: Vec<(&str, usize)> = found.iter().map(|f| (f.0.as_str(), f.1)).collect();
    assert_eq!(lines, [("E0084", 4), ("E0084", 7)], "{found:?}");
    assert!(
        found[0]
            .2
            .contains("core.user -> the values core.maker returns -> the lambda on line 2 in core.maker -> a list literal on line 2"),
        "{}",
        found[0].2
    );
    assert!(
        found[1]
            .2
            .contains("a lambda that captures values on line 8"),
        "{}",
        found[1].2
    );
    let _ = std::fs::remove_dir_all(&dir);
}
