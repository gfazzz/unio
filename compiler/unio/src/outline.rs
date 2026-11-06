//! `unio outline`: the declarations of a file with their signatures and lines (C-12).
//! Signatures are written as the formatter writes them, on one line.

use crate::ast::*;
use crate::format::{flat, modifiers, Printer};

/// One entry of the outline.
pub struct Entry {
    pub kind: &'static str,
    pub name: String,
    pub signature: String,
    pub line: usize,
    pub members: Vec<Entry>,
}

pub fn outline(file: &File) -> Vec<Entry> {
    let p = Printer { source: None };
    let mut entries = Vec::new();
    for item in &file.items {
        match item {
            Item::Import(_) => {}
            Item::Test(t) => {
                let description = flat(&p.expr(&Expr {
                    kind: ExprKind::Text(t.description.clone()),
                    span: Default::default(),
                }));
                entries.push(Entry {
                    kind: "test",
                    name: description.trim_matches('"').to_string(),
                    signature: format!("test {description}"),
                    line: t.span.start.line,
                    members: Vec::new(),
                });
            }
            Item::Extend(e) => entries.push(Entry {
                kind: "extend",
                name: p.ty(&e.target),
                signature: p.extend_header(e),
                line: e.span.start.line,
                members: e
                    .members
                    .iter()
                    .map(|m| Entry {
                        kind: "method",
                        name: m.signature.name.text.clone(),
                        signature: format!(
                            "{}{}",
                            modifiers(m.public, m.external),
                            flat(&p.signature(&m.signature))
                        ),
                        line: m.signature.span.start.line,
                        members: Vec::new(),
                    })
                    .collect(),
            }),
            Item::Declaration(d) => entries.push(declaration(&p, d)),
        }
    }
    entries
}

fn fields(p: &Printer, fields: &Option<Vec<Field>>) -> Vec<Entry> {
    fields
        .iter()
        .flatten()
        .map(|f| Entry {
            kind: "field",
            name: f.name.text.clone(),
            signature: flat(&p.field(f)),
            line: f.name.span.start.line,
            members: Vec::new(),
        })
        .collect()
}

fn declaration(p: &Printer, d: &Declaration) -> Entry {
    let public = if d.public { "public " } else { "" };
    let line = d.span.start.line;
    let (kind, name, signature, members) = match &d.kind {
        DeclarationKind::Function(f) => (
            "function",
            f.signature.name.text.clone(),
            format!("{public}{}", flat(&p.signature(&f.signature))),
            Vec::new(),
        ),
        DeclarationKind::External(s) => (
            "function",
            s.name.text.clone(),
            format!("{public}external {}", flat(&p.signature(s))),
            Vec::new(),
        ),
        DeclarationKind::Struct(s) => (
            "struct",
            s.name.text.clone(),
            format!("{public}struct {}{}", s.name.text, p.generics(&s.generics)),
            fields(p, &s.fields),
        ),
        DeclarationKind::Error(e) => (
            "error",
            e.name.text.clone(),
            format!("{public}error {}", e.name.text),
            fields(p, &e.fields),
        ),
        DeclarationKind::Enum(e) => (
            "enum",
            e.name.text.clone(),
            format!("{public}enum {}{}", e.name.text, p.generics(&e.generics)),
            e.variants
                .iter()
                .map(|v| Entry {
                    kind: "variant",
                    name: v.name.text.clone(),
                    signature: p.variant(v),
                    line: v.name.span.start.line,
                    members: Vec::new(),
                })
                .collect(),
        ),
        DeclarationKind::Interface(i) => (
            "interface",
            i.name.text.clone(),
            format!(
                "{public}interface {}{}",
                i.name.text,
                p.generics(&i.generics)
            ),
            i.members
                .iter()
                .map(|(s, _)| Entry {
                    kind: "method",
                    name: s.name.text.clone(),
                    signature: flat(&p.signature(s)),
                    line: s.span.start.line,
                    members: Vec::new(),
                })
                .collect(),
        ),
        DeclarationKind::Constant(c) => {
            let ty =
                c.ty.as_ref()
                    .map(|t| format!(" as {}", p.ty(t)))
                    .unwrap_or_default();
            (
                "constant",
                c.name.text.clone(),
                format!("{public}let {}{ty}", c.name.text),
                Vec::new(),
            )
        }
    };
    Entry {
        kind,
        name,
        signature,
        line,
        members,
    }
}

/// The text form: one line per declaration, members indented.
pub fn outline_text(file: &File) -> String {
    let mut out = String::new();
    for e in outline(file) {
        out.push_str(&format!("{}: {}\n", e.line, e.signature));
        for m in &e.members {
            out.push_str(&format!("    {}: {}\n", m.line, m.signature));
        }
    }
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn entry_json(e: &Entry, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let members: Vec<String> = e
        .members
        .iter()
        .map(|m| entry_json(m, indent + 4))
        .collect();
    let members = if members.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n{}\n{pad}  ]", members.join(",\n"))
    };
    format!(
        "{pad}{{\n{pad}  \"kind\": {},\n{pad}  \"name\": {},\n{pad}  \"signature\": {},\n{pad}  \"line\": {},\n{pad}  \"members\": {members}\n{pad}}}",
        json_string(e.kind),
        json_string(&e.name),
        json_string(&e.signature),
        e.line
    )
}

/// The JSON form: an array of objects with kind, name, signature, line, and members.
pub fn outline_json(file: &File) -> String {
    let entries: Vec<String> = outline(file).iter().map(|e| entry_json(e, 2)).collect();
    if entries.is_empty() {
        "[]\n".to_string()
    } else {
        format!("[\n{}\n]\n", entries.join(",\n"))
    }
}
