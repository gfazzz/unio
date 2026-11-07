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
//! Limits of the policy: the heap through the runtime's own accounting (code 102), wall and
//! CPU time through a watchdog thread (code 102), the number of processes through the runtime,
//! and the size of written files through the runtime and `RLIMIT_FSIZE`.

pub mod toml;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
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
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
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

/// The path as the file system sees it: absolute, without `.` and `..`, and with the links of
/// its longest existing prefix resolved, so that a link cannot lead out of a scope.
pub fn resolve(path: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_in(path, &cwd)
}

fn resolve_in(path: &Path, cwd: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    let mut rest = Vec::new();
    let mut prefix = lexical.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(prefix) {
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

/// The external policy of the operator (SB-3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// `None` when the policy has no `[permissions]` table and narrows nothing; otherwise the
    /// only permissions it lets through: a kind it does not list is not permitted.
    pub permissions: Option<Permissions>,
    pub limits: Limits,
    pub allow_weaker: bool,
}

fn texts(table: &Table, key: &str) -> Result<Vec<String>, String> {
    match table.get(key) {
        None => Ok(Vec::new()),
        Some(TomlValue::Texts(items)) => Ok(items.clone()),
        Some(_) => Err(format!("{key} must be a list of texts")),
    }
}

/// Reads the `permissions.` keys of a manifest or a policy.
fn read_permissions(table: &Table, places: &Places) -> Result<Option<Permissions>, String> {
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
    let mut permissions = Permissions::default();
    for (key, scopes) in [
        ("permissions.files.read", &mut permissions.read),
        ("permissions.files.write", &mut permissions.write),
    ] {
        for entry in texts(table, key)? {
            if let Some(scope) = Scope::expand(&entry, places)? {
                if !scopes.contains(&scope) {
                    scopes.push(scope);
                }
            }
        }
    }
    permissions.env = texts(table, "permissions.env")?.into_iter().collect();
    permissions.process = texts(table, "permissions.process")?.into_iter().collect();
    Ok(Some(permissions))
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
}

impl Sandbox {
    pub fn new(permissions: Permissions, limits: Limits) -> Sandbox {
        Sandbox {
            permissions,
            limits,
            home: crate::externs::system::home_dir(),
        }
    }
}

struct Active {
    sandbox: Sandbox,
    processes: Cell<u64>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Rc<Active>>> = const { RefCell::new(None) };
}

fn active() -> Option<Rc<Active>> {
    ACTIVE.with(|a| a.borrow().clone())
}

/// Whether the program may read the path (files.read). Without a sandbox on this thread, as in
/// the tests of the runtime, everything is permitted.
pub fn may_read(path: &str) -> bool {
    match active() {
        None => true,
        Some(a) => {
            let resolved = resolve(Path::new(path));
            a.sandbox
                .permissions
                .read
                .iter()
                .any(|s| s.covers(&resolved))
        }
    }
}

/// Whether the program may write, create, rename or remove the path (files.write).
pub fn may_write(path: &str) -> bool {
    match active() {
        None => true,
        Some(a) => {
            let resolved = resolve(Path::new(path));
            a.sandbox
                .permissions
                .write
                .iter()
                .any(|s| s.covers(&resolved))
        }
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
    let started = a.processes.get();
    if let Some(limit) = a.sandbox.limits.processes {
        if started >= limit {
            return Err(format!(
                "denied by the sandbox: the policy limits the program to {limit} processes"
            ));
        }
    }
    a.processes.set(started + 1);
    Ok(())
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
    let missing = apply_os(&sandbox, process_wide, allow_weaker)?;
    let stop = Arc::new(AtomicBool::new(false));
    if sandbox.limits.wall_seconds.is_some() || sandbox.limits.cpu_seconds.is_some() {
        watchdog(&sandbox.limits, Arc::clone(&stop));
    }
    ACTIVE.with(|a| {
        *a.borrow_mut() = Some(Rc::new(Active {
            sandbox,
            processes: Cell::new(0),
        }))
    });
    Ok(Guard { stop, missing })
}

/// What a weaker sandbox lacks, as words for the operator.
#[cfg(not(target_os = "macos"))]
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
) -> Result<Vec<String>, Refused> {
    let missing = linux::missing();
    if !missing.is_empty() && !allow_weaker {
        return Err(refuse(&missing));
    }
    linux::apply(&sandbox.permissions, &sandbox.limits).map_err(Refused)?;
    Ok(missing)
}

#[cfg(target_os = "macos")]
fn apply_os(
    sandbox: &Sandbox,
    process_wide: &Permissions,
    _allow_weaker: bool,
) -> Result<Vec<String>, Refused> {
    macos::apply(process_wide, &sandbox.limits).map_err(Refused)?;
    Ok(Vec::new())
}

/// Windows and other systems: AppContainer and Job Objects are not implemented yet (SB-4), so
/// only the checks of the runtime and the limits of the watchdog apply.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn apply_os(
    _sandbox: &Sandbox,
    _process_wide: &Permissions,
    allow_weaker: bool,
) -> Result<Vec<String>, Refused> {
    let missing = vec![
        "file system restrictions of the operating system (AppContainer is not implemented)"
            .to_string(),
        "restrictions of child processes and the network (Job Object and AppContainer are not \
         implemented)"
            .to_string(),
    ];
    if !allow_weaker {
        return Err(refuse(&missing));
    }
    Ok(missing)
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
fn process_cpu_time() -> Duration {
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
fn process_cpu_time() -> Duration {
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
fn process_cpu_time() -> Duration {
    Duration::ZERO
}

/// Applies the limit of the size of written files to a child before it runs the program;
/// the limit of CPU time too, since a child has its own CPU time.
#[cfg(unix)]
pub fn limit_child(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    let Some(a) = active() else {
        return;
    };
    let cpu = a.sandbox.limits.cpu_seconds;
    if cpu.is_none() {
        return;
    }
    // SAFETY: the closure only calls setrlimit, which is async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            if let Some(seconds) = cpu {
                set_limit(RLIMIT_CPU, seconds)?;
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn limit_child(_: &mut std::process::Command) {}

#[cfg(unix)]
const RLIMIT_CPU: i32 = 0;
#[cfg(unix)]
const RLIMIT_FSIZE: i32 = 1;
#[cfg(unix)]
const RLIMIT_CORE: i32 = 4;

/// Sets a soft and hard resource limit; a limit can only be lowered.
#[cfg(unix)]
fn set_limit(resource: i32, value: u64) -> std::io::Result<()> {
    #[repr(C)]
    struct RLimit {
        current: u64,
        maximum: u64,
    }
    extern "C" {
        fn getrlimit(resource: i32, limit: *mut RLimit) -> i32;
        fn setrlimit(resource: i32, limit: *const RLimit) -> i32;
    }
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
            "allow_weaker = true\n[limits]\nheap_bytes = 1_000_000\nwall_seconds = 5\ncpu_seconds = 2\nprocesses = 0\nfile_size_bytes = 10\n",
            &p,
        )
        .unwrap();
        assert!(policy.allow_weaker);
        assert_eq!(policy.permissions, None);
        assert_eq!(
            policy.limits,
            Limits {
                heap_bytes: Some(1_000_000),
                wall_seconds: Some(5),
                cpu_seconds: Some(2),
                processes: Some(0),
                file_size_bytes: Some(10),
            }
        );
        assert!(parse_policy("[limits]\nheap = 5\n", &p).is_err());
        assert!(parse_policy("[limits]\nwall_seconds = 0\n", &p).is_err());
        assert!(parse_policy("[limits]\nheap_bytes = \"1 GiB\"\n", &p).is_err());
        assert!(parse_policy("allow_weaker = 1\n", &p).is_err());
        assert!(parse_policy("[limits\n", &p).is_err());
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
                        processes: Cell::new(0),
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
                ACTIVE.with(|a| *a.borrow_mut() = None);
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
}
