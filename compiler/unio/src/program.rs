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
    /// Whether the module is one of the files the command was given, not only imported: the
    /// tests `unio test` runs are those of the given modules.
    pub given: bool,
    pub symbols: HashMap<String, Symbol>,
    pub imports: HashMap<String, Binding>,
    /// Names that an import failed to bind (E0048, E0053, E0054): known but poisoned, so that
    /// their uses give no further message (semantics 2.8).
    pub failed_imports: HashSet<String>,
    /// Imported modules with the span of their import, for cycles.
    pub edges: Vec<(ModId, Span)>,
    /// For a module of a locked dependency (T-14): the package it belongs to, which is part
    /// of the identity of the module and of its types; `None` for the packages of the given
    /// files and the standard library.
    pub dependency: Option<DependencyOf>,
}

/// The package a module of a dependency belongs to (T-14).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DependencyOf {
    /// The identity of the package (`registry+...`, `path+...`, `archive+...`).
    pub id: String,
    /// The root package whose `unio.lock` locked it.
    pub root: PathBuf,
    /// `name@version`, for traces and messages.
    pub label: String,
}

/// Why an import found no module.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Missing {
    NotFound,
    /// The file lies outside the root it was searched in (E0053).
    Outside,
    /// `deps.<alias>...` could not be resolved (E0048).
    Dependency(catalog::DependencyProblem),
}

/// Where an import leads: the file and the package it belongs to.
struct Located {
    file: PathBuf,
    dependency: Option<DependencyOf>,
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

/// A call of a generic function or method with its type arguments as the check inferred
/// them, for the conversion check of `std.json` (E0046). `caller` is None in a test or a
/// constant; a type argument may name the caller's own generic parameters.
pub struct GenericCall {
    pub caller: Option<FnId>,
    pub types: HashMap<ParamId, Ty>,
    pub module: ModId,
    pub span: Span,
}

/// The tests that run with the program, and so count in the effects analysis (RFC 0005, tests
/// in the analysis): none for `unio check` and `unio run`; for `unio test`, the tests of the
/// given modules whose description contains the filter. The tests of imported modules, of the
/// same package or another, and of the standard library run only when their modules are given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TestScope {
    #[default]
    None,
    Given {
        filter: Option<String>,
    },
}

impl TestScope {
    /// Whether a test of a given module with this description runs.
    pub fn selects(&self, description: &str) -> bool {
        match self {
            TestScope::None => false,
            TestScope::Given { filter } => filter
                .as_ref()
                .is_none_or(|f| description.contains(f.as_str())),
        }
    }
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
    /// Calls of generic functions with their type arguments (E0046).
    pub generic_calls: Vec<GenericCall>,
    /// The type parameters that reach a JSON conversion of `std.json` (semantics 3.8): a call
    /// that infers them passes the inferred types at run time, as written ones are.
    pub conversion_params: std::collections::HashSet<ParamId>,
    /// Places where a function is named, for `unio symbol`.
    pub usages: Vec<(FnId, ModId, Span)>,
    /// What the effects need beyond `calls`: implicit calls through interfaces, uses of
    /// constants and field defaults, and allocating expressions (semantics 8.4).
    pub effect_facts: crate::effects::Facts,
    /// The tests that run, whose values count in the effects analysis (RFC 0005).
    pub tests: TestScope,
    /// The `or` expressions on an Option whose right side is an Option too, by module and
    /// place: they give an Option, the others the inner value (semantics 4.3).
    pub option_ors: HashSet<(ModId, (usize, usize))>,
    /// Modules by their package and canonical path: the same file of two locked packages is
    /// two modules with types of their own (T-14).
    by_path: HashMap<(Option<String>, PathBuf), ModId>,
    stdlib_root: Option<PathBuf>,
    /// The lock of each root package, as the loader reads it (T-14).
    locks: HashMap<PathBuf, Result<Option<crate::package::Resolved>, String>>,
    /// The manifests of the reported packages that have mistakes (E0088, T-6), each as the
    /// analysis of a file of its own.
    pub manifests: Vec<Analysis>,
}

