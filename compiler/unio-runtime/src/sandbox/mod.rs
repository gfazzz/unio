//! The sandbox of every program that `unio run` and `unio test` start (requirements SB-1 to
//! SB-9, T-6; docs/runtime.md 7.4).
//!
//! The effective permissions are the permissions of the manifest (`unio.toml`) narrowed by the
//! external policy of the operator (SB-2, SB-3). They are enforced twice:
//!
//! 1. by the runtime: every external function of `std.files`, `std.env` and `std.process`
//!    asks [`may_read`], [`may_write`], [`may_use_env`] or [`may_run`] first and reports a
//!    denied operation through its declared error (SB-6, S-8);
//! 2. by the operating system, on the thread that runs the program and on every process it
//!    starts (SB-1, SB-4): Landlock, seccomp-bpf, `no_new_privs` and resource limits on Linux
//!    (`linux.rs`), a Seatbelt profile and resource limits on macOS (`macos.rs`). Where a
//!    mechanism is missing, [`enter`] fails closed with code 103 unless the operator allows a
//!    weaker sandbox (SB-5).
//!
//! On Linux and macOS the program gets no right to write files from the operating system: a
//! writer process started before the sandbox applies does every write-class operation of
//! `std.files` and checks the write scopes itself (`writer.rs`, docs/runtime.md 9.2).
//!
//! Limits of the policy: the heap through the count of the global allocator of the runtime
//! (code 102, `heap.rs`), backed by the limit of the address space on Linux and the memory
//! limit of a Job Object on Windows (RFC 0010, docs/runtime.md 3.5), wall and
//! CPU time through a watchdog thread (code 102), the number of processes through the runtime,
//! and the size of written files through the runtime and `RLIMIT_FSIZE`.

pub mod toml;

pub mod beneath;
#[cfg(unix)]
mod children;
#[cfg(unix)]
mod fdio;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod tree;
#[cfg(windows)]
#[doc(hidden)]
pub mod windows;
pub mod worker;
#[cfg(unix)]
pub mod writer;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use self::toml::{Table, TomlValue};

// ---------------------------------------------------------------- scopes

/// The values of the placeholders of T-6 and the directory that relative entries start from.
#[derive(Clone, Debug)]
pub struct Places {
    pub home: Option<PathBuf>,
    pub cwd: PathBuf,
    pub temp: PathBuf,
    /// The directory of the manifest or the policy; relative entries are below it.
    pub base: PathBuf,
}

impl Places {
    /// The places of this process, with relative entries below `base`.
    pub fn current(base: &Path) -> Places {
        Places {
            home: crate::externs::system::home_dir().map(PathBuf::from),
            cwd: working_directory().to_path_buf(),
            temp: std::env::temp_dir(),
            base: base.to_path_buf(),
        }
    }
}

/// A path of a `files.read` or `files.write` permission: one file or directory, or with
/// `/**` a directory and everything below it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scope {
    pub root: PathBuf,
    pub subtree: bool,
}

impl Scope {
    /// Whether the resolved path lies in the scope.
    pub fn covers(&self, path: &Path) -> bool {
        if self.subtree {
            path.starts_with(&self.root)
        } else {
            path == self.root
        }
    }

    /// Whether every path of this scope lies in the other one.
    pub fn within(&self, other: &Scope) -> bool {
        other.covers(&self.root) && (!self.subtree || other.subtree)
    }

    /// Expands an entry of a manifest or a policy; `None` for `$HOME` without a home directory,
    /// which then grants nothing.
    pub fn expand(entry: &str, places: &Places) -> Result<Option<Scope>, String> {
        let (body, subtree) = match entry.strip_suffix("/**") {
            Some(body) => (body, true),
            None => (entry, false),
        };
        if body.contains('*') {
            return Err(format!(
                "'{entry}': only the suffix /** may contain '*' (T-6)"
            ));
        }
        let path = match body.strip_prefix('$') {
            Some(after) => {
                let (name, rest) = after.split_once('/').unwrap_or((after, ""));
                let mut path = match name {
                    "HOME" => match &places.home {
                        Some(home) => home.clone(),
                        None => return Ok(None),
                    },
                    "CWD" => places.cwd.clone(),
                    "TEMP" => places.temp.clone(),
                    _ => {
                        return Err(format!(
                            "'{entry}': unknown placeholder ${name}; use $HOME, $CWD or $TEMP"
                        ))
                    }
                };
                for part in rest.split('/').filter(|p| !p.is_empty()) {
                    path.push(part);
                }
                path
            }
            None if body.is_empty() => return Err("an empty path is not a scope".into()),
            None if Path::new(body).is_absolute() => PathBuf::from(body),
            None => places.base.join(body),
        };
        Ok(Some(Scope {
            root: resolve_in(&path, &places.cwd),
            subtree,
        }))
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.root.display())?;
        if self.subtree {
            write!(f, "{}**", std::path::MAIN_SEPARATOR)?;
        }
        Ok(())
    }
}

/// The path as the file system sees it: absolute, without `.` and `..`, and with every link of
/// its existing part resolved, so that a link cannot lead out of a scope.
///
/// On Linux and macOS the system follows a link before it takes the `..` after it, so
/// `a/link/..` is the parent of the link's target, not `a`; the path is resolved one component
/// at a time in that order ([`walk_path`]). On Windows `..` is removed from the text before the
/// system looks at the path, so it is removed lexically there first.
pub fn resolve(path: &Path) -> PathBuf {
    resolve_in(path, working_directory())
}

/// The path of the entry that a write-class operation creates, replaces, renames or removes:
/// its directory resolved as [`resolve`] resolves it, and its last name kept as given, so that a
/// link there is the entry itself and not what it points to. A path that ends in `.` or `..` is
/// resolved as a whole. `None` for a path that names no entry, such as `/`.
pub fn entry_path(path: &Path) -> Option<PathBuf> {
    let absolute = absolute(path);
    match absolute.components().next_back() {
        Some(Component::Normal(name)) => {
            let name = name.to_os_string();
            let parent = absolute.parent()?;
            Some(resolve(parent).join(name))
        }
        _ => {
            let resolved = resolve(&absolute);
            resolved.file_name()?;
            Some(resolved)
        }
    }
}

/// The path made absolute against the remembered working directory, without touching the file
/// system. `canonicalize` of a relative path calls `getcwd`, which Seatbelt refuses when the
/// working directory itself is not readable, so callers resolve the absolute path instead.
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory().join(path)
    }
}

/// The working directory of the process, read once. Unio programs cannot change it, and under
/// Seatbelt `getcwd` fails when the directory itself is not readable, so it is read before the
/// sandbox is applied; otherwise relative paths would not resolve into their scopes.
pub fn working_directory() -> &'static Path {
    static CWD: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CWD.get_or_init(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        std::fs::canonicalize(&cwd).unwrap_or(cwd)
    })
}

fn resolve_in(path: &Path, cwd: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    if cfg!(windows) {
        resolve_lexically(&absolute)
    } else {
        walk_path(&absolute, &mut look_up)
    }
}

/// Windows: `..` and `.` are removed from the text, as the system does, and then the longest
/// existing prefix is resolved through the file system.
fn resolve_lexically(absolute: &Path) -> PathBuf {
    let lexical = lexically_normal(absolute);
    let mut rest = Vec::new();
    let mut prefix = lexical.as_path();
    loop {
        if let Ok(real) = canonical(prefix) {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (prefix.parent(), prefix.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                prefix = parent;
            }
            _ => return lexical,
        }
    }
}

/// `std::fs::canonicalize`, also where an AppContainer cannot normalize a name the usual way
/// (`windows::canonicalize`).
#[cfg(windows)]
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    windows::canonicalize(path)
}

#[cfg(not(windows))]
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// The path without `.`, and with each `..` removing the name before it, by the text alone.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    lexical
}

/// What the file system holds at a path, as [`walk_path`] asks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// Nothing, or nothing the process may see.
    Missing,
    /// A symbolic link with this target.
    Link(PathBuf),
    /// A file or a directory.
    Present,
}

/// The real file system, without following a link at the path itself.
fn look_up(path: &Path) -> Found {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => match std::fs::read_link(path) {
            Ok(target) => Found::Link(target),
            Err(_) => Found::Missing,
        },
        Ok(_) => Found::Present,
        Err(_) => Found::Missing,
    }
}

/// How many links one resolution follows, as Linux allows (ELOOP after 40).
const MAX_LINKS: usize = 40;

