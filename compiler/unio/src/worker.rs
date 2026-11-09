//! The worker processes of the tools (requirements SB-10, SB-11, SB-14; docs/runtime.md 9.7).
//!
//! The tool (`unio check`, `fmt`, `build`, `outline`, `symbol`, `parse`, and the compile steps
//! of `unio test` and `unio run`) only handles its arguments, starts a worker with its scopes and
//! limits, and shows what the worker returns. The worker is the same executable, started with
//! the first argument [`ARG`], an empty environment except the variables that select the
//! language of messages, and its configuration on standard input. Before it reads anything of
//! the project it applies the rules of `unio_runtime::sandbox::worker`: it may read the
//! packages of the given files and the standard library, read and write its own temporary
//! directory, and nothing else; it has no network and cannot start processes. `unio fmt`
//! writes the files it was given through the writer process of the sandbox, which the worker
//! starts before its own rules apply and which may write exactly those files.
//!
//! The tool keeps the time limit and, on macOS, the memory limit: a worker over a limit is
//! stopped, and the tool reports it with code 102 instead of hanging.
//!
//! `unio build` links in a second worker, the link step: it may execute exactly the C compiler
//! and the programs it runs, read the toolchain, the object file, and `libunio.a`, and write
//! only the temporary directory of the build, without network. The tool then copies the
//! executable to the path the operator chose.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use unio_runtime::sandbox::{self, worker as os, Scope};

/// The first argument of a worker process.
pub const ARG: &str = "__unio-worker";
const VERSION: &str = "1";
/// The variables a worker receives, when they are set: those that select the language of
/// messages (SB-11). Nothing else of the environment of the tool reaches it.
pub const LANGUAGE_VARIABLES: &[&str] = &["UNIO_LANG", "LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES"];
/// The variable that makes a worker try operations after its rules apply and report each
/// result on standard error, so that the rules of the operating system can be seen to hold
/// without the compiler's own checks in the way (the SB-15 suite; docs/runtime.md 9.7).
pub const PROBE: &str = "UNIO_WORKER_PROBE";
/// The code of a worker stopped by a limit, as for programs (C-18).
pub const EXIT_LIMIT: u8 = 102;
/// The code of a worker whose sandbox cannot be applied (SB-5).
pub const EXIT_SANDBOX: u8 = 103;
/// The code of a worker that ended abnormally.
pub const EXIT_ABNORMAL: u8 = 101;

// ---------------------------------------------------------------- what a worker gets

/// The limits of a worker: the policy's, or the defaults of SB-11.
#[derive(Clone, Debug)]
pub struct Limits {
    pub memory_bytes: u64,
    pub wall_seconds: u64,
    pub cpu_seconds: Option<u64>,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            memory_bytes: os::DEFAULT_MEMORY_BYTES,
            wall_seconds: os::DEFAULT_WALL_SECONDS,
            cpu_seconds: None,
        }
    }
}

/// What the tool gives one worker.
#[derive(Clone, Debug, Default)]
pub struct Spec {
    /// The packages of the given files and the standard library.
    pub read: Vec<Scope>,
    /// Single files besides, such as `libunio.a` for the link step.
    pub read_files: Vec<PathBuf>,
    /// The files `unio fmt` may replace, through the writer process.
    pub formats: Vec<PathBuf>,
    pub limits: Limits,
    pub allow_weaker: bool,
    pub stdlib: Option<PathBuf>,
}

/// The directory with `unio.toml` nearest above the path, by its written path and without
/// following a link at the end: a source that is a link to a file elsewhere stays in the
/// package it is listed in, and the operating system then refuses to read its target. Without
/// a manifest, the directory of the file is its package, as in the compiler.
fn package_of(path: &Path) -> PathBuf {
    let absolute = sandbox::absolute(path);
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    let dir = lexical.parent().map(Path::to_path_buf).unwrap_or(lexical);
    dir.ancestors()
        .find(|a| a.join("unio.toml").is_file())
        .map(Path::to_path_buf)
        .unwrap_or(dir)
}

fn tree(path: &Path) -> Scope {
    Scope {
        root: sandbox::resolve(path),
        subtree: true,
    }
}

