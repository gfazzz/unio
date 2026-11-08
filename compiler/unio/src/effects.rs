//! Derived effects and layers (docs/semantics.md, section 8): the effects of every function
//! from the call graph (L-9b, C-11), the lowest library layer it needs (S-11, C-21), the
//! check of `unio check --profile=core` (E0084), and the permissions of the manifest (E0085).
//!
//! The graph has a node for every function, every constant (its initializer) and every struct
//! or error (the defaults of its fields). Its edges are the calls the checks record, the
//! implicit calls through interfaces, the uses of constants and field defaults, and an edge
//! from every interface method to each of its implementations, so that a call through an
//! interface has the union of what they need (semantics 8.4). Allocating expressions are
//! sources of `allocates` and of the layer `alloc`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::catalog;
use crate::diag::{Diagnostic, Span};
use crate::program::{DefKind, FnId, ModId, Owner, Program, LAYER_NAMES};
use crate::symbol::Node;
use crate::types::Ty;

/// An expression that allocates by itself, without a call (semantics 8.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocation {
    List,
    Map,
    Comprehension,
    Interpolation,
    Concatenation,
    Closure,
}

impl Allocation {
    /// What a diagnostic calls it, in English and in Russian.
    pub fn describe(self) -> (&'static str, &'static str) {
        match self {
            Allocation::List => ("a list literal", "литерал списка"),
            Allocation::Map => ("a map literal", "литерал словаря"),
            Allocation::Comprehension => ("a list comprehension", "генератор списка"),
            Allocation::Interpolation => ("a text with interpolation", "текст с интерполяцией"),
            Allocation::Concatenation => ("joining texts with +", "соединение текстов через +"),
            Allocation::Closure => (
                "a lambda that captures values",
                "лямбда, захватывающая значения",
            ),
        }
    }
}

/// A use that is not a recorded call: `from` needs what `to` needs.
pub struct Link {
    pub from: Node,
    pub to: Node,
    pub module: ModId,
    pub span: Span,
}

/// An allocating expression inside a function, a constant or a field default.
pub struct Site {
    pub at: Node,
    pub what: Allocation,
    pub module: ModId,
    pub span: Span,
}

/// What the checks find for the effects besides `Program::calls`.
#[derive(Default)]
pub struct Facts {
    pub links: Vec<Link>,
    pub sites: Vec<Site>,
}

/// Why a node has an effect or a layer: a node it uses, or an allocating expression of its own.
#[derive(Clone, Copy, Debug)]
pub enum Via {
    Node(usize, Span),
    Site(usize),
}

/// One step of a chain: a node of the graph or, last, an allocating expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Node(usize),
    Site(usize),
}

/// The derived effects, in the order `unio symbol` prints them. `files.read` and `files.write`
/// are the scopes of the manifest of the same names (T-6).
pub const EFFECTS: [&str; 6] = [
    "files.read",
    "files.write",
    "network",
    "process",
    "env",
    "allocates",
];
const FILES_READ: usize = 0;
const FILES_WRITE: usize = 1;

/// What each node needs, and for each need the use that brings it in, so that a diagnostic
/// can show the chain of calls down to the function or expression that has it itself. Nodes
/// are numbered functions first, so `effects[f]` is the function `f`; then constants, then
/// the field defaults of each declaration.
pub struct Derived {
    /// A bit per entry of EFFECTS.
    pub effects: Vec<u8>,
    pub effect_via: Vec<[Option<Via>; EFFECTS.len()]>,
    /// The index in LAYER_NAMES of the lowest layer the node needs.
    pub layer: Vec<usize>,
    pub layer_via: Vec<Option<Via>>,
    functions: usize,
    constants: usize,
}

/// The effects an external function of a standard-library module has itself, as bits. An
/// external of `std.files` has the scopes its "Permission:" line names (semantics 8.2).
fn external_effects(p: &Program, f: FnId) -> u8 {
    let def = &p.funcs[f];
    let module = &p.modules[def.module];
    let effect = match module.name.as_str() {
        "std.files" => return file_scopes(&module.analysis.source, def.span.start.line),
        "std.net" => "network",
        "std.process" => "process",
        "std.env" => "env",
        _ => return 0,
    };
    EFFECTS
        .iter()
        .position(|e| *e == effect)
        .map_or(0, |e| 1 << e)
}

/// The file scopes named by the "Permission:" sentence of the documentation comment that ends
/// above the line; both scopes when there is none, so that a missing line is never permissive.
fn file_scopes(source: &str, line: usize) -> u8 {
    let lines: Vec<&str> = source.lines().collect();
    let mut doc = String::new();
    let mut at = line.saturating_sub(1).min(lines.len());
    while at > 0 && lines[at - 1].trim_start().starts_with("##") {
        at -= 1;
        doc.insert_str(0, lines[at]);
    }
    let Some((_, permission)) = doc.split_once("Permission:") else {
        return (1 << FILES_READ) | (1 << FILES_WRITE);
    };
    let mut scopes = 0;
    if permission.contains("files.read") {
        scopes |= 1 << FILES_READ;
    }
    if permission.contains("files.write") {
        scopes |= 1 << FILES_WRITE;
    }
    if permission.trim_start().starts_with("none") {
        scopes = 0;
    }
    scopes
}

