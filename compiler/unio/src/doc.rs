//! `unio doc` (table 6.1, L-3, L-9b): the documentation of a package as HTML pages.
//!
//! Every module of the package gets a page with its documentation comment and, in the order of
//! its source, each public declaration with its signature and documentation comment; for a
//! function also its errors, the effects the compiler derives from the call graph (RFC 0005),
//! the parameters whose calls count for the caller, and the library layer it needs, as
//! `unio symbol` reports them (C-11). The methods of an `extend` block are listed when they are
//! public or implement an interface; private declarations, private fields, and tests are left
//! out. An index page names the package and lists its modules.
//!
//! The pages are the same bytes for the same sources: they hold no time and no path of the
//! machine, only paths relative to the package. The worker of `unio doc` makes them (SB-10);
//! the tool writes them.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::ast;
use crate::diag::Span;
use crate::effects::Derived;
use crate::program::{DefKind, ModId, Owner, Program};
use crate::symbol::{self, Hashes, Node};

/// The name of the index page.
pub const INDEX: &str = "index.html";

/// The marker of a page made by `unio doc`: a page of an earlier run that carries it and is not
/// made again is removed, and no other file is touched.
pub const GENERATOR: &str = "<meta name=\"generator\" content=\"unio doc\">";

/// One page: its file name in the output directory and its text.
pub struct Page {
    pub name: String,
    pub html: String,
}

/// The source files of the package at `root`: every `.unio` file below it, except in hidden
/// directories, in `target/` of the root, and in nested packages (a directory with its own
/// `unio.toml`). Links are not followed. Sorted.
pub fn package_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if name.starts_with('.') {
                continue;
            }
            if kind.is_dir() {
                if (dir == root && name == "target") || path.join("unio.toml").exists() {
                    continue;
                }
                stack.push(path);
            } else if kind.is_file() && name.ends_with(".unio") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The pages of the package at `root`, or the analyses with errors when it does not compile.
pub fn generate(root: &Path) -> Result<Vec<Page>, Vec<crate::Analysis>> {
    let root = root.to_path_buf();
    let files = package_files(&root);
    crate::on_front_end_stack(move || {
        let program = crate::load_for_run(&files, crate::effects::Profile::Application)?;
        Ok(pages(&program, &root))
    })
}

/// The name and version of the package from its manifest, or the name of its directory.
fn title(root: &Path) -> String {
    let manifest = crate::vfs::read_to_string(&root.join("unio.toml"))
        .ok()
        .and_then(|text| crate::package::manifest::Manifest::parse(&text).ok());
    match manifest {
        Some(m) => match m.version {
            Some(v) => format!("{} {v}", m.name),
            None => m.name,
        },
        None => root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "package".into()),
    }
}

/// The file name of a module's page. A module named `index` would take the name of the index,
/// so its page gets a name no module can have.
fn page_name(module: &str) -> String {
    if module == "index" {
        "index-module.html".into()
    } else {
        format!("{module}.html")
    }
}

fn pages(p: &Program, root: &Path) -> Vec<Page> {
    let package = title(root);
    let derived = crate::effects::derive(p);
    let hashes = symbol::hashes(p);
    let mut modules: Vec<ModId> = (0..p.modules.len())
        .filter(|&m| p.modules[m].given && !p.modules[m].stdlib)
        .collect();
    modules.sort_by(|a, b| p.modules[*a].name.cmp(&p.modules[*b].name));
    let context = Context {
        p,
        derived: &derived,
        hashes: &hashes,
        root,
        package: &package,
    };
    let mut out = vec![Page {
        name: INDEX.into(),
        html: index(&context, &modules),
    }];
    for &m in &modules {
        out.push(Page {
            name: page_name(&p.modules[m].name),
            html: module_page(&context, m),
        });
    }
    out
}

struct Context<'a> {
    p: &'a Program,
    derived: &'a Derived,
    hashes: &'a Hashes,
    root: &'a Path,
    package: &'a str,
}