impl Spec {
    /// The scopes of a worker that reads the given files: their packages and the standard
    /// library (SB-11). There are no locked dependencies to read yet: release 0.1 installs
    /// none (T-14), so the package cache is not a scope.
    pub fn for_files(files: &[PathBuf], options: &unio::sandbox_cli::Options) -> Result<Spec, u8> {
        let (limits, allow_weaker) = limits(options)?;
        let mut read: Vec<Scope> = Vec::new();
        for file in files {
            let scope = tree(&package_of(file));
            if !read.contains(&scope) {
                read.push(scope);
            }
        }
        let stdlib = unio::program::trusted_stdlib();
        if let Some(dir) = &stdlib {
            let scope = tree(dir);
            if !read.contains(&scope) {
                read.push(scope);
            }
        }
        Ok(Spec {
            read,
            limits,
            allow_weaker,
            stdlib,
            ..Spec::default()
        })
    }

    /// The scopes of a worker that reads the package around the working directory, as
    /// `unio symbol` does.
    pub fn for_directory(dir: &Path, options: &unio::sandbox_cli::Options) -> Result<Spec, u8> {
        Spec::for_files(&[dir.join("unio.toml")], options)
    }
}

/// The limits of the policy (`--policy` or `UNIO_POLICY`, SB-3a) for a worker, or the defaults
/// of SB-11; the permissions of a policy do not apply to the tools, whose scopes are fixed.
fn limits(options: &unio::sandbox_cli::Options) -> Result<(Limits, bool), u8> {
    let policy = unio::sandbox_cli::read_policy(options).map_err(|message| {
        eprintln!("unio: {message}");
        EXIT_SANDBOX
    })?;
    let defaults = Limits::default();
    Ok((
        Limits {
            memory_bytes: policy.limits.heap_bytes.unwrap_or(defaults.memory_bytes),
            wall_seconds: policy.limits.wall_seconds.unwrap_or(defaults.wall_seconds),
            cpu_seconds: policy.limits.cpu_seconds,
        },
        options.allow_weaker || policy.allow_weaker,
    ))
}

// ---------------------------------------------------------------- the temporary directory

/// The temporary directory of a worker, removed when the tool is done with it.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Result<TempDir, String> {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir();
        for _ in 0..16 {
            let n = COUNT.fetch_add(1, Ordering::SeqCst);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let dir = base.join(format!(
                "unio-worker-{}-{n}-{nanos:08x}",
                std::process::id()
            ));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&dir) {
                Ok(()) => return Ok(TempDir(sandbox::resolve(&dir))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("the temporary directory {}: {e}", dir.display())),
            }
        }
        Err("no temporary directory for the worker".into())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------- the configuration

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn path_hex(path: &Path) -> String {
    hex(path.as_os_str().as_encoded_bytes())
}

fn hex_path(text: &str) -> Option<PathBuf> {
    let bytes = unhex(text)?;
    // SAFETY: the bytes come from `as_encoded_bytes` of the tool, the same executable on the
    // same platform.
    Some(PathBuf::from(unsafe {
        std::ffi::OsString::from_encoded_bytes_unchecked(bytes)
    }))
}

/// The configuration a worker reads on standard input before anything else.
#[derive(Debug, Default)]
struct Config {
    parent: u32,
    temp: PathBuf,
    stdlib: Option<PathBuf>,
    read: Vec<Scope>,
    read_files: Vec<PathBuf>,
    formats: Vec<PathBuf>,
    memory: Option<u64>,
    cpu: Option<u64>,
    allow_weaker: bool,
    probe: Option<String>,
}

fn encode(spec: &Spec, temp: &Path) -> String {
    let mut out = format!(
        "unio-worker {VERSION}\nparent {}\ntemp {}\n",
        std::process::id(),
        path_hex(temp)
    );
    match &spec.stdlib {
        Some(dir) => out.push_str(&format!("stdlib {}\n", path_hex(dir))),
        None => out.push_str("stdlib -\n"),
    }
    for scope in &spec.read {
        out.push_str(&format!(
            "read {} {}\n",
            u8::from(scope.subtree),
            path_hex(&scope.root)
        ));
    }
    for file in &spec.read_files {
        out.push_str(&format!("read-file {}\n", path_hex(file)));
    }
    for file in &spec.formats {
        out.push_str(&format!("format {}\n", path_hex(file)));
    }
    out.push_str(&format!("memory {}\n", spec.limits.memory_bytes));
    if let Some(cpu) = spec.limits.cpu_seconds {
        out.push_str(&format!("cpu {cpu}\n"));
    }
    out.push_str(&format!("weaker {}\n", u8::from(spec.allow_weaker)));
    if let Some(probe) = std::env::var_os(PROBE) {
        out.push_str(&format!("probe {}\n", hex(probe.as_encoded_bytes())));
    }
    out.push_str("end\n");
    out
}

