//! `unio api-diff` (T-15, docs/language-contracts.md section 6): the public API of a package and
//! the comparison of two versions of it.
//!
//! The API of a package records, for every public declaration of its modules: the kind, the
//! generic parameters with their constraints, the parameters with their labels, `named` and
//! `var` markers and default values, the result, the error set, the effects a call needs, the
//! public fields, the variants, the members of an interface, and the content hash (C-11a,
//! C-11b); and the permissions the manifest requires. A hash that changed while everything
//! else stayed is an implementation change, never an incompatibility by itself: hashes alone
//! are not a verdict.
//!
//! Incompatible changes need a new major version (for `0.y.z`, a new minor version), additions
//! a new minor version (for `0.y.z`, a new patch version). The comparison reports each change
//! with its level, the callers of a given package that it affects, and a migration when one is
//! unambiguous: a renamed function, a renamed label, a parameter that became `named`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::ast;
use crate::lsp::json::Json;
use crate::program::{DefKind, FnId, Owner, Program};
use crate::symbol::{self, Node};

/// The version of the API record.
pub const API_SCHEME: &str = "unio-api-v1";

/// One parameter of a public function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub named: bool,
    pub var: bool,
    /// The canonical form of the default value.
    pub default: Option<String>,
}

/// A public field of a struct or an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub var: bool,
    pub default: bool,
}

/// One public declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: &'static str,
    pub generics: Vec<(String, Vec<String>)>,
    pub params: Vec<Param>,
    /// `Some(var)` for a method with `self`.
    pub self_param: Option<bool>,
    pub returns: String,
    pub fails: Vec<String>,
    pub effects: Vec<String>,
    pub fields: Vec<Field>,
    /// Whether a struct has fields that are not public, so that callers cannot construct it.
    pub private_fields: bool,
    pub variants: Vec<(String, Vec<String>)>,
    pub members: Vec<String>,
    pub hash: String,
    /// Where it is declared, relative to the package: `strings.unio:3`.
    pub declared: String,
}

/// The public API of one package.
#[derive(Clone, Debug, Default)]
pub struct Api {
    pub name: String,
    pub version: String,
    /// The permissions of `[permissions]`, as `key = element`.
    pub permissions: BTreeSet<String>,
    pub items: BTreeMap<String, Item>,
}

/// How a change affects the users of a package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Only the implementation changed (or nothing).
    Patch,
    /// Something was added; existing callers keep working.
    Minor,
    /// Existing callers can stop compiling or behave differently.
    Major,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Patch => "patch",
            Level::Minor => "minor",
            Level::Major => "major",
        }
    }
}

/// An unambiguous rewrite of the callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rewrite {
    /// Calls of the function use the new name.
    RenameFunction { from: String, to: String },
    /// The named argument `from` is written `to`.
    RenameLabel { from: String, to: String },
    /// The positional argument at this index gets the label.
    InsertLabel { index: usize, label: String },
}

impl Rewrite {
    fn describe(&self) -> String {
        match self {
            Rewrite::RenameFunction { from, to } => format!("rename the calls of '{from}' to '{to}'"),
            Rewrite::RenameLabel { from, to } => format!("write the argument '{from} = ' as '{to} = '"),
            Rewrite::InsertLabel { index, label } => format!(
                "give argument {} its label: '{label} = ' (unio fmt --migrate does this through E0040)",
                index + 1
            ),
        }
    }
}

/// One difference between two APIs.
#[derive(Clone, Debug)]
pub struct Change {
    pub level: Level,
    pub item: String,
    pub message: String,
    pub rewrite: Option<Rewrite>,
    /// The places of a caller package that the change affects.
    pub callers: Vec<String>,
    /// Edits that apply the rewrite at those places: `file:line:column: old -> new`.
    pub edits: Vec<String>,
}

// ---------------------------------------------------------------- reading a package

/// The `.unio` files of a package, without nested packages and hidden directories.
pub fn package_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if !path.join("unio.toml").is_file() {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "unio") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The values of the simple `key = value` lines of a table of a manifest.
fn manifest_table(text: &str, table: &str) -> Vec<(String, String)> {
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or("").trim();
        if line.starts_with('[') {
            inside = line == format!("[{table}]");
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            out.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    out
}

fn unquote(text: &str) -> String {
    text.trim().trim_matches('"').to_string()
}

/// The elements of a permission value: a list of texts, or the value itself.
fn elements(value: &str) -> Vec<String> {
    let value = value.trim();
    match value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        Some(list) => list
            .split(',')
            .map(unquote)
            .filter(|e| !e.is_empty())
            .collect(),
        None => vec![value.to_string()],
    }
}

/// The name of a module relative to its package: `a/b.unio` is `a.b`.
fn module_key(package: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(package).unwrap_or(path);
    let mut parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(last) = parts.last_mut() {
        if let Some(stem) = last.strip_suffix(".unio") {
            *last = stem.to_string();
        }
    }
    parts.join(".")
}

fn site(package: &Path, p: &Program, m: usize, line: usize) -> String {
    let relative = p.modules[m]
        .path
        .strip_prefix(package)
        .unwrap_or(&p.modules[m].path)
        .to_string_lossy()
        .replace('\\', "/");
    format!("{relative}:{line}")
}

fn contains(outer: crate::diag::Span, inner: crate::diag::Span) -> bool {
    outer.start.offset <= inner.start.offset && inner.end.offset <= outer.end.offset
}

