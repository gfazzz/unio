//! The session model of `unio repl` (T-13, docs/language-contracts.md section 5).
//!
//! A session is a module of the package around the working directory, kept in memory under the
//! name [`SESSION_FILE`] at the package root: the declarations entered so far, then a function
//! whose body is the current input, after one `let` (or `var`) per binding kept from earlier
//! inputs. Every input is checked as that module, with the checks, the diagnostics, and the
//! manifest of `unio check`; only then is it run, in the sandbox of `unio run`.
//!
//! A binding is kept from one input to the next by its value, written as source ([`literal`]):
//! numbers, texts, truth values, lists, maps, sets, tuples, `Option` and `Result` of those,
//! and structs and enums declared in the session. A later input starts from those literals,
//! so earlier statements never run again and their effects are never repeated. A function
//! value, a resource, bytes, a path, and a value of a type declared elsewhere are not kept;
//! the session says so. A resource never outlives the `using` block of its input.

use std::path::Path;

use unio_runtime::value::Value;

use crate::diag::{Diagnostic, Lang, Pos, Severity, Span};

/// The name of the session module at the root of the package.
pub const SESSION_FILE: &str = "unio_repl_session.unio";
/// The function whose body is the current input.
pub const INPUT_FUNCTION: &str = "unio_repl_input";
/// The binding that receives the value of an expression input.
pub const VALUE: &str = "unio_repl_value";

/// A binding kept from earlier inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    pub var: bool,
    pub ty: String,
    pub literal: String,
}

/// How an input is placed in the session module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Declarations: functions, types, imports.
    Declarations,
    /// Statements: the body of the input function.
    Statements,
    /// One expression whose value is shown.
    Expression,
}

impl Mode {
    pub fn code(self) -> u32 {
        match self {
            Mode::Declarations => 0,
            Mode::Statements => 1,
            Mode::Expression => 2,
        }
    }

    pub fn from_code(code: u32) -> Option<Mode> {
        Some(match code {
            0 => Mode::Declarations,
            1 => Mode::Statements,
            2 => Mode::Expression,
            _ => return None,
        })
    }
}

/// The words an input of declarations starts with.
const DECLARATION_WORDS: &[&str] = &[
    "public",
    "function",
    "struct",
    "enum",
    "error",
    "interface",
    "extend",
    "import",
    "from",
    "external",
    "unsafe",
    "test",
];

/// The words a statement starts with; an input starting with one is not an expression.
const STATEMENT_WORDS: &[&str] = &[
    "let", "var", "if", "for", "while", "match", "try", "using", "return", "fail", "assert",
    "break", "continue", "pass",
];

fn first_word(input: &str) -> &str {
    let trimmed = input.trim_start();
    let end = trimmed
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(trimmed.len());
    &trimmed[..end]
}

