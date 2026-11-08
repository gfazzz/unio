//! A program: the modules a check needs, their declarations, imports and types
//! (docs/semantics.md, sections 1–3). The bodies of functions are checked by `check.rs`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast;
use crate::catalog;
use crate::diag::{Diagnostic, Span};
use crate::types::{DefId, FnTy, ParamId, Prim, Ty};
use crate::Analysis;

pub type ModId = usize;
pub type FnId = usize;
pub type ConstId = usize;
pub type ImplId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    Func(FnId),
    Def(DefId),
    Const(ConstId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    Module(ModId),
    Symbol(ModId, Symbol),
}

pub struct Module {
    pub name: String,
    pub path: PathBuf,
    pub analysis: Analysis,
    pub stdlib: bool,
    pub prelude: bool,
    pub package: PathBuf,
    pub report: bool,
    pub symbols: HashMap<String, Symbol>,
    pub imports: HashMap<String, Binding>,
    /// Imported modules with the span of their import, for cycles.
    pub edges: Vec<(ModId, Span)>,
}

pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    pub public: bool,
    /// Declared `var`: the field can change through a mutable place (L-11).
    pub var: bool,
    pub has_default: bool,
}

pub struct VariantDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

pub enum DefKind {
    Struct(Vec<FieldDef>),
    Error(Vec<FieldDef>),
    Enum(Vec<VariantDef>),
    Interface(Vec<FnId>),
}

pub struct Def {
    pub module: ModId,
    pub name: String,
    pub public: bool,
    pub span: Span,
    pub generics: Vec<ParamId>,
    pub kind: DefKind,
}

pub struct ParamDef {
    pub name: String,
    pub ty: Ty,
    /// A `var` parameter: the argument is a `var` name of the caller (L-11).
    pub var: bool,
    pub named_only: bool,
    pub has_default: bool,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Module,
    Impl(ImplId),
    Interface(DefId),
}

/// Where the body of a function is: the item and, in an extend or interface, the member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyRef {
    pub module: ModId,
    pub item: usize,
    pub member: Option<usize>,
}

pub struct FnDef {
    pub module: ModId,
    pub name: String,
    pub public: bool,
    pub generics: Vec<ParamId>,
    pub params: Vec<ParamDef>,
    /// Some(var) for a method with `self` or `var self`.
    pub self_param: Option<bool>,
    pub ret: Ty,
    pub fails: Vec<Ty>,
    pub body: Option<BodyRef>,
    pub owner: Owner,
    pub span: Span,
}

pub struct ParamInfo {
    pub name: String,
    pub bounds: Vec<Ty>,
}

pub struct Impl {
    pub module: ModId,
    pub generics: Vec<ParamId>,
    pub target: Ty,
    pub interface: Option<Ty>,
    pub methods: Vec<FnId>,
    pub span: Span,
}

pub struct ConstDef {
    pub module: ModId,
    pub name: String,
    pub public: bool,
    pub annotated: Option<Ty>,
    pub item: usize,
    pub span: Span,
}

/// A call of one function from another.
#[derive(Clone, Copy, Debug)]
pub struct CallEdge {
    pub caller: FnId,
    pub callee: FnId,
    pub module: ModId,
    pub span: Span,
}

/// The layers of the standard library in order (S-11): each may use itself and those before it.
pub const LAYER_NAMES: [&str; 3] = ["core", "alloc", "std"];

pub struct Program {
    pub modules: Vec<Module>,
    pub defs: Vec<Def>,
    pub funcs: Vec<FnDef>,
    pub consts: Vec<ConstDef>,
    pub impls: Vec<Impl>,
    pub params: Vec<ParamInfo>,
    pub prelude: HashMap<String, (ModId, Symbol)>,
    pub diagnostics: Vec<(ModId, Diagnostic)>,
    /// Calls found by the checks: caller, callee, and the place of the call (sprint S4).
    pub calls: Vec<CallEdge>,
    /// Places where a function is named, for `unio symbol`.
    pub usages: Vec<(FnId, ModId, Span)>,
    /// What the effects need beyond `calls`: implicit calls through interfaces, uses of
    /// constants and field defaults, and allocating expressions (semantics 8.4).
    pub effect_facts: crate::effects::Facts,
    by_path: HashMap<PathBuf, ModId>,
    stdlib_root: Option<PathBuf>,
}

/// The directory of the standard library: `UNIO_STDLIB`, an ancestor of the first file that
/// holds `stdlib/core/prelude.unio`, or the repository of the compiler.
fn find_stdlib(start: &Path) -> Option<PathBuf> {
    if let Some(embedded) = crate::vfs::embedded_stdlib() {
        return embedded;
    }
    if let Ok(dir) = std::env::var("UNIO_STDLIB") {
        let dir = PathBuf::from(dir);
        if dir.join("core/prelude.unio").is_file() {
            return Some(dir);
        }
    }
    let absolute = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    for ancestor in absolute.ancestors() {
        let candidate = ancestor.join("stdlib");
        if candidate.join("core/prelude.unio").is_file() {
            return Some(candidate);
        }
    }
    let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
    if built_in.join("core/prelude.unio").is_file() {
        return std::fs::canonicalize(built_in).ok();
    }
    None
}

const LAYERS: [&str; 3] = ["core", "alloc", "std"];

fn package_root(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    for ancestor in dir.ancestors() {
        if crate::vfs::is_file(&ancestor.join("unio.toml")) {
            return ancestor.to_path_buf();
        }
    }
    dir
}

