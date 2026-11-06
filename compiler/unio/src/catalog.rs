//! The diagnostic catalog (R-12, C-9): every message of the compiler in English and Russian,
//! and for every code its explanation, an example of the error, and the example fixed.
//! Messages follow one template: what is wrong (the message), where (the span), and how to
//! fix it (the fix of the diagnostic where one exists, the explanation otherwise). A message
//! never names compiler internals: `INTERNAL_NAMES` lists them, and tests/catalog.rs checks
//! every message and explanation against it. The review checklist is compiler/DIAGNOSTICS.md.

use crate::diag::{Lang, Msg};

/// Words that must not appear in a message or an explanation: token classes, names of the
/// syntax tree and of the passes, and Rust types. A new internal name is added here.
pub const INTERNAL_NAMES: &[&str] = &[
    "INDENT",
    "DEDENT",
    "NEWLINE",
    "EOF",
    "NAME",
    "INT",
    "FLOAT",
    "TEXT",
    "Kind",
    "Token",
    "TokenKind",
    "Expr",
    "ExprKind",
    "Statement",
    "StatementKind",
    "PatternKind",
    "DeclarationKind",
    "Simple",
    "Piece",
    "Analysis",
    "Parser",
    "Lexer",
    "Formatter",
    "sexpr",
    "lexer",
    "parser",
    "AST",
    "IR",
    "handled_statement",
    "handled_head",
    "branch_block",
    "simple_statement",
    "inline_handler",
    "lambda_param",
    "primary_pattern",
    "postfix",
    "Vec",
    "String",
    "usize",
    "panic",
    "unwrap",
];

/// Every code the front end reports.
pub const CODES: &[&str] = &[
    "E0001", "E0002", "E0003", "E0005", "E0006", "E0007", "E0008", "E0009", "E0010", "E0011",
    "E0012", "E0013", "E0020", "E0023", "E0024", "E0030", "E0032", "E0034", "E0040", "E0041",
    "E0044", "E0049", "E0050", "E0051", "E0052", "E0053", "E0054", "E0055", "E0056", "E0057",
    "E0058", "E0059", "E0060", "E0061", "E0062", "E0063", "E0064", "E0065", "E0066", "E0067",
    "E0069", "E0071", "E0072",
];

/// Codes of the name and type checks (sprint S3); their examples need the whole check.
pub const SEMANTIC_CODES: &[&str] = &[
    "E0040", "E0041", "E0044", "E0049", "E0050", "E0051", "E0052", "E0053", "E0054", "E0055",
    "E0056", "E0057", "E0058", "E0059", "E0060", "E0061", "E0062", "E0063", "E0064", "E0065",
    "E0066", "E0067", "E0069", "E0071", "E0072",
];

/// A catalog entry: the explanation and a pair of examples.
pub struct Entry {
    pub code: &'static str,
    pub explanation: Msg,
    /// A source that produces the diagnostic.
    pub example: &'static str,
    /// The same source fixed.
    pub fixed: &'static str,
}

