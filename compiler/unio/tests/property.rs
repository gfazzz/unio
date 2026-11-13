//! Property tests under `unio test` (T-12, docs/language-contracts.md section 5): a failing
//! test reports the counterexample of `std.test.check_property` with its scheme, seeds, values
//! and shrink attempts and the command that replays it; `--seed` replaces the seed of every
//! property, and the same seed gives the same counterexample.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const PROPERTY: &str = "import std.test

function sample(seed as Int) returns Int:
    return seed % 1000

function smaller(value as Int) returns List[Int]:
    return [value / 2, value - 1] if value > 0 else []

test \"small numbers stay small\":
    let found = try test.check_property(sample, smaller, function (v as Int) returns Bool: v < 500, seed = 3) on test.PropertyError as e:
        assert_fail(e.message)
    assert found == none
";

fn unio(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .args(args)
        .output()
        .unwrap()
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

#[test]
fn a_failing_property_reports_its_replay_and_a_seed_replays_it() {
    let dir = std::env::temp_dir().join(format!("unio-property-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("p.unio"), PROPERTY).unwrap();
    let text = unio(&dir, &["test", "p.unio"]);
    assert_eq!(text.status.code(), Some(1));
    let text = out(&text);
    assert!(
        text.contains("property unio-property-v1: seed 3, case 3, sample seed 1216431607, original 607, minimal 500"),
        "{text}"
    );
    assert!(
        text.contains("replay: unio test --seed 3 \"small numbers stay small\" p.unio"),
        "{text}"
    );
    // The printed command gives the same counterexample.
    let replay = out(&unio(
        &dir,
        &["test", "--seed", "3", "small numbers", "p.unio"],
    ));
    assert!(replay.contains("sample seed 1216431607"), "{replay}");
    // Another seed is accepted and is deterministic.
    let first = out(&unio(&dir, &["test", "--json", "--seed=11", "p.unio"]));
    let second = out(&unio(&dir, &["test", "--json", "--seed", "11", "p.unio"]));
    let strip = |s: &str| {
        s.split("\"time_ms\"").next().unwrap().to_string()
            + s.split("\"properties\"").nth(1).unwrap_or("")
    };
    assert_eq!(strip(&first), strip(&second));
    for field in [
        "\"scheme\": \"unio-property-v1\"",
        "\"seed\": 11",
        "\"case_index\": 1",
        "\"sample_seed\": 43785881",
        "\"original\": \"881\"",
        "\"minimal\": \"500\"",
        "\"shrink_attempts\": ",
        "\"compiler\": \"unio ",
        "\"library\": ",
    ] {
        assert!(first.contains(field), "{field}: {first}");
    }
    let bad = unio(&dir, &["test", "--seed", "x", "p.unio"]);
    assert_eq!(bad.status.code(), Some(2));
    let _ = fs::remove_dir_all(&dir);
}

/// Generators and tests that never end, or never stop allocating, and the test after them.
const HANGING: &str = "import std.test
import std.time

function spin(seed as Int) returns Int:
    var n = seed
    while n >= 0:
        n = (n + 1) % 1000
    return n

function count(seed as Int) returns Int:
    var total = 0
    for i in range(0, 1000000000000000000):
        total = (total + plus_one(i)) % 1000
    return total + seed % 2

function plus_one(n as Int) returns Int:
    return n + 1

function keep(value as Int) returns List[Int]:
    return []

test \"a generator that loops\":
    let found = try test.check_property(spin, keep, function (v as Int) returns Bool: v >= 0) on test.PropertyError as e:
        assert_fail(e.message)
    assert found == none

test \"a generator that counts without end\":
    let found = try test.check_property(count, keep, function (v as Int) returns Bool: v >= 0) on test.PropertyError as e:
        assert_fail(e.message)
    assert found == none

test \"a test that sleeps\":
    time.sleep(time.minutes(10))

test \"a test that grows a list\":
    var all as List[Int] = []
    while true:
        all.append(1)

test \"the next test runs\":
    assert 1 + 1 == 2
";

/// T-12: the runner enforces time and memory limits even when a generator hangs. A generator
/// that loops or recurses without end, a test that sleeps, and a test that allocates without
/// end each fail with the limit it used up, and the tests after them still run; with a policy
/// its `[limits]` hold for each test.
#[test]
fn hanging_generators_and_tests_fail_at_their_limits_and_the_run_goes_on() {
    let dir = std::env::temp_dir().join(format!("unio-property-limits-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("h.unio"), HANGING).unwrap();
    fs::write(dir.join("policy.toml"), "[limits]\nwall_seconds = 1\n").unwrap();
    let started = std::time::Instant::now();
    let output = unio(&dir, &["test", "--policy", "policy.toml", "h.unio"]);
    let text = out(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
    for name in [
        "a generator that loops",
        "a generator that counts without end",
        "a test that sleeps",
        "a test that grows a list",
    ] {
        assert!(
            text.contains(&format!("FAIL h.unio: {name}")),
            "{name}: {text}"
        );
    }
    assert_eq!(
        text.matches("used up its wall time limit of 1 s (the [limits] of the policy)")
            .count(),
        4,
        "{text}"
    );
    assert_eq!(
        text.matches(
            "a generator, predicate or shrinker of std.test.check_property did not return"
        )
        .count(),
        2,
        "{text}"
    );
    assert!(text.contains("ok   h.unio: the next test runs"), "{text}");
    assert!(text.contains("1 passed, 4 failed"), "{text}");
    // Without a policy the heap of each test has the default limit: the list fails its test.
    let output = unio(&dir, &["test", "a test that grows a list", "h.unio"]);
    let text = out(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("the test holds more than the default heap limit of 1073741824 bytes"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// T-12: without a policy, a generator that never returns fails at the default time limit of
/// a test (60 seconds), and the next test runs: `unio test` does not hang.
#[test]
fn a_hanging_generator_without_a_policy_fails_at_the_default_limit() {
    let dir = std::env::temp_dir().join(format!("unio-property-default-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("h.unio"), HANGING).unwrap();
    let output = unio(&dir, &["test", "loops", "h.unio"]);
    let text = out(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("used up its wall time limit of 60 s (the default of unio test"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&dir);
}
