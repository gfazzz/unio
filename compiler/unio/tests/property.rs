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