pub fn entry(code: &str) -> Option<Entry> {
    let (en, ru, example, fixed) = match code {
        "E0001" => (
            "Indentation consists of spaces only. Replace the tab with four spaces; `unio fmt` does this for a whole file.",
            "Отступ состоит только из пробелов. Замените табуляцию четырьмя пробелами; `unio fmt` делает это для всего файла.",
            "function f():\n\tpass\n",
            "function f():\n    pass\n",
        ),
        "E0002" => (
            "Each block is indented by exactly four spaces more than the line that opens it. Indent the block by a multiple of four spaces.",
            "Каждый блок сдвинут ровно на четыре пробела относительно строки, которая его открывает. Сдвиньте блок на число пробелов, кратное четырём.",
            "function f():\n  pass\n",
            "function f():\n    pass\n",
        ),
        "E0003" => (
            "Indentation may grow only after a line that ends with ':', and only by one level of four spaces. Add the missing ':' or remove the extra spaces.",
            "Отступ может расти только после строки, которая заканчивается на ':', и только на один уровень в четыре пробела. Добавьте пропущенное ':' или уберите лишние пробелы.",
            "function f():\n    let x = 1\n        let y = 2\n",
            "function f():\n    let x = 1\n    let y = 2\n",
        ),
        "E0005" => (
            "A text literal starts and ends with '\"' on the same line. Close the text or use \\n for a line end.",
            "Текстовый литерал начинается и заканчивается '\"' в одной строке. Закройте текст или используйте \\n для конца строки.",
            "let A = \"unclosed\n",
            "let A = \"closed\"\n",
        ),
        "E0006" => (
            "Valid escapes are \\n \\t \\r \\0 \\\\ \\\" \\{ \\} and \\u{hex} with one to six hexadecimal digits. Write a backslash itself as \\\\.",
            "Допустимые escape-последовательности: \\n \\t \\r \\0 \\\\ \\\" \\{ \\} и \\u{hex} с одной–шестью шестнадцатеричными цифрами. Сама обратная косая черта пишется как \\\\.",
            "let A = \"\\q\"\n",
            "let A = \"\\\\q\"\n",
        ),
        "E0007" => (
            "Outside text and comments only ASCII letters, digits, '_', white space and the punctuation of the language may appear. Remove the character or put it into a text.",
            "Вне текста и комментариев допустимы только латинские буквы, цифры, '_', пробелы и знаки языка. Уберите символ или поместите его в текст.",
            "let A = 1 $ 2\n",
            "let A = 1 + 2\n",
        ),
        "E0008" => (
            "A number is decimal (1_000, 2.5, 1e-3), hexadecimal (0xFF) or binary (0b1010) and cannot be followed directly by a letter. Separate the number from the name that follows it.",
            "Число бывает десятичным (1_000, 2.5, 1e-3), шестнадцатеричным (0xFF) или двоичным (0b1010) и не может сразу продолжаться буквой. Отделите число от следующего за ним имени.",
            "let A = 12abc\n",
            "let A = 12\n",
        ),
        "E0009" => (
            "Every '(', '[' and '{' needs its closing bracket. Inside brackets line ends are ignored, so the rest of the file was read as one line. Add the closing bracket.",
            "Каждой '(', '[' и '{' нужна закрывающая скобка. Внутри скобок концы строк игнорируются, поэтому остаток файла прочитан как одна строка. Добавьте закрывающую скобку.",
            "let A = (1 + 2\n",
            "let A = (1 + 2)\n",
        ),
        "E0010" => (
            "A closing bracket must match the innermost open bracket. Use the bracket that matches, or remove the extra one.",
            "Закрывающая скобка должна соответствовать последней открытой. Поставьте парную скобку или уберите лишнюю.",
            "let A = [1, 2)\n",
            "let A = [1, 2]\n",
        ),
        "E0011" => (
            "An interpolation {expression} in text needs an expression and a closing '}' on the same line; a literal brace is written \\{ or \\}.",
            "Интерполяции {выражение} в тексте нужны выражение и закрывающая '}' в той же строке; сама фигурная скобка пишется \\{ или \\}.",
            "let A = \"{}\"\n",
            "let A = \"\\{\\}\"\n",
        ),
        "E0012" => (
            "The code does not follow the grammar of Unio (spec/grammar.ebnf). The message names what is expected at this place; write that, or remove what stands there.",
            "Код не соответствует грамматике Unio (spec/grammar.ebnf). Сообщение называет, что ожидается в этом месте; напишите это или уберите то, что там стоит.",
            "let = 1\n",
            "let A = 1\n",
        ),
        "E0013" => (
            "Expressions, texts and blocks may be nested only to a limited depth. Split the expression or move inner parts into functions or named values.",
            "Выражения, тексты и блоки можно вкладывать только на ограниченную глубину. Разбейте выражение или вынесите внутренние части в функции или именованные значения.",
            "DEEP",
            "let A = 1\n",
        ),
        "E0020" => (
            "Comparisons do not chain. Write `a < b and b < c` instead of `a < b < c`.",
            "Сравнения не объединяются в цепочку. Пишите `a < b and b < c` вместо `a < b < c`.",
            "let A = 1 < 2 < 3\n",
            "let A = 1 < 2 and 2 < 3\n",
        ),
        "E0023" => (
            "Positional arguments come before named ones: f(1, name = 2). Move the positional argument before the named ones.",
            "Позиционные аргументы идут перед именованными: f(1, name = 2). Перенесите позиционный аргумент перед именованными.",
            "let A = f(x = 1, 2)\n",
            "let A = f(2, x = 1)\n",
        ),
        "E0024" => (
            "An inline handler `try X on E as e:` is the last part of a statement, there is one per statement, and its block follows on the next lines. End the line with ':' and write the handler below it.",
            "Строчный обработчик `try X on E as e:` — последняя часть инструкции, он один на инструкцию, и его блок следует на следующих строках. Закончите строку на ':' и запишите обработчик ниже.",
            "function f():\n    let x = try g() on E as e\n    pass\n",
            "function f() fails with E:\n    let x = try g() on E as e:\n        fail e\n",
        ),
        "E0030" => (
            "Only a name, a field (a.b) or an element (a[i]) can be assigned to. Assign to a variable declared with var.",
            "Присвоить можно только имени, полю (a.b) или элементу (a[i]). Присваивайте переменной, объявленной через var.",
            "function f():\n    g() = 1\n",
            "function f():\n    var x = 1\n    x = 2\n",
        ),
        "E0032" => (
            "The word is reserved for a later release and cannot be used as a name. Choose another name.",
            "Слово зарезервировано для следующих выпусков и не может быть именем. Выберите другое имя.",
            "let uses = 1\n",
            "let used = 1\n",
        ),
        "E0034" => (
            "A list pattern has at most one rest element `name...`, and it is the last element. Move the rest element to the end.",
            "В образце списка не больше одного остаточного элемента `name...`, и он последний. Перенесите остаточный элемент в конец.",
            "function f(x as List[Int]) returns Int:\n    match x:\n        case [rest..., last]: return last\n        case _: return 0\n",
            "function f(x as List[Int]) returns Int:\n    match x:\n        case [first, rest...]: return first\n        case _: return 0\n",
        ),
        _ => semantic_entry(code)?,
    };
    let example = if example == "DEEP" { DEEP } else { example };
    Some(Entry {
        code: CODES
            .iter()
            .copied()
            .find(|c| *c == code)
            .unwrap_or("E0000"),
        explanation: Msg::new(en, ru),
        example,
        fixed,
    })
}

const DEEP: &str = "let A = ((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((1))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))\n";

/// The explanation of a code; a fallback for codes the catalog does not know.
pub fn explanation(code: &str) -> Msg {
    entry(code).map(|e| e.explanation).unwrap_or_else(|| {
        Msg::new(
            "No explanation is recorded for this code yet.",
            "Объяснение для этого кода пока не записано.",
        )
    })
}