const ALLOCATES: usize = 5;

/// The implementations of an interface method: the method of the same name in every
/// `extend ... with` of its interface in the program. A type implements an interface only in
/// its own module or in the interface's, and every module of the program is loaded, so the
/// set is complete for the program (semantics 8.4).
fn implementations(p: &Program) -> Vec<(FnId, FnId, Span)> {
    let mut found = Vec::new();
    for imp in &p.impls {
        let Some(Ty::Named(i, _)) = &imp.interface else {
            continue;
        };
        let DefKind::Interface(members) = &p.defs[*i].kind else {
            continue;
        };
        for &m in members {
            for &f in &imp.methods {
                if p.funcs[f].name == p.funcs[m].name {
                    found.push((m, f, p.funcs[f].span));
                }
            }
        }
    }
    found
}

impl Derived {
    fn node(&self, n: Node) -> usize {
        match n {
            Node::Func(f) => f,
            Node::Const(c) => self.functions + c,
            Node::Def(d) => self.functions + self.constants + d,
        }
    }

    fn holder(&self, i: usize) -> Node {
        if i < self.functions {
            Node::Func(i)
        } else if i < self.functions + self.constants {
            Node::Const(i - self.functions)
        } else {
            Node::Def(i - self.functions - self.constants)
        }
    }
}

/// Effects and layers of every node: what it has itself, joined with what the nodes it uses
/// have, until nothing changes.
pub fn derive(p: &Program) -> Derived {
    let functions = p.funcs.len();
    let constants = p.consts.len();
    let n = functions + constants + p.defs.len();
    let mut d = Derived {
        effects: vec![0; n],
        effect_via: vec![[None; EFFECTS.len()]; n],
        layer: vec![0; n],
        layer_via: vec![None; n],
        functions,
        constants,
    };
    for (f, def) in p.funcs.iter().enumerate() {
        let layer = p.module_layer(def.module);
        d.layer[f] = layer.unwrap_or(0);
        if layer == Some(1) {
            d.effects[f] |= 1 << ALLOCATES;
        }
        let external = def.body.is_none() && !matches!(def.owner, Owner::Interface(_));
        if external && p.modules[def.module].stdlib {
            d.effects[f] |= external_effects(p, f);
        }
    }
    for (i, site) in p.effect_facts.sites.iter().enumerate() {
        let at = d.node(site.at);
        if d.effects[at] & (1 << ALLOCATES) == 0 {
            d.effects[at] |= 1 << ALLOCATES;
            d.effect_via[at][ALLOCATES] = Some(Via::Site(i));
        }
        if d.layer[at] < 1 {
            d.layer[at] = 1;
            d.layer_via[at] = Some(Via::Site(i));
        }
    }
    let mut edges: Vec<(usize, usize, Span)> = p
        .calls
        .iter()
        .map(|c| (c.caller, c.callee, c.span))
        .chain(
            p.effect_facts
                .links
                .iter()
                .map(|l| (d.node(l.from), d.node(l.to), l.span)),
        )
        .chain(implementations(p))
        .collect();
    edges.sort_by_key(|(a, b, s)| (*a, s.start.offset, *b));
    edges.dedup_by_key(|(a, b, _)| (*a, *b));
    loop {
        let mut changed = false;
        for &(caller, callee, span) in &edges {
            let added = d.effects[callee] & !d.effects[caller];
            if added != 0 {
                d.effects[caller] |= added;
                for e in 0..EFFECTS.len() {
                    if added & (1 << e) != 0 {
                        d.effect_via[caller][e] = Some(Via::Node(callee, span));
                    }
                }
                changed = true;
            }
            if d.layer[callee] > d.layer[caller] {
                d.layer[caller] = d.layer[callee];
                d.layer_via[caller] = Some(Via::Node(callee, span));
                changed = true;
            }
        }
        if !changed {
            return d;
        }
    }
}

impl Derived {
    /// The steps from `f` down to the function or expression that has the effect itself.
    pub fn effect_chain(&self, f: FnId, effect: usize) -> Vec<Step> {
        follow(f, |g| self.effect_via[g][effect])
    }

    /// The steps from `f` down to the first function or expression of the layer it needs.
    pub fn layer_chain(&self, f: FnId) -> Vec<Step> {
        follow(f, |g| self.layer_via[g])
    }

    pub fn effect_names(&self, f: FnId) -> Vec<&'static str> {
        (0..EFFECTS.len())
            .filter(|e| self.effects[f] & (1 << e) != 0)
            .map(|e| EFFECTS[e])
            .collect()
    }
}