/// The files that may declare the module named by a prefix of a symbol name: a module of the
/// standard library (`std.files`) or of the package around `start`. Longer prefixes first.
pub fn symbol_module_files(query: &str, start: &Path) -> Vec<PathBuf> {
    let parts: Vec<&str> = query.split('.').collect();
    let stdlib = find_stdlib(&start.join("unio.toml"));
    let package = package_root(&start.join("unio.toml"));
    let mut found = Vec::new();
    for end in (1..parts.len()).rev() {
        let module = &parts[..end];
        if module[0] == "std" && module.len() > 1 {
            if let Some(root) = &stdlib {
                let rest = module[1..].join("/") + ".unio";
                found.extend(
                    LAYERS
                        .iter()
                        .map(|l| root.join(l).join(&rest))
                        .filter(|p| p.is_file()),
                );
            }
        } else {
            let file = package.join(module.join("/") + ".unio");
            if file.is_file() {
                found.push(file);
            }
        }
    }
    if found.is_empty() {
        if let Some(root) = stdlib {
            found.push(root.join("core/prelude.unio"));
        }
    }
    found
}

impl Program {
    /// The directory of the standard library the program was loaded with.
    pub fn stdlib_root(&self) -> Option<&Path> {
        self.stdlib_root.as_deref()
    }

    /// The layer of a module of the standard library: 0 core, 1 alloc, 2 std; None elsewhere.
    pub fn module_layer(&self, module: ModId) -> Option<usize> {
        let m = &self.modules[module];
        if !m.stdlib {
            return None;
        }
        let parts: Vec<String> = m
            .path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let at = parts.iter().rposition(|c| c == "stdlib")?;
        LAYER_NAMES
            .iter()
            .position(|l| parts.get(at + 1).map(String::as_str) == Some(*l))
    }

    /// Loads the files, what they import, and the standard library.
    pub fn load(files: &[PathBuf]) -> Program {
        let stdlib_root = files.first().and_then(|f| find_stdlib(f));
        let mut program = Program {
            modules: Vec::new(),
            defs: Vec::new(),
            funcs: Vec::new(),
            consts: Vec::new(),
            impls: Vec::new(),
            params: Vec::new(),
            prelude: HashMap::new(),
            diagnostics: Vec::new(),
            calls: Vec::new(),
            usages: Vec::new(),
            effect_facts: Default::default(),
            by_path: HashMap::new(),
            stdlib_root,
        };
        if let Some(root) = program.stdlib_root.clone() {
            for layer in LAYERS {
                let dir = root.join(layer);
                let mut preludes = vec![dir.join("prelude.unio")];
                preludes.extend(crate::vfs::unio_files_in(&dir.join("builtin")));
                for path in preludes {
                    if crate::vfs::is_file(&path) {
                        program.load_file(&path, false);
                    }
                }
            }
        }
        for file in files {
            let id = program.load_file(file, true);
            if let Some(id) = id {
                program.modules[id].report = true;
            }
        }
        // Imports load further modules; follow them until nothing new appears.
        let mut next = 0;
        while next < program.modules.len() {
            program.load_imports(next);
            next += 1;
        }
        program.collect_declarations();
        program.build_prelude();
        program.resolve_imports();
        program.find_cycles();
        program.resolve_signatures();
        program.check_impls();
        program
    }

    fn stdlib_name(&self, path: &Path) -> Option<(String, bool)> {
        let root = self.stdlib_root.as_ref()?;
        let relative = path.strip_prefix(root).ok()?;
        let parts: Vec<String> = relative
            .iter()
            .map(|p| p.to_string_lossy().trim_end_matches(".unio").to_string())
            .collect();
        let rest = &parts[1..];
        if rest == ["prelude"] || rest.first().is_some_and(|p| p == "builtin") {
            return Some((format!("prelude:{}", parts.join("/")), true));
        }
        Some((format!("std.{}", rest.join(".")), false))
    }

