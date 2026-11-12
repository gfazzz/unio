//! One mistake gives one message (docs/semantics.md 2.8, C-2), and the checks that the R-10
//! control set found missing: each test gives a program with one mistake and the exact list of
//! errors `unio check` must report for it.

#[allow(dead_code)]
mod common;

use std::path::PathBuf;

/// The errors of `unio check` on the source, as (line, code), in order of position.
fn errors(name: &str, source: &str) -> Vec<(usize, String)> {
    let dir = std::env::temp_dir().join(format!("unio-one-message-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file: PathBuf = dir.join(format!("{name}.unio"));
    std::fs::write(&file, source).unwrap();
    let results = unio::check_files(std::slice::from_ref(&file));
    let found = results
        .iter()
        .find(|a| std::path::Path::new(&a.path) == file)
        .map(|a| {
            a.diagnostics
                .iter()
                .filter(|d| d.severity == unio::Severity::Error)
                .map(|d| (d.line(), d.code.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    found
}

/// The errors of the lexer and the parser alone, as (line, code).
fn syntax_errors(source: &str) -> Vec<(usize, String)> {
    unio::check_source("syntax.unio", source)
        .diagnostics
        .iter()
        .map(|d| (d.line(), d.code.to_string()))
        .collect()
}

fn one(line: usize, code: &str) -> Vec<(usize, String)> {
    vec![(line, code.to_string())]
}

// ------------------------------------------------------------ 1. failed imports

#[test]
fn a_module_that_is_not_found_poisons_its_name() {
    let source = "import std.filez\n\nfunction f() returns Bool:\n    return filez.exists(Path(\"x\"))\n\nfunction g() returns Bool:\n    let found = filez.exists(Path(\"y\"))\n    return found\n";
    assert_eq!(errors("module_not_found", source), one(1, "E0053"));
}

#[test]
fn a_name_the_module_does_not_have_is_poisoned() {
    let source = "from std.text import join_all\n\nfunction f(cells as List[Text]) returns Text:\n    return join_all(cells, \",\")\n";
    assert_eq!(errors("name_missing", source), one(1, "E0054"));
}

#[test]
fn a_poisoned_name_used_as_a_type_gives_no_message() {
    let source = "from std.text import Missing\n\nfunction f(m as Missing) returns List[Missing]:\n    return [m]\n";
    assert_eq!(errors("type_missing", source), one(1, "E0054"));
}

#[test]
fn a_dependency_that_is_not_installed_poisons_its_names() {
    let source = "from deps.strings.casing import join\nimport deps.strings.casing as casing\n\nfunction f(parts as List[Text]) returns Text:\n    return join(parts, \" \") + casing.upper(\"x\")\n";
    assert_eq!(
        errors("dependency", source),
        vec![(1, "E0048".to_string()), (2, "E0048".to_string())]
    );
}

#[test]
fn other_unknown_names_are_still_reported_after_a_failed_import() {
    let source =
        "import std.filez\n\nfunction f() returns Bool:\n    return filez.exists(Path(\"x\")) and nope\n";
    assert_eq!(
        errors("other_unknown", source),
        vec![(1, "E0053".to_string()), (4, "E0056".to_string())]
    );
}

// ------------------------------------------------------------ 2. types that failed to resolve

#[test]
fn a_type_with_the_wrong_number_of_arguments_gives_no_mismatch() {
    let source = "function index(names as List[Text]) returns Map[List[Text]]:\n    var index as Map[Text, List[Text]] = {}\n    for name in names:\n        index[name] = [name]\n    return index\n";
    assert_eq!(errors("map_arity", source), one(1, "E0059"));
}

#[test]
fn an_unknown_type_gives_no_mismatch() {
    let source = "function f(names as List[Text]) returns Lsit[Text]:\n    return names\n";
    assert_eq!(errors("unknown_type", source), one(1, "E0056"));
}

#[test]
fn a_local_with_a_wrong_type_gives_no_mismatch() {
    let source = "function f() returns Int:\n    let pairs as Map[Int] = {1: 2}\n    return pairs.length()\n";
    assert_eq!(errors("local_arity", source), one(2, "E0059"));
}

// ------------------------------------------------------------ 3. resources (L-20, E0081)

const HANDLE: &str = "struct Handle:\n    name as Text\n\nextend Handle with Resource:\n    function release(var self):\n        print(\"closed {self.name}\")\n\nfunction open(name as Text) returns Handle:\n    return Handle(name = name)\n\n";
/// The number of lines of HANDLE.
const H: usize = 10;

fn resource_errors(name: &str, body: &str) -> Vec<(usize, String)> {
    errors(name, &format!("{HANDLE}{body}"))
}

#[test]
fn appending_a_resource_to_an_outer_list_is_an_escape() {
    let body = "function f() returns Int:\n    var kept as List[Handle] = []\n    using h = open(\"a\"):\n        kept.append(h)\n    return kept.length()\n";
    assert_eq!(resource_errors("append", body), one(H + 4, "E0081"));
}

#[test]
fn putting_a_resource_into_a_map_is_an_escape() {
    let body = "function f() returns Int:\n    var kept as Map[Text, Handle] = {}\n    using h = open(\"a\"):\n        kept.put(\"a\", h)\n    return kept.length()\n";
    assert_eq!(resource_errors("put", body), one(H + 4, "E0081"));
}

#[test]
fn setting_a_map_element_to_a_resource_is_an_escape() {
    let body = "function f() returns Int:\n    var kept as Map[Text, Handle] = {}\n    using h = open(\"a\"):\n        kept[\"a\"] = h\n    return kept.length()\n";
    assert_eq!(resource_errors("index_set", body), one(H + 4, "E0081"));
}

#[test]
fn appending_a_list_that_holds_a_resource_is_an_escape() {
    let body = "function f() returns Int:\n    var kept as List[Handle] = []\n    using h = open(\"a\"):\n        kept.append_all([h])\n    return kept.length()\n";
    assert_eq!(resource_errors("append_all", body), one(H + 4, "E0081"));
}

#[test]
fn a_resource_passed_to_a_generic_parameter_is_an_escape() {
    let body = "function keep[T](var items as List[T], item as T):\n    items.append(item)\n\nfunction f() returns Int:\n    var kept as List[Handle] = []\n    using h = open(\"a\"):\n        keep(kept, h)\n    return kept.length()\n";
    assert_eq!(resource_errors("generic", body), one(H + 7, "E0081"));
}

#[test]
fn a_resource_wrapped_in_some_and_stored_in_a_field_is_an_escape() {
    let body = "struct Holder[T]:\n    var item as Option[T]\n\nfunction f() returns Int:\n    var box = Holder[Handle](item = none)\n    using h = open(\"a\"):\n        box.item = some(h)\n    return 0\n";
    assert_eq!(resource_errors("some_field", body), one(H + 7, "E0081"));
}

#[test]
fn a_resource_in_an_enum_variant_is_an_escape() {
    let body = "enum Slot:\n    Empty\n    Full(handle as Handle)\n\nfunction f() returns Int:\n    var slot = Slot.Empty\n    using h = open(\"a\"):\n        slot = Slot.Full(h)\n    return 0\n";
    assert_eq!(resource_errors("variant", body), one(H + 8, "E0081"));
}

#[test]
fn a_resource_parameter_appended_to_a_list_is_an_escape() {
    let body = "function keep(var items as List[Handle], h as Handle):\n    items.append(h)\n";
    assert_eq!(resource_errors("parameter", body), one(H + 2, "E0081"));
}

#[test]
fn a_resource_passed_to_a_resource_parameter_is_used_not_kept() {
    let body = "function look(h as Handle) returns Text:\n    return h.name\n\nfunction same[T with Resource](a as T, b as T) returns Bool:\n    return is_same(a, b)\n\nfunction f() returns Text:\n    using h = open(\"a\"):\n        let shown = look(h)\n        let check = function (x as Handle) returns Text: x.name\n        assert same(h, h)\n        return shown + check(h)\n";
    assert_eq!(resource_errors("used", body), vec![]);
}

// ------------------------------------------------------------ 4. var parameters (L-29, E0042)

#[test]
fn one_name_for_var_self_and_a_var_parameter_is_e0042() {
    let source = "struct A:\n    var n as Int\n\nextend A:\n    function absorb(var self, var other as A):\n        self.n = self.n + other.n\n\nfunction f() returns Int:\n    var a = A(n = 1)\n    a.absorb(a)\n    return a.n\n";
    assert_eq!(errors("var_self_twice", source), one(10, "E0042"));
}

#[test]
fn a_field_of_the_receiver_for_a_var_parameter_is_e0042() {
    let source = "struct A:\n    var n as Int\n    var inner as List[Int]\n\nextend A:\n    function absorb(var self, var other as List[Int]):\n        self.n = other.length()\n\nfunction f() returns Int:\n    var a = A(n = 1, inner = [])\n    a.absorb(a.inner)\n    return a.n\n";
    assert_eq!(errors("var_self_field", source), one(11, "E0042"));
}

#[test]
fn another_name_for_a_var_parameter_is_accepted() {
    let source = "struct A:\n    var n as Int\n\nextend A:\n    function absorb(var self, var other as A):\n        self.n = self.n + other.n\n\nfunction f() returns Int:\n    var a = A(n = 1)\n    var b = A(n = 2)\n    a.absorb(b)\n    return a.n\n";
    assert_eq!(errors("var_self_other", source), vec![]);
}

// ------------------------------------------------------------ 5. receivers of implementations

#[test]
fn an_implementation_with_another_receiver_mutability_is_e0069() {
    let source = "interface Counter:\n    function bump(var self)\n\nstruct C:\n    var n as Int\n\nextend C with Counter:\n    function bump(self):\n        print(\"{self.n}\")\n";
    assert_eq!(errors("receiver_var", source), one(8, "E0069"));
}

#[test]
fn an_implementation_with_var_self_for_self_is_e0069() {
    let source = "interface Shown:\n    function show(self) returns Text\n\nstruct C:\n    var n as Int\n\nextend C with Shown:\n    function show(var self) returns Text:\n        self.n = self.n + 1\n        return \"{self.n}\"\n";
    assert_eq!(errors("receiver_let", source), one(8, "E0069"));
}

#[test]
fn an_implementation_with_another_var_parameter_is_e0069() {
    let source = "interface Filler:\n    function fill(self, var items as List[Int])\n\nstruct C:\n    n as Int\n\nextend C with Filler:\n    function fill(self, items as List[Int]):\n        print(\"{items.length()}\")\n";
    assert_eq!(errors("param_var", source), one(8, "E0069"));
}

// ------------------------------------------------------------ 6. duplicate functions

#[test]
fn a_duplicate_function_is_checked_with_its_own_parameters() {
    let source = "function f(a as Int) returns Int:\n    return a\n\nfunction f(b as Int) returns Int:\n    return b\n";
    assert_eq!(errors("duplicate", source), one(4, "E0051"));
}

#[test]
fn a_duplicate_method_is_checked_with_its_own_parameters() {
    let source = "struct A:\n    n as Int\n\nextend A:\n    function f(self, a as Int) returns Int:\n        return a + self.n\n\n    function f(self, b as Int) returns Int:\n        return b\n";
    assert_eq!(errors("duplicate_method", source), one(8, "E0051"));
}

#[test]
fn a_mistake_in_a_duplicate_function_is_still_reported() {
    let source = "function f(a as Int) returns Int:\n    return a\n\nfunction f(b as Int) returns Int:\n    return c\n";
    assert_eq!(
        errors("duplicate_mistake", source),
        vec![(4, "E0051".to_string()), (5, "E0056".to_string())]
    );
}

// ------------------------------------------------------------ 7, 8 and minor: syntax

#[test]
fn a_broken_last_statement_of_a_block_gives_one_error() {
    let source = "function f(values as List[Int]) returns Int:\n    var total = 0\n    for value in values:\n        total += value\n    return total\n";
    assert_eq!(syntax_errors(source), one(4, "E0012"));
}

#[test]
fn a_broken_only_statement_of_an_if_gives_one_error() {
    let source = "function f(x as Int) returns Int:\n    if x > 0:\n        x =\n    return x\n";
    assert_eq!(syntax_errors(source), one(3, "E0012"));
}

const FAILING: &str =
    "error E:\n    code as Int\n\nfunction g() returns Int fails with E:\n    return 1\n\n";

#[test]
fn a_handler_inside_parentheses_is_e0024() {
    let source = format!(
        "{FAILING}function f() returns Int:\n    let x = (try g() on E as e: return 0) + 1\n    return x\n"
    );
    assert_eq!(syntax_errors(&source), one(8, "E0024"));
}

#[test]
fn a_handler_inside_brackets_or_arguments_is_e0024() {
    let list = format!(
        "{FAILING}function f() returns Int:\n    let x = [try g() on E as e: 1]\n    return x[0]\n"
    );
    assert_eq!(syntax_errors(&list), one(8, "E0024"));
    let call = format!(
        "{FAILING}function f() returns Int:\n    let x = g(try g() on E as e: 0)\n    return x\n"
    );
    assert_eq!(syntax_errors(&call), one(8, "E0024"));
}

#[test]
fn a_second_handler_of_one_statement_is_e0024() {
    let source = format!(
        "{FAILING}function f() returns Int:\n    let x = try g() on E as e:\n        return 0\n    on E as e:\n        return 1\n    return x\n"
    );
    assert_eq!(syntax_errors(&source), one(10, "E0024"));
}

#[test]
fn a_handler_body_on_its_header_line_is_e0024() {
    let source = format!(
        "{FAILING}function f() returns Int:\n    let x = try g() on E as e: return 0\n    return x\n"
    );
    assert_eq!(syntax_errors(&source), one(8, "E0024"));
}

#[test]
fn a_doubled_invalid_character_is_one_error() {
    let source = "function f(a as Bool, b as Bool) returns Bool:\n    return a && b\n";
    assert_eq!(syntax_errors(source), one(2, "E0007"));
}

#[test]
fn a_bracket_closed_over_an_open_parenthesis_is_one_error() {
    let source = "function g(a as Int, b as Int) returns Int:\n    return a\n\nfunction f() returns List[Int]:\n    return [g(1, 2]\n";
    let found = unio::check_source("brackets.unio", source);
    let codes: Vec<(usize, String)> = found
        .diagnostics
        .iter()
        .map(|d| (d.line(), d.code.to_string()))
        .collect();
    assert_eq!(codes, one(5, "E0010"));
    assert!(
        found.render_text(unio::Lang::En).contains("'('"),
        "the message names the parenthesis left open"
    );
}