fn decode(text: &str) -> Result<Config, String> {
    let mut lines = text.lines();
    if lines.next() != Some(&format!("unio-worker {VERSION}")) {
        return Err("started with another version of the configuration".into());
    }
    let mut config = Config::default();
    let bad = |line: &str| format!("a malformed line of the configuration: {line}");
    for line in lines {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "parent" => config.parent = value.parse().map_err(|_| bad(line))?,
            "temp" => config.temp = hex_path(value).ok_or_else(|| bad(line))?,
            "stdlib" if value == "-" => config.stdlib = None,
            "stdlib" => config.stdlib = Some(hex_path(value).ok_or_else(|| bad(line))?),
            "read" => {
                let (subtree, path) = value.split_once(' ').ok_or_else(|| bad(line))?;
                config.read.push(Scope {
                    root: hex_path(path).ok_or_else(|| bad(line))?,
                    subtree: subtree == "1",
                });
            }
            "read-file" => config
                .read_files
                .push(hex_path(value).ok_or_else(|| bad(line))?),
            "format" => config
                .formats
                .push(hex_path(value).ok_or_else(|| bad(line))?),
            "memory" => config.memory = Some(value.parse().map_err(|_| bad(line))?),
            "cpu" => config.cpu = Some(value.parse().map_err(|_| bad(line))?),
            "weaker" => config.allow_weaker = value == "1",
            "probe" => {
                let bytes = unhex(value).ok_or_else(|| bad(line))?;
                config.probe = Some(String::from_utf8_lossy(&bytes).into_owned());
            }
            "end" => return Ok(config),
            _ => return Err(bad(line)),
        }
    }
    Err("the configuration has no end".into())
}

// ---------------------------------------------------------------- inside the worker

static INSIDE: OnceLock<Inside> = OnceLock::new();

struct Inside {
    temp: PathBuf,
    #[cfg(unix)]
    writer: Option<sandbox::writer::Handle>,
}

/// Whether this process is a worker.
pub fn inside() -> bool {
    INSIDE.get().is_some()
}

/// The temporary directory of this worker.
pub fn temp_dir() -> Option<&'static Path> {
    INSIDE.get().map(|i| i.temp.as_path())
}

/// Replaces a file `unio fmt` was given: through the writer process on Linux and macOS, which
/// may write exactly the given files; directly elsewhere.
pub fn replace_file(path: &Path, contents: &str) -> Result<(), String> {
    #[cfg(unix)]
    if let Some(writer) = INSIDE.get().and_then(|i| i.writer.as_ref()) {
        let target = sandbox::writer::target_path(&path.to_string_lossy())
            .ok_or_else(|| "not a file name".to_string())?;
        let (id, _, mut file) = writer.open(&target).map_err(|f| f.to_string())?;
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        return writer.commit(id, &target).map_err(|f| f.to_string());
    }
    std::fs::write(path, contents).map_err(|e| e.to_string())
}

/// What `main` does after [`enter`].
pub enum Entered {
    /// A worker of a tool: run the command of these arguments, inside the sandbox.
    Tool(Vec<String>),
    /// The worker is done, or could not start.
    Done(ExitCode),
}

/// Starts this process as a worker: `arguments` follow [`ARG`]: the role and the arguments of
/// the command.
pub fn enter(arguments: &[String]) -> Entered {
    let fail = |message: &str| {
        eprintln!("unio: the worker: {message}");
        Entered::Done(ExitCode::from(EXIT_SANDBOX))
    };
    let Some((role, rest)) = arguments.split_first() else {
        return fail("started without a role");
    };
    let mut text = String::new();
    if std::io::stdin()
        .lock()
        .take(1 << 20)
        .read_to_string(&mut text)
        .is_err()
    {
        return fail("the configuration cannot be read");
    }
    let config = match decode(&text) {
        Ok(config) => config,
        Err(problem) => return fail(&problem),
    };
    if !os::end_with_parent(config.parent) {
        return fail("its tool has already ended");
    }
    match role.as_str() {
        "tool" => enter_tool(config, rest),
        "link" => Entered::Done(link_step(&config, rest)),
        _ => fail("started with an unknown role"),
    }
}