/// The directory of the trusted standard library, chosen by its origin only (RFC 0004): the
/// files a worker was given by its tool, `UNIO_STDLIB` set by the operator, the `stdlib/` of the
/// checkout whose build directory this compiler runs from, or else the library embedded in the
/// compiler ([`crate::vfs::BUILTIN_ROOT`]), which an installed `unio` uses. A `stdlib`
/// directory in or above a package is never used, so a package cannot replace library modules,
/// their effects, or their layers.
pub fn trusted_stdlib() -> Option<PathBuf> {
    if let Some(embedded) = crate::vfs::embedded_stdlib() {
        return embedded;
    }
    if let Ok(dir) = std::env::var("UNIO_STDLIB") {
        let dir = PathBuf::from(dir);
        if dir.join("core/prelude.unio").is_file() {
            // Canonical, like the paths of the modules it is compared with: on macOS the
            // temporary directory /var is a link to /private/var.
            return Some(unio_runtime::sandbox::canonical(&dir).unwrap_or(dir));
        }
        return None;
    }
    checkout_stdlib().or_else(crate::vfs::builtin_stdlib)
}

/// The library of the checkout this compiler was built in, while it runs from there: the
/// executable lies below `compiler/target` of a checkout that has `compiler/unio/build.rs` and
/// `stdlib/`, as `cargo build` and `cargo test` leave it. The origin is the place of the
/// compiler, never the project, and the compiler records no path of its build (C-4): a copy
/// installed anywhere else uses the library embedded in it. Inside the checkout the library is
/// read from disk, so that its modules are tested with `unio test stdlib/...` and edited
/// without a rebuild.
fn checkout_stdlib() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = unio_runtime::sandbox::canonical(&exe).unwrap_or(exe);
    let target = exe
        .ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|n| n == "target"))?;
    let workspace = target.parent()?;
    let root = workspace.parent()?;
    let library = root.join("stdlib");
    let is_checkout = workspace.file_name().is_some_and(|n| n == "compiler")
        && workspace.join("unio/build.rs").is_file()
        && library.join("core/prelude.unio").is_file();
    if is_checkout {
        return unio_runtime::sandbox::canonical(&library).ok();
    }
    None
}

fn find_stdlib(_start: &Path) -> Option<PathBuf> {
    trusted_stdlib()
}

const LAYERS: [&str; 3] = ["core", "alloc", "std"];

/// Whether the mistakes of the package's `unio.toml` (E0088) already cover the dependency
/// with the alias: a manifest that does not parse, or a mistake in that entry. Its imports
/// then give no E0048 of their own (semantics 2.8).
fn manifest_covers_dependency(package: &Path, alias: &str) -> bool {
    use crate::package::manifest::Mistake;
    let Ok(text) = crate::vfs::read_to_string(&package.join("unio.toml")) else {
        return false;
    };
    crate::package::manifest::check(&text)
        .iter()
        .any(|(_, mistake)| match mistake {
            Mistake::Syntax(_) => true,
            Mistake::Dependency(a, _) => a == alias,
            _ => false,
        })
}

/// The span of the text of a line, from 1, of a manifest.
fn line_span(source: &str, line: usize) -> Span {
    let begin = match line {
        0 | 1 => 0,
        _ => source
            .match_indices('\n')
            .nth(line - 2)
            .map_or(source.len(), |(i, _)| i + 1),
    };
    let end = source[begin..]
        .find('\n')
        .map_or(source.len(), |i| begin + i);
    let text = source[begin..end].trim_end_matches('\r');
    let indent = text.len() - text.trim_start().len();
    let pos = |offset: usize| crate::diag::Pos {
        offset: begin + offset,
        line,
        column: text[..offset].chars().count() + 1,
    };
    Span::new(pos(indent), pos(text.len().max(indent)))
}

