//! The `unio` command (front-end subset): check, fmt, outline, symbol, parse --ast, explain.
//!
//! The commands that read a project run their work in a worker process with the sandbox of
//! SB-11 (`worker.rs`, docs/runtime.md 9.7); this process handles the arguments, starts the
//! worker, and shows what it returns.

// The language server: the protocol here, its analyses in `unio::lsp` (T-10, SB-13).
#[path = "lsp/server.rs"]
mod lsp_server;
mod packages;
mod worker;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use unio::Lang;

use unio::sandbox_cli;

const USAGE: &str = "Usage:
    unio check [--diagnostics=json] [--lang=en|ru] [--profile=application|core] FILE...
    unio fmt [--check] FILE...
    unio run [--policy FILE] [--allow-weaker-sandbox] FILE [-- ARGS...]
    unio build [--release] [--policy FILE] [--allow-weaker-sandbox] FILE [-o OUT]
    unio build --print-toolchain
    unio test [--json] [--policy FILE] [--allow-weaker-sandbox] [FILTER] FILE...
    unio outline [--json] FILE
    unio symbol [--json] MODULE.NAME
    unio parse --ast FILE
    unio explain CODE
    unio lsp [--policy FILE] [--allow-weaker-sandbox] [--lang=en|ru]
    unio add ALIAS [VERSION] (--path DIR | --package OWNER/NAME [--registry NAME] |
             --archive FILE|URL --sha384 HEX) [--unsafe-ok] [--offline]
    unio remove ALIAS
    unio update [ALIAS...] [--unsafe-ok] [--offline]
    unio install [--offline] [--unsafe-ok]

The language of messages is taken from --lang, then from UNIO_LANG, and is English otherwise.
Every command that reads sources takes --policy FILE (or UNIO_POLICY), whose [compile] table
limits its worker, and --allow-weaker-sandbox (docs/runtime.md 9.7).";

/// Finds `link.exe` of Visual Studio as rustc does (compiler/README.md "The linker on Windows").
#[cfg(windows)]
fn find_msvc_linker() -> Option<unio::native::MsvcLinker> {
    let tool = find_msvc_tools::find_tool("x86_64-pc-windows-msvc", "link.exe")?;
    Some(unio::native::MsvcLinker {
        path: tool.path().to_path_buf(),
        env: tool.env().into_iter().cloned().collect(),
    })
}

