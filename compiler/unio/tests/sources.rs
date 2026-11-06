//! Every Unio source of the repository parses: reference programs, the standard library, tools.

mod common;

use common::{read, repo_root, unio_files};

#[test]
fn repository_sources_parse() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in [
        "docs/examples",
        "docs/ru/examples",
        "examples",
        "stdlib",
        "tools",
    ] {
        files.extend(unio_files(&root.join(dir)));
    }
    assert!(
        files.len() > 20,
        "expected the repository sources, found {}",
        files.len()
    );
    let mut failures = Vec::new();
    for path in &files {
        let analysis = unio::check_source(&path.display().to_string(), &read(path));
        if !analysis.diagnostics.is_empty() {
            failures.push(analysis.render_text(unio::Lang::En));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ast_dump_is_stable_and_has_no_spans() {
    let text = "function f(x as Int) returns Int:\n    return -x ** 2 + 1\n";
    let analysis = unio::check_source("a.unio", text);
    assert!(analysis.diagnostics.is_empty());
    let dump = analysis.ast_sexpr().expect("a tree");
    assert_eq!(
        dump,
        unio::check_source("b.unio", text).ast_sexpr().unwrap()
    );
    assert!(
        dump.contains("(binary + (unary - (binary ** (name x) (int 2))) (int 1))"),
        "{dump}"
    );
}