/// The parameters of the declaration of a function as the source writes them.
fn signature_of(p: &Program, f: FnId) -> Option<&ast::Signature> {
    let def = &p.funcs[f];
    for item in &p.modules[def.module].analysis.file.items {
        match item {
            ast::Item::Declaration(d) if contains(d.span, def.span) => {
                return match &d.kind {
                    ast::DeclarationKind::Function(x) => Some(&x.signature),
                    ast::DeclarationKind::External(s) => Some(s),
                    ast::DeclarationKind::Interface(i) => i
                        .members
                        .iter()
                        .map(|(s, _)| s)
                        .find(|s| s.name.text == def.name),
                    _ => None,
                };
            }
            ast::Item::Extend(e) if contains(e.span, def.span) => {
                return e
                    .members
                    .iter()
                    .find(|m| {
                        contains(m.signature.span, def.span) || m.signature.name.span == def.span
                    })
                    .map(|m| &m.signature);
            }
            _ => {}
        }
    }
    None
}

fn defaults_of(p: &Program, f: FnId) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let source = &p.modules[p.funcs[f].module].analysis.source;
    if let Some(sig) = signature_of(p, f) {
        for param in &sig.params {
            match param {
                ast::Param::Named {
                    name,
                    default: Some(d),
                    ..
                }
                | ast::Param::Plain {
                    name,
                    default: Some(d),
                    ..
                } => {
                    // The value as written, with white space made uniform.
                    let text = source
                        .get(d.span.start.offset..d.span.end.offset)
                        .unwrap_or("")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    out.insert(name.text.clone(), text);
                }
                _ => {}
            }
        }
    }
    out
}

fn generics(p: &Program, params: &[crate::types::ParamId]) -> Vec<(String, Vec<String>)> {
    params
        .iter()
        .map(|g| {
            let info = &p.params[*g];
            let mut bounds: Vec<String> = info.bounds.iter().map(|b| p.show(b)).collect();
            bounds.sort();
            (info.name.clone(), bounds)
        })
        .collect()
}

fn empty_item(kind: &'static str, hash: String, declared: String) -> Item {
    Item {
        kind,
        generics: Vec::new(),
        params: Vec::new(),
        self_param: None,
        returns: String::new(),
        fails: Vec::new(),
        effects: Vec::new(),
        fields: Vec::new(),
        private_fields: false,
        variants: Vec::new(),
        members: Vec::new(),
        hash,
        declared,
    }
}

fn function_item(
    p: &Program,
    derived: &crate::effects::Derived,
    hashes: &symbol::Hashes,
    package: &Path,
    f: FnId,
) -> Item {
    let def = &p.funcs[f];
    let defaults = defaults_of(p, f);
    let mut item = empty_item(
        "function",
        hashes.hash.get(&Node::Func(f)).cloned().unwrap_or_default(),
        site(package, p, def.module, def.span.start.line),
    );
    item.generics = generics(p, &def.generics);
    item.params = def
        .params
        .iter()
        .map(|q| Param {
            name: q.name.clone(),
            ty: p.show(&q.ty),
            named: q.named_only,
            var: q.var,
            default: if q.has_default {
                Some(defaults.get(&q.name).cloned().unwrap_or_default())
            } else {
                None
            },
        })
        .collect();
    item.self_param = def.self_param;
    item.returns = p.show(&def.ret);
    let mut fails: Vec<String> = def.fails.iter().map(|t| p.show(t)).collect();
    fails.sort();
    item.fails = fails;
    item.effects = derived
        .effect_names(f)
        .into_iter()
        .map(str::to_string)
        .collect();
    item
}

/// Reads the API of the package at `root`: its manifest and the public declarations of all
/// its modules. The program must check without errors.
pub fn read(root: &Path) -> Result<Api, String> {
    let root = root.to_path_buf();
    crate::on_front_end_stack(move || read_here(&root))
}