fn main() -> ExitCode {
    #[cfg(windows)]
    unio::native::set_msvc_finder(find_msvc_linker);
    // Started as the writer process of a sandbox, this serves and ends here (docs/runtime.md 9.2).
    #[cfg(unix)]
    unio_runtime::sandbox::writer::serve_if_requested();
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    // Started as a worker of a tool: apply its sandbox, then run the command (SB-11).
    if arguments.first().map(String::as_str) == Some(worker::ARG) {
        match worker::enter(&arguments[1..]) {
            worker::Entered::Tool(rest) => arguments = rest,
            worker::Entered::Done(code) => return code,
        }
    }
    if arguments.first().map(String::as_str) == Some("run") {
        return run(&arguments[1..]);
    }
    if arguments.first().map(String::as_str) == Some("build") {
        return build(&arguments[1..]);
    }
    if arguments.first().map(String::as_str) == Some("lsp") {
        return lsp_server::main(&arguments[1..]);
    }
    if let Some(command @ ("add" | "remove" | "update" | "install")) =
        arguments.first().map(String::as_str)
    {
        return packages::command(command, &arguments[1..]);
    }
    let (sandbox, arguments) = match sandbox_cli::take(&arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    let mut lang = std::env::var("UNIO_LANG")
        .ok()
        .and_then(|v| Lang::parse(&v))
        .unwrap_or(Lang::En);
    let mut json = false;
    let mut ast = false;
    let mut check_only = false;
    let mut profile = unio::effects::Profile::Application;
    let mut positional = Vec::new();
    for argument in &arguments {
        if let Some(value) = argument.strip_prefix("--lang=") {
            match Lang::parse(value) {
                Some(l) => lang = l,
                None => return usage(&format!("unknown language '{value}'; use en or ru")),
            }
        } else if let Some(value) = argument.strip_prefix("--profile=") {
            profile = match value {
                "application" => unio::effects::Profile::Application,
                "core" => unio::effects::Profile::Core,
                _ => {
                    return usage(&format!(
                        "unknown profile '{value}'; use application or core"
                    ))
                }
            };
        } else if argument == "--diagnostics=json" {
            json = true;
        } else if argument == "--diagnostics=text" {
            json = false;
        } else if argument == "--ast" {
            ast = true;
        } else if argument == "--check" {
            check_only = true;
        } else if argument == "--json" {
            json = true;
        } else if argument == "-h" || argument == "--help" {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        } else if argument.starts_with("--") {
            return usage(&format!("unknown option '{argument}'"));
        } else {
            positional.push(argument.clone());
        }
    }
    let Some((command, files)) = positional.split_first() else {
        return usage("a command is required");
    };
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let for_files = || worker::Spec::for_files(&paths, &sandbox);
    match command.as_str() {
        "check" => isolated(&arguments, for_files, || check(files, json, lang, profile)),
        "symbol" if files.len() == 1 => isolated(
            &arguments,
            || worker::Spec::for_directory(unio_runtime::sandbox::working_directory(), &sandbox),
            || symbol(&files[0], json),
        ),
        "parse" if ast && files.len() == 1 => {
            isolated(&arguments, for_files, || parse(&files[0], lang))
        }
        "fmt" => isolated(
            &arguments,
            || {
                let mut spec = for_files()?;
                if !check_only {
                    spec.formats = format_targets(&paths);
                }
                Ok(spec)
            },
            || fmt(files, check_only, lang),
        ),
        "test" => test(&arguments, files, json, lang, &sandbox),
        "outline" if files.len() == 1 => {
            isolated(&arguments, for_files, || outline(&files[0], json, lang))
        }
        "explain" if files.len() == 1 => match unio::catalog::explain(&files[0], lang) {
            Some(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("unio: unknown code {}", files[0]);
                ExitCode::from(2)
            }
        },
        _ => usage(&format!(
            "unknown command or arguments: {}",
            positional.join(" ")
        )),
    }
}

/// Runs the work here when this process is the worker; otherwise starts a worker with the
/// scopes of `spec` for the same arguments and shows its results (SB-10).
fn isolated(
    arguments: &[String],
    spec: impl FnOnce() -> Result<worker::Spec, u8>,
    work: impl FnOnce() -> ExitCode,
) -> ExitCode {
    if worker::inside() {
        return work();
    }
    match spec() {
        Ok(spec) => worker::delegate(arguments, &spec),
        Err(code) => ExitCode::from(code),
    }
}

/// The files `unio fmt` may replace: each given file as an entry of its directory, so that a
/// link among them is replaced and never followed (docs/runtime.md 9.2).
fn format_targets(files: &[PathBuf]) -> Vec<PathBuf> {
    #[cfg(unix)]
    return files
        .iter()
        .filter_map(|f| unio_runtime::sandbox::writer::target_path(&f.to_string_lossy()))
        .collect();
    #[cfg(not(unix))]
    return files.to_vec();
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("unio: {problem}\n\n{USAGE}");
    ExitCode::from(2)
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("unio: {path}: {e}"))
}

/// `unio symbol`: what the compiler knows about one declaration (C-11).
fn symbol(name: &str, json: bool) -> ExitCode {
    let start = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let reports = unio::symbol(name, &start);
    if reports.is_empty() {
        eprintln!("unio: no declaration named {name}");
        return ExitCode::from(1);
    }
    if json {
        let items: Vec<String> = reports.iter().map(|r| r.json()).collect();
        println!("[{}]", items.join(","));
    } else {
        for report in reports {
            print!("{}", report.text());
        }
    }
    ExitCode::SUCCESS
}

