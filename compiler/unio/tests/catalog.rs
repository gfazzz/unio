//! The diagnostic catalog (R-12, C-9): one place for every message, a template, no internals.

use unio::catalog::{self, INTERNAL_NAMES};
use unio::Lang;

#[test]
fn every_code_has_an_explanation_with_an_example_and_its_fix() {
    for code in catalog::CODES {
        let entry = catalog::entry(code).unwrap_or_else(|| panic!("{code} is not in the catalog"));
        for lang in [Lang::En, Lang::Ru] {
            let text = entry.explanation.get(lang);
            assert!(text.len() > 20, "{code}: explanation too short");
        }
        assert!(
            !entry.example.trim().is_empty(),
            "{code}: no example of the error"
        );
        assert!(
            !entry.fixed.trim().is_empty(),
            "{code}: no example of the fix"
        );
        assert_ne!(
            entry.example, entry.fixed,
            "{code}: the fix is the same as the error"
        );
    }
}

/// Checks an example as a whole file: syntax only for syntax codes, names and types too for
/// the codes of docs/semantics.md. Each semantic example gets its own directory, so that the
/// example of E0058 can import itself. E0084 is checked with the core profile, and E0085 in a
/// package whose manifest permits nothing.
fn check_example(code: &str, source: &str, which: &str) -> unio::Analysis {
    if !catalog::SEMANTIC_CODES.contains(&code) {
        return unio::check_source("example.unio", source);
    }
    let dir = std::env::temp_dir().join(format!(
        "unio-catalog-{}-{code}-{which}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("example.unio");
    std::fs::write(&file, source).unwrap();
    if code == "E0085" {
        std::fs::write(
            dir.join("unio.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\n\n[permissions]\n",
        )
        .unwrap();
    }
    let profile = if code == "E0084" {
        unio::effects::Profile::Core
    } else {
        unio::effects::Profile::Application
    };
    let mut results = unio::check_files_with(std::slice::from_ref(&file), profile);
    let _ = std::fs::remove_dir_all(&dir);
    results.remove(0)
}

#[test]
fn every_example_shows_its_error_and_every_fix_is_clean() {
    for code in catalog::CODES {
        let entry = catalog::entry(code).unwrap();
        let analysis = check_example(code, entry.example, "error");
        assert!(
            analysis.diagnostics.iter().any(|d| d.code == *code),
            "{code}: the example does not produce it: {:?}",
            analysis
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        );
        let fixed = check_example(code, entry.fixed, "fixed");
        assert!(
            fixed.diagnostics.is_empty(),
            "{code}: the fixed example has errors: {}",
            fixed.render_text(Lang::En)
        );
    }
}

#[test]
fn messages_follow_the_template() {
    for (code, message) in catalog::sample_messages() {
        for lang in [Lang::En, Lang::Ru] {
            let text = message.get(lang);
            assert!(!text.is_empty(), "{code}: empty message");
            let first = text.chars().next().unwrap();
            assert!(
                !first.is_uppercase() || first == 'I',
                "{code}: a message starts in lower case: {text}"
            );
            assert!(
                !text.ends_with('.'),
                "{code}: a message has no final period: {text}"
            );
            assert!(
                text.chars().count() <= 200,
                "{code}: longer than 200 characters (R-11)"
            );
        }
    }
}

#[test]
fn no_message_names_compiler_internals() {
    let mut texts = Vec::new();
    for (code, message) in catalog::sample_messages() {
        texts.push((code.to_string(), message.en.clone()));
        texts.push((code.to_string(), message.ru.clone()));
    }
    for code in catalog::CODES {
        let entry = catalog::entry(code).unwrap();
        texts.push((code.to_string(), entry.explanation.en.clone()));
        texts.push((code.to_string(), entry.explanation.ru.clone()));
    }
    for (code, text) in texts {
        for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')) {
            assert!(
                !INTERNAL_NAMES.contains(&word),
                "{code}: the message names the internal '{word}': {text}"
            );
        }
    }
}

#[test]
fn explain_prints_the_catalog_entry() {
    let text = catalog::explain("E0001", Lang::En).expect("E0001 is known");
    assert!(text.contains("E0001"));
    assert!(text.contains("\t"), "the example shows the tab");
    assert!(catalog::explain("E9999", Lang::En).is_none());
}