fn read_here(root: &Path) -> Result<Api, String> {
    let manifest_path = root.join("unio.toml");
    let manifest = crate::vfs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let package = manifest_table(&manifest, "package");
    let field = |key: &str| {
        package
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| unquote(v))
            .unwrap_or_default()
    };
    let mut api = Api {
        name: field("name"),
        version: field("version"),
        ..Api::default()
    };
    for (key, value) in manifest_table(&manifest, "permissions") {
        for element in elements(&value) {
            api.permissions.insert(format!("{key} = {element}"));
        }
    }
    let files = package_files(root);
    if files.is_empty() {
        return Ok(api);
    }
    let program = crate::load_checked(&files, crate::effects::Profile::Application);
    let failed = program
        .modules
        .iter()
        .any(|m| m.report && m.analysis.has_errors())
        || program
            .diagnostics
            .iter()
            .any(|(_, d)| d.severity == crate::Severity::Error);
    if failed {
        let mut text = String::new();
        for analysis in crate::analyses(program) {
            if analysis.has_errors() {
                text.push_str(&analysis.render_text(crate::Lang::En));
            }
        }
        return Err(format!(
            "the package {} does not check:\n{text}",
            root.display()
        ));
    }
    let package_dir = program
        .modules
        .iter()
        .find(|m| m.report)
        .map(|m| m.package.clone())
        .unwrap_or_else(|| root.to_path_buf());
    let derived = crate::effects::derive(&program);
    let hashes = symbol::hashes(&program);
    let ours = |m: usize| {
        let module = &program.modules[m];
        module.report && !module.stdlib && module.dependency.is_none()
    };
    let key_of = |m: usize| module_key(&package_dir, &program.modules[m].path);
    for (f, def) in program.funcs.iter().enumerate() {
        if !ours(def.module) || !def.public {
            continue;
        }
        let name = match def.owner {
            Owner::Module => format!("{}.{}", key_of(def.module), def.name),
            Owner::Impl(i) => {
                let target = program.show(&program.impls[i].target);
                let target = target.split('[').next().unwrap_or(&target).to_string();
                // A method of a type that is not public is not part of the API.
                let public_target = program
                    .defs
                    .iter()
                    .any(|d| d.name == target && d.public && ours(d.module));
                if !public_target {
                    continue;
                }
                format!("{}.{target}.{}", key_of(def.module), def.name)
            }
            Owner::Interface(_) => continue,
        };
        api.items.insert(
            name,
            function_item(&program, &derived, &hashes, &package_dir, f),
        );
    }
    for (d, def) in program.defs.iter().enumerate() {
        if !ours(def.module) || !def.public {
            continue;
        }
        let name = format!("{}.{}", key_of(def.module), def.name);
        let hash = hashes.hash.get(&Node::Def(d)).cloned().unwrap_or_default();
        let declared = site(&package_dir, &program, def.module, def.span.start.line);
        let mut item = match &def.kind {
            DefKind::Struct(fields) | DefKind::Error(fields) => {
                let kind = if matches!(def.kind, DefKind::Struct(_)) {
                    "struct"
                } else {
                    "error"
                };
                let mut item = empty_item(kind, hash, declared);
                item.private_fields = fields.iter().any(|f| !f.public);
                item.fields = fields
                    .iter()
                    .filter(|f| f.public)
                    .map(|f| Field {
                        name: f.name.clone(),
                        ty: program.show(&f.ty),
                        var: f.var,
                        default: f.has_default,
                    })
                    .collect();
                item
            }
            DefKind::Enum(variants) => {
                let mut item = empty_item("enum", hash, declared);
                item.variants = variants
                    .iter()
                    .map(|v| {
                        (
                            v.name.clone(),
                            v.fields.iter().map(|(_, t)| program.show(t)).collect(),
                        )
                    })
                    .collect();
                item
            }
            DefKind::Interface(members) => {
                let mut item = empty_item("interface", hash, declared);
                item.members = members
                    .iter()
                    .map(|m| {
                        let f = function_item(&program, &derived, &hashes, &package_dir, *m);
                        format!("{}{}", program.funcs[*m].name, shape(&f))
                    })
                    .collect();
                item
            }
        };
        item.generics = generics(&program, &def.generics);
        api.items.insert(name, item);
    }
    for (c, def) in program.consts.iter().enumerate() {
        if !ours(def.module) || !def.public {
            continue;
        }
        let name = format!("{}.{}", key_of(def.module), def.name);
        let hash = hashes
            .hash
            .get(&Node::Const(c))
            .cloned()
            .unwrap_or_default();
        let declared = site(&package_dir, &program, def.module, def.span.start.line);
        let mut item = empty_item("constant", hash, declared);
        item.returns = def
            .annotated
            .as_ref()
            .map(|t| program.show(t))
            .unwrap_or_default();
        api.items.insert(name, item);
    }
    Ok(api)
}