fn os_limits(config: &Config) -> os::Limits {
    os::Limits {
        memory_bytes: config.memory,
        cpu_seconds: config.cpu,
        file_size_bytes: config.memory,
    }
}

/// Applies the rules, or reports what is missing: a missing mechanism stops the worker with
/// code 103, before it reads anything of the project, unless the operator allowed a weaker
/// sandbox with --allow-weaker-sandbox or allow_weaker in the external policy (SB-5, SB-10);
/// a project cannot allow it through its manifest. On Windows the restrictions of files and
/// the network are missing, so the tools need that permission there; the Job Object (memory,
/// processes) and the time limit still apply (docs/runtime.md 9.7).
fn apply(rules: &os::Rules, allow_weaker: bool) -> Result<(), ExitCode> {
    let missing = os::missing();
    if !missing.is_empty() {
        if allow_weaker {
            eprintln!(
                "unio: warning: the operator allowed a weaker sandbox for the worker; missing: {}",
                missing.join("; ")
            );
        } else {
            eprintln!(
                "unio: the sandbox of the worker cannot be applied on this system; missing: {}. \
                 The operator can allow a weaker sandbox with --allow-weaker-sandbox or \
                 allow_weaker = true in the policy (SB-5, SB-10)",
                missing.join("; ")
            );
            return Err(ExitCode::from(EXIT_SANDBOX));
        }
    }
    if let Err(problem) = os::apply(rules) {
        eprintln!("unio: the sandbox of the worker cannot be applied: {problem}");
        return Err(ExitCode::from(EXIT_SANDBOX));
    }
    Ok(())
}

fn enter_tool(config: Config, rest: &[String]) -> Entered {
    unio::vfs::use_stdlib(config.stdlib.clone());
    // The writer starts before the rules apply, so that it does not inherit them; it applies
    // its own, which permit exactly the given files (docs/runtime.md 9.2).
    #[cfg(unix)]
    let writer = if config.formats.is_empty() {
        None
    } else {
        let scopes: Vec<Scope> = config
            .formats
            .iter()
            .map(|f| Scope {
                root: f.clone(),
                subtree: false,
            })
            .collect();
        match sandbox::writer::Handle::start(&scopes) {
            Ok(handle) => Some(handle),
            Err(problem) => {
                eprintln!("unio: the worker: {problem}");
                return Entered::Done(ExitCode::from(EXIT_SANDBOX));
            }
        }
    };
    let mut read = config.read.clone();
    for file in &config.read_files {
        read.push(Scope {
            root: file.clone(),
            subtree: false,
        });
    }
    let rules = os::Rules {
        read,
        write: vec![Scope {
            root: config.temp.clone(),
            subtree: true,
        }],
        execute: Vec::new(),
        processes: false,
        limits: os_limits(&config),
    };
    if let Err(code) = apply(&rules, config.allow_weaker) {
        return Entered::Done(code);
    }
    let _ = INSIDE.set(Inside {
        temp: config.temp.clone(),
        #[cfg(unix)]
        writer,
    });
    if let Some(probe) = &config.probe {
        probe::run(probe);
    }
    Entered::Tool(rest.to_vec())
}

// ---------------------------------------------------------------- the link step

