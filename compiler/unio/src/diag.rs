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
    crate::catalog::explanation(code)
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
