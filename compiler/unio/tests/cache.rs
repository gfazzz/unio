//! The incremental test cache (C-11a, C-11b; docs/tools.md, "The cache"): `unio test --json`
//! reports the content hash of every test; a test that passed and read nothing from outside
//! the program is reused while its key matches; a change of any input of the key runs it
//! again; and a damaged entry is detected, never trusted, and made again.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-cache-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn unio(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .args(args)
        .output()
        .unwrap()
}

const MANIFEST: &str = "[package]\nname = \"cached\"\nversion = \"0.1.0\"\n";

const HELPER: &str = "public function double(n as Int) returns Int:\n    return n * 2\n";

const TESTS: &str = "import helper
import std.time

test \"pure\":
    assert helper.double(2) == 4

test \"reads the clock\":
    let stamp = time.now()
    assert stamp == stamp

test \"fails\":
    assert helper.double(2) == 5
";

/// The JSON record of the test `name` in the output of `unio test --json`.
fn record<'a>(json: &'a str, name: &str) -> &'a str {
    let marker = format!("\"name\": \"{name}\"");
    let start = json
        .find(&marker)
        .unwrap_or_else(|| panic!("no test {name}: {json}"));
    let end = json[start..].find('}').map_or(json.len(), |e| start + e);
    &json[start..end]
}

fn cached(json: &str, name: &str) -> bool {
    let r = record(json, name);
    assert!(r.contains("\"hash\": \"unio-hash-v1:sha384:"), "{r}");
    r.contains("\"cached\": true")
}

/// The hash of the test `name`.
fn hash(json: &str, name: &str) -> String {
    let r = record(json, name);
    let start = r.find("\"hash\": \"").unwrap() + 9;
    r[start..start + r[start..].find('"').unwrap()].to_string()
}

fn run(dir: &Path, extra: &[&str]) -> String {
    let mut args = vec!["test", "--json"];
    args.extend_from_slice(extra);
    args.push("t.unio");
    let output = unio(dir, &args);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn all(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr)
}

fn package(name: &str) -> PathBuf {
    let dir = scratch(name);
    fs::write(dir.join("unio.toml"), MANIFEST).unwrap();
    fs::write(dir.join("helper.unio"), HELPER).unwrap();
    fs::write(dir.join("t.unio"), TESTS).unwrap();
    dir
}

/// The entries of the test results in the cache of the package.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir.join("target/unio-cache/tests"))
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// A second run reuses the result of the test that passed and read no outside input; the
/// test that read the clock and the test that failed run again. The hash is reported for every
/// test, cached or not, and is stable.
#[test]
fn a_second_run_reuses_what_passed_and_read_nothing_from_outside() {
    let dir = package("reuse");
    let first = run(&dir, &[]);
    for name in ["pure", "reads the clock", "fails"] {
        assert!(!cached(&first, name), "{first}");
    }
    let second = run(&dir, &[]);
    assert!(cached(&second, "pure"), "{second}");
    assert!(!cached(&second, "reads the clock"), "{second}");
    assert!(!cached(&second, "fails"), "{second}");
    assert!(record(&second, "pure").contains("\"status\": \"passed\""));
    for name in ["pure", "reads the clock", "fails"] {
        assert_eq!(hash(&first, name), hash(&second, name));
    }
    // Text mode says which result was reused.
    let text = unio(&dir, &["test", "t.unio"]);
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("ok   t.unio: pure (cached)"),
        "{}",
        all(&text)
    );
    // Exactly one entry: the result of `pure`.
    assert_eq!(entries(&dir).len(), 1, "{:?}", entries(&dir));
    let _ = fs::remove_dir_all(&dir);
}