/// Resolves an absolute path in the order the system of Linux and macOS does: from the root,
/// one component at a time; a link is replaced by its target, relative to the directory the
/// link is in, before the components after it are taken; `..` goes up from the directory
/// reached so far, which never contains a link. From the first missing component on, nothing
/// below exists, so the rest is taken by its text, and `..` there only removes a missing name.
/// A path with more than [`MAX_LINKS`] links keeps the link at which it stopped, so that an
/// operation on it fails as the system's would. The result has no `.`, no `..`, and no link in
/// the part of it that exists; `lookup` makes it independent of the platform, for the tests.
pub(crate) fn walk_path(absolute: &Path, lookup: &mut dyn FnMut(&Path) -> Found) -> PathBuf {
    enum Part {
        Root(PathBuf),
        Up,
        Name(std::ffi::OsString),
    }
    fn parts(path: &Path, into: &mut Vec<Part>) {
        let mut found = Vec::new();
        let mut root = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => root.push(component.as_os_str()),
                Component::CurDir => {}
                Component::ParentDir => found.push(Part::Up),
                Component::Normal(name) => found.push(Part::Name(name.to_os_string())),
            }
        }
        // The parts go on a stack, the last one first.
        into.extend(found.into_iter().rev());
        if !root.as_os_str().is_empty() {
            into.push(Part::Root(root));
        }
    }
    let mut stack = Vec::new();
    parts(absolute, &mut stack);
    let mut out = PathBuf::new();
    // Names pushed since the first missing one; zero while everything so far exists.
    let mut missing = 0usize;
    let mut links = 0usize;
    while let Some(part) = stack.pop() {
        match part {
            Part::Root(root) => {
                out = root;
                missing = 0;
            }
            Part::Up => {
                missing = missing.saturating_sub(1);
                out.pop();
            }
            Part::Name(name) => {
                out.push(&name);
                if missing > 0 {
                    missing += 1;
                    continue;
                }
                match lookup(&out) {
                    Found::Present => {}
                    Found::Missing => missing = 1,
                    Found::Link(_) if links == MAX_LINKS => missing = 1,
                    Found::Link(target) => {
                        links += 1;
                        out.pop();
                        parts(&target, &mut stack);
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- permissions and policies

/// What a program may do besides reading its arguments and standard input and writing standard
/// output and error (SB-2). The network is never permitted in 0.1 (T-6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Permissions {
    pub read: Vec<Scope>,
    pub write: Vec<Scope>,
    pub env: BTreeSet<String>,
    pub process: BTreeSet<String>,
}

impl Permissions {
    /// Adds the scopes and names of the other permissions.
    pub fn add(&mut self, other: &Permissions) {
        for scope in &other.read {
            if !self.read.contains(scope) {
                self.read.push(scope.clone());
            }
        }
        for scope in &other.write {
            if !self.write.contains(scope) {
                self.write.push(scope.clone());
            }
        }
        self.env.extend(other.env.iter().cloned());
        self.process.extend(other.process.iter().cloned());
    }
}

/// The limits of a policy (SB-3); `None` leaves the limit to the system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub heap_bytes: Option<u64>,
    pub wall_seconds: Option<u64>,
    pub cpu_seconds: Option<u64>,
    pub processes: Option<u64>,
    pub file_size_bytes: Option<u64>,
}

/// The limits of the workers of the tools, the `[compile]` table of a policy (SB-3a, SB-11,
/// RFC 0006): apart from the limits of the program, which never reach a worker. `None` leaves
/// the default of the tools; a program ignores this table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompileLimits {
    pub memory_bytes: Option<u64>,
    pub wall_seconds: Option<u64>,
    pub cpu_seconds: Option<u64>,
}

/// The external policy of the operator (SB-3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// `None` when the policy has no `[permissions]` table and narrows nothing; otherwise the
    /// only permissions it lets through: a kind it does not list is not permitted.
    pub permissions: Option<Permissions>,
    /// The limits of the program.
    pub limits: Limits,
    /// The limits of the workers of the tools.
    pub compile: CompileLimits,
    pub allow_weaker: bool,
}

fn texts(table: &Table, key: &str) -> Result<Vec<String>, String> {
    match table.get(key) {
        None => Ok(Vec::new()),
        Some(TomlValue::Texts(items)) => Ok(items.clone()),
        Some(_) => Err(format!("{key} must be a list of texts")),
    }
}

/// The `[permissions]` of a manifest or a policy as written, before the placeholders and
/// relative entries are expanded: what an executable made by `unio build` keeps of its
/// manifest (docs/runtime.md 8.4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    pub read: Vec<String>,
    pub write: Vec<String>,
    pub env: Vec<String>,
    pub process: Vec<String>,
}

impl Declared {
    /// The permissions with the placeholders and relative entries of `places`.
    pub fn expand(&self, places: &Places) -> Result<Permissions, String> {
        let mut permissions = Permissions::default();
        for (entries, scopes) in [
            (&self.read, &mut permissions.read),
            (&self.write, &mut permissions.write),
        ] {
            for entry in entries {
                if let Some(scope) = Scope::expand(entry, places)? {
                    if !scopes.contains(&scope) {
                        scopes.push(scope);
                    }
                }
            }
        }
        permissions.env = self.env.iter().cloned().collect();
        permissions.process = self.process.iter().cloned().collect();
        Ok(permissions)
    }

    /// The entries of `files.read` and `files.write` that are relative paths, which have a
    /// meaning only next to the file that declares them (T-6a).
    pub fn relative_entries(&self) -> Vec<(&'static str, &str)> {
        let relative = |entry: &str| {
            let body = entry.strip_suffix("/**").unwrap_or(entry);
            !body.is_empty() && !body.starts_with('$') && !Path::new(body).is_absolute()
        };
        let mut found = Vec::new();
        for (key, entries) in [("files.read", &self.read), ("files.write", &self.write)] {
            for entry in entries.iter().filter(|e| relative(e)) {
                found.push((key, entry.as_str()));
            }
        }
        found
    }
}

/// Reads the `permissions.` keys of a manifest or a policy, without expanding them.
fn read_declared(table: &Table) -> Result<Option<Declared>, String> {
    let mut found = false;
    for (key, value) in table {
        let Some(name) = key.strip_prefix("permissions.") else {
            continue;
        };
        found = true;
        match name {
            "files.read" | "files.write" | "env" | "process" => {}
            "network" => {
                if *value != TomlValue::Bool(false) {
                    return Err("permissions.network must be false in Unio 0.1 (T-6)".into());
                }
            }
            _ => {
                return Err(format!(
                    "unknown permission {key}; use files.read, files.write, env, process or network"
                ))
            }
        }
    }
    if !found {
        return Ok(None);
    }
    Ok(Some(Declared {
        read: texts(table, "permissions.files.read")?,
        write: texts(table, "permissions.files.write")?,
        env: texts(table, "permissions.env")?,
        process: texts(table, "permissions.process")?,
    }))
}

/// Reads the `permissions.` keys of a manifest or a policy.
fn read_permissions(table: &Table, places: &Places) -> Result<Option<Permissions>, String> {
    match read_declared(table)? {
        Some(declared) => declared.expand(places).map(Some),
        None => Ok(None),
    }
}

/// The `[permissions]` of a manifest as written; none declared permits nothing (SB-2).
pub fn declared_in_manifest(source: &str) -> Result<Declared, String> {
    let table = toml::parse(source)?;
    Ok(read_declared(&table)?.unwrap_or_default())
}

/// The permissions of a manifest; a manifest without `[permissions]` permits nothing (SB-2).
pub fn parse_manifest(source: &str, places: &Places) -> Result<Permissions, String> {
    let table = toml::parse(source)?;
    Ok(read_permissions(&table, places)?.unwrap_or_default())
}

/// A policy file. Unknown keys are errors, so that a mistyped limit is never silently absent.
pub fn parse_policy(source: &str, places: &Places) -> Result<Policy, String> {
    let table = toml::parse(source)?;
    let mut policy = Policy {
        permissions: read_permissions(&table, places)?,
        ..Policy::default()
    };
    for (key, value) in &table {
        if key.starts_with("permissions.") {
            continue;
        }
        if key == "allow_weaker" {
            match value {
                TomlValue::Bool(b) => policy.allow_weaker = *b,
                _ => return Err("allow_weaker must be true or false".into()),
            }
            continue;
        }
        let limit = match key.as_str() {
            "limits.heap_bytes" => &mut policy.limits.heap_bytes,
            "limits.wall_seconds" => &mut policy.limits.wall_seconds,
            "limits.cpu_seconds" => &mut policy.limits.cpu_seconds,
            "limits.processes" => &mut policy.limits.processes,
            "limits.file_size_bytes" => &mut policy.limits.file_size_bytes,
            "compile.memory_bytes" => &mut policy.compile.memory_bytes,
            "compile.wall_seconds" => &mut policy.compile.wall_seconds,
            "compile.cpu_seconds" => &mut policy.compile.cpu_seconds,
            _ => return Err(format!("unknown policy key {key}")),
        };
        let TomlValue::Int(n) = value else {
            return Err(format!("{key} must be a whole number"));
        };
        let positive = !matches!(key.as_str(), "limits.processes" | "limits.file_size_bytes");
        if *n < 0 || (positive && *n == 0) {
            return Err(format!(
                "{key} must be {}",
                if positive { "positive" } else { "zero or more" }
            ));
        }
        *limit = Some(n.unsigned_abs());
    }
    if let Some(heap) = policy.limits.heap_bytes {
        let minimum = crate::reserve::MINIMUM_HEAP_LIMIT as u64;
        if heap < minimum {
            return Err(format!(
                "limits.heap_bytes = {heap} is below the minimum of {minimum} bytes (2 MiB): the \
                 runtime sets aside a reserve of {} bytes (1 MiB) for release within the limit \
                 (C-17)",
                crate::reserve::RESERVE_BYTES
            ));
        }
    }
    Ok(policy)
}

/// The scopes in both lists: a scope of one list that lies in a scope of the other.
fn intersect_scopes(
    kind: &str,
    manifest: &[Scope],
    policy: &[Scope],
    warnings: &mut Vec<String>,
) -> Vec<Scope> {
    let mut out: Vec<Scope> = Vec::new();
    for m in manifest {
        if policy.iter().any(|p| m.within(p)) && !out.contains(m) {
            out.push(m.clone());
        }
    }
    for p in policy {
        if manifest.iter().any(|m| p.within(m)) {
            if !out.iter().any(|o| p.within(o)) {
                out.push(p.clone());
            }
        } else if !manifest.iter().any(|m| m.within(p)) {
            warnings.push(format!(
                "the policy permits {kind} {p}, which the manifest does not; it has no effect"
            ));
        }
    }
    out
}

fn intersect_names(
    kind: &str,
    manifest: &BTreeSet<String>,
    policy: &BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> BTreeSet<String> {
    for extra in policy.difference(manifest) {
        warnings.push(format!(
            "the policy permits {kind} {extra}, which the manifest does not; it has no effect"
        ));
    }
    manifest.intersection(policy).cloned().collect()
}

/// The effective permissions: the manifest narrowed by the policy (SB-2), with a warning for
/// every permission of the policy that the manifest does not have (SB-3).
pub fn effective(manifest: &Permissions, policy: &Policy) -> (Permissions, Vec<String>) {
    let Some(narrow) = &policy.permissions else {
        return (manifest.clone(), Vec::new());
    };
    let mut warnings = Vec::new();
    let permissions = Permissions {
        read: intersect_scopes("files.read", &manifest.read, &narrow.read, &mut warnings),
        write: intersect_scopes("files.write", &manifest.write, &narrow.write, &mut warnings),
        env: intersect_names("env", &manifest.env, &narrow.env, &mut warnings),
        process: intersect_names("process", &manifest.process, &narrow.process, &mut warnings),
    };
    (permissions, warnings)
}

// ---------------------------------------------------------------- the sandbox of a program

/// Everything the sandbox of one program needs.
#[derive(Clone, Debug)]
pub struct Sandbox {
    pub permissions: Permissions,
    pub limits: Limits,
    /// The home directory, asked before the restrictions apply (std.env.home_dir).
    pub home: Option<String>,
    /// The writer process of the write scopes, when it was started before [`enter`].
    #[cfg(unix)]
    writer: Option<Arc<writer::Handle>>,
}

impl Sandbox {
    pub fn new(permissions: Permissions, limits: Limits) -> Sandbox {
        Sandbox {
            permissions,
            limits,
            home: crate::externs::system::home_dir(),
            #[cfg(unix)]
            writer: None,
        }
    }

    /// Starts the writer process of the write scopes now, if there are any.
    #[cfg(unix)]
    fn start_writer(&mut self) -> Result<(), Refused> {
        if self.writer.is_none() && !self.permissions.write.is_empty() {
            let handle = writer::Handle::start(&self.permissions.write).map_err(Refused)?;
            self.writer = Some(Arc::new(handle));
        }
        Ok(())
    }
}

/// Starts the writer processes of the sandboxes before any restriction applies. Seatbelt
/// restricts the whole process once, and a writer started after it would inherit the profile of
/// the programs, which permits no write, so on macOS every writer starts here; elsewhere each
/// starts in [`enter`], from the thread of its program.
pub fn start_writers(sandboxes: &mut [Sandbox]) -> Result<(), Refused> {
    #[cfg(target_os = "macos")]
    for sandbox in sandboxes.iter_mut() {
        sandbox.start_writer()?;
    }
    let _ = sandboxes;
    Ok(())
}

/// Applies the writer's own restrictions of the operating system, in the writer process.
#[cfg(unix)]
fn apply_writer_os(scopes: &[Scope]) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    return linux::apply_writer(scopes);
    #[cfg(target_os = "macos")]
    return macos::apply_writer(scopes);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = scopes;
        Ok(())
    }
}

