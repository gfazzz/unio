//! `unio fmt`: writes a source in the one style of docs/style.md (requirements T-1 to T-3).
//!
//! The tree gives the structure; the tokens of the lexer give the lines of keywords that the
//! tree does not keep (`else`, the end of a header); the comments of the lexer are placed by
//! their source lines. Expressions become a line of text pieces and bracketed groups, and a
//! line that does not fit the width breaks its outermost group that ends last (rule 4).

use std::collections::HashSet;

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{self, Comment, Kind, Token};

/// The line width of docs/style.md, rule 1.3.
pub const WIDTH: usize = 100;
const STEP: usize = 4;

/// Formats a source. A source with errors is not formatted; its diagnostics are returned.
pub fn format_source(source: &str) -> Result<String, Vec<Diagnostic>> {
    let source = source.to_string();
    crate::on_front_end_stack(move || {
        let analysis = crate::analyse("<fmt>", &source);
        if analysis.has_errors() {
            return Err(analysis.diagnostics);
        }
        let lexed = lexer::lex(&source);
        let mut formatter = Formatter::new(&source, &lexed.tokens, &analysis.comments);
        formatter.file(&analysis.file);
        Ok(formatter.finish())
    })
}

/// The tree with its imports in the order the formatter writes them (rule 2.2): every import
/// at the place of the first one, `import` lines before `from` lines, each sorted by module
/// path, with the names of a `from` line sorted. Used to check T-2.
pub fn normalize_imports(file: &File) -> File {
    let mut imports: Vec<Import> = Vec::new();
    let mut first = None;
    let mut rest = Vec::new();
    for item in &file.items {
        match item {
            Item::Import(import) => {
                if first.is_none() {
                    first = Some(rest.len());
                }
                imports.push(import.clone());
            }
            other => rest.push(other.clone()),
        }
    }
    for import in &mut imports {
        import.names.sort_by(|a, b| a.text.cmp(&b.text));
    }
    imports.sort_by_key(import_key);
    let mut items = rest;
    if let Some(at) = first {
        for (offset, import) in imports.into_iter().enumerate() {
            items.insert(at + offset, Item::Import(import));
        }
    }
    File {
        unsafe_module: file.unsafe_module,
        items,
    }
}

fn import_key(import: &Import) -> (bool, String, String) {
    let path: Vec<&str> = import.module.iter().map(|n| n.text.as_str()).collect();
    let alias = import
        .alias
        .as_ref()
        .map(|a| a.text.clone())
        .unwrap_or_default();
    (!import.names.is_empty(), path.join("."), alias)
}

// ---------------------------------------------------------------- pieces and line breaking

/// A line under construction: text, and bracketed groups that may be broken.
#[derive(Clone, Debug)]
pub(crate) enum Piece {
    Text(String),
    Group(Group),
}

#[derive(Clone, Debug)]
pub(crate) struct Group {
    open: &'static str,
    close: &'static str,
    items: Vec<Vec<Piece>>,
    /// A one-element tuple keeps its comma on one line: `(x,)`.
    single_comma: bool,
}

fn text(s: impl Into<String>) -> Piece {
    Piece::Text(s.into())
}

fn group(open: &'static str, items: Vec<Vec<Piece>>, close: &'static str) -> Piece {
    if items.is_empty() {
        return text(format!("{open}{close}"));
    }
    Piece::Group(Group {
        open,
        close,
        items,
        single_comma: false,
    })
}

pub(crate) fn flat(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(s) => out.push_str(s),
            Piece::Group(g) => {
                out.push_str(g.open);
                let items: Vec<String> = g.items.iter().map(|i| flat(i)).collect();
                out.push_str(&items.join(", "));
                if g.single_comma {
                    out.push(',');
                }
                out.push_str(g.close);
            }
        }
    }
    out
}

/// Rule 4: one line if it fits; otherwise break the outermost group that ends last, then
/// apply the rule to every new line.
fn render(pieces: &[Piece], indent: usize) -> Vec<String> {
    let line = flat(pieces);
    if indent + line.chars().count() <= WIDTH {
        return vec![format!("{}{line}", " ".repeat(indent))];
    }
    let Some(k) = pieces.iter().rposition(|p| matches!(p, Piece::Group(_))) else {
        return vec![format!("{}{line}", " ".repeat(indent))];
    };
    let Piece::Group(g) = &pieces[k] else {
        unreachable!()
    };
    let mut prefix = pieces[..k].to_vec();
    prefix.push(text(g.open));
    let mut lines = render(&prefix, indent);
    for item in &g.items {
        let mut item = item.clone();
        item.push(text(","));
        lines.extend(render(&item, indent + STEP));
    }
    let mut suffix = vec![text(g.close)];
    suffix.extend(pieces[k + 1..].iter().cloned());
    lines.extend(render(&suffix, indent));
    lines
}

// ---------------------------------------------------------------- printing the tree

/// Prints expressions, patterns, types and signatures. With a source, text literals are
/// written exactly as in it; without one (the outline of a tree), they are rebuilt.
pub(crate) struct Printer<'a> {
    pub(crate) source: Option<&'a str>,
}