/// The E0088 diagnostics of the text of a manifest (T-6), each over the text of its line.
pub fn manifest_diagnostics(source: &str) -> Vec<Diagnostic> {
    crate::package::manifest::check(source)
        .into_iter()
        .map(|(line, mistake)| {
            let span = line_span(source, line);
            Diagnostic::error("E0088", span, catalog::manifest_mistake(&mistake))
        })
        .collect()
}

/// The diagnostics of the manifest of the package in `package`, whose text is `source`: its
/// mistakes (E0088), and the modules of the package that carry `unsafe module` compared with
/// `package.unsafe_modules`, which names exactly them (E0089, T-6, RFC 0004), in the order of
/// the lines.
pub fn package_manifest_diagnostics(package: &Path, source: &str) -> Vec<Diagnostic> {
    let mut found = manifest_diagnostics(source);
    if let Some((listed, line)) = crate::package::manifest::unsafe_modules(source) {
        let marked = marked_modules(package);
        let span = line_span(source, line);
        for module in marked.iter().filter(|m| !listed.contains(m)) {
            let message = catalog::unsafe_module_not_listed(module);
            found.push(Diagnostic::error("E0089", span, message));
        }
        let mut seen = Vec::new();
        for module in listed.iter().filter(|m| !marked.contains(m)) {
            if !seen.contains(&module) {
                seen.push(module);
                let message = catalog::unsafe_module_not_marked(module);
                found.push(Diagnostic::error("E0089", span, message));
            }
        }
    }
    found.sort_by_key(|d| d.span.start.line);
    found
}

/// The modules of the package in `package` that carry the `unsafe module` marker, found by
/// parsing every module of its tree, in the order of their names. As when a package is copied
/// (`package::tree`), names that start with a dot and `target/` at the top are left out, and a
/// link is not followed; a directory with a `unio.toml` of its own is another package.
pub fn marked_modules(package: &Path) -> Vec<String> {
    let mut marked = Vec::new();
    let mut stack: Vec<(PathBuf, Vec<String>)> = vec![(package.to_path_buf(), Vec::new())];
    while let Some((dir, parts)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') || (parts.is_empty() && name == "target") {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let mut child = parts.clone();
            if kind.is_dir() {
                if !path.join("unio.toml").exists() {
                    child.push(name);
                    stack.push((path, child));
                }
            } else if kind.is_file() {
                let Some(stem) = name.strip_suffix(".unio") else {
                    continue;
                };
                let Ok(source) = crate::vfs::read_to_string(&path) else {
                    continue;
                };
                let lexed = crate::lexer::lex(&source);
                if crate::parser::Parser::new(&lexed.tokens)
                    .file()
                    .unsafe_module
                {
                    child.push(stem.to_string());
                    marked.push(child.join("."));
                }
            }
        }
    }
    marked.sort();
    marked
}

/// Whether `[dependencies]` of the package's `unio.toml` lists the alias (T-6, T-14).
fn declares_dependency(package: &Path, alias: &str) -> bool {
    let Ok(text) = crate::vfs::read_to_string(&package.join("unio.toml")) else {
        return false;
    };
    let mut section = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_string();
            continue;
        }
        if section == "dependencies" {
            let key = line
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"');
            if !line.is_empty() && key == alias {
                return true;
            }
        }
    }
    false
}

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
                        .filter(|p| crate::vfs::is_file(p)),
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

/// What an executable made by `unio build` keeps of a module (docs/runtime.md 8.4).
pub struct PreparedModule {
    pub name: String,
    /// The path of traces ([`Program::trace_path`]).
    pub path: String,
    pub stdlib: bool,
    pub prelude: bool,
    pub report: bool,
}

