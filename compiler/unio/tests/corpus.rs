//! spec/tests: valid files parse without diagnostics; invalid files report their first error
//! on the line, and with the code, given by `# expect-error: LINE [CODE]`.

mod common;

use common::{read, repo_root, unio_files};

fn expectation(text: &str) -> Option<(usize, Option<String>)> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# expect-error:") {
            let mut words = rest.split_whitespace();
            let number = words.next()?.parse().ok()?;
            return Some((number, words.next().map(str::to_string)));
        }
    }
    None
}

#[test]
fn valid_corpus_parses_without_diagnostics() {
    let files = unio_files(&repo_root().join("spec/tests/valid"));
    assert!(!files.is_empty(), "no valid corpus files found");
    let mut failures = Vec::new();
    for path in &files {
        let analysis = unio::check_source(&path.display().to_string(), &read(path));
        if !analysis.diagnostics.is_empty() {
            failures.push(format!(
                "{}: {}",
                path.display(),
                analysis.render_text(unio::Lang::En)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn invalid_corpus_reports_the_expected_first_error() {
    let files = unio_files(&repo_root().join("spec/tests/invalid"));
    assert!(!files.is_empty(), "no invalid corpus files found");
    let mut failures = Vec::new();
    for path in &files {
        let text = read(path);
        let Some((line, code)) = expectation(&text) else {
            failures.push(format!("{}: missing '# expect-error:'", path.display()));
            continue;
        };
        let analysis = unio::check_source(&path.display().to_string(), &text);
        match analysis.diagnostics.first() {
            None => failures.push(format!(
                "{}: parsed, expected an error on line {line}",
                path.display()
            )),
            Some(first) => {
                let code_ok = code.as_deref().is_none_or(|c| c == first.code);
                if first.line() != line || !code_ok {
                    failures.push(format!(
                        "{}: expected line {line} {}, got line {} {}: {}",
                        path.display(),
                        code.as_deref().unwrap_or(""),
                        first.line(),
                        first.code,
                        first.message.en
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
