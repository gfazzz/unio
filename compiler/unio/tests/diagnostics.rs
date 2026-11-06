//! C-2, C-7, C-8, C-10: several errors per file, text and JSON formats, two languages.

#[test]
fn several_errors_are_reported_in_one_pass() {
    let text =
        "function f(x) returns Int:\n    return 1\n\nfunction g() returns Int:\n    return 1 +\n";
    let analysis = unio::check_source("two.unio", text);
    assert!(
        analysis.diagnostics.len() >= 2,
        "{}",
        analysis.render_text(unio::Lang::En)
    );
    assert_eq!(analysis.diagnostics[0].line(), 1);
    assert!(analysis.diagnostics.iter().any(|d| d.line() == 5));
}

#[test]
fn text_format_has_code_location_snippet_and_underline() {
    let analysis = unio::check_source("demo.unio", "let x = (1, 2\n");
    let text = analysis.render_text(unio::Lang::En);
    assert!(text.contains("error[E0009]"), "{text}");
    assert!(text.contains("demo.unio:1:9"), "{text}");
    assert!(text.contains("let x = (1, 2"), "{text}");
    assert!(text.contains('^'), "{text}");
}

#[test]
fn json_format_has_the_required_fields() {
    let analysis = unio::check_source("demo.unio", "\tlet x = 1\n");
    let json = analysis.render_json(unio::Lang::En);
    for field in [
        "\"code\":\"E0001\"",
        "\"severity\":\"error\"",
        "\"message\":",
        "\"explanation\":",
        "\"spans\":",
        "\"fixes\":",
        "\"related\":",
        "\"file\":\"demo.unio\"",
    ] {
        assert!(json.contains(field), "missing {field} in {json}");
    }
    assert!(json.trim_start().starts_with('['));
}

#[test]
fn messages_exist_in_english_and_russian_with_the_same_codes() {
    let analysis = unio::check_source("demo.unio", "let t = \"open\n");
    let en = analysis.render_json(unio::Lang::En);
    let ru = analysis.render_json(unio::Lang::Ru);
    assert!(en.contains("E0005") && ru.contains("E0005"));
    assert_ne!(en, ru);
    assert!(ru.contains("текст"), "{ru}");
}

#[test]
fn reserved_word_and_chained_comparison_have_codes() {
    let a = unio::check_source("a.unio", "function f() uses gpu:\n    return\n");
    assert_eq!(a.diagnostics[0].code, "E0032");
    let b = unio::check_source(
        "b.unio",
        "function f(a as Int) returns Bool:\n    return a < 1 < 2\n",
    );
    assert_eq!(b.diagnostics[0].code, "E0020");
    let c = unio::check_source("c.unio", "function f():\n    g(a = 1, 2)\n");
    assert_eq!(c.diagnostics[0].code, "E0023");
    let d = unio::check_source("d.unio", "function f():\n    1 + 2 = 3\n");
    assert_eq!(d.diagnostics[0].code, "E0030");
    let e = unio::check_source("e.unio", "function f(x as List[Int]):\n    match x:\n        case [rest..., last]:\n            pass\n");
    assert_eq!(e.diagnostics[0].code, "E0034");
}
