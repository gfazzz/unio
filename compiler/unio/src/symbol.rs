//! `unio symbol` (C-11): the signature, documentation, declaration site, usages, error set,
//! derived effects, needed layer and called functions of a declaration, and the content
//! hashes of every function and type (C-11a, C-11b).
//!
//! A content hash is SHA-384 over the canonical form of the declaration (the S-expression
//! of its syntax tree, which has no spans, comments or formatting) together with the
//! hashes of everything it references. Declarations that reference each other are hashed
//! as one strongly connected component: its members are sorted by identity, references
//! inside the component are written as identities and references outside it as their
//! hashes; a member's hash binds the component hash and the member's identity.

use std::collections::{BTreeSet, HashMap};

use crate::ast;
use crate::diag::Span;
use crate::effects::{self, Derived};
use crate::program::{DefKind, FnId, ModId, Owner, Program, Symbol, LAYER_NAMES};
use crate::types::{DefId, Ty};

/// The version of the hashing scheme; it is part of every hash (C-11a).
pub const HASH_SCHEME: &str = "unio-hash-v1";

/// A declaration that has a content hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    Func(FnId),
    Def(DefId),
    Const(usize),
}

/// The name a person writes for a function: `std.files.read_text`, `List.map`, `main`.
pub fn qualified_name(p: &Program, f: FnId) -> String {
    let def = &p.funcs[f];
    let mut out = module_prefix(p, def.module);
    match def.owner {
        Owner::Module => {}
        Owner::Impl(i) => {
            let target = p.show(&p.impls[i].target);
            let target = target.split('[').next().unwrap_or(&target).to_string();
            out.push_str(&target);
            out.push('.');
        }
        Owner::Interface(d) => {
            out.push_str(&p.defs[d].name);
            out.push('.');
        }
    }
    out.push_str(&def.name);
    out
}

fn module_prefix(p: &Program, m: ModId) -> String {
    if p.modules[m].prelude {
        String::new()
    } else {
        format!("{}.", p.modules[m].name)
    }
}

pub fn node_name(p: &Program, n: Node) -> String {
    match n {
        Node::Func(f) => qualified_name(p, f),
        Node::Def(d) => format!("{}{}", module_prefix(p, p.defs[d].module), p.defs[d].name),
        Node::Const(c) => format!(
            "{}{}",
            module_prefix(p, p.consts[c].module),
            p.consts[c].name
        ),
    }
}

fn node_module(p: &Program, n: Node) -> ModId {
    match n {
        Node::Func(f) => p.funcs[f].module,
        Node::Def(d) => p.defs[d].module,
        Node::Const(c) => p.consts[c].module,
    }
}

fn node_span(p: &Program, n: Node) -> Span {
    match n {
        Node::Func(f) => p.funcs[f].span,
        Node::Def(d) => p.defs[d].span,
        Node::Const(c) => p.consts[c].span,
    }
}

/// The identity used inside hashes: the module and the qualified name.
fn identity(p: &Program, n: Node) -> String {
    format!("{}|{}", p.modules[node_module(p, n)].name, node_name(p, n))
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start.offset <= inner.start.offset && inner.end.offset <= outer.end.offset
}

/// The item of the module that declares the node, and the extend member for a method.
fn declaration_of(p: &Program, n: Node) -> Option<(&ast::Item, Option<&ast::Member>)> {
    let span = node_span(p, n);
    let items = &p.modules[node_module(p, n)].analysis.file.items;
    for item in items {
        match item {
            ast::Item::Declaration(d) if contains(d.span, span) => return Some((item, None)),
            ast::Item::Extend(e) if contains(e.span, span) => {
                let member = e
                    .members
                    .iter()
                    .find(|m| contains(m.signature.span, span) || m.signature.name.span == span);
                return Some((item, member));
            }
            _ => {}
        }
    }
    None
}

/// The signature of a method declared inside an interface.
fn interface_member(p: &Program, n: Node) -> Option<&ast::Signature> {
    let Node::Func(f) = n else {
        return None;
    };
    if !matches!(p.funcs[f].owner, Owner::Interface(_)) {
        return None;
    }
    let Some((ast::Item::Declaration(d), _)) = declaration_of(p, n) else {
        return None;
    };
    let ast::DeclarationKind::Interface(i) = &d.kind else {
        return None;
    };
    let span = p.funcs[f].span;
    i.members
        .iter()
        .map(|(sig, _)| sig)
        .find(|sig| sig.name.span == span || contains(sig.span, span))
}