/// The shape of a function: what a caller writes and gets, without its name.
fn shape(item: &Item) -> String {
    let params: Vec<String> = item
        .params
        .iter()
        .map(|q| {
            let mut text = String::new();
            if q.named {
                text.push_str("named ");
            }
            if q.var {
                text.push_str("var ");
            }
            text.push_str(&format!("{} as {}", q.name, q.ty));
            if let Some(d) = &q.default {
                text.push_str(&format!(" = {d}"));
            }
            text
        })
        .collect();
    let generics: Vec<String> = item
        .generics
        .iter()
        .map(|(n, b)| {
            if b.is_empty() {
                n.clone()
            } else {
                format!("{n} with {}", b.join(", "))
            }
        })
        .collect();
    let mut out = String::new();
    if !generics.is_empty() {
        out.push_str(&format!("[{}]", generics.join(", ")));
    }
    let receiver = match item.self_param {
        Some(true) => vec!["var self".to_string()],
        Some(false) => vec!["self".to_string()],
        None => Vec::new(),
    };
    out.push_str(&format!(
        "({})",
        receiver
            .into_iter()
            .chain(params)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if item.returns != "Unit" && !item.returns.is_empty() {
        out.push_str(&format!(" returns {}", item.returns));
    }
    if !item.fails.is_empty() {
        out.push_str(&format!(" fails with {}", item.fails.join(", ")));
    }
    out
}

// ---------------------------------------------------------------- comparing

fn change(level: Level, item: &str, message: String) -> Change {
    Change {
        level,
        item: item.to_string(),
        message,
        rewrite: None,
        callers: Vec::new(),
        edits: Vec::new(),
    }
}

fn names<T>(list: &[(String, T)]) -> Vec<&str> {
    list.iter().map(|(n, _)| n.as_str()).collect()
}

fn compare_function(name: &str, old: &Item, new: &Item, out: &mut Vec<Change>) {
    let before = out.len();
    if old.self_param != new.self_param {
        out.push(change(
            Level::Major,
            name,
            "the receiver changed".to_string(),
        ));
    }
    for (g, bounds) in &new.generics {
        match old.generics.iter().find(|(o, _)| o == g) {
            None => out.push(change(
                Level::Major,
                name,
                format!("type parameter '{g}' was added"),
            )),
            Some((_, old_bounds)) => {
                for b in bounds.iter().filter(|b| !old_bounds.contains(b)) {
                    out.push(change(
                        Level::Major,
                        name,
                        format!("type parameter '{g}' needs '{b}' now: the constraint is stricter"),
                    ));
                }
                for b in old_bounds.iter().filter(|b| !bounds.contains(b)) {
                    out.push(change(
                        Level::Minor,
                        name,
                        format!("type parameter '{g}' no longer needs '{b}'"),
                    ));
                }
            }
        }
    }
    for (g, _) in &old.generics {
        if !new.generics.iter().any(|(n, _)| n == g) {
            out.push(change(
                Level::Major,
                name,
                format!("type parameter '{g}' was removed"),
            ));
        }
    }
    // Parameters by position: a label renamed in place is one change with a migration.
    let common = old.params.len().min(new.params.len());
    for i in 0..common {
        let (o, n) = (&old.params[i], &new.params[i]);
        if o.name != n.name {
            // Every parameter can be given by its label, so a renamed label breaks the calls
            // that write it (docs/language-contracts.md section 6).
            let mut c = change(
                Level::Major,
                name,
                format!("parameter '{}' is named '{}' now", o.name, n.name),
            );
            if o.ty == n.ty {
                c.rewrite = Some(Rewrite::RenameLabel {
                    from: o.name.clone(),
                    to: n.name.clone(),
                });
            }
            out.push(c);
        }
        if o.ty != n.ty {
            out.push(change(
                Level::Major,
                name,
                format!("parameter '{}' has type {} now, was {}", n.name, n.ty, o.ty),
            ));
        }
        if o.var != n.var {
            out.push(change(
                Level::Major,
                name,
                format!("parameter '{}' changed whether it is var", n.name),
            ));
        }
        if !o.named && n.named {
            let mut c = change(
                Level::Major,
                name,
                format!("parameter '{}' is given only by name now", n.name),
            );
            c.rewrite = Some(Rewrite::InsertLabel {
                index: i,
                label: n.name.clone(),
            });
            out.push(c);
        } else if o.named && !n.named {
            out.push(change(
                Level::Minor,
                name,
                format!("parameter '{}' may be given by position too", n.name),
            ));
        }
        match (&o.default, &n.default) {
            (Some(_), None) => out.push(change(
                Level::Major,
                name,
                format!("parameter '{}' lost its default", n.name),
            )),
            (None, Some(_)) => out.push(change(
                Level::Minor,
                name,
                format!("parameter '{}' has a default now", n.name),
            )),
            (Some(a), Some(b)) if a != b => out.push(change(
                Level::Major,
                name,
                format!(
                    "the default of parameter '{}' changed: callers that omit it behave differently",
                    n.name
                ),
            )),
            _ => {}
        }
    }
    for q in &new.params[common..] {
        if q.default.is_some() {
            out.push(change(
                Level::Minor,
                name,
                format!("parameter '{}' was added with a default", q.name),
            ));
        } else {
            out.push(change(
                Level::Major,
                name,
                format!("parameter '{}' was added without a default", q.name),
            ));
        }
    }
    for q in &old.params[common..] {
        out.push(change(
            Level::Major,
            name,
            format!("parameter '{}' was removed", q.name),
        ));
    }
    if old.returns != new.returns {
        out.push(change(
            Level::Major,
            name,
            format!("the result is {} now, was {}", new.returns, old.returns),
        ));
    }
    for e in new.fails.iter().filter(|e| !old.fails.contains(e)) {
        out.push(change(
            Level::Major,
            name,
            format!("the error set is wider: it fails with {e} now"),
        ));
    }
    for e in old.fails.iter().filter(|e| !new.fails.contains(e)) {
        out.push(change(
            Level::Minor,
            name,
            format!("the error set is narrower: it no longer fails with {e}"),
        ));
    }
    for e in new.effects.iter().filter(|e| !old.effects.contains(e)) {
        out.push(change(
            Level::Major,
            name,
            format!("a call needs the effect {e} now"),
        ));
    }
    for e in old.effects.iter().filter(|e| !new.effects.contains(e)) {
        out.push(change(
            Level::Minor,
            name,
            format!("a call no longer needs the effect {e}"),
        ));
    }
    if out.len() == before && old.hash != new.hash {
        out.push(change(
            Level::Patch,
            name,
            "the implementation changed (content hash)".to_string(),
        ));
    }
}

fn compare_fields(name: &str, old: &Item, new: &Item, out: &mut Vec<Change>) {
    let constructible = !new.private_fields;
    for f in &new.fields {
        match old.fields.iter().find(|o| o.name == f.name) {
            None if f.default || !constructible => out.push(change(
                Level::Minor,
                name,
                format!("public field '{}' was added", f.name),
            )),
            None => out.push(change(
                Level::Major,
                name,
                format!(
                    "public field '{}' was added without a default: constructions must give it",
                    f.name
                ),
            )),
            Some(o) => {
                if o.ty != f.ty {
                    out.push(change(
                        Level::Major,
                        name,
                        format!("field '{}' has type {} now, was {}", f.name, f.ty, o.ty),
                    ));
                }
                if o.var && !f.var {
                    out.push(change(
                        Level::Major,
                        name,
                        format!("field '{}' can no longer change", f.name),
                    ));
                }
                if o.default && !f.default {
                    out.push(change(
                        Level::Major,
                        name,
                        format!("field '{}' lost its default", f.name),
                    ));
                }
            }
        }
    }
    for o in &old.fields {
        if !new.fields.iter().any(|f| f.name == o.name) {
            out.push(change(
                Level::Major,
                name,
                format!("public field '{}' was removed or made private", o.name),
            ));
        }
    }
    if !old.private_fields && new.private_fields {
        out.push(change(
            Level::Major,
            name,
            "the type has a private field now: callers can no longer construct it".to_string(),
        ));
    }
}

fn compare_variants(name: &str, old: &Item, new: &Item, out: &mut Vec<Change>) {
    for (v, fields) in &new.variants {
        match old.variants.iter().find(|(o, _)| o == v) {
            None => out.push(change(
                Level::Major,
                name,
                format!(
                    "variant '{v}' was added: a match without 'case _' no longer covers every variant"
                ),
            )),
            Some((_, old_fields)) if old_fields != fields => out.push(change(
                Level::Major,
                name,
                format!("the fields of variant '{v}' changed"),
            )),
            _ => {}
        }
    }
    for v in names(&old.variants) {
        if !names(&new.variants).contains(&v) {
            out.push(change(
                Level::Major,
                name,
                format!("variant '{v}' was removed"),
            ));
        }
    }
}

/// The changes from `old` to `new`, most severe first, then by name.
pub fn compare(old: &Api, new: &Api) -> Vec<Change> {
    let mut out = Vec::new();
    let removed: Vec<&String> = old
        .items
        .keys()
        .filter(|k| !new.items.contains_key(*k))
        .collect();
    let added: Vec<&String> = new
        .items
        .keys()
        .filter(|k| !old.items.contains_key(*k))
        .collect();
    let module_of = |k: &str| {
        k.rsplit_once('.')
            .map(|(m, _)| m.to_string())
            .unwrap_or_default()
    };
    let mut renamed_to: BTreeSet<&String> = BTreeSet::new();
    for name in &removed {
        let item = &old.items[*name];
        let mut c = change(
            Level::Major,
            name,
            format!("the public {} was removed", item.kind),
        );
        // A rename: exactly one added function of the same module with the same shape.
        if item.kind == "function" {
            let same: Vec<&&String> = added
                .iter()
                .filter(|a| {
                    let n = &new.items[**a];
                    n.kind == "function"
                        && module_of(a) == module_of(name)
                        && shape(n) == shape(item)
                })
                .collect();
            let removed_same = removed
                .iter()
                .filter(|r| {
                    let o = &old.items[**r];
                    o.kind == "function"
                        && module_of(r) == module_of(name)
                        && shape(o) == shape(item)
                })
                .count();
            if let ([to], 1) = (same.as_slice(), removed_same) {
                let from_short = name.rsplit('.').next().unwrap_or(name).to_string();
                let to_short = to.rsplit('.').next().unwrap_or(to).to_string();
                c.message = format!("the function was renamed to '{to}'");
                c.rewrite = Some(Rewrite::RenameFunction {
                    from: from_short,
                    to: to_short,
                });
                renamed_to.insert(to);
            }
        }
        out.push(c);
    }
    for name in &added {
        if renamed_to.contains(name) {
            continue;
        }
        out.push(change(
            Level::Minor,
            name,
            format!("the public {} was added", new.items[*name].kind),
        ));
    }
    for (name, o) in &old.items {
        let Some(n) = new.items.get(name) else {
            continue;
        };
        let before = out.len();
        if o.kind != n.kind {
            out.push(change(
                Level::Major,
                name,
                format!("a {} became a {}", o.kind, n.kind),
            ));
            continue;
        }
        match o.kind {
            "function" => compare_function(name, o, n, &mut out),
            "struct" | "error" => compare_fields(name, o, n, &mut out),
            "enum" => compare_variants(name, o, n, &mut out),
            "interface" => {
                for m in n.members.iter().filter(|m| !o.members.contains(m)) {
                    out.push(change(
                        Level::Major,
                        name,
                        format!("member '{m}' is new: implementations must provide it"),
                    ));
                }
                for m in o.members.iter().filter(|m| !n.members.contains(m)) {
                    out.push(change(
                        Level::Major,
                        name,
                        format!("member '{m}' was removed or changed"),
                    ));
                }
            }
            _ => {
                if o.returns != n.returns {
                    out.push(change(
                        Level::Major,
                        name,
                        format!("the type is {} now, was {}", n.returns, o.returns),
                    ));
                }
            }
        }
        if o.kind != "function" && o.generics != n.generics {
            out.push(change(
                Level::Major,
                name,
                "the type parameters or their constraints changed".to_string(),
            ));
        }
        if out.len() == before && o.hash != n.hash && o.kind != "function" {
            out.push(change(
                Level::Patch,
                name,
                "the implementation changed (content hash)".to_string(),
            ));
        }
    }
    for p in new.permissions.difference(&old.permissions) {
        out.push(change(
            Level::Major,
            "unio.toml",
            format!("the package requires the permission {p} now: installing it needs a new confirmation"),
        ));
    }
    for p in old.permissions.difference(&new.permissions) {
        out.push(change(
            Level::Minor,
            "unio.toml",
            format!("the package no longer requires the permission {p}"),
        ));
    }
    out.sort_by(|a, b| b.level.cmp(&a.level).then(a.item.cmp(&b.item)));
    out
}

// ---------------------------------------------------------------- versions

/// A semantic version `major.minor.patch`.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// The smallest version after `old` that the level of change allows.
pub fn required_version(old: (u64, u64, u64), level: Level) -> (u64, u64, u64) {
    let (major, minor, patch) = old;
    match (major, level) {
        (0, Level::Major) => (0, minor + 1, 0),
        (0, _) => (0, minor, patch + 1),
        (_, Level::Major) => (major + 1, 0, 0),
        (_, Level::Minor) => (major, minor + 1, 0),
        (_, Level::Patch) => (major, minor, patch + 1),
    }
}

/// The verdict of the semantic-version rule: `None` when the new version is enough.
pub fn version_problem(old: &str, new: &str, level: Level) -> Option<String> {
    let (Some(o), Some(n)) = (parse_version(old), parse_version(new)) else {
        return Some(format!(
            "the versions '{old}' and '{new}' are not semantic versions"
        ));
    };
    let need = required_version(o, level);
    let format = |v: (u64, u64, u64)| format!("{}.{}.{}", v.0, v.1, v.2);
    if n >= need || (level == Level::Patch && n > o) {
        None
    } else if level == Level::Patch && n == o {
        // Nothing but the implementation changed: the same version is a republication.
        Some(format!(
            "version {} is the old version; changed sources need at least {}",
            format(n),
            format(need)
        ))
    } else {
        Some(format!(
            "version {} is too small for {} changes: they need at least {}",
            format(n),
            level.name(),
            format(need)
        ))
    }
}

/// The level of a set of changes and the problem with the new version, if any. Identical APIs
/// with the same version are no problem.
pub fn verdict(old: &Api, new: &Api, changes: &[Change]) -> (Level, Option<String>) {
    let level = changes
        .iter()
        .map(|c| c.level)
        .max()
        .unwrap_or(Level::Patch);
    if changes.is_empty() && parse_version(&old.version).is_some() && old.version == new.version {
        return (level, None);
    }
    (level, version_problem(&old.version, &new.version, level))
}

// ---------------------------------------------------------------- callers and migrations

/// The byte ranges of the arguments of the first call of `name` in `text`, and the range of
/// the name: a small scanner that skips texts and nested brackets.
/// A byte range of a text.
type Range = (usize, usize);

fn call_arguments(text: &str, name: &str) -> Option<(Range, Vec<Range>)> {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(found) = text[from..].find(name) {
        let start = from + found;
        let end = start + name.len();
        from = end;
        let before_ok = start == 0 || {
            let c = bytes[start - 1] as char;
            !(c.is_alphanumeric() || c == '_')
        };
        let after = text[end..].trim_start();
        if !before_ok || !(after.starts_with('(') || after.starts_with('[')) {
            continue;
        }
        if text[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        // Skip explicit type arguments.
        let mut i = end + (text[end..].len() - after.len());
        if bytes.get(i) == Some(&b'[') {
            i = matching(bytes, i)? + 1;
        }
        if bytes.get(i) != Some(&b'(') {
            continue;
        }
        let close = matching(bytes, i)?;
        let mut args = Vec::new();
        let mut depth = 0;
        let mut arg_start = i + 1;
        let mut k = i + 1;
        while k < close {
            match bytes[k] {
                b'"' => k = skip_text(bytes, k),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => {
                    args.push((arg_start, k));
                    arg_start = k + 1;
                }
                _ => {}
            }
            k += 1;
        }
        if !text[arg_start..close].trim().is_empty() {
            args.push((arg_start, close));
        }
        return Some(((start, end), args));
    }
    None
}

fn skip_text(bytes: &[u8], mut k: usize) -> usize {
    k += 1;
    while k < bytes.len() && bytes[k] != b'"' {
        if bytes[k] == b'\\' {
            k += 1;
        }
        k += 1;
    }
    k
}

/// The index of the bracket that closes the one at `open`.
fn matching(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut k = open;
    while k < bytes.len() {
        match bytes[k] {
            b'"' => k = skip_text(bytes, k),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// The label of an argument, `name = value`, with its range.
fn label_of(text: &str, (start, end): (usize, usize)) -> Option<(String, usize, usize)> {
    let arg = &text[start..end];
    let trimmed = arg.trim_start();
    let offset = start + (arg.len() - trimmed.len());
    let name: String = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    let rest = trimmed[name.len()..].trim_start();
    (rest.starts_with('=') && !rest.starts_with("==")).then(|| {
        let len = name.len();
        (name, offset, offset + len)
    })
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, column)
}

/// The edit of a rewrite at one call, as `file:line:column: old -> new`.
fn edit_at(
    path: &str,
    source: &str,
    span: (usize, usize),
    function: &str,
    rewrite: &Rewrite,
) -> Option<String> {
    let text = source.get(span.0..span.1)?;
    let ((name_start, name_end), args) = call_arguments(text, function)?;
    let at = |offset: usize| line_column(source, span.0 + offset);
    match rewrite {
        Rewrite::RenameFunction { to, .. } => {
            let (l, c) = at(name_start);
            Some(format!(
                "{path}:{l}:{c}: {} -> {to}",
                &text[name_start..name_end]
            ))
        }
        Rewrite::RenameLabel { from, to } => args.iter().find_map(|a| {
            let (label, s, _) = label_of(text, *a)?;
            (label == *from).then(|| {
                let (l, c) = at(s);
                format!("{path}:{l}:{c}: {from} -> {to}")
            })
        }),
        Rewrite::InsertLabel { index, label } => {
            let positional: Vec<&(usize, usize)> = args
                .iter()
                .filter(|a| label_of(text, **a).is_none())
                .collect();
            let a = positional.get(*index)?;
            let arg = &text[a.0..a.1];
            let offset = a.0 + (arg.len() - arg.trim_start().len());
            let (l, c) = at(offset);
            Some(format!("{path}:{l}:{c}: insert '{label} = '"))
        }
    }
}

/// Finds the places of the caller package at `root` that use the changed items of the package
/// `name`, and the edits of the unambiguous migrations there.
pub fn find_callers(root: &Path, name: &str, changes: &mut [Change]) -> Result<(), String> {
    let root = root.to_path_buf();
    let name = name.to_string();
    let mut owned = changes.to_vec();
    let owned =
        crate::on_front_end_stack(move || callers_here(&root, &name, &mut owned).map(|()| owned))?;
    changes.clone_from_slice(&owned);
    Ok(())
}

fn callers_here(root: &Path, name: &str, changes: &mut [Change]) -> Result<(), String> {
    let canonical =
        crate::vfs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let root = canonical.as_path();
    let files = package_files(root);
    let program = crate::load_checked(&files, crate::effects::Profile::Application);
    let prefix = format!("{name}@");
    let is_target = |m: usize| {
        program.modules[m]
            .dependency
            .as_ref()
            .is_some_and(|d| d.label.starts_with(&prefix))
    };
    let key_of = |m: usize| module_key(&program.modules[m].package, &program.modules[m].path);
    let place = |m: usize, line: usize, column: usize| {
        let path = program.modules[m]
            .path
            .strip_prefix(root)
            .unwrap_or(&program.modules[m].path)
            .to_string_lossy()
            .replace('\\', "/");
        format!("{path}:{line}:{column}")
    };
    // Functions by their key in the API.
    let mut function_keys: BTreeMap<FnId, String> = BTreeMap::new();
    for (f, def) in program.funcs.iter().enumerate() {
        if !is_target(def.module) {
            continue;
        }
        let key = match def.owner {
            Owner::Module => format!("{}.{}", key_of(def.module), def.name),
            Owner::Impl(i) => {
                let target = program.show(&program.impls[i].target);
                let target = target.split('[').next().unwrap_or(&target).to_string();
                format!("{}.{target}.{}", key_of(def.module), def.name)
            }
            Owner::Interface(_) => continue,
        };
        function_keys.insert(f, key);
    }
    let hashes = symbol::hashes(&program);
    for change in changes.iter_mut() {
        let mut callers = BTreeSet::new();
        let mut edits = BTreeSet::new();
        for (f, m, span) in &program.usages {
            if is_target(*m) || program.modules[*m].stdlib {
                continue;
            }
            if function_keys.get(f) != Some(&change.item) {
                continue;
            }
            callers.insert(place(*m, span.start.line, span.start.column));
            if let Some(rewrite) = &change.rewrite {
                let function = &program.funcs[*f].name;
                let source = &program.modules[*m].analysis.source;
                let path = place(*m, 0, 0);
                let path = path.trim_end_matches(":0:0");
                if let Some(edit) = edit_at(
                    path,
                    source,
                    (span.start.offset, span.end.offset),
                    function,
                    rewrite,
                ) {
                    edits.insert(edit);
                }
            }
        }
        // Types, variants and constants: the declarations of the caller that reference them.
        for (d, def) in program.defs.iter().enumerate() {
            if !is_target(def.module)
                || format!("{}.{}", key_of(def.module), def.name) != change.item
            {
                continue;
            }
            for (holder, refs) in &hashes.references {
                if !refs.contains(&Node::Def(d)) {
                    continue;
                }
                let (m, span) = match holder {
                    Node::Func(f) => (program.funcs[*f].module, program.funcs[*f].span),
                    Node::Def(x) => (program.defs[*x].module, program.defs[*x].span),
                    Node::Const(c) => (program.consts[*c].module, program.consts[*c].span),
                };
                if !is_target(m) && !program.modules[m].stdlib {
                    callers.insert(place(m, span.start.line, span.start.column));
                }
            }
        }
        change.callers = callers.into_iter().collect();
        change.edits = edits.into_iter().collect();
    }
    Ok(())
}

// ---------------------------------------------------------------- output

fn item_json(name: &str, item: &Item) -> Json {
    let strings =
        |list: &[String]| Json::Array(list.iter().map(|s| Json::str(s.clone())).collect());
    let mut fields = vec![
        ("name", Json::str(name)),
        ("kind", Json::str(item.kind)),
        ("declared", Json::str(item.declared.clone())),
        ("hash", Json::str(item.hash.clone())),
    ];
    if !item.generics.is_empty() {
        fields.push((
            "generics",
            Json::Array(
                item.generics
                    .iter()
                    .map(|(n, b)| {
                        Json::object(vec![
                            ("name", Json::str(n.clone())),
                            ("constraints", strings(b)),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    if item.kind == "function" {
        fields.push(("signature", Json::str(shape(item))));
        fields.push((
            "params",
            Json::Array(
                item.params
                    .iter()
                    .map(|q| {
                        Json::object(vec![
                            ("name", Json::str(q.name.clone())),
                            ("type", Json::str(q.ty.clone())),
                            ("named", Json::Bool(q.named)),
                            ("var", Json::Bool(q.var)),
                            (
                                "default",
                                q.default.clone().map(Json::str).unwrap_or(Json::Null),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ));
        fields.push(("returns", Json::str(item.returns.clone())));
        fields.push(("fails", strings(&item.fails)));
        fields.push(("effects", strings(&item.effects)));
    }
    if matches!(item.kind, "struct" | "error") {
        fields.push((
            "fields",
            Json::Array(
                item.fields
                    .iter()
                    .map(|f| {
                        Json::object(vec![
                            ("name", Json::str(f.name.clone())),
                            ("type", Json::str(f.ty.clone())),
                            ("var", Json::Bool(f.var)),
                            ("default", Json::Bool(f.default)),
                        ])
                    })
                    .collect(),
            ),
        ));
        fields.push(("private_fields", Json::Bool(item.private_fields)));
    }
    if item.kind == "enum" {
        fields.push((
            "variants",
            Json::Array(
                item.variants
                    .iter()
                    .map(|(n, f)| {
                        Json::object(vec![("name", Json::str(n.clone())), ("fields", strings(f))])
                    })
                    .collect(),
            ),
        ));
    }
    if item.kind == "interface" {
        fields.push(("members", strings(&item.members)));
    }
    if item.kind == "constant" {
        fields.push(("type", Json::str(item.returns.clone())));
    }
    Json::object(fields)
}

/// The API as JSON.
pub fn api_json(api: &Api) -> Json {
    Json::object(vec![
        ("scheme", Json::str(API_SCHEME)),
        ("package", Json::str(api.name.clone())),
        ("version", Json::str(api.version.clone())),
        (
            "permissions",
            Json::Array(
                api.permissions
                    .iter()
                    .map(|p| Json::str(p.clone()))
                    .collect(),
            ),
        ),
        (
            "items",
            Json::Array(api.items.iter().map(|(n, i)| item_json(n, i)).collect()),
        ),
    ])
}

/// The API as text, one declaration per line.
pub fn api_text(api: &Api) -> String {
    let mut out = format!("{} {} ({API_SCHEME})\n", api.name, api.version);
    for p in &api.permissions {
        out.push_str(&format!("permission {p}\n"));
    }
    for (name, item) in &api.items {
        let detail = match item.kind {
            "function" => {
                let mut s = shape(item);
                if !item.effects.is_empty() {
                    s.push_str(&format!(" uses {}", item.effects.join(", ")));
                }
                s
            }
            "struct" | "error" => {
                let fields: Vec<String> = item
                    .fields
                    .iter()
                    .map(|f| format!("{} as {}", f.name, f.ty))
                    .collect();
                format!(
                    "({}{})",
                    fields.join(", "),
                    if item.private_fields {
                        ", private fields"
                    } else {
                        ""
                    }
                )
            }
            "enum" => format!("({})", names(&item.variants).join(", ")),
            "interface" => format!("({})", item.members.join("; ")),
            _ => format!(" as {}", item.returns),
        };
        out.push_str(&format!("{} {name}{detail}\n", item.kind));
    }
    out
}

/// The report of a comparison as JSON.
pub fn diff_json(old: &Api, new: &Api, changes: &[Change]) -> Json {
    let (level, problem) = verdict(old, new, changes);
    Json::object(vec![
        ("scheme", Json::str(API_SCHEME)),
        ("package", Json::str(new.name.clone())),
        ("old_version", Json::str(old.version.clone())),
        ("new_version", Json::str(new.version.clone())),
        ("level", Json::str(level.name())),
        (
            "required_version",
            parse_version(&old.version)
                .map(|o| {
                    let v = required_version(o, level);
                    Json::str(format!("{}.{}.{}", v.0, v.1, v.2))
                })
                .unwrap_or(Json::Null),
        ),
        ("version_ok", Json::Bool(problem.is_none())),
        (
            "version_problem",
            problem.map(Json::str).unwrap_or(Json::Null),
        ),
        (
            "changes",
            Json::Array(
                changes
                    .iter()
                    .map(|c| {
                        Json::object(vec![
                            ("level", Json::str(c.level.name())),
                            ("item", Json::str(c.item.clone())),
                            ("message", Json::str(c.message.clone())),
                            (
                                "migration",
                                c.rewrite
                                    .as_ref()
                                    .map(|r| Json::str(r.describe()))
                                    .unwrap_or(Json::Null),
                            ),
                            (
                                "callers",
                                Json::Array(
                                    c.callers.iter().map(|s| Json::str(s.clone())).collect(),
                                ),
                            ),
                            (
                                "edits",
                                Json::Array(c.edits.iter().map(|s| Json::str(s.clone())).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// The report of a comparison as text.
pub fn diff_text(old: &Api, new: &Api, changes: &[Change]) -> String {
    let (level, problem) = verdict(old, new, changes);
    let mut out = format!("{} {} -> {}\n", new.name, old.version, new.version);
    for c in changes {
        out.push_str(&format!(
            "{:<5} {}: {}\n",
            c.level.name(),
            c.item,
            c.message
        ));
        if let Some(r) = &c.rewrite {
            out.push_str(&format!("      migration: {}\n", r.describe()));
        }
        for caller in &c.callers {
            out.push_str(&format!("      caller: {caller}\n"));
        }
        for edit in &c.edits {
            out.push_str(&format!("      edit: {edit}\n"));
        }
    }
    if changes.is_empty() {
        out.push_str("no changes to the public API\n");
    }
    match problem {
        None => out.push_str(&format!(
            "level: {}; version {} is enough\n",
            level.name(),
            new.version
        )),
        Some(problem) => out.push_str(&format!("level: {}; {problem}\n", level.name())),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_semantic_version_rule() {
        assert_eq!(required_version((1, 2, 3), Level::Major), (2, 0, 0));
        assert_eq!(required_version((1, 2, 3), Level::Minor), (1, 3, 0));
        assert_eq!(required_version((1, 2, 3), Level::Patch), (1, 2, 4));
        assert_eq!(required_version((0, 2, 3), Level::Major), (0, 3, 0));
        assert_eq!(required_version((0, 2, 3), Level::Minor), (0, 2, 4));
        assert!(version_problem("1.2.3", "2.0.0", Level::Major).is_none());
        assert!(version_problem("1.2.3", "1.3.0", Level::Major).is_some());
        assert!(version_problem("0.2.3", "0.3.0", Level::Major).is_none());
        assert!(version_problem("1.2.3", "1.2.3", Level::Patch).is_some());
        assert!(version_problem("1.2.3", "1.2.4", Level::Patch).is_none());
        assert!(version_problem("x", "1.0.0", Level::Patch).is_some());
    }

    #[test]
    fn the_scanner_finds_arguments_and_labels() {
        let text = "pad(\"a, (b\", width = 3, f(1, 2))";
        let ((s, e), args) = call_arguments(text, "pad").unwrap();
        assert_eq!(&text[s..e], "pad");
        assert_eq!(args.len(), 3);
        assert_eq!(
            label_of(text, args[1]).map(|l| l.0),
            Some("width".to_string())
        );
        assert!(label_of(text, args[0]).is_none());
        assert!(label_of("x == 1", (0, 6)).is_none());
    }
}