/// `unio explain CODE`: the explanation, an example of the error, and the example fixed.
pub fn explain(code: &str, lang: Lang) -> Option<String> {
    let e = entry(code)?;
    let (example, fixed) = match lang {
        Lang::En => ("Example of the error:", "Fixed:"),
        Lang::Ru => ("Пример ошибки:", "Исправлено:"),
    };
    let indent = |s: &str| {
        s.lines()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Some(format!(
        "{}: {}\n\n{example}\n{}\n\n{fixed}\n{}\n",
        e.code,
        e.explanation.get(lang),
        indent(e.example),
        indent(e.fixed)
    ))
}

// ---------------------------------------------------------------- messages

pub fn tab_in_indentation() -> Msg {
    Msg::new("tab in indentation", "табуляция в отступе")
}

pub fn replace_tab() -> Msg {
    Msg::new(
        "replace the tab with four spaces",
        "заменить табуляцию четырьмя пробелами",
    )
}

pub fn indentation_not_multiple() -> Msg {
    Msg::new(
        "indentation is not a multiple of 4 spaces",
        "отступ не кратен 4 пробелам",
    )
}

pub fn indentation_grows_too_much() -> Msg {
    Msg::new(
        "indentation grows by more than one level",
        "отступ вырос больше чем на один уровень",
    )
}

pub fn unexpected_indentation() -> Msg {
    Msg::new(
        "unexpected indentation: the previous line does not end with ':'",
        "неожиданный отступ: предыдущая строка не заканчивается на ':'",
    )
}

pub fn unclosed(open: char) -> Msg {
    Msg::new(format!("unclosed '{open}'"), format!("незакрытая '{open}'"))
}

pub fn reserved(word: &str) -> Msg {
    Msg::new(
        format!("'{word}' is reserved for a later release"),
        format!("'{word}' зарезервировано для следующих выпусков"),
    )
}

pub fn unmatched(close: &str) -> Msg {
    Msg::new(
        format!("unmatched '{close}'"),
        format!("'{close}' без парной открывающей скобки"),
    )
}

pub fn mismatched(close: &str, open: char) -> Msg {
    Msg::new(
        format!("'{close}' does not match '{open}'"),
        format!("'{close}' не соответствует '{open}'"),
    )
}

pub fn invalid_character(shown: &str) -> Msg {
    Msg::new(
        format!("invalid character '{shown}'"),
        format!("недопустимый символ '{shown}'"),
    )
}

pub fn invalid_number(bad: &str) -> Msg {
    Msg::new(
        format!("invalid number literal '{bad}'"),
        format!("недопустимый числовой литерал '{bad}'"),
    )
}

pub fn texts_nested_too_deeply() -> Msg {
    Msg::new(
        "texts are nested too deeply inside interpolations",
        "тексты вложены в интерполяции слишком глубоко",
    )
}

pub fn unterminated_text() -> Msg {
    Msg::new("unterminated text literal", "незакрытый текстовый литерал")
}

pub fn invalid_unicode_escape() -> Msg {
    Msg::new(
        "invalid \\u{...} escape",
        "недопустимая последовательность \\u{...}",
    )
}

pub fn invalid_escape(shown: &str) -> Msg {
    Msg::new(
        format!("invalid escape sequence '\\{shown}'"),
        format!("недопустимая escape-последовательность '\\{shown}'"),
    )
}

pub fn brace_in_text() -> Msg {
    Msg::new(
        "'}' in text must be written as '\\}'",
        "'}' в тексте пишется как '\\}'",
    )
}

pub fn unterminated_interpolation() -> Msg {
    Msg::new("unterminated interpolation", "незакрытая интерполяция")
}

pub fn empty_interpolation() -> Msg {
    Msg::new("empty interpolation", "пустая интерполяция")
}

/// E0012: `what` is the expected thing, `found` the token that stands there.
pub fn expected(what: &Msg, found: &Msg) -> Msg {
    Msg::new(
        format!("expected {}, found {}", what.en, found.en),
        format!("здесь ожидается {}, а стоит {}", what.ru, found.ru),
    )
}

pub fn add_colon() -> Msg {
    Msg::new(
        "add ':' at the end of the line",
        "добавить ':' в конце строки",
    )
}

pub fn nested_too_deeply() -> Msg {
    Msg::new(
        "the code is nested too deeply",
        "код вложен слишком глубоко",
    )
}

pub fn handler_in_branch() -> Msg {
    Msg::new(
        "an inline handler needs a block; write this branch on its own indented lines",
        "строчному обработчику нужен блок; запишите эту ветку отдельными строками с отступом",
    )
}

pub fn one_inline_handler() -> Msg {
    Msg::new(
        "a statement may have only one inline handler",
        "в инструкции может быть только один строчный обработчик",
    )
}

pub fn handler_not_last() -> Msg {
    Msg::new(
        "the inline handler must be the last part of the statement",
        "строчный обработчик должен быть последней частью инструкции",
    )
}

pub fn handler_needs_block() -> Msg {
    Msg::new(
        "an inline handler needs a block: end the line with ':' and indent the handler",
        "строчному обработчику нужен блок: закончите строку ':' и запишите обработчик с отступом",
    )
}

pub fn invalid_assignment_target() -> Msg {
    Msg::new(
        "only a name, a field or an element can be assigned to",
        "присвоить можно только имени, полю или элементу",
    )
}

pub fn chained_comparison() -> Msg {
    Msg::new(
        "comparisons cannot be chained; join them with 'and'",
        "сравнения нельзя объединять в цепочку; соедините их через 'and'",
    )
}

pub fn positional_after_named() -> Msg {
    Msg::new(
        "a positional argument cannot follow a named one",
        "позиционный аргумент не может идти после именованного",
    )
}

pub fn handler_in_interpolation() -> Msg {
    Msg::new(
        "an inline handler cannot stand inside an interpolation",
        "строчный обработчик не может стоять внутри интерполяции",
    )
}

pub fn rest_not_last() -> Msg {
    Msg::new(
        "the rest element 'name...' must be the last element",
        "остаточный элемент 'name...' должен быть последним",
    )
}

/// Every message with sample arguments, for the template and internal-name tests.
pub fn sample_messages() -> Vec<(&'static str, Msg)> {
    let found = Msg::new("name 'x'", "имя 'x'");
    let what = Msg::new("':'", "':'");
    vec![
        ("E0001", tab_in_indentation()),
        ("E0001", replace_tab()),
        ("E0002", indentation_not_multiple()),
        ("E0003", indentation_grows_too_much()),
        ("E0003", unexpected_indentation()),
        ("E0009", unclosed('(')),
        ("E0032", reserved("uses")),
        ("E0010", unmatched(")")),
        ("E0010", mismatched(")", '[')),
        ("E0007", invalid_character("$")),
        ("E0008", invalid_number("12abc")),
        ("E0013", texts_nested_too_deeply()),
        ("E0005", unterminated_text()),
        ("E0006", invalid_unicode_escape()),
        ("E0006", invalid_escape("q")),
        ("E0011", brace_in_text()),
        ("E0011", unterminated_interpolation()),
        ("E0011", empty_interpolation()),
        ("E0012", expected(&what, &found)),
        ("E0012", add_colon()),
        ("E0013", nested_too_deeply()),
        ("E0024", handler_in_branch()),
        ("E0024", one_inline_handler()),
        ("E0024", handler_not_last()),
        ("E0024", handler_needs_block()),
        ("E0030", invalid_assignment_target()),
        ("E0020", chained_comparison()),
        ("E0023", positional_after_named()),
        ("E0024", handler_in_interpolation()),
        ("E0034", rest_not_last()),
        ("E0051", duplicate("x")),
        ("E0052", import_collision("x")),
        ("E0053", module_not_found("std.x")),
        ("E0054", name_not_in_module("x", "std.text")),
        ("E0058", import_cycle("a -> b -> a")),
        ("E0059", type_arity("Map", 2, 1)),
        ("E0057", private_used("x")),
        ("E0057", private_fields("AtomicFile")),
        ("E0056", did_you_mean("count")),
        ("E0056", unknown_type("Itme")),
        ("E0056", unknown_name("x")),
        ("E0056", no_field("x", "Point")),
        ("E0056", not_in_module("x", "std.files")),
        ("E0056", no_member("x", "Value")),
        ("E0056", no_method("x", "Text")),
        ("E0071", not_object_safe("Equal", "equals")),
        ("E0041", parameter_grouping()),
        ("E0055", local_repeats("x")),
        ("E0044", second_implementation("Display", "Point")),
        ("E0044", foreign_implementation("Display", "Int")),
        ("E0069", missing_method("Display", "to_text")),
        ("E0069", signature_differs("to_text")),
        ("E0060", cannot_infer()),
        ("E0061", returns_nothing()),
        ("E0061", missing_return_value("Int")),
        ("E0061", cannot_destructure("Int", 2)),
        ("E0061", not_iterable("Int")),
        ("E0061", type_mismatch("Int", "Text")),
        ("E0061", type_is_not_a_value("Point")),
        ("E0061", module_is_not_a_value("files")),
        ("E0061", no_membership("Int")),
        ("E0061", not_callable("Int")),
        ("E0040", missing_label("port")),
        ("E0064", too_many_arguments(1)),
        ("E0064", argument_twice("x")),
        ("E0064", unknown_argument("x")),
        ("E0064", missing_argument("x")),
        ("E0062", constraint_not_met("Point", "Order")),
        ("E0063", fields_are_named("Point")),
        ("E0063", unknown_field("z", "Point")),
        ("E0063", missing_field("y", "Point")),
        ("E0063", variant_fields("Add", 1)),
        ("E0072", variant_collision("List", "Command")),
        ("E0050", is_name_not_bound("x")),
        ("E0065", operator_not_defined("+", "Point")),
        ("E0065", operands_differ("==", "Int", "Text")),
        ("E0065", text_plus("Int")),
        ("E0065", not_indexable("Int")),
        ("E0066", resource_equality()),
        ("E0067", not_display("Point")),
        ("E0049", lambda_needs_types("x")),
    ]
}

// ---------------------------------------------------------------- what E0012 expects

/// A symbol or a keyword in quotes; the same in both languages.
pub fn quoted(s: &str) -> Msg {
    Msg::new(format!("'{s}'"), format!("'{s}'"))
}

pub fn a_name() -> Msg {
    Msg::new("a name", "имя")
}

pub fn end_of_line() -> Msg {
    Msg::new("the end of the line", "конец строки")
}

pub fn declaration_after_public() -> Msg {
    Msg::new("a declaration after 'public'", "объявление после 'public'")
}

pub fn declaration_or_import() -> Msg {
    Msg::new(
        "a declaration, an import, 'extend' or 'test'",
        "объявление, импорт, 'extend' или 'test'",
    )
}

pub fn block_of_fields() -> Msg {
    Msg::new("an indented block of fields", "блок полей с отступом")
}

pub fn indented_block() -> Msg {
    Msg::new("an indented block", "блок с отступом")
}

pub fn a_function() -> Msg {
    Msg::new("a function", "функция")
}

pub fn a_statement() -> Msg {
    Msg::new("a statement", "инструкция")
}

pub fn on_after_try() -> Msg {
    Msg::new("'on' after the block of 'try:'", "'on' после блока 'try:'")
}

pub fn brace_after_interpolation() -> Msg {
    Msg::new(
        "'}' after the interpolated expression",
        "'}' после выражения интерполяции",
    )
}

pub fn an_expression() -> Msg {
    Msg::new("an expression", "выражение")
}

pub fn a_pattern() -> Msg {
    Msg::new("a pattern", "образец")
}

// ---------------------------------------------------------------- what stands in the source

pub fn token_name(text: &str) -> Msg {
    Msg::new(format!("name '{text}'"), format!("имя '{text}'"))
}

pub fn token_number(text: &str) -> Msg {
    Msg::new(format!("number {text}"), format!("число {text}"))
}

pub fn token_text() -> Msg {
    Msg::new("text", "текст")
}

pub fn token_line_end() -> Msg {
    Msg::new("the end of the line", "конец строки")
}

pub fn token_block_start() -> Msg {
    Msg::new("an indented block", "блок с отступом")
}

pub fn token_block_end() -> Msg {
    Msg::new("the end of the block", "конец блока")
}

pub fn token_file_end() -> Msg {
    Msg::new("the end of the file", "конец файла")
}

// ---------------------------------------------------------------- names and types (sprint S3)

type EntryText = (&'static str, &'static str, &'static str, &'static str);

/// Entries of the codes of docs/semantics.md, sections 1–4, and of RFC 0001 and RFC 0002.
/// Examples are whole files; the example of E0058 is named example.unio and imports itself.
fn semantic_entry(code: &str) -> Option<EntryText> {
    Some(match code {
        "E0040" => (
            "A parameter declared `named` must be given with its name at every call (RFC 0001). Write the name before the value.",
            "Параметр, объявленный `named`, передаётся с именем при каждом вызове (RFC 0001). Напишите имя перед значением.",
            "function connect(host as Text, named port as Int) returns Text:\n    return host\n\nlet A as Text = connect(\"h\", 80)\n",
            "function connect(host as Text, named port as Int) returns Text:\n    return host\n\nlet A as Text = connect(\"h\", port = 80)\n",
        ),
        "E0041" => (
            "Parameters given only by name follow the ordinary ones, and parameters with defaults come last within each group (RFC 0001). Reorder the parameters.",
            "Параметры только по имени идут после обычных, а параметры со значениями по умолчанию — последними в каждой группе (RFC 0001). Переставьте параметры.",
            "function connect(named port as Int, host as Text) returns Text:\n    return host\n",
            "function connect(host as Text, named port as Int) returns Text:\n    return host\n",
        ),
        "E0044" => (
            "A type implements an interface once, in the module that declares the type or the interface. Remove the second implementation or move it there.",
            "Тип реализует интерфейс один раз, в модуле, где объявлен тип или интерфейс. Удалите вторую реализацию или перенесите её туда.",
            "struct P:\n    x as Int\n\nextend P with Display:\n    function to_text(self) returns Text:\n        return \"p\"\n\nextend P with Display:\n    function to_text(self) returns Text:\n        return \"q\"\n",
            "struct P:\n    x as Int\n\nextend P with Display:\n    function to_text(self) returns Text:\n        return \"p\"\n",
        ),
        "E0049" => (
            "A lambda may leave out its types only where a function type is expected, as in an argument (L-8a). Write the types of its parameters.",
            "Анонимная функция может опустить типы только там, где ожидается тип функции, например в аргументе (Я-8a). Запишите типы её параметров.",
            "let DOUBLE = function (x): x * 2\n",
            "let DOUBLE = function (x as Int) returns Int: x * 2\n",
        ),
        "E0050" => (
            "A name bound by `is` exists only in the branch the test guards (L-33). Use it inside that branch, or use `match`.",
            "Имя, связанное `is`, существует только в ветке, которую охраняет проверка (Я-33). Используйте его внутри ветки или примените `match`.",
            "function f(x as Option[Int]) returns Int:\n    if x is some(v):\n        return v\n    return v\n",
            "function f(x as Option[Int]) returns Int:\n    if x is some(v):\n        return v\n    return 0\n",
        ),
        "E0051" => (
            "A module declares each name once, and a type has one method of each name. Rename or remove one of the declarations.",
            "Модуль объявляет каждое имя один раз, а у типа один метод с каждым именем. Переименуйте или удалите одно из объявлений.",
            "function f() returns Int:\n    return 1\n\nfunction f() returns Int:\n    return 2\n",
            "function f() returns Int:\n    return 1\n\nfunction g() returns Int:\n    return 2\n",
        ),
        "E0052" => (
            "An imported name may not repeat a declaration of the module or another import. Import the module under another name with `as`.",
            "Импортированное имя не может повторять объявление модуля или другой импорт. Импортируйте модуль под другим именем через `as`.",
            "import std.files\n\nfunction files() returns Int:\n    return 1\n",
            "import std.files as file_system\n\nfunction files() returns Int:\n    return 1\n",
        ),
        "E0053" => (
            "The module is not in the standard library or in the package. Check the spelling of the module path.",
            "Модуля нет ни в стандартной библиотеке, ни в пакете. Проверьте написание пути модуля.",
            "import std.filez\n",
            "import std.files\n",
        ),
        "E0054" => (
            "The module does not declare the imported name, or does not make it public. Check the name in the module.",
            "Модуль не объявляет импортируемое имя или не делает его публичным. Проверьте имя в модуле.",
            "from std.text import joined\n",
            "from std.text import join\n",
        ),
        "E0055" => (
            "A local name may not repeat a local name that is visible at that place, so that a reader always knows which one is meant. Choose another name.",
            "Локальное имя не может повторять видимое в этом месте локальное имя, чтобы читатель всегда знал, какое имеется в виду. Выберите другое имя.",
            "function f(items as List[Int]) returns Int:\n    let total = 0\n    for total in items:\n        pass\n    return total\n",
            "function f(items as List[Int]) returns Int:\n    let total = 0\n    for item in items:\n        pass\n    return total\n",
        ),
        "E0056" => (
            "The name is not declared where it is used. Check the spelling; the fix suggests the closest visible name.",
            "Имя не объявлено там, где используется. Проверьте написание; исправление предлагает ближайшее видимое имя.",
            "function f(count as Int) returns Int:\n    return cuont\n",
            "function f(count as Int) returns Int:\n    return count\n",
        ),
        "E0057" => (
            "The declaration, field, or method is private to its module (L-23). Use a public function of that module, or make the member public there.",
            "Объявление, поле или метод приватны для своего модуля (Я-23). Используйте публичную функцию этого модуля или сделайте член публичным там.",
            "import std.files\n\nfunction f(file as files.AtomicFile) returns Bool:\n    return file.durable\n",
            "import std.files\n\nfunction f(file as files.AtomicFile) returns Bool:\n    return file.is_replaced()\n",
        ),
        "E0058" => (
            "Modules may not import each other in a cycle (L-24). Move what both need into a third module.",
            "Модули не могут импортировать друг друга по кругу (Я-24). Вынесите общее в третий модуль.",
            "import example\n\nlet A as Int = 1\n",
            "let A as Int = 1\n",
        ),
        "E0059" => (
            "A generic type takes a fixed number of type arguments: List one, Map two. Give the right number.",
            "Обобщённый тип принимает фиксированное число аргументов типа: List — один, Map — два. Укажите нужное число.",
            "let A as Map[Text] = {}\n",
            "let A as Map[Text, Int] = {}\n",
        ),
        "E0060" => (
            "The value does not tell its type, as with `[]`, `{}` or `none` alone. Write the type: `let items as List[Int] = []`.",
            "Значение не сообщает свой тип, как `[]`, `{}` или одно `none`. Запишите тип: `let items as List[Int] = []`.",
            "let A = []\n",
            "let A as List[Int] = []\n",
        ),
        "E0061" => (
            "The value has another type than the place expects, and there are no implicit conversions (L-5). Convert explicitly, or change the type of the place.",
            "Значение имеет не тот тип, который ожидает место, а неявных преобразований нет (Я-5). Преобразуйте явно или измените тип места.",
            "let A as Int = \"3\"\n",
            "let A as Int = 3\n",
        ),
        "E0062" => (
            "The type argument does not implement the interface the generic parameter requires. Implement it with `extend`, or use another type.",
            "Аргумент типа не реализует интерфейс, которого требует обобщённый параметр. Реализуйте его через `extend` или используйте другой тип.",
            "struct P:\n    x as Int\n\nfunction f(a as P, b as P) returns P:\n    return max(a, b)\n",
            "function f(a as Int, b as Int) returns Int:\n    return max(a, b)\n",
        ),
        "E0063" => (
            "A struct value names every field that has no default, and only declared fields. Add the missing field or remove the unknown one.",
            "Значение структуры называет каждое поле без значения по умолчанию и только объявленные поля. Добавьте недостающее поле или уберите лишнее.",
            "struct P:\n    x as Int\n    y as Int\n\nlet A as P = P(x = 1)\n",
            "struct P:\n    x as Int\n    y as Int\n\nlet A as P = P(x = 1, y = 2)\n",
        ),
        "E0064" => (
            "The arguments do not match the parameters: their number, a name that does not exist, or a name given twice. Compare the call with the signature.",
            "Аргументы не соответствуют параметрам: их число, несуществующее имя или имя, указанное дважды. Сверьте вызов с сигнатурой.",
            "function f(n as Int) returns Int:\n    return n\n\nlet A as Int = f(1, 2)\n",
            "function f(n as Int) returns Int:\n    return n\n\nlet A as Int = f(1)\n",
        ),
        "E0065" => (
            "The operator is not defined for these types; both operands have one type, and `+` on Text joins two texts. Use interpolation to put a number into text: \"count: {n}\".",
            "Оператор не определён для этих типов; оба операнда одного типа, а `+` для Text соединяет два текста. Чтобы вставить число в текст, используйте интерполяцию: \"count: {n}\".",
            "function f(n as Int) returns Text:\n    return \"count: \" + n\n",
            "function f(n as Int) returns Text:\n    return \"count: {n}\"\n",
        ),
        "E0066" => (
            "Resources have identity, not content, so `==` is not defined for them. Use `is_same(a, b)` to ask whether two names hold the same resource.",
            "У ресурсов есть тождественность, а не содержимое, поэтому `==` для них не определено. Чтобы узнать, один ли это ресурс, используйте `is_same(a, b)`.",
            "import std.files\n\nfunction f(a as files.AtomicFile, b as files.AtomicFile) returns Bool:\n    return a == b\n",
            "import std.files\n\nfunction f(a as files.AtomicFile, b as files.AtomicFile) returns Bool:\n    return is_same(a, b)\n",
        ),
        "E0067" => (
            "Only values of types that implement Display can be put into text. Implement Display for the type, or interpolate one of its fields.",
            "В текст можно подставить только значение типа, реализующего Display. Реализуйте Display для типа или подставьте одно из его полей.",
            "struct P:\n    x as Int\n\nfunction f(p as P) returns Text:\n    return \"{p}\"\n",
            "struct P:\n    x as Int\n\nfunction f(p as P) returns Text:\n    return \"{p.x}\"\n",
        ),
        "E0069" => (
            "An implementation defines every method of the interface that has no default, with the signature of the interface. Add the method or correct its signature.",
            "Реализация определяет каждый метод интерфейса без реализации по умолчанию, с сигнатурой из интерфейса. Добавьте метод или исправьте его сигнатуру.",
            "struct P:\n    x as Int\n\nextend P with Display:\n    function show(self) returns Text:\n        return \"p\"\n",
            "struct P:\n    x as Int\n\nextend P with Display:\n    function to_text(self) returns Text:\n        return \"p\"\n",
        ),
        "E0071" => (
            "An interface whose method uses Self outside the receiver, or has type parameters, cannot hold values of different types (L-6b). Use a generic parameter `[T with I]` instead.",
            "Интерфейс, метод которого использует Self вне получателя или имеет параметры типа, не может хранить значения разных типов (Я-6b). Используйте обобщённый параметр `[T with I]`.",
            "function f(items as List[Equal]) returns Int:\n    return items.length()\n",
            "function f[T with Equal](items as List[T]) returns Int:\n    return items.length()\n",
        ),
        "E0072" => (
            "A variant whose name is also a visible type, function, or another variant must be written with its enum name (L-6a). Write `Enum.Variant`.",
            "Вариант, имя которого совпадает с видимым типом, функцией или другим вариантом, пишется с именем перечисления (Я-6a). Пишите `Enum.Variant`.",
            "enum C:\n    List\n    Show\n\nlet A as C = List\n",
            "enum C:\n    List\n    Show\n\nlet A as C = C.List\n",
        ),
        _ => return None,
    })
}

pub fn duplicate(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is declared twice; rename one of them"),
        format!("'{name}' объявлено дважды; переименуйте одно из объявлений"),
    )
}