/// The canonical form of a declaration: independent of formatting and comments.
fn canonical(p: &Program, n: Node) -> String {
    match declaration_of(p, n) {
        Some((ast::Item::Extend(e), Some(m))) => {
            format!(
                "(member {} {})",
                crate::sexpr::extend_head(e),
                crate::sexpr::member(m)
            )
        }
        Some((item, _)) => {
            let mut text = crate::sexpr::item(item);
            if let Node::Func(f) = n {
                // A method of an interface is hashed with its interface and its name.
                text.push_str(&format!(" (method {})", p.funcs[f].name));
            }
            text
        }
        None => format!("(unknown {})", node_name(p, n)),
    }
}

/// The atoms of an S-expression outside quoted texts.
fn atoms(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'(' | b')' | b' ' => i += 1,
            _ => {
                let start = i;
                while i < bytes.len() && !matches!(bytes[i], b'(' | b')' | b' ' | b'"') {
                    i += 1;
                }
                out.push(&text[start..i]);
            }
        }
    }
    out
}

fn symbol_node(s: Symbol) -> Node {
    match s {
        Symbol::Func(f) => Node::Func(f),
        Symbol::Def(d) => Node::Def(d),
        Symbol::Const(c) => Node::Const(c),
    }
}

/// The content hashes of every function, type and constant, and the references found.
pub struct Hashes {
    pub hash: HashMap<Node, String>,
    pub references: HashMap<Node, BTreeSet<Node>>,
}

pub fn all_nodes(p: &Program) -> Vec<Node> {
    let mut nodes: Vec<Node> = (0..p.funcs.len()).map(Node::Func).collect();
    nodes.extend((0..p.defs.len()).map(Node::Def));
    nodes.extend((0..p.consts.len()).map(Node::Const));
    nodes
}

/// What each declaration references: the names in its canonical form that resolve to
/// declarations, names reached through a module alias, and the calls found by the checks.
/// A local name that is spelled like a declaration adds an edge too; that can only make a
/// hash change more often, never less.
fn references(
    p: &Program,
    nodes: &[Node],
    texts: &HashMap<Node, String>,
) -> HashMap<Node, BTreeSet<Node>> {
    let mut refs: HashMap<Node, BTreeSet<Node>> = HashMap::new();
    for &n in nodes {
        let module = node_module(p, n);
        let words = atoms(&texts[&n]);
        let mut set = BTreeSet::new();
        for word in &words {
            if !word.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                continue;
            }
            if let Some((_, s)) = p.lookup(module, word) {
                set.insert(symbol_node(s));
            }
            if let Some(alias) = p.module_alias(module, word) {
                for other in &words {
                    if let Some(s) = p.modules[alias].symbols.get(*other) {
                        set.insert(symbol_node(*s));
                    }
                }
            }
        }
        refs.insert(n, set);
    }
    for call in &p.calls {
        refs.entry(Node::Func(call.caller))
            .or_default()
            .insert(Node::Func(call.callee));
    }
    refs
}

/// Strongly connected components in an order where every component comes after the
/// components it references (Tarjan).
fn components(nodes: &[Node], refs: &HashMap<Node, BTreeSet<Node>>) -> Vec<Vec<Node>> {
    struct State<'a> {
        refs: &'a HashMap<Node, BTreeSet<Node>>,
        index: HashMap<Node, usize>,
        low: HashMap<Node, usize>,
        stack: Vec<Node>,
        on_stack: std::collections::HashSet<Node>,
        next: usize,
        out: Vec<Vec<Node>>,
    }
    fn visit(s: &mut State, v: Node) {
        s.index.insert(v, s.next);
        s.low.insert(v, s.next);
        s.next += 1;
        s.stack.push(v);
        s.on_stack.insert(v);
        let targets: Vec<Node> = s
            .refs
            .get(&v)
            .map(|r| r.iter().copied().collect())
            .unwrap_or_default();
        for w in targets {
            if !s.index.contains_key(&w) {
                visit(s, w);
                let low = s.low[&v].min(s.low[&w]);
                s.low.insert(v, low);
            } else if s.on_stack.contains(&w) {
                let low = s.low[&v].min(s.index[&w]);
                s.low.insert(v, low);
            }
        }
        if s.low[&v] == s.index[&v] {
            let mut component = Vec::new();
            while let Some(w) = s.stack.pop() {
                s.on_stack.remove(&w);
                component.push(w);
                if w == v {
                    break;
                }
            }
            s.out.push(component);
        }
    }
    let mut s = State {
        refs,
        index: HashMap::new(),
        low: HashMap::new(),
        stack: Vec::new(),
        on_stack: Default::default(),
        next: 0,
        out: Vec::new(),
    };
    for &n in nodes {
        if !s.index.contains_key(&n) {
            visit(&mut s, n);
        }
    }
    s.out
}

