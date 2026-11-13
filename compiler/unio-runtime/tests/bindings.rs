//! Every external declaration of stdlib/ has a binding, and every binding has a declaration
//! (docs/runtime.md, 7.2). The declarations are found by a scan of the sources, so this test
//! does not depend on the compiler.

use std::collections::BTreeSet;
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

/// The full name of every external function: `T.name` in `extend T`, the bare name in a
/// prelude or builtin file, and `std.module.name` elsewhere.
fn declared() -> BTreeSet<String> {
    let root = stdlib();
    let mut files = Vec::new();
    sources(&root, &mut files);
    let mut names = BTreeSet::new();
    for file in files {
        let relative = file.strip_prefix(&root).unwrap().with_extension("");
        let parts: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let in_module = &parts[1..];
        let prefix = if in_module == ["prelude"] || in_module[0] == "builtin" {
            String::new()
        } else {
            format!("std.{}.", in_module.join("."))
        };
        let mut extended: Option<String> = None;
        for line in std::fs::read_to_string(&file).unwrap().lines() {
            if !line.starts_with(' ') && !line.trim().is_empty() && !line.starts_with('#') {
                extended = line.strip_prefix("extend").map(|rest| {
                    let rest = rest.trim_start();
                    let rest = if rest.starts_with('[') {
                        &rest[rest.find(']').unwrap() + 1..]
                    } else {
                        rest
                    };
                    rest.trim_start()
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap()
                        .to_string()
                });
            }
            if let Some((_, rest)) = line.split_once("external function ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let full = match (&extended, line.starts_with(' ')) {
                    (Some(t), true) => format!("{t}.{name}"),
                    _ => format!("{prefix}{name}"),
                };
                assert!(names.insert(full.clone()), "{full} is declared twice");
            }
        }
    }
    names
}

#[test]
fn bindings_and_declarations_agree() {
    let declared = declared();
    let bound: BTreeSet<String> = unio_runtime::bindings()
        .keys()
        .map(|k| k.to_string())
        .collect();
    let missing: Vec<_> = declared.difference(&bound).collect();
    let extra: Vec<_> = bound.difference(&declared).collect();
    assert!(missing.is_empty(), "declared but not bound: {missing:?}");
    assert!(extra.is_empty(), "bound but not declared: {extra:?}");
    assert_eq!(
        declared.len(),
        115,
        "docs/runtime.md, section 7.1, counts 115 externals"
    );
}

/// R-5: the checklist of the two-person review lists every external the runtime binds, once,
/// and nothing else, in English and in Russian (docs/extern-review.md).
#[test]
fn the_review_checklist_lists_every_external() {
    let bound: BTreeSet<String> = unio_runtime::bindings()
        .keys()
        .map(|k| k.to_string())
        .collect();
    for document in ["docs/extern-review.md", "docs/ru/extern-review.md"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(document);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut listed = BTreeSet::new();
        for line in text.lines() {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() > 3 && cells[1].parse::<usize>().is_ok() {
                let name = cells[2].trim_matches('`').to_string();
                assert!(
                    listed.insert(name.clone()),
                    "{document}: {name} is listed twice"
                );
            }
        }
        let missing: Vec<_> = bound.difference(&listed).collect();
        let extra: Vec<_> = listed.difference(&bound).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{document}: bound but not listed: {missing:?}; listed but not bound: {extra:?}"
        );
    }
}