struct Active {
    sandbox: Sandbox,
    /// The processes the program has started: its calls of `process.run`, and, where the
    /// system lets the runtime see them, every process a child or its descendants created
    /// (docs/runtime.md 9.5). Shared with the thread or the Job Object that counts the trees.
    processes: Arc<AtomicU64>,
    #[cfg(unix)]
    writer: Option<writer::Session>,
    /// The AppContainers of the program and the tokens of its thread (Windows).
    #[cfg(windows)]
    windows: Option<windows::Program>,
}

/// The limit of processes of the running program and the count it is checked against; `None`
/// without a sandbox or without a limit.
pub fn process_budget() -> Option<(u64, Arc<AtomicU64>)> {
    let a = active()?;
    let limit = a.sandbox.limits.processes?;
    Some((limit, Arc::clone(&a.processes)))
}

/// Runs a write-class operation of `std.files`. On Windows the thread of a program takes the
/// token of its writer for it, the AppContainer that holds the write grants, and returns to
/// the token of the program afterwards (docs/runtime.md 9.2); elsewhere it just runs.
pub(crate) fn writing<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(windows)]
    if let Some(a) = active() {
        if let Some(program) = &a.windows {
            return program.as_writer(f);
        }
    }
    f()
}

/// The AppContainer the children of the running program are created in, with the thread's own
/// token set aside while it creates them (Windows); `None` without a sandbox.
#[cfg(windows)]
pub(crate) fn with_container<R>(f: impl FnOnce(Option<&windows::Container>) -> R) -> R {
    match active() {
        Some(a) => match &a.windows {
            Some(program) => program.without_token(|| f(Some(program.container()))),
            None => f(None),
        },
        None => f(None),
    }
}

/// Calls `f` with the writer of the running program; `None` without a sandbox or without
/// write scopes, and on systems without a writer process (Windows).
#[cfg(unix)]
pub(crate) fn with_writer<R>(f: impl FnOnce(&writer::Session) -> R) -> Option<R> {
    let a = active()?;
    a.writer.as_ref().map(f)
}

thread_local! {
    static ACTIVE: RefCell<Option<Rc<Active>>> = const { RefCell::new(None) };
}

fn active() -> Option<Rc<Active>> {
    ACTIVE.with(|a| a.borrow().clone())
}

/// The path the program reads, checked against the read scopes (files.read): the path
/// resolved as the system resolves it ([`resolve`]), which the operation then opens, so that
/// what is checked and what is opened are the same. `None` if no read scope covers it. Without
/// a sandbox on this thread, as in the tests of the runtime, everything is permitted and the
/// path is used as given.
pub fn read_target(path: &str) -> Option<PathBuf> {
    match active() {
        None => Some(PathBuf::from(path)),
        Some(a) => {
            let resolved = resolve(Path::new(path));
            a.sandbox
                .permissions
                .read
                .iter()
                .any(|s| s.covers(&resolved))
                .then_some(resolved)
        }
    }
}

/// The entry a write-class operation changes, checked against the write scopes (files.write):
/// [`entry_path`], which the operation then uses, or the writer process is asked about.
/// `None` if no write scope covers it. Without a sandbox, the path as given.
pub fn write_target(path: &str) -> Option<PathBuf> {
    match active() {
        None => Some(PathBuf::from(path)),
        Some(a) => {
            let entry = entry_path(Path::new(path))?;
            a.sandbox
                .permissions
                .write
                .iter()
                .any(|s| s.covers(&entry))
                .then_some(entry)
        }
    }
}

/// Whether the program may read the path (files.read).
pub fn may_read(path: &str) -> bool {
    read_target(path).is_some()
}

/// Whether the program may write, create, rename or remove the path (files.write).
pub fn may_write(path: &str) -> bool {
    write_target(path).is_some()
}

/// The path that [`read_target`] returned, split at the scope that covers it, when a sandbox
/// is active on this thread; `None` without one, where the path is used as given.
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn below_scope(target: &Path) -> std::io::Result<Option<beneath::Anchored>> {
    let Some(a) = active() else {
        return Ok(None);
    };
    beneath::anchor(target, &a.sandbox.permissions.read)
        .map(Some)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::PermissionDenied))
}

// The read-class operations below use what the check authorized (release condition 5,
// docs/runtime.md 9.2). In a sandbox on Linux and macOS each opens the checked path from the
// directory of its scope without following a link in any component ([`fdio::open_checked`]),
// so a link that another process puts on the path after the check makes the operation fail
// instead of leading it out of the scope. On Windows the same walk opens each name relative to
// the handle of the one before (`NtCreateFile` with `RootDirectory` and
// `FILE_OPEN_REPARSE_POINT`) and refuses a handle whose attributes name a reparse point
// ([`windows::open_checked`]).

/// Opens a file for reading at a path that [`read_target`] returned.
pub fn open_for_reading(target: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    if let Some(anchored) = below_scope(target)? {
        return fdio::open_checked(&anchored, fdio::Want::Read).map(std::fs::File::from);
    }
    #[cfg(windows)]
    if let Some(anchored) = below_scope(target)? {
        return windows::open_checked(&anchored, windows::Want::Read).map(std::fs::File::from);
    }
    std::fs::File::open(target)
}

/// What exists at a path that [`read_target`] returned, following no link put there after the
/// check (`exists`, `is_directory`).
pub fn entry_kind(target: &Path) -> std::io::Result<beneath::Kind> {
    #[cfg(unix)]
    if let Some(anchored) = below_scope(target)? {
        return fdio::kind_checked(&anchored);
    }
    #[cfg(windows)]
    if let Some(anchored) = below_scope(target)? {
        return windows::kind_checked(&anchored);
    }
    let metadata = std::fs::metadata(target)?;
    Ok(if metadata.is_dir() {
        beneath::Kind::Directory
    } else {
        beneath::Kind::Other
    })
}

/// Calls `each` with the names in the directory at a path that [`read_target`] returned,
/// without `.` and `..`; stops early when `each` returns false.
pub fn list_directory(
    target: &Path,
    each: &mut dyn FnMut(&std::ffi::OsStr) -> bool,
) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(anchored) = below_scope(target)? {
        let dir = fdio::open_checked(&anchored, fdio::Want::Directory)?;
        return fdio::list_names(dir, each);
    }
    #[cfg(windows)]
    if let Some(anchored) = below_scope(target)? {
        let dir = windows::open_checked(&anchored, windows::Want::Directory)?;
        return windows::list_names(&dir, each);
    }
    for entry in std::fs::read_dir(target)? {
        if !each(&entry?.file_name()) {
            break;
        }
    }
    Ok(())
}

/// The existing path that a path [`read_target`] returned names, with every link resolved. In
/// a sandbox the check has resolved it already, so it is confirmed to exist without following a
/// link and returned as it is: the name of a place outside the scope is never reported.
pub fn resolve_existing(target: &Path) -> std::io::Result<PathBuf> {
    #[cfg(any(unix, windows))]
    if below_scope(target)?.is_some() {
        entry_kind(target)?;
        return Ok(target.to_path_buf());
    }
    canonical(&absolute(target))
}

/// The directory that holds an entry of a write-class operation, opened from the directory of
/// the write scope that covers the entry without following a link in any component, and the
/// last name of the entry. On Windows, which has no writer process, the writes of the runtime
/// use it, so that a link put on the path after the check cannot lead them out of the scope
/// (release condition 5, docs/runtime.md 9.2). A temporary file of a commit next to a target of
/// a scope of one file is opened from the directory of that target. `None` without a sandbox.
#[cfg(windows)]
fn write_parent(
    entry: &Path,
) -> std::io::Result<Option<(std::os::windows::io::OwnedHandle, std::ffi::OsString)>> {
    let Some(a) = active() else {
        return Ok(None);
    };
    let write = &a.sandbox.permissions.write;
    let anchored = match beneath::anchor(entry, write) {
        Some(anchored) => anchored,
        None => match (entry.parent(), entry.file_name()) {
            (Some(dir), Some(name))
                if write
                    .iter()
                    .any(|s| !s.subtree && s.root.parent() == Some(dir)) =>
            {
                beneath::Anchored {
                    anchor: dir.to_path_buf(),
                    rest: vec![name.to_os_string()],
                }
            }
            _ => return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        },
    };
    windows::open_parent(&anchored).map(Some)
}

/// Creates a new file for writing at an entry the checks permitted, relative to its directory
/// opened below its scope (Windows); `None` without a sandbox.
#[cfg(windows)]
pub(crate) fn create_new_checked(entry: &Path) -> Option<std::io::Result<std::fs::File>> {
    match write_parent(entry) {
        Ok(Some((dir, name))) => Some(windows::create_new_in(&dir, &name)),
        Ok(None) => None,
        Err(e) => Some(Err(e)),
    }
}

/// Creates a directory at an entry the checks permitted, as [`create_new_checked`].
#[cfg(windows)]
pub(crate) fn make_directory_checked(entry: &Path) -> Option<std::io::Result<()>> {
    match write_parent(entry) {
        Ok(Some((dir, name))) => Some(windows::make_directory_in(&dir, &name)),
        Ok(None) => None,
        Err(e) => Some(Err(e)),
    }
}

/// Removes the file at an entry the checks permitted, as [`create_new_checked`].
#[cfg(windows)]
pub(crate) fn remove_checked(entry: &Path) -> Option<std::io::Result<()>> {
    match write_parent(entry) {
        Ok(Some((dir, name))) => Some(windows::remove_in(&dir, &name)),
        Ok(None) => None,
        Err(e) => Some(Err(e)),
    }
}

/// Renames one entry the checks permitted to another, each opened relative to its directory
/// below its scope; with `replace` an existing target is replaced, written through.
#[cfg(windows)]
pub(crate) fn rename_checked(from: &Path, to: &Path, replace: bool) -> Option<std::io::Result<()>> {
    let parents = write_parent(from).and_then(|f| Ok((f, write_parent(to)?)));
    match parents {
        Ok((Some((from_dir, from_name)), Some((to_dir, to_name)))) => Some(windows::rename_in(
            &from_dir, &from_name, &to_dir, &to_name, replace,
        )),
        Ok(_) => None,
        Err(e) => Some(Err(e)),
    }
}