pub fn import_collision(name: &str) -> Msg {
    Msg::new(format!("the import '{name}' repeats another name of the module; import it with 'as' under another name"), format!("импорт '{name}' повторяет другое имя модуля; импортируйте его через 'as' под другим именем"))
}

pub fn module_not_found(name: &str) -> Msg {
    Msg::new(
        format!("module '{name}' not found in the standard library or the package"),
        format!("модуль '{name}' не найден ни в стандартной библиотеке, ни в пакете"),
    )
}

pub fn name_not_in_module(name: &str, module: &str) -> Msg {
    Msg::new(
        format!("module '{module}' has no public '{name}'"),
        format!("в модуле '{module}' нет публичного '{name}'"),
    )
}

pub fn import_cycle(chain: &str) -> Msg {
    Msg::new(format!("modules import each other in a cycle: {chain}; move what they share into another module"), format!("модули импортируют друг друга по кругу: {chain}; вынесите общее в другой модуль"))
}

pub fn type_arity(name: &str, expected: usize, found: usize) -> Msg {
    Msg::new(
        format!("'{name}' takes {expected} type argument(s), not {found}"),
        format!("'{name}' принимает аргументов типа: {expected}, а не {found}"),
    )
}

pub fn private_used(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is private to its module; use a public function of that module"),
        format!("'{name}' приватно для своего модуля; используйте публичную функцию этого модуля"),
    )
}