    fn load_file(&mut self, file: &Path, report: bool) -> Option<ModId> {
        let canonical = crate::vfs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        if let Some(id) = self.by_path.get(&canonical) {
            return Some(*id);
        }
        let source = crate::vfs::read_to_string(&canonical).ok()?;
        let analysis = crate::analyse(&file.to_string_lossy(), &source);
        let (name, prelude, stdlib, package) = match self.stdlib_name(&canonical) {
            Some((name, prelude)) => (name, prelude, true, self.stdlib_root.clone().unwrap()),
            None => {
                let package = package_root(&canonical);
                let relative = canonical.strip_prefix(&package).unwrap_or(&canonical);
                let name = relative
                    .iter()
                    .map(|p| p.to_string_lossy().trim_end_matches(".unio").to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                (name, false, false, package)
            }
        };
        let id = self.modules.len();
        self.modules.push(Module {
            name,
            path: canonical.clone(),
            analysis,
            stdlib,
            prelude,
            package,
            report,
            symbols: HashMap::new(),
            imports: HashMap::new(),
            edges: Vec::new(),
        });
        self.by_path.insert(canonical, id);
        Some(id)
    }

    /// The file of a module path written in an import, if it exists and stays inside the root
    /// it was found in.
    fn module_file(&self, from: ModId, path: &[String]) -> Option<PathBuf> {
        self.locate_module(from, path).ok()
    }

    /// Finds the file of a module path: below the standard library for `std.*`, below the
    /// package otherwise. A file whose resolved path (links followed) leaves that root is
    /// `Err(true)`: an import cannot lead out of the package or the standard library through
    /// a link. A missing file is `Err(false)`.
    fn locate_module(&self, from: ModId, path: &[String]) -> Result<PathBuf, bool> {
        let (root, file) = if path.first().is_some_and(|p| p == "std") && path.len() > 1 {
            let root = self.stdlib_root.as_ref().ok_or(false)?;
            let rest = path[1..].join("/") + ".unio";
            let file = LAYERS
                .iter()
                .map(|layer| root.join(layer).join(&rest))
                .find(|p| crate::vfs::is_file(p))
                .ok_or(false)?;
            (root, file)
        } else {
            let root = &self.modules[from].package;
            let file = root.join(path.join("/") + ".unio");
            if !crate::vfs::is_file(&file) {
                return Err(false);
            }
            (root, file)
        };
        let root = crate::vfs::canonicalize(root).unwrap_or_else(|_| root.clone());
        match crate::vfs::canonicalize(&file) {
            Ok(real) if real.starts_with(&root) => Ok(file),
            _ => Err(true),
        }
    }

    fn load_imports(&mut self, id: ModId) {
        let imports: Vec<Vec<String>> = self.modules[id]
            .analysis
            .file
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Import(i) => Some(i.module.iter().map(|n| n.text.clone()).collect()),
                _ => None,
            })
            .collect();
        let report = self.modules[id].report && !self.modules[id].stdlib;
        for path in imports {
            if let Some(file) = self.module_file(id, &path) {
                let before = self.modules.len();
                if let Some(loaded) = self.load_file(&file, false) {
                    if loaded >= before && report && !self.modules[loaded].stdlib {
                        self.modules[loaded].report = true;
                    }
                }
            }
        }
    }

    pub fn error(&mut self, module: ModId, diagnostic: Diagnostic) {
        self.diagnostics.push((module, diagnostic));
    }

    // ------------------------------------------------------------ declarations

    fn new_params(&mut self, generics: &[ast::GenericParam]) -> Vec<ParamId> {
        generics
            .iter()
            .map(|g| {
                self.params.push(ParamInfo {
                    name: g.name.text.clone(),
                    bounds: Vec::new(),
                });
                self.params.len() - 1
            })
            .collect()
    }

    fn declare(&mut self, module: ModId, name: &ast::Name, symbol: Symbol) {
        if self.modules[module].symbols.contains_key(&name.text) {
            self.error(
                module,
                Diagnostic::error("E0051", name.span, catalog::duplicate(&name.text)),
            );
            return;
        }
        self.modules[module]
            .symbols
            .insert(name.text.clone(), symbol);
    }

    fn placeholder_fn(
        &mut self,
        module: ModId,
        sig: &ast::Signature,
        public: bool,
        body: Option<BodyRef>,
        owner: Owner,
    ) -> FnId {
        let generics = self.new_params(&sig.generics);
        let self_param = sig.params.iter().find_map(|p| match p {
            ast::Param::SelfParam { var, .. } => Some(*var),
            _ => None,
        });
        self.funcs.push(FnDef {
            module,
            name: sig.name.text.clone(),
            public,
            generics,
            params: Vec::new(),
            self_param,
            ret: Ty::Unknown,
            fails: Vec::new(),
            body,
            owner,
            span: sig.name.span,
        });
        self.funcs.len() - 1
    }

    fn collect_declarations(&mut self) {
        for module in 0..self.modules.len() {
            let items = self.modules[module].analysis.file.items.clone();
            for (index, item) in items.iter().enumerate() {
                match item {
                    ast::Item::Declaration(d) => {
                        let public = d.public;
                        let body = Some(BodyRef {
                            module,
                            item: index,
                            member: None,
                        });
                        match &d.kind {
                            ast::DeclarationKind::Function(f) => {
                                let id = self.placeholder_fn(
                                    module,
                                    &f.signature,
                                    public,
                                    body,
                                    Owner::Module,
                                );
                                self.declare(module, &f.signature.name, Symbol::Func(id));
                            }
                            ast::DeclarationKind::External(sig) => {
                                let id =
                                    self.placeholder_fn(module, sig, public, None, Owner::Module);
                                self.declare(module, &sig.name, Symbol::Func(id));
                            }
                            ast::DeclarationKind::Constant(c) => {
                                self.consts.push(ConstDef {
                                    module,
                                    name: c.name.text.clone(),
                                    public,
                                    annotated: None,
                                    item: index,
                                    span: c.name.span,
                                });
                                let id = self.consts.len() - 1;
                                self.declare(module, &c.name, Symbol::Const(id));
                            }
                            ast::DeclarationKind::Struct(s) => {
                                let generics = self.new_params(&s.generics);
                                let id = self.add_def(
                                    module,
                                    &s.name,
                                    public,
                                    generics,
                                    DefKind::Struct(Vec::new()),
                                );
                                self.declare(module, &s.name, Symbol::Def(id));
                            }
                            ast::DeclarationKind::Error(e) => {
                                let id = self.add_def(
                                    module,
                                    &e.name,
                                    public,
                                    Vec::new(),
                                    DefKind::Error(Vec::new()),
                                );
                                self.declare(module, &e.name, Symbol::Def(id));
                            }
                            ast::DeclarationKind::Enum(e) => {
                                let generics = self.new_params(&e.generics);
                                let id = self.add_def(
                                    module,
                                    &e.name,
                                    public,
                                    generics,
                                    DefKind::Enum(Vec::new()),
                                );
                                self.declare(module, &e.name, Symbol::Def(id));
                            }
                            ast::DeclarationKind::Interface(i) => {
                                let generics = self.new_params(&i.generics);
                                let id = self.add_def(
                                    module,
                                    &i.name,
                                    public,
                                    generics,
                                    DefKind::Interface(Vec::new()),
                                );
                                self.declare(module, &i.name, Symbol::Def(id));
                                let mut methods = Vec::new();
                                for (m, (sig, block)) in i.members.iter().enumerate() {
                                    let body = block.as_ref().map(|_| BodyRef {
                                        module,
                                        item: index,
                                        member: Some(m),
                                    });
                                    methods.push(self.placeholder_fn(
                                        module,
                                        sig,
                                        true,
                                        body,
                                        Owner::Interface(id),
                                    ));
                                }
                                self.defs[id].kind = DefKind::Interface(methods);
                            }
                        }
                    }
                    ast::Item::Extend(e) => {
                        let generics = self.new_params(&e.generics);
                        let impl_id = self.impls.len();
                        self.impls.push(Impl {
                            module,
                            generics,
                            target: Ty::Unknown,
                            interface: None,
                            methods: Vec::new(),
                            span: e.span,
                        });
                        let mut methods = Vec::new();
                        for (m, member) in e.members.iter().enumerate() {
                            let body = member.body.as_ref().map(|_| BodyRef {
                                module,
                                item: index,
                                member: Some(m),
                            });
                            methods.push(self.placeholder_fn(
                                module,
                                &member.signature,
                                member.public || e.interface.is_some(),
                                body,
                                Owner::Impl(impl_id),
                            ));
                        }
                        self.impls[impl_id].methods = methods;
                    }
                    _ => {}
                }
            }
        }
    }

    fn add_def(
        &mut self,
        module: ModId,
        name: &ast::Name,
        public: bool,
        generics: Vec<ParamId>,
        kind: DefKind,
    ) -> DefId {
        self.defs.push(Def {
            module,
            name: name.text.clone(),
            public,
            span: name.span,
            generics,
            kind,
        });
        self.defs.len() - 1
    }

    fn build_prelude(&mut self) {
        for (id, module) in self.modules.iter().enumerate() {
            if !module.prelude {
                continue;
            }
            for (name, symbol) in &module.symbols {
                if self.symbol_public(*symbol) {
                    self.prelude.entry(name.clone()).or_insert((id, *symbol));
                }
            }
        }
    }

    pub fn symbol_public(&self, symbol: Symbol) -> bool {
        match symbol {
            Symbol::Func(f) => self.funcs[f].public,
            Symbol::Def(d) => self.defs[d].public,
            Symbol::Const(c) => self.consts[c].public,
        }
    }

    // ------------------------------------------------------------ imports

    fn resolve_imports(&mut self) {
        for module in 0..self.modules.len() {
            let items = self.modules[module].analysis.file.items.clone();
            for item in &items {
                let ast::Item::Import(import) = item else {
                    continue;
                };
                let path: Vec<String> = import.module.iter().map(|n| n.text.clone()).collect();
                let dotted = path.join(".");
                let located = self.locate_module(module, &path);
                let target = located
                    .as_ref()
                    .ok()
                    .and_then(|f| crate::vfs::canonicalize(f).ok())
                    .and_then(|f| self.by_path.get(&f).copied());
                let Some(target) = target else {
                    let span = import.module.first().unwrap().span.to(import
                        .module
                        .last()
                        .unwrap()
                        .span);
                    let message = if located == Err(true) {
                        catalog::module_outside(&dotted)
                    } else {
                        catalog::module_not_found(&dotted)
                    };
                    self.error(module, Diagnostic::error("E0053", span, message));
                    continue;
                };
                self.modules[module].edges.push((target, import.span));
                let mut bindings = Vec::new();
                if import.names.is_empty() {
                    let name = import
                        .alias
                        .clone()
                        .unwrap_or_else(|| import.module.last().unwrap().clone());
                    bindings.push((name, Binding::Module(target)));
                } else {
                    for name in &import.names {
                        match self.modules[target].symbols.get(&name.text).copied() {
                            Some(symbol) if self.symbol_public(symbol) => {
                                bindings.push((name.clone(), Binding::Symbol(target, symbol)));
                            }
                            _ => self.error(
                                module,
                                Diagnostic::error(
                                    "E0054",
                                    name.span,
                                    catalog::name_not_in_module(&name.text, &dotted),
                                ),
                            ),
                        }
                    }
                }
                for (name, binding) in bindings {
                    let taken = self.modules[module].symbols.contains_key(&name.text)
                        || self.modules[module].imports.contains_key(&name.text);
                    if taken {
                        self.error(
                            module,
                            Diagnostic::error(
                                "E0052",
                                name.span,
                                catalog::import_collision(&name.text),
                            ),
                        );
                    } else {
                        self.modules[module]
                            .imports
                            .insert(name.text.clone(), binding);
                    }
                }
            }
        }
    }

    fn find_cycles(&mut self) {
        // A module is in a cycle when it can reach itself through imports.
        for start in 0..self.modules.len() {
            if self.modules[start].stdlib {
                continue;
            }
            for (first, span) in self.modules[start].edges.clone() {
                let mut seen = HashSet::new();
                let mut stack = vec![(first, vec![start, first])];
                let mut cycle = None;
                while let Some((at, chain)) = stack.pop() {
                    if at == start {
                        cycle = Some(chain);
                        break;
                    }
                    if !seen.insert(at) {
                        continue;
                    }
                    for (next, _) in &self.modules[at].edges {
                        let mut longer = chain.clone();
                        longer.push(*next);
                        stack.push((*next, longer));
                    }
                }
                if let Some(chain) = cycle {
                    let names: Vec<String> = chain
                        .iter()
                        .map(|m| self.modules[*m].name.clone())
                        .collect();
                    self.error(
                        start,
                        Diagnostic::error(
                            "E0058",
                            span,
                            catalog::import_cycle(&names.join(" -> ")),
                        ),
                    );
                    break;
                }
            }
        }
    }

    // ------------------------------------------------------------ names of types

    /// What a name means in a module, outside any function: its own declarations, then its
    /// imports, then the prelude.
    pub fn lookup(&self, module: ModId, name: &str) -> Option<(ModId, Symbol)> {
        if let Some(symbol) = self.modules[module].symbols.get(name) {
            return Some((module, *symbol));
        }
        if let Some(Binding::Symbol(m, symbol)) = self.modules[module].imports.get(name) {
            return Some((*m, *symbol));
        }
        // The public error types of an imported module are visible by their names, as the
        // reference programs and concept section 3.2 write them: `on IoError as e` after
        // `import std.files`. Other types are written with the module name.
        for binding in self.modules[module].imports.values() {
            if let Binding::Module(m) = binding {
                if let Some(Symbol::Def(d)) = self.modules[*m].symbols.get(name) {
                    if self.defs[*d].public && matches!(self.defs[*d].kind, DefKind::Error(_)) {
                        return Some((*m, Symbol::Def(*d)));
                    }
                }
            }
        }
        self.prelude.get(name).copied()
    }

    pub fn module_alias(&self, module: ModId, name: &str) -> Option<ModId> {
        match self.modules[module].imports.get(name) {
            Some(Binding::Module(m)) => Some(*m),
            _ => None,
        }
    }

    /// Resolves a written type. `generics` maps the names of generic parameters in scope.
    /// `value` is true where the type describes a value, so that an interface there must be
    /// usable as a type (E0071).
    pub fn resolve_type(
        &mut self,
        module: ModId,
        ty: &ast::Type,
        generics: &HashMap<String, ParamId>,
        value: bool,
    ) -> Ty {
        let names: Vec<&str> = ty.path.iter().map(|n| n.text.as_str()).collect();
        let arguments: Vec<Ty> = ty
            .arguments
            .iter()
            .map(|a| self.resolve_argument(module, a, generics, value))
            .collect();
        let arity = |program: &mut Program, expected: usize| {
            if arguments.len() != expected {
                program.error(
                    module,
                    Diagnostic::error(
                        "E0059",
                        ty.span,
                        catalog::type_arity(&names.join("."), expected, arguments.len()),
                    ),
                );
                false
            } else {
                true
            }
        };
        if names.len() == 1 {
            let name = names[0];
            if let Some(p) = generics.get(name) {
                arity(self, 0);
                return Ty::Param(*p);
            }
            if let Some(prim) = Prim::from_name(name) {
                return if arity(self, 0) {
                    Ty::Prim(prim)
                } else {
                    Ty::Unknown
                };
            }
            let one = |args: &[Ty], i: usize| args.get(i).cloned().unwrap_or(Ty::Unknown);
            match name {
                "Never" => {
                    return if arity(self, 0) {
                        Ty::Never
                    } else {
                        Ty::Unknown
                    }
                }
                "Self" => return Ty::SelfTy,
                "List" => {
                    arity(self, 1);
                    return Ty::List(Box::new(one(&arguments, 0)));
                }
                "Set" => {
                    arity(self, 1);
                    return Ty::Set(Box::new(one(&arguments, 0)));
                }
                "Map" => {
                    arity(self, 2);
                    return Ty::Map(Box::new(one(&arguments, 0)), Box::new(one(&arguments, 1)));
                }
                "Tuple" => return Ty::Tuple(arguments),
                "Function" => {
                    arity(self, 1);
                    return match arguments.into_iter().next() {
                        Some(f @ Ty::Func(_)) => f,
                        _ => Ty::Unknown,
                    };
                }
                _ => {}
            }
        }
        let found = match names.as_slice() {
            [name] => self.lookup(module, name),
            [alias, name] => self.module_alias(module, alias).and_then(|m| {
                let symbol = self.modules[m].symbols.get(*name).copied()?;
                Some((m, symbol))
            }),
            _ => None,
        };
        match found {
            Some((owner, Symbol::Def(def))) => {
                if owner != module && !self.defs[def].public {
                    self.error(
                        module,
                        Diagnostic::error(
                            "E0057",
                            ty.span,
                            catalog::private_used(&names.join(".")),
                        ),
                    );
                }
                if !arity(self, self.defs[def].generics.len()) {
                    return Ty::Unknown;
                }
                if value {
                    if let Some(method) = self.not_object_safe(def) {
                        self.error(
                            module,
                            Diagnostic::error(
                                "E0071",
                                ty.span,
                                catalog::not_object_safe(&self.defs[def].name, &method),
                            ),
                        );
                    }
                }
                Ty::Named(def, arguments)
            }
            _ => {
                let candidates = self.visible_type_names(module);
                let name = names.join(".");
                let mut diagnostic =
                    Diagnostic::error("E0056", ty.span, catalog::unknown_type(&name));
                if let Some(best) = closest(&name, candidates.iter().map(String::as_str)) {
                    diagnostic.fixes.push(crate::diag::Fix {
                        span: ty.path.last().unwrap().span,
                        new_text: best.to_string(),
                        description: catalog::did_you_mean(best),
                    });
                }
                self.error(module, diagnostic);
                Ty::Unknown
            }
        }
    }

    fn visible_type_names(&self, module: ModId) -> Vec<String> {
        let mut names: Vec<String> = [
            "Int", "Float", "Bool", "Text", "Bytes", "Path", "List", "Map", "Set", "Tuple",
            "Function", "Never",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for (name, symbol) in &self.modules[module].symbols {
            if matches!(symbol, Symbol::Def(_)) {
                names.push(name.clone());
            }
        }
        for (name, (_, symbol)) in &self.prelude {
            if matches!(symbol, Symbol::Def(_)) {
                names.push(name.clone());
            }
        }
        names
    }

    fn resolve_argument(
        &mut self,
        module: ModId,
        argument: &ast::TypeArgument,
        generics: &HashMap<String, ParamId>,
        value: bool,
    ) -> Ty {
        match argument {
            ast::TypeArgument::Type(t) => self.resolve_type(module, t, generics, value),
            ast::TypeArgument::Shape {
                params,
                returns,
                fails,
            } => Ty::Func(Box::new(FnTy {
                params: params
                    .iter()
                    .map(|(label, t)| {
                        (
                            label.as_ref().map(|l| l.text.clone()),
                            self.resolve_type(module, t, generics, true),
                        )
                    })
                    .collect(),
                ret: returns
                    .as_ref()
                    .map(|t| self.resolve_type(module, t, generics, true))
                    .unwrap_or(Ty::Unit),
                fails: fails
                    .iter()
                    .map(|t| self.resolve_type(module, t, generics, true))
                    .collect(),
            })),
        }
    }

    /// The method that keeps an interface from being used as a type, if any (L-6b).
    pub fn not_object_safe(&self, def: DefId) -> Option<String> {
        let DefKind::Interface(methods) = &self.defs[def].kind else {
            return None;
        };
        for f in methods {
            let f = &self.funcs[*f];
            if !f.generics.is_empty()
                || f.params.iter().any(|p| p.ty.mentions_self())
                || f.ret.mentions_self()
            {
                return Some(f.name.clone());
            }
        }
        None
    }

    // ------------------------------------------------------------ signatures

    fn generic_scope(
        &mut self,
        module: ModId,
        ids: &[ParamId],
        written: &[ast::GenericParam],
        outer: &HashMap<String, ParamId>,
    ) -> HashMap<String, ParamId> {
        let mut scope = outer.clone();
        for id in ids {
            scope.insert(self.params[*id].name.clone(), *id);
        }
        for (id, g) in ids.iter().zip(written) {
            let bounds: Vec<Ty> = g
                .bounds
                .iter()
                .map(|b| self.resolve_type(module, b, &scope, false))
                .collect();
            self.params[*id].bounds = bounds;
        }
        scope
    }

    fn resolve_fields(
        &mut self,
        module: ModId,
        fields: &Option<Vec<ast::Field>>,
        scope: &HashMap<String, ParamId>,
        always_public: bool,
    ) -> Vec<FieldDef> {
        let mut result = Vec::new();
        for field in fields.iter().flatten() {
            if result.iter().any(|f: &FieldDef| f.name == field.name.text) {
                self.error(
                    module,
                    Diagnostic::error(
                        "E0051",
                        field.name.span,
                        catalog::duplicate(&field.name.text),
                    ),
                );
            }
            let ty = self.resolve_type(module, &field.ty, scope, true);
            result.push(FieldDef {
                name: field.name.text.clone(),
                ty,
                public: field.public || always_public,
                var: field.var,
                has_default: field.default.is_some(),
            });
        }
        result
    }

    fn resolve_signature(
        &mut self,
        id: FnId,
        sig: &ast::Signature,
        outer: &HashMap<String, ParamId>,
    ) {
        let module = self.funcs[id].module;
        let generics = self.funcs[id].generics.clone();
        let scope = self.generic_scope(module, &generics, &sig.generics, outer);
        let mut params = Vec::new();
        let mut seen_named = false;
        let mut default_plain = false;
        let mut default_named = false;
        for param in &sig.params {
            let (name, ty, named_only, default, var) = match param {
                ast::Param::SelfParam { .. } => continue,
                ast::Param::Named { name, ty, default } => {
                    (name, ty, true, default.is_some(), false)
                }
                ast::Param::Plain {
                    name,
                    ty,
                    default,
                    var,
                } => (name, ty, false, default.is_some(), *var),
            };
            let grouping_error = if named_only {
                let bad = default_named && !default;
                seen_named = true;
                default_named |= default;
                bad
            } else {
                let bad = seen_named || (default_plain && !default);
                default_plain |= default;
                bad
            };
            if grouping_error {
                self.error(
                    module,
                    Diagnostic::error("E0041", name.span, catalog::parameter_grouping()),
                );
            }
            if params.iter().any(|p: &ParamDef| p.name == name.text) {
                self.error(
                    module,
                    Diagnostic::error("E0055", name.span, catalog::local_repeats(&name.text)),
                );
            }
            let ty = self.resolve_type(module, ty, &scope, true);
            params.push(ParamDef {
                name: name.text.clone(),
                ty,
                var,
                named_only,
                has_default: default,
                span: name.span,
            });
        }
        let ret = sig
            .returns
            .as_ref()
            .map(|t| self.resolve_type(module, t, &scope, true))
            .unwrap_or(Ty::Unit);
        let fails = sig
            .fails
            .iter()
            .map(|t| self.resolve_type(module, t, &scope, true))
            .collect();
        let f = &mut self.funcs[id];
        f.params = params;
        f.ret = ret;
        f.fails = fails;
    }

    fn resolve_signatures(&mut self) {
        for module in 0..self.modules.len() {
            let items = self.modules[module].analysis.file.items.clone();
            let mut next_impl = self.impls.iter().position(|i| i.module == module);
            for item in &items {
                match item {
                    ast::Item::Declaration(d) => match &d.kind {
                        ast::DeclarationKind::Function(f) => {
                            if let Some(Symbol::Func(id)) = self.modules[module]
                                .symbols
                                .get(&f.signature.name.text)
                                .copied()
                            {
                                if self.funcs[id].span == f.signature.name.span {
                                    self.resolve_signature(id, &f.signature, &HashMap::new());
                                }
                            }
                        }
                        ast::DeclarationKind::External(sig) => {
                            if let Some(Symbol::Func(id)) =
                                self.modules[module].symbols.get(&sig.name.text).copied()
                            {
                                if self.funcs[id].span == sig.name.span {
                                    self.resolve_signature(id, sig, &HashMap::new());
                                }
                            }
                        }
                        ast::DeclarationKind::Constant(c) => {
                            if let Some(Symbol::Const(id)) =
                                self.modules[module].symbols.get(&c.name.text).copied()
                            {
                                if let Some(t) = &c.ty {
                                    let ty = self.resolve_type(module, t, &HashMap::new(), true);
                                    self.consts[id].annotated = Some(ty);
                                }
                            }
                        }
                        ast::DeclarationKind::Struct(s) => {
                            let Some(def) = self.def_named(module, &s.name) else {
                                continue;
                            };
                            let generics = self.defs[def].generics.clone();
                            let scope =
                                self.generic_scope(module, &generics, &s.generics, &HashMap::new());
                            let fields = self.resolve_fields(module, &s.fields, &scope, false);
                            self.defs[def].kind = DefKind::Struct(fields);
                        }
                        ast::DeclarationKind::Error(e) => {
                            let Some(def) = self.def_named(module, &e.name) else {
                                continue;
                            };
                            let fields =
                                self.resolve_fields(module, &e.fields, &HashMap::new(), true);
                            self.defs[def].kind = DefKind::Error(fields);
                        }
                        ast::DeclarationKind::Enum(e) => {
                            let Some(def) = self.def_named(module, &e.name) else {
                                continue;
                            };
                            let generics = self.defs[def].generics.clone();
                            let scope =
                                self.generic_scope(module, &generics, &e.generics, &HashMap::new());
                            let mut variants: Vec<VariantDef> = Vec::new();
                            for v in &e.variants {
                                if variants.iter().any(|x| x.name == v.name.text) {
                                    self.error(
                                        module,
                                        Diagnostic::error(
                                            "E0051",
                                            v.name.span,
                                            catalog::duplicate(&v.name.text),
                                        ),
                                    );
                                }
                                let fields = v
                                    .fields
                                    .iter()
                                    .map(|(n, t)| {
                                        (n.text.clone(), self.resolve_type(module, t, &scope, true))
                                    })
                                    .collect();
                                variants.push(VariantDef {
                                    name: v.name.text.clone(),
                                    fields,
                                });
                            }
                            self.defs[def].kind = DefKind::Enum(variants);
                        }
                        ast::DeclarationKind::Interface(i) => {
                            let Some(def) = self.def_named(module, &i.name) else {
                                continue;
                            };
                            let generics = self.defs[def].generics.clone();
                            let scope =
                                self.generic_scope(module, &generics, &i.generics, &HashMap::new());
                            let DefKind::Interface(methods) = &self.defs[def].kind else {
                                continue;
                            };
                            let methods = methods.clone();
                            for (id, (sig, _)) in methods.iter().zip(&i.members) {
                                self.resolve_signature(*id, sig, &scope);
                            }
                        }
                    },
                    ast::Item::Extend(e) => {
                        let Some(impl_id) = next_impl else { continue };
                        next_impl = self
                            .impls
                            .iter()
                            .enumerate()
                            .skip(impl_id + 1)
                            .find(|(_, i)| i.module == module)
                            .map(|(n, _)| n);
                        let generics = self.impls[impl_id].generics.clone();
                        let scope =
                            self.generic_scope(module, &generics, &e.generics, &HashMap::new());
                        let target = self.resolve_type(module, &e.target, &scope, false);
                        let interface = e
                            .interface
                            .as_ref()
                            .map(|t| self.resolve_type(module, t, &scope, false));
                        self.impls[impl_id].target = target;
                        self.impls[impl_id].interface = interface;
                        let methods = self.impls[impl_id].methods.clone();
                        for (id, member) in methods.iter().zip(&e.members) {
                            self.resolve_signature(*id, &member.signature, &scope);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn def_named(&self, module: ModId, name: &ast::Name) -> Option<DefId> {
        match self.modules[module].symbols.get(&name.text) {
            Some(Symbol::Def(d)) if self.defs[*d].span == name.span => Some(*d),
            _ => None,
        }
    }

    // ------------------------------------------------------------ implementations

    /// A key that identifies the type constructor an extend block applies to.
    pub fn type_key(ty: &Ty) -> Option<String> {
        Some(match ty {
            Ty::Prim(p) => p.name().to_string(),
            Ty::List(_) => "List".into(),
            Ty::Map(..) => "Map".into(),
            Ty::Set(_) => "Set".into(),
            Ty::Tuple(_) => "Tuple".into(),
            Ty::Named(d, _) => format!("#{d}"),
            _ => return None,
        })
    }

    fn owns(&self, module: ModId, ty: &Ty) -> bool {
        match ty {
            Ty::Named(d, _) => {
                self.defs[*d].module == module
                    || (self.modules[module].stdlib && self.modules[self.defs[*d].module].stdlib)
            }
            _ => self.modules[module].stdlib,
        }
    }

    fn check_impls(&mut self) {
        // Inherent methods: one name per type (E0051).
        let mut seen: HashMap<(String, String), FnId> = HashMap::new();
        let mut implemented: HashMap<(String, DefId), ImplId> = HashMap::new();
        for impl_id in 0..self.impls.len() {
            let target = self.impls[impl_id].target.clone();
            let Some(key) = Program::type_key(&target) else {
                continue;
            };
            let module = self.impls[impl_id].module;
            match self.impls[impl_id].interface.clone() {
                None => {
                    for f in self.impls[impl_id].methods.clone() {
                        let name = self.funcs[f].name.clone();
                        if seen.insert((key.clone(), name.clone()), f).is_some() {
                            let span = self.funcs[f].span;
                            self.error(
                                module,
                                Diagnostic::error("E0051", span, catalog::duplicate(&name)),
                            );
                        }
                    }
                }
                Some(Ty::Named(iface, args)) => {
                    let span = self.impls[impl_id].span;
                    let iface_name = self.defs[iface].name.clone();
                    let type_name = self.show(&target);
                    if implemented.insert((key.clone(), iface), impl_id).is_some() {
                        self.error(
                            module,
                            Diagnostic::error(
                                "E0044",
                                span,
                                catalog::second_implementation(&iface_name, &type_name),
                            ),
                        );
                        continue;
                    }
                    if !self.owns(module, &target)
                        && !self.owns(module, &Ty::Named(iface, Vec::new()))
                    {
                        self.error(
                            module,
                            Diagnostic::error(
                                "E0044",
                                span,
                                catalog::foreign_implementation(&iface_name, &type_name),
                            ),
                        );
                    }
                    self.check_implementation(impl_id, iface, &args);
                }
                _ => {}
            }
        }
    }

    fn check_implementation(&mut self, impl_id: ImplId, iface: DefId, args: &[Ty]) {
        let DefKind::Interface(required) = &self.defs[iface].kind else {
            return;
        };
        let required = required.clone();
        let module = self.impls[impl_id].module;
        let span = self.impls[impl_id].span;
        let target = self.impls[impl_id].target.clone();
        let substitution: HashMap<ParamId, Ty> = self.defs[iface]
            .generics
            .iter()
            .copied()
            .zip(args.iter().cloned())
            .collect();
        let methods = self.impls[impl_id].methods.clone();
        for r in required {
            let name = self.funcs[r].name.clone();
            let provided = methods
                .iter()
                .copied()
                .find(|m| self.funcs[*m].name == name);
            match provided {
                None if self.funcs[r].body.is_none() => {
                    let iface_name = self.defs[iface].name.clone();
                    self.error(
                        module,
                        Diagnostic::error(
                            "E0069",
                            span,
                            catalog::missing_method(&iface_name, &name),
                        ),
                    );
                }
                None => {}
                Some(m) => {
                    let expected: Vec<Ty> = self.funcs[r]
                        .params
                        .iter()
                        .map(|p| p.ty.subst(&substitution, Some(&target)))
                        .collect();
                    let expected_ret = self.funcs[r].ret.subst(&substitution, Some(&target));
                    let given: Vec<Ty> = self.funcs[m]
                        .params
                        .iter()
                        .map(|p| p.ty.subst(&HashMap::new(), Some(&target)))
                        .collect();
                    let given_ret = self.funcs[m].ret.subst(&HashMap::new(), Some(&target));
                    let same = expected.len() == given.len()
                        && expected
                            .iter()
                            .zip(&given)
                            .all(|(a, b)| loosely_equal(a, b))
                        && loosely_equal(&expected_ret, &given_ret)
                        && self.funcs[m].self_param.is_some() == self.funcs[r].self_param.is_some();
                    if !same {
                        let span = self.funcs[m].span;
                        self.error(
                            module,
                            Diagnostic::error("E0069", span, catalog::signature_differs(&name)),
                        );
                    }
                }
            }
        }
        let iface_name = self.defs[iface].name.clone();
        let _ = iface_name;
    }

    /// A type as a reader writes it.
    pub fn show(&self, ty: &Ty) -> String {
        let list = |ts: &[Ty]| {
            ts.iter()
                .map(|t| self.show(t))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match ty {
            Ty::Unknown => "?".into(),
            Ty::Never => "Never".into(),
            Ty::Unit => "nothing".into(),
            Ty::Prim(p) => p.name().into(),
            Ty::List(t) => format!("List[{}]", self.show(t)),
            Ty::Set(t) => format!("Set[{}]", self.show(t)),
            Ty::Map(k, v) => format!("Map[{}, {}]", self.show(k), self.show(v)),
            Ty::Tuple(ts) => format!("Tuple[{}]", list(ts)),
            Ty::Func(f) => {
                let params = f
                    .params
                    .iter()
                    .map(|(_, t)| self.show(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                let fails = if f.fails.is_empty() {
                    String::new()
                } else {
                    format!(" fails with {}", list(&f.fails))
                };
                if f.ret == Ty::Unit {
                    format!("Function[({params}){fails}]")
                } else {
                    format!("Function[({params}) -> {}{fails}]", self.show(&f.ret))
                }
            }
            Ty::Named(d, args) if args.is_empty() => self.defs[*d].name.clone(),
            Ty::Named(d, args) => format!("{}[{}]", self.defs[*d].name, list(args)),
            Ty::Param(p) => self.params[*p].name.clone(),
            Ty::Var(_) => "?".into(),
            Ty::SelfTy => "Self".into(),
        }
    }
}

/// Equality that lets Unknown match anything.
pub fn loosely_equal(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Unknown, _) | (_, Ty::Unknown) => true,
        (Ty::List(x), Ty::List(y)) | (Ty::Set(x), Ty::Set(y)) => loosely_equal(x, y),
        (Ty::Map(a1, b1), Ty::Map(a2, b2)) => loosely_equal(a1, a2) && loosely_equal(b1, b2),
        (Ty::Tuple(x), Ty::Tuple(y)) | (Ty::Named(_, x), Ty::Named(_, y)) if x.len() == y.len() => {
            let same_head = match (a, b) {
                (Ty::Named(d1, _), Ty::Named(d2, _)) => d1 == d2,
                _ => true,
            };
            same_head && x.iter().zip(y).all(|(p, q)| loosely_equal(p, q))
        }
        (Ty::Func(f), Ty::Func(g)) => {
            f.params.len() == g.params.len()
                && f.params
                    .iter()
                    .zip(&g.params)
                    .all(|((_, p), (_, q))| loosely_equal(p, q))
                && loosely_equal(&f.ret, &g.ret)
        }
        _ => a == b,
    }
}

/// The candidate closest to a misspelled name, if it is close enough to be a likely fix.
pub fn closest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (name.chars().count() / 3).clamp(1, 3);
    candidates
        .filter(|c| *c != name)
        .map(|c| (distance(name, c), c))
        .filter(|(d, _)| *d <= limit)
        .min()
        .map(|(_, c)| c)
}

fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            let cost = usize::from(x != y);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[b.len()]
}