/// Whether the path is a temporary file `.<name>.unio-tmp-<id>` of the atomic commit protocol
/// next to a target the program may write: a write scope of one file permits its temporary
/// files too.
pub fn may_write_temporary(path: &str) -> bool {
    if may_write(path) {
        return true;
    }
    let p = Path::new(path);
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(target) = name
        .strip_prefix('.')
        .and_then(|n| n.rsplit_once(".unio-tmp-"))
        .map(|(target, _)| target)
    else {
        return false;
    };
    let target = p.with_file_name(target);
    may_write(&target.to_string_lossy())
}

/// The entry of a temporary file that [`may_write_temporary`] permits, as [`write_target`]
/// gives it; `None` if it is not permitted.
pub fn temporary_target(path: &str) -> Option<PathBuf> {
    if !may_write_temporary(path) {
        return None;
    }
    match active() {
        None => Some(PathBuf::from(path)),
        Some(_) => entry_path(Path::new(path)),
    }
}

/// Whether the program may sync the directory: it may read it, or it holds a target the
/// program may write.
pub fn may_sync_directory(path: &str) -> bool {
    match active() {
        None => true,
        Some(a) => {
            let resolved = resolve(Path::new(path));
            may_read(path)
                || a.sandbox
                    .permissions
                    .write
                    .iter()
                    .any(|s| s.covers(&resolved) || s.root.parent() == Some(resolved.as_path()))
        }
    }
}

/// Whether the program may see the environment variable (env).
pub fn may_use_env(name: &str) -> bool {
    match active() {
        None => true,
        Some(a) => a.sandbox.permissions.env.contains(name),
    }
}

/// Whether the program may start the program (process): it must be listed in the manifest,
/// by its bare name or its path exactly as written, and the limit of processes of the policy
/// must not be reached. A listed bare name does not permit a path that ends with that name.
pub fn may_run(program: &str) -> Result<(), String> {
    let Some(a) = active() else {
        return Ok(());
    };
    if !a.sandbox.permissions.process.contains(program) {
        return Err(format!(
            "denied by the sandbox: the manifest does not permit process {program}"
        ));
    }
    let started = a.processes.load(Ordering::SeqCst);
    if let Some(limit) = a.sandbox.limits.processes {
        if started >= limit {
            return Err(format!(
                "denied by the sandbox: the policy limits the program to {limit} processes"
            ));
        }
    }
    a.processes.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// The variables a child process always receives when they are set, besides those granted under
/// `env`: `PATH`, so that the child finds the programs it runs as the runtime found it; on
/// Windows also `SystemRoot`, without which system libraries of the child fail to load. Nothing
/// else is passed: locale, time zone, home, and temporary directory variables reach a child only
/// when the manifest grants them, and a child without them uses the defaults of its system.
#[cfg(windows)]
pub const CHILD_ENVIRONMENT: &[&str] = &["PATH", "SystemRoot"];
#[cfg(not(windows))]
pub const CHILD_ENVIRONMENT: &[&str] = &["PATH"];

/// The environment of a child process (SB-1, SB-6): the variables the program may see under
/// `env`, plus [`CHILD_ENVIRONMENT`], with the values of this process. `None` without a sandbox
/// on this thread, as in the tests of the runtime, where the child inherits everything, just as
/// [`may_use_env`] permits everything there.
pub fn child_environment() -> Option<Vec<(String, std::ffi::OsString)>> {
    let a = active()?;
    let names: BTreeSet<&str> = a
        .sandbox
        .permissions
        .env
        .iter()
        .map(String::as_str)
        .chain(CHILD_ENVIRONMENT.iter().copied())
        .collect();
    Some(
        names
            .into_iter()
            .filter_map(|name| std::env::var_os(name).map(|value| (name.to_string(), value)))
            .collect(),
    )
}

/// The heap limit of the policy in bytes, for [`crate::Runtime::new`].
pub fn heap_limit() -> Option<usize> {
    active()
        .and_then(|a| a.sandbox.limits.heap_bytes)
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
}

/// The limit of the size of written files in bytes.
pub fn file_size_limit() -> Option<u64> {
    active().and_then(|a| a.sandbox.limits.file_size_bytes)
}

/// The home directory asked before the sandbox applied; `None` without a sandbox.
pub fn home_dir() -> Option<Option<String>> {
    active().map(|a| a.sandbox.home.clone())
}

/// The reason of a denied file operation (SB-6).
pub fn denied(permission: &str, path: &str) -> String {
    format!("denied by the sandbox: the permissions do not include {permission} for {path}")
}

// ---------------------------------------------------------------- entering the sandbox

/// A sandbox that could not be applied: the program does not start (code 103).
#[derive(Debug)]
pub struct Refused(pub String);

/// The sandbox of the current thread; dropping it stops the watchdog of the limits and ends the
/// runtime checks of this thread. The restrictions of the operating system stay.
pub struct Guard {
    stop: Arc<AtomicBool>,
    /// The restrictions the operating system could not apply, when the operator allowed a
    /// weaker sandbox; the caller reports them (SB-5).
    pub missing: Vec<String>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }
}

/// Applies the sandbox to the current thread before the program runs (SB-1): the restrictions
/// of the operating system, then the checks of the runtime and the watchdog of the limits.
///
/// `process_wide` is what a mechanism that applies to the whole process at once (Seatbelt)
/// must permit: the union of the programs this process runs, and the sources it still loads.
/// Without a required mechanism this fails, unless `allow_weaker`; then
/// [`Guard::missing`] names what is missing (SB-5).
pub fn enter(
    sandbox: Sandbox,
    process_wide: &Permissions,
    allow_weaker: bool,
) -> Result<Guard, Refused> {
    #[cfg(unix)]
    let mut sandbox = sandbox;
    // The writer starts before the restrictions apply, so that it does not inherit them.
    #[cfg(unix)]
    sandbox.start_writer()?;
    let stop = Arc::new(AtomicBool::new(false));
    let processes = Arc::new(AtomicU64::new(0));
    let applied = apply_os(&sandbox, process_wide, allow_weaker, &processes, &stop)?;
    if sandbox.limits.wall_seconds.is_some() || sandbox.limits.cpu_seconds.is_some() {
        watchdog(&sandbox.limits, Arc::clone(&stop));
    }
    #[cfg(unix)]
    let writer = sandbox.writer.clone().map(writer::Session::new);
    ACTIVE.with(|a| {
        *a.borrow_mut() = Some(Rc::new(Active {
            sandbox,
            processes,
            #[cfg(unix)]
            writer,
            #[cfg(windows)]
            windows: applied.windows,
        }))
    });
    Ok(Guard {
        stop,
        missing: applied.missing,
    })
}

/// Enters only the runtime part of the sandbox on this thread: its checks and its limits,
/// without the restrictions of the operating system, without the watchdog, and without a writer
/// process, so that write-class operations run in the runtime as on Windows. This is what is
/// left where the operator allowed a weaker sandbox and no mechanism is available (SB-5); the
/// tests of the runtime checks use it to show that those checks hold on their own.
#[doc(hidden)]
pub fn enter_checks_only(sandbox: Sandbox) -> Guard {
    ACTIVE.with(|a| {
        *a.borrow_mut() = Some(Rc::new(Active {
            sandbox,
            processes: Arc::new(AtomicU64::new(0)),
            #[cfg(unix)]
            writer: None,
            #[cfg(windows)]
            windows: None,
        }))
    });
    Guard {
        stop: Arc::new(AtomicBool::new(false)),
        missing: Vec::new(),
    }
}

/// What the restrictions of the operating system left: what is missing (SB-5), and on Windows
/// the AppContainers of the program.
struct Applied {
    missing: Vec<String>,
    #[cfg(windows)]
    windows: Option<windows::Program>,
}

/// What a weaker sandbox lacks, as words for the operator.
#[cfg(target_os = "linux")]
fn refuse(missing: &[String]) -> Refused {
    Refused(format!(
        "the sandbox cannot be applied on this system; missing: {}. The operator can allow a \
         weaker sandbox with --allow-weaker-sandbox or allow_weaker = true in the policy (SB-5)",
        missing.join("; ")
    ))
}

#[cfg(target_os = "linux")]
fn apply_os(
    sandbox: &Sandbox,
    _process_wide: &Permissions,
    allow_weaker: bool,
    processes: &Arc<AtomicU64>,
    stop: &Arc<AtomicBool>,
) -> Result<Applied, Refused> {
    working_directory();
    let missing = linux::missing();
    if !allow_weaker {
        // A mechanism the test hook reports as absent counts only where it refuses the start.
        let mut absent = missing.clone();
        absent.extend(linux::missing_by_test_fault());
        if !absent.is_empty() {
            return Err(refuse(&absent));
        }
    }
    linux::apply(&sandbox.permissions, &sandbox.limits).map_err(Refused)?;
    // The limit of processes holds for the whole tree of every child: each process a
    // descendant creates is counted, and creations past the limit fail (docs/runtime.md 9.5).
    if let Some(limit) = sandbox.limits.processes {
        if missing.is_empty() {
            linux::count_processes(limit, Arc::clone(processes), Arc::clone(stop))
                .map_err(Refused)?;
        }
    }
    Ok(Applied { missing })
}

#[cfg(target_os = "macos")]
fn apply_os(
    sandbox: &Sandbox,
    process_wide: &Permissions,
    _allow_weaker: bool,
    _processes: &Arc<AtomicU64>,
    _stop: &Arc<AtomicBool>,
) -> Result<Applied, Refused> {
    working_directory();
    // RFC 0011: the limit of processes is exact only for a user other than root.
    if sandbox.limits.processes.is_some() {
        let (real, effective) = users();
        if let Some(reason) = root_refusal(real, effective) {
            return Err(Refused(reason));
        }
    }
    macos::apply(process_wide, &sandbox.limits).map_err(Refused)?;
    Ok(Applied {
        missing: Vec::new(),
    })
}

/// The Job Object behind the heap limit, once per process: a process limit of a job can only
/// be set for the whole process, and `unio test` enters the sandbox once per file (RFC 0010).
#[cfg(windows)]
fn program_memory_backstop(heap: u64) -> Result<(), String> {
    static APPLIED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    APPLIED
        .get_or_init(|| worker::limit_program_memory(backstop_bytes(heap)))
        .clone()
}

