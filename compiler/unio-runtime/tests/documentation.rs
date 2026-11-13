//! Requirement S-3: every module of stdlib/ is documented, and every public function and
//! constant has a documentation comment with an example. The examples are blocks under
//! `## Example:` that `unio test` runs as tests of their module (T-13); this test checks only
//! that they are there, by a scan of the sources, so it does not depend on the compiler.

use std::path::{Path, PathBuf};

fn stdlib() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib")
}

fn sources(directory: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "unio") {
            out.push(path);
        }
    }
}

/// The documentation comment that ends right above the line, without the markers.
fn comment_above(lines: &[&str], line: usize) -> Vec<String> {
    let mut at = line;
    let mut comment = Vec::new();
    while at > 0 {
        let text = lines[at - 1].trim_start();
        match text.strip_prefix("##") {
            Some(rest) if !rest.starts_with('#') => {
                comment.insert(0, rest.strip_prefix(' ').unwrap_or(rest).to_string());
                at -= 1;
            }
            _ => break,
        }
    }
    comment
}

/// Whether the comment has a sentence of description and an example: a line `Example:` or
/// `Examples:` followed by a line indented by four spaces.
fn described_with_example(comment: &[String]) -> (bool, bool) {
    let heading = |l: &str| matches!(l.trim(), "Example:" | "Examples:");
    let described = comment
        .iter()
        .take_while(|l| !heading(l))
        .any(|l| !l.trim().is_empty());
    let example = comment
        .windows(2)
        .any(|pair| heading(&pair[0]) && pair[1].starts_with("    "));
    (described, example)
}

#[test]
fn every_module_and_public_function_is_documented_with_an_example() {
    let root = stdlib();
    let mut files = Vec::new();
    sources(&root, &mut files);
    let mut problems = Vec::new();
    let mut functions = 0;
    for file in &files {
        let source = std::fs::read_to_string(file).unwrap();
        let lines: Vec<&str> = source.lines().collect();
        let name = file.strip_prefix(&root).unwrap().display().to_string();
        if !lines.first().is_some_and(|l| l.starts_with("## ")) {
            problems.push(format!("{name}: the module has no documentation comment"));
        }
        for (index, line) in lines.iter().enumerate() {
            let text = line.trim_start();
            let public = text.starts_with("public function ")
                || text.starts_with("public external function ")
                || text.starts_with("public let ");
            if !public {
                continue;
            }
            functions += 1;
            let (described, example) = described_with_example(&comment_above(&lines, index));
            if !described {
                problems.push(format!("{name}:{}: no description: {text}", index + 1));
            }
            if !example {
                problems.push(format!("{name}:{}: no example: {text}", index + 1));
            }
        }
    }
    assert!(functions > 200, "only {functions} public functions found");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