/// Appends a field with its length, so that no two sequences of fields encode alike.
fn field(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u64).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn digest(domain: &str, body: &[u8]) -> String {
    let mut input = Vec::new();
    field(&mut input, HASH_SCHEME);
    field(&mut input, domain);
    input.extend_from_slice(body);
    format!("{HASH_SCHEME}:sha384:{}", crate::sha::sha384_hex(&input))
}

pub fn hashes(p: &Program) -> Hashes {
    let nodes = all_nodes(p);
    let texts: HashMap<Node, String> = nodes.iter().map(|&n| (n, canonical(p, n))).collect();
    let refs = references(p, &nodes, &texts);
    let mut hash: HashMap<Node, String> = HashMap::new();
    for component in components(&nodes, &refs) {
        let mut members: Vec<(String, Node)> =
            component.iter().map(|&n| (identity(p, n), n)).collect();
        members.sort();
        let inside: std::collections::HashSet<Node> = component.iter().copied().collect();
        let mut body = Vec::new();
        for (id, n) in &members {
            field(&mut body, "member");
            field(&mut body, id);
            field(&mut body, &texts[n]);
            let mut internal = BTreeSet::new();
            let mut external = BTreeSet::new();
            for r in refs.get(n).into_iter().flatten() {
                if inside.contains(r) {
                    internal.insert(identity(p, *r));
                } else if let Some(h) = hash.get(r) {
                    external.insert((identity(p, *r), h.clone()));
                }
            }
            for id in internal {
                field(&mut body, "internal");
                field(&mut body, &id);
            }
            for (id, h) in external {
                field(&mut body, "external");
                field(&mut body, &id);
                field(&mut body, &h);
            }
        }
        let component_hash = digest("component", &body);
        for (id, n) in members {
            let mut body = Vec::new();
            field(&mut body, &component_hash);
            field(&mut body, &id);
            hash.insert(n, digest("member", &body));
        }
    }
    Hashes {
        hash,
        references: refs,
    }
}

/// The content hash of a `test` block of module `m` (C-11a), which `unio test --json` reports
/// for every test: SHA-384 over the canonical form of the block, as for a declaration, with
/// the hashes of every declaration it names or calls. Formatting and comments do not change
/// it; a change of the test or of anything it references does.
pub fn test_hash(p: &Program, hashes: &Hashes, m: ModId, test: &ast::Test) -> String {
    let text = crate::sexpr::item(&ast::Item::Test(test.clone()));
    let words = atoms(&text);
    let mut referenced: BTreeSet<Node> = BTreeSet::new();
    for word in &words {
        if !word.starts_with(|c: char| c.is_alphabetic() || c == '_') {
            continue;
        }
        if let Some((_, s)) = p.lookup(m, word) {
            referenced.insert(symbol_node(s));
        }
        if let Some(alias) = p.module_alias(m, word) {
            for other in &words {
                if let Some(s) = p.modules[alias].symbols.get(*other) {
                    referenced.insert(symbol_node(*s));
                }
            }
        }
    }
    for (f, module, span) in &p.usages {
        if *module == m && contains(test.span, *span) {
            referenced.insert(Node::Func(*f));
        }
    }
    let external: BTreeSet<(String, String)> = referenced
        .into_iter()
        .filter_map(|n| Some((identity(p, n), hashes.hash.get(&n)?.clone())))
        .collect();
    let mut body = Vec::new();
    field(&mut body, &p.modules[m].name);
    field(&mut body, &text);
    for (id, h) in external {
        field(&mut body, "external");
        field(&mut body, &id);
        field(&mut body, &h);
    }
    digest("test", &body)
}