/// Every input of the key invalidates the result: a module the test uses, the test file
/// (formatting included, since traces name lines), the manifest and its permissions, the seed,
/// and the policy. A comment changes the source digest but not the content hash of the test.
#[test]
fn a_change_of_any_input_runs_the_test_again() {
    let dir = package("invalidate");
    run(&dir, &[]);
    assert!(cached(&run(&dir, &[]), "pure"));
    let pure_hash = hash(&run(&dir, &[]), "pure");
    // A module the test uses: another hash, and the test runs.
    fs::write(
        dir.join("helper.unio"),
        "public function double(n as Int) returns Int:\n    return n + n\n",
    )
    .unwrap();
    let changed = run(&dir, &[]);
    assert!(!cached(&changed, "pure"), "{changed}");
    assert_ne!(hash(&changed, "pure"), pure_hash);
    assert!(cached(&run(&dir, &[]), "pure"));
    // A comment in the test file: the same content hash, but the sources changed.
    fs::write(dir.join("t.unio"), format!("# a comment\n{TESTS}")).unwrap();
    let commented = run(&dir, &[]);
    assert!(!cached(&commented, "pure"), "{commented}");
    assert!(cached(&run(&dir, &[]), "pure"));
    // The permissions of the manifest.
    fs::write(
        dir.join("unio.toml"),
        format!("{MANIFEST}\n[permissions]\nenv = [\"HOME\"]\n"),
    )
    .unwrap();
    assert!(!cached(&run(&dir, &[]), "pure"));
    assert!(cached(&run(&dir, &[]), "pure"));
    // The seed of the properties.
    assert!(!cached(&run(&dir, &["--seed", "5"]), "pure"));
    assert!(cached(&run(&dir, &["--seed", "5"]), "pure"));
    assert!(cached(&run(&dir, &[]), "pure"));
    // The policy and its limits.
    fs::write(dir.join("policy.toml"), "[limits]\nwall_seconds = 30\n").unwrap();
    assert!(!cached(&run(&dir, &["--policy", "policy.toml"]), "pure"));
    assert!(cached(&run(&dir, &["--policy", "policy.toml"]), "pure"));
    let _ = fs::remove_dir_all(&dir);
}

/// A damaged entry is detected by its digest and never trusted: the test runs, and its new
/// result replaces the entry. An entry of another key copied in its place does not match.
#[test]
fn a_damaged_entry_is_detected_and_made_again() {
    let dir = package("damaged");
    run(&dir, &[]);
    assert!(cached(&run(&dir, &[]), "pure"));
    let entry = entries(&dir).pop().unwrap();
    let good = fs::read(&entry).unwrap();
    // A flipped byte of the payload.
    let mut bad = good.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0x20;
    fs::write(&entry, &bad).unwrap();
    let after = run(&dir, &[]);
    assert!(!cached(&after, "pure"), "{after}");
    assert!(record(&after, "pure").contains("\"status\": \"passed\""));
    assert!(cached(&run(&dir, &[]), "pure"));
    assert_eq!(fs::read(&entry).unwrap(), good);
    // A truncated entry, and a forged one whose payload says another test passed.
    fs::write(&entry, &good[..good.len() / 2]).unwrap();
    assert!(!cached(&run(&dir, &[]), "pure"));
    assert!(cached(&run(&dir, &[]), "pure"));
    let forged = String::from_utf8_lossy(&good).replace("pure", "PURE");
    fs::write(&entry, forged).unwrap();
    assert!(!cached(&run(&dir, &[]), "pure"));
    assert!(cached(&run(&dir, &[]), "pure"));
    let _ = fs::remove_dir_all(&dir);
}

/// A file outside a package has no build directory: nothing is cached and nothing is written
/// next to it, but the hash is still reported.
#[test]
fn a_file_outside_a_package_is_not_cached() {
    let dir = scratch("loose");
    fs::write(dir.join("helper.unio"), HELPER).unwrap();
    fs::write(dir.join("t.unio"), TESTS).unwrap();
    run(&dir, &[]);
    let second = run(&dir, &[]);
    assert!(!cached(&second, "pure"), "{second}");
    assert!(!dir.join("target").exists());
    let _ = fs::remove_dir_all(&dir);
}
