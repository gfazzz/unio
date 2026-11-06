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