pub fn private_fields(name: &str) -> Msg {
    Msg::new(format!("'{name}' has private fields and is built only in its module; use one of its public functions"), format!("у '{name}' есть приватные поля, и строится оно только в своём модуле; используйте одну из его публичных функций"))
}

pub fn did_you_mean(name: &str) -> Msg {
    Msg::new(
        format!("replace with '{name}'"),
        format!("заменить на '{name}'"),
    )
}

pub fn unknown_type(name: &str) -> Msg {
    Msg::new(
        format!("unknown type '{name}'"),
        format!("неизвестный тип '{name}'"),
    )
}

pub fn unknown_name(name: &str) -> Msg {
    Msg::new(
        format!("unknown name '{name}'"),
        format!("неизвестное имя '{name}'"),
    )
}

pub fn not_object_safe(interface: &str, method: &str) -> Msg {
    Msg::new(format!("interface '{interface}' cannot be a type because of its method '{method}'; use a generic parameter [T with {interface}]"), format!("интерфейс '{interface}' не может быть типом из-за метода '{method}'; используйте обобщённый параметр [T with {interface}]"))
}

pub fn parameter_grouping() -> Msg {
    Msg::new("parameters given by name follow the others, and parameters with defaults come last in each group", "параметры по имени идут после остальных, а параметры со значениями по умолчанию — последними в каждой группе")
}

