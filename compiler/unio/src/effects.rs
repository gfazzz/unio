//! Derived effects and layers (docs/semantics.md, section 8): the effects of every function
//! from the call graph (L-9b, C-11), the lowest library layer it needs (S-11, C-21), the
//! check of `unio check --profile=core` (E0084), and the permissions of the manifest (E0085).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::catalog;
use crate::diag::{Diagnostic, Span};
use crate::program::{FnId, ModId, Owner, Program, LAYER_NAMES};

/// The derived effects, in the order `unio symbol` prints them.
pub const EFFECTS: [&str; 5] = ["files", "network", "process", "env", "allocates"];

/// What a function needs, and for each need the call that brings it in, so that a
/// diagnostic can show the chain of calls down to the function that has it itself.
pub struct Derived {
    /// A bit per entry of EFFECTS.
    pub effects: Vec<u8>,
    pub effect_via: Vec<[Option<(FnId, Span)>; 5]>,
    /// The index in LAYER_NAMES of the lowest layer the function needs.
    pub layer: Vec<usize>,
    pub layer_via: Vec<Option<(FnId, Span)>>,
}

/// The effect an external function of a standard-library module has itself.
fn module_effect(module: &str) -> Option<usize> {
    let effect = match module {
        "std.files" => "files",
        "std.net" => "network",
        "std.process" => "process",
        "std.env" => "env",
        _ => return None,
    };
    EFFECTS.iter().position(|e| *e == effect)
}

const ALLOCATES: usize = 4;

/// Effects and layers of every function: what it has itself, joined with what its callees
/// have, until nothing changes.
pub fn derive(p: &Program) -> Derived {
    let n = p.funcs.len();
    let mut d = Derived {
        effects: vec![0; n],
        effect_via: vec![[None; 5]; n],
        layer: vec![0; n],
        layer_via: vec![None; n],
    };
    for (f, def) in p.funcs.iter().enumerate() {
        let layer = p.module_layer(def.module);
        d.layer[f] = layer.unwrap_or(0);
        if layer == Some(1) {
            d.effects[f] |= 1 << ALLOCATES;
        }
        let external = def.body.is_none() && !matches!(def.owner, Owner::Interface(_));
        if external && p.modules[def.module].stdlib {
            if let Some(e) = module_effect(&p.modules[def.module].name) {
                d.effects[f] |= 1 << e;
            }
        }
    }
    let mut edges: Vec<(FnId, FnId, Span)> = p
        .calls
        .iter()
        .map(|c| (c.caller, c.callee, c.span))
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
                        d.effect_via[caller][e] = Some((callee, span));
                    }
                }
                changed = true;
            }
            if d.layer[callee] > d.layer[caller] {
                d.layer[caller] = d.layer[callee];
                d.layer_via[caller] = Some((callee, span));
                changed = true;
            }
        }
        if !changed {
            return d;
        }
    }
}

impl Derived {
    /// The functions from `f` down to the one that has the effect itself.
    pub fn effect_chain(&self, f: FnId, effect: usize) -> Vec<FnId> {
        follow(f, |g| self.effect_via[g][effect].map(|(h, _)| h))
    }

    /// The functions from `f` down to the first one of the layer it needs.
    pub fn layer_chain(&self, f: FnId) -> Vec<FnId> {
        follow(f, |g| self.layer_via[g].map(|(h, _)| h))
    }

    pub fn effect_names(&self, f: FnId) -> Vec<&'static str> {
        (0..EFFECTS.len())
            .filter(|e| self.effects[f] & (1 << e) != 0)
            .map(|e| EFFECTS[e])
            .collect()
    }
}

fn follow(start: FnId, next: impl Fn(FnId) -> Option<FnId>) -> Vec<FnId> {
    let mut chain = vec![start];
    let mut seen = HashSet::from([start]);
    let mut at = start;
    while let Some(n) = next(at) {
        if !seen.insert(n) {
            break;
        }
        chain.push(n);
        at = n;
    }
    chain
}

fn show_chain(p: &Program, chain: &[FnId]) -> String {
    chain
        .iter()
        .map(|f| crate::symbol::qualified_name(p, *f))
        .collect::<Vec<_>>()
        .join(" -> ")
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
        let effect = match key.split('.').next().unwrap_or("") {
            "files" => "files",
            "network" | "net" => "network",
            "process" => "process",
            "env" => "env",
            _ => continue,
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
            let message = catalog::profile_violation(
                &def.name,
                LAYER_NAMES[d.layer[f]],
                &show_chain(p, &chain),
            );
            found.push((def.module, Diagnostic::error("E0084", def.span, message)));
        }
    }
    let mut permissions: HashMap<ModId, Option<HashSet<&'static str>>> = HashMap::new();
    let mut reported_effects: HashSet<(ModId, usize)> = HashSet::new();
    let mut calls: Vec<_> = p.calls.iter().collect();
    calls.sort_by_key(|c| (c.module, c.span.start.offset));
    for call in calls {
        if !reported(p, call.module) {
            continue;
        }
        let permitted = permissions
            .entry(call.module)
            .or_insert_with(|| manifest_permissions(&p.modules[call.module].package));
        let Some(permitted) = permitted else {
            continue;
        };
        for (e, effect) in EFFECTS.iter().enumerate() {
            if e == ALLOCATES || d.effects[call.callee] & (1 << e) == 0 {
                continue;
            }
            if permitted.contains(effect) || !reported_effects.insert((call.module, e)) {
                continue;
            }
            let mut chain = vec![call.caller];
            chain.extend(d.effect_chain(call.callee, e));
            let message = catalog::effect_not_permitted(effect, &show_chain(p, &chain));
            found.push((call.module, Diagnostic::error("E0085", call.span, message)));
        }
    }
    for (m, diagnostic) in found {
        p.error(m, diagnostic);
    }
}