impl Program {
    /// A program read from the image of an executable: its declarations without syntax trees,
    /// sources, or imports, which the run does not need once every body is lowered.
    pub fn prepared(
        modules: Vec<PreparedModule>,
        defs: Vec<Def>,
        funcs: Vec<FnDef>,
        consts: Vec<ConstDef>,
        impls: Vec<Impl>,
        params: Vec<ParamInfo>,
    ) -> Program {
        let modules = modules
            .into_iter()
            .map(|m| Module {
                name: m.name,
                analysis: Analysis {
                    path: m.path.clone(),
                    source: String::new(),
                    diagnostics: Vec::new(),
                    comments: Vec::new(),
                    file: ast::File {
                        unsafe_module: false,
                        items: Vec::new(),
                    },
                },
                path: PathBuf::from(m.path),
                stdlib: m.stdlib,
                prelude: m.prelude,
                package: PathBuf::new(),
                report: m.report,
                given: m.report,
                symbols: HashMap::new(),
                imports: HashMap::new(),
                failed_imports: HashSet::new(),
                edges: Vec::new(),
                dependency: None,
            })
            .collect();
        Program {
            modules,
            defs,
            funcs,
            consts,
            impls,
            params,
            prelude: HashMap::new(),
            diagnostics: Vec::new(),
            calls: Vec::new(),
            generic_calls: Vec::new(),
            conversion_params: Default::default(),
            usages: Vec::new(),
            effect_facts: Default::default(),
            tests: TestScope::None,
            option_ors: HashSet::new(),
            by_path: HashMap::new(),
            stdlib_root: None,
            locks: HashMap::new(),
            manifests: Vec::new(),
        }
    }