pub fn local_repeats(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is already a visible local name; choose another name"),
        format!("'{name}' уже видимое локальное имя; выберите другое имя"),
    )
}

pub fn second_implementation(interface: &str, ty: &str) -> Msg {
    Msg::new(
        format!("'{ty}' already implements '{interface}'; remove this implementation"),
        format!("'{ty}' уже реализует '{interface}'; удалите эту реализацию"),
    )
}

pub fn foreign_implementation(interface: &str, ty: &str) -> Msg {
    Msg::new(
        format!(
            "'{ty}' can implement '{interface}' only in the module of the type or of the interface"
        ),
        format!("'{ty}' может реализовать '{interface}' только в модуле типа или интерфейса"),
    )
}

pub fn missing_method(interface: &str, method: &str) -> Msg {
    Msg::new(
        format!("the implementation of '{interface}' lacks the method '{method}'"),
        format!("в реализации '{interface}' нет метода '{method}'"),
    )
}

pub fn signature_differs(method: &str) -> Msg {
    Msg::new(format!("'{method}' has another signature than in the interface; write it as the interface declares it"), format!("у '{method}' другая сигнатура, чем в интерфейсе; запишите её так, как объявляет интерфейс"))
}

pub fn cannot_infer() -> Msg {
    Msg::new("the type cannot be inferred from this value; write it, as in 'let items as List[Int] = []'", "тип нельзя вывести из этого значения; запишите его, как в 'let items as List[Int] = []'")
}