/// Windows: the thread of the program impersonates a token of a fresh AppContainer that holds
/// grants on exactly the read scopes and the listed programs; a second AppContainer holds the
/// write grants for the writes of the runtime; children are created in the first one
/// (`windows.rs`, docs/runtime.md 9.4).
#[cfg(windows)]
fn apply_os(
    sandbox: &Sandbox,
    _process_wide: &Permissions,
    _allow_weaker: bool,
    _processes: &Arc<AtomicU64>,
    _stop: &Arc<AtomicBool>,
) -> Result<Applied, Refused> {
    working_directory();
    if let Some(heap) = sandbox.limits.heap_bytes {
        program_memory_backstop(heap).map_err(Refused)?;
    }
    let entered = if test_fault("appcontainer") {
        Err(std::io::Error::other(
            "the AppContainer cannot be created (test fault)",
        ))
    } else {
        windows::Program::enter(&sandbox.permissions)
    };
    let program = entered.map_err(|e| {
        Refused(format!(
            "the AppContainer of the program cannot be set up: {e}"
        ))
    })?;
    Ok(Applied {
        missing: Vec::new(),
        windows: Some(program),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn apply_os(
    _sandbox: &Sandbox,
    _process_wide: &Permissions,
    allow_weaker: bool,
    _processes: &Arc<AtomicU64>,
    _stop: &Arc<AtomicBool>,
) -> Result<Applied, Refused> {
    let missing =
        vec!["restrictions of the operating system (no sandbox for this system)".to_string()];
    if !allow_weaker {
        return Err(Refused(format!(
            "the sandbox cannot be applied on this system; missing: {}. The operator can allow \
             a weaker sandbox with --allow-weaker-sandbox or allow_weaker = true in the policy \
             (SB-5)",
            missing.join("; ")
        )));
    }
    Ok(Applied { missing })
}

/// Ends the process with code 102 when the wall or CPU time of the policy is used up (C-18:
/// resources are not released on exhaustion).
fn watchdog(limits: &Limits, stop: Arc<AtomicBool>) {
    let wall = limits.wall_seconds.map(Duration::from_secs);
    let cpu = limits.cpu_seconds.map(Duration::from_secs);
    let started = Instant::now();
    let cpu_start = process_cpu_time();
    let spawned = std::thread::Builder::new()
        .name("unio-limits".into())
        .spawn(move || loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let exhausted = if wall.is_some_and(|w| started.elapsed() >= w) {
                Some(("wall time", wall))
            } else if cpu.is_some_and(|c| {
                process_cpu_time().saturating_sub(cpu_start) >= c
            }) {
                Some(("CPU time", cpu))
            } else {
                None
            };
            if let Some((what, limit)) = exhausted {
                let seconds = limit.map_or(0, |l| l.as_secs());
                crate::externs::process::stop_children();
                eprintln!(
                    "fault: the {what} limit of the policy ({seconds} s) is used up; the program ends with code 102"
                );
                std::process::exit(crate::fault::EXIT_EXHAUSTED);
            }
            std::thread::sleep(Duration::from_millis(10));
        });
    // Without a thread for the watchdog the limits cannot be kept: fail closed.
    if spawned.is_err() {
        eprintln!("unio: the watchdog of the time limits cannot start");
        std::process::exit(crate::fault::EXIT_SANDBOX);
    }
}

/// The CPU time this process has used.
#[cfg(unix)]
pub(crate) fn process_cpu_time() -> Duration {
    #[repr(C)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    #[cfg(target_os = "linux")]
    const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
    #[cfg(not(target_os = "linux"))]
    const CLOCK_PROCESS_CPUTIME_ID: i32 = 12;
    let mut time = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes one timespec, whose layout matches on 64-bit Linux and macOS.
    if unsafe { clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &mut time) } != 0 {
        return Duration::ZERO;
    }
    Duration::new(
        time.tv_sec.max(0) as u64,
        time.tv_nsec.clamp(0, 999_999_999) as u32,
    )
}

#[cfg(windows)]
pub(crate) fn process_cpu_time() -> Duration {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    // SAFETY: the pseudo handle of the current process and four FILETIME outputs.
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) } == 0 {
        return Duration::ZERO;
    }
    let ticks = |t: &FileTime| (u64::from(t.high) << 32) | u64::from(t.low);
    Duration::from_nanos((ticks(&k) + ticks(&u)).saturating_mul(100))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn process_cpu_time() -> Duration {
    Duration::ZERO
}

/// The command that starts a child of `process.run` (Unix): the program with its limits
/// ([`limit_child`]). On macOS under a limit of processes (RFC 0011) the files a child may
/// execute are checked first ([`verify_executables`]): a set-user-ID or set-group-ID file fails
/// the call with `PermissionDenied`; the program is then executed by the path the sandbox
/// resolved and checked, with its name as written as `argv[0]`, and the child checks the same
/// files again right before it executes.
#[cfg(unix)]
pub fn child_command(program: &str) -> std::io::Result<std::process::Command> {
    #[cfg(target_os = "macos")]
    if let Some(a) = active() {
        if a.sandbox.limits.processes.is_some() {
            use std::os::unix::process::CommandExt;
            let files = match macos::executable() {
                Some(files) => files.to_vec(),
                None => children::needs(&a.sandbox.permissions.process, &[])
                    .execute
                    .into_iter()
                    .collect(),
            };
            let verified = verify_executables(&files).map_err(|reason| {
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason)
            })?;
            let mut command = match program_paths(program).into_iter().next() {
                Some(path) => {
                    let mut command = std::process::Command::new(path);
                    command.arg0(program);
                    command
                }
                None => std::process::Command::new(program),
            };
            limit_child(&mut command, Some(verified));
            return Ok(command);
        }
    }
    let mut command = std::process::Command::new(program);
    limit_child(&mut command, None);
    Ok(command)
}

/// Applies the limit of CPU time to a child before it runs the program, since a child has its
/// own CPU time, and gives it back the address space that the backstop of the heap limit took
/// from the program (RFC 0010). On macOS, under a limit of processes, the child may start no
/// process (RFC 0011, [`deny_process_creation`]) and checks the `verified` files again before
/// it executes ([`recheck_executables`]).
#[cfg(unix)]
fn limit_child(command: &mut std::process::Command, verified: Option<Vec<Verified>>) {
    use std::os::unix::process::CommandExt;
    let Some(a) = active() else {
        return;
    };
    let cpu = a.sandbox.limits.cpu_seconds;
    let no_creation = cfg!(target_os = "macos") && a.sandbox.limits.processes.is_some();
    if cpu.is_none() && a.sandbox.limits.heap_bytes.is_none() && !no_creation {
        return;
    }
    // SAFETY: the closure calls only getrlimit, setrlimit, open, fstat, lstat, and close on
    // memory prepared before the fork, which are async-signal-safe. An error fails the spawn,
    // so a child whose limits could not be set never runs its program.
    unsafe {
        command.pre_exec(move || {
            // The heap limit is the program's: the child gets its address space back.
            release_backstop();
            if let Some(seconds) = cpu {
                set_limit(RLIMIT_CPU, seconds)?;
            }
            if no_creation {
                deny_process_creation()?;
                if let Some(files) = &verified {
                    recheck_executables(files)?;
                }
            }
            Ok(())
        });
    }
}

/// `RLIMIT_NPROC` of macOS.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const RLIMIT_NPROC_MACOS: i32 = 7;

/// In a child of `process.run` on macOS, before it runs its program, under a limit of processes
/// (RFC 0011, docs/runtime.md 9.5): sets the soft and the hard `RLIMIT_NPROC` to 0, so that the
/// child and every process it would start can create no process. `fork1` of XNU, which serves
/// `fork`, `vfork`, and `posix_spawn`, fails with `EAGAIN` when the processes of the real user
/// of the caller, counting the new one, exceed its soft limit, and a process that is not root
/// cannot raise its hard limit again. The limit is inherited across `fork` and `exec`. A real
/// user of 0 is exempt from the check, so a program run as root is refused
/// ([`root_refusal`]) and no set-user-ID or set-group-ID file may be executed
/// ([`verify_executables`]). Seatbelt cannot express this: the child runs under the profile of
/// the program, which must allow `process-fork` for `process.run` itself, and a process under a
/// profile cannot apply a second one. Async-signal-safe.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn deny_process_creation() -> std::io::Result<()> {
    let none = RLimit {
        current: 0,
        maximum: 0,
    };
    // SAFETY: setrlimit reads one struct rlimit.
    if unsafe { setrlimit(RLIMIT_NPROC_MACOS, &none) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The test hook that makes one mechanism of the operating system fail when the sandbox of a
/// program is applied, so that the tests show the start failing closed with code 103 (SB-5)
/// on systems that have every mechanism. Its values: `landlock-missing` and `seccomp-missing`
/// (Linux) report the mechanism as absent from the kernel, which stops the start unless the
/// operator allows a weaker sandbox, and is then ignored, so that every mechanism still
/// applies; `landlock-refused` and `seccomp-refused` (Linux), `sandbox-init` (macOS) and
/// `appcontainer` (Windows) make the call that applies the mechanism fail, which stops the
/// start whatever the operator allows. It can only refuse a start, never weaken a sandbox.
/// Workers and the writer get a cleared environment and do not see it.
pub const TEST_SANDBOX_FAULT: &str = "UNIO_TEST_SANDBOX_FAULT";

/// Whether [`TEST_SANDBOX_FAULT`] names this fault.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
pub(crate) fn test_fault(name: &str) -> bool {
    std::env::var_os(TEST_SANDBOX_FAULT).is_some_and(|v| v == name)
}

/// The test hook that makes the runtime treat the program as run by root (RFC 0011): set to
/// `1`, a program with a limit of processes on macOS is refused as under root. It can only
/// refuse a start, never permit one.
pub const TEST_AS_ROOT: &str = "UNIO_TEST_AS_ROOT";

/// RFC 0011: why a program with a limit of processes may not start on macOS under these users,
/// or `None`. The kernel does not apply `RLIMIT_NPROC` to a real user of 0, and an effective
/// user of 0 can make its real user 0, so either would make the limit inexact.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn root_refusal(real: u32, effective: u32) -> Option<String> {
    (real == 0 || effective == 0).then(|| {
        format!(
            "the sandbox cannot hold limits.processes for this program: on macOS the limit is \
             exact only because a child may create no process, which the system does not \
             enforce for root, and the program runs with real user {real} and effective user \
             {effective} (RFC 0011). Run it as a user other than root, or without \
             limits.processes"
        )
    })
}

/// The real and effective users of this process, both 0 under [`TEST_AS_ROOT`].
#[cfg(target_os = "macos")]
fn users() -> (u32, u32) {
    extern "C" {
        fn getuid() -> u32;
        fn geteuid() -> u32;
    }
    if std::env::var_os(TEST_AS_ROOT).is_some_and(|v| v == "1") {
        return (0, 0);
    }
    // SAFETY: getuid and geteuid have no memory effects.
    unsafe { (getuid(), geteuid()) }
}