/// The link step of `unio build`: applies its rules and runs the C compiler with `arguments`
/// (the compiler first). It may execute exactly the compiler and the programs it starts to link
/// (`collect2`, `ld`, as the compiler names them), read the library directories, the files
/// those programs need, the given files and its temporary directory, write only that directory,
/// and open no socket (docs/runtime.md 9.7).
fn link_step(config: &Config, arguments: &[String]) -> ExitCode {
    let Some((cc, args)) = arguments.split_first() else {
        eprintln!("unio: the link step: no compiler");
        return ExitCode::from(EXIT_SANDBOX);
    };
    let mut programs = std::collections::BTreeSet::new();
    let mut own_dirs: Vec<Scope> = Vec::new();
    programs.insert(cc.clone());
    // `link.exe` of Windows runs no programs to link; a C compiler names those it runs.
    let tools: &[&str] = if cfg!(windows) {
        &[]
    } else {
        &["collect2", "ld"]
    };
    for tool in tools {
        if let Ok(out) = Command::new(cc)
            .arg(format!("-print-prog-name={tool}"))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !name.is_empty() {
                // The directory of the compiler's own programs also holds the plugins the
                // linker loads (`liblto_plugin.so`): it may be read, not executed.
                let path = Path::new(&name);
                if path.is_absolute() {
                    if let Some(dir) = path.parent() {
                        own_dirs.push(tree(dir));
                    }
                }
                programs.insert(name);
            }
        }
    }
    let toolchain = os::toolchain(&programs);
    let mut read = toolchain.read;
    read.extend(own_dirs);
    read.extend(config.read.iter().cloned());
    for file in &config.read_files {
        read.push(Scope {
            root: file.clone(),
            subtree: false,
        });
    }
    let rules = os::Rules {
        read,
        write: vec![Scope {
            root: config.temp.clone(),
            subtree: true,
        }],
        execute: toolchain.execute,
        processes: true,
        limits: os_limits(config),
    };
    if let Err(code) = apply(&rules, config.allow_weaker) {
        return code;
    }
    // The linker's own messages go to standard error: link.exe writes its errors to standard
    // output, which a tool may read as data.
    match Command::new(cc).args(args).stdin(Stdio::null()).output() {
        Ok(out) if out.status.success() => ExitCode::SUCCESS,
        Ok(out) => {
            eprintln!(
                "unio build: the linker {cc} failed ({}):\n{}{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("unio build: the C compiler {cc} cannot be run: {e}");
            ExitCode::from(1)
        }
    }
}

// ---------------------------------------------------------------- the tool's side

/// What a worker returned.
pub struct Finished {
    /// The exit code of the worker.
    pub code: u8,
    /// Its standard output, when the tool asked for it as data.
    pub data: Vec<u8>,
}

/// Runs a worker of a tool for the command of `arguments` and relays its output; the exit code
/// is the worker's, or 102 and a diagnostic when it was stopped by a limit.
pub fn delegate(arguments: &[String], spec: &Spec) -> ExitCode {
    let temp = match TempDir::new() {
        Ok(temp) => temp,
        Err(problem) => {
            eprintln!("unio: {problem}");
            return ExitCode::from(EXIT_SANDBOX);
        }
    };
    match start("tool", arguments, spec, &temp, false, &[]) {
        Ok(finished) => ExitCode::from(finished.code),
        Err(code) => ExitCode::from(code),
    }
}

