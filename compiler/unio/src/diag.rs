//! Diagnostics: codes, bilingual messages, explanations, and the text and JSON formats
//! of requirements C-6 to C-10.

use std::fmt::Write as _;

/// A position in a source file. `offset` counts bytes; `line` and `column` start at 1 and
/// count lines and Unicode scalar values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Pos {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

/// A range of a source file, from `start` up to, but not including, `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn new(start: Pos, end: Pos) -> Span {
        Span { start, end }
    }

    /// From the start of `self` to the end of `other`.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}

/// The language of messages (C-10). Codes and the JSON schema do not depend on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    /// `en` or `ru`; anything else is `None`.
    pub fn parse(value: &str) -> Option<Lang> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" => Some(Lang::En),
            "ru" => Some(Lang::Ru),
            _ => None,
        }
    }
}

/// A text in both message languages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Msg {
    pub en: String,
    pub ru: String,
}

impl Msg {
    pub fn new(en: impl Into<String>, ru: impl Into<String>) -> Msg {
        Msg {
            en: en.into(),
            ru: ru.into(),
        }
    }

    pub fn get(&self, lang: Lang) -> &str {
        match lang {
            Lang::En => &self.en,
            Lang::Ru => &self.ru,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

impl Severity {
    fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Hint => "hint",
        }
    }
}

/// A suggested replacement of a range with new text (C-8 `fixes`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub span: Span,
    pub new_text: String,
    pub description: Msg,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: Msg,
    pub span: Span,
    pub label: Option<Msg>,
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    pub fn error(code: &'static str, span: Span, message: Msg) -> Diagnostic {
        Diagnostic {
            code,
            severity: Severity::Error,
            message,
            span,
            label: None,
            fixes: Vec::new(),
        }
    }

    pub fn with_label(mut self, label: Msg) -> Diagnostic {
        self.label = Some(label);
        self
    }

    pub fn with_fix(mut self, fix: Fix) -> Diagnostic {
        self.fixes.push(fix);
        self
    }

    /// The line of the start of the diagnostic, from 1.
    pub fn line(&self) -> usize {
        self.span.start.line
    }
}

/// The extended explanation of a code (C-8 `explanation`, C-9 `unio explain`).
pub fn explanation(code: &str) -> Msg {
    let (en, ru) = match code {
        "E0001" => (
            "Indentation consists of spaces only. Replace the tab with four spaces; `unio fmt` does this for a whole file.",
            "Отступ состоит только из пробелов. Замените табуляцию четырьмя пробелами; `unio fmt` делает это для всего файла.",
        ),
        "E0002" => (
            "Each block is indented by exactly four spaces more than the line that opens it.",
            "Каждый блок сдвинут ровно на четыре пробела относительно строки, которая его открывает.",
        ),
        "E0003" => (
            "Indentation may grow only after a line that ends with ':', and only by one level of four spaces. Add the missing ':' or remove the extra spaces.",
            "Отступ может расти только после строки, которая заканчивается на ':', и только на один уровень в четыре пробела. Добавьте пропущенное ':' или уберите лишние пробелы.",
        ),
        "E0005" => (
            "A text literal starts and ends with '\"' on the same line. Close the text or use \\n for a line end.",
            "Текстовый литерал начинается и заканчивается '\"' в одной строке. Закройте текст или используйте \\n для конца строки.",
        ),
        "E0006" => (
            "Valid escapes are \\n \\t \\r \\0 \\\\ \\\" \\{ \\} and \\u{hex} with one to six hexadecimal digits.",
            "Допустимые escape-последовательности: \\n \\t \\r \\0 \\\\ \\\" \\{ \\} и \\u{hex} с одной–шестью шестнадцатеричными цифрами.",
        ),
        "E0007" => (
            "Outside text and comments only ASCII letters, digits, '_', white space and the punctuation of the grammar may appear.",
            "Вне текста и комментариев допустимы только латинские буквы, цифры, '_', пробелы и знаки грамматики.",
        ),
        "E0008" => (
            "A number is decimal (1_000, 2.5, 1e-3), hexadecimal (0xFF) or binary (0b1010) and cannot be followed directly by a letter.",
            "Число бывает десятичным (1_000, 2.5, 1e-3), шестнадцатеричным (0xFF) или двоичным (0b1010) и не может сразу продолжаться буквой.",
        ),
        "E0009" => (
            "Every '(', '[' and '{' needs its closing bracket. Inside brackets line ends are ignored, so the rest of the file was read as one line.",
            "Каждой '(', '[' и '{' нужна закрывающая скобка. Внутри скобок концы строк игнорируются, поэтому остаток файла прочитан как одна строка.",
        ),
        "E0010" => (
            "A closing bracket must match the innermost open bracket.",
            "Закрывающая скобка должна соответствовать последней открытой.",
        ),
        "E0011" => (
            "An interpolation {expression} in text needs an expression and a closing '}' on the same line; a literal brace is written \\{ or \\}.",
            "Интерполяции {выражение} в тексте нужны выражение и закрывающая '}' в той же строке; сама фигурная скобка пишется \\{ или \\}.",
        ),
        "E0012" => (
            "The code does not follow the grammar (spec/grammar.ebnf). The message names what was expected at this place.",
            "Код не соответствует грамматике (spec/grammar.ebnf). Сообщение называет, что ожидалось в этом месте.",
        ),
        "E0013" => (
            "Expressions, texts and blocks may be nested only to a limited depth. Split the expression or move inner parts into functions or named values.",
            "Выражения, тексты и блоки можно вкладывать только на ограниченную глубину. Разбейте выражение или вынесите внутренние части в функции или именованные значения.",
        ),
        "E0020" => (
            "Comparisons do not chain. Write `a < b and b < c` instead of `a < b < c`.",
            "Сравнения не объединяются в цепочку. Пишите `a < b and b < c` вместо `a < b < c`.",
        ),
        "E0023" => (
            "Positional arguments come before named ones: f(1, name = 2).",
            "Позиционные аргументы идут перед именованными: f(1, name = 2).",
        ),
        "E0024" => (
            "An inline handler `try X on E as e:` is the last part of a statement, there is one per statement, and its block follows on the next lines.",
            "Строчный обработчик `try X on E as e:` — последняя часть инструкции, он один на инструкцию, и его блок следует на следующих строках.",
        ),
        "E0030" => (
            "Only a name, a field (a.b) or an element (a[i]) can be assigned to.",
            "Присвоить можно только имени, полю (a.b) или элементу (a[i]).",
        ),
        "E0032" => (
            "The word is reserved for a later release and cannot be used as a name.",
            "Слово зарезервировано для следующих выпусков и не может быть именем.",
        ),
        "E0034" => (
            "A list pattern has at most one rest element `name...`, and it is the last element.",
            "В образце списка не больше одного остаточного элемента `name...`, и он последний.",
        ),
        _ => ("No explanation is recorded for this code yet.", "Объяснение для этого кода пока не записано."),
    };
    Msg::new(en, ru)
}

