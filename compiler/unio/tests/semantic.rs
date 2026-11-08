//! The semantic corpus (spec/tests/semantic) and the exit criterion of sprint S3: every Unio
//! source of the repository passes name and type checks. The corpus is checked by the compiler
//! only; spec/check_grammar.py checks syntax.

#[allow(dead_code)]
mod common;

use common::{repo_root as repository, unio_files};

fn expectation(source: &str) -> (usize, String) {
    let line = source.lines().next().unwrap_or_default();
    let rest = line
        .strip_prefix("# expect-error:")
        .unwrap_or_else(|| panic!("no '# expect-error: LINE CODE' line: {line}"));
    let mut words = rest.split_whitespace();
    let number = words.next().unwrap().parse().unwrap();
    let code = words.next().expect("a code").to_string();
    (number, code)
}

#[test]
fn valid_semantic_corpus_has_no_diagnostics() {
    let dir = repository().join("spec/tests/semantic/valid");
    for file in unio_files(&dir) {
        let results = unio::check_files(std::slice::from_ref(&file));
        for analysis in &results {
            assert!(
                analysis.diagnostics.is_empty(),
                "{}:\n{}",
                analysis.path,
                analysis.render_text(unio::Lang::En)
            );
        }
    }
}

#[test]
fn invalid_semantic_corpus_reports_the_expected_first_error() {
    let dir = repository().join("spec/tests/semantic/invalid");
    let mut failures = Vec::new();
    for file in unio_files(&dir) {
        let source = std::fs::read_to_string(&file).unwrap();
        let (line, code) = expectation(&source);
        let results = unio::check_files(std::slice::from_ref(&file));
        let analysis = results
            .iter()
            .find(|a| std::path::Path::new(&a.path) == file)
            .expect("the file is checked");
        let mut found: Vec<_> = analysis.diagnostics.iter().collect();
        found.sort_by_key(|d| (d.line(), d.span.start.column));
        match found.first() {
            Some(d) if d.line() == line && d.code == code => {}
            Some(_) => failures.push(format!(
                "{}: expected {code} on line {line}, got:\n{}",
                file.display(),
                analysis.render_text(unio::Lang::En)
            )),
            None => failures.push(format!(
                "{}: expected {code} on line {line}, got nothing",
                file.display()
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_repository_source_passes_name_and_type_checks() {
    let root = repository();
    let mut files = Vec::new();
    for dir in ["docs/examples", "docs/ru/examples", "examples", "tools"] {
        files.extend(unio_files(&root.join(dir)));
    }
    files.extend(unio_files(&root.join("stdlib")));
    let results = unio::check_files(&files);
    let report: Vec<String> = results
        .iter()
        .filter(|a| !a.diagnostics.is_empty())
        .map(|a| a.render_text(unio::Lang::En))
        .collect();
    assert!(report.is_empty(), "{}", report.join("\n"));
}

/// R-1 for the name and type checks: renaming names at random keeps the syntax valid and must
/// never make the checker panic. Deterministic, so a failure can be reproduced.
#[test]
fn renamed_names_never_make_the_checker_panic() {
    let root = repository();
    let mut sources = unio_files(&root.join("examples"));
    sources.extend(unio_files(&root.join("docs/examples")));
    let iterations: usize = std::env::var("UNIO_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let dir = std::env::temp_dir().join(format!("unio-semantic-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut state: u64 = 0x5eed;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    for i in 0..iterations {
        let source = std::fs::read_to_string(&sources[i % sources.len()]).unwrap();
        let words: Vec<(usize, usize)> = word_spans(&source);
        if words.len() < 2 {
            continue;
        }
        let mut mutated = source.clone();
        for _ in 0..3 {
            let (start, end) = words[next() % words.len()];
            let (s2, e2) = words[next() % words.len()];
            if end > mutated.len() || e2 > source.len() {
                continue;
            }
            let replacement = source[s2..e2].to_string();
            mutated.replace_range(start..end.min(mutated.len()), &replacement);
            break;
        }
        let file = dir.join("mutated.unio");
        std::fs::write(&file, &mutated).unwrap();
        let _ = unio::check_files(std::slice::from_ref(&file));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Byte ranges of lower-case identifiers outside comments and texts.
fn word_spans(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    let mut in_text = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_text = !in_text;
        } else if c == b'#' && !in_text {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if !in_text && c.is_ascii_lowercase() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            spans.push((start, i));
            continue;
        }
        i += 1;
    }
    spans
}

/// An import resolves inside the package or the standard library; a link that leads out of the
/// package is not followed (docs/semantics.md section 1, E0053).
#[cfg(unix)]
#[test]
fn an_import_through_a_link_out_of_the_package_is_rejected() {
    let dir = std::env::temp_dir().join(format!("unio-import-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("package/lib")).unwrap();
    std::fs::create_dir_all(dir.join("outside")).unwrap();
    std::fs::write(dir.join("package/unio.toml"), "[package]\nname = \"p\"\n").unwrap();
    std::fs::write(
        dir.join("outside/secret.unio"),
        "public function value() returns Int:\n    return 1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("package/lib/inner.unio"),
        "public function value() returns Int:\n    return 2\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        dir.join("outside/secret.unio"),
        dir.join("package/secret.unio"),
    )
    .unwrap();
    std::os::unix::fs::symlink(dir.join("outside"), dir.join("package/linked")).unwrap();
    std::os::unix::fs::symlink(
        dir.join("package/lib/inner.unio"),
        dir.join("package/alias.unio"),
    )
    .unwrap();
    let main = dir.join("package/main.unio");
    std::fs::write(
        &main,
        "import secret\nimport linked.secret as other\nimport alias\n\npublic function main() returns Int:\n    return alias.value()\n",
    )
    .unwrap();
    let results = unio::check_files(std::slice::from_ref(&main));
    let analysis = results
        .iter()
        .find(|a| std::path::Path::new(&a.path) == main)
        .expect("the file is checked");
    let rendered = analysis.render_text(unio::Lang::En);
    let lines: Vec<usize> = analysis
        .diagnostics
        .iter()
        .filter(|d| d.code == "E0053")
        .map(|d| d.line())
        .collect();
    assert_eq!(lines, [1, 2], "{rendered}");
    assert!(rendered.contains("outside the package"), "{rendered}");
    assert_eq!(analysis.diagnostics.len(), 2, "{rendered}");
    let _ = std::fs::remove_dir_all(&dir);
}