/// The declarations whose name is the query: a full name such as `std.files.read_text` or
/// `examples.wc.count`, a method such as `List.map`, or a prelude name such as `assert_fail`.
pub fn find(p: &Program, query: &str) -> Vec<Node> {
    all_nodes(p)
        .into_iter()
        .filter(|&n| node_name(p, n) == query)
        .collect()
}

fn site(p: &Program, m: ModId, span: Span) -> String {
    format!(
        "{}:{}:{}",
        p.modules[m].analysis.path, span.start.line, span.start.column
    )
}

/// The `##` comments right above a declaration.
fn documentation(p: &Program, n: Node) -> String {
    let module = &p.modules[node_module(p, n)];
    let Some(start) = (match (interface_member(p, n), declaration_of(p, n)) {
        (Some(sig), _) => Some(sig.span.start.line),
        (_, Some((ast::Item::Extend(_), Some(m)))) => Some(m.signature.span.start.line),
        (_, Some((ast::Item::Declaration(d), _))) => Some(d.span.start.line),
        _ => None,
    }) else {
        return String::new();
    };
    let mut lines = Vec::new();
    let mut want = start;
    for comment in module.analysis.comments.iter().rev() {
        if comment.doc && comment.span.start.line + 1 == want {
            let text = comment.text.trim_start_matches('#');
            lines.push(text.strip_prefix(' ').unwrap_or(text).to_string());
            want = comment.span.start.line;
        }
    }
    lines.reverse();
    lines.join("\n")
}

/// The signature as the source writes it: the first line of the declaration.
fn signature(p: &Program, n: Node) -> String {
    let module = &p.modules[node_module(p, n)];
    let span = match (interface_member(p, n), declaration_of(p, n)) {
        // A method of an interface: its own signature, not the whole interface.
        (Some(sig), _) => sig.span,
        (_, Some((ast::Item::Extend(_), Some(m)))) => m.signature.span,
        (_, Some((ast::Item::Declaration(d), _))) => match &d.kind {
            ast::DeclarationKind::Function(f) => f.signature.span,
            ast::DeclarationKind::External(s) => s.span,
            _ => d.span,
        },
        _ => node_span(p, n),
    };
    let source = &module.analysis.source;
    let text = source.get(span.start.offset..span.end.offset).unwrap_or("");
    let text = if matches!(n, Node::Func(_)) {
        text
    } else {
        text.lines().next().unwrap_or("")
    };
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
}

/// The implementation of one method of an interface that a type uses (L-6c).
pub struct MethodImplementation {
    /// The name of the method in the interface.
    pub name: String,
    /// The function that runs: the type's own method, or the default of the interface.
    pub function: String,
    pub declared: String,
    /// The type uses the default of the interface.
    pub default: bool,
}

/// An interface that a type implements in an `extend ... with` block (L-6c).
pub struct Implementation {
    pub interface: String,
    /// The `extend` block.
    pub declared: String,
    pub methods: Vec<MethodImplementation>,
}

/// The interface of an impl, when it implements one, and whether its target is the type.
fn implements(p: &Program, impl_id: usize, d: DefId) -> Option<DefId> {
    let i = &p.impls[impl_id];
    match (&i.target, &i.interface) {
        (Ty::Named(target, _), Some(Ty::Named(interface, _))) if *target == d => Some(*interface),
        _ => None,
    }
}

/// The function that a type uses for a method of an interface it implements in an impl: its
/// own method of that name, or the default of the interface; and whether it is the default.
fn implementation_of(p: &Program, impl_id: usize, method: FnId) -> Option<(FnId, bool)> {
    let name = &p.funcs[method].name;
    if let Some(own) = p.impls[impl_id]
        .methods
        .iter()
        .copied()
        .find(|f| &p.funcs[*f].name == name)
    {
        return Some((own, false));
    }
    p.funcs[method].body.map(|_| (method, true))
}

fn interface_methods(p: &Program, d: DefId) -> &[FnId] {
    match &p.defs[d].kind {
        DefKind::Interface(methods) => methods,
        _ => &[],
    }
}

