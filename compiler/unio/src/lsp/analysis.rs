//! The analyses of the language server, run inside its worker (T-10, SB-13): diagnostics, hover,
//! go to definition, completion, formatting, and rename. Each takes a [`Request`] and gives the
//! result of LSP as JSON, which the server checks and passes on.
//!
//! The worker reads the document's package and the standard library under its sandbox; the
//! unsaved buffers it was given take the place of their files on disk ([`crate::vfs::overlay`]).
//! Names are resolved from the syntax tree, the declarations of the program, and what the
//! checker recorded for the language server ([`super::facts`]): the type of each expression,
//! each local binding, and which binding a local name refers to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::facts::Facts;
use super::json::Json;
use super::{offset_of, path_to_uri, range, Kind, Request};
use crate::ast::{self, ExprKind, PatternKind, StatementKind};
use crate::diag::{Diagnostic, Severity, Span};
use crate::program::{Binding, DefKind, FnId, ModId, Program, Symbol};
use crate::types::{DefId, Ty};
use crate::Lang;

/// The answer to one request.
pub fn answer(request: &Request) -> Json {
    let mut paths = Paths::default();
    let doc = paths.canonical(&request.path);
    paths.client.insert(doc.clone(), request.path.clone());
    let mut overlay = Vec::new();
    for (path, text) in &request.buffers {
        let canonical = paths.canonical(path);
        paths.client.insert(canonical.clone(), path.clone());
        overlay.push((canonical, text.clone()));
    }
    let text = request
        .buffers
        .iter()
        .find(|(p, _)| *p == request.path)
        .map(|(_, t)| t.clone())
        .or_else(|| std::fs::read_to_string(&request.path).ok())
        .unwrap_or_default();
    let offset = offset_of(&text, request.line, request.character);
    let ctx = Context {
        request,
        paths,
        doc,
        text,
        offset,
    };
    match request.kind {
        Kind::Formatting => ctx.formatting(),
        Kind::Diagnostics => {
            crate::vfs::overlay(overlay);
            ctx.diagnostics()
        }
        Kind::Completion => {
            // A document being typed into seldom parses; the place of the cursor is blanked
            // (keeping every offset) until it does, so that the rest is checked.
            let repaired = repair(&ctx.text, ctx.offset);
            overlay.retain(|(p, _)| *p != ctx.doc);
            overlay.push((ctx.doc.clone(), repaired));
            crate::vfs::overlay(overlay);
            ctx.completion()
        }
        Kind::Hover | Kind::Definition | Kind::InlayHints => {
            // The rest of a document whose lines do not all parse is still analysed.
            let repaired = repair_lines(&ctx.text);
            overlay.retain(|(p, _)| *p != ctx.doc);
            overlay.push((ctx.doc.clone(), repaired));
            crate::vfs::overlay(overlay);
            ctx.navigate()
        }
        Kind::Rename => {
            crate::vfs::overlay(overlay);
            ctx.navigate()
        }
    }
}

fn error(message: impl Into<String>) -> Json {
    Json::object(vec![("error", Json::str(message))])
}

/// The canonical paths of the analysis and the paths the client named them by.
#[derive(Default)]
struct Paths {
    client: HashMap<PathBuf, PathBuf>,
}

impl Paths {
    /// The canonical path, as the loader computes it (inside the AppContainer of a worker on
    /// Windows too, docs/runtime.md 9.4); for a file not saved yet, its directory's.
    fn canonical(&self, path: &Path) -> PathBuf {
        if let Ok(p) = unio_runtime::sandbox::canonical(path) {
            return p;
        }
        match (path.parent(), path.file_name()) {
            (Some(dir), Some(name)) => unio_runtime::sandbox::canonical(dir)
                .map(|d| d.join(name))
                .unwrap_or_else(|_| path.to_path_buf()),
            _ => path.to_path_buf(),
        }
    }

    /// The URI of a module's file, by the path the client uses for it; None for a module that
    /// is no file (the standard library embedded in the compiler).
    fn uri(&self, path: &Path) -> Option<String> {
        let shown = self.client.get(path).map_or(path, PathBuf::as_path);
        shown.is_absolute().then(|| path_to_uri(shown))
    }
}

struct Context<'a> {
    request: &'a Request,
    paths: Paths,
    doc: PathBuf,
    text: String,
    offset: usize,
}

fn lang_text(lang: Lang, en: &str, ru: &str) -> String {
    match lang {
        Lang::En => en.to_string(),
        Lang::Ru => ru.to_string(),
    }
}

