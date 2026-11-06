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

#[test]
fn every_example_shows_its_error_and_every_fix_is_clean() {
    for code in catalog::CODES {
        let entry = catalog::entry(code).unwrap();
        let analysis = unio::check_source("example.unio", entry.example);
        assert!(
            analysis.diagnostics.iter().any(|d| d.code == *code),
            "{code}: the example does not produce it: {:?}",
            analysis
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        );
        let fixed = unio::check_source("fixed.unio", entry.fixed);
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
