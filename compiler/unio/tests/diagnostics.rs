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

/// A validator of the subset of JSON Schema that `compiler/diagnostics.schema.json` uses: `type`,
/// `enum`, `pattern` (only `^E[0-9]{4}$`), `minimum`, `required`, `properties`,
/// `additionalProperties: false`, `items`, and local `$ref`.
fn validate(
    schema: &unio::lsp::json::Json,
    root: &unio::lsp::json::Json,
    value: &unio::lsp::json::Json,
    at: &str,
) -> Vec<String> {
    use unio::lsp::json::Json;
    let get = |j: &Json, key: &str| -> Option<Json> {
        match j {
            Json::Object(members) => members
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone()),
            _ => None,
        }
    };
    if let Some(Json::String(reference)) = get(schema, "$ref") {
        let mut target = root.clone();
        for part in reference.trim_start_matches("#/").split('/') {
            target = get(&target, part).unwrap_or_else(|| panic!("no {reference}"));
        }
        return validate(&target, root, value, at);
    }
    let mut errors = Vec::new();
    let kind = match value {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(n) if n.fract() == 0.0 => "integer",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    };
    match get(schema, "type") {
        Some(Json::String(t)) if t != kind => errors.push(format!("{at}: {kind}, not {t}")),
        Some(Json::Array(ts)) if !ts.contains(&Json::String(kind.to_string())) => {
            errors.push(format!("{at}: {kind}, not one of {ts:?}"))
        }
        _ => {}
    }
    if let Some(Json::Array(allowed)) = get(schema, "enum") {
        if !allowed.contains(value) {
            errors.push(format!("{at}: {value:?} not in {allowed:?}"));
        }
    }
    if let (Some(Json::String(_)), Json::String(text)) = (get(schema, "pattern"), value) {
        let ok = text.len() == 5
            && text.starts_with('E')
            && text[1..].chars().all(|c| c.is_ascii_digit());
        if !ok {
            errors.push(format!("{at}: {text} does not match the pattern"));
        }
    }
    if let (Some(Json::Number(min)), Json::Number(n)) = (get(schema, "minimum"), value) {
        if n < &min {
            errors.push(format!("{at}: {n} below {min}"));
        }
    }
    if let Json::Object(members) = value {
        if let Some(Json::Array(required)) = get(schema, "required") {
            for r in required {
                if let Json::String(r) = r {
                    if !members.iter().any(|(k, _)| k == &r) {
                        errors.push(format!("{at}: missing {r}"));
                    }
                }
            }
        }
        let properties = get(schema, "properties");
        for (k, v) in members {
            match properties.as_ref().and_then(|p| get(p, k)) {
                Some(s) => errors.extend(validate(&s, root, v, &format!("{at}.{k}"))),
                None if get(schema, "additionalProperties") == Some(Json::Bool(false)) => {
                    errors.push(format!("{at}: unexpected {k}"))
                }
                None => {}
            }
        }
    }
    if let (Some(items), Json::Array(values)) = (get(schema, "items"), value) {
        for (i, v) in values.iter().enumerate() {
            errors.extend(validate(&items, root, v, &format!("{at}[{i}]")));
        }
    }
    errors
}

/// C-8: the JSON of the diagnostics is described by the published, versioned schema
/// `compiler/diagnostics.schema.json`, and the output of syntax and semantic errors, with and
/// without fixes, in both languages, conforms to it.
#[test]
fn json_diagnostics_conform_to_the_published_schema() {
    use unio::lsp::json::Json;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../diagnostics.schema.json");
    let schema = Json::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let id = match &schema {
        Json::Object(m) => m.iter().find(|(k, _)| k == "$id").map(|(_, v)| v.clone()),
        _ => None,
    };
    assert_eq!(id, Some(Json::String("urn:unio:diagnostics:1".to_string())));
    let mut outputs = Vec::new();
    for source in [
        "\tlet x = 1\n",
        "function f()\n    return 1\n",
        "let t = \"open\n",
        "function f() uses gpu:\n    return\n",
        "",
    ] {
        let analysis = unio::check_source("demo.unio", source);
        outputs.push(analysis.render_json(unio::Lang::En));
        outputs.push(analysis.render_json(unio::Lang::Ru));
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("schema");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("types.unio");
    std::fs::write(&file, "function f() returns Int:\n    return \"x\"\n").unwrap();
    for analysis in unio::check_files(&[file]) {
        outputs.push(analysis.render_json(unio::Lang::En));
    }
    assert!(
        outputs.iter().any(|o| o.contains("\"new_text\"")),
        "no output with a fix"
    );
    for output in outputs {
        let value = Json::parse(&output).unwrap();
        let errors = validate(&schema, &schema, &value, "$");
        assert!(errors.is_empty(), "{errors:#?}\n{output}");
    }
}