/// A file a child may execute, as [`verify_executables`] found it: its path, ready for a call
/// after the fork, and the device and inode of the file that was checked.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct Verified {
    path: std::ffi::CString,
    device: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
const ELOOP_ERRNO: i32 = 40;
#[cfg(all(unix, not(target_os = "linux")))]
const ELOOP_ERRNO: i32 = 62;

/// The set-user-ID and set-group-ID bits of a mode.
#[cfg(unix)]
const SET_ID: u32 = 0o6000;

/// RFC 0011: checks that no file a child may execute is set-user-ID or set-group-ID, since such
/// a file could make the child root, whom `RLIMIT_NPROC` does not bind. Each file is opened
/// without following a link as its last name and checked through the open descriptor
/// (`fstat`), so the mode is that of the file the path names, not of a file a link leads to;
/// a file that cannot be opened for reading is checked with `lstat`. The paths are those the
/// sandbox resolved for the profile, links resolved, so a path that is a link now, or a file
/// that is not regular, is refused. A file that does not exist is skipped: it cannot be
/// executed. Returns the files as checked, for [`recheck_executables`].
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn verify_executables(paths: &[PathBuf]) -> Result<Vec<Verified>, String> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const O_NOFOLLOW: i32 = 0o400000;
    #[cfg(all(target_os = "linux", not(target_arch = "x86_64")))]
    const O_NOFOLLOW: i32 = 0o100000;
    #[cfg(not(target_os = "linux"))]
    const O_NOFOLLOW: i32 = 0x100;
    let mut verified = Vec::new();
    for path in paths {
        let shown = path.display();
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path);
        let metadata = match opened {
            Ok(file) => file.metadata(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) if e.raw_os_error() == Some(ELOOP_ERRNO) => {
                return Err(format!(
                    "denied by the sandbox: {shown}, which a child may execute, is now a link, \
                     not the file the sandbox resolved (RFC 0011)"
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                std::fs::symlink_metadata(path)
            }
            Err(e) => Err(e),
        }
        .map_err(|e| {
            format!(
                "denied by the sandbox: {shown}, which a child may execute, cannot be checked: {e}"
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(format!(
                "denied by the sandbox: {shown}, which a child may execute, is not a regular \
                 file (RFC 0011)"
            ));
        }
        if metadata.mode() & SET_ID != 0 {
            return Err(format!(
                "denied by the sandbox: under a limit of processes on macOS a child may not \
                 execute the set-user-ID or set-group-ID file {shown} (RFC 0011)"
            ));
        }
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("denied by the sandbox: {shown} contains a zero byte"))?;
        verified.push(Verified {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        });
    }
    Ok(verified)
}

/// In the child, right before it executes (RFC 0011): each verified file must still be the
/// same file (device and inode) and still not set-user-ID or set-group-ID; otherwise the spawn
/// fails with `EPERM`. Async-signal-safe: no allocation, only open, fstat, lstat, and close.
#[cfg(target_os = "macos")]
fn recheck_executables(files: &[Verified]) -> std::io::Result<()> {
    use std::os::raw::{c_char, c_int};
    extern "C" {
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn close(fd: c_int) -> c_int;
        #[cfg_attr(target_arch = "x86_64", link_name = "fstat$INODE64")]
        fn fstat(fd: c_int, buf: *mut u8) -> c_int;
        #[cfg_attr(target_arch = "x86_64", link_name = "lstat$INODE64")]
        fn lstat(path: *const c_char, buf: *mut u8) -> c_int;
        fn __error() -> *mut c_int;
    }
    const O_NOFOLLOW: c_int = 0x100;
    const O_CLOEXEC: c_int = 0x0100_0000;
    const EACCES: c_int = 13;
    const EPERM: i32 = 1;
    const S_IFMT: u16 = 0o170000;
    const S_IFREG: u16 = 0o100000;
    for file in files {
        // struct stat (64-bit inodes) is 144 bytes: st_dev (i32) at 0, st_mode (u16) at 4,
        // st_ino (u64) at 8.
        let mut buffer = [0u64; 32];
        let out = buffer.as_mut_ptr() as *mut u8;
        // SAFETY: the path ends with 0; the buffer is larger than struct stat; the descriptor
        // is closed once.
        let result = unsafe {
            let fd = open(file.path.as_ptr(), O_NOFOLLOW | O_CLOEXEC);
            if fd >= 0 {
                let r = fstat(fd, out);
                close(fd);
                r
            } else if *__error() == EACCES {
                lstat(file.path.as_ptr(), out)
            } else {
                -1
            }
        };
        if result != 0 {
            return Err(std::io::Error::from_raw_os_error(EPERM));
        }
        // SAFETY: the offsets are inside the buffer.
        let (device, mode, inode) = unsafe {
            (
                std::ptr::read_unaligned(out as *const i32),
                std::ptr::read_unaligned(out.add(4) as *const u16),
                std::ptr::read_unaligned(out.add(8) as *const u64),
            )
        };
        let same = device as u64 == file.device && inode == file.inode;
        if !same || mode & S_IFMT != S_IFREG || u32::from(mode) & SET_ID != 0 {
            return Err(std::io::Error::from_raw_os_error(EPERM));
        }
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn recheck_executables(_: &[Verified]) -> std::io::Result<()> {
    Ok(())
}

/// Keeps every descendant of a child in the process group the child leads (docs/runtime.md
/// 9.5): on Linux the child installs a seccomp filter without `setsid` and `setpgid` before it
/// runs the program. Elsewhere this does nothing; macOS finds descendants that left the group
/// through their parents when it stops the tree, and Windows uses a Job Object.
#[cfg(target_os = "linux")]
pub fn keep_in_tree(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    let Some(filter) = linux::tree_filter() else {
        return;
    };
    // SAFETY: the closure makes two prctl calls on memory owned by the closure, which are
    // async-signal-safe. The standard library puts the child in its process group before
    // the closures run, so the filter does not deny that step.
    unsafe {
        command.pre_exec(move || filter.install());
    }
}

#[cfg(not(target_os = "linux"))]
pub fn keep_in_tree(_: &mut std::process::Command) {}

#[cfg(unix)]
const RLIMIT_CPU: i32 = 0;
#[cfg(unix)]
const RLIMIT_FSIZE: i32 = 1;
#[cfg(unix)]
const RLIMIT_CORE: i32 = 4;

/// The allowance of the limit of the operating system over the heap limit of a program (RFC
/// 0010, docs/runtime.md 3.5): mappings made after the sandbox applies besides the heap (the
/// stacks and arenas of the allocator of the threads `process.run` starts, libraries the system
/// loads), 512 MiB of address space, which costs no memory unless it is used.
pub const BACKSTOP_ALLOWANCE: u64 = 512 * 1024 * 1024;

/// What the limit of the operating system allows above what the process maps when the sandbox
/// applies: the heap limit twice, once for the heap and once for what the allocator adds to it
/// (headers, size classes, freed pages it keeps; measured at most 0.6 of the heap for blocks of
/// values, docs/runtime.md 3.5), and [`BACKSTOP_ALLOWANCE`].
pub fn backstop_bytes(heap: u64) -> u64 {
    heap.saturating_mul(2).saturating_add(BACKSTOP_ALLOWANCE)
}

/// Whether this process lowered its limit of the address space behind the heap limit.
#[cfg(target_os = "linux")]
static BACKSTOP: AtomicBool = AtomicBool::new(false);

/// In a child before it runs its program: gives the limit of the address space back up to the
/// hard limit, which the backstop of the program left as it was. Async-signal-safe; a failure
/// leaves the child with the limit of the program.
#[cfg(target_os = "linux")]
pub(crate) fn release_backstop() {
    if BACKSTOP.load(Ordering::Relaxed) {
        let _ = raise_soft_limit(linux::RLIMIT_AS);
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn release_backstop() {}

#[repr(C)]
#[cfg(unix)]
struct RLimit {
    current: u64,
    maximum: u64,
}

#[cfg(unix)]
extern "C" {
    fn getrlimit(resource: i32, limit: *mut RLimit) -> i32;
    fn setrlimit(resource: i32, limit: *const RLimit) -> i32;
}

/// Lowers the soft limit of a resource to `value`, keeping the hard limit.
#[cfg(target_os = "linux")]
fn set_soft_limit(resource: i32, value: u64) -> std::io::Result<()> {
    let mut old = RLimit {
        current: 0,
        maximum: 0,
    };
    // SAFETY: getrlimit and setrlimit read and write one struct rlimit.
    unsafe {
        if getrlimit(resource, &mut old) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let new = RLimit {
            current: value.min(old.maximum),
            maximum: old.maximum,
        };
        if setrlimit(resource, &new) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Raises the soft limit of a resource to its hard limit.
#[cfg(target_os = "linux")]
fn raise_soft_limit(resource: i32) -> std::io::Result<()> {
    let mut old = RLimit {
        current: 0,
        maximum: 0,
    };
    // SAFETY: getrlimit and setrlimit read and write one struct rlimit; both are
    // async-signal-safe.
    unsafe {
        if getrlimit(resource, &mut old) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        old.current = old.maximum;
        if setrlimit(resource, &old) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The soft limit of a resource, for tests.
#[cfg(all(test, target_os = "linux"))]
fn soft_limit(resource: i32) -> u64 {
    let mut old = RLimit {
        current: 0,
        maximum: 0,
    };
    // SAFETY: getrlimit writes one struct rlimit.
    unsafe { getrlimit(resource, &mut old) };
    old.current
}

/// Sets a soft and hard resource limit; a limit can only be lowered.
#[cfg(unix)]
fn set_limit(resource: i32, value: u64) -> std::io::Result<()> {
    let mut old = RLimit {
        current: 0,
        maximum: 0,
    };
    // SAFETY: getrlimit and setrlimit read and write one struct rlimit.
    unsafe {
        if getrlimit(resource, &mut old) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let value = value.min(old.maximum);
        let new = RLimit {
            current: value,
            maximum: value,
        };
        if setrlimit(resource, &new) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The resource limits of both Unix systems: no core files, which could leak the memory of the
/// program, and the size of written files, where exceeding it fails the write with EFBIG
/// instead of a signal.
#[cfg(unix)]
fn apply_rlimits(limits: &Limits) -> Result<(), String> {
    extern "C" {
        fn signal(signal: i32, handler: usize) -> usize;
    }
    const SIGXFSZ: i32 = 25;
    const SIG_IGN: usize = 1;
    set_limit(RLIMIT_CORE, 0).map_err(|e| format!("RLIMIT_CORE: {e}"))?;
    if let Some(bytes) = limits.file_size_bytes {
        // SAFETY: ignoring SIGXFSZ makes a write over the limit fail with EFBIG.
        unsafe {
            signal(SIGXFSZ, SIG_IGN);
        }
        set_limit(RLIMIT_FSIZE, bytes).map_err(|e| format!("RLIMIT_FSIZE: {e}"))?;
    }
    Ok(())
}

/// Every path that `PATH` names for a bare program, or the path itself; tried in order.
#[cfg(unix)]
fn program_paths(program: &str) -> Vec<PathBuf> {
    if program.contains('/') {
        return vec![resolve(Path::new(program))];
    }
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(program))
                .filter(|p| p.is_file())
                .map(|p| resolve(&p))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 0010: the heap limit of a program is backed by the limit of the address space, so
    /// that memory the count of the allocator does not see cannot grow without bound: beyond
    /// the backstop the system refuses it, and the program ends with code 102 and the prepared
    /// diagnostic. The test runs itself again as a child, which lowers its own limit.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_heap_limit_is_backed_by_the_address_space() {
        if std::env::var_os("UNIO_TEST_BACKSTOP").is_some() {
            let heap = 64 << 20;
            let mapped = || {
                let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
                statm
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
                    * 4096
            };
            let before = mapped();
            linux::backstop(heap).unwrap();
            let soft = soft_limit(linux::RLIMIT_AS);
            assert!(soft >= before + backstop_bytes(heap), "{soft}");
            assert!(soft <= mapped() + backstop_bytes(heap), "{soft}");
            assert_eq!(backstop_bytes(heap), 2 * heap + BACKSTOP_ALLOWANCE);
            // A block outside the count of the heap, larger than the backstop allows.
            let huge = crate::heap::outside(|| Vec::<u8>::with_capacity(soft as usize));
            std::hint::black_box(&huge);
            unreachable!("the system gave {soft} bytes beyond the backstop");
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sandbox::tests::the_heap_limit_is_backed_by_the_address_space",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("UNIO_TEST_BACKSTOP", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(102), "{stderr}");
        assert!(
            stderr.contains(crate::fault::Exhaustion::Heap.message()),
            "{stderr}"
        );
    }

    #[cfg(unix)]
    fn places() -> Places {
        Places {
            home: Some(PathBuf::from("/h/ann")),
            cwd: PathBuf::from("/w"),
            temp: PathBuf::from("/t"),
            base: PathBuf::from("/w/pkg"),
        }
    }

    fn scope(root: &str, subtree: bool) -> Scope {
        Scope {
            root: PathBuf::from(root),
            subtree,
        }
    }

    #[cfg(unix)]
    #[test]
    fn placeholders_subtrees_and_relative_entries() {
        let p = places();
        let s = |e: &str| Scope::expand(e, &p).unwrap().unwrap();
        assert_eq!(s("$HOME/.todo.json"), scope("/h/ann/.todo.json", false));
        assert_eq!(s("$CWD/**"), scope("/w", true));
        assert_eq!(s("$TEMP/out/**"), scope("/t/out", true));
        assert_eq!(s("data/a/../b.txt"), scope("/w/pkg/data/b.txt", false));
        assert_eq!(s("/abs/hosts"), scope("/abs/hosts", false));
        assert!(Scope::expand("$USER/x", &p).is_err());
        assert!(Scope::expand("*.txt", &p).is_err());
        let homeless = Places { home: None, ..p };
        assert_eq!(Scope::expand("$HOME/x", &homeless).unwrap(), None);
    }

    #[test]
    fn scopes_cover_paths_and_lie_in_each_other() {
        let tree = scope("/work", true);
        let file = scope("/work/a.txt", false);
        assert!(tree.covers(Path::new("/work")));
        assert!(tree.covers(Path::new("/work/x/y")));
        assert!(!tree.covers(Path::new("/workshop")));
        assert!(file.covers(Path::new("/work/a.txt")));
        assert!(!file.covers(Path::new("/work/a.txt/b")));
        assert!(file.within(&tree));
        assert!(!tree.within(&file));
        assert!(scope("/work/sub", true).within(&tree));
        assert!(!scope("/work", true).within(&scope("/work", false)));
    }

    #[cfg(unix)]
    #[test]
    fn manifests_without_permissions_permit_nothing() {
        let p = places();
        assert_eq!(
            parse_manifest("[package]\nname = \"x\"\n", &p).unwrap(),
            Permissions::default()
        );
        let m = parse_manifest(
            "[permissions]\nfiles.read = [\"$CWD/**\"]\nfiles.write = [\"$HOME/.todo.json\"]\nenv = [\"HOME\", \"TODO_FILE\"]\nprocess = [\"git\"]\nnetwork = false\n",
            &p,
        )
        .unwrap();
        assert_eq!(m.read, vec![scope("/w", true)]);
        assert_eq!(m.write, vec![scope("/h/ann/.todo.json", false)]);
        assert!(m.env.contains("TODO_FILE") && m.process.contains("git"));
        assert!(parse_manifest("[permissions]\nnetwork = true\n", &p).is_err());
        assert!(parse_manifest("[permissions]\nfiles.exec = [\"/bin\"]\n", &p).is_err());
        assert!(parse_manifest("[permissions]\nenv = \"HOME\"\n", &p).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn policies_read_limits_and_reject_mistakes() {
        let p = places();
        let policy = parse_policy(
            "allow_weaker = true\n[limits]\nheap_bytes = 4_000_000\nwall_seconds = 5\ncpu_seconds = 2\nprocesses = 0\nfile_size_bytes = 10\n",
            &p,
        )
        .unwrap();
        assert!(policy.allow_weaker);
        assert_eq!(policy.permissions, None);
        assert_eq!(
            policy.limits,
            Limits {
                heap_bytes: Some(4_000_000),
                wall_seconds: Some(5),
                cpu_seconds: Some(2),
                processes: Some(0),
                file_size_bytes: Some(10),
            }
        );
        assert!(parse_policy("[limits]\nheap = 5\n", &p).is_err());
        assert!(parse_policy("[limits]\nwall_seconds = 0\n", &p).is_err());
        assert!(parse_policy("[limits]\nheap_bytes = \"1 GiB\"\n", &p).is_err());
        // The reserve for `release` lies within the heap limit, so the limit is at least 2 MiB.
        let low = parse_policy("[limits]\nheap_bytes = 2_097_151\n", &p).unwrap_err();
        assert!(
            low.contains("2097151") && low.contains("2097152") && low.contains("1048576"),
            "{low}"
        );
        assert!(parse_policy("[limits]\nheap_bytes = 2_097_152\n", &p).is_ok());
        assert!(parse_policy("allow_weaker = 1\n", &p).is_err());
        assert!(parse_policy("[limits\n", &p).is_err());
    }

    /// The limits of the workers of the tools are a table of their own (RFC 0006): the limits
    /// of the program leave them unset, and their keys follow the rules of the other limits.
    #[test]
    fn policies_read_compile_limits_apart_from_the_program_limits() {
        let p = Places::current(&std::env::temp_dir());
        let policy = parse_policy(
            "[limits]\nheap_bytes = 2_097_152\nwall_seconds = 1\ncpu_seconds = 1\n",
            &p,
        )
        .unwrap();
        assert_eq!(policy.compile, CompileLimits::default());
        let policy = parse_policy(
            "[compile]\nmemory_bytes = 536_870_912\nwall_seconds = 30\ncpu_seconds = 20\n",
            &p,
        )
        .unwrap();
        assert_eq!(
            policy.compile,
            CompileLimits {
                memory_bytes: Some(536_870_912),
                wall_seconds: Some(30),
                cpu_seconds: Some(20),
            }
        );
        assert_eq!(policy.limits, Limits::default());
        let unknown = parse_policy("[compile]\nheap_bytes = 1\n", &p).unwrap_err();
        assert!(
            unknown.contains("unknown policy key compile.heap_bytes"),
            "{unknown}"
        );
        assert!(parse_policy("[compile]\nwall_seconds = 0\n", &p).is_err());
        assert!(parse_policy("[compile]\nmemory_bytes = -1\n", &p).is_err());
        assert!(parse_policy("[compile]\ncpu_seconds = \"1\"\n", &p).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_policy_only_narrows_and_warns_about_extra_permissions() {
        let p = places();
        let manifest = parse_manifest(
            "[permissions]\nfiles.read = [\"$CWD/**\", \"/abs/hosts\"]\nfiles.write = [\"$CWD/out/**\"]\nenv = [\"HOME\", \"LANG\"]\nprocess = [\"git\"]\n",
            &p,
        )
        .unwrap();
        // No [permissions] in the policy: nothing is narrowed.
        let (same, warnings) = effective(&manifest, &Policy::default());
        assert_eq!(same, manifest);
        assert!(warnings.is_empty());
        let policy = parse_policy(
            "[permissions]\nfiles.read = [\"$CWD/src/**\", \"/abs/**\", \"/h/**\"]\nfiles.write = [\"$CWD/**\"]\nenv = [\"HOME\", \"SECRET\"]\n",
            &p,
        )
        .unwrap();
        let (narrowed, warnings) = effective(&manifest, &policy);
        assert_eq!(
            narrowed.read,
            vec![scope("/abs/hosts", false), scope("/w/src", true)]
        );
        assert_eq!(narrowed.write, vec![scope("/w/out", true)]);
        assert_eq!(narrowed.env, BTreeSet::from(["HOME".to_string()]));
        // A kind the policy does not list is not permitted.
        assert!(narrowed.process.is_empty());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("/h")));
        assert!(warnings.iter().any(|w| w.contains("env SECRET")));
    }

    #[test]
    fn the_runtime_checks_follow_the_active_sandbox() {
        let dir = std::env::temp_dir().join(format!("unio-sandbox-checks-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("in")).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let text = |p: PathBuf| p.to_string_lossy().into_owned();
        let target = dir.join("in").join("t.json");
        let result = std::thread::spawn({
            let dir = dir.clone();
            move || {
                assert!(may_read("anything") && may_use_env("ANY") && may_run("x").is_ok());
                let permissions = Permissions {
                    read: vec![Scope {
                        root: dir.join("in"),
                        subtree: true,
                    }],
                    write: vec![Scope {
                        root: target.clone(),
                        subtree: false,
                    }],
                    env: BTreeSet::from(["HOME".to_string()]),
                    process: BTreeSet::from(["git".to_string()]),
                };
                let limits = Limits {
                    processes: Some(1),
                    heap_bytes: Some(4096),
                    ..Limits::default()
                };
                let sandbox = Sandbox::new(permissions, limits);
                // Only the runtime layer: the restrictions of the operating system are tested
                // in their own modules.
                ACTIVE.with(|a| {
                    *a.borrow_mut() = Some(Rc::new(Active {
                        sandbox,
                        processes: Arc::new(AtomicU64::new(0)),
                        #[cfg(unix)]
                        writer: None,
                        #[cfg(windows)]
                        windows: None,
                    }))
                });
                assert!(may_read(&text(dir.join("in"))));
                assert!(may_read(&text(dir.join("in/sub/../x.txt"))));
                assert!(!may_read(&text(dir.join("in/../secret"))));
                assert!(!may_read(&text(dir.clone())));
                assert!(may_write(&text(target.clone())));
                assert!(!may_write(&text(dir.join("in/other.json"))));
                assert!(may_write_temporary(&text(
                    dir.join("in/.t.json.unio-tmp-12-1")
                )));
                assert!(!may_write_temporary(&text(dir.join("in/.x.unio-tmp-1"))));
                assert!(may_sync_directory(&text(dir.join("in"))));
                assert!(!may_sync_directory(&text(dir.clone())));
                assert!(may_use_env("HOME") && !may_use_env("PATH"));
                assert!(may_run("/usr/bin/git").is_err());
                assert!(may_run("git").is_ok());
                assert!(may_run("git").unwrap_err().contains("limits"));
                assert_eq!(heap_limit(), Some(4096));
                let environment = child_environment().unwrap();
                assert!(environment
                    .iter()
                    .all(|(name, _)| name == "HOME" || CHILD_ENVIRONMENT.contains(&name.as_str())));
                ACTIVE.with(|a| *a.borrow_mut() = None);
            }
        })
        .join();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_ok());
    }

    /// A file system of names: a link maps to its target, anything else listed is present.
    fn fake(entries: &[(&str, Option<&str>)]) -> impl FnMut(&Path) -> Found {
        let entries: Vec<(PathBuf, Option<PathBuf>)> = entries
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.map(PathBuf::from)))
            .collect();
        move |path: &Path| match entries.iter().find(|(p, _)| p == path) {
            None => Found::Missing,
            Some((_, None)) => Found::Present,
            Some((_, Some(target))) => Found::Link(target.clone()),
        }
    }

    /// The resolution of Linux and macOS, on a file system that needs no platform: a link is
    /// followed before the `..` after it, as the system does.
    #[test]
    fn dot_dot_after_a_link_goes_up_from_the_link_target() {
        let mut fs = fake(&[
            ("/a", None),
            ("/a/link", Some("/other/deep")),
            ("/a/relative", Some("../other/deep")),
            ("/a/secret", None),
            ("/other", None),
            ("/other/deep", None),
            ("/other/secret", None),
            ("/b", None),
            ("/b/back", Some("/a/sub")),
            ("/a/sub", None),
            ("/a/loop", Some("/a/loop")),
            ("/a/chain", Some("link/..")),
        ]);
        let r = |p: &str, fs: &mut dyn FnMut(&Path) -> Found| walk_path(Path::new(p), fs);
        // The audit's case: lexically /a/secret, really /other/secret.
        assert_eq!(r("/a/link/../secret", &mut fs), Path::new("/other/secret"));
        assert_eq!(
            r("/a/relative/../secret", &mut fs),
            Path::new("/other/secret")
        );
        assert_eq!(r("/a/chain/secret", &mut fs), Path::new("/other/secret"));
        // The converse: lexically /b/x, really /a/x.
        assert_eq!(r("/b/back/../x", &mut fs), Path::new("/a/x"));
        // Without links `..` and `.` are what they look like.
        assert_eq!(r("/a/./sub/../secret", &mut fs), Path::new("/a/secret"));
        assert_eq!(r("/../a/..", &mut fs), Path::new("/"));
        // Below a missing name nothing exists: the rest is taken by its text, and a `..` that
        // leaves the missing part looks at the file system again.
        assert_eq!(r("/a/new/x/../y", &mut fs), Path::new("/a/new/y"));
        assert_eq!(
            r("/a/new/../link/../secret", &mut fs),
            Path::new("/other/secret")
        );
        // A loop stops at the link, which the operation then fails on.
        assert_eq!(r("/a/loop/x", &mut fs), Path::new("/a/loop/x"));
    }

    #[cfg(unix)]
    #[test]
    fn the_checks_authorize_what_the_system_opens() {
        let dir = std::env::temp_dir().join(format!("unio-sandbox-dotdot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["a/sub", "other/deep", "b"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("other/secret"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("other/deep"), dir.join("a/link")).unwrap();
        std::os::unix::fs::symlink(dir.join("a/sub"), dir.join("b/back")).unwrap();
        let real = std::fs::canonicalize(&dir).unwrap();
        let text = |p: PathBuf| p.to_string_lossy().into_owned();
        let result = std::thread::spawn({
            let real = real.clone();
            move || {
                let tree = Scope {
                    root: real.join("a"),
                    subtree: true,
                };
                let permissions = Permissions {
                    read: vec![tree.clone()],
                    write: vec![tree],
                    ..Permissions::default()
                };
                let _guard = enter_checks_only(Sandbox::new(permissions, Limits::default()));
                let escape = text(real.join("a/link/../secret"));
                assert_eq!(resolve(Path::new(&escape)), real.join("other/secret"));
                assert!(!may_read(&escape) && !may_write(&escape));
                assert!(read_target(&escape).is_none());
                let inside = text(real.join("b/back/../x"));
                assert_eq!(read_target(&inside), Some(real.join("a/x")));
                assert_eq!(write_target(&inside), Some(real.join("a/x")));
                // A write names the entry: a link as the last name is not followed.
                assert_eq!(
                    write_target(&text(real.join("a/link"))),
                    Some(real.join("a/link"))
                );
                assert!(read_target(&text(real.join("a/link"))).is_none());
                assert_eq!(
                    writer::target_path(&text(real.join("a/link/../y"))),
                    Some(real.join("other/y"))
                );
            }
        })
        .join();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn links_do_not_lead_out_of_a_scope() {
        let dir = std::env::temp_dir().join(format!("unio-sandbox-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("in")).unwrap();
        std::fs::write(dir.join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("secret"), dir.join("in/link")).unwrap();
        let real = std::fs::canonicalize(&dir).unwrap();
        let tree = Scope {
            root: real.join("in"),
            subtree: true,
        };
        assert!(!tree.covers(&resolve(&dir.join("in/link"))));
        assert!(tree.covers(&resolve(&dir.join("in/new/file"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RFC 0011: a program with a limit of processes is refused when its real or effective
    /// user is root; the reason names both users and the way out.
    #[cfg(unix)]
    #[test]
    fn root_is_refused_a_limit_of_processes() {
        assert_eq!(root_refusal(501, 501), None);
        for (real, effective) in [(0, 501), (501, 0), (0, 0)] {
            let reason = root_refusal(real, effective).expect("root is refused");
            assert!(
                reason.contains(&format!("real user {real} and effective user {effective}"))
                    && reason.contains("RFC 0011")
                    && reason.contains("without limits.processes"),
                "{reason}"
            );
        }
    }

    /// RFC 0011: the files a child may execute are checked through an open descriptor: a
    /// set-user-ID or set-group-ID file, a path that is now a link, and a directory are refused;
    /// a missing file is skipped; a plain executable passes with its device and inode.
    #[cfg(unix)]
    #[test]
    fn set_id_files_links_and_directories_are_refused_to_children() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = std::env::temp_dir().join(format!("unio-sandbox-setid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let plain = dir.join("plain");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        let verified = verify_executables(&[plain.clone(), dir.join("missing")]).unwrap();
        assert_eq!(verified.len(), 1, "a missing file is skipped");
        let metadata = std::fs::metadata(&plain).unwrap();
        assert_eq!(
            (verified[0].device, verified[0].inode),
            (metadata.dev(), metadata.ino())
        );
        for (name, mode, words) in [
            ("setuid", 0o4755, "set-user-ID or set-group-ID"),
            ("setgid", 0o2755, "set-user-ID or set-group-ID"),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, "x").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let mode_set = std::fs::metadata(&path).unwrap().mode() & 0o7777;
            assert_eq!(mode_set, mode, "the file system keeps the mode of {name}");
            let reason = verify_executables(&[plain.clone(), path.clone()]).unwrap_err();
            assert!(reason.contains(words) && reason.contains(name), "{reason}");
        }
        let link = dir.join("link");
        std::os::unix::fs::symlink(&plain, &link).unwrap();
        let reason = verify_executables(&[link]).unwrap_err();
        assert!(reason.contains("is now a link"), "{reason}");
        let reason = verify_executables(std::slice::from_ref(&dir)).unwrap_err();
        assert!(reason.contains("not a regular file"), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// RFC 0011: a child whose `RLIMIT_NPROC` was set by [`deny_process_creation`] can start no
    /// process. The child tries `fork` inside the closure that runs before its program, and
    /// reports the result as the error of the spawn: `EAGAIN` when the fork was refused. The
    /// real user of 0 is exempt from the limit, so the test fails under root instead of
    /// passing without checking anything.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_child_without_process_creation_cannot_fork() {
        use std::os::unix::process::CommandExt;
        extern "C" {
            fn fork() -> i32;
            fn getuid() -> u32;
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
            fn _exit(code: i32) -> !;
        }
        const EAGAIN: i32 = 35;
        // SAFETY: getuid has no memory effects.
        let user = unsafe { getuid() };
        assert_ne!(
            user, 0,
            "the test needs a user other than root, whom RLIMIT_NPROC does not bind"
        );
        let mut command = std::process::Command::new("/usr/bin/true");
        // SAFETY: the closure calls setrlimit, fork, waitpid and _exit, which are
        // async-signal-safe; the grandchild, if any, ends at once.
        unsafe {
            command.pre_exec(|| {
                deny_process_creation()?;
                let pid = fork();
                if pid == 0 {
                    _exit(0);
                }
                if pid > 0 {
                    waitpid(pid, std::ptr::null_mut(), 0);
                    return Err(std::io::Error::other("BREACH: fork succeeded"));
                }
                Err(std::io::Error::last_os_error())
            });
        }
        let error = command
            .spawn()
            .expect_err("the closure always fails the spawn");
        assert_eq!(error.raw_os_error(), Some(EAGAIN), "{error}");
        // Without the limit the same closure forks, so the refusal above is the limit's.
        let mut control = std::process::Command::new("/usr/bin/true");
        // SAFETY: as above.
        unsafe {
            control.pre_exec(|| {
                let pid = fork();
                if pid == 0 {
                    _exit(0);
                }
                if pid > 0 {
                    waitpid(pid, std::ptr::null_mut(), 0);
                    return Ok(());
                }
                Err(std::io::Error::last_os_error())
            });
        }
        let status = control.status().expect("a child without the limit forks");
        assert!(status.success());
    }
}