const STYLE: &str =
    "body{font-family:system-ui,sans-serif;line-height:1.5;margin:0;color:#1d1d1f;background:#fff}\
nav{padding:.75rem 1rem;border-bottom:1px solid #ddd}\
main{max-width:52rem;margin:0 auto;padding:1rem}\
code,pre{font-family:ui-monospace,monospace;font-size:.92em}\
pre{background:#f4f4f6;padding:.6rem .8rem;overflow-x:auto;border-radius:4px}\
section.item{border-top:1px solid #e3e3e6;margin-top:1.4rem;padding-top:.4rem}\
.kind{color:#666;font-weight:normal;font-size:.85em}\
.source,.note{color:#555;font-size:.9em}\
dl.facts{display:grid;grid-template-columns:max-content 1fr;gap:.2rem 1rem;font-size:.92em}\
dl.facts dt{color:#555}dl.facts dd{margin:0}\
@media (prefers-color-scheme:dark){body{background:#151517;color:#e6e6e9}pre{background:#232327}\
nav,section.item{border-color:#333}.kind,.source,.note,dl.facts dt{color:#a0a0a8}a{color:#8ab4f8}}";

fn head(out: &mut String, title: &str) {
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(GENERATOR);
    out.push('\n');
    let _ = writeln!(out, "<title>{}</title>", escape(title));
    let _ = writeln!(out, "<style>{STYLE}</style>\n</head>\n<body>");
}

fn index(c: &Context, modules: &[ModId]) -> String {
    let mut out = String::new();
    head(&mut out, c.package);
    out.push_str("<main>\n");
    let _ = writeln!(out, "<h1>{}</h1>", escape(c.package));
    out.push_str("<h2>Modules</h2>\n<ul>\n");
    for &m in modules {
        let module = &c.p.modules[m];
        let summary = first_sentence(&module_documentation(module));
        let _ = write!(
            out,
            "<li><a href=\"{}\"><code>{}</code></a>",
            escape(&page_name(&module.name)),
            escape(&module.name)
        );
        if !summary.is_empty() {
            let _ = write!(out, ": {}", inline(&summary));
        }
        out.push_str("</li>\n");
    }
    out.push_str("</ul>\n</main>\n</body>\n</html>\n");
    out
}

/// The `##` comment that opens a module: the comments at its start that are not the
/// documentation of the declaration right below them.
fn module_documentation(module: &crate::program::Module) -> String {
    let comments = &module.analysis.comments;
    let mut lines = Vec::new();
    let mut next = 1;
    for comment in comments {
        if comment.span.start.line != next || !comment.doc {
            break;
        }
        lines.push(doc_line(&comment.text));
        next += 1;
    }
    if lines.is_empty() {
        return String::new();
    }
    // A block right above a declaration documents that declaration.
    let attached = module
        .analysis
        .file
        .items
        .iter()
        .any(|item| item_start(item) == Some(next) && !matches!(item, ast::Item::Import(_)));
    if attached {
        return String::new();
    }
    lines.join("\n")
}

fn doc_line(text: &str) -> String {
    let text = text.trim_start_matches('#');
    text.strip_prefix(' ').unwrap_or(text).to_string()
}

fn item_start(item: &ast::Item) -> Option<usize> {
    match item {
        ast::Item::Declaration(d) => Some(d.span.start.line),
        ast::Item::Extend(e) => Some(e.span.start.line),
        ast::Item::Test(t) => Some(t.span.start.line),
        ast::Item::Import(_) => None,
    }
}

/// The first sentence of a text: up to the first `. ` or the end of its first paragraph.
fn first_sentence(text: &str) -> String {
    let paragraph = text.split("\n\n").next().unwrap_or("").replace('\n', " ");
    match paragraph.find(". ") {
        Some(i) => paragraph[..=i].to_string(),
        None => paragraph.trim().to_string(),
    }
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start.offset <= inner.start.offset && inner.end.offset <= outer.end.offset
}

