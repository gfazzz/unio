//! A stable S-expression form of the syntax tree, without spans, for `unio parse --ast`
//! and for comparing parses ("AST before = AST after" for the formatter, T-2).

use crate::ast::*;

pub fn file(f: &File) -> String {
    let mut out = String::from("(file");
    if f.unsafe_module {
        out.push_str(" unsafe-module");
    }
    for item in &f.items {
        out.push_str("\n  ");
        out.push_str(&self::item(item));
    }
    out.push_str(")\n");
    out
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn list<T>(head: &str, items: &[T], f: impl Fn(&T) -> String) -> String {
    let mut out = format!("({head}");
    for item in items {
        out.push(' ');
        out.push_str(&f(item));
    }
    out.push(')');
    out
}

fn names(items: &[Name]) -> String {
    items
        .iter()
        .map(|n| n.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub fn item(item: &Item) -> String {
    match item {
        Item::Import(i) => {
            if i.names.is_empty() {
                let alias = i
                    .alias
                    .as_ref()
                    .map(|a| format!(" (as {})", a.text))
                    .unwrap_or_default();
                format!("(import {}{alias})", names(&i.module))
            } else {
                format!(
                    "(from {} {})",
                    names(&i.module),
                    list("import", &i.names, |n| n.text.clone())
                )
            }
        }
        Item::Declaration(d) => {
            let inner = declaration(&d.kind);
            if d.public {
                format!("(public {inner})")
            } else {
                inner
            }
        }
        Item::Extend(e) => {
            let mut out = format!("(extend{} {}", generics(&e.generics), ty(&e.target));
            if let Some(i) = &e.interface {
                out.push_str(&format!(" (with {})", ty(i)));
            }
            for m in &e.members {
                out.push(' ');
                out.push_str(&member(m));
            }
            out.push(')');
            out
        }
        Item::Test(t) => format!("(test {} {})", text(&t.description), block(&t.body)),
    }
}

/// One member of an extend block.
pub fn member(m: &Member) -> String {
    let inner = match &m.body {
        Some(body) => format!("(function {} {})", signature(&m.signature), block(body)),
        None => format!("(external {})", signature(&m.signature)),
    };
    if m.public {
        format!("(public {inner})")
    } else {
        inner
    }
}

/// The head of an extend block without its members: generics, target and interface.
pub fn extend_head(e: &Extend) -> String {
    let mut out = format!("(extend{} {}", generics(&e.generics), ty(&e.target));
    if let Some(i) = &e.interface {
        out.push_str(&format!(" (with {})", ty(i)));
    }
    out.push(')');
    out
}

fn generics(g: &[GenericParam]) -> String {
    if g.is_empty() {
        return String::new();
    }
    format!(
        " {}",
        list("generics", g, |p| if p.bounds.is_empty() {
            p.name.text.clone()
        } else {
            format!("({} {})", p.name.text, list("with", &p.bounds, ty))
        })
    )
}

fn declaration(kind: &DeclarationKind) -> String {
    match kind {
        DeclarationKind::Function(f) => {
            format!("(function {} {})", signature(&f.signature), block(&f.body))
        }
        DeclarationKind::External(s) => format!("(external {})", signature(s)),
        DeclarationKind::Struct(s) => format!(
            "(struct {}{}{})",
            s.name.text,
            generics(&s.generics),
            fields(&s.fields)
        ),
        DeclarationKind::Error(e) => format!("(error {}{})", e.name.text, fields(&e.fields)),
        DeclarationKind::Enum(e) => {
            let variants = list("variants", &e.variants, |v| {
                if v.fields.is_empty() {
                    v.name.text.clone()
                } else {
                    format!(
                        "({} {})",
                        v.name.text,
                        list("fields", &v.fields, |(n, t)| format!(
                            "({} {})",
                            n.text,
                            ty(t)
                        ))
                    )
                }
            });
            format!("(enum {}{} {variants})", e.name.text, generics(&e.generics))
        }
        DeclarationKind::Interface(i) => {
            let members = list("members", &i.members, |(s, body)| match body {
                Some(b) => format!("(default {} {})", signature(s), block(b)),
                None => format!("(required {})", signature(s)),
            });
            format!(
                "(interface {}{} {members})",
                i.name.text,
                generics(&i.generics)
            )
        }
        DeclarationKind::Constant(c) => {
            let t =
                c.ty.as_ref()
                    .map(|t| format!(" {}", ty(t)))
                    .unwrap_or_default();
            format!("(let {}{t} {})", c.name.text, expr(&c.value))
        }
    }
}

fn fields(f: &Option<Vec<Field>>) -> String {
    match f {
        None => String::new(),
        Some(fields) => format!(
            " {}",
            list("fields", fields, |f| {
                let mut out = String::from("(");
                if f.public {
                    out.push_str("public ");
                }
                if f.var {
                    out.push_str("var ");
                }
                out.push_str(&format!("{} {}", f.name.text, ty(&f.ty)));
                if let Some(d) = &f.default {
                    out.push_str(&format!(" (default {})", expr(d)));
                }
                out.push(')');
                out
            })
        ),
    }
}

fn signature(s: &Signature) -> String {
    let mut out = format!(
        "{}{} {}",
        s.name.text,
        generics(&s.generics),
        params(&s.params)
    );
    if let Some(r) = &s.returns {
        out.push_str(&format!(" (returns {})", ty(r)));
    }
    if !s.fails.is_empty() {
        out.push_str(&format!(" {}", list("fails", &s.fails, ty)));
    }
    out
}

fn params(p: &[Param]) -> String {
    list("params", p, |p| match p {
        Param::SelfParam { var, .. } => {
            if *var {
                "(var self)".to_string()
            } else {
                "self".to_string()
            }
        }
        Param::Named {
            name,
            ty: t,
            default,
        } => format!("(named {} {}{})", name.text, ty(t), default_of(default)),
        Param::Plain {
            var,
            name,
            ty: t,
            default,
        } => {
            format!(
                "({}{} {}{})",
                if *var { "var " } else { "" },
                name.text,
                ty(t),
                default_of(default)
            )
        }
    })
}

fn default_of(d: &Option<Expr>) -> String {
    d.as_ref()
        .map(|e| format!(" (default {})", expr(e)))
        .unwrap_or_default()
}

pub fn ty(t: &Type) -> String {
    if t.arguments.is_empty() {
        return names(&t.path);
    }
    let args: Vec<String> = t
        .arguments
        .iter()
        .map(|a| match a {
            TypeArgument::Type(t) => ty(t),
            TypeArgument::Shape {
                params,
                returns,
                fails,
            } => {
                let mut out = list("shape", params, |(n, t)| match n {
                    Some(n) => format!("(named {} {})", n.text, ty(t)),
                    None => ty(t),
                });
                out.pop();
                if let Some(r) = returns {
                    out.push_str(&format!(" (returns {})", ty(r)));
                }
                if !fails.is_empty() {
                    out.push_str(&format!(" {}", list("fails", fails, ty)));
                }
                out.push(')');
                out
            }
        })
        .collect();
    format!("({} {})", names(&t.path), args.join(" "))
}

fn block(b: &Block) -> String {
    list("block", b, statement)
}

fn statement(s: &Statement) -> String {
    match &s.kind {
        StatementKind::Simple { statement, handler } => {
            let inner = simple(statement);
            match handler {
                Some(h) => format!("(handled {inner} {})", block(h)),
                None => inner,
            }
        }
        StatementKind::If {
            branches,
            otherwise,
        } => {
            let mut out = String::from("(if");
            for (c, b) in branches {
                out.push_str(&format!(" ({} {})", expr(c), block(b)));
            }
            if let Some(o) = otherwise {
                out.push_str(&format!(" (else {})", block(o)));
            }
            out.push(')');
            out
        }
        StatementKind::For {
            names: n,
            iterable,
            body,
        } => {
            format!(
                "(for {} {} {})",
                list("names", n, |n| n.text.clone()),
                expr(iterable),
                block(body)
            )
        }
        StatementKind::While { condition, body } => {
            format!("(while {} {})", expr(condition), block(body))
        }
        StatementKind::Match { subject, cases } => {
            let cases = cases
                .iter()
                .map(|c| {
                    let guard = c
                        .guard
                        .as_ref()
                        .map(|g| format!(" (if {})", expr(g)))
                        .unwrap_or_default();
                    format!("(case {}{guard} {})", pattern(&c.pattern), block(&c.body))
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("(match {} {cases})", expr(subject))
        }
        StatementKind::Try { body, handlers } => {
            let handlers = handlers
                .iter()
                .map(|h| {
                    let name = h
                        .name
                        .as_ref()
                        .map(|n| format!(" (as {})", n.text))
                        .unwrap_or_default();
                    format!("(on {}{name} {})", ty(&h.ty), block(&h.body))
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("(try {} {handlers})", block(body))
        }
        StatementKind::Using { name, value, body } => {
            format!("(using {} {} {})", name.text, expr(value), block(body))
        }
    }
}

fn simple(s: &Simple) -> String {
    match s {
        Simple::Let {
            var,
            name,
            ty: t,
            value,
        } => {
            let t = t
                .as_ref()
                .map(|t| format!(" {}", ty(t)))
                .unwrap_or_default();
            format!(
                "({} {}{t} {})",
                if *var { "var" } else { "let" },
                name.text,
                expr(value)
            )
        }
        Simple::Assign { target, value } => format!("(assign {} {})", expr(target), expr(value)),
        Simple::Return(None) => "(return)".to_string(),
        Simple::Return(Some(e)) => format!("(return {})", expr(e)),
        Simple::Fail(e) => format!("(fail {})", expr(e)),
        Simple::Assert(e) => format!("(assert {})", expr(e)),
        Simple::Break => "(break)".to_string(),
        Simple::Continue => "(continue)".to_string(),
        Simple::Pass => "(pass)".to_string(),
        Simple::Expr(e) => expr(e),
    }
}

fn text(parts: &[TextPart]) -> String {
    list("text", parts, |p| match p {
        TextPart::Literal(t) => quote(t),
        TextPart::Interpolation(e) => format!("(interpolation {})", expr(e)),
    })
}

fn arguments(a: &[Argument]) -> String {
    list("arguments", a, |a| match &a.name {
        Some(n) => format!("({} = {})", n.text, expr(&a.value)),
        None => expr(&a.value),
    })
}

pub fn expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(t) => format!("(int {t})"),
        ExprKind::Float(t) => format!("(float {t})"),
        ExprKind::Text(parts) => text(parts),
        ExprKind::Bool(b) => format!("({b})"),
        ExprKind::SelfValue => "(self)".to_string(),
        ExprKind::Name(n) => format!("(name {n})"),
        ExprKind::Paren(inner) => format!("(paren {})", expr(inner)),
        ExprKind::Tuple(items) => list("tuple", items, expr),
        ExprKind::List(items) => list("list", items, expr),
        ExprKind::Comprehension {
            element,
            names: n,
            iterable,
            condition,
        } => {
            let cond = condition
                .as_ref()
                .map(|c| format!(" (if {})", expr(c)))
                .unwrap_or_default();
            format!(
                "(comprehension {} {} {}{cond})",
                expr(element),
                list("names", n, |n| n.text.clone()),
                expr(iterable)
            )
        }
        ExprKind::Map(entries) => list("map", entries, |(k, v)| {
            format!("({} {})", expr(k), expr(v))
        }),
        ExprKind::Lambda {
            params: p,
            returns,
            fails,
            body,
        } => {
            let r = returns
                .as_ref()
                .map(|r| format!(" (returns {})", ty(r)))
                .unwrap_or_default();
            let f = if fails.is_empty() {
                String::new()
            } else {
                format!(" {}", list("fails", fails, ty))
            };
            let p = list("params", p, |p| {
                let head = if p.named { "named " } else { "" };
                match &p.ty {
                    Some(t) => format!("({head}{} {})", p.name.text, ty(t)),
                    None => format!("({head}{})", p.name.text),
                }
            });
            format!("(lambda {p}{r}{f} {})", expr(body))
        }
        ExprKind::Is { value, pattern: p } => format!("(is {} {})", expr(value), pattern(p)),
        ExprKind::Conditional {
            then,
            condition,
            otherwise,
        } => {
            format!(
                "(conditional {} {} {})",
                expr(then),
                expr(condition),
                expr(otherwise)
            )
        }
        ExprKind::Binary { op, left, right } => {
            format!("(binary {op} {} {})", expr(left), expr(right))
        }
        ExprKind::Unary { op, operand } => format!("(unary {op} {})", expr(operand)),
        ExprKind::Try { operand, handler } => match handler {
            None => format!("(try {})", expr(operand)),
            Some((t, name)) => {
                let n = name
                    .as_ref()
                    .map(|n| format!(" (as {})", n.text))
                    .unwrap_or_default();
                format!("(try {} (on {}{n}))", expr(operand), ty(t))
            }
        },
        ExprKind::Call {
            callee,
            arguments: a,
        } => format!("(call {} {})", expr(callee), arguments(a)),
        ExprKind::GenericCall {
            callee,
            types,
            arguments: a,
        } => {
            format!(
                "(generic-call {} {} {})",
                expr(callee),
                list("types", types, ty),
                arguments(a)
            )
        }
        ExprKind::Index { target, index } => format!("(index {} {})", expr(target), expr(index)),
        ExprKind::Field { target, name } => format!("(field {} {})", expr(target), name.text),
        ExprKind::SafeField { target, name } => {
            format!("(safe-field {} {})", expr(target), name.text)
        }
        ExprKind::Error => "(error)".to_string(),
    }
}

fn pattern(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".to_string(),
        PatternKind::Literal(Literal::Int(t)) => format!("(int {t})"),
        PatternKind::Literal(Literal::Text(parts)) => text(parts),
        PatternKind::Literal(Literal::Bool(b)) => format!("({b})"),
        PatternKind::Name(n) => format!("(name {})", names(n)),
        PatternKind::Constructor { name, arguments: a } => {
            let args = list("arguments", a, |(label, p)| match label {
                Some(l) => format!("({} = {})", l.text, pattern(p)),
                None => pattern(p),
            });
            format!("(constructor {} {args})", names(name))
        }
        PatternKind::List(elements) => list("list", elements, |e| match e {
            ListElement::Rest(n) => format!("(rest {})", n.text),
            ListElement::Pattern(p) => pattern(p),
        }),
        PatternKind::Tuple(items) => list("tuple", items, pattern),
        PatternKind::Bind {
            pattern: inner,
            name,
        } => format!("(bind {} {})", pattern(inner), name.text),
        PatternKind::Or(options) => list("or", options, pattern),
    }
}