/// The interfaces a type implements, with the implementation of each method it uses.
fn implementations(p: &Program, d: DefId) -> Vec<Implementation> {
    let mut out = Vec::new();
    for impl_id in 0..p.impls.len() {
        let Some(interface) = implements(p, impl_id, d) else {
            continue;
        };
        let methods = interface_methods(p, interface)
            .iter()
            .filter_map(|&m| {
                let (f, default) = implementation_of(p, impl_id, m)?;
                Some(MethodImplementation {
                    name: p.funcs[m].name.clone(),
                    function: qualified_name(p, f),
                    declared: site(p, p.funcs[f].module, p.funcs[f].span),
                    default,
                })
            })
            .collect();
        let i = &p.impls[impl_id];
        out.push(Implementation {
            interface: node_name(p, Node::Def(interface)),
            declared: site(p, i.module, i.span),
            methods,
        });
    }
    out
}

/// The defaults of interfaces that the query names through a type that uses them, such as
/// `M.Square.name` for the default `name` of an interface that `Square` implements without
/// replacing it.
fn defaults_through_types(p: &Program, query: &str) -> Vec<FnId> {
    let Some((type_name, method)) = query.rsplit_once('.') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for d in 0..p.defs.len() {
        if node_name(p, Node::Def(d)) != type_name {
            continue;
        }
        for impl_id in 0..p.impls.len() {
            let Some(interface) = implements(p, impl_id, d) else {
                continue;
            };
            for &m in interface_methods(p, interface) {
                if p.funcs[m].name == method
                    && implementation_of(p, impl_id, m) == Some((m, true))
                    && !out.contains(&m)
                {
                    out.push(m);
                }
            }
        }
    }
    out
}

/// Everything `unio symbol` reports about one declaration.
pub struct Report {
    pub name: String,
    pub kind: &'static str,
    pub signature: String,
    pub documentation: String,
    pub declared: String,
    pub usages: Vec<String>,
    pub errors: Vec<String>,
    pub effects: Vec<&'static str>,
    /// The parameters whose calls count for the caller, with the effects of the argument
    /// (RFC 0005).
    pub argument_effects: Vec<String>,
    pub layer: &'static str,
    pub calls: Vec<String>,
    pub hash: String,
    /// For a type: the interfaces it implements and the implementation of each method (L-6c).
    pub implementations: Vec<Implementation>,
    /// For a method that a type gets from an interface, named through the type: the default
    /// of the interface that it is (L-6c); empty otherwise.
    pub default_of: String,
}