fn follow(start: usize, next: impl Fn(usize) -> Option<Via>) -> Vec<Step> {
    let mut chain = vec![Step::Node(start)];
    let mut seen = HashSet::from([start]);
    let mut at = start;
    while let Some(via) = next(at) {
        match via {
            Via::Site(s) => {
                chain.push(Step::Site(s));
                break;
            }
            Via::Node(n, _) => {
                if !seen.insert(n) {
                    break;
                }
                chain.push(Step::Node(n));
                at = n;
            }
        }
    }
    chain
}

/// The chain as a diagnostic shows it, in English and in Russian: the names of the functions,
/// constants and types, and last the allocating expression with its line.
fn show_chain(p: &Program, d: &Derived, chain: &[Step]) -> (String, String) {
    let mut en = Vec::new();
    let mut ru = Vec::new();
    for step in chain {
        match *step {
            Step::Node(n) => {
                let name = crate::symbol::node_name(p, d.holder(n));
                en.push(name.clone());
                ru.push(name);
            }
            Step::Site(s) => {
                let site = &p.effect_facts.sites[s];
                let (what_en, what_ru) = site.what.describe();
                let line = site.span.start.line;
                en.push(format!("{what_en} on line {line}"));
                ru.push(format!("{what_ru} в строке {line}"));
            }
        }
    }
    (en.join(" -> "), ru.join(" -> "))
}

/// The build profile of `unio check` (C-21).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    #[default]
    Application,
    Core,
}

/// The effects a manifest permits, or None when it has no `[permissions]` table.
pub fn manifest_permissions(package: &Path) -> Option<HashSet<&'static str>> {
    let text = std::fs::read_to_string(package.join("unio.toml")).ok()?;
    let mut section = String::new();
    let mut found = false;
    let mut permitted = HashSet::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_string();
            found |= section == "permissions";
            continue;
        }
        if section != "permissions" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let granted = match value {
            "false" | "[]" | "\"\"" => false,
            _ => !value.is_empty(),
        };
        if !granted {
            continue;
        }
        let effect = match key {
            "files.read" => "files.read",
            "files.write" => "files.write",
            _ => match key.split('.').next().unwrap_or("") {
                "network" | "net" => "network",
                "process" => "process",
                "env" => "env",
                _ => continue,
            },
        };
        permitted.insert(effect);
    }
    found.then_some(permitted)
}

/// E0084 for the core profile and E0085 for the manifest, on the modules that are reported.
pub fn report(p: &mut Program, profile: Profile) {
    let d = derive(p);
    let mut found: Vec<(ModId, Diagnostic)> = Vec::new();
    let reported = |p: &Program, m: ModId| p.modules[m].report && !p.modules[m].stdlib;
    if profile == Profile::Core {
        for (f, def) in p.funcs.iter().enumerate() {
            if !reported(p, def.module) || d.layer[f] == 0 {
                continue;
            }
            let chain = d.layer_chain(f);
            let (en, ru) = show_chain(p, &d, &chain);
            let message =
                catalog::profile_violation(&def.name, LAYER_NAMES[d.layer[f]], (&en, &ru));
            found.push((def.module, Diagnostic::error("E0084", def.span, message)));
        }
    }
    let mut permissions: HashMap<ModId, Option<HashSet<&'static str>>> = HashMap::new();
    let mut reported_effects: HashSet<(ModId, usize)> = HashSet::new();
    // Every use at a place: recorded calls, and the implicit calls and uses of constants and
    // field defaults (semantics 8.4).
    let mut uses: Vec<(ModId, Span, usize, usize)> = p
        .calls
        .iter()
        .map(|c| (c.module, c.span, c.caller, c.callee))
        .chain(
            p.effect_facts
                .links
                .iter()
                .map(|l| (l.module, l.span, d.node(l.from), d.node(l.to))),
        )
        .collect();
    uses.sort_by_key(|(m, span, _, _)| (*m, span.start.offset));
    for (module, span, from, to) in uses {
        if !reported(p, module) {
            continue;
        }
        let permitted = permissions
            .entry(module)
            .or_insert_with(|| manifest_permissions(&p.modules[module].package));
        let Some(permitted) = permitted else {
            continue;
        };
        for (e, effect) in EFFECTS.iter().enumerate() {
            if e == ALLOCATES || d.effects[to] & (1 << e) == 0 {
                continue;
            }
            if permitted.contains(effect) || !reported_effects.insert((module, e)) {
                continue;
            }
            let mut chain = vec![Step::Node(from)];
            chain.extend(follow(to, |g| d.effect_via[g][e]));
            let (en, ru) = show_chain(p, &d, &chain);
            let message = catalog::effect_not_permitted(effect, (&en, &ru));
            found.push((module, Diagnostic::error("E0085", span, message)));
        }
    }
    for (m, diagnostic) in found {
        p.error(m, diagnostic);
    }
}