/// Starts a worker in `role` with the arguments and waits for it within its limits. With
/// `capture`, its standard output is returned as data instead of being shown.
pub fn start(
    role: &str,
    arguments: &[String],
    spec: &Spec,
    temp: &TempDir,
    capture: bool,
    extra_env: &[(&str, OsString)],
) -> Result<Finished, u8> {
    #[cfg(target_os = "linux")]
    let exe = PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("unio: the executable of the worker: {e}");
            return Err(EXIT_SANDBOX);
        }
    };
    let mut command = Command::new(exe);
    command
        .arg(ARG)
        .arg(role)
        .args(arguments)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in LANGUAGE_VARIABLES {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    // System libraries on Windows do not load without it.
    #[cfg(windows)]
    if let Some(value) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", value);
    }
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("unio: the worker cannot start: {e}");
            return Err(EXIT_SANDBOX);
        }
    };
    let config = encode(spec, temp.path());
    if let Some(mut stdin) = child.stdin.take() {
        // A worker that ends early closes the pipe; its code tells why.
        let _ = stdin.write_all(config.as_bytes());
    }
    let mut stdout = child.stdout.take().expect("a piped standard output");
    let mut stderr = child.stderr.take().expect("a piped standard error");
    let limit = spec.limits.memory_bytes;
    let out_thread = std::thread::spawn(move || {
        let mut data = Vec::new();
        if capture {
            // The data of a worker is bounded by its memory limit.
            let _ = (&mut stdout).take(limit).read_to_end(&mut data);
        } else {
            let mut buffer = [0u8; 8192];
            let out = std::io::stdout();
            while let Ok(n) = stdout.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let mut out = out.lock();
                let _ = out.write_all(&buffer[..n]);
                let _ = out.flush();
            }
        }
        data
    });
    let err_thread = std::thread::spawn(move || {
        // The last bytes, to tell an exhausted worker from a crashed one.
        let mut tail: Vec<u8> = Vec::new();
        let mut buffer = [0u8; 8192];
        let err = std::io::stderr();
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            let _ = err.lock().write_all(&buffer[..n]);
            tail.extend_from_slice(&buffer[..n]);
            if tail.len() > 4096 {
                tail.drain(..tail.len() - 4096);
            }
        }
        tail
    });
    let started = Instant::now();
    let deadline = Duration::from_secs(spec.limits.wall_seconds);
    let mut pause = Duration::from_micros(200);
    let mut stopped: Option<String> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if started.elapsed() >= deadline {
            stopped = Some(format!(
                "it ran longer than the time limit of {} s",
                spec.limits.wall_seconds
            ));
        } else if os::footprint(child.id()).is_some_and(|bytes| bytes > limit) {
            stopped = Some(format!(
                "it used more than the memory limit of {limit} bytes"
            ));
        }
        if stopped.is_some() {
            let _ = child.kill();
            break child.wait().ok();
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(10));
    };
    let data = out_thread.join().unwrap_or_default();
    let tail = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
    if stopped.is_none() && status.is_some_and(|s| s.code().is_none() || !s.success()) {
        let exhausted = [
            "memory allocation of",
            "failed to spawn thread",
            "out of memory",
            "Cannot allocate memory",
            "capacity overflow",
        ];
        if exhausted.iter().any(|m| tail.contains(m)) {
            stopped = Some(format!(
                "it used more than the memory limit of {limit} bytes"
            ));
        }
    }
    if let Some(why) = stopped {
        eprintln!(
            "unio: error: the worker of the tool was stopped: {why} (SB-11; the limits come from \
             limits.heap_bytes and limits.wall_seconds of the policy)"
        );
        return Err(EXIT_LIMIT);
    }
    match status.and_then(|s| s.code()) {
        Some(code) => Ok(Finished {
            code: u8::try_from(code.clamp(0, 255)).unwrap_or(EXIT_ABNORMAL),
            data,
        }),
        None => {
            eprintln!(
                "unio: error: the worker of the tool ended abnormally ({})",
                status.map_or_else(|| "unknown".to_string(), |s| s.to_string())
            );
            Err(EXIT_ABNORMAL)
        }
    }
}

/// Links in the link step: runs the linker with its arguments in its sandbox, with `PATH`, the
/// temporary directory, and the variables the linker needs (on Windows those that locate the
/// libraries of Visual Studio and the Windows SDK).
pub fn link(linker: &unio::native::Linker, spec: &Spec, temp: &TempDir) -> Result<Finished, u8> {
    let mut arguments = vec![linker.program.clone()];
    arguments.extend(linker.args.iter().map(|a| a.to_string_lossy().into_owned()));
    let dir = temp.path().as_os_str().to_owned();
    let mut env: Vec<(&str, OsString)> = vec![("TMPDIR", dir.clone())];
    if cfg!(windows) {
        env.push(("TMP", dir.clone()));
        env.push(("TEMP", dir));
    }
    let own: Vec<(String, OsString)> = linker
        .env
        .iter()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.clone()))
        .collect();
    if !own.iter().any(|(k, _)| k.eq_ignore_ascii_case("PATH")) {
        if let Some(path) = std::env::var_os("PATH") {
            env.push(("PATH", path));
        }
    }
    env.extend(own.iter().map(|(k, v)| (k.as_str(), v.clone())));
    start("link", &arguments, spec, temp, false, &env)
}

// ---------------------------------------------------------------- data from a worker

/// Length-prefixed data that a worker returns on standard output.
#[derive(Default)]
pub struct Enc(pub Vec<u8>);

impl Enc {
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    pub fn text(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    pub fn path(&mut self, p: &Path) -> &mut Self {
        self.bytes(p.as_os_str().as_encoded_bytes())
    }
}

pub struct Dec<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Dec<'a> {
    pub fn new(data: &'a [u8]) -> Dec<'a> {
        Dec { data, at: 0 }
    }
    pub fn u32(&mut self) -> Option<u32> {
        let end = self.at.checked_add(4)?;
        let v = u32::from_le_bytes(self.data.get(self.at..end)?.try_into().ok()?);
        self.at = end;
        Some(v)
    }
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        let end = self.at.checked_add(n)?;
        let v = self.data.get(self.at..end)?;
        self.at = end;
        Some(v)
    }
    pub fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }
    pub fn path(&mut self) -> Option<PathBuf> {
        let bytes = self.bytes()?.to_vec();
        // SAFETY: the bytes come from `as_encoded_bytes` of a worker, the same executable on
        // the same platform.
        Some(PathBuf::from(unsafe {
            OsString::from_encoded_bytes_unchecked(bytes)
        }))
    }
    pub fn end(&self) -> bool {
        self.at == self.data.len()
    }
}

