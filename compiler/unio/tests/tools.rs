//! The tools of tools/ under `unio run` give the same results as the temporary Python
//! checkers they replace (docs/runtime.md 8.3, plan section 12): the same standard output and
//! the same exit code for every check that CI runs, and for the failure cases.
//!
//! Ignored by default: the Unio tools are interpreted and the API check takes about half a
//! minute in a release build. CI runs it with `cargo test --release --test tools -- --ignored`
//! as long as the Python checkers exist; it is deleted together with them.

#[allow(dead_code)]
mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{repo_root, unio_files};

fn python() -> &'static str {
    for candidate in ["python3", "python"] {
        let found = Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if found {
            return candidate;
        }
    }
    panic!("the comparison needs python3 or python");
}

/// Standard output with line ends and path separators made uniform, and the exit code.
fn result(command: &mut Command) -> (String, Option<i32>) {
    let output = command.current_dir(repo_root()).output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .replace('\\', "/");
    (text, output.status.code())
}

fn relative(paths: &[PathBuf]) -> Vec<String> {
    let root = repo_root();
    paths
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
#[ignore = "slow: runs the interpreted tools; CI runs it in a release build"]
fn unio_tools_agree_with_the_python_checkers() {
    let root = repo_root();
    let mut sources = Vec::new();
    for dir in [
        "docs/examples",
        "docs/ru/examples",
        "examples",
        "stdlib",
        "tools",
    ] {
        sources.extend(unio_files(&root.join(dir)));
    }
    let sources = relative(&sources);
    let invalid = relative(&unio_files(&root.join("spec/tests/invalid")));
    let valid = relative(&unio_files(&root.join("spec/tests/valid")));
    let mut tree = vec!["--tree".to_string()];
    tree.extend(valid);
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("documentation", ".github/scripts/check_docs.py", vec![]),
        (
            "grammar corpus",
            "spec/check_grammar.py",
            vec!["--corpus".into()],
        ),
        (
            "grammar self-check",
            "spec/check_grammar.py",
            vec!["--self-check".into()],
        ),
        ("sources parse", "spec/check_grammar.py", sources),
        ("invalid sources", "spec/check_grammar.py", invalid),
        ("parse trees", "spec/check_grammar.py", tree),
        ("standard library API", "spec/check_api.py", vec![]),
    ];
    let unio_tool = |script: &str| match script {
        ".github/scripts/check_docs.py" => "tools/check_docs.unio",
        "spec/check_grammar.py" => "tools/check_grammar.unio",
        _ => "tools/check_api.unio",
    };
    let mut differences = Vec::new();
    for (name, script, args) in &cases {
        let expected = result(Command::new(python()).arg(script).args(args));
        let found = result(
            Command::new(env!("CARGO_BIN_EXE_unio"))
                .arg("run")
                .arg(unio_tool(script))
                .arg("--")
                .args(args),
        );
        if expected != found {
            differences.push(format!(
                "{name}: Python exit {:?}, Unio exit {:?}\n--- Python\n{}\n--- Unio\n{}",
                expected.1, found.1, expected.0, found.0
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n\n"));
}