pub fn returns_nothing() -> Msg {
    Msg::new(
        "this function returns nothing; remove the value or declare 'returns'",
        "эта функция ничего не возвращает; уберите значение или объявите 'returns'",
    )
}

pub fn missing_return_value(ty: &str) -> Msg {
    Msg::new(
        format!("'return' needs a value of type {ty}"),
        format!("'return' нужно значение типа {ty}"),
    )
}

pub fn cannot_destructure(ty: &str, count: usize) -> Msg {
    Msg::new(
        format!("a value of type {ty} cannot be split into {count} names"),
        format!("значение типа {ty} нельзя разложить на имена: {count}"),
    )
}

pub fn not_iterable(ty: &str) -> Msg {
    Msg::new(
        format!("'for' cannot go through a value of type {ty}"),
        format!("'for' не может пройти по значению типа {ty}"),
    )
}

pub fn type_mismatch(expected: &str, found: &str) -> Msg {
    Msg::new(
        format!("expected {expected}, found {found}"),
        format!("ожидался {expected}, найден {found}"),
    )
}

pub fn missing_label(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is given only by name: write '{name} = ...'"),
        format!("'{name}' передаётся только по имени: пишите '{name} = ...'"),
    )
}

pub fn too_many_arguments(count: usize) -> Msg {
    Msg::new(
        format!("too many arguments; the function takes {count}"),
        format!("слишком много аргументов; функция принимает {count}"),
    )
}

