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

/// Checks the source as the file `name` of a fresh directory, with the manifest when given, and
/// gives the analyses.
fn checked_package(
    name: &str,
    files: &[(&str, &str)],
) -> (std::path::PathBuf, Vec<unio::Analysis>) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("related")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let analyses = unio::check_files(&[dir.join("main.unio")]);
    (dir, analyses)
}

/// The related locations of the diagnostics with the code: line and message of each.
fn related_of(analyses: &[unio::Analysis], code: &str) -> Vec<(Option<String>, usize, String)> {
    analyses
        .iter()
        .flat_map(|a| &a.diagnostics)
        .filter(|d| d.code == code)
        .flat_map(|d| &d.related)
        .map(|r| (r.file.clone(), r.span.start.line, r.message.en.clone()))
        .collect()
}

/// C-8: `related` holds the second location that matters: the earlier declaration of a
/// duplicate, the declaration of a type in a mismatch, the `fails with` of a function called
/// without `try`, the signature and the handlers for an unhandled error, the binding of a name
/// that cannot change, the function whose parameters a call misses.
#[test]
fn related_locations_name_the_second_place_of_a_diagnostic() {
    let source = "\
struct Point:
    x as Int

error Broken:
    reason as Text

error Other:
    reason as Text

function total() returns Int:
    return 1

function total() returns Int:
    return 2

function risky() returns Int fails with Broken:
    fail Broken(reason = \"no\")

function point() returns Point:
    return 3

function call() returns Int:
    return risky()

function handled() returns Int:
    try:
        return try risky()
    on Other as e:
        return 0

function fixed() returns Int:
    let n = 1
    n = 2
    return n

function short() returns Int:
    return add(1)

function add(a as Int, b as Int) returns Int:
    return a + b
";
    let (_, analyses) = checked_package("kinds", &[("main.unio", source)]);
    assert_eq!(
        related_of(&analyses, "E0051"),
        vec![(None, 10, "'total' is first declared here".to_string())]
    );
    assert_eq!(
        related_of(&analyses, "E0061"),
        vec![(None, 1, "the type 'Point' is declared here".to_string())]
    );
    assert_eq!(
        related_of(&analyses, "E0073"),
        vec![(
            None,
            16,
            "'risky' lists the errors it may fail with here".to_string()
        )]
    );
    assert_eq!(
        related_of(&analyses, "E0074"),
        vec![
            (
                None,
                28,
                "this handler receives 'Other', not the error of the call".to_string()
            ),
            (
                None,
                25,
                "the error may be added to the 'fails with' list of 'handled' here".to_string()
            ),
        ]
    );
    assert_eq!(
        related_of(&analyses, "E0077"),
        vec![(None, 32, "'n' is bound here".to_string())]
    );
    assert_eq!(
        related_of(&analyses, "E0064"),
        vec![(
            None,
            39,
            "the parameters of 'add' are declared here".to_string()
        )]
    );
    // The full name of a declaration travels as `symbol`.
    let duplicate = analyses
        .iter()
        .flat_map(|a| &a.diagnostics)
        .find(|d| d.code == "E0051")
        .unwrap();
    assert_eq!(duplicate.related[0].symbol.as_deref(), Some("main.total"));
}

/// C-8: a location in another file names it: a declaration in another module, the
/// `[permissions]` table of the manifest for E0085, a marked module for E0089, and the table of
/// a key for E0088.
#[test]
fn related_locations_in_other_files_name_the_file() {
    let main = "\
import shapes

function area() returns Int:
    return shapes.hidden()
";
    let shapes = "function hidden() returns Int:\n    return 1\n";
    let (_, analyses) = checked_package("modules", &[("main.unio", main), ("shapes.unio", shapes)]);
    let related = related_of(&analyses, "E0057");
    assert_eq!(related.len(), 1, "{related:?}");
    assert!(
        related[0]
            .0
            .as_deref()
            .is_some_and(|f| f.ends_with("shapes.unio")),
        "{related:?}"
    );
    assert_eq!(related[0].1, 1);

    let manifest =
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\nenv = [\"HOME\"]\n";
    let main = "\
import std.files

function main() returns Int:
    let text = try files.read_text(Path(\"a\")) on IoError as e:
        return 1
    return text.length()
";
    let (_, analyses) = checked_package(
        "permissions",
        &[("main.unio", main), ("unio.toml", manifest)],
    );
    let related = related_of(&analyses, "E0085");
    assert_eq!(related.len(), 1, "{related:?}");
    assert!(related[0]
        .0
        .as_deref()
        .is_some_and(|f| f.ends_with("unio.toml")));
    assert_eq!(related[0].1, 5);
    assert_eq!(
        related[0].2,
        "the permissions of the package are listed here"
    );

    let manifest =
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[permissions]\nfiles.read = true\n";
    let main = "unsafe module\n\nfunction main() returns Int:\n    return 0\n";
    let (_, analyses) =
        checked_package("manifest", &[("main.unio", main), ("unio.toml", manifest)]);
    let related = related_of(&analyses, "E0088");
    assert_eq!(
        related,
        vec![(
            None,
            5,
            "the key belongs to the table [permissions] here".to_string()
        )]
    );
    let related = related_of(&analyses, "E0089");
    assert_eq!(related.len(), 1, "{related:?}");
    assert!(
        related[0]
            .0
            .as_deref()
            .is_some_and(|f| f.ends_with("main.unio")),
        "{related:?}"
    );
    assert_eq!(related[0].1, 1);
    assert_eq!(related[0].2, "'main' is marked 'unsafe module' here");
}

/// C-8: the text form shows each related location as a note with its place, and the JSON form
/// gives it with its symbol and span, which conform to the published schema.
#[test]
fn related_locations_are_in_the_text_and_the_json() {
    let source =
        "function f() returns Int:\n    return 1\n\nfunction f() returns Int:\n    return 2\n";
    let (dir, analyses) = checked_package("forms", &[("main.unio", source)]);
    let analysis = analyses
        .iter()
        .find(|a| a.path.ends_with("main.unio"))
        .unwrap();
    let text = analysis.render_text(unio::Lang::En);
    let path = dir.join("main.unio");
    let note = format!(
        "  = note: {}:1:10: 'f' is first declared here",
        path.display()
    );
    assert!(text.contains(&note), "{text}");
    let russian = analysis.render_text(unio::Lang::Ru);
    assert!(russian.contains("примечание: "), "{russian}");
    let json = analysis.render_json(unio::Lang::En);
    assert!(
        json.contains(
            "\"related\":[{\"symbol\":\"main.f\",\"message\":\"'f' is first declared here\""
        ),
        "{json}"
    );
    use unio::lsp::json::Json;
    let schema_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../diagnostics.schema.json");
    let schema = Json::parse(&std::fs::read_to_string(&schema_path).unwrap()).unwrap();
    let value = Json::parse(&json).unwrap();
    let errors = validate(&schema, &schema, &value, "$");
    assert!(errors.is_empty(), "{errors:#?}\n{json}");
    // A mismatched bracket names the bracket that is still open.
    let analysis = unio::check_source("demo.unio", "let x = (1, 2]\n");
    let json = analysis.render_json(unio::Lang::En);
    assert!(
        json.contains("\"message\":\"'(' is opened here\""),
        "{json}"
    );
}
