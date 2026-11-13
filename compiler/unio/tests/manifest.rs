//! `unio check` validates the whole `unio.toml` of the package (T-6, E0088): the name, the
//! version, `unsafe_modules`, `[permissions]` with their scopes and placeholders, and the forms
//! of `[dependencies]`. Each mistake is one E0088 at its line of the manifest (semantics 2.8),
//! from `unio check` and not only from `unio run` and `unio test`; an invalid manifest gives no
//! E0085 or E0048 of its own besides.

#[allow(dead_code)]
mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A package in a temporary directory with the manifest and `main.unio`.
struct Package {
    dir: PathBuf,
}

impl Package {
    fn new(name: &str, manifest: &str, main: &str) -> Package {
        let dir = std::env::temp_dir().join(format!("unio-manifest-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("unio.toml"), manifest).unwrap();
        fs::write(dir.join("main.unio"), main).unwrap();
        Package { dir }
    }

    /// (code, file name, line) of every error `unio::check_files` reports.
    fn errors(&self) -> Vec<(String, String, usize)> {
        let mut found = Vec::new();
        for analysis in unio::check_files(&[self.dir.join("main.unio")]) {
            let file = PathBuf::from(&analysis.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            for d in &analysis.diagnostics {
                if d.severity == unio::Severity::Error {
                    found.push((d.code.to_string(), file.clone(), d.span.start.line));
                }
            }
        }
        found
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const MAIN: &str = "public function main() returns Int:\n    return 0\n";

const HEAD: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n";

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Each malformed form, with the line of its mistake.
fn malformed() -> Vec<(&'static str, String, usize)> {
    let p = |rest: &str| format!("{HEAD}[permissions]\n{rest}\n");
    let d = |rest: &str| format!("{HEAD}[dependencies]\n{rest}\n");
    vec![
        // The finding of the R-10 model trial: a permission that is not a list of scopes.
        ("read_true", p("files.read = true"), 6),
        ("write_text", p("files.write = \"$HOME/x\""), 6),
        ("unknown_placeholder", p("files.read = [\"$HOEM/data\"]"), 6),
        ("star_inside", p("files.read = [\"$HOME/*.txt\"]"), 6),
        ("empty_path", p("files.read = [\"$CWD/**\", \"\"]"), 6),
        ("env_text", p("env = \"HOME\""), 6),
        ("env_assignment", p("env = [\"HOME=/root\"]"), 6),
        ("process_true", p("process = true"), 6),
        ("process_empty", p("process = [\"\"]"), 6),
        ("network_true", p("network = true"), 6),
        ("unknown_permission", p("net = false"), 6),
        (
            "list_over_lines",
            p("files.read = [\n    \"$CWD/**\",\n    \"$WORK/x\",\n]"),
            6,
        ),
        (
            "name_spaces",
            "[package]\nname = \"Bad Name\"\n".to_string(),
            2,
        ),
        ("name_number", "[package]\nname = 3\n".to_string(), 2),
        (
            "version_word",
            "[package]\nname = \"app\"\nversion = \"one\"\n".to_string(),
            3,
        ),
        (
            "unsafe_text",
            "[package]\nname = \"app\"\nunsafe_modules = \"raw\"\n".to_string(),
            3,
        ),
        (
            "unknown_package_key",
            "[package]\nname = \"app\"\nnmae = \"app\"\n".to_string(),
            3,
        ),
        (
            "unknown_table",
            format!("{HEAD}[permision]\nfiles.read = []\n"),
            5,
        ),
        ("dependency_text", d("util = \"1.0\""), 6),
        ("dependency_alias", d("Util = { path = \"../util\" }"), 6),
        (
            "dependency_two_sources",
            d("u = { path = \"x\", archive = \"y\" }"),
            6,
        ),
        (
            "dependency_no_version",
            d("u = { registry = \"main\", package = \"acme/u\" }"),
            6,
        ),
        (
            "dependency_bad_package",
            d("u = { registry = \"main\", package = \"u\", version = \"1\" }"),
            6,
        ),
        ("dependency_no_hash", d("u = { archive = \"x.tar\" }"), 6),
        (
            "dependency_short_hash",
            d("u = { archive = \"x.tar\", sha384 = \"abc\" }"),
            6,
        ),
        (
            "dependency_unknown_key",
            d("u = { path = \"x\", colour = \"red\" }"),
            6,
        ),
        (
            "dependency_bad_version",
            d("u = { path = \"x\", version = \"one\" }"),
            6,
        ),
        ("dependency_empty_path", d("u = { path = \"\" }"), 6),
        (
            "dependency_table_form",
            format!("{HEAD}[dependencies.vendored]\narchive = \"v.tar\"\n"),
            5,
        ),
        ("syntax_header", format!("{HEAD}[permissions\n"), 5),
        (
            "syntax_twice",
            format!("{HEAD}[permissions]\nenv = []\nenv = []\n"),
            7,
        ),
        (
            "syntax_value",
            format!("{HEAD}[permissions]\nenv = [HOME]\n"),
            6,
        ),
    ]
}

#[test]
fn every_malformed_manifest_is_one_e0088_at_its_line() {
    let mut failures = Vec::new();
    for (name, manifest, line) in malformed() {
        let package = Package::new(name, &manifest, MAIN);
        let found = package.errors();
        let expected = vec![("E0088".to_string(), "unio.toml".to_string(), line)];
        if found != expected {
            failures.push(format!("{name}: expected {expected:?}, got {found:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_mistake_of_a_manifest_is_reported_once() {
    let manifest = format!(
        "{HEAD}[permissions]\nfiles.read = true\nenv = [\"HOME\"]\nnetwork = true\n\n\
         [dependencies]\nutil = \"1.0\"\n"
    );
    let package = Package::new("several", &manifest, MAIN);
    let lines: Vec<usize> = package.errors().iter().map(|e| e.2).collect();
    assert_eq!(lines, [6, 8, 11]);
    assert!(package.errors().iter().all(|e| e.0 == "E0088"));
}

#[test]
fn a_valid_manifest_gives_no_diagnostic() {
    let manifest = format!(
        "{HEAD}unsafe_modules = []\n\n[permissions]\nfiles.read = [\"$CWD/**\", \"data/in.txt\", \"/etc/hosts\"]\n\
         files.write = [\"$TEMP/**\", \"$HOME/.app.json\"]\nenv = [\"HOME\", \"APP_MODE\"]\n\
         process = [\"git\", \"/usr/bin/env\"]\nnetwork = false\n\n[dependencies]\n\
         util = {{ path = \"../util\" }}\njson = {{ registry = \"main\", package = \"acme/json\", version = \"^1.2\" }}\n\
         [dependencies.vendored]\narchive = \"vendor/v.tar\"\nsha384 = \"{HASH}\"\n"
    );
    let package = Package::new("valid", &manifest, MAIN);
    assert_eq!(package.errors(), []);
    let minimal = Package::new("minimal", "[package]\nname = \"demo\"\n", MAIN);
    assert_eq!(minimal.errors(), []);
}

/// One code per mistake: a permission that is not a list is E0088 only, without E0085 for the
/// read it was meant to permit.
#[test]
fn an_invalid_manifest_adds_no_effect_error() {
    let main = "import std.files\n\npublic function main() returns Int:\n    let text = try files.read_text(Path(\"a\")) on IoError:\n        return 1\n    return text.length()\n";
    for (name, permissions) in [
        ("effect_true", "files.read = true"),
        ("effect_syntax", "files.read = [\"$CWD/**\""),
    ] {
        let manifest = format!("{HEAD}[permissions]\n{permissions}\n");
        let package = Package::new(name, &manifest, main);
        let codes: Vec<String> = package.errors().into_iter().map(|e| e.0).collect();
        assert_eq!(codes, ["E0088"], "{name}");
    }
}

/// A dependency written in a wrong form is E0088 in the manifest, not E0048 at its import too.
#[test]
fn an_invalid_dependency_adds_no_import_error() {
    let manifest = format!("{HEAD}[dependencies]\nutil = \"1.0\"\n");
    let main = "import deps.util.text\n\npublic function main() returns Int:\n    return 0\n";
    let package = Package::new("dependency_import", &manifest, main);
    let codes: Vec<String> = package.errors().into_iter().map(|e| e.0).collect();
    assert_eq!(codes, ["E0088"]);
}

/// The command reports the manifest as a file of its own, in text and JSON, and fails.
#[test]
fn unio_check_reports_the_manifest_line_and_fails() {
    let manifest = format!("{HEAD}[permissions]\nfiles.read = true\n");
    let package = Package::new("command", &manifest, MAIN);
    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
        command.current_dir(&package.dir).args(args);
        command.output().unwrap()
    };
    let text = run(&["check", "main.unio"]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert_eq!(text.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("error[E0088]"), "{stdout}");
    assert!(stdout.contains("--> unio.toml:6:1"), "{stdout}");
    assert!(stdout.contains("files.read"), "{stdout}");
    let json = run(&["check", "--diagnostics=json", "main.unio"]);
    let stdout = String::from_utf8_lossy(&json.stdout);
    assert_eq!(json.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("\"code\":\"E0088\""), "{stdout}");
    assert!(stdout.contains("\"file\":\"unio.toml\""), "{stdout}");
    let ru = run(&["check", "--lang=ru", "main.unio"]);
    let stdout = String::from_utf8_lossy(&ru.stdout);
    assert!(stdout.contains("error[E0088]"), "{stdout}");
    assert!(
        !stdout.contains("must be"),
        "the Russian message is Russian: {stdout}"
    );
}