/// Whether the single line has an assignment `=` outside brackets and texts.
fn assigns(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut depth = 0i32;
    let mut in_text = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_text {
            if c == b'\\' {
                i += 1;
            } else if c == b'"' {
                in_text = false;
            }
        } else {
            match c {
                b'"' => in_text = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'=' if depth == 0 => {
                    let before = if i > 0 { bytes[i - 1] } else { b' ' };
                    let after = bytes.get(i + 1).copied().unwrap_or(b' ');
                    if !matches!(before, b'=' | b'!' | b'<' | b'>') && after != b'=' {
                        return true;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    false
}

/// The mode an input is entered in, by its first word and its shape.
pub fn mode_of(input: &str) -> Mode {
    let word = first_word(input);
    if DECLARATION_WORDS.contains(&word) {
        return Mode::Declarations;
    }
    let single = input.trim_end().lines().count() == 1;
    if single
        && !STATEMENT_WORDS.contains(&word)
        && !assigns(input)
        && !input.trim_end().ends_with(':')
    {
        Mode::Expression
    } else {
        Mode::Statements
    }
}

/// The names an input binds at its top level with `let` or `var`.
pub fn top_level_names(input: &str) -> Vec<String> {
    input
        .lines()
        .filter_map(|line| {
            let rest = line
                .strip_prefix("let ")
                .or_else(|| line.strip_prefix("var "))?;
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// The text of the session module for an input, and where the input is in it.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub text: String,
    pub mode: Mode,
    /// The line of the first line of the input, from 1.
    pub first_line: usize,
    /// The columns added before each line of the input.
    pub indent: usize,
    /// The columns added before the first line only, besides `indent` (an expression).
    pub first_indent: usize,
    /// The line of each kept binding.
    pub binding_lines: Vec<(usize, String)>,
}

/// The session module for an input.
pub fn candidate(
    declarations: &[String],
    bindings: &[Binding],
    input: &str,
    mode: Mode,
) -> Candidate {
    let mut text = String::new();
    for d in declarations {
        text.push_str(d.trim_end());
        text.push_str("\n\n");
    }
    let mut binding_lines = Vec::new();
    if mode == Mode::Declarations {
        let first_line = text.lines().count() + 1;
        text.push_str(input.trim_end());
        text.push('\n');
        return Candidate {
            text,
            mode,
            first_line,
            indent: 0,
            first_indent: 0,
            binding_lines,
        };
    }
    text.push_str(&format!("function {INPUT_FUNCTION}():\n"));
    let redeclared = top_level_names(input);
    for b in bindings.iter().filter(|b| !redeclared.contains(&b.name)) {
        binding_lines.push((text.lines().count() + 1, b.name.clone()));
        let word = if b.var { "var" } else { "let" };
        text.push_str(&format!(
            "    {word} {} as {} = {}\n",
            b.name, b.ty, b.literal
        ));
    }
    let first_line = text.lines().count() + 1;
    let prefix = if mode == Mode::Expression {
        format!("let {VALUE} = ")
    } else {
        String::new()
    };
    for (i, line) in input.trim_end().lines().enumerate() {
        if line.trim().is_empty() {
            text.push('\n');
            continue;
        }
        text.push_str("    ");
        if i == 0 {
            text.push_str(&prefix);
        }
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("    pass\n");
    Candidate {
        text,
        mode,
        first_line,
        indent: 4,
        first_indent: prefix.chars().count(),
        binding_lines,
    }
}

/// A fix of a rejected input, in the coordinates of the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputFix {
    pub start: usize,
    pub end: usize,
    pub new_text: String,
    pub description: String,
}

/// The offset of a (line, column) of the input, both from 1; `None` outside it.
fn input_offset(input: &str, line: usize, column: usize) -> Option<usize> {
    let mut offset = 0;
    for (i, text) in input.split('\n').enumerate() {
        if i + 1 == line {
            let chars: Vec<(usize, char)> = text.char_indices().collect();
            let at = column.checked_sub(1)?;
            return Some(offset + chars.get(at).map_or(text.len(), |(o, _)| *o));
        }
        offset += text.len() + 1;
    }
    None
}

impl Candidate {
    /// The position of the input for a position of the candidate; `None` before the input.
    fn to_input(&self, input: &str, p: Pos) -> Option<Pos> {
        let line = p.line.checked_sub(self.first_line)? + 1;
        let shift = self.indent + if line == 1 { self.first_indent } else { 0 };
        let column = p.column.saturating_sub(shift).max(1);
        let offset = input_offset(input, line, column).unwrap_or(input.len());
        Some(Pos {
            offset,
            line,
            column,
        })
    }

    /// The diagnostics of the session module in the coordinates of the input, the fixes of
    /// the errors among them, and the bindings whose lines have errors.
    pub fn map(&self, input: &str, diagnostics: &[Diagnostic], lang: Lang) -> Mapped {
        let mut out = Vec::new();
        let mut fixes = Vec::new();
        let mut bad_bindings = Vec::new();
        let last_input_line = self.first_line + input.trim_end().lines().count();
        for d in diagnostics {
            if let Some((_, name)) = self
                .binding_lines
                .iter()
                .find(|(line, _)| *line == d.span.start.line)
            {
                if d.severity == Severity::Error && !bad_bindings.contains(name) {
                    bad_bindings.push(name.clone());
                }
                continue;
            }
            // The `pass` that ends the input function, or a line after the input.
            if d.span.start.line >= last_input_line && self.mode != Mode::Declarations {
                continue;
            }
            let (Some(start), Some(end)) = (
                self.to_input(input, d.span.start),
                self.to_input(input, d.span.end),
            ) else {
                continue;
            };
            let mut mapped = d.clone();
            mapped.span = Span::new(start, end);
            mapped.fixes.clear();
            for fix in &d.fixes {
                if let (Some(s), Some(e)) = (
                    self.to_input(input, fix.span.start),
                    self.to_input(input, fix.span.end),
                ) {
                    let mut f = fix.clone();
                    f.span = Span::new(s, e);
                    if d.severity == Severity::Error {
                        fixes.push(InputFix {
                            start: s.offset,
                            end: e.offset,
                            new_text: fix.new_text.clone(),
                            description: fix.description.get(lang).to_string(),
                        });
                    }
                    mapped.fixes.push(f);
                }
            }
            out.push(mapped);
        }
        Mapped {
            diagnostics: out,
            fixes,
            bad_bindings,
        }
    }
}

/// Diagnostics mapped to an input.
#[derive(Clone, Debug)]
pub struct Mapped {
    pub diagnostics: Vec<Diagnostic>,
    pub fixes: Vec<InputFix>,
    pub bad_bindings: Vec<String>,
}

/// Applies one fix to an input.
pub fn apply_fix(input: &str, fix: &InputFix) -> Option<String> {
    if fix.start > fix.end
        || fix.end > input.len()
        || !input.is_char_boundary(fix.start)
        || !input.is_char_boundary(fix.end)
    {
        return None;
    }
    let mut text = input.to_string();
    text.replace_range(fix.start..fix.end, &fix.new_text);
    Some(text)
}

// ---------------------------------------------------------------- checking an input

/// What checking an input as the session module gave.
pub struct Checked {
    pub candidate: Candidate,
    pub errors: bool,
    /// The diagnostics, in the coordinates of the input, as `unio check` writes them.
    pub rendered: String,
    pub fixes: Vec<InputFix>,
    /// Kept bindings whose literal no longer checks; they must be dropped.
    pub bad_bindings: Vec<String>,
    /// The names the input binds at its top level: (name, var, type).
    pub types: Vec<(String, bool, String)>,
    /// The type of the value of an expression input.
    pub value_type: Option<String>,
    /// The name of the session module, which prefixes the identities of its types.
    pub module_name: String,
}

/// Checks an input as the session module at `path` (absolute and canonical), with the checks
/// of `unio check` and the manifest of the package. Call it once per process, on a thread with
/// the stack of the front end: the module is given to the loader as an unsaved buffer.
pub fn check_input(
    path: &Path,
    declarations: &[String],
    bindings: &[Binding],
    input: &str,
    mode: Mode,
    lang: Lang,
) -> (Checked, crate::program::Program) {
    let c = candidate(declarations, bindings, input, mode);
    crate::vfs::overlay(vec![(path.to_path_buf(), c.text.clone())]);
    crate::lsp::facts::start();
    let program = crate::load_checked(&[path.to_path_buf()], crate::effects::Profile::Application);
    let facts = crate::lsp::facts::take().unwrap_or_default();
    let module = program.modules.iter().position(|m| m.path == path);
    let mut diagnostics = Vec::new();
    let mut types: Vec<(String, bool, String)> = Vec::new();
    let mut value_type = None;
    let mut module_name = String::new();
    if let Some(m) = module {
        module_name = program.modules[m].name.clone();
        diagnostics = program.modules[m].analysis.diagnostics.clone();
        for (owner, d) in &program.diagnostics {
            if *owner == m && !diagnostics.contains(d) {
                diagnostics.push(d.clone());
            }
        }
        diagnostics.sort_by_key(|d| (d.span.start.offset, d.span.end.offset));
        let lines: Vec<&str> = c.text.lines().collect();
        for local in facts.locals.iter().filter(|l| l.module == m && !l.param) {
            let line = local.span.start.line;
            if line < c.first_line {
                continue;
            }
            let text = lines.get(line - 1).copied().unwrap_or("");
            let top = local.span.start.column == 9
                && (text.starts_with("    let ") || text.starts_with("    var "));
            if !top {
                continue;
            }
            let shown = program.show(&local.ty);
            if local.name == VALUE {
                value_type = Some(shown);
                continue;
            }
            types.retain(|(n, _, _)| *n != local.name);
            types.push((local.name.clone(), text.starts_with("    var "), shown));
        }
    }
    let mapped = c.map(input, &diagnostics, lang);
    let mut rendered = crate::diag::render_text(&mapped.diagnostics, "input", input, lang);
    for manifest in &program.manifests {
        rendered.push_str(&manifest.render_text(lang));
    }
    let errors = module.is_none()
        || !program.manifests.is_empty()
        || !mapped.bad_bindings.is_empty()
        || mapped
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error);
    (
        Checked {
            candidate: c,
            errors,
            rendered,
            fixes: mapped.fixes,
            bad_bindings: mapped.bad_bindings,
            types,
            value_type,
            module_name,
        },
        program,
    )
}

// ---------------------------------------------------------------- values as source

fn text_literal(text: &str) -> Option<String> {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => return None,
            c => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}

fn all(items: impl Iterator<Item = Option<String>>) -> Option<Vec<String>> {
    items.collect()
}

/// The source of a value that gives the same value without effects, or `None` when the value
/// cannot be written so (a function, a resource, bytes, a path, a type declared outside the
/// session). `session` is the name of the session module, which prefixes its types.
pub fn literal(value: &Value, session: &str) -> Option<String> {
    Some(match value {
        Value::Int(i) if *i == i64::MIN => return None,
        Value::Int(i) => i.to_string(),
        Value::Float(x) if !x.is_finite() => return None,
        Value::Float(x) => unio_runtime::externs::float_text::float_to_text(*x),
        Value::Bool(b) => b.to_string(),
        Value::Text(t) => text_literal(t.as_str())?,
        Value::List(items) => format!(
            "[{}]",
            all(items.iter().map(|v| literal(v, session)))?.join(", ")
        ),
        Value::Tuple(items) => {
            let parts = all(items.iter().map(|v| literal(v, session)))?;
            if parts.len() == 1 {
                format!("({},)", parts[0])
            } else {
                format!("({})", parts.join(", "))
            }
        }
        Value::Set(set) => format!(
            "[{}].to_set()",
            all(set.items().map(|v| literal(v, session)))?.join(", ")
        ),
        Value::Map(map) => {
            let parts = all(map.entries().map(|(k, v)| {
                Some(format!(
                    "{}: {}",
                    literal(k, session)?,
                    literal(v, session)?
                ))
            }))?;
            format!("{{{}}}", parts.join(", "))
        }
        Value::Struct(s) => {
            let short = session_type(&s.type_name, session)?;
            let fields = all(s
                .fields
                .iter()
                .map(|(n, v)| Some(format!("{n} = {}", literal(v, session)?))))?;
            format!("{short}({})", fields.join(", "))
        }
        Value::Enum(e) => {
            let fields = all(e.fields.iter().map(|(_, v)| literal(v, session)))?;
            let head = match &*e.type_name {
                "Option" | "Result" => e.variant.to_string(),
                name => format!("{}.{}", session_type(name, session)?, e.variant),
            };
            if fields.is_empty() {
                head
            } else {
                format!("{head}({})", fields.join(", "))
            }
        }
        Value::Unit
        | Value::Bytes(_)
        | Value::Path(_)
        | Value::Function(_)
        | Value::Resource(_) => return None,
    })
}

/// The short name of a type declared in the session module.
fn session_type<'a>(identity: &'a str, session: &str) -> Option<&'a str> {
    let (module, name) = identity.rsplit_once('.')?;
    (module == session).then_some(name)
}

/// Whether a path names the session module.
pub fn is_session(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == SESSION_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_declarations_statements_or_expressions() {
        assert_eq!(mode_of("function f():\n    pass"), Mode::Declarations);
        assert_eq!(mode_of("import std.files"), Mode::Declarations);
        assert_eq!(mode_of("let x = 1"), Mode::Statements);
        assert_eq!(mode_of("x = x + 1"), Mode::Statements);
        assert_eq!(mode_of("x == 1"), Mode::Expression);
        assert_eq!(mode_of("[1, 2].length()"), Mode::Expression);
        assert_eq!(mode_of("print(\"a = b\")"), Mode::Expression);
        assert_eq!(
            mode_of("for i in range(0, 2):\n    print(\"{i}\")"),
            Mode::Statements
        );
    }

    #[test]
    fn a_candidate_keeps_bindings_and_maps_back_to_the_input() {
        let bindings = vec![Binding {
            name: "x".into(),
            var: true,
            ty: "Int".into(),
            literal: "1".into(),
        }];
        let c = candidate(
            &["function f() returns Int:\n    return 1".into()],
            &bindings,
            "x + f()",
            Mode::Expression,
        );
        assert!(c.text.contains("    var x as Int = 1\n"), "{}", c.text);
        assert!(
            c.text.contains(&format!("    let {VALUE} = x + f()\n")),
            "{}",
            c.text
        );
        let line = c.text.lines().position(|l| l.contains(VALUE)).unwrap() + 1;
        assert_eq!(c.first_line, line);
        let p = c
            .to_input(
                "x + f()",
                Pos {
                    offset: 0,
                    line,
                    column: 5 + VALUE.len() + 7 + 4,
                },
            )
            .unwrap();
        assert_eq!((p.line, p.column, p.offset), (1, 5, 4));
        // A name bound again by the input replaces the kept binding.
        let again = candidate(&[], &bindings, "let x = 2", Mode::Statements);
        assert!(!again.text.contains("var x as Int"));
    }

    #[test]
    fn values_are_written_as_source_when_they_can_be() {
        use std::rc::Rc;
        assert_eq!(literal(&Value::Int(-3), "s").as_deref(), Some("-3"));
        assert_eq!(literal(&Value::Bool(true), "s").as_deref(), Some("true"));
        assert_eq!(literal(&Value::Float(0.5), "s").as_deref(), Some("0.5"));
        assert_eq!(literal(&Value::Float(f64::NAN), "s"), None);
        let text = Value::Text(unio_runtime::value::Text::new("a\"{b}\n"));
        assert_eq!(literal(&text, "s").as_deref(), Some("\"a\\\"\\{b\\}\\n\""));
        let s = Value::Struct(Rc::new(unio_runtime::value::StructValue {
            type_name: "s.P".into(),
            fields: vec![("x".into(), Value::Int(1))],
        }));
        assert_eq!(literal(&s, "s").as_deref(), Some("P(x = 1)"));
        assert_eq!(literal(&s, "other"), None);
    }
}