impl Printer<'_> {
    fn raw(&self, span: Span) -> Option<String> {
        self.source
            .and_then(|s| s.get(span.start.offset..span.end.offset))
            .map(str::to_string)
    }

    fn text_literal(&self, parts: &[TextPart], span: Span) -> String {
        if let Some(raw) = self.raw(span) {
            return raw;
        }
        let mut out = String::from("\"");
        for part in parts {
            match part {
                TextPart::Literal(s) => {
                    for c in s.chars() {
                        match c {
                            '\\' => out.push_str("\\\\"),
                            '"' => out.push_str("\\\""),
                            '{' => out.push_str("\\{"),
                            '}' => out.push_str("\\}"),
                            '\n' => out.push_str("\\n"),
                            '\t' => out.push_str("\\t"),
                            '\r' => out.push_str("\\r"),
                            '\0' => out.push_str("\\0"),
                            c if c.is_control() => out.push_str(&format!("\\u{{{:X}}}", c as u32)),
                            c => out.push(c),
                        }
                    }
                }
                TextPart::Interpolation(e) => {
                    out.push('{');
                    out.push_str(&flat(&self.expr(e)));
                    out.push('}');
                }
            }
        }
        out.push('"');
        out
    }

    pub(crate) fn ty(&self, t: &Type) -> String {
        let mut out = names(&t.path);
        if !t.arguments.is_empty() {
            let args: Vec<String> = t
                .arguments
                .iter()
                .map(|a| match a {
                    TypeArgument::Type(t) => self.ty(t),
                    TypeArgument::Shape {
                        params,
                        returns,
                        fails,
                    } => {
                        let params: Vec<String> = params
                            .iter()
                            .map(|(label, t)| match label {
                                Some(l) => format!("named {} as {}", l.text, self.ty(t)),
                                None => self.ty(t),
                            })
                            .collect();
                        let mut s = format!("({})", params.join(", "));
                        if let Some(r) = returns {
                            s.push_str(&format!(" -> {}", self.ty(r)));
                        }
                        if !fails.is_empty() {
                            s.push_str(&format!(" fails with {}", self.types(fails)));
                        }
                        s
                    }
                })
                .collect();
            out.push_str(&format!("[{}]", args.join(", ")));
        }
        out
    }

    fn types(&self, types: &[Type]) -> String {
        types
            .iter()
            .map(|t| self.ty(t))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn generics(&self, generics: &[GenericParam]) -> String {
        if generics.is_empty() {
            return String::new();
        }
        let items: Vec<String> = generics
            .iter()
            .map(|g| {
                if g.bounds.is_empty() {
                    g.name.text.clone()
                } else {
                    let bounds: Vec<String> = g.bounds.iter().map(|b| self.ty(b)).collect();
                    format!("{} with {}", g.name.text, bounds.join(" and "))
                }
            })
            .collect();
        format!("[{}]", items.join(", "))
    }

    fn param(&self, p: &Param) -> Vec<Piece> {
        match p {
            Param::SelfParam { var, .. } => vec![text(if *var { "var self" } else { "self" })],
            Param::Named { name, ty, default } => {
                let mut out = vec![text(format!("named {} as {}", name.text, self.ty(ty)))];
                if let Some(d) = default {
                    out.push(text(" = "));
                    out.extend(self.expr(d));
                }
                out
            }
            Param::Plain {
                var,
                name,
                ty,
                default,
            } => {
                let var = if *var { "var " } else { "" };
                let mut out = vec![text(format!("{var}{} as {}", name.text, self.ty(ty)))];
                if let Some(d) = default {
                    out.push(text(" = "));
                    out.extend(self.expr(d));
                }
                out
            }
        }
    }

    /// `function name[G](params) returns T fails with E`
    pub(crate) fn signature(&self, sig: &Signature) -> Vec<Piece> {
        let mut out = vec![text(format!(
            "function {}{}",
            sig.name.text,
            self.generics(&sig.generics)
        ))];
        out.push(group(
            "(",
            sig.params.iter().map(|p| self.param(p)).collect(),
            ")",
        ));
        if let Some(r) = &sig.returns {
            out.push(text(format!(" returns {}", self.ty(r))));
        }
        if !sig.fails.is_empty() {
            out.push(text(format!(" fails with {}", self.types(&sig.fails))));
        }
        out
    }

    fn arguments(&self, arguments: &[Argument]) -> Piece {
        group(
            "(",
            arguments
                .iter()
                .map(|a| {
                    let mut out = Vec::new();
                    if let Some(n) = &a.name {
                        out.push(text(format!("{} = ", n.text)));
                    }
                    out.extend(self.expr(&a.value));
                    out
                })
                .collect(),
            ")",
        )
    }

    pub(crate) fn expr(&self, e: &Expr) -> Vec<Piece> {
        match &e.kind {
            ExprKind::Int(s) | ExprKind::Float(s) => vec![text(s.clone())],
            ExprKind::Text(parts) => vec![text(self.text_literal(parts, e.span))],
            ExprKind::Bool(b) => vec![text(if *b { "true" } else { "false" })],
            ExprKind::SelfValue => vec![text("self")],
            ExprKind::Name(n) => vec![text(n.clone())],
            ExprKind::Paren(inner) => {
                let mut out = vec![text("(")];
                out.extend(self.expr(inner));
                out.push(text(")"));
                out
            }
            ExprKind::Tuple(items) => {
                let items: Vec<Vec<Piece>> = items.iter().map(|i| self.expr(i)).collect();
                let single = items.len() == 1;
                vec![Piece::Group(Group {
                    open: "(",
                    close: ")",
                    items,
                    single_comma: single,
                })]
            }
            ExprKind::List(items) => {
                vec![group(
                    "[",
                    items.iter().map(|i| self.expr(i)).collect(),
                    "]",
                )]
            }
            ExprKind::Comprehension {
                element,
                names: bound,
                iterable,
                condition,
            } => {
                let mut out = vec![text("[")];
                out.extend(self.expr(element));
                let bound: Vec<&str> = bound.iter().map(|n| n.text.as_str()).collect();
                out.push(text(format!(" for {} in ", bound.join(", "))));
                out.extend(self.expr(iterable));
                if let Some(c) = condition {
                    out.push(text(" if "));
                    out.extend(self.expr(c));
                }
                out.push(text("]"));
                out
            }
            ExprKind::Map(entries) => vec![group(
                "{",
                entries
                    .iter()
                    .map(|(k, v)| {
                        let mut out = self.expr(k);
                        out.push(text(": "));
                        out.extend(self.expr(v));
                        out
                    })
                    .collect(),
                "}",
            )],
            ExprKind::Lambda {
                params,
                returns,
                fails,
                body,
            } => {
                let mut out = vec![text("function ")];
                out.push(group(
                    "(",
                    params
                        .iter()
                        .map(|p| {
                            let named = if p.named { "named " } else { "" };
                            let ty =
                                p.ty.as_ref()
                                    .map(|t| format!(" as {}", self.ty(t)))
                                    .unwrap_or_default();
                            vec![text(format!("{named}{}{ty}", p.name.text))]
                        })
                        .collect(),
                    ")",
                ));
                if let Some(r) = returns {
                    out.push(text(format!(" returns {}", self.ty(r))));
                }
                if !fails.is_empty() {
                    out.push(text(format!(" fails with {}", self.types(fails))));
                }
                out.push(text(": "));
                out.extend(self.expr(body));
                out
            }
            ExprKind::Is { value, pattern } => {
                let mut out = self.expr(value);
                out.push(text(format!(" is {}", self.pattern(pattern))));
                out
            }
            ExprKind::Conditional {
                then,
                condition,
                otherwise,
            } => {
                let mut out = self.expr(then);
                out.push(text(" if "));
                out.extend(self.expr(condition));
                out.push(text(" else "));
                out.extend(self.expr(otherwise));
                out
            }
            ExprKind::Binary { op, left, right } => {
                let mut out = self.expr(left);
                out.push(text(format!(" {op} ")));
                out.extend(self.expr(right));
                out
            }
            ExprKind::Unary { op, operand } => {
                let mut out = vec![text(if *op == "-" {
                    "-".to_string()
                } else {
                    format!("{op} ")
                })];
                out.extend(self.expr(operand));
                out
            }
            ExprKind::Try { operand, handler } => {
                let mut out = vec![text("try ")];
                out.extend(self.expr(operand));
                if let Some((ty, name)) = handler {
                    out.push(text(format!(" on {}", self.ty(ty))));
                    if let Some(n) = name {
                        out.push(text(format!(" as {}", n.text)));
                    }
                }
                out
            }
            ExprKind::Call { callee, arguments } => {
                let mut out = self.expr(callee);
                out.push(self.arguments(arguments));
                out
            }
            ExprKind::GenericCall {
                callee,
                types,
                arguments,
            } => {
                let mut out = self.expr(callee);
                out.push(text(format!("[{}]", self.types(types))));
                out.push(self.arguments(arguments));
                out
            }
            ExprKind::Index { target, index } => {
                let mut out = self.expr(target);
                out.push(text("["));
                out.extend(self.expr(index));
                out.push(text("]"));
                out
            }
            ExprKind::Field { target, name } => {
                let mut out = self.expr(target);
                out.push(text(format!(".{}", name.text)));
                out
            }
            ExprKind::SafeField { target, name } => {
                let mut out = self.expr(target);
                out.push(text(format!("?.{}", name.text)));
                out
            }
            ExprKind::Error => vec![],
        }
    }

    pub(crate) fn pattern(&self, p: &Pattern) -> String {
        match &p.kind {
            PatternKind::Wildcard => "_".to_string(),
            PatternKind::Literal(literal) => self.raw(p.span).unwrap_or_else(|| match literal {
                Literal::Int(s) => s.clone(),
                Literal::Text(parts) => self.text_literal(parts, p.span),
                Literal::Bool(b) => b.to_string(),
            }),
            PatternKind::Name(path) => names(path),
            PatternKind::Constructor { name, arguments } => {
                let args: Vec<String> = arguments
                    .iter()
                    .map(|(label, p)| match label {
                        Some(l) => format!("{} = {}", l.text, self.pattern(p)),
                        None => self.pattern(p),
                    })
                    .collect();
                format!("{}({})", names(name), args.join(", "))
            }
            PatternKind::List(elements) => {
                let items: Vec<String> = elements
                    .iter()
                    .map(|e| match e {
                        ListElement::Rest(n) => format!("{}...", n.text),
                        ListElement::Pattern(p) => self.pattern(p),
                    })
                    .collect();
                format!("[{}]", items.join(", "))
            }
            PatternKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|i| self.pattern(i)).collect();
                if items.len() == 1 {
                    format!("({},)", items[0])
                } else {
                    format!("({})", items.join(", "))
                }
            }
            PatternKind::Bind { pattern, name } => {
                format!("{} as {}", self.pattern(pattern), name.text)
            }
            PatternKind::Or(alternatives) => alternatives
                .iter()
                .map(|a| self.pattern(a))
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }

    pub(crate) fn simple(&self, s: &Simple) -> Vec<Piece> {
        match s {
            Simple::Let {
                var,
                name,
                ty,
                value,
            } => {
                let ty = ty
                    .as_ref()
                    .map(|t| format!(" as {}", self.ty(t)))
                    .unwrap_or_default();
                let keyword = if *var { "var" } else { "let" };
                let mut out = vec![text(format!("{keyword} {}{ty} = ", name.text))];
                out.extend(self.expr(value));
                out
            }
            Simple::Assign { target, value } => {
                let mut out = self.expr(target);
                out.push(text(" = "));
                out.extend(self.expr(value));
                out
            }
            Simple::Return(value) => {
                let mut out = vec![text("return")];
                if let Some(v) = value {
                    out.push(text(" "));
                    out.extend(self.expr(v));
                }
                out
            }
            Simple::Fail(v) => prefixed("fail ", self.expr(v)),
            Simple::Assert(v) => prefixed("assert ", self.expr(v)),
            Simple::Break => vec![text("break")],
            Simple::Continue => vec![text("continue")],
            Simple::Pass => vec![text("pass")],
            Simple::Expr(e) => self.expr(e),
        }
    }

    pub(crate) fn field(&self, f: &Field) -> Vec<Piece> {
        let public = if f.public { "public " } else { "" };
        let var = if f.var { "var " } else { "" };
        let mut out = vec![text(format!(
            "{public}{var}{} as {}",
            f.name.text,
            self.ty(&f.ty)
        ))];
        if let Some(d) = &f.default {
            out.push(text(" = "));
            out.extend(self.expr(d));
        }
        out
    }

    pub(crate) fn variant(&self, v: &Variant) -> String {
        if v.fields.is_empty() {
            return v.name.text.clone();
        }
        let fields: Vec<String> = v
            .fields
            .iter()
            .map(|(n, t)| format!("{} as {}", n.text, self.ty(t)))
            .collect();
        format!("{}({})", v.name.text, fields.join(", "))
    }

    pub(crate) fn constant(&self, c: &Constant) -> Vec<Piece> {
        let ty =
            c.ty.as_ref()
                .map(|t| format!(" as {}", self.ty(t)))
                .unwrap_or_default();
        let mut out = vec![text(format!("let {}{ty} = ", c.name.text))];
        out.extend(self.expr(&c.value));
        out
    }

    pub(crate) fn extend_header(&self, e: &Extend) -> String {
        let mut out = format!(
            "extend{} {}",
            self.generics(&e.generics),
            self.ty(&e.target)
        );
        if let Some(i) = &e.interface {
            out.push_str(&format!(" with {}", self.ty(i)));
        }
        out
    }

    pub(crate) fn import(&self, import: &Import) -> String {
        let module = names(&import.module);
        if import.names.is_empty() {
            match &import.alias {
                Some(a) => format!("import {module} as {}", a.text),
                None => format!("import {module}"),
            }
        } else {
            let mut imported: Vec<&str> = import.names.iter().map(|n| n.text.as_str()).collect();
            imported.sort_unstable();
            format!("from {module} import {}", imported.join(", "))
        }
    }
}