    /// The file of a module as traces show it (C-14): relative to the root of its package
    /// (the directory of its `unio.toml`, or of the file without one), and `std:` with the
    /// path below the standard library for its modules, so that a trace names the same files
    /// wherever the program was built or runs.
    pub fn trace_path(&self, module: ModId) -> String {
        let m = &self.modules[module];
        let label;
        let (prefix, root) = if m.stdlib {
            ("std:", self.stdlib_root.as_deref())
        } else if let Some(dependency) = &m.dependency {
            label = format!("{}:", dependency.label);
            (label.as_str(), Some(m.package.as_path()))
        } else {
            ("", Some(m.package.as_path()))
        };
        match root.and_then(|r| m.path.strip_prefix(r).ok()) {
            Some(relative) => {
                let parts: Vec<String> = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                format!("{prefix}{}", parts.join("/"))
            }
            None => m.path.to_string_lossy().into_owned(),
        }
    }

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
            generic_calls: Vec::new(),
            conversion_params: Default::default(),
            usages: Vec::new(),
            effect_facts: Default::default(),
            tests: TestScope::None,
            option_ors: HashSet::new(),
            by_path: HashMap::new(),
            stdlib_root,
            locks: HashMap::new(),
            manifests: Vec::new(),
        };
        if let Some(root) = program.stdlib_root.clone() {
            for layer in LAYERS {
                let dir = root.join(layer);
                let mut preludes = vec![dir.join("prelude.unio")];
                preludes.extend(crate::vfs::unio_files_in(&dir.join("builtin")));
                for path in preludes {
                    if crate::vfs::is_file(&path) {
                        program.load_file(&path, false, None);
                    }
                }
            }
        }
        for file in files {
            let id = program.load_file(file, true, None);
            if let Some(id) = id {
                program.modules[id].report = true;
                program.modules[id].given = true;
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

    fn load_file(
        &mut self,
        file: &Path,
        report: bool,
        dependency: Option<DependencyOf>,
    ) -> Option<ModId> {
        let canonical = crate::vfs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        let key = (dependency.as_ref().map(|d| d.id.clone()), canonical.clone());
        if let Some(id) = self.by_path.get(&key) {
            return Some(*id);
        }
        let source = crate::vfs::read_to_string(&canonical).ok()?;
        let analysis = crate::analyse(&file.to_string_lossy(), &source);
        let stdlib_name = match dependency {
            Some(_) => None,
            None => self.stdlib_name(&canonical),
        };
        let (name, prelude, stdlib, package) = match stdlib_name {
            Some((name, prelude)) => (name, prelude, true, self.stdlib_root.clone().unwrap()),
            None => {
                let package = package_root(&canonical);
                let relative = canonical.strip_prefix(&package).unwrap_or(&canonical);
                let mut name = relative
                    .iter()
                    .map(|p| p.to_string_lossy().trim_end_matches(".unio").to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                if let Some(dependency) = &dependency {
                    name = format!("{}:{name}", dependency.label);
                } else if !self.locks.contains_key(&package) {
                    // The lock of a root package, read once (T-14).
                    let lock = crate::package::resolved(&package);
                    self.locks.insert(package.clone(), lock);
                }
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
            given: false,
            symbols: HashMap::new(),
            imports: HashMap::new(),
            failed_imports: HashSet::new(),
            edges: Vec::new(),
            dependency,
        });
        self.by_path.insert(key, id);
        Some(id)
    }

    /// The file of a module path written in an import, if it exists and stays inside the root
    /// it was found in.
    fn module_file(&self, from: ModId, path: &[String]) -> Option<Located> {
        self.locate_module(from, path).ok()
    }

    /// The module of a locked dependency: `deps.<alias>.<module>` through the aliases of the
    /// importing package in the lock of its root package (T-14).
    fn locate_dependency(&self, from: ModId, path: &[String]) -> Result<Located, Missing> {
        use catalog::DependencyProblem as P;
        let module = &self.modules[from];
        let alias = &path[1];
        let (root, id) = match &module.dependency {
            Some(d) => (d.root.clone(), Some(d.id.as_str())),
            None => (module.package.clone(), None),
        };
        let declared = || declares_dependency(&module.package, alias);
        let lock = match self.locks.get(&root) {
            Some(Ok(Some(lock))) => lock,
            Some(Err(e)) => return Err(Missing::Dependency(P::BadLock(e.clone()))),
            _ if declared() => return Err(Missing::Dependency(P::NotLocked)),
            _ => return Err(Missing::Dependency(P::NotDeclared)),
        };
        let table = match id {
            Some(id) => lock.packages.get(id),
            None => Some(&lock.root),
        };
        let Some(target) = table.and_then(|t| t.get(alias)) else {
            return Err(Missing::Dependency(if declared() {
                P::NotLocked
            } else {
                P::NotDeclared
            }));
        };
        let label = lock
            .labels
            .get(&target.id)
            .cloned()
            .unwrap_or_else(|| target.id.clone());
        if path.len() < 3 {
            return Err(Missing::Dependency(P::NoModule(label, String::new())));
        }
        if !crate::vfs::is_file(&target.entry.join("unio.toml")) {
            return Err(Missing::Dependency(P::NotInstalled(label)));
        }
        let rest = path[2..].join(".");
        let file = target.entry.join(path[2..].join("/") + ".unio");
        if !crate::vfs::is_file(&file) {
            return Err(Missing::Dependency(P::NoModule(label, rest)));
        }
        let entry =
            crate::vfs::canonicalize(&target.entry).unwrap_or_else(|_| target.entry.clone());
        match crate::vfs::canonicalize(&file) {
            Ok(real) if real.starts_with(&entry) => Ok(Located {
                file,
                dependency: Some(DependencyOf {
                    id: target.id.clone(),
                    root,
                    label,
                }),
            }),
            _ => Err(Missing::Outside),
        }
    }

    /// Finds the file of a module path: below the standard library for `std.*`, in the cache
    /// entry of a locked dependency for `deps.*`, below the package otherwise. A file whose
    /// resolved path (links followed) leaves that root is [`Missing::Outside`]: an import
    /// cannot lead out of the package or the standard library through a link.
    fn locate_module(&self, from: ModId, path: &[String]) -> Result<Located, Missing> {
        // `deps` is reserved as the root of manifest dependencies (T-14), never a directory of
        // the package.
        if path.len() > 1 && path[0] == "deps" {
            return self.locate_dependency(from, path);
        }
        let mut dependency = None;
        let (root, file) = if path.first().is_some_and(|p| p == "std") && path.len() > 1 {
            let root = self.stdlib_root.as_ref().ok_or(Missing::NotFound)?;
            let rest = path[1..].join("/") + ".unio";
            let file = LAYERS
                .iter()
                .map(|layer| root.join(layer).join(&rest))
                .find(|p| crate::vfs::is_file(p))
                .ok_or(Missing::NotFound)?;
            (root, file)
        } else {
            let module = &self.modules[from];
            // A module of a dependency imports the modules of its own package.
            dependency = module.dependency.clone();
            let root = &module.package;
            let file = root.join(path.join("/") + ".unio");
            if !crate::vfs::is_file(&file) {
                return Err(Missing::NotFound);
            }
            (root, file)
        };
        let root = crate::vfs::canonicalize(root).unwrap_or_else(|_| root.clone());
        match crate::vfs::canonicalize(&file) {
            Ok(real) if real.starts_with(&root) => Ok(Located { file, dependency }),
            _ => Err(Missing::Outside),
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
            if let Some(located) = self.module_file(id, &path) {
                let before = self.modules.len();
                if let Some(loaded) = self.load_file(&located.file, false, located.dependency) {
                    if loaded >= before && report && !self.modules[loaded].stdlib {
                        self.modules[loaded].report = true;
                    }
                }
            }
        }
    }

    /// Checks the manifests of the packages of the reported modules (E0088, E0089, T-6): the packages
    /// of the given files and of the modules they import from them, not the standard library
    /// or a dependency, whose manifest was checked when it was installed.
    pub fn check_manifests(&mut self) {
        let mut seen: Vec<PathBuf> = Vec::new();
        for m in &self.modules {
            if !m.report || m.stdlib || m.dependency.is_some() || seen.contains(&m.package) {
                continue;
            }
            seen.push(m.package.clone());
            let path = m.package.join("unio.toml");
            let Ok(source) = crate::vfs::read_to_string(&path) else {
                continue;
            };
            let diagnostics = package_manifest_diagnostics(&m.package, &source);
            if diagnostics.is_empty() {
                continue;
            }
            // Named as the file of the module was given: `unio.toml` beside `main.unio`.
            let shown = m
                .path
                .parent()
                .and_then(|dir| dir.strip_prefix(&m.package).ok())
                .and_then(|below| {
                    let given = Path::new(&m.analysis.path).parent()?;
                    given.ancestors().nth(below.components().count())
                })
                .map_or(path.clone(), |dir| dir.join("unio.toml"));
            self.manifests.push(Analysis {
                path: shown.to_string_lossy().into_owned(),
                source,
                diagnostics,
                comments: Vec::new(),
                file: ast::File {
                    unsafe_module: false,
                    items: Vec::new(),
                },
            });
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
                let target = located.as_ref().ok().and_then(|l| {
                    let file = crate::vfs::canonicalize(&l.file).ok()?;
                    let key = (l.dependency.as_ref().map(|d| d.id.clone()), file);
                    self.by_path.get(&key).copied()
                });
                let Some(target) = target else {
                    // The names the import would bind are poisoned (semantics 2.8).
                    let bound: Vec<String> = if import.names.is_empty() {
                        let name = import
                            .alias
                            .as_ref()
                            .unwrap_or(import.module.last().unwrap());
                        vec![name.text.clone()]
                    } else {
                        import.names.iter().map(|n| n.text.clone()).collect()
                    };
                    self.modules[module].failed_imports.extend(bound);
                    let span = import.module.first().unwrap().span.to(import
                        .module
                        .last()
                        .unwrap()
                        .span);
                    let problem = match located {
                        Err(Missing::Dependency(problem)) => problem,
                        // `deps.<alias>.<module>` names a locked dependency (T-14).
                        Ok(_) if path.len() > 1 && path[0] == "deps" => {
                            catalog::DependencyProblem::NoModule(path[1].clone(), String::new())
                        }
                        Err(Missing::Outside) => {
                            let message = catalog::module_outside(&dotted);
                            self.error(module, Diagnostic::error("E0053", span, message));
                            continue;
                        }
                        _ => {
                            let message = catalog::module_not_found(&dotted);
                            self.error(module, Diagnostic::error("E0053", span, message));
                            continue;
                        }
                    };
                    if path.len() > 1
                        && path[0] == "deps"
                        && manifest_covers_dependency(&self.modules[module].package, &path[1])
                    {
                        continue;
                    }
                    if path.len() > 1 && path[0] == "deps" {
                        let message = catalog::dependency_missing(&path[1], &problem);
                        self.error(module, Diagnostic::error("E0048", span, message));
                        continue;
                    }
                    let message = catalog::module_not_found(&dotted);
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
                            _ => {
                                self.modules[module]
                                    .failed_imports
                                    .insert(name.text.clone());
                                self.error(
                                    module,
                                    Diagnostic::error(
                                        "E0054",
                                        name.span,
                                        catalog::name_not_in_module(&name.text, &dotted),
                                    ),
                                )
                            }
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
                // A type with the wrong number of arguments is unknown, so that it gives no
                // further mismatch (semantics 2.8).
                "List" => {
                    if !arity(self, 1) {
                        return Ty::Unknown;
                    }
                    return Ty::List(Box::new(one(&arguments, 0)));
                }
                "Set" => {
                    if !arity(self, 1) {
                        return Ty::Unknown;
                    }
                    let element = self.hashable_key(module, ty, &arguments, true);
                    return Ty::Set(Box::new(element));
                }
                "Map" => {
                    if !arity(self, 2) {
                        return Ty::Unknown;
                    }
                    let key = self.hashable_key(module, ty, &arguments, false);
                    return Ty::Map(Box::new(key), Box::new(one(&arguments, 1)));
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
            _ if self.modules[module].failed_imports.contains(names[0]) => Ty::Unknown,
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

    /// The key of a written `Map[K, V]` or the element of a `Set[T]`: a type made of `Float`
    /// has no `Hash` (L-9) and is E0045 at the argument; the key is then unknown, so that the
    /// mistake is reported once.
    fn hashable_key(&mut self, module: ModId, ty: &ast::Type, arguments: &[Ty], set: bool) -> Ty {
        let key = arguments.first().cloned().unwrap_or(Ty::Unknown);
        if !key.float_inside() {
            return key;
        }
        let span = match ty.arguments.first() {
            Some(ast::TypeArgument::Type(t)) => t.span,
            _ => ty.span,
        };
        self.error(
            module,
            Diagnostic::error("E0045", span, catalog::float_key(set)),
        );
        Ty::Unknown
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

    /// The function of a module-level declaration, a duplicate included.
    fn module_function(&self, module: ModId, sig: &ast::Signature) -> Option<FnId> {
        if let Some(Symbol::Func(id)) = self.modules[module].symbols.get(&sig.name.text) {
            if self.funcs[*id].span == sig.name.span {
                return Some(*id);
            }
        }
        self.funcs.iter().position(|f| {
            f.module == module && f.span == sig.name.span && matches!(f.owner, Owner::Module)
        })
    }

    fn resolve_signatures(&mut self) {
        for module in 0..self.modules.len() {
            let items = self.modules[module].analysis.file.items.clone();
            let mut next_impl = self.impls.iter().position(|i| i.module == module);
            for item in &items {
                match item {
                    ast::Item::Declaration(d) => match &d.kind {
                        // A duplicate (E0051) has a function of its own, checked with its
                        // own parameters, so that the duplicate is its one message.
                        ast::DeclarationKind::Function(f) => {
                            if let Some(id) = self.module_function(module, &f.signature) {
                                self.resolve_signature(id, &f.signature, &HashMap::new());
                            }
                        }
                        ast::DeclarationKind::External(sig) => {
                            if let Some(id) = self.module_function(module, sig) {
                                self.resolve_signature(id, sig, &HashMap::new());
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
                        // `self` and `var self`, and a `var` parameter, are part of the
                        // signature: a caller of the interface relies on them (L-11, L-29).
                        && self.funcs[m].self_param == self.funcs[r].self_param
                        && self.funcs[m]
                            .params
                            .iter()
                            .zip(&self.funcs[r].params)
                            .all(|(a, b)| a.var == b.var && a.named_only == b.named_only);
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
