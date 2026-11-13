//! `unio fmt --migrate` (R-9): every registered migration rewrites a program written before its
//! language change into the program the catalog shows as fixed, the result has no diagnostic of
//! that code, and a second run changes nothing.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use unio::catalog;
use unio::migrate::MIGRATIONS;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-migrate-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn unio(dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command.current_dir(dir);
    command
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

#[test]
fn every_migration_turns_the_catalog_example_into_its_fix_once() {
    for migration in MIGRATIONS {
        let entry = catalog::entry(migration.code).expect("an entry");
        let dir = scratch(migration.id);
        let file = dir.join("example.unio");
        fs::write(&file, entry.example).unwrap();
        let output = unio(&dir)
            .args(["fmt", "--migrate", "example.unio"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            migration.id,
            text(&output.stderr)
        );
        assert!(
            text(&output.stdout).contains(migration.id),
            "{}: the report names the migration: {}",
            migration.id,
            text(&output.stdout)
        );
        let migrated = fs::read_to_string(&file).unwrap();
        let fixed = unio::format::format_source(entry.fixed).unwrap();
        assert_eq!(migrated, fixed, "{}", migration.id);
        let check = unio(&dir).args(["check", "example.unio"]).output().unwrap();
        assert!(check.status.success(), "{}", text(&check.stdout));
        // Idempotent: nothing is left to migrate.
        let again = unio(&dir)
            .args(["fmt", "--migrate", "--check", "example.unio"])
            .output()
            .unwrap();
        assert!(again.status.success(), "{}", text(&again.stdout));
        assert_eq!(text(&again.stdout), "");
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn check_lists_the_file_and_from_skips_older_migrations() {
    let entry = catalog::entry("E0072").unwrap();
    let dir = scratch("check");
    let file = dir.join("old.unio");
    fs::write(&file, entry.example).unwrap();
    let output = unio(&dir)
        .args(["fmt", "--migrate", "--check", "old.unio"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stdout).contains("old.unio"));
    assert_eq!(fs::read_to_string(&file).unwrap(), entry.example);
    // The variant rule came with language 0.4: migrating from 0.4 leaves it alone.
    let output = unio(&dir)
        .args(["fmt", "--migrate", "--from=0.4", "old.unio"])
        .output()
        .unwrap();
    assert!(!text(&output.stdout).contains("variant-in-full"));
    let bad = unio(&dir)
        .args(["fmt", "--migrate", "--from=new", "old.unio"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let list = unio(&dir).args(["fmt", "--migrate"]).output().unwrap();
    for migration in MIGRATIONS {
        assert!(text(&list.stdout).contains(migration.id));
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_ambiguous_label_gets_no_fix_and_is_not_migrated() {
    let dir = scratch("ambiguous");
    let file = dir.join("two.unio");
    let source = "function f(named a as Int, named b as Int) returns Int:\n    return a + b\n\nlet X as Int = f(1, b = 2)\nlet Y as Int = f(1, 2)\n";
    fs::write(&file, source).unwrap();
    let output = unio(&dir)
        .args(["fmt", "--migrate", "two.unio"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let migrated = fs::read_to_string(&file).unwrap();
    // One open parameter: the label is known. Two open: the first argument could be either.
    assert!(migrated.contains("f(a = 1, b = 2)"), "{migrated}");
    assert!(migrated.contains("f(1, 2)"), "{migrated}");
    let _ = fs::remove_dir_all(&dir);
}