pub fn argument_twice(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is given twice"),
        format!("'{name}' передано дважды"),
    )
}

pub fn unknown_argument(name: &str) -> Msg {
    Msg::new(
        format!("no parameter named '{name}'"),
        format!("нет параметра с именем '{name}'"),
    )
}

pub fn missing_argument(name: &str) -> Msg {
    Msg::new(
        format!("the argument '{name}' is missing"),
        format!("не хватает аргумента '{name}'"),
    )
}

pub fn constraint_not_met(ty: &str, interface: &str) -> Msg {
    Msg::new(
        format!("the function requires {interface}, which {ty} does not implement"),
        format!("функция требует {interface}, которого не реализует {ty}"),
    )
}

pub fn fields_are_named(ty: &str) -> Msg {
    Msg::new(
        format!("fields of '{ty}' are given by name: field = value"),
        format!("поля '{ty}' передаются по имени: поле = значение"),
    )
}

pub fn unknown_field(field: &str, ty: &str) -> Msg {
    Msg::new(
        format!("'{ty}' has no field '{field}'"),
        format!("у '{ty}' нет поля '{field}'"),
    )
}

pub fn missing_field(field: &str, ty: &str) -> Msg {
    Msg::new(
        format!("the field '{field}' of '{ty}' is missing"),
        format!("не хватает поля '{field}' у '{ty}'"),
    )
}

pub fn variant_fields(variant: &str, count: usize) -> Msg {
    Msg::new(
        format!("variant '{variant}' has {count} field(s)"),
        format!("у варианта '{variant}' полей: {count}"),
    )
}

pub fn variant_collision(variant: &str, enum_name: &str) -> Msg {
    Msg::new(
        format!("'{variant}' is also another visible name; write '{enum_name}.{variant}'"),
        format!("'{variant}' — ещё и другое видимое имя; пишите '{enum_name}.{variant}'"),
    )
}

pub fn is_name_not_bound(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is bound by 'is' only in the branch it guards"),
        format!("'{name}' связано через 'is' только в охраняемой ветке"),
    )
}

pub fn type_is_not_a_value(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is a type, not a value"),
        format!("'{name}' — тип, а не значение"),
    )
}

pub fn module_is_not_a_value(name: &str) -> Msg {
    Msg::new(
        format!("'{name}' is a module, not a value; use one of its members"),
        format!("'{name}' — модуль, а не значение; используйте один из его членов"),
    )
}

pub fn operator_not_defined(op: &str, ty: &str) -> Msg {
    Msg::new(
        format!("'{op}' is not defined for {ty}"),
        format!("'{op}' не определён для {ty}"),
    )
}

pub fn operands_differ(op: &str, left: &str, right: &str) -> Msg {
    Msg::new(
        format!("'{op}' needs operands of one type, found {left} and {right}"),
        format!("'{op}' требует операндов одного типа, найдены {left} и {right}"),
    )
}

pub fn text_plus(right: &str) -> Msg {
    Msg::new(format!("'+' joins two texts, found Text and {right}; use interpolation: \"text {{value}}\""), format!("'+' соединяет два текста, найдены Text и {right}; используйте интерполяцию: \"text {{value}}\""))
}

pub fn resource_equality() -> Msg {
    Msg::new(
        "resources have no '=='; use is_same(a, b)",
        "у ресурсов нет '=='; используйте is_same(a, b)",
    )
}

pub fn not_indexable(ty: &str) -> Msg {
    Msg::new(
        format!("a value of type {ty} cannot be indexed"),
        format!("значение типа {ty} нельзя индексировать"),
    )
}

pub fn not_display(ty: &str) -> Msg {
    Msg::new(
        format!("a value of type {ty} cannot be put into text: it does not implement Display"),
        format!("значение типа {ty} нельзя подставить в текст: оно не реализует Display"),
    )
}

pub fn lambda_needs_types(name: &str) -> Msg {
    Msg::new(
        format!("write the type of '{name}': no function type is expected here"),
        format!("запишите тип '{name}': здесь не ожидается тип функции"),
    )
}

pub fn no_membership(ty: &str) -> Msg {
    Msg::new(
        format!("'in' needs a contains method, which {ty} does not have"),
        format!("'in' требует метода contains, которого нет у {ty}"),
    )
}

pub fn no_field(field: &str, ty: &str) -> Msg {
    Msg::new(
        format!("no field '{field}' in {ty}"),
        format!("нет поля '{field}' у {ty}"),
    )
}

pub fn not_in_module(name: &str, module: &str) -> Msg {
    Msg::new(
        format!("module '{module}' has no '{name}'"),
        format!("в модуле '{module}' нет '{name}'"),
    )
}

pub fn no_member(name: &str, ty: &str) -> Msg {
    Msg::new(
        format!("'{ty}' has no variant or function '{name}'"),
        format!("у '{ty}' нет варианта или функции '{name}'"),
    )
}

pub fn no_method(name: &str, ty: &str) -> Msg {
    Msg::new(
        format!("no method '{name}' for {ty}"),
        format!("нет метода '{name}' у {ty}"),
    )
}

pub fn not_callable(ty: &str) -> Msg {
    Msg::new(
        format!("a value of type {ty} cannot be called"),
        format!("значение типа {ty} нельзя вызвать"),
    )
}