pub fn report(p: &Program, derived: &Derived, hashes: &Hashes, n: Node) -> Report {
    let module = node_module(p, n);
    let mut usages: Vec<String> = Vec::new();
    let mut argument_effects = Vec::new();
    let (kind, errors, effects, layer, calls) = match n {
        Node::Func(f) => {
            let mut seen = Vec::new();
            for (g, m, span) in &p.usages {
                if *g == f && !seen.contains(&(m, span.start.offset)) {
                    seen.push((m, span.start.offset));
                    usages.push(site(p, *m, *span));
                }
            }
            let calls: BTreeSet<String> = p
                .calls
                .iter()
                .filter(|c| c.caller == f)
                .map(|c| qualified_name(p, c.callee))
                .collect();
            argument_effects = derived.argument_effects(p, f);
            (
                "function",
                p.funcs[f].fails.iter().map(|t| p.show(t)).collect(),
                derived.effect_names(f),
                LAYER_NAMES[derived.layer[f]],
                calls.into_iter().collect(),
            )
        }
        Node::Def(_) | Node::Const(_) => {
            for (other, refs) in &hashes.references {
                if *other != n && refs.contains(&n) {
                    usages.push(site(p, node_module(p, *other), node_span(p, *other)));
                }
            }
            let kind = if matches!(n, Node::Def(_)) {
                "type"
            } else {
                "constant"
            };
            let layer = LAYER_NAMES[p.module_layer(module).unwrap_or(0)];
            (kind, Vec::new(), Vec::new(), layer, Vec::new())
        }
    };
    usages.sort();
    usages.dedup();
    Report {
        name: node_name(p, n),
        kind,
        signature: signature(p, n),
        documentation: documentation(p, n),
        declared: site(p, module, node_span(p, n)),
        usages,
        errors,
        effects,
        argument_effects,
        layer,
        calls,
        hash: hashes.hash.get(&n).cloned().unwrap_or_default(),
        implementations: match n {
            Node::Def(d) => implementations(p, d),
            _ => Vec::new(),
        },
        default_of: String::new(),
    }
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

impl Report {
    pub fn text(&self) -> String {
        let mut out = format!("{} ({})\n", self.name, self.kind);
        out.push_str(&format!("  signature: {}\n", self.signature));
        out.push_str(&format!("  declared: {}\n", self.declared));
        if !self.default_of.is_empty() {
            out.push_str(&format!("  default of: {}\n", self.default_of));
        }
        if !self.documentation.is_empty() {
            out.push_str("  documentation:\n");
            for line in self.documentation.lines() {
                out.push_str(&format!("    {line}\n"));
            }
        }
        if self.kind == "function" {
            out.push_str(&format!("  errors: {}\n", list(&self.errors)));
            let effects: Vec<String> = self.effects.iter().map(|e| e.to_string()).collect();
            out.push_str(&format!("  effects: {}\n", list(&effects)));
            if !self.argument_effects.is_empty() {
                // Calls through these parameters count for the caller, with the effects of the
                // value it passes (RFC 0005).
                out.push_str(&format!(
                    "  argument effects: {}\n",
                    self.argument_effects.join(", ")
                ));
            }
            out.push_str(&format!("  layer: {}\n", self.layer));
            out.push_str(&format!("  calls: {}\n", list(&self.calls)));
        }
        for i in &self.implementations {
            out.push_str(&format!("  implements: {} ({})\n", i.interface, i.declared));
            for m in &i.methods {
                let default = if m.default { "default, " } else { "" };
                out.push_str(&format!(
                    "    {}: {} ({default}{})\n",
                    m.name, m.function, m.declared
                ));
            }
        }
        out.push_str(&format!("  usages: {}\n", self.usages.len()));
        for usage in &self.usages {
            out.push_str(&format!("    {usage}\n"));
        }
        out.push_str(&format!("  hash: {}\n", self.hash));
        out
    }

    pub fn json(&self) -> String {
        let strings = |items: &[String]| {
            let quoted: Vec<String> = items.iter().map(|s| json_string(s)).collect();
            format!("[{}]", quoted.join(","))
        };
        let effects: Vec<String> = self.effects.iter().map(|e| e.to_string()).collect();
        let implementations: Vec<String> = self
            .implementations
            .iter()
            .map(|i| {
                let methods: Vec<String> = i
                    .methods
                    .iter()
                    .map(|m| {
                        format!(
                            "{{\"name\":{},\"function\":{},\"declared\":{},\"default\":{}}}",
                            json_string(&m.name),
                            json_string(&m.function),
                            json_string(&m.declared),
                            m.default
                        )
                    })
                    .collect();
                format!(
                    "{{\"interface\":{},\"declared\":{},\"methods\":[{}]}}",
                    json_string(&i.interface),
                    json_string(&i.declared),
                    methods.join(",")
                )
            })
            .collect();
        format!(
            "{{\"name\":{},\"kind\":{},\"signature\":{},\"documentation\":{},\"declared\":{},\"default_of\":{},\"usages\":{},\"errors\":{},\"effects\":{},\"argument_effects\":{},\"layer\":{},\"calls\":{},\"implementations\":[{}],\"hash\":{}}}",
            json_string(&self.name),
            json_string(self.kind),
            json_string(&self.signature),
            json_string(&self.documentation),
            json_string(&self.declared),
            json_string(&self.default_of),
            strings(&self.usages),
            strings(&self.errors),
            strings(&effects),
            strings(&self.argument_effects),
            json_string(self.layer),
            strings(&self.calls),
            implementations.join(","),
            json_string(&self.hash),
        )
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The reports of the declarations named by the query, after checking the program.
/// A method that a type gets from a default of an interface is reported as that default,
/// under the name given through the type and with `default_of` naming the default (L-6c).
pub fn lookup(p: &Program, query: &str) -> Vec<Report> {
    let nodes = find(p, query);
    let defaults = if nodes.is_empty() {
        defaults_through_types(p, query)
    } else {
        Vec::new()
    };
    if nodes.is_empty() && defaults.is_empty() {
        return Vec::new();
    }
    let derived = effects::derive(p);
    let hashes = hashes(p);
    let mut reports: Vec<Report> = nodes
        .into_iter()
        .map(|n| report(p, &derived, &hashes, n))
        .collect();
    for f in defaults {
        let mut r = report(p, &derived, &hashes, Node::Func(f));
        r.default_of = std::mem::replace(&mut r.name, query.to_string());
        reports.push(r);
    }
    reports
}
