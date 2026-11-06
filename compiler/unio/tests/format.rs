//! `unio fmt`: the rules of docs/style.md and the guarantees T-1, T-2 and T-3.

mod common;

use common::{read, repo_root, unio_files};
use unio::format::{format_source, normalize_imports};

/// Formats and checks the result against the expected text.
fn assert_formats(input: &str, expected: &str) {
    let formatted = match format_source(input) {
        Ok(text) => text,
        Err(diagnostics) => panic!("the input has errors: {diagnostics:?}\n{input}"),
    };
    assert_eq!(
        formatted, expected,
        "\n--- input\n{input}\n--- got\n{formatted}"
    );
    assert_eq!(
        format_source(&formatted).unwrap(),
        formatted,
        "formatting is not idempotent"
    );
}

#[test]
fn spaces_around_operators_commas_and_named_arguments() {
    assert_formats(
        "let X=1+2*3\nlet Y=f( a ,b,named=1 )\nlet Z = {\"a\" :1,\"b\":-x}\nlet W = a?.b . c[ 1 ]\n",
        "let X = 1 + 2 * 3\nlet Y = f(a, b, named = 1)\nlet Z = {\"a\": 1, \"b\": -x}\nlet W = a?.b.c[1]\n",
    );
}

#[test]
fn keywords_have_single_spaces() {
    assert_formats(
        "function f(x  as  Int)  returns  Int  fails  with  E:\n    return x\n",
        "function f(x as Int) returns Int fails with E:\n    return x\n",
    );
}

#[test]
fn one_blank_line_between_declarations_and_at_most_one_in_blocks() {
    assert_formats(
        "function a():\n\n    pass\n\n\n\nfunction b():\n    let x = 1\n\n\n    let y = 2\n\n",
        "function a():\n    pass\n\nfunction b():\n    let x = 1\n\n    let y = 2\n",
    );
}

#[test]
fn consecutive_constants_keep_at_most_one_blank_line() {
    assert_formats(
        "let A = 1\nlet B = 2\n\n\nlet C = 3\n",
        "let A = 1\nlet B = 2\n\nlet C = 3\n",
    );
}

#[test]
fn imports_are_one_sorted_group() {
    assert_formats(
        "from std.text import split, join\n\nimport std.json as json\nimport std.args\nlet A = 1\n",
        "import std.args\nimport std.json as json\nfrom std.text import join, split\n\nlet A = 1\n",
    );
}

#[test]
fn module_header_is_followed_by_one_blank_line() {
    assert_formats(
        "## The module.\nunsafe module\nexternal function f()\n",
        "## The module.\nunsafe module\n\nexternal function f()\n",
    );
}

#[test]
fn a_branch_of_one_simple_statement_goes_on_its_header_line() {
    assert_formats(
        "function f(x as Int) returns Int:\n    if x > 0:\n        return 1\n    else:\n        return 0\n",
        "function f(x as Int) returns Int:\n    if x > 0: return 1\n    else: return 0\n",
    );
    assert_formats(
        "function f(v as Option[Int]) returns Int:\n    match v:\n        case some(x):\n            return x\n        case none: return 0\n",
        "function f(v as Option[Int]) returns Int:\n    match v:\n        case some(x): return x\n        case none: return 0\n",
    );
}

#[test]
fn a_branch_that_does_not_fit_or_has_two_statements_keeps_its_block() {
    let long = "    if x > 0: return some_function_with_a_long_name(first_argument, second_argument_value, the_third)\n";
    let input = format!("function f(x as Int) returns Int:\n{long}    return 0\n");
    let expected = "function f(x as Int) returns Int:\n    if x > 0:\n        return some_function_with_a_long_name(first_argument, second_argument_value, the_third)\n    return 0\n";
    assert_formats(&input, expected);
    assert_formats(
        "function f(x as Int):\n    if x > 0:\n        print(\"a\")\n        print(\"b\")\n",
        "function f(x as Int):\n    if x > 0:\n        print(\"a\")\n        print(\"b\")\n",
    );
}

#[test]
fn loops_and_functions_always_keep_a_block() {
    assert_formats(
        "function f(items as List[Int]):\n    for item in items:\n        print(item)\n",
        "function f(items as List[Int]):\n    for item in items:\n        print(item)\n",
    );
}

#[test]
fn a_long_line_breaks_the_outermost_group_that_ends_last() {
    assert_formats(
        "let RESULT = compute(first_argument_value, second_argument_value, third_argument_value, fourth_value)\n",
        "let RESULT = compute(\n    first_argument_value,\n    second_argument_value,\n    third_argument_value,\n    fourth_value,\n)\n",
    );
}

#[test]
fn breaking_is_applied_again_to_each_new_line() {
    let input = "let R = outer(inner_function_name(argument_number_one, argument_number_two, argument_number_three), x)\n";
    let expected = "let R = outer(\n    inner_function_name(argument_number_one, argument_number_two, argument_number_three),\n    x,\n)\n";
    assert_formats(input, expected);
}