fn prefixed(prefix: &str, mut rest: Vec<Piece>) -> Vec<Piece> {
    rest.insert(0, text(prefix));
    rest
}

fn names(path: &[Name]) -> String {
    path.iter()
        .map(|n| n.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub(crate) fn modifiers(public: bool, external: bool) -> String {
    let mut out = String::new();
    if public {
        out.push_str("public ");
    }
    if external {
        out.push_str("external ");
    }
    out
}

// ---------------------------------------------------------------- the file and its comments

/// How many blank lines come before the next thing written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Blank {
    /// The first thing of a block or of the file: none.
    None,
    /// As in the source, at most one.
    Preserve,
    /// Exactly one.
    One,
}

struct Note {
    line: usize,
    column: usize,
    text: String,
    /// Code stands before the comment on its line.
    trailing: bool,
}

struct CodeToken {
    offset: usize,
    line: usize,
    column: usize,
}

struct Formatter<'a> {
    printer: Printer<'a>,
    tokens: Vec<CodeToken>,
    /// Lines whose first code token is `else`.
    else_lines: Vec<usize>,
    notes: Vec<Note>,
    used: Vec<bool>,
    next: usize,
    out: Vec<String>,
    /// The source line of the last thing written.
    last_line: usize,
}

impl<'a> Formatter<'a> {
    fn new(source: &'a str, tokens: &[Token], comments: &[Comment]) -> Formatter<'a> {
        let code: Vec<CodeToken> = tokens
            .iter()
            .filter(|t| {
                !matches!(
                    t.kind,
                    Kind::Newline | Kind::Indent | Kind::Dedent | Kind::Eof
                )
            })
            .map(|t| CodeToken {
                offset: t.span.start.offset,
                line: t.span.start.line,
                column: t.span.start.column,
            })
            .collect();
        let mut first_on_line: HashSet<usize> = HashSet::new();
        let mut else_lines = Vec::new();
        for t in tokens {
            if matches!(
                t.kind,
                Kind::Newline | Kind::Indent | Kind::Dedent | Kind::Eof
            ) {
                continue;
            }
            if first_on_line.insert(t.span.start.line) && t.kind == Kind::Keyword("else") {
                else_lines.push(t.span.start.line);
            }
        }
        let notes = comments
            .iter()
            .map(|c| Note {
                line: c.span.start.line,
                column: c.span.start.column,
                text: c.text.trim_end().to_string(),
                trailing: code
                    .iter()
                    .any(|t| t.line == c.span.start.line && t.column < c.span.start.column),
            })
            .collect::<Vec<_>>();
        let used = vec![false; notes.len()];
        Formatter {
            printer: Printer {
                source: Some(source),
            },
            tokens: code,
            else_lines,
            notes,
            used,
            next: 0,
            out: Vec::new(),
            last_line: 0,
        }
    }

    fn finish(mut self) -> String {
        while let Some(i) = self.peek_note() {
            self.emit_note(i, 0, Blank::Preserve);
        }
        let mut lines: Vec<String> = self.out.iter().map(|l| l.trim_end().to_string()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }

    // ------------------------------------------------------------ lines of the source

    /// The line of the last code token inside the span.
    fn last_code_line(&self, span: Span) -> usize {
        let end = self.tokens.partition_point(|t| t.offset < span.end.offset);
        if end == 0 {
            return span.start.line;
        }
        let t = &self.tokens[end - 1];
        if t.offset < span.start.offset {
            span.start.line
        } else {
            t.line
        }
    }

    /// The last line of a header that starts on `first` and is followed by code on
    /// `child`: the last line with code before `child`. A header on the line of its child
    /// has no line of its own, which is returned as `first - 1`.
    fn header_last(&self, first: usize, child: usize) -> usize {
        if child <= first {
            return first - 1;
        }
        let end = self.tokens.partition_point(|t| t.line < child);
        match end.checked_sub(1).map(|i| &self.tokens[i]) {
            Some(t) if t.line >= first => t.line,
            _ => first,
        }
    }

    fn line_start_column(&self, line: usize) -> usize {
        let i = self.tokens.partition_point(|t| t.line < line);
        self.tokens
            .get(i)
            .filter(|t| t.line == line)
            .map_or(1, |t| t.column)
    }

    fn else_line(&self, after: usize, until: usize) -> usize {
        self.else_lines
            .iter()
            .copied()
            .find(|l| *l > after && *l <= until)
            .unwrap_or(until)
    }

    // ------------------------------------------------------------ comments and blank lines

    fn peek_note(&mut self) -> Option<usize> {
        while self.next < self.notes.len() && self.used[self.next] {
            self.next += 1;
        }
        (self.next < self.notes.len()).then_some(self.next)
    }

    fn blank(&mut self, line: usize, blank: Blank) {
        let wanted = match blank {
            Blank::None => false,
            Blank::One => true,
            Blank::Preserve => self.last_line > 0 && line > self.last_line + 1,
        };
        if wanted && self.out.last().is_some_and(|l| !l.is_empty()) {
            self.out.push(String::new());
        }
    }

    fn emit_note(&mut self, i: usize, indent: usize, blank: Blank) {
        let line = self.notes[i].line;
        self.blank(line, blank);
        self.out
            .push(format!("{}{}", " ".repeat(indent), self.notes[i].text));
        self.used[i] = true;
        self.last_line = line;
    }

    /// Writes the comments before `line` and returns the blank rule for what follows them.
    fn leading(&mut self, line: usize, indent: usize, blank: Blank) -> Blank {
        let mut blank = blank;
        while let Some(i) = self.peek_note() {
            if self.notes[i].line >= line {
                break;
            }
            self.emit_note(i, indent, blank);
            blank = Blank::Preserve;
        }
        blank
    }

    /// The comments from the cursor up to and including `last`, not yet written.
    fn notes_upto(&mut self, last: usize) -> Vec<usize> {
        let mut found = Vec::new();
        let mut i = self.next;
        while i < self.notes.len() && self.notes[i].line <= last {
            if !self.used[i] {
                found.push(i);
            }
            i += 1;
        }
        found
    }

    /// Writes a construct that starts on source line `first` and ends on `last`: the
    /// comments before it, the construct itself, and a comment at the end of its last line.
    fn construct(
        &mut self,
        level: usize,
        pieces: Vec<Piece>,
        first: usize,
        last: usize,
        blank: Blank,
    ) {
        let indent = level * STEP;
        let mut blank = self.leading(first, indent, blank);
        let mut inner = self.notes_upto(last);
        let trailing = match inner.last() {
            Some(&i) if self.notes[i].trailing && self.notes[i].line == last => inner.pop(),
            _ => None,
        };
        for i in inner {
            self.emit_note(i, indent, blank);
            blank = Blank::None;
        }
        self.blank(first, blank);
        let mut lines = render(&pieces, indent);
        if let Some(i) = trailing {
            if let Some(line) = lines.last_mut() {
                line.push_str("  ");
                line.push_str(&self.notes[i].text);
            }
            self.used[i] = true;
        }
        self.out.extend(lines);
        self.last_line = self.last_line.max(first).max(last);
    }

    /// Comments at the end of a block, indented at least as deep as the block.
    fn end_of_block(&mut self, level: usize, limit: usize, column: usize) {
        while let Some(i) = self.peek_note() {
            if self.notes[i].line >= limit || self.notes[i].column < column {
                break;
            }
            self.emit_note(i, level * STEP, Blank::Preserve);
        }
    }

    // ------------------------------------------------------------ statements

    fn block(&mut self, statements: &[Statement], level: usize, limit: usize) {
        for (k, s) in statements.iter().enumerate() {
            let blank = if k == 0 { Blank::None } else { Blank::Preserve };
            let child_limit = statements.get(k + 1).map_or(limit, |n| n.span.start.line);
            self.statement(s, level, child_limit, blank);
        }
        if let Some(first) = statements.first() {
            self.end_of_block(level, limit, first.span.start.column);
        }
    }

    fn header(
        &mut self,
        level: usize,
        pieces: Vec<Piece>,
        first: usize,
        child: usize,
        blank: Blank,
    ) {
        let last = self.header_last(first, child);
        self.construct(level, pieces, first, last, blank);
        self.last_line = self.last_line.max(first);
    }

    /// A branch of if, else, case or on (T-1a): one simple statement on the header line
    /// when it fits and no comment would have to move, a block otherwise.
    fn branch(
        &mut self,
        level: usize,
        mut pieces: Vec<Piece>,
        body: &[Statement],
        first: usize,
        limit: usize,
        blank: Blank,
    ) {
        if let [only] = body {
            if let StatementKind::Simple {
                statement,
                handler: None,
            } = &only.kind
            {
                let last = self.last_code_line(only.span);
                let notes = self.notes_upto(last);
                let movable = notes
                    .iter()
                    .all(|&i| self.notes[i].trailing && self.notes[i].line >= first)
                    && notes.len() <= 1;
                let simple = self.printer.simple(statement);
                let line = format!("{} {}", flat(&pieces), flat(&simple));
                if movable && level * STEP + line.chars().count() <= WIDTH {
                    pieces.push(text(" "));
                    pieces.extend(simple);
                    self.construct(level, pieces, first, last, blank);
                    return;
                }
            }
        }
        let child = body.first().map_or(first, |s| s.span.start.line);
        self.header(level, pieces, first, child, blank);
        self.block(body, level + 1, limit);
    }

    fn statement(&mut self, s: &Statement, level: usize, limit: usize, blank: Blank) {
        let first = s.span.start.line;
        let p = &self.printer;
        match &s.kind {
            StatementKind::Simple {
                statement,
                handler: None,
            } => {
                let pieces = p.simple(statement);
                let last = self.last_code_line(s.span);
                self.construct(level, pieces, first, last, blank);
            }
            StatementKind::Simple {
                statement,
                handler: Some(body),
            } => {
                let mut pieces = p.simple(statement);
                pieces.push(text(":"));
                let child = body[0].span.start.line;
                self.header(level, pieces, first, child, blank);
                self.block(body, level + 1, limit);
            }
            StatementKind::If {
                branches,
                otherwise,
            } => {
                let mut previous_end = 0;
                for (k, (condition, body)) in branches.iter().enumerate() {
                    let start = if k == 0 {
                        first
                    } else {
                        condition.span.start.line
                    };
                    let next_start = match branches.get(k + 1) {
                        Some((c, _)) => c.span.start.line,
                        None => match otherwise {
                            Some(o) => self.else_line(
                                body.last().map_or(start, |s| self.last_code_line(s.span)),
                                o[0].span.start.line,
                            ),
                            None => limit,
                        },
                    };
                    let keyword = if k == 0 { "if " } else { "else if " };
                    let mut pieces = vec![text(keyword)];
                    pieces.extend(self.printer.expr(condition));
                    pieces.push(text(":"));
                    let b = if k == 0 { blank } else { Blank::None };
                    self.branch(level, pieces, body, start, next_start, b);
                    previous_end = body.last().map_or(start, |s| self.last_code_line(s.span));
                }
                if let Some(body) = otherwise {
                    let start = self.else_line(previous_end, body[0].span.start.line);
                    self.branch(level, vec![text("else:")], body, start, limit, Blank::None);
                }
            }
            StatementKind::For {
                names: bound,
                iterable,
                body,
            } => {
                let bound: Vec<&str> = bound.iter().map(|n| n.text.as_str()).collect();
                let mut pieces = vec![text(format!("for {} in ", bound.join(", ")))];
                pieces.extend(p.expr(iterable));
                pieces.push(text(":"));
                self.header(level, pieces, first, body[0].span.start.line, blank);
                self.block(body, level + 1, limit);
            }
            StatementKind::While { condition, body } => {
                let mut pieces = vec![text("while ")];
                pieces.extend(p.expr(condition));
                pieces.push(text(":"));
                self.header(level, pieces, first, body[0].span.start.line, blank);
                self.block(body, level + 1, limit);
            }
            StatementKind::Using { name, value, body } => {
                let mut pieces = vec![text(format!("using {} = ", name.text))];
                pieces.extend(p.expr(value));
                pieces.push(text(":"));
                self.header(level, pieces, first, body[0].span.start.line, blank);
                self.block(body, level + 1, limit);
            }
            StatementKind::Match { subject, cases } => {
                let mut pieces = vec![text("match ")];
                pieces.extend(p.expr(subject));
                pieces.push(text(":"));
                let child = cases.first().map_or(first, |c| c.pattern.span.start.line);
                self.header(level, pieces, first, child, blank);
                for (k, case) in cases.iter().enumerate() {
                    let start = case.pattern.span.start.line;
                    let case_limit = cases
                        .get(k + 1)
                        .map_or(limit, |c| c.pattern.span.start.line);
                    let mut pieces = vec![text(format!(
                        "case {}",
                        self.printer.pattern(&case.pattern)
                    ))];
                    if let Some(g) = &case.guard {
                        pieces.push(text(" if "));
                        pieces.extend(self.printer.expr(g));
                    }
                    pieces.push(text(":"));
                    let b = if k == 0 { Blank::None } else { Blank::Preserve };
                    self.branch(level + 1, pieces, &case.body, start, case_limit, b);
                }
                if let Some(case) = cases.first() {
                    let column = self.line_start_column(case.pattern.span.start.line);
                    self.end_of_block(level + 1, limit, column);
                }
            }
            StatementKind::Try { body, handlers } => {
                let child = body[0].span.start.line;
                self.header(level, vec![text("try:")], first, child, blank);
                let body_limit = handlers.first().map_or(limit, |h| h.ty.span.start.line);
                self.block(body, level + 1, body_limit);
                for (k, h) in handlers.iter().enumerate() {
                    let start = h.ty.span.start.line;
                    let h_limit = handlers.get(k + 1).map_or(limit, |n| n.ty.span.start.line);
                    let mut header = format!("on {}", self.printer.ty(&h.ty));
                    if let Some(n) = &h.name {
                        header.push_str(&format!(" as {}", n.text));
                    }
                    header.push(':');
                    self.branch(
                        level,
                        vec![text(header)],
                        &h.body,
                        start,
                        h_limit,
                        Blank::None,
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------ declarations

    fn file(&mut self, file: &File) {
        let mut blank = Blank::None;
        if file.unsafe_module {
            if let Some(t) = self.tokens.first() {
                let line = t.line;
                self.construct(0, vec![text("unsafe module")], line, line, Blank::None);
                blank = Blank::One;
            }
        }
        let attached = self.attach_import_notes(file);
        let mut imports_written = false;
        let mut previous_constant = false;
        let starts: Vec<usize> = file.items.iter().map(item_line).collect();
        for (k, item) in file.items.iter().enumerate() {
            let limit = starts.get(k + 1).copied().unwrap_or(usize::MAX);
            if let Item::Import(_) = item {
                if !imports_written {
                    self.imports(file, &attached, starts[k], blank);
                    imports_written = true;
                    blank = Blank::One;
                    previous_constant = false;
                }
                continue;
            }
            let constant = matches!(
                item,
                Item::Declaration(Declaration {
                    kind: DeclarationKind::Constant(_),
                    ..
                })
            );
            let b = if blank == Blank::None {
                Blank::None
            } else if constant && previous_constant {
                Blank::Preserve
            } else {
                Blank::One
            };
            self.item(item, limit, b);
            previous_constant = constant;
            blank = Blank::One;
        }
    }

    /// The comments that travel with each import when imports are sorted: those between the
    /// previous item and the import, and one at the end of its line. Before the first item,
    /// only comments directly above it travel; the others belong to the module header.
    fn attach_import_notes(&mut self, file: &File) -> Vec<Vec<usize>> {
        let mut attached = Vec::new();
        let mut previous_end = 0;
        for (k, item) in file.items.iter().enumerate() {
            let start = item_line(item);
            let end = self.last_code_line(item_span(item));
            if let Item::Import(_) = item {
                let mut mine: Vec<usize> = (0..self.notes.len())
                    .filter(|&i| {
                        let n = &self.notes[i];
                        n.line > previous_end && (n.line < start || (n.line == start && n.trailing))
                    })
                    .collect();
                if k == 0 {
                    // Keep only the run of comments directly above the import.
                    let mut keep = Vec::new();
                    let mut expected = start;
                    for &i in mine.iter().rev() {
                        let n = &self.notes[i];
                        if n.line == start || n.line + 1 == expected {
                            keep.push(i);
                            if n.line != start {
                                expected = n.line;
                            }
                        } else {
                            break;
                        }
                    }
                    keep.reverse();
                    mine = keep;
                }
                for &i in &mine {
                    self.used[i] = true;
                }
                attached.push(mine);
            }
            previous_end = end;
        }
        attached
    }

    fn imports(&mut self, file: &File, attached: &[Vec<usize>], first_line: usize, blank: Blank) {
        let mut blank = self.leading(first_line, 0, blank);
        let mut entries: Vec<(&Import, &Vec<usize>)> = Vec::new();
        let mut a = 0;
        for item in &file.items {
            if let Item::Import(import) = item {
                entries.push((import, &attached[a]));
                a += 1;
            }
        }
        entries.sort_by_key(|(import, _)| import_key(import));
        let mut last = 0;
        // The group as a whole stands where the first import of the source stood.
        let mut check_line = first_line;
        for (import, notes) in entries {
            let mut line = self.printer.import(import);
            for &i in notes.iter() {
                let (trailing, text) = {
                    let n = &self.notes[i];
                    (
                        n.trailing && n.line == import.span.start.line,
                        n.text.clone(),
                    )
                };
                if trailing {
                    line.push_str("  ");
                    line.push_str(&text);
                } else {
                    self.blank(check_line, blank);
                    blank = Blank::None;
                    self.out.push(text);
                }
            }
            self.blank(check_line, blank);
            blank = Blank::None;
            check_line = 0;
            self.out.push(line);
            last = last.max(self.last_code_line(import.span));
        }
        self.last_line = self.last_line.max(last);
    }

    fn item(&mut self, item: &Item, limit: usize, blank: Blank) {
        let p = &self.printer;
        match item {
            Item::Import(_) => {}
            Item::Test(t) => {
                let description = self.test_description(t);
                let first = t.span.start.line;
                self.header(
                    0,
                    vec![text(format!("test {description}:"))],
                    first,
                    t.body[0].span.start.line,
                    blank,
                );
                self.block(&t.body, 1, limit);
            }
            Item::Extend(e) => {
                let header = format!("{}:", p.extend_header(e));
                let first = e.span.start.line;
                let child = e
                    .members
                    .first()
                    .map_or(first, |m| m.signature.span.start.line);
                self.header(0, vec![text(header)], first, child, blank);
                self.members(&e.members, limit);
            }
            Item::Declaration(d) => self.declaration(d, limit, blank),
        }
    }

    fn test_description(&self, t: &Test) -> String {
        let source = self.printer.source.unwrap_or_default();
        let after = t.span.start.offset;
        let rest = &source[after..];
        let quote = rest.find('"').unwrap_or(0);
        // The description is a text without interpolation braces of its own on one line.
        let tokens = lexer::lex(&rest[quote..]);
        tokens
            .tokens
            .first()
            .filter(|t| t.kind == Kind::Text)
            .map(|t| rest[quote + t.span.start.offset..quote + t.span.end.offset].to_string())
            .unwrap_or_else(|| self.printer.text_literal(&t.description, Span::default()))
    }

    fn members(&mut self, members: &[Member], limit: usize) {
        for (k, m) in members.iter().enumerate() {
            let first = m.signature.span.start.line;
            let m_limit = members
                .get(k + 1)
                .map_or(limit, |n| n.signature.span.start.line);
            let blank = if k == 0 { Blank::None } else { Blank::One };
            let mut pieces = vec![text(modifiers(m.public, m.external))];
            pieces.extend(self.printer.signature(&m.signature));
            match &m.body {
                Some(body) => {
                    pieces.push(text(":"));
                    self.header(1, pieces, first, body[0].span.start.line, blank);
                    self.block(body, 2, m_limit);
                }
                None => {
                    let last = self.last_code_line(m.signature.span);
                    self.construct(1, pieces, first, last, blank);
                }
            }
        }
        if let Some(m) = members.first() {
            let column = self.line_start_column(m.signature.span.start.line);
            self.end_of_block(1, limit, column);
        }
    }

    fn fields(&mut self, fields: &[Field], limit: usize) {
        for (k, f) in fields.iter().enumerate() {
            let first = f.name.span.start.line;
            let end = f.default.as_ref().map_or(f.ty.span.end, |d| d.span.end);
            let last = self.last_code_line(Span::new(f.name.span.start, end));
            let blank = if k == 0 { Blank::None } else { Blank::Preserve };
            let pieces = self.printer.field(f);
            self.construct(1, pieces, first, last, blank);
        }
        if let Some(f) = fields.first() {
            let column = self.line_start_column(f.name.span.start.line);
            self.end_of_block(1, limit, column);
        }
    }

    fn declaration(&mut self, d: &Declaration, limit: usize, blank: Blank) {
        let first = d.span.start.line;
        let public = if d.public { "public " } else { "" };
        match &d.kind {
            DeclarationKind::Function(f) => {
                let mut pieces = vec![text(public)];
                pieces.extend(self.printer.signature(&f.signature));
                pieces.push(text(":"));
                self.header(0, pieces, first, f.body[0].span.start.line, blank);
                self.block(&f.body, 1, limit);
            }
            DeclarationKind::External(sig) => {
                let mut pieces = vec![text(format!("{public}external "))];
                pieces.extend(self.printer.signature(sig));
                let last = self.last_code_line(d.span);
                self.construct(0, pieces, first, last, blank);
            }
            DeclarationKind::Constant(c) => {
                let mut pieces = vec![text(public)];
                pieces.extend(self.printer.constant(c));
                let last = self.last_code_line(d.span);
                self.construct(0, pieces, first, last, blank);
            }
            DeclarationKind::Struct(s) => {
                let head = format!(
                    "{public}struct {}{}",
                    s.name.text,
                    self.printer.generics(&s.generics)
                );
                self.with_fields(head, s.fields.as_deref(), first, d.span, limit, blank);
            }
            DeclarationKind::Error(e) => {
                let head = format!("{public}error {}", e.name.text);
                self.with_fields(head, e.fields.as_deref(), first, d.span, limit, blank);
            }
            DeclarationKind::Enum(e) => {
                let head = format!(
                    "{public}enum {}{}:",
                    e.name.text,
                    self.printer.generics(&e.generics)
                );
                let child = e.variants.first().map_or(first, |v| v.name.span.start.line);
                self.header(0, vec![text(head)], first, child, blank);
                for (k, v) in e.variants.iter().enumerate() {
                    let line = v.name.span.start.line;
                    let next = e
                        .variants
                        .get(k + 1)
                        .map_or(limit, |n| n.name.span.start.line);
                    let end = self.notes_free_last(line, next);
                    let blank = if k == 0 { Blank::None } else { Blank::Preserve };
                    let pieces = vec![text(self.printer.variant(v))];
                    self.construct(1, pieces, line, end, blank);
                }
                if let Some(v) = e.variants.first() {
                    let column = self.line_start_column(v.name.span.start.line);
                    self.end_of_block(1, limit, column);
                }
            }
            DeclarationKind::Interface(i) => {
                let head = format!(
                    "{public}interface {}{}:",
                    i.name.text,
                    self.printer.generics(&i.generics)
                );
                let child = i.members.first().map_or(first, |(s, _)| s.span.start.line);
                self.header(0, vec![text(head)], first, child, blank);
                for (k, (sig, body)) in i.members.iter().enumerate() {
                    let line = sig.span.start.line;
                    let next = i
                        .members
                        .get(k + 1)
                        .map_or(limit, |(s, _)| s.span.start.line);
                    let blank = if k == 0 {
                        Blank::None
                    } else if body.is_some() || i.members[k - 1].1.is_some() {
                        Blank::One
                    } else {
                        Blank::Preserve
                    };
                    let mut pieces = self.printer.signature(sig);
                    match body {
                        Some(body) => {
                            pieces.push(text(":"));
                            self.header(1, pieces, line, body[0].span.start.line, blank);
                            self.block(body, 2, next);
                        }
                        None => {
                            let last = self.last_code_line(sig.span);
                            self.construct(1, pieces, line, last, blank);
                        }
                    }
                }
                if let Some((s, _)) = i.members.first() {
                    let column = self.line_start_column(s.span.start.line);
                    self.end_of_block(1, limit, column);
                }
            }
        }
    }

    /// The last line with code of an entry that starts on `line`, before `next`.
    fn notes_free_last(&self, line: usize, next: usize) -> usize {
        let end = self.tokens.partition_point(|t| t.line < next);
        match end.checked_sub(1).map(|i| &self.tokens[i]) {
            Some(t) if t.line >= line => t.line,
            _ => line,
        }
    }

    fn with_fields(
        &mut self,
        head: String,
        fields: Option<&[Field]>,
        first: usize,
        span: Span,
        limit: usize,
        blank: Blank,
    ) {
        match fields {
            Some(fields) if !fields.is_empty() => {
                let child = fields[0].name.span.start.line;
                self.header(0, vec![text(format!("{head}:"))], first, child, blank);
                self.fields(fields, limit);
            }
            _ => {
                let last = self.last_code_line(span);
                self.construct(0, vec![text(head)], first, last, blank);
            }
        }
    }
}

fn item_span(item: &Item) -> Span {
    match item {
        Item::Import(i) => i.span,
        Item::Declaration(d) => d.span,
        Item::Extend(e) => e.span,
        Item::Test(t) => t.span,
    }
}

fn item_line(item: &Item) -> usize {
    item_span(item).start.line
}