/// Blanks the identifier before the offset, and a `.` before it, then the whole line, until
/// the text parses; every offset stays where it was.
fn repair(text: &str, offset: usize) -> String {
    if !crate::check_source("", text).has_errors() {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut start = offset.min(bytes.len());
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    if start > 0 && bytes[start - 1] == b'.' {
        start -= 1;
    }
    let blank = |from: usize, to: usize| {
        let mut out = text.as_bytes().to_vec();
        for b in &mut out[from..to] {
            *b = b' ';
        }
        String::from_utf8(out).unwrap_or_else(|_| text.to_string())
    };
    let first = blank(start, offset.min(bytes.len()));
    if !crate::check_source("", &first).has_errors() {
        return first;
    }
    let first = repair_lines(&first);
    if !crate::check_source("", &first).has_errors() {
        return first;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[offset.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |i| offset + i);
    // Only ASCII bytes are blanked, so the text stays valid UTF-8 with the same offsets.
    let mut out = first.into_bytes();
    for b in &mut out[line_start..line_end] {
        if b.is_ascii() {
            *b = b' ';
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Blanks the lines of the syntax errors, a few rounds at most, keeping every offset.
fn repair_lines(text: &str) -> String {
    let mut out = text.as_bytes().to_vec();
    for _ in 0..8 {
        let current = String::from_utf8_lossy(&out).into_owned();
        let analysis = crate::check_source("", &current);
        if !analysis.has_errors() {
            return current;
        }
        for d in analysis
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
        {
            let start = current[..d.span.start.offset.min(current.len())]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            let from = d
                .span
                .end
                .offset
                .max(d.span.start.offset)
                .min(current.len());
            let end = current[from..]
                .find('\n')
                .map_or(current.len(), |i| from + i);
            for b in &mut out[start..end] {
                if b.is_ascii() {
                    *b = b' ';
                }
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// ---------------------------------------------------------------- loading

/// Loads and checks the files with the facts of the language server recorded.
fn load(files: &[PathBuf]) -> (Program, Facts) {
    super::facts::start();
    let program = crate::load_checked(files, crate::effects::Profile::Application);
    let facts = super::facts::take().unwrap_or_default();
    (program, facts)
}

fn module_of(p: &Program, path: &Path) -> Option<ModId> {
    p.modules.iter().position(|m| m.path == path)
}

/// The nearest directory above the file with `unio.toml`, else the file's directory: the
/// package, as the compiler finds it.
fn package_root(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    dir.ancestors()
        .find(|a| crate::vfs::is_file(&a.join("unio.toml")))
        .map(Path::to_path_buf)
        .unwrap_or(dir)
}

/// The `.unio` files of a package, without hidden directories and without nested packages.
fn package_files(root: &Path, buffers: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if found.len() > 10_000 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && !hidden && !path.join("unio.toml").is_file() {
                stack.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "unio") {
                found.push(path);
            }
        }
    }
    for b in buffers {
        if b.starts_with(root) && !found.contains(b) {
            found.push(b.clone());
        }
    }
    found.sort();
    found
}

// ---------------------------------------------------------------- formatting and diagnostics

impl Context<'_> {
    fn formatting(&self) -> Json {
        match crate::format::format_source(&self.text) {
            Ok(formatted) if formatted == self.text => Json::Array(Vec::new()),
            Ok(formatted) => Json::Array(vec![Json::object(vec![
                ("range", range(&self.text, 0, self.text.len())),
                ("newText", Json::str(formatted)),
            ])]),
            // A file with syntax errors is left unchanged, as by `unio fmt`.
            Err(_) => Json::Null,
        }
    }

    fn diagnostics(&self) -> Json {
        let lang = self.request.lang;
        let mut items = Vec::new();
        // The manifest of the package (E0088, E0089, T-6): in its own document at its lines, and in
        // a source document at its start, with the place in the manifest as related
        // information, since an editor may show only the documents of the language.
        let package = package_root(&self.doc);
        let manifest = package.join("unio.toml");
        let (manifest_text, manifest_found) = match crate::vfs::read_to_string(&manifest) {
            Ok(text) => {
                let found = crate::program::package_manifest_diagnostics(&package, &text);
                (text, found)
            }
            Err(_) => (String::new(), Vec::new()),
        };
        if self.doc.file_name().is_some_and(|n| n == "unio.toml") {
            for d in &manifest_found {
                let at = range(&manifest_text, d.span.start.offset, d.span.end.offset);
                let own = self.related_items(d, None, &manifest, &manifest_text, &package);
                items.push(lsp_diagnostic(d, at, lang, None, own));
            }
            return Json::object(vec![("diagnostics", Json::Array(items))]);
        }
        let program = crate::load_checked(
            std::slice::from_ref(&self.doc),
            crate::effects::Profile::Application,
        );
        if let Some(m) = module_of(&program, &self.doc) {
            let module = &program.modules[m];
            let mut all = module.analysis.diagnostics.clone();
            for (owner, d) in &program.diagnostics {
                if *owner == m && !all.contains(d) {
                    all.push(d.clone());
                }
            }
            all.sort_by_key(|d| (d.span.start.offset, d.span.end.offset));
            let source = &module.analysis.source;
            if let Some(uri) = self.paths.uri(&manifest) {
                for d in &manifest_found {
                    let at = range(&manifest_text, d.span.start.offset, d.span.end.offset);
                    let related = Json::object(vec![
                        (
                            "location",
                            Json::object(vec![("uri", Json::str(uri.clone())), ("range", at)]),
                        ),
                        (
                            "message",
                            Json::str(format!("unio.toml:{}", d.span.start.line)),
                        ),
                    ]);
                    let own = self.related_items(d, None, &manifest, &manifest_text, &package);
                    let at = range(source, 0, 0);
                    items.push(lsp_diagnostic(d, at, lang, Some(related), own));
                }
            }
            let mut holes = 0;
            for d in all {
                let at = range(source, d.span.start.offset, d.span.end.offset);
                let own = self.related_items(&d, Some(&program), &self.doc, source, &package);
                let mut item = lsp_diagnostic(&d, at, lang, None, own);
                // The fixes and the expected type travel with the diagnostic; the server binds
                // them to the revision of the document they were made for (T-13).
                if let Some(Json::Object(data)) = match &mut item {
                    Json::Object(fields) => {
                        fields.iter_mut().find(|(k, _)| k == "data").map(|(_, v)| v)
                    }
                    _ => None,
                } {
                    let expected = d.expected.clone().or_else(|| {
                        // A syntax error of incomplete code: the type expected where the code
                        // is missing, found by checking a name put there (T-13).
                        if holes < 3 && hole_text(source, &d).is_some() {
                            holes += 1;
                            self.hole_expected(source, &d)
                        } else {
                            None
                        }
                    });
                    if let Some(expected) = expected {
                        data.push(("expected".to_string(), Json::str(expected)));
                    }
                    let fixes: Vec<Json> = d
                        .fixes
                        .iter()
                        .map(|f| {
                            Json::object(vec![
                                ("title", Json::str(f.description.get(lang))),
                                (
                                    "range",
                                    range(source, f.span.start.offset, f.span.end.offset),
                                ),
                                ("newText", Json::str(f.new_text.clone())),
                            ])
                        })
                        .collect();
                    if !fixes.is_empty() {
                        data.push(("fixes".to_string(), Json::Array(fixes)));
                    }
                }
                items.push(item);
            }
        }
        Json::object(vec![("diagnostics", Json::Array(items))])
    }
}

/// The name put in the place of missing code (T-13). It is an ordinary name: if the program
/// declares it too, the expected type is found all the same.
const HOLE: &str = "unio_expected_here";

/// The text of a document with a name in the place of the code that a syntax error of
/// incomplete code misses, and the offset of the name: for E0012 that expects an expression,
/// at the place of the error; for E0009, at the end of the line of the unclosed bracket, after
/// an operator, a comma or an opening bracket, followed by the brackets that close the line.
fn hole_text(text: &str, d: &Diagnostic) -> Option<(String, usize)> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    match d.code {
        "E0012" if d.message.en.starts_with("expected an expression") => {
            let at = d.span.start.offset.min(text.len());
            let mut out = text[..at].to_string();
            if out.chars().last().is_some_and(word) {
                out.push(' ');
            }
            let hole = out.len();
            out.push_str(HOLE);
            if text[at..].chars().next().is_some_and(word) {
                out.push(' ');
            }
            out.push_str(&text[at..]);
            Some((out, hole))
        }
        "E0009" => {
            let open = d.span.start.offset.min(text.len());
            let end = text[open..].find('\n').map_or(text.len(), |i| open + i);
            let mut closers = Vec::new();
            let mut in_text = false;
            let mut escaped = false;
            let mut content = end;
            for (i, c) in text[open..end].char_indices() {
                if in_text {
                    match c {
                        '\\' if !escaped => escaped = true,
                        '"' if !escaped => in_text = false,
                        _ => escaped = false,
                    }
                    continue;
                }
                match c {
                    '"' => in_text = true,
                    '(' => closers.push(')'),
                    '[' => closers.push(']'),
                    '{' => closers.push('}'),
                    ')' | ']' | '}' => {
                        closers.pop();
                    }
                    '#' => {
                        content = open + i;
                        break;
                    }
                    _ => {}
                }
            }
            if in_text || closers.is_empty() {
                return None;
            }
            let content = open + text[open..content].trim_end().len();
            let last = text[..content].chars().last()?;
            if !"([{,=+-*/%<>".contains(last) {
                return None;
            }
            let mut out = text[..content].to_string();
            if !"([{".contains(last) {
                out.push(' ');
            }
            let hole = out.len();
            out.push_str(HOLE);
            out.extend(closers.iter().rev());
            out.push_str(&text[content..]);
            Some((out, hole))
        }
        _ => None,
    }
}

impl Context<'_> {
    /// The type expected in the place of the code that a syntax error of incomplete code
    /// misses: the document is checked with a name there, the rest of its lines that do not
    /// parse blanked, and the checker records the type expected where the name is (T-13).
    fn hole_expected(&self, text: &str, d: &Diagnostic) -> Option<String> {
        let (filled, hole) = hole_text(text, d)?;
        let filled = repair_lines(&filled);
        if !filled.is_char_boundary(hole) || !filled[hole..].starts_with(HOLE) {
            return None;
        }
        let doc = self.doc.clone();
        crate::vfs::with_replaced(&self.doc, filled, || {
            super::facts::start();
            let program = crate::load_checked(
                std::slice::from_ref(&doc),
                crate::effects::Profile::Application,
            );
            let facts = super::facts::take().unwrap_or_default();
            let m = module_of(&program, &doc)?;
            facts
                .expected
                .iter()
                .find(|(module, span, _)| *module == m && span.start.offset == hole)
                .filter(|(_, _, ty)| !ty.is_partial())
                .map(|(_, _, ty)| program.show(ty))
        })
    }

    /// The related locations of a diagnostic (C-8) as related information of the protocol. A
    /// location without a file is in `path`, whose text is `text`; another file is a module of
    /// the program, or a file relative to the package (E0089) or named by the command.
    fn related_items(
        &self,
        d: &Diagnostic,
        program: Option<&crate::program::Program>,
        path: &Path,
        text: &str,
        package: &Path,
    ) -> Vec<Json> {
        let lang = self.request.lang;
        d.related
            .iter()
            .filter_map(|r| {
                let (file, source) = match &r.file {
                    None => (path.to_path_buf(), text.to_string()),
                    Some(f) => {
                        let module =
                            program.and_then(|p| p.modules.iter().find(|m| m.analysis.path == *f));
                        match module {
                            Some(m) => (m.path.clone(), m.analysis.source.clone()),
                            None => {
                                let named = Path::new(f);
                                let file = if named.is_absolute() {
                                    named.to_path_buf()
                                } else {
                                    package.join(named)
                                };
                                let source = crate::vfs::read_to_string(&file).ok()?;
                                (file, source)
                            }
                        }
                    }
                };
                let uri = self.paths.uri(&file)?;
                let at = range(&source, r.span.start.offset, r.span.end.offset);
                Some(Json::object(vec![
                    (
                        "location",
                        Json::object(vec![("uri", Json::str(uri)), ("range", at)]),
                    ),
                    ("message", Json::str(r.message.get(lang))),
                ]))
            })
            .collect()
    }
}

/// A diagnostic of the protocol: the range, the code with the link to its explanation, and the
/// message; with `manifest`, a manifest diagnostic shown in a source document, whose message
/// starts with its place; `own` holds the related locations of the diagnostic itself (C-8).
fn lsp_diagnostic(
    d: &Diagnostic,
    at: Json,
    lang: Lang,
    manifest: Option<Json>,
    own: Vec<Json>,
) -> Json {
    let severity = match d.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Hint => 4,
    };
    let mut message = d.message.get(lang).to_string();
    if let Some(label) = &d.label {
        message.push_str(&format!(" ({})", label.get(lang)));
    }
    if manifest.is_some() {
        message = format!("unio.toml:{}: {message}", d.span.start.line);
    }
    let mut fields = vec![
        ("range", at),
        ("severity", Json::int(severity)),
        ("code", Json::str(d.code)),
        (
            "codeDescription",
            Json::object(vec![("href", Json::str(explanation_uri(d.code)))]),
        ),
        ("source", Json::str("unio")),
        ("message", Json::str(message)),
        (
            "data",
            Json::object(vec![(
                "explanation",
                Json::str(crate::diag::explanation(d.code).get(lang)),
            )]),
        ),
    ];
    let related: Vec<Json> = manifest.into_iter().chain(own).collect();
    if !related.is_empty() {
        fields.push(("relatedInformation", Json::Array(related)));
    }
    Json::object(fields)
}

/// The link of a code to its explanation in the catalog (C-9): the editor shows it as the text
/// of `unio explain CODE`, which the server gives for `unio/explain`.
pub fn explanation_uri(code: &str) -> String {
    format!("unio-explain:/{code}")
}

// ---------------------------------------------------------------- names

/// What a name in the source stands for.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    /// A local binding of a module, by the place where it is bound.
    Local(ModId, Span),
    Func(FnId),
    Def(DefId),
    Const(usize),
    Module(ModId),
    Field(DefId, String),
    Variant(DefId, usize),
}

/// A name in a module: its place, what it stands for, and whether it declares it.
#[derive(Clone, Debug)]
struct Occurrence {
    span: Span,
    target: Target,
    decl: bool,
}

fn key(span: Span) -> (usize, usize) {
    (span.start.offset, span.end.offset)
}

/// What the checker recorded, by module and place.
struct Recorded {
    types: HashMap<(ModId, (usize, usize)), Ty>,
    local_refs: HashMap<(ModId, (usize, usize)), Span>,
    locals: Vec<super::facts::Local>,
    usages: HashMap<(ModId, (usize, usize)), FnId>,
}

impl Recorded {
    fn new(p: &Program, facts: Facts) -> Recorded {
        let mut types = HashMap::new();
        for (m, span, ty) in facts.types {
            // A check that runs again for its types only may know less than the first.
            let entry = types.entry((m, key(span))).or_insert(Ty::Unknown);
            if !ty.is_unknown() {
                *entry = ty;
            }
        }
        let local_refs = facts
            .local_refs
            .into_iter()
            .map(|(m, used, bound)| ((m, key(used)), bound))
            .collect();
        let mut locals: Vec<super::facts::Local> = Vec::new();
        for local in facts.locals {
            match locals
                .iter_mut()
                .find(|l| l.module == local.module && l.span == local.span)
            {
                Some(known) if !local.ty.is_unknown() => known.ty = local.ty,
                Some(_) => {}
                None => locals.push(local),
            }
        }
        let usages = p
            .usages
            .iter()
            .map(|(f, m, span)| ((*m, key(*span)), *f))
            .collect();
        Recorded {
            types,
            local_refs,
            locals,
            usages,
        }
    }

    fn ty(&self, m: ModId, span: Span) -> Option<&Ty> {
        self.types.get(&(m, key(span))).filter(|t| !t.is_unknown())
    }
}

/// Collects the names of one module.
struct Walk<'a> {
    p: &'a Program,
    r: &'a Recorded,
    m: ModId,
    out: Vec<Occurrence>,
}

impl<'a> Walk<'a> {
    fn module(p: &'a Program, r: &'a Recorded, m: ModId) -> Vec<Occurrence> {
        let mut w = Walk {
            p,
            r,
            m,
            out: Vec::new(),
        };
        for local in r.locals.iter().filter(|l| l.module == m) {
            w.add(local.span, Target::Local(m, local.span), true);
        }
        for item in &p.modules[m].analysis.file.items {
            w.item(item);
        }
        let mut out = w.out;
        out.sort_by_key(|o| (o.span.start.offset, o.span.end.offset));
        // The first finding of a place wins; the declaration of a local is found first.
        out.dedup_by(|b, a| key(a.span) == key(b.span));
        out
    }

    fn add(&mut self, span: Span, target: Target, decl: bool) {
        if span.end.offset > span.start.offset {
            self.out.push(Occurrence { span, target, decl });
        }
    }

    fn symbol_target(symbol: Symbol) -> Target {
        match symbol {
            Symbol::Func(f) => Target::Func(f),
            Symbol::Def(d) => Target::Def(d),
            Symbol::Const(c) => Target::Const(c),
        }
    }

    fn own(&self, name: &str) -> Option<Symbol> {
        self.p.modules[self.m].symbols.get(name).copied()
    }

    fn item(&mut self, item: &ast::Item) {
        match item {
            ast::Item::Import(i) => {
                for name in &i.names {
                    if let Some(Binding::Symbol(_, s)) =
                        self.p.modules[self.m].imports.get(&name.text)
                    {
                        self.add(name.span, Walk::symbol_target(*s), false);
                    }
                }
                let alias = i.alias.as_ref().or(i.module.last());
                if let (Some(alias), Some(last)) = (alias, i.module.last()) {
                    if let Some(target) = self.p.module_alias(self.m, &alias.text) {
                        self.add(last.span, Target::Module(target), false);
                        if let Some(a) = &i.alias {
                            self.add(a.span, Target::Module(target), true);
                        }
                    }
                }
            }
            ast::Item::Declaration(d) => match &d.kind {
                ast::DeclarationKind::Function(f) => {
                    if let Some(s) = self.own(&f.signature.name.text) {
                        self.add(f.signature.name.span, Walk::symbol_target(s), true);
                    }
                    self.signature(&f.signature);
                    self.block(&f.body);
                }
                ast::DeclarationKind::External(sig) => {
                    if let Some(s) = self.own(&sig.name.text) {
                        self.add(sig.name.span, Walk::symbol_target(s), true);
                    }
                    self.signature(sig);
                }
                ast::DeclarationKind::Struct(s) => {
                    let def = self.own(&s.name.text);
                    self.fields(def, &s.name, s.fields.as_deref());
                }
                ast::DeclarationKind::Error(e) => {
                    let def = self.own(&e.name.text);
                    self.fields(def, &e.name, e.fields.as_deref());
                }
                ast::DeclarationKind::Enum(e) => {
                    if let Some(Symbol::Def(d)) = self.own(&e.name.text) {
                        self.add(e.name.span, Target::Def(d), true);
                        for (v, variant) in e.variants.iter().enumerate() {
                            self.add(variant.name.span, Target::Variant(d, v), true);
                            for (_, ty) in &variant.fields {
                                self.ty(ty);
                            }
                        }
                    }
                }
                ast::DeclarationKind::Interface(i) => {
                    if let Some(Symbol::Def(d)) = self.own(&i.name.text) {
                        self.add(i.name.span, Target::Def(d), true);
                        let methods = match &self.p.defs[d].kind {
                            DefKind::Interface(ms) => ms.clone(),
                            _ => Vec::new(),
                        };
                        for (k, (sig, body)) in i.members.iter().enumerate() {
                            if let Some(f) = methods.get(k) {
                                self.add(sig.name.span, Target::Func(*f), true);
                            }
                            self.signature(sig);
                            if let Some(body) = body {
                                self.block(body);
                            }
                        }
                    }
                }
                ast::DeclarationKind::Constant(c) => {
                    if let Some(s) = self.own(&c.name.text) {
                        self.add(c.name.span, Walk::symbol_target(s), true);
                    }
                    if let Some(t) = &c.ty {
                        self.ty(t);
                    }
                    self.expr(&c.value);
                }
            },
            ast::Item::Extend(e) => {
                self.ty(&e.target);
                if let Some(i) = &e.interface {
                    self.ty(i);
                }
                let methods = self
                    .p
                    .impls
                    .iter()
                    .find(|i| i.module == self.m && i.span == e.span)
                    .map(|i| i.methods.clone())
                    .unwrap_or_default();
                for (k, member) in e.members.iter().enumerate() {
                    if let Some(f) = methods.get(k) {
                        self.add(member.signature.name.span, Target::Func(*f), true);
                    }
                    self.signature(&member.signature);
                    if let Some(body) = &member.body {
                        self.block(body);
                    }
                }
            }
            ast::Item::Test(t) => self.block(&t.body),
        }
    }

    fn fields(&mut self, def: Option<Symbol>, name: &ast::Name, fields: Option<&[ast::Field]>) {
        let Some(Symbol::Def(d)) = def else {
            return;
        };
        self.add(name.span, Target::Def(d), true);
        for field in fields.unwrap_or_default() {
            self.add(
                field.name.span,
                Target::Field(d, field.name.text.clone()),
                true,
            );
            self.ty(&field.ty);
            if let Some(value) = &field.default {
                self.expr(value);
            }
        }
    }

    fn signature(&mut self, sig: &ast::Signature) {
        for g in &sig.generics {
            for b in &g.bounds {
                self.ty(b);
            }
        }
        for param in &sig.params {
            match param {
                ast::Param::SelfParam { .. } => {}
                ast::Param::Named { ty, default, .. } | ast::Param::Plain { ty, default, .. } => {
                    self.ty(ty);
                    if let Some(d) = default {
                        self.expr(d);
                    }
                }
            }
        }
        if let Some(r) = &sig.returns {
            self.ty(r);
        }
        for f in &sig.fails {
            self.ty(f);
        }
    }

    fn ty(&mut self, t: &ast::Type) {
        match t.path.as_slice() {
            [name] => {
                if let Some((_, Symbol::Def(d))) = self.p.lookup(self.m, &name.text) {
                    self.add(name.span, Target::Def(d), false);
                }
            }
            [alias, name] => {
                if let Some(target) = self.p.module_alias(self.m, &alias.text) {
                    self.add(alias.span, Target::Module(target), false);
                    if let Some(Symbol::Def(d)) = self.p.modules[target].symbols.get(&name.text) {
                        self.add(name.span, Target::Def(*d), false);
                    }
                }
            }
            _ => {}
        }
        for a in &t.arguments {
            match a {
                ast::TypeArgument::Type(t) => self.ty(t),
                ast::TypeArgument::Shape {
                    params,
                    returns,
                    fails,
                } => {
                    for (_, t) in params {
                        self.ty(t);
                    }
                    if let Some(r) = returns {
                        self.ty(r);
                    }
                    for f in fails {
                        self.ty(f);
                    }
                }
            }
        }
    }

    fn block(&mut self, block: &ast::Block) {
        for s in block {
            self.statement(s);
        }
    }

    fn statement(&mut self, s: &ast::Statement) {
        match &s.kind {
            StatementKind::Simple { statement, handler } => {
                match statement {
                    ast::Simple::Let { ty, value, .. } => {
                        if let Some(t) = ty {
                            self.ty(t);
                        }
                        self.expr(value);
                    }
                    ast::Simple::Assign { target, value } => {
                        self.expr(target);
                        self.expr(value);
                    }
                    ast::Simple::Return(Some(e))
                    | ast::Simple::Fail(e)
                    | ast::Simple::Assert(e)
                    | ast::Simple::Expr(e) => self.expr(e),
                    _ => {}
                }
                if let Some(h) = handler {
                    self.block(h);
                }
            }
            StatementKind::If {
                branches,
                otherwise,
            } => {
                for (c, b) in branches {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(o) = otherwise {
                    self.block(o);
                }
            }
            StatementKind::For { iterable, body, .. } => {
                self.expr(iterable);
                self.block(body);
            }
            StatementKind::While { condition, body } => {
                self.expr(condition);
                self.block(body);
            }
            StatementKind::Match { subject, cases } => {
                self.expr(subject);
                for c in cases {
                    self.pattern(&c.pattern);
                    if let Some(g) = &c.guard {
                        self.expr(g);
                    }
                    self.block(&c.body);
                }
            }
            StatementKind::Try { body, handlers } => {
                self.block(body);
                for h in handlers {
                    self.ty(&h.ty);
                    self.block(&h.body);
                }
            }
            StatementKind::Using { value, body, .. } => {
                self.expr(value);
                self.block(body);
            }
        }
    }

    /// The variant a pattern or a short name designates, by its enum or by its name alone.
    fn variant(&self, path: &[ast::Name]) -> Option<(DefId, usize)> {
        let (enum_def, name) = match path {
            [name] => (None, name),
            [e, name] => match self.p.lookup(self.m, &e.text) {
                Some((_, Symbol::Def(d))) => (Some(d), name),
                _ => return None,
            },
            _ => return None,
        };
        let find = |d: DefId| match &self.p.defs[d].kind {
            DefKind::Enum(vs) => vs.iter().position(|v| v.name == name.text).map(|v| (d, v)),
            _ => None,
        };
        match enum_def {
            Some(d) => find(d),
            None => {
                // A short variant: the enums of this module first, then all.
                let own = (0..self.p.defs.len()).filter(|d| self.p.defs[*d].module == self.m);
                own.chain(0..self.p.defs.len()).find_map(find)
            }
        }
    }

    fn pattern(&mut self, p: &ast::Pattern) {
        match &p.kind {
            PatternKind::Name(path) => {
                let bound = path.len() == 1
                    && self
                        .r
                        .locals
                        .iter()
                        .any(|l| l.module == self.m && l.span == path[0].span);
                if !bound {
                    if let (Some((d, v)), Some(last)) = (self.variant(path), path.last()) {
                        self.add(last.span, Target::Variant(d, v), false);
                    }
                }
            }
            PatternKind::Constructor { name, arguments } => {
                if let (Some((d, v)), Some(last)) = (self.variant(name), name.last()) {
                    self.add(last.span, Target::Variant(d, v), false);
                } else if let Some(last) = name.last() {
                    if let Some((_, Symbol::Def(d))) = self.p.lookup(self.m, &last.text) {
                        self.add(last.span, Target::Def(d), false);
                        for (field, _) in arguments {
                            if let Some(f) = field {
                                self.add(f.span, Target::Field(d, f.text.clone()), false);
                            }
                        }
                    }
                }
                for (_, a) in arguments {
                    self.pattern(a);
                }
            }
            PatternKind::List(elements) => {
                for e in elements {
                    if let ast::ListElement::Pattern(p) = e {
                        self.pattern(p);
                    }
                }
            }
            PatternKind::Tuple(ps) | PatternKind::Or(ps) => {
                for p in ps {
                    self.pattern(p);
                }
            }
            PatternKind::Bind { pattern, .. } => self.pattern(pattern),
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
        }
    }

    /// The name of an expression that names something directly: `name` or `alias.name`.
    fn named(&self, e: &ast::Expr) -> Option<(Span, Target)> {
        match &e.kind {
            ExprKind::Name(n) => {
                if let Some(bound) = self.r.local_refs.get(&(self.m, key(e.span))) {
                    return Some((e.span, Target::Local(self.m, *bound)));
                }
                if let Some(f) = self.r.usages.get(&(self.m, key(e.span))) {
                    return Some((e.span, Target::Func(*f)));
                }
                if let Some((_, s)) = self.p.lookup(self.m, n) {
                    return Some((e.span, Walk::symbol_target(s)));
                }
                if let Some(target) = self.p.module_alias(self.m, n) {
                    return Some((e.span, Target::Module(target)));
                }
                let name = ast::Name {
                    text: n.clone(),
                    span: e.span,
                };
                self.variant(std::slice::from_ref(&name))
                    .map(|(d, v)| (e.span, Target::Variant(d, v)))
            }
            ExprKind::Field { target, name } => {
                let ExprKind::Name(alias) = &target.kind else {
                    return None;
                };
                if self.r.local_refs.contains_key(&(self.m, key(target.span))) {
                    return None;
                }
                if let Some(m) = self.p.module_alias(self.m, alias) {
                    return self.p.modules[m]
                        .symbols
                        .get(&name.text)
                        .map(|s| (name.span, Walk::symbol_target(*s)));
                }
                if let Some((_, Symbol::Def(d))) = self.p.lookup(self.m, alias) {
                    if let DefKind::Enum(vs) = &self.p.defs[d].kind {
                        if let Some(v) = vs.iter().position(|v| v.name == name.text) {
                            return Some((name.span, Target::Variant(d, v)));
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn call(&mut self, e: &ast::Expr, callee: &ast::Expr, arguments: &[ast::Argument]) {
        let name_span = match &callee.kind {
            ExprKind::Name(_) => Some(callee.span),
            ExprKind::Field { name, .. } | ExprKind::SafeField { name, .. } => Some(name.span),
            _ => None,
        };
        if let (Some(span), Some(f)) = (name_span, self.r.usages.get(&(self.m, key(e.span)))) {
            if !self.r.local_refs.contains_key(&(self.m, key(callee.span))) {
                self.add(span, Target::Func(*f), false);
            }
        }
        // The fields a struct or an error is built with.
        if let Some((_, Target::Def(d))) = self.named(callee) {
            for a in arguments {
                if let Some(n) = &a.name {
                    self.add(n.span, Target::Field(d, n.text.clone()), false);
                }
            }
        }
    }

    fn expr(&mut self, e: &ast::Expr) {
        if let Some((span, target)) = self.named(e) {
            self.add(span, target, false);
        }
        match &e.kind {
            ExprKind::Name(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::SelfValue
            | ExprKind::Error => {}
            ExprKind::Text(parts) => {
                for part in parts {
                    if let ast::TextPart::Interpolation(e) = part {
                        self.expr(e);
                    }
                }
            }
            ExprKind::Paren(inner) => self.expr(inner),
            ExprKind::Tuple(items) | ExprKind::List(items) => {
                for i in items {
                    self.expr(i);
                }
            }
            ExprKind::Comprehension {
                element,
                iterable,
                condition,
                ..
            } => {
                self.expr(iterable);
                self.expr(element);
                if let Some(c) = condition {
                    self.expr(c);
                }
            }
            ExprKind::Map(entries) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            ExprKind::Lambda {
                params,
                returns,
                fails,
                body,
            } => {
                for p in params {
                    if let Some(t) = &p.ty {
                        self.ty(t);
                    }
                }
                if let Some(r) = returns {
                    self.ty(r);
                }
                for f in fails {
                    self.ty(f);
                }
                self.expr(body);
            }
            ExprKind::Is { value, pattern } => {
                self.expr(value);
                self.pattern(pattern);
            }
            ExprKind::Conditional {
                then,
                condition,
                otherwise,
            } => {
                self.expr(then);
                self.expr(condition);
                self.expr(otherwise);
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            ExprKind::Unary { operand, .. } => self.expr(operand),
            ExprKind::Try { operand, handler } => {
                self.expr(operand);
                if let Some((t, _)) = handler {
                    self.ty(t);
                }
            }
            ExprKind::Call { callee, arguments } => {
                self.call(e, callee, arguments);
                self.expr(callee);
                for a in arguments {
                    self.expr(&a.value);
                }
            }
            ExprKind::GenericCall {
                callee,
                types,
                arguments,
            } => {
                self.call(e, callee, arguments);
                self.expr(callee);
                for t in types {
                    self.ty(t);
                }
                for a in arguments {
                    self.expr(&a.value);
                }
            }
            ExprKind::Index { target, index } => {
                self.expr(target);
                self.expr(index);
            }
            ExprKind::Field { target, name } => {
                if self.named(e).is_none() {
                    if let Some(Ty::Named(d, _)) = self.r.ty(self.m, target.span) {
                        let has = matches!(&self.p.defs[*d].kind,
                            DefKind::Struct(fs) | DefKind::Error(fs) if fs.iter().any(|f| f.name == name.text));
                        if has {
                            self.add(name.span, Target::Field(*d, name.text.clone()), false);
                        }
                    }
                }
                self.expr(target);
            }
            ExprKind::SafeField { target, .. } => self.expr(target),
        }
    }
}

// ---------------------------------------------------------------- navigation

struct Navigation {
    p: Program,
    r: Recorded,
    /// The names of each module, computed when needed.
    occurrences: HashMap<ModId, Vec<Occurrence>>,
}

impl Navigation {
    fn new(p: Program, facts: Facts) -> Navigation {
        let r = Recorded::new(&p, facts);
        Navigation {
            p,
            r,
            occurrences: HashMap::new(),
        }
    }

    fn of(&mut self, m: ModId) -> &[Occurrence] {
        if !self.occurrences.contains_key(&m) {
            let found = Walk::module(&self.p, &self.r, m);
            self.occurrences.insert(m, found);
        }
        &self.occurrences[&m]
    }

    /// The name at the offset: the shortest one that contains it, its end included.
    fn at(&mut self, m: ModId, offset: usize) -> Option<Occurrence> {
        self.of(m)
            .iter()
            .filter(|o| o.span.start.offset <= offset && offset <= o.span.end.offset)
            .min_by_key(|o| o.span.end.offset - o.span.start.offset)
            .cloned()
    }

    /// The module and place that declare the target.
    fn declaration(&mut self, target: &Target) -> Option<(ModId, Span)> {
        let p = &self.p;
        match target {
            Target::Local(m, span) => Some((*m, *span)),
            Target::Func(f) => Some((p.funcs[*f].module, p.funcs[*f].span)),
            Target::Def(d) => Some((p.defs[*d].module, p.defs[*d].span)),
            Target::Const(c) => Some((p.consts[*c].module, p.consts[*c].span)),
            Target::Module(m) => Some((*m, Span::default())),
            Target::Field(d, _) | Target::Variant(d, _) => {
                let m = p.defs[*d].module;
                self.of(m)
                    .iter()
                    .find(|o| o.decl && o.target == *target)
                    .map(|o| (m, o.span))
            }
        }
    }
}

impl Context<'_> {
    fn navigate(&self) -> Json {
        let rename = self.request.kind == Kind::Rename;
        let root = package_root(&self.doc);
        let files = if rename {
            // Every module of the package, so that every reference is found.
            let buffers: Vec<PathBuf> = self
                .request
                .buffers
                .iter()
                .map(|(p, _)| self.paths.canonical(p))
                .collect();
            let mut files = vec![self.doc.clone()];
            files.extend(
                package_files(&root, &buffers)
                    .into_iter()
                    .filter(|f| *f != self.doc),
            );
            files
        } else {
            vec![self.doc.clone()]
        };
        let (program, facts) = load(&files);
        let lang = self.request.lang;
        let Some(m) = module_of(&program, &self.doc) else {
            return Json::Null;
        };
        if rename {
            let broken = program
                .modules
                .iter()
                .any(|module| !module.stdlib && module.analysis.has_errors());
            if broken {
                return error(lang_text(
                    lang,
                    "the package has syntax errors; rename needs every module of the package to parse",
                    "в пакете есть синтаксические ошибки; для переименования все модули пакета должны разбираться",
                ));
            }
        }
        let mut nav = Navigation::new(program, facts);
        if self.request.kind == Kind::InlayHints {
            return inlay_hints(&nav, m);
        }
        let found = nav.at(m, self.offset);
        match self.request.kind {
            Kind::Definition => match found {
                Some(o) => self.location(&mut nav, &o.target),
                None => Json::Null,
            },
            Kind::Hover => self.hover(&mut nav, m, found),
            _ => match found {
                Some(o) => self.rename(&mut nav, &root, &o.target),
                None => error(lang_text(
                    lang,
                    "there is no name to rename here",
                    "здесь нет имени для переименования",
                )),
            },
        }
    }

    fn location(&self, nav: &mut Navigation, target: &Target) -> Json {
        let Some((m, span)) = nav.declaration(target) else {
            return Json::Null;
        };
        let module = &nav.p.modules[m];
        match self.paths.uri(&module.path) {
            Some(uri) => Json::object(vec![
                ("uri", Json::str(uri)),
                (
                    "range",
                    range(&module.analysis.source, span.start.offset, span.end.offset),
                ),
            ]),
            None => Json::Null,
        }
    }

    fn hover(&self, nav: &mut Navigation, m: ModId, found: Option<Occurrence>) -> Json {
        let source = nav.p.modules[m].analysis.source.clone();
        let (text, span) = match found {
            Some(o) => (describe(nav, &o.target), o.span),
            None => {
                // Not a name: the type of the shortest expression at the cursor.
                let offset = self.offset;
                let best = nav
                    .r
                    .types
                    .iter()
                    .filter(|((module, (s, e)), t)| {
                        *module == m && *s <= offset && offset < *e && !t.is_unknown()
                    })
                    .min_by_key(|((_, (s, e)), _)| e - s);
                match best {
                    Some(((_, (s, e)), t)) => (
                        code_block(&nav.p.show(t)),
                        Span {
                            start: crate::diag::Pos {
                                offset: *s,
                                ..Default::default()
                            },
                            end: crate::diag::Pos {
                                offset: *e,
                                ..Default::default()
                            },
                        },
                    ),
                    None => return Json::Null,
                }
            }
        };
        if text.is_empty() {
            return Json::Null;
        }
        Json::object(vec![
            (
                "contents",
                Json::object(vec![
                    ("kind", Json::str("markdown")),
                    ("value", Json::str(text)),
                ]),
            ),
            ("range", range(&source, span.start.offset, span.end.offset)),
        ])
    }

    fn rename(&self, nav: &mut Navigation, root: &Path, target: &Target) -> Json {
        let lang = self.request.lang;
        let new_name = self.request.new_name.as_str();
        let valid = new_name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && new_name.bytes().all(is_ident)
            && !crate::lexer::KEYWORDS.contains(&new_name)
            && !crate::lexer::CONTEXTUAL.contains(&new_name)
            && !crate::lexer::RESERVED.contains(&new_name);
        if !valid {
            return error(lang_text(
                lang,
                &format!("'{new_name}' is not a name that can be declared"),
                &format!("«{new_name}» нельзя объявить как имя"),
            ));
        }
        if matches!(target, Target::Module(_)) {
            return error(lang_text(
                lang,
                "a module is named by its file; rename the file instead",
                "модуль называется по своему файлу; переименуйте файл",
            ));
        }
        let Some((decl_module, _)) = nav.declaration(target) else {
            return error(lang_text(
                lang,
                "the declaration of this name is not found",
                "объявление этого имени не найдено",
            ));
        };
        let owner = &nav.p.modules[decl_module];
        if owner.stdlib || !owner.path.starts_with(root) {
            return error(lang_text(
                lang,
                "this name is declared outside the package (in the standard library or another package) and cannot be renamed here",
                "это имя объявлено вне пакета (в стандартной библиотеке или другом пакете), его нельзя переименовать здесь",
            ));
        }
        let top_level = matches!(target, Target::Func(_) | Target::Def(_) | Target::Const(_));
        if top_level && owner.symbols.contains_key(new_name) {
            return error(lang_text(
                lang,
                &format!("the module already declares '{new_name}'"),
                &format!("в модуле уже объявлено «{new_name}»"),
            ));
        }
        let modules: Vec<ModId> = match target {
            Target::Local(m, _) => vec![*m],
            _ => (0..nav.p.modules.len())
                .filter(|m| !nav.p.modules[*m].stdlib && nav.p.modules[*m].path.starts_with(root))
                .collect(),
        };
        let mut changes = Vec::new();
        for m in modules {
            let spans: Vec<Span> = nav
                .of(m)
                .iter()
                .filter(|o| o.target == *target)
                .map(|o| o.span)
                .collect();
            if spans.is_empty() {
                continue;
            }
            let module = &nav.p.modules[m];
            let Some(uri) = self.paths.uri(&module.path) else {
                continue;
            };
            let edits = spans
                .iter()
                .map(|s| {
                    Json::object(vec![
                        (
                            "range",
                            range(&module.analysis.source, s.start.offset, s.end.offset),
                        ),
                        ("newText", Json::str(new_name)),
                    ])
                })
                .collect();
            changes.push((uri, Json::Array(edits)));
        }
        Json::object(vec![("changes", Json::Object(changes))])
    }
}

/// The names of the `let` and `var` bindings of a block written without a type, nested blocks
/// included.
fn untyped_lets<'b>(block: &'b ast::Block, out: &mut Vec<&'b ast::Name>) {
    for s in block {
        match &s.kind {
            StatementKind::Simple { statement, handler } => {
                if let ast::Simple::Let { ty: None, name, .. } = statement {
                    out.push(name);
                }
                if let Some(h) = handler {
                    untyped_lets(h, out);
                }
            }
            StatementKind::If {
                branches,
                otherwise,
            } => {
                for (_, b) in branches {
                    untyped_lets(b, out);
                }
                if let Some(o) = otherwise {
                    untyped_lets(o, out);
                }
            }
            StatementKind::For { body, .. }
            | StatementKind::While { body, .. }
            | StatementKind::Using { body, .. } => untyped_lets(body, out),
            StatementKind::Match { cases, .. } => {
                for c in cases {
                    untyped_lets(&c.body, out);
                }
            }
            StatementKind::Try { body, handlers } => {
                untyped_lets(body, out);
                for h in handlers {
                    untyped_lets(&h.body, out);
                }
            }
        }
    }
}

/// Type hints: the inferred type after each binding written without one (T-10).
fn inlay_hints(nav: &Navigation, m: ModId) -> Json {
    let p = &nav.p;
    let module = &p.modules[m];
    let mut names = Vec::new();
    for item in &module.analysis.file.items {
        match item {
            ast::Item::Declaration(d) => {
                if let ast::DeclarationKind::Function(f) = &d.kind {
                    untyped_lets(&f.body, &mut names);
                }
                if let ast::DeclarationKind::Interface(i) = &d.kind {
                    for (_, body) in &i.members {
                        if let Some(b) = body {
                            untyped_lets(b, &mut names);
                        }
                    }
                }
            }
            ast::Item::Extend(e) => {
                for member in &e.members {
                    if let Some(b) = &member.body {
                        untyped_lets(b, &mut names);
                    }
                }
            }
            ast::Item::Test(t) => untyped_lets(&t.body, &mut names),
            ast::Item::Import(_) => {}
        }
    }
    let source = &module.analysis.source;
    let hints = names
        .into_iter()
        .filter_map(|name| {
            let local = nav
                .r
                .locals
                .iter()
                .find(|l| l.module == m && l.span == name.span)?;
            if local.ty.is_unknown() {
                return None;
            }
            let (line, character) = super::position_of(source, name.span.end.offset);
            Some(Json::object(vec![
                (
                    "position",
                    Json::object(vec![
                        ("line", Json::int(line)),
                        ("character", Json::int(character)),
                    ]),
                ),
                ("label", Json::str(format!(" as {}", p.show(&local.ty)))),
                ("kind", Json::int(1)),
            ]))
        })
        .collect();
    Json::Array(hints)
}

fn code_block(text: &str) -> String {
    format!("```unio\n{text}\n```")
}

/// The text of a hover: the signature, type, documentation, errors and effects that `unio
/// symbol` reports (C-11).
fn describe(nav: &mut Navigation, target: &Target) -> String {
    let p = &nav.p;
    let report = |node: crate::symbol::Node| {
        let derived = crate::effects::derive(p);
        let hashes = crate::symbol::hashes(p);
        crate::symbol::report(p, &derived, &hashes, node)
    };
    match target {
        Target::Local(m, span) => {
            let local = nav
                .r
                .locals
                .iter()
                .find(|l| l.module == *m && l.span == *span);
            match local {
                Some(l) => {
                    let what = if l.param { "parameter" } else { "local" };
                    format!(
                        "{}\n\n{what}",
                        code_block(&format!("{} as {}", l.name, p.show(&l.ty)))
                    )
                }
                None => String::new(),
            }
        }
        Target::Func(f) => {
            let r = report(crate::symbol::Node::Func(*f));
            let mut out = code_block(&r.signature);
            if !r.documentation.is_empty() {
                out.push_str(&format!("\n\n{}", r.documentation));
            }
            let list = |items: &[String]| {
                if items.is_empty() {
                    "none".to_string()
                } else {
                    items.join(", ")
                }
            };
            let effects: Vec<String> = r.effects.iter().map(|e| e.to_string()).collect();
            out.push_str(&format!(
                "\n\n`{}`  \nerrors: {}  \neffects: {}  \nlayer: {}",
                r.name,
                list(&r.errors),
                list(&effects),
                r.layer
            ));
            out
        }
        Target::Def(d) => {
            let r = report(crate::symbol::Node::Def(*d));
            let mut out = code_block(&r.signature);
            if !r.documentation.is_empty() {
                out.push_str(&format!("\n\n{}", r.documentation));
            }
            out.push_str(&format!("\n\n`{}`", r.name));
            out
        }
        Target::Const(c) => {
            let r = report(crate::symbol::Node::Const(*c));
            let mut out = code_block(&r.signature);
            if !r.documentation.is_empty() {
                out.push_str(&format!("\n\n{}", r.documentation));
            }
            out
        }
        Target::Module(m) => code_block(&format!("import {}", p.modules[*m].name)),
        Target::Field(d, name) => {
            let def = &p.defs[*d];
            let field = match &def.kind {
                DefKind::Struct(fs) | DefKind::Error(fs) => fs.iter().find(|f| f.name == *name),
                _ => None,
            };
            match field {
                Some(f) => format!(
                    "{}\n\nfield of `{}`",
                    code_block(&format!(
                        "{}{} as {}",
                        if f.var { "var " } else { "" },
                        f.name,
                        p.show(&f.ty)
                    )),
                    def.name
                ),
                None => String::new(),
            }
        }
        Target::Variant(d, v) => {
            let def = &p.defs[*d];
            match &def.kind {
                DefKind::Enum(vs) => {
                    let variant = &vs[*v];
                    let fields: Vec<String> = variant
                        .fields
                        .iter()
                        .map(|(n, t)| format!("{n} as {}", p.show(t)))
                        .collect();
                    let shown = if fields.is_empty() {
                        variant.name.clone()
                    } else {
                        format!("{}({})", variant.name, fields.join(", "))
                    };
                    format!("{}\n\nvariant of `{}`", code_block(&shown), def.name)
                }
                _ => String::new(),
            }
        }
    }
}

// ---------------------------------------------------------------- completion

/// The kinds of completion items of LSP.
mod item_kind {
    pub const METHOD: i64 = 2;
    pub const FUNCTION: i64 = 3;
    pub const FIELD: i64 = 5;
    pub const VARIABLE: i64 = 6;
    pub const MODULE: i64 = 9;
    pub const KEYWORD: i64 = 14;
    pub const ENUM_MEMBER: i64 = 20;
    pub const CONSTANT: i64 = 21;
    pub const STRUCT: i64 = 22;
}

fn fn_detail(p: &Program, f: FnId) -> String {
    let def = &p.funcs[f];
    let params: Vec<String> = def
        .params
        .iter()
        .map(|a| format!("{} as {}", a.name, p.show(&a.ty)))
        .collect();
    let mut out = format!("function {}({})", def.name, params.join(", "));
    if def.ret != Ty::Unit && !def.ret.is_unknown() {
        out.push_str(&format!(" returns {}", p.show(&def.ret)));
    }
    if !def.fails.is_empty() {
        let fails: Vec<String> = def.fails.iter().map(|t| p.show(t)).collect();
        out.push_str(&format!(" fails with {}", fails.join(", ")));
    }
    out
}

struct Items {
    prefix: String,
    items: Vec<Json>,
    seen: std::collections::HashSet<String>,
}

impl Items {
    fn add(&mut self, label: &str, kind: i64, detail: String) {
        if !label.starts_with(self.prefix.as_str())
            || label.starts_with('<')
            || !self.seen.insert(label.to_string())
        {
            return;
        }
        let mut item = vec![("label", Json::str(label)), ("kind", Json::int(kind))];
        if !detail.is_empty() {
            item.push(("detail", Json::str(detail)));
        }
        self.items.push(Json::object(item));
    }

    fn symbol(&mut self, p: &Program, name: &str, symbol: Symbol) {
        match symbol {
            Symbol::Func(f) => self.add(name, item_kind::FUNCTION, fn_detail(p, f)),
            Symbol::Def(d) => self.add(name, item_kind::STRUCT, p.defs[d].name.clone()),
            Symbol::Const(_) => self.add(name, item_kind::CONSTANT, String::new()),
        }
    }
}

/// The indentation of the line that holds the offset.
fn indentation_at(text: &str, offset: usize) -> usize {
    let start = text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    text[start..].bytes().take_while(|b| *b == b' ').count()
}

/// Whether a local bound at `decl` is in scope at `offset`: Unio's blocks are indented, so the
/// binding holds until a line indented less than its own (a parameter: up to the next line as
/// little indented as its function).
fn in_scope(text: &str, decl: usize, param: bool, offset: usize) -> bool {
    if decl >= offset || offset > text.len() {
        return false;
    }
    let own = indentation_at(text, decl) + usize::from(param);
    let first_line_end = text[decl..].find('\n').map_or(text.len(), |i| decl + i);
    if offset <= first_line_end {
        return true;
    }
    let cursor_line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let cursor_indent = text[cursor_line_start..offset]
        .bytes()
        .take_while(|b| *b == b' ')
        .count();
    if cursor_indent < own {
        return false;
    }
    for line in text[first_line_end + 1..cursor_line_start].split('\n') {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.bytes().take_while(|b| *b == b' ').count();
        if indent < own {
            return false;
        }
    }
    true
}

impl Context<'_> {
    fn completion(&self) -> Json {
        let bytes = self.text.as_bytes();
        let offset = self.offset.min(bytes.len());
        let mut start = offset;
        while start > 0 && is_ident(bytes[start - 1]) {
            start -= 1;
        }
        let prefix = self.text[start..offset].to_string();
        let member = start > 0 && bytes[start - 1] == b'.';
        let (program, facts) = load(std::slice::from_ref(&self.doc));
        let mut items = Items {
            prefix,
            items: Vec::new(),
            seen: Default::default(),
        };
        if let Some(m) = module_of(&program, &self.doc) {
            let r = Recorded::new(&program, facts);
            if member {
                self.members(&program, &r, m, start - 1, &mut items);
            } else {
                self.names(&program, &r, m, offset, &mut items);
            }
        } else if !member {
            for k in crate::lexer::KEYWORDS {
                items.add(k, item_kind::KEYWORD, String::new());
            }
        }
        Json::object(vec![
            ("isIncomplete", Json::Bool(false)),
            ("items", Json::Array(items.items)),
        ])
    }

    fn names(&self, p: &Program, r: &Recorded, m: ModId, offset: usize, items: &mut Items) {
        let text = &self.text;
        let mut locals: Vec<&super::facts::Local> = r
            .locals
            .iter()
            .filter(|l| l.module == m && in_scope(text, l.span.start.offset, l.param, offset))
            .collect();
        // The innermost binding of a name first.
        locals.sort_by_key(|l| std::cmp::Reverse(l.span.start.offset));
        for l in locals {
            let detail = format!("{} as {}", l.name, p.show(&l.ty));
            items.add(&l.name, item_kind::VARIABLE, detail);
        }
        let module = &p.modules[m];
        let mut own: Vec<(&String, &Symbol)> = module.symbols.iter().collect();
        own.sort_by(|a, b| a.0.cmp(b.0));
        for (name, symbol) in own {
            items.symbol(p, name, *symbol);
        }
        let mut imports: Vec<(&String, &Binding)> = module.imports.iter().collect();
        imports.sort_by_key(|(n, _)| (*n).clone());
        for (name, binding) in imports {
            match binding {
                Binding::Module(target) => {
                    items.add(name, item_kind::MODULE, p.modules[*target].name.clone())
                }
                Binding::Symbol(_, s) => items.symbol(p, name, *s),
            }
        }
        let mut prelude: Vec<(&String, &(ModId, Symbol))> = p.prelude.iter().collect();
        prelude.sort_by_key(|(n, _)| (*n).clone());
        for (name, (_, symbol)) in prelude {
            if p.symbol_public(*symbol) {
                items.symbol(p, name, *symbol);
            }
        }
        for k in crate::lexer::KEYWORDS {
            items.add(k, item_kind::KEYWORD, String::new());
        }
    }

    /// The members after the `.` at `dot`: of a module, an enum, or a value by its type.
    fn members(&self, p: &Program, r: &Recorded, m: ModId, dot: usize, items: &mut Items) {
        let bytes = self.text.as_bytes();
        let mut word_start = dot;
        while word_start > 0 && is_ident(bytes[word_start - 1]) {
            word_start -= 1;
        }
        let word = &self.text[word_start..dot];
        let simple = !word.is_empty() && (word_start == 0 || bytes[word_start - 1] != b'.');
        let local = r
            .locals
            .iter()
            .filter(|l| {
                l.module == m
                    && l.name == word
                    && in_scope(&self.text, l.span.start.offset, l.param, dot)
            })
            .max_by_key(|l| l.span.start.offset);
        if simple && local.is_none() {
            if let Some(target) = p.module_alias(m, word) {
                let mut symbols: Vec<(&String, &Symbol)> =
                    p.modules[target].symbols.iter().collect();
                symbols.sort_by(|a, b| a.0.cmp(b.0));
                for (name, symbol) in symbols {
                    if p.symbol_public(*symbol) {
                        items.symbol(p, name, *symbol);
                    }
                }
                return;
            }
            if let Some((_, Symbol::Def(d))) = p.lookup(m, word) {
                if let DefKind::Enum(vs) = &p.defs[d].kind {
                    for v in vs {
                        items.add(&v.name, item_kind::ENUM_MEMBER, p.defs[d].name.clone());
                    }
                }
                let ty = Ty::Named(d, Vec::new());
                self.methods(p, m, &ty, true, items);
                return;
            }
        }
        // The type of the expression that ends at the dot: the longest one recorded.
        let recorded = r
            .types
            .iter()
            .filter(|((module, (_, end)), t)| *module == m && *end == dot && !t.is_unknown())
            .min_by_key(|((_, (start, _)), _)| *start)
            .map(|(_, t)| t.clone());
        let Some(ty) = recorded.or_else(|| local.map(|l| l.ty.clone())) else {
            return;
        };
        if let Ty::Named(d, _) = &ty {
            match &p.defs[*d].kind {
                DefKind::Struct(fs) | DefKind::Error(fs) => {
                    for f in fs {
                        if f.public || p.defs[*d].module == m {
                            items.add(&f.name, item_kind::FIELD, p.show(&f.ty));
                        }
                    }
                }
                DefKind::Interface(ms) => {
                    for f in ms {
                        items.add(&p.funcs[*f].name, item_kind::METHOD, fn_detail(p, *f));
                    }
                }
                DefKind::Enum(_) => {}
            }
        }
        self.methods(p, m, &ty, false, items);
    }

    /// The methods of a type, from its `extend` blocks and the interfaces they implement;
    /// `static`: those without `self`.
    fn methods(&self, p: &Program, m: ModId, ty: &Ty, static_only: bool, items: &mut Items) {
        let Some(wanted) = Program::type_key(ty) else {
            return;
        };
        let mut found: Vec<FnId> = Vec::new();
        for i in &p.impls {
            if Program::type_key(&i.target).as_deref() != Some(wanted.as_str()) {
                continue;
            }
            found.extend(i.methods.iter().copied());
            if let Some(Ty::Named(iface, _)) = &i.interface {
                if let DefKind::Interface(ms) = &p.defs[*iface].kind {
                    found.extend(ms.iter().copied());
                }
            }
        }
        found.sort_by_key(|f| p.funcs[*f].name.clone());
        for f in found {
            let def = &p.funcs[f];
            if def.self_param.is_some() == static_only || !(def.public || def.module == m) {
                continue;
            }
            items.add(&def.name, item_kind::METHOD, fn_detail(p, f));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_line_is_blanked_with_its_offsets() {
        let text = "function main():\n    let x = 1\n    x.\n";
        let at = text.find("x.").unwrap() + 2;
        let repaired = repair(text, at);
        assert_eq!(repaired.len(), text.len());
        assert!(
            !crate::check_source("", &repaired).has_errors(),
            "{repaired}"
        );
        assert!(repaired.contains("    x \n"));
    }

    #[test]
    fn scopes_follow_indentation() {
        let text = "function f(a as Int):\n    let x = 1\n    if a > 0:\n        let y = 2\n    \n    pass\n\nfunction g():\n    pass\n";
        let a = text.find("a as").unwrap();
        let x = text.find("x = 1").unwrap();
        let y = text.find("y = 2").unwrap();
        let later = text.find("    pass\n\n").unwrap() + 4;
        let other = text.rfind("pass").unwrap();
        assert!(in_scope(text, a, true, later));
        assert!(in_scope(text, x, false, later));
        assert!(!in_scope(text, y, false, later));
        assert!(!in_scope(text, a, true, other));
        assert!(!in_scope(text, x, false, other));
    }
}