// ---------------------------------------------------------------- probes

/// The probes of [`PROBE`]: operations separated by `;`, each `name` or `name=argument`. The
/// worker tries each one after its rules apply, with the plain calls of the standard library of
/// Rust and of the system, not through the compiler or the runtime of Unio, and reports
/// `unio worker probe: <operation>: allowed` or `... denied (<error>)` on standard error.
mod probe {
    use std::io::Write;

    fn report(operation: &str, result: Result<String, String>) {
        let line = match result {
            Ok(detail) if detail.is_empty() => format!("unio worker probe: {operation}: allowed"),
            Ok(detail) => format!("unio worker probe: {operation}: allowed {detail}"),
            Err(e) => format!("unio worker probe: {operation}: denied ({e})"),
        };
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{line}");
        let _ = err.flush();
    }

    pub fn run(probes: &str) {
        for operation in probes.split(';').filter(|p| !p.is_empty()) {
            let (name, argument) = operation.split_once('=').unwrap_or((operation, ""));
            let result: Result<String, String> = match name {
                "read" => std::fs::read(argument)
                    .map(|b| format!("{} bytes", b.len()))
                    .map_err(|e| e.to_string()),
                "list" => std::fs::read_dir(argument)
                    .map(|d| format!("{} entries", d.count()))
                    .map_err(|e| e.to_string()),
                "write" => std::fs::write(argument, "probe")
                    .map(|()| String::new())
                    .map_err(|e| e.to_string()),
                "mkdir" => std::fs::create_dir(argument)
                    .map(|()| String::new())
                    .map_err(|e| e.to_string()),
                "remove" => std::fs::remove_file(argument)
                    .map(|()| String::new())
                    .map_err(|e| e.to_string()),
                "write-temp" => match super::temp_dir() {
                    Some(dir) => std::fs::write(dir.join("probe.txt"), "probe")
                        .map(|()| String::new())
                        .map_err(|e| e.to_string()),
                    None => Err("no temporary directory".into()),
                },
                "exec" => std::process::Command::new(argument)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.to_string())
                    .map_err(|e| e.to_string()),
                "fork" => fork(),
                "connect" => std::net::TcpStream::connect(argument)
                    .map(|_| String::new())
                    .map_err(|e| e.to_string()),
                "udp" => std::net::UdpSocket::bind("127.0.0.1:0")
                    .map(|_| String::new())
                    .map_err(|e| e.to_string()),
                "env" => {
                    let names: Vec<String> = std::env::vars_os()
                        .map(|(k, _)| k.to_string_lossy().into_owned())
                        .collect();
                    Ok(format!("[{}]", names.join(",")))
                }
                "pid" => Ok(std::process::id().to_string()),
                "sleep" => {
                    let seconds: u64 = argument.parse().unwrap_or(1);
                    std::thread::sleep(std::time::Duration::from_secs(seconds));
                    Ok(String::new())
                }
                "spin" => {
                    report(operation, Ok("spinning".into()));
                    #[allow(clippy::empty_loop)]
                    loop {
                        std::hint::spin_loop();
                    }
                }
                "alloc" => {
                    let bytes: usize = argument.parse().unwrap_or(0);
                    report(operation, Ok("allocating".into()));
                    // Touched, so that the pages count wherever the memory is measured.
                    let block = vec![1u8; bytes];
                    Ok(format!("{} bytes", std::hint::black_box(block).len()))
                }
                _ => Err("unknown probe".into()),
            };
            report(operation, result);
        }
    }

    #[cfg(unix)]
    fn fork() -> Result<String, String> {
        extern "C" {
            fn fork() -> i32;
            fn _exit(code: i32) -> !;
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        }
        // SAFETY: the child only calls _exit; the parent reaps it.
        unsafe {
            let pid = fork();
            if pid == 0 {
                _exit(0);
            }
            if pid < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut status = 0;
            waitpid(pid, &mut status, 0);
            Ok(String::new())
        }
    }

    #[cfg(not(unix))]
    fn fork() -> Result<String, String> {
        Err("no fork on this system".into())
    }
}