fn check(files: &[String], json: bool, lang: Lang, profile: unio::effects::Profile) -> ExitCode {
    if files.is_empty() {
        return usage("check needs at least one file");
    }
    let mut failed = false;
    let mut all_json = Vec::new();
    for path in files {
        if let Err(message) = read(path) {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    }
    let paths: Vec<std::path::PathBuf> = files.iter().map(std::path::PathBuf::from).collect();
    for analysis in unio::check_files_with(&paths, profile) {
        failed |= analysis.has_errors();
        if json {
            all_json.push(analysis.render_json(lang));
        } else {
            print!("{}", analysis.render_text(lang));
        }
    }
    if json {
        // One array for all files.
        let items: Vec<&str> = all_json
            .iter()
            .map(|a| {
                a.trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
            })
            .filter(|a| !a.is_empty())
            .collect();
        if items.is_empty() {
            println!("[]");
        } else {
            println!("[\n  {}\n]", items.join(",\n  "));
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn parse(path: &str, lang: Lang) -> ExitCode {
    let source = match read(path) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let analysis = unio::check_source(path, &source);
    match analysis.ast_sexpr() {
        Some(tree) => {
            print!("{tree}");
            ExitCode::SUCCESS
        }
        None => {
            eprint!("{}", analysis.render_text(lang));
            ExitCode::from(1)
        }
    }
}

/// `unio fmt`: writes each file in the one style; with `--check`, lists the files that differ.
fn fmt(files: &[String], check_only: bool, lang: Lang) -> ExitCode {
    if files.is_empty() {
        return usage("fmt needs at least one file");
    }
    let mut failed = false;
    for path in files {
        let source = match read(path) {
            Ok(source) => source,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
        };
        match unio::format::format_source(&source) {
            Ok(formatted) if formatted == source => {}
            Ok(formatted) => {
                if check_only {
                    println!("{path}");
                    failed = true;
                } else if let Err(e) = worker::replace_file(Path::new(path), &formatted) {
                    eprintln!("unio: {path}: {e}");
                    return ExitCode::from(2);
                }
            }
            Err(_) => {
                let analysis = unio::check_source(path, &source);
                eprint!("{}", analysis.render_text(lang));
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// `unio outline`: the declarations of a file with their signatures (C-12).
fn outline(path: &str, json: bool, lang: Lang) -> ExitCode {
    let source = match read(path) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let analysis = unio::check_source(path, &source);
    if analysis.has_errors() {
        eprint!("{}", analysis.render_text(lang));
        return ExitCode::from(1);
    }
    if json {
        print!("{}", unio::outline::outline_json(&analysis.file));
    } else {
        print!("{}", unio::outline::outline_text(&analysis.file));
    }
    ExitCode::SUCCESS
}

/// The stack of the interpreter thread; deep Unio recursion ends with code 102 at the
/// runtime's frame limit long before this is used up.
const RUN_STACK: usize = 1024 * 1024 * 1024;

fn on_run_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    on_stack(RUN_STACK, f)
}

/// The stack of the front end in a worker, as the compiler's own (`lib.rs`): the address space
/// of a worker is limited (SB-11), so it does not reserve the stack of the interpreter.
const FRONT_END_STACK: usize = 64 * 1024 * 1024;

fn on_stack<T: Send + 'static>(size: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    let thread = match std::thread::Builder::new().stack_size(size).spawn(f) {
        Ok(thread) => thread,
        Err(e) => {
            // Under the address-space limit of a worker this is how memory runs out first.
            eprintln!("unio: memory allocation of a stack of {size} bytes failed: {e}");
            std::process::exit(i32::from(worker::EXIT_LIMIT));
        }
    };
    thread.join().expect("the interpreter thread")
}

// ---------------------------------------------------------------- compiled sources

/// What the compile step of `unio run` and `unio test` returns from the worker: every file the
/// program is loaded from, so that the run loads it again from memory inside the sandbox of
/// the program and never reads the project outside the worker (docs/runtime.md 9.7).
struct Sources {
    stdlib: Option<PathBuf>,
    files: Vec<(PathBuf, String)>,
}

impl Sources {
    /// The sources of a loaded program: its modules, the manifests that decide its packages,
    /// and the lock files of its root packages, which lead to its dependencies (T-14).
    fn of(program: &unio::program::Program) -> Sources {
        let mut files: Vec<(PathBuf, String)> = program
            .modules
            .iter()
            .map(|m| (m.path.clone(), m.analysis.source.clone()))
            .collect();
        for m in program.modules.iter().filter(|m| !m.stdlib) {
            let toml = m.package.join("unio.toml");
            if let Ok(text) = std::fs::read_to_string(&toml) {
                files.push((toml, text));
            }
            let lock = m.package.join(unio::package::lock::FILE);
            if m.dependency.is_none() {
                if let Ok(text) = std::fs::read_to_string(&lock) {
                    files.push((lock, text));
                }
            }
        }
        Sources {
            stdlib: program.stdlib_root().map(Path::to_path_buf),
            files,
        }
    }

    fn add(&mut self, other: Sources) {
        if self.stdlib.is_none() {
            self.stdlib = other.stdlib;
        }
        let known: std::collections::HashSet<PathBuf> =
            self.files.iter().map(|(p, _)| p.clone()).collect();
        self.files
            .extend(other.files.into_iter().filter(|(p, _)| !known.contains(p)));
    }

    fn encode(&self, out: &mut worker::Enc) {
        match &self.stdlib {
            Some(dir) => out.u32(1).path(dir),
            None => out.u32(0),
        };
        out.u32(self.files.len() as u32);
        for (path, text) in &self.files {
            out.path(path).text(text);
        }
    }

    fn decode(d: &mut worker::Dec) -> Option<Sources> {
        let stdlib = match d.u32()? {
            0 => None,
            _ => Some(d.path()?),
        };
        let n = d.u32()?;
        let mut files = Vec::new();
        for _ in 0..n {
            files.push((d.path()?, d.text()?));
        }
        Some(Sources { stdlib, files })
    }

    /// Makes these files the only ones programs are loaded from in this process.
    fn install(mut self) {
        self.files.sort();
        self.files.dedup_by(|a, b| a.0 == b.0);
        unio::vfs::install(self.stdlib, self.files);
    }
}

/// The main module of a loaded program: the first module that is reported.
fn main_path(program: &unio::program::Program) -> Option<PathBuf> {
    program
        .modules
        .iter()
        .find(|m| m.report)
        .map(|m| m.path.clone())
}

/// Writes the data of the worker to standard output, for the tool.
fn emit(out: &worker::Enc) -> ExitCode {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(&out.0)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

/// Runs the compile step of `arguments` in a worker and returns its data, or the exit code.
fn compile_in_worker(
    arguments: &[String],
    files: &[PathBuf],
    sandbox: &sandbox_cli::Options,
) -> Result<Vec<u8>, ExitCode> {
    // The compile limits of the policy hold here as for every worker (SB-11, RFC 0006); the
    // limits of the program apply only to the program.
    let spec = worker::Spec::for_files(files, sandbox).map_err(ExitCode::from)?;
    let temp = worker::TempDir::new().map_err(|problem| {
        eprintln!("unio: {problem}");
        ExitCode::from(worker::EXIT_SANDBOX)
    })?;
    let finished =
        worker::start("tool", arguments, &spec, &temp, true, &[]).map_err(ExitCode::from)?;
    if finished.code != 0 {
        return Err(ExitCode::from(finished.code));
    }
    Ok(finished.data)
}

fn malformed() -> ExitCode {
    eprintln!("unio: the worker returned malformed data");
    ExitCode::from(worker::EXIT_ABNORMAL)
}

// ---------------------------------------------------------------- build

/// `unio build [--release] FILE [-o OUT]`: one executable (C-5, docs/runtime.md 8.4). The worker
/// checks the program and writes its object file into its temporary directory; the link step
/// links it there; this process copies the executable to OUT, the path the operator chose
/// (docs/runtime.md 9.7). The identity of the linker and the SDK is computed here, before the
/// compile step, and the worker records it in the program image (C-4); `--print-toolchain`
/// prints it without building.
fn build(arguments: &[String]) -> ExitCode {
    let (sandbox, arguments) = match sandbox_cli::take(arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    let mut release = false;
    let mut out = None;
    let mut file = None;
    let mut print_toolchain = false;
    let mut toolchain = None;
    let mut i = 0;
    while i < arguments.len() {
        let a = &arguments[i];
        if a == "--release" {
            release = true;
        } else if a == "--print-toolchain" {
            print_toolchain = true;
        } else if let Some(t) = a.strip_prefix(TOOLCHAIN_ARG).filter(|_| worker::inside()) {
            // Given by the tool to its worker only.
            toolchain = Some(t.to_string());
        } else if a == "-o" {
            match arguments.get(i + 1) {
                Some(o) => out = Some(PathBuf::from(o)),
                None => return usage("-o needs a file"),
            }
            i += 1;
        } else if let Some(o) = a.strip_prefix("-o") {
            out = Some(PathBuf::from(o));
        } else if a.starts_with('-') {
            return usage(&format!("unknown option '{a}'"));
        } else if file.is_none() {
            file = Some(PathBuf::from(a));
        } else {
            return usage("unio build takes one file");
        }
        i += 1;
    }
    let fail = |m: String| {
        eprintln!("unio build: {m}");
        ExitCode::from(1)
    };
    if print_toolchain {
        if file.is_some() || out.is_some() || release {
            return usage("--print-toolchain takes no file and no other option");
        }
        return match unio::native::toolchain::identity() {
            Ok(identity) => {
                println!("{identity}");
                ExitCode::SUCCESS
            }
            Err(m) => fail(m),
        };
    }
    let Some(file) = file else {
        return usage("unio build needs a file");
    };
    if worker::inside() {
        let Some(toolchain) = toolchain else {
            return fail("the compile step was not given the identity of the toolchain".into());
        };
        return build_object(&file, release, &toolchain);
    }
    // The policy and its compile limits, which hold for the compile step and the link step
    // alike, are read first: a limit that cannot be honoured stops the build here (RFC 0006).
    let mut spec = match worker::Spec::for_files(std::slice::from_ref(&file), &sandbox) {
        Ok(spec) => spec,
        Err(code) => return ExitCode::from(code),
    };
    let Some(target) = unio::native::Target::host() else {
        return fail(unio::native::unsupported());
    };
    // The linker and the SDK of this build, recorded in its image; a missing linker stops here.
    let toolchain = match unio::native::toolchain::identity() {
        Ok(identity) => identity,
        Err(m) => return fail(m),
    };
    let out = out.unwrap_or_else(|| {
        let mut name = file.file_stem().unwrap_or_default().to_os_string();
        name.push(target.executable_suffix());
        PathBuf::from(name)
    });
    let runtime = match unio::native::runtime_library() {
        Ok(runtime) => runtime,
        Err(m) => return fail(m),
    };
    let temp = match worker::TempDir::new() {
        Ok(temp) => temp,
        Err(m) => return fail(m),
    };
    let mut compile = vec!["build".to_string()];
    if release {
        compile.push("--release".into());
    }
    compile.push(format!("{TOOLCHAIN_ARG}{toolchain}"));
    compile.push(file.to_string_lossy().into_owned());
    match worker::start("tool", &compile, &spec, &temp, false, &[]) {
        Ok(finished) if finished.code == 0 => {}
        Ok(finished) => return ExitCode::from(finished.code),
        Err(code) => return ExitCode::from(code),
    }
    let object = temp.path().join(unio::native::object_file_name());
    let executable = temp.path().join(unio::native::executable_file_name());
    let linker = match unio::native::link_command(&object, &runtime, &executable) {
        Ok(linker) => linker,
        Err(m) => return fail(m),
    };
    spec.read = linker
        .read
        .iter()
        .map(|dir| unio_runtime::sandbox::Scope {
            root: unio_runtime::sandbox::resolve(dir),
            subtree: true,
        })
        .collect();
    spec.read_files = vec![unio_runtime::sandbox::resolve(&runtime)];
    spec.stdlib = None;
    match worker::link(&linker, &spec, &temp) {
        Ok(finished) if finished.code == 0 => {}
        Ok(_) => return fail("linking failed".into()),
        Err(code) => return ExitCode::from(code),
    }
    match place(&executable, &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("{}: {e}", out.display())),
    }
}

/// How the tool hands the identity of the toolchain to the compile step in its worker.
const TOOLCHAIN_ARG: &str = "--toolchain=";

/// The compile step of `unio build` in the worker: the object file in its temporary directory.
fn build_object(file: &Path, release: bool, toolchain: &str) -> ExitCode {
    let Some(dir) = worker::temp_dir() else {
        return ExitCode::from(worker::EXIT_SANDBOX);
    };
    match unio::native::compile_object(file, release, toolchain) {
        Ok(object) => match std::fs::write(dir.join(unio::native::object_file_name()), object) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("unio build: the object file: {e}");
                ExitCode::from(1)
            }
        },
        Err(unio::native::BuildError::Program(analyses)) => {
            for analysis in analyses.iter().filter(|a| a.has_errors()) {
                eprint!("{}", analysis.render_text(Lang::En));
            }
            ExitCode::from(1)
        }
        Err(unio::native::BuildError::Message(m)) => {
            eprintln!("unio build: {m}");
            ExitCode::from(1)
        }
    }
}

/// Copies the executable to the path the operator chose, replacing it in one step.
fn place(from: &Path, to: &Path) -> std::io::Result<()> {
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = to.with_file_name(format!(".{name}.unio-tmp-{}", std::process::id()));
    std::fs::copy(from, &temporary)?;
    std::fs::rename(&temporary, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

// ---------------------------------------------------------------- run

fn run(arguments: &[String]) -> ExitCode {
    let (sandbox, arguments) = match sandbox_cli::take_run(arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    let (file, rest) = match arguments.split_first() {
        Some((file, rest)) => (PathBuf::from(file), rest),
        None => return usage("unio run needs a file"),
    };
    if worker::inside() {
        return run_compile(file);
    }
    let program_args: Vec<String> = match rest.first().map(String::as_str) {
        Some("--") => rest[1..].to_vec(),
        Some(_) => rest.to_vec(),
        None => Vec::new(),
    };
    let plan = match sandbox_cli::prepare(std::slice::from_ref(&file), &sandbox, false) {
        Ok(plan) => plan,
        Err(code) => return ExitCode::from(code as u8),
    };
    // The worker loads and checks the program once: a file with errors is reported there and
    // not run.
    let compile = ["run".to_string(), file.to_string_lossy().into_owned()];
    let data = match compile_in_worker(&compile, std::slice::from_ref(&file), &sandbox) {
        Ok(data) => data,
        Err(code) => return code,
    };
    let mut d = worker::Dec::new(&data);
    let (Some(main), Some(sources)) = (d.path(), Sources::decode(&mut d)) else {
        return malformed();
    };
    sources.install();
    let code = on_run_stack(move || {
        // SB-1: the sandbox applies before the program is loaded again, from memory, and run.
        let _sandbox = sandbox_cli::enter_or_exit(&plan, 0);
        let program = match unio::load_for_run(&[main], unio::effects::Profile::Application) {
            Ok(program) => program,
            Err(analyses) => {
                for analysis in analyses.iter().filter(|a| a.has_errors()) {
                    eprint!("{}", analysis.render_text(Lang::En));
                }
                return 1;
            }
        };
        let module = program.modules.iter().position(|m| m.report).unwrap_or(0);
        let interp = unio::interp::Interp::new(program);
        let mut rt = unio_runtime::Runtime::new(program_args);
        unio::interp::run_main(&interp, &mut rt, module)
    });
    ExitCode::from(code.clamp(0, 255) as u8)
}

/// The compile step of `unio run` in the worker: the diagnostics of a program with errors, or
/// its sources for the run.
fn run_compile(file: PathBuf) -> ExitCode {
    let loaded = on_stack(FRONT_END_STACK, move || {
        unio::load_for_run(&[file], unio::effects::Profile::Application)
            .map(|program| (main_path(&program), Sources::of(&program)))
    });
    match loaded {
        Ok((Some(main), sources)) => {
            let mut out = worker::Enc::default();
            out.path(&main);
            sources.encode(&mut out);
            emit(&out)
        }
        Ok((None, _)) => {
            eprintln!("unio: the program has no main module");
            ExitCode::from(1)
        }
        Err(analyses) => {
            for analysis in analyses.iter().filter(|a| a.has_errors()) {
                eprint!("{}", analysis.render_text(Lang::En));
            }
            ExitCode::from(1)
        }
    }
}

fn json_text(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What the compile step of `unio test` found for one file.
enum Compiled {
    /// Nothing to report or run.
    Nothing,
    /// The file does not compile outside its tests: the diagnostics, as text.
    Broken(String),
    /// The main module to load, and the tests that do not compile, by item, with their errors.
    Tests(PathBuf, Vec<(usize, String)>),
}

/// The filter and the files of `unio test [FILTER] FILE...`.
fn test_files(arguments: &[String]) -> (Option<String>, Vec<PathBuf>) {
    match arguments.split_first() {
        Some((first, rest)) if !first.ends_with(".unio") && !Path::new(first).is_file() => (
            Some(first.clone()),
            rest.iter().map(PathBuf::from).collect(),
        ),
        _ => (None, arguments.iter().map(PathBuf::from).collect()),
    }
}

/// The compile step of `unio test` in the worker (SB-10): for every file, whether it compiles
/// and which tests do not, and the sources of the programs to run.
fn test_compile(files: Vec<PathBuf>, filter: Option<String>, lang: Lang) -> ExitCode {
    let mut out = worker::Enc::default();
    let mut sources = Sources {
        stdlib: None,
        files: Vec::new(),
    };
    out.u32(files.len() as u32);
    for file in files {
        // As `unio::check_files_for_test`, with the sources of the same load for the run.
        let path = file.clone();
        let filter = filter.clone();
        let (analyses, main, found) = on_stack(FRONT_END_STACK, move || {
            let program = unio::load_for_test(&[path], filter.as_deref());
            let found = Sources::of(&program);
            let main = main_path(&program);
            (unio::analyses(program), main, found)
        });
        let Some(analysis) = analyses.into_iter().next() else {
            out.u32(0);
            continue;
        };
        let tests: Vec<(usize, usize, usize)> = analysis
            .file
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                unio::ast::Item::Test(t) => Some((i, t.span.start.line, t.span.end.line)),
                _ => None,
            })
            .collect();
        let errors: Vec<&unio::Diagnostic> = analysis
            .diagnostics
            .iter()
            .filter(|d| d.severity == unio::Severity::Error)
            .collect();
        let outside = errors.iter().any(|d| {
            !tests
                .iter()
                .any(|(_, s, e)| (*s..=*e).contains(&d.span.start.line))
        });
        if outside || analysis.has_errors() && tests.is_empty() {
            out.u32(1).text(&analysis.render_text(lang));
            continue;
        }
        let failing: Vec<(usize, String)> = tests
            .iter()
            .filter_map(|(i, s, e)| {
                let mine: Vec<String> = errors
                    .iter()
                    .filter(|d| (*s..=*e).contains(&d.span.start.line))
                    .map(|d| format!("{}:{}: {}", d.span.start.line, d.code, d.message.get(lang)))
                    .collect();
                (!mine.is_empty()).then(|| (*i, mine.join("\n")))
            })
            .collect();
        let Some(main) = main else {
            out.u32(0);
            continue;
        };
        sources.add(found);
        out.u32(2).path(&main).u32(failing.len() as u32);
        for (item, message) in &failing {
            out.u32(*item as u32).text(message);
        }
    }
    sources.encode(&mut out);
    emit(&out)
}

fn decode_compiled(data: &[u8]) -> Option<(Vec<Compiled>, Sources)> {
    let mut d = worker::Dec::new(data);
    let n = d.u32()?;
    let mut compiled = Vec::new();
    for _ in 0..n {
        compiled.push(match d.u32()? {
            0 => Compiled::Nothing,
            1 => Compiled::Broken(d.text()?),
            _ => {
                let main = d.path()?;
                let k = d.u32()?;
                let mut failing = Vec::new();
                for _ in 0..k {
                    failing.push((d.u32()? as usize, d.text()?));
                }
                Compiled::Tests(main, failing)
            }
        });
    }
    let sources = Sources::decode(&mut d)?;
    d.end().then_some((compiled, sources))
}

/// `unio test [--json] [FILTER] FILE...` (T-4, T-5, T-5a). The worker compiles the files; each
/// file's tests then run in the sandbox of its package, which applies before the program is
/// loaded again from the sources the worker returned.
fn test(
    all_arguments: &[String],
    arguments: &[String],
    json: bool,
    lang: Lang,
    sandbox: &sandbox_cli::Options,
) -> ExitCode {
    let (filter, files) = test_files(arguments);
    if files.is_empty() {
        return usage("unio test needs files");
    }
    if worker::inside() {
        return test_compile(files, filter, lang);
    }
    let plan = match sandbox_cli::prepare(&files, sandbox, false) {
        Ok(plan) => plan,
        Err(code) => return ExitCode::from(code as u8),
    };
    let data = match compile_in_worker(all_arguments, &files, sandbox) {
        Ok(data) => data,
        Err(code) => return code,
    };
    let Some((compiled, sources)) = decode_compiled(&data) else {
        return malformed();
    };
    if compiled.len() != files.len() {
        return malformed();
    }
    sources.install();
    let mut records = Vec::new();
    let (mut passed, mut failed) = (0, 0);
    for (index, (file, compiled)) in files.into_iter().zip(compiled).enumerate() {
        let display = file.display().to_string();
        let (path, failing) = match compiled {
            Compiled::Nothing => continue,
            Compiled::Broken(text) => {
                eprint!("{text}");
                failed += 1;
                records.push(format!(
                    "{{\"file\": {}, \"name\": null, \"status\": \"failed\", \"message\": {}}}",
                    json_text(&display),
                    json_text("the file does not compile")
                ));
                continue;
            }
            Compiled::Tests(path, failing) => (path, failing),
        };
        let filter = filter.clone();
        let plan = plan.clone();
        let results = on_run_stack(move || {
            // SB-1: each file's tests run in the sandbox of its package.
            let _sandbox = sandbox_cli::enter_or_exit(&plan, index);
            let program = unio::load_for_test(&[path], filter.as_deref());
            let module = program.modules.iter().position(|m| m.report).unwrap_or(0);
            let interp = unio::interp::Interp::new(program);
            let mut results = Vec::new();
            for (i, item) in interp.program.modules[module]
                .analysis
                .file
                .items
                .iter()
                .enumerate()
            {
                let unio::ast::Item::Test(t) = item else {
                    continue;
                };
                let name = unio::interp::description(&t.description);
                if filter.as_ref().is_some_and(|f| !name.contains(f.as_str())) {
                    continue;
                }
                if let Some((_, message)) = failing.iter().find(|(j, _)| *j == i) {
                    results.push(unio::interp::TestResult {
                        name,
                        passed: false,
                        message: format!("the test does not compile:\n{message}\n"),
                        output: String::new(),
                        millis: 0,
                    });
                    continue;
                }
                results.push(unio::interp::run_test(&interp, module, i));
            }
            results
        });
        for r in results {
            if r.passed {
                passed += 1;
            } else {
                failed += 1;
            }
            if !json {
                if r.passed {
                    println!("ok   {display}: {} ({} ms)", r.name, r.millis);
                } else {
                    println!("FAIL {display}: {}", r.name);
                    for line in r.message.lines().chain(r.output.lines()) {
                        println!("     {line}");
                    }
                }
            }
            records.push(format!(
                "{{\"file\": {}, \"name\": {}, \"status\": \"{}\", \"time_ms\": {}, \"output\": {}, \"message\": {}}}",
                json_text(&display),
                json_text(&r.name),
                if r.passed { "passed" } else { "failed" },
                r.millis,
                json_text(&r.output),
                json_text(&r.message)
            ));
        }
    }
    if json {
        println!("[{}]", records.join(",\n "));
    } else {
        println!("{passed} passed, {failed} failed");
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