/// The path of a module relative to the package, with `/`.
fn relative(c: &Context, path: &Path) -> String {
    path.strip_prefix(c.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The ids of the sections of one page, each used once.
struct Ids(HashSet<String>);

impl Ids {
    fn take(&mut self, wanted: &str) -> String {
        let mut id = wanted.to_string();
        let mut n = 2;
        while !self.0.insert(id.clone()) {
            id = format!("{wanted}-{n}");
            n += 1;
        }
        id
    }
}

fn module_page(c: &Context, m: ModId) -> String {
    let p = c.p;
    let module = &p.modules[m];
    let source = relative(c, &module.path);
    let mut body = String::new();
    let mut contents: Vec<(String, String)> = Vec::new();
    let mut ids = Ids(HashSet::new());
    for item in &module.analysis.file.items {
        match item {
            ast::Item::Declaration(d) if d.public => {
                declaration(c, m, d, &source, &mut ids, &mut contents, &mut body);
            }
            ast::Item::Extend(e) => extend(c, m, e, &source, &mut ids, &mut contents, &mut body),
            _ => {}
        }
    }
    let mut out = String::new();
    head(&mut out, &format!("{} - {}", module.name, c.package));
    let _ = writeln!(
        out,
        "<nav><a href=\"{INDEX}\">{}</a></nav>\n<main>",
        escape(c.package)
    );
    let _ = writeln!(
        out,
        "<h1>Module <code>{}</code></h1>\n<p class=\"source\">Source: <code>{}</code></p>",
        escape(&module.name),
        escape(&source)
    );
    out.push_str(&documentation(&module_documentation(module)));
    if !contents.is_empty() {
        out.push_str("<h2>Contents</h2>\n<ul>\n");
        for (id, label) in &contents {
            let _ = writeln!(
                out,
                "<li><a href=\"#{}\"><code>{}</code></a></li>",
                escape(id),
                escape(label)
            );
        }
        out.push_str("</ul>\n");
    }
    out.push_str(&body);
    out.push_str("</main>\n</body>\n</html>\n");
    out
}

/// The source text of a span, with the comment lines and, for a struct, the lines of private
/// fields left out; whether a private field was left out.
fn declaration_text(source: &str, span: Span, kind: &ast::DeclarationKind) -> (String, bool) {
    let text = source.get(span.start.offset..span.end.offset).unwrap_or("");
    let mut lines = Vec::new();
    let mut private = false;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match kind {
            ast::DeclarationKind::Interface(_) if i > 0 => break,
            ast::DeclarationKind::Struct(_) if i > 0 && !trimmed.starts_with("public ") => {
                private = true;
                continue;
            }
            _ => {}
        }
        lines.push(line.trim_end());
    }
    (lines.join("\n"), private)
}

#[allow(clippy::too_many_arguments)]
fn declaration(
    c: &Context,
    m: ModId,
    d: &ast::Declaration,
    source_path: &str,
    ids: &mut Ids,
    contents: &mut Vec<(String, String)>,
    body: &mut String,
) {
    let p = c.p;
    let (name, kind) = match &d.kind {
        ast::DeclarationKind::Function(f) => (&f.signature.name.text, "function"),
        ast::DeclarationKind::External(s) => (&s.name.text, "external function"),
        ast::DeclarationKind::Struct(s) => (&s.name.text, "struct"),
        ast::DeclarationKind::Enum(e) => (&e.name.text, "enum"),
        ast::DeclarationKind::Error(e) => (&e.name.text, "error"),
        ast::DeclarationKind::Interface(i) => (&i.name.text, "interface"),
        ast::DeclarationKind::Constant(k) => (&k.name.text, "constant"),
    };
    let node = match &d.kind {
        ast::DeclarationKind::Function(_) | ast::DeclarationKind::External(_) => (0..p.funcs.len())
            .find(|&f| {
                p.funcs[f].module == m
                    && p.funcs[f].owner == Owner::Module
                    && &p.funcs[f].name == name
            })
            .map(Node::Func),
        ast::DeclarationKind::Constant(_) => (0..p.consts.len())
            .find(|&k| p.consts[k].module == m && &p.consts[k].name == name)
            .map(Node::Const),
        _ => (0..p.defs.len())
            .find(|&t| p.defs[t].module == m && &p.defs[t].name == name)
            .map(Node::Def),
    };
    let Some(node) = node else {
        return;
    };
    let report = symbol::report(p, c.derived, c.hashes, node);
    let id = ids.take(name);
    contents.push((id.clone(), name.clone()));
    let signature = match &d.kind {
        ast::DeclarationKind::Function(_)
        | ast::DeclarationKind::External(_)
        | ast::DeclarationKind::Constant(_) => (report.signature.clone(), false),
        kind => declaration_text(&p.modules[m].analysis.source, d.span, kind),
    };
    item(
        body,
        &id,
        name,
        kind,
        &signature.0,
        signature.1,
        &report,
        &format!("{source_path}:{}", d.span.start.line),
    );
    if let Node::Def(def) = node {
        if let DefKind::Interface(methods) = &p.defs[def].kind {
            for &f in methods {
                let report = symbol::report(p, c.derived, c.hashes, Node::Func(f));
                let method = &p.funcs[f].name;
                let id = ids.take(&format!("{name}.{method}"));
                contents.push((id.clone(), format!("{name}.{method}")));
                let kind = if p.funcs[f].body.is_some() {
                    "method with a default"
                } else {
                    "method"
                };
                item(
                    body,
                    &id,
                    &format!("{name}.{method}"),
                    kind,
                    &report.signature,
                    false,
                    &report,
                    &format!("{source_path}:{}", p.funcs[f].span.start.line),
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extend(
    c: &Context,
    m: ModId,
    e: &ast::Extend,
    source_path: &str,
    ids: &mut Ids,
    contents: &mut Vec<(String, String)>,
    body: &mut String,
) {
    let p = c.p;
    let Some(impl_id) =
        (0..p.impls.len()).find(|&i| p.impls[i].module == m && contains(e.span, p.impls[i].span))
    else {
        return;
    };
    let implements = p.impls[impl_id].interface.is_some();
    let members: Vec<&ast::Member> = e
        .members
        .iter()
        .filter(|member| member.public || implements)
        .collect();
    if members.is_empty() {
        return;
    }
    let source = &p.modules[m].analysis.source;
    let head = source
        .get(e.span.start.offset..e.span.end.offset)
        .and_then(|t| t.lines().next())
        .unwrap_or("")
        .trim()
        .trim_end_matches(':')
        .to_string();
    let target = p.show(&p.impls[impl_id].target);
    let target = target.split('[').next().unwrap_or(&target).to_string();
    let id = ids.take(&format!("extend-{target}"));
    contents.push((id.clone(), head.clone()));
    let _ = writeln!(
        body,
        "<h2 id=\"{}\"><code>{}</code></h2>",
        escape(&id),
        escape(&head)
    );
    for member in members {
        let method = &member.signature.name.text;
        let Some(f) = p.impls[impl_id]
            .methods
            .iter()
            .copied()
            .find(|&f| &p.funcs[f].name == method)
        else {
            continue;
        };
        let report = symbol::report(p, c.derived, c.hashes, Node::Func(f));
        let label = format!("{target}.{method}");
        let id = ids.take(&label);
        let kind = if member.external {
            "external method"
        } else {
            "method"
        };
        item(
            body,
            &id,
            &label,
            kind,
            &report.signature,
            false,
            &report,
            &format!("{source_path}:{}", member.signature.span.start.line),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn item(
    out: &mut String,
    id: &str,
    name: &str,
    kind: &str,
    signature: &str,
    private_fields: bool,
    report: &symbol::Report,
    declared: &str,
) {
    let _ = writeln!(
        out,
        "<section class=\"item\" id=\"{}\">\n<h3><code>{}</code> <span class=\"kind\">{}</span></h3>",
        escape(id),
        escape(name),
        escape(kind)
    );
    let _ = writeln!(
        out,
        "<pre class=\"signature\"><code>{}</code></pre>",
        escape(signature)
    );
    if private_fields {
        out.push_str(
            "<p class=\"note\">Private fields: the type is built by the functions of its module.</p>\n",
        );
    }
    out.push_str(&documentation(&report.documentation));
    out.push_str("<dl class=\"facts\">\n");
    if report.kind == "function" {
        fact(out, "Errors", &report.errors);
        let effects: Vec<String> = report.effects.iter().map(|e| e.to_string()).collect();
        fact(out, "Effects", &effects);
        if !report.argument_effects.is_empty() {
            fact(out, "Effects through arguments", &report.argument_effects);
        }
        fact(out, "Layer", &[report.layer.to_string()]);
    }
    let _ = writeln!(
        out,
        "<dt>Declared</dt><dd><code>{}</code></dd>",
        escape(declared)
    );
    out.push_str("</dl>\n</section>\n");
}

fn fact(out: &mut String, name: &str, items: &[String]) {
    let value = if items.is_empty() {
        "none".to_string()
    } else {
        items
            .iter()
            .map(|i| format!("<code>{}</code>", escape(i)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(out, "<dt>{name}</dt><dd>{value}</dd>");
}

/// A documentation comment as HTML: paragraphs separated by empty lines, and lines indented by
/// four spaces as code, as the documentation tests read them; `code` in backticks.
pub fn documentation(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let mut out = String::from("<div class=\"doc\">\n");
    let mut paragraph: Vec<&str> = Vec::new();
    let mut code: Vec<&str> = Vec::new();
    let flush_paragraph = |out: &mut String, paragraph: &mut Vec<&str>| {
        if !paragraph.is_empty() {
            let _ = writeln!(out, "<p>{}</p>", inline(&paragraph.join(" ")));
            paragraph.clear();
        }
    };
    let flush_code = |out: &mut String, code: &mut Vec<&str>| {
        while code.last().is_some_and(|l| l.trim().is_empty()) {
            code.pop();
        }
        if !code.is_empty() {
            let _ = writeln!(out, "<pre><code>{}</code></pre>", escape(&code.join("\n")));
            code.clear();
        }
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("    ") {
            flush_paragraph(&mut out, &mut paragraph);
            code.push(rest);
        } else if line.trim().is_empty() {
            if code.is_empty() {
                flush_paragraph(&mut out, &mut paragraph);
            } else {
                code.push("");
            }
        } else {
            flush_code(&mut out, &mut code);
            paragraph.push(line.trim());
        }
    }
    flush_paragraph(&mut out, &mut paragraph);
    flush_code(&mut out, &mut code);
    out.push_str("</div>\n");
    out
}

/// A line of prose: escaped, with `code` in backticks.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut code = false;
    let pieces: Vec<&str> = text.split('`').collect();
    // An unmatched backtick stays a backtick.
    let pairs = pieces.len() % 2 == 1;
    for (i, part) in pieces.iter().enumerate() {
        if i > 0 {
            if pairs {
                out.push_str(if code { "</code>" } else { "<code>" });
                code = !code;
            } else {
                out.push('`');
            }
        }
        out.push_str(&escape(part));
    }
    out
}

/// Text escaped for HTML.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documentation_has_paragraphs_code_and_escapes() {
        let html = documentation("First line\nsame paragraph.\n\nExample:\n    let a = 1 < 2\n\n    print(a)\n\nAfter `x & y`.");
        assert_eq!(
            html,
            "<div class=\"doc\">\n<p>First line same paragraph.</p>\n<p>Example:</p>\n<pre><code>let a = 1 &lt; 2\n\nprint(a)</code></pre>\n<p>After <code>x &amp; y</code>.</p>\n</div>\n"
        );
        assert_eq!(inline("a ` b"), "a ` b");
        assert_eq!(documentation("  \n"), "");
    }

    #[test]
    fn the_first_sentence_ends_at_a_full_stop_or_the_paragraph() {
        assert_eq!(first_sentence("One. Two."), "One.");
        assert_eq!(first_sentence("One\nline.\n\nTwo."), "One line.");
    }

    #[test]
    fn a_module_named_index_does_not_replace_the_index() {
        assert_eq!(page_name("index"), "index-module.html");
        assert_eq!(page_name("util.text"), "util.text.html");
    }
}