/// The text form (C-7): code, path, line and column, the source line with an underline,
/// the message, and the explanation as help.
pub fn render_text(diags: &[Diagnostic], path: &str, source: &str, lang: Lang) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut out = String::new();
    for d in diags {
        let line = d.span.start.line;
        let column = d.span.start.column;
        let _ = writeln!(
            out,
            "{}[{}]: {}",
            d.severity.name(),
            d.code,
            d.message.get(lang)
        );
        let _ = writeln!(out, "  --> {path}:{line}:{column}");
        if let Some(text) = lines.get(line.saturating_sub(1)) {
            let text = text.trim_end_matches('\r');
            let number = line.to_string();
            let pad = " ".repeat(number.len());
            let width = if d.span.end.line == line {
                d.span.end.column.saturating_sub(column).max(1)
            } else {
                text.chars().count().saturating_sub(column - 1).max(1)
            };
            let _ = writeln!(out, " {pad} |");
            let _ = writeln!(out, " {number} | {text}");
            let label = d
                .label
                .as_ref()
                .map(|l| format!(" {}", l.get(lang)))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                " {pad} | {}{}{}",
                " ".repeat(column.saturating_sub(1)),
                "^".repeat(width),
                label
            );
        }
        let help = match lang {
            Lang::En => "help",
            Lang::Ru => "подсказка",
        };
        let _ = writeln!(out, "  = {help}: {}", explanation(d.code).get(lang));
        for fix in &d.fixes {
            let _ = writeln!(out, "  = fix: {}", fix.description.get(lang));
        }
        out.push('\n');
    }
    out
}

fn json_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn json_pos(out: &mut String, pos: Pos) {
    let _ = write!(
        out,
        "{{\"line\":{},\"column\":{},\"offset\":{}}}",
        pos.line, pos.column, pos.offset
    );
}

/// The JSON form (C-8): an array of objects with code, severity, message, explanation,
/// spans, fixes and related.
pub fn render_json(diags: &[Diagnostic], path: &str, lang: Lang) -> String {
    let mut out = String::from("[");
    for (i, d) in diags.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("\n  {\"code\":");
        json_string(&mut out, d.code);
        out.push_str(",\"severity\":");
        json_string(&mut out, d.severity.name());
        out.push_str(",\"message\":");
        json_string(&mut out, d.message.get(lang));
        out.push_str(",\"explanation\":");
        json_string(&mut out, explanation(d.code).get(lang));
        out.push_str(",\"spans\":[{\"file\":");
        json_string(&mut out, path);
        out.push_str(",\"start\":");
        json_pos(&mut out, d.span.start);
        out.push_str(",\"end\":");
        json_pos(&mut out, d.span.end);
        out.push_str(",\"label\":");
        match &d.label {
            Some(label) => json_string(&mut out, label.get(lang)),
            None => out.push_str("null"),
        }
        out.push_str("}],\"fixes\":[");
        for (k, fix) in d.fixes.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            out.push_str("{\"file\":");
            json_string(&mut out, path);
            out.push_str(",\"range\":{\"start\":");
            json_pos(&mut out, fix.span.start);
            out.push_str(",\"end\":");
            json_pos(&mut out, fix.span.end);
            out.push_str("},\"new_text\":");
            json_string(&mut out, &fix.new_text);
            out.push_str(",\"description\":");
            json_string(&mut out, fix.description.get(lang));
            out.push('}');
        }
        out.push_str("],\"related\":[]}");
    }
    if !diags.is_empty() {
        out.push('\n');
    }
    out.push_str("]\n");
    out
}
