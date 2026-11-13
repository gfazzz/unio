//! `unio new` (table 6.1): a program or a library package that `unio check`, `unio test`, and
//! `unio fmt --check` accept as written, and that `unio run` and `unio doc` take.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn all(output: &Output) -> String {
    text(&output.stdout) + &text(&output.stderr)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-new-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn unio(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .args(args)
        .output()
        .unwrap()
}

#[track_caller]
fn succeeds(dir: &Path, args: &[&str]) -> String {
    let output = unio(dir, args);
    assert_eq!(output.status.code(), Some(0), "{args:?}: {}", all(&output));
    text(&output.stdout)
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_new_program_checks_tests_formats_and_runs() {
    let root = scratch("program");
    succeeds(&root, &["new", "hello-app"]);
    let dir = root.join("hello-app");
    assert_eq!(files(&dir), [".gitignore", "main.unio", "unio.toml"]);
    let manifest = fs::read_to_string(dir.join("unio.toml")).unwrap();
    assert!(manifest.contains("name = \"hello-app\""), "{manifest}");
    assert!(manifest.contains("version = \"0.1.0\""), "{manifest}");
    succeeds(&dir, &["check", "main.unio"]);
    succeeds(&dir, &["fmt", "--check", "main.unio"]);
    let tests = succeeds(&dir, &["test", "main.unio"]);
    assert!(
        tests.contains("the greeting names whom it greets"),
        "{tests}"
    );
    assert_eq!(succeeds(&dir, &["run", "main.unio"]), "Hello, world!\n");
    assert_eq!(
        succeeds(&dir, &["run", "main.unio", "--", "Anna"]),
        "Hello, Anna!\n"
    );
    // `--bin` is the default.
    succeeds(&root, &["new", "--bin", "other"]);
    assert_eq!(
        fs::read_to_string(root.join("other/main.unio")).unwrap(),
        fs::read_to_string(dir.join("main.unio"))
            .unwrap()
            .replace("hello-app", "other")
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_new_library_checks_tests_formats_and_documents() {
    let root = scratch("library");
    succeeds(&root, &["new", "--lib", "word-tools"]);
    let dir = root.join("word-tools");
    assert_eq!(files(&dir), [".gitignore", "unio.toml", "word_tools.unio"]);
    succeeds(&dir, &["check", "word_tools.unio"]);
    succeeds(&dir, &["fmt", "--check", "word_tools.unio"]);
    let tests = succeeds(&dir, &["test", "word_tools.unio"]);
    // The test block and the example of the documentation comment.
    assert!(
        tests.contains("the greeting names whom it greets"),
        "{tests}"
    );
    assert!(tests.contains("doc: greeting"), "{tests}");
    succeeds(&dir, &["doc"]);
    let page = fs::read_to_string(dir.join("target/doc/word_tools.html")).unwrap();
    assert!(page.contains("<p>The greeting for a name.</p>"), "{page}");
    // An application uses it through a path dependency.
    succeeds(&root, &["new", "app"]);
    let app = root.join("app");
    succeeds(&app, &["add", "words", "--path", "../word-tools"]);
    fs::write(
        app.join("main.unio"),
        "import deps.words.word_tools\n\npublic function main():\n    print(word_tools.greeting(\"Anna\"))\n",
    )
    .unwrap();
    assert_eq!(succeeds(&app, &["run", "main.unio"]), "Hello, Anna!\n");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_new_package_is_made_in_a_path_and_never_over_a_directory() {
    let root = scratch("paths");
    succeeds(&root, &["new", "nested/dir/tool"]);
    let dir = root.join("nested/dir/tool");
    assert!(dir.join("main.unio").is_file());
    let before = fs::read_to_string(dir.join("main.unio")).unwrap();
    let output = unio(&root, &["new", "nested/dir/tool"]);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("already exists"),
        "{}",
        all(&output)
    );
    assert_eq!(fs::read_to_string(dir.join("main.unio")).unwrap(), before);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn wrong_names_and_options_are_usage_errors() {
    let root = scratch("usage");
    for args in [
        vec!["new"],
        vec!["new", "Upper"],
        vec!["new", "9lives"],
        vec!["new", "has space"],
        vec!["new", "--lib", "--bin", "both"],
        vec!["new", "--unknown", "name"],
        vec!["new", "one", "two"],
        // The module of a library would be a keyword, or a prefix of other modules.
        vec!["new", "--lib", "match"],
        vec!["new", "--lib", "std"],
    ] {
        let output = unio(&root, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", all(&output));
    }
    assert!(files(&root).is_empty(), "{:?}", files(&root));
    // A program may be named like a keyword: its module is main.unio.
    succeeds(&root, &["new", "match"]);
    succeeds(&root.join("match"), &["check", "main.unio"]);
    let _ = fs::remove_dir_all(&root);
}