#[test]
fn the_form_of_a_group_does_not_depend_on_the_source() {
    assert_formats("let R = f(\n    a,\n    b,\n)\n", "let R = f(a, b)\n");
}

#[test]
fn comments_keep_their_place() {
    assert_formats(
        "# before f\nfunction f():\n    # inside\n    let x = 1   # trailing\n    # at the end of the block\n\n# before g\nfunction g():\n    pass\n",
        "# before f\nfunction f():\n    # inside\n    let x = 1  # trailing\n    # at the end of the block\n\n# before g\nfunction g():\n    pass\n",
    );
}

#[test]
fn a_documentation_comment_stays_directly_before_its_declaration() {
    assert_formats(
        "let A = 1\n## Doubles.\nfunction double(x as Int) returns Int:\n    return x * 2\n",
        "let A = 1\n\n## Doubles.\nfunction double(x as Int) returns Int:\n    return x * 2\n",
    );
}

#[test]
fn literals_and_parentheses_are_written_as_in_the_source() {
    assert_formats(
        "let A = (1_000 + 0xFF) * 2.5e-3\nlet T = \"a {x+1} \\n \\u{1F600}\"\n",
        "let A = (1_000 + 0xFF) * 2.5e-3\nlet T = \"a {x+1} \\n \\u{1F600}\"\n",
    );
}

#[test]
fn extend_members_are_separated_by_one_blank_line() {
    assert_formats(
        "extend Point:\n    function x(self) returns Int:\n        return self.x\n    function y(self) returns Int:\n        return self.y\n",
        "extend Point:\n    function x(self) returns Int:\n        return self.x\n\n    function y(self) returns Int:\n        return self.y\n",
    );
}

#[test]
fn trailing_white_space_is_removed_and_one_line_end_ends_the_file() {
    assert_formats("let A = 1   \n\n\n", "let A = 1\n");
}

#[test]
fn a_source_with_errors_is_not_formatted() {
    assert!(format_source("function f(:\n").is_err());
}

/// Every comment text of the source, in order.
fn comment_texts(source: &str) -> Vec<String> {
    unio::lexer::lex(source)
        .comments
        .iter()
        .map(|c| c.text.trim_end().to_string())
        .collect()
}

#[test]
fn the_corpus_and_the_repository_format_with_the_same_tree() {
    let root = repo_root();
    let mut files = unio_files(&root.join("spec/tests/valid"));
    for dir in [
        "docs/examples",
        "docs/ru/examples",
        "examples",
        "stdlib",
        "tools",
    ] {
        files.extend(unio_files(&root.join(dir)));
    }
    assert!(files.len() > 40, "found {} files", files.len());
    let mut failures = Vec::new();
    for path in &files {
        let name = path.display().to_string();
        let source = read(path);
        let before = unio::check_source(&name, &source);
        assert!(before.diagnostics.is_empty(), "{name} has errors");
        let formatted = match format_source(&source) {
            Ok(text) => text,
            Err(d) => {
                failures.push(format!("{name}: not formatted: {d:?}"));
                continue;
            }
        };
        let after = unio::check_source(&name, &formatted);
        if !after.diagnostics.is_empty() {
            failures.push(format!(
                "{name}: the result has errors:\n{}",
                after.render_text(unio::Lang::En)
            ));
            continue;
        }
        let expected = unio::sexpr::file(&normalize_imports(&before.file));
        if unio::sexpr::file(&after.file) != expected {
            failures.push(format!("{name}: the tree changed"));
        }
        if format_source(&formatted).ok().as_deref() != Some(formatted.as_str()) {
            failures.push(format!("{name}: formatting is not idempotent"));
        }
        if comment_texts(&source) != comment_texts(&formatted) {
            failures.push(format!("{name}: comments were lost or reordered"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A generated source of about 1,000 lines with every kind of statement.
fn thousand_lines() -> String {
    let mut text = String::from("import std.text\n\n");
    let mut n = 0;
    while text.lines().count() < 1000 {
        n += 1;
        text.push_str(&format!(
            "## Function {n}.\nfunction f{n}(items as List[Int], limit as Int) returns Int fails with E:\n    var total = 0\n    for item in items:\n        if item > limit:\n            total = total + item * 2\n        else:\n            total = total - 1\n    match lookup(items, limit):\n        case some(value):\n            return value\n        case none:\n            pass\n    let text = \"total {{total}}\"  # a comment\n    return compute(total, limit, [1, 2, 3], {{\"a\": 1}}, function (x): x + 1)\n\n"
        ));
    }
    text
}

#[test]
fn a_thousand_line_source_formats_within_the_time_limit() {
    let source = thousand_lines();
    assert!(source.lines().count() >= 1000);
    format_source(&source).expect("the generated source is valid");
    let start = std::time::Instant::now();
    format_source(&source).unwrap();
    let elapsed = start.elapsed();
    // T-3 applies to release builds; a debug build is about ten times slower.
    let limit = if cfg!(debug_assertions) { 1000 } else { 50 };
    assert!(
        elapsed.as_millis() <= limit,
        "formatting 1,000 lines took {elapsed:?}, the limit is {limit} ms"
    );
}
