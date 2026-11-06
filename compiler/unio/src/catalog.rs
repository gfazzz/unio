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
    "E0012", "E0013", "E0020", "E0023", "E0024", "E0030", "E0032", "E0034",
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
        _ => return None,
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
