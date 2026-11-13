//! The `unio` command (front-end subset): check, fmt, outline, symbol, parse --ast, explain.
//!
//! The commands that read a project run their work in a worker process with the sandbox of
//! SB-11 (`worker.rs`, docs/runtime.md 9.7); this process handles the arguments, starts the
//! worker, and shows what it returns.

// The language server: the protocol here, its analyses in `unio::lsp` (T-10, SB-13).
#[path = "lsp/server.rs"]
mod lsp_server;

mod api_diff;
mod doc_tool;
mod new_package;
mod packages;
mod publish;
mod repl_tool;
mod worker;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use unio::Lang;

use unio::sandbox_cli;

const USAGE: &str = "Usage:
    unio check [--diagnostics=json] [--lang=en|ru] [--profile=application|core] FILE...
    unio fmt [--check] FILE...
    unio fmt --migrate [--from=VERSION] [--check] [FILE...]
    unio run [--policy FILE] [--allow-weaker-sandbox] FILE [-- ARGS...]
    unio build [--release] [--target TRIPLE] [--policy FILE] [--allow-weaker-sandbox] FILE [-o OUT]
    unio build --print-toolchain [--target TRIPLE]
    unio new NAME [--bin | --lib]
    unio doc [--open] [-o DIR] [--policy FILE] [--allow-weaker-sandbox] [PACKAGE]
    unio test [--json] [--seed N] [--policy FILE] [--allow-weaker-sandbox] [FILTER] FILE...
    unio outline [--json] FILE
    unio symbol [--json] MODULE.NAME
    unio api-diff [--json] [--callers DIR]... OLD [NEW]
    unio parse --ast FILE
    unio explain CODE
    unio repl [--policy FILE] [--allow-weaker-sandbox] [--lang=en|ru]
    unio lsp [--policy FILE] [--allow-weaker-sandbox] [--lang=en|ru]
    unio add ALIAS [VERSION] (--path DIR | --package OWNER/NAME [--registry NAME] |
             --archive FILE|URL --sha384 HEX) [--unsafe-ok] [--offline]
    unio remove ALIAS
    unio update [ALIAS...] [--unsafe-ok] [--offline]
    unio install [--offline] [--unsafe-ok]
    unio key new NAME [--dir DIR]
    unio key id FILE
    unio key endorse PUBLIC.pub --key SIGNER.key --registry DIR [--expires YYYY-MM-DD]
    unio key revoke PUBLIC.pub|sha384:ID --key ANCHOR.key --registry DIR
    unio registry init DIR --anchor ANCHOR.key
    unio registry resign DIR --key KEY.key
    unio publish --registry DIR --owner OWNER --key KEY.key
    unio release sign --key KEY.key --subject NAME --sequence N --out MANIFEST FILE...
    unio release verify MANIFEST --public KEY.pub --subject NAME [--at-least N]

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
    match arguments.first().map(String::as_str) {
        Some("key") => return publish::key(&arguments[1..]),
        Some("registry") => return publish::registry_command(&arguments[1..]),
        Some("publish") => return publish::publish(&arguments[1..]),
        Some("release") => return publish::release(&arguments[1..]),
        Some("api-diff") => return api_diff::command(&arguments, &arguments[1..]),
        Some("repl") => return repl_tool::command(&arguments[1..]),
        Some("new") => return new_package::command(&arguments[1..]),
        Some("doc") => return doc_tool::command(&arguments[1..]),
        _ => {}
    }
    let (sandbox, arguments) = match sandbox_cli::take(&arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    // `--seed N` is `--seed=N` (T-12).
    let mut arguments = arguments;
    if let Some(i) = arguments.iter().position(|a| a == "--seed") {
        match arguments.get(i + 1).cloned() {
            Some(value) => {
                arguments.splice(i..i + 2, [format!("--seed={value}")]);
            }
            None => return usage("--seed needs a number"),
        }
    }
    let mut lang = std::env::var("UNIO_LANG")
        .ok()
        .and_then(|v| Lang::parse(&v))
        .unwrap_or(Lang::En);
    let mut json = false;
    let mut ast = false;
    let mut check_only = false;
    let mut migrate = false;
    let mut from: Option<String> = None;
    let mut seed: Option<i64> = None;
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
        } else if let Some(value) = argument.strip_prefix("--seed=") {
            match value.parse::<i64>() {
                Ok(n) => seed = Some(n),
                Err(_) => return usage(&format!("--seed needs a whole number, not '{value}'")),
            }
        } else if argument == "--migrate" {
            migrate = true;
        } else if let Some(value) = argument.strip_prefix("--from=") {
            from = Some(value.to_string());
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
        "fmt" if migrate && files.is_empty() => {
            print!("{}", unio::migrate::list());
            ExitCode::SUCCESS
        }
        "fmt" if migrate => isolated(
            &arguments,
            || {
                let mut spec = for_files()?;
                if !check_only {
                    spec.formats = format_targets(&paths);
                }
                Ok(spec)
            },
            || fmt_migrate(files, check_only, from.as_deref(), lang),
        ),
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
        "test" => test(&arguments, files, json, lang, &sandbox, seed),
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

/// `unio fmt --migrate` (R-9): applies the registered migrations of the language, then formats.
fn fmt_migrate(files: &[String], check_only: bool, from: Option<&str>, lang: Lang) -> ExitCode {
    let migrations = match unio::migrate::selected(from) {
        Ok(migrations) => migrations,
        Err(problem) => return usage(&problem),
    };
    for path in files {
        if let Err(message) = read(path) {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    }
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let mut failed = false;
    for migrated in unio::migrate::migrate(&paths, &migrations) {
        let path = migrated.path.display().to_string();
        let formatted = match unio::format::format_source(&migrated.text) {
            Ok(formatted) => formatted,
            Err(_) => {
                let analysis = unio::check_source(&path, &migrated.text);
                eprint!("{}", analysis.render_text(lang));
                failed = true;
                continue;
            }
        };
        for change in &migrated.applied {
            println!(
                "{path}:{}:{}: {}: {}",
                change.line, change.column, change.id, change.new_text
            );
        }
        if formatted == migrated.source {
            continue;
        }
        if check_only {
            println!("{path}");
            failed = true;
        } else if let Err(e) = worker::replace_file(Path::new(&path), &formatted) {
            eprintln!("unio: {path}: {e}");
            return ExitCode::from(2);
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
    let mut target = None;
    let mut i = 0;
    while i < arguments.len() {
        let a = &arguments[i];
        let named = if a == "--target" {
            i += 1;
            match arguments.get(i) {
                Some(t) => Some(t.as_str()),
                None => return usage("--target needs a target"),
            }
        } else {
            a.strip_prefix("--target=")
        };
        if let Some(name) = named {
            // `unio build --target T` (table 6.1): one of the four targets of R-8.
            match unio::native::Target::parse(name) {
                Some(t) => target = Some(t),
                None => {
                    return usage(&format!(
                        "unknown target '{name}'; the targets are {}",
                        unio::native::target_names()
                    ))
                }
            }
        } else if a == "--release" {
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
    let Some(target) = target.or_else(unio::native::Target::host) else {
        return fail(unio::native::unsupported());
    };
    if print_toolchain {
        if file.is_some() || out.is_some() || release {
            return usage("--print-toolchain takes no file and no option but --target");
        }
        return match unio::native::toolchain::identity_for(target) {
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
        return build_object(&file, target, release, &toolchain);
    }
    // A pair of machines that cannot be built for stops before anything is read or written.
    if let Some(problem) = unio::native::cross_problem(target) {
        return fail(problem);
    }
    // The policy and its compile limits, which hold for the compile step and the link step
    // alike, are read first: a limit that cannot be honoured stops the build here (RFC 0006).
    let mut spec = match worker::Spec::for_files(std::slice::from_ref(&file), &sandbox) {
        Ok(spec) => spec,
        Err(code) => return ExitCode::from(code),
    };
    // The linker and the SDK of this build, recorded in its image; a missing linker stops here.
    let toolchain = match unio::native::toolchain::identity_for(target) {
        Ok(identity) => identity,
        Err(m) => return fail(m),
    };
    let out = out.unwrap_or_else(|| {
        let mut name = file.file_stem().unwrap_or_default().to_os_string();
        name.push(target.executable_suffix());
        PathBuf::from(name)
    });
    let runtime = match unio::native::runtime_library_for(target) {
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
    compile.push(format!("--target={}", target.rust_triple()));
    compile.push(format!("{TOOLCHAIN_ARG}{toolchain}"));
    compile.push(file.to_string_lossy().into_owned());
    match worker::start("tool", &compile, &spec, &temp, false, &[]) {
        Ok(finished) if finished.code == 0 => {}
        Ok(finished) => return ExitCode::from(finished.code),
        Err(code) => return ExitCode::from(code),
    }
    store_object(&file, temp.path());
    let object = temp.path().join(unio::native::object_file_name());
    let executable = temp
        .path()
        .join(format!("program{}", target.executable_suffix()));
    let linker = match unio::native::link_command_for(target, &object, &runtime, &executable) {
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

/// The files of the temporary directory of a build that name the cache key of its object
/// file: one reused from the cache, or a new one for the tool to store (C-11b).
const CACHED_OBJECT: &str = "object.cached";
const NEW_OBJECT: &str = "object.key";

/// The artifact key of the object file of `file` (C-11b, `unio::cache`): the compiler, the
/// target and its ABI, the profile, the linker and the SDK recorded in the image, and every
/// text the program is loaded from, its manifests and lock included. `None` outside a package
/// or for a program that does not load, which is compiled to report its errors.
fn artifact_key(
    file: &Path,
    target: unio::native::Target,
    release: bool,
    toolchain: &str,
) -> Option<String> {
    unio::cache::dir_of(file)?;
    let path = file.to_path_buf();
    let sources = on_stack(FRONT_END_STACK, move || {
        unio::load_for_run(&[path], unio::effects::Profile::Application)
            .ok()
            .map(|program| Sources::of(&program))
    })?;
    let mut key = unio::cache::Key::new("artifact");
    key.field("toolchain", &unio::cache::toolchain())
        .field("target", &format!("{target:?}"))
        .field("object", unio::native::object_file_name())
        .field("profile", if release { "release" } else { "debug" })
        .field("linker", toolchain)
        .field("stdlib", &format!("{:?}", sources.stdlib))
        .field("sources", &unio::cache::sources_digest(&sources.files))
        .field("main", &file.to_string_lossy());
    Some(key.finish())
}

/// Stores a new object file of a build in the cache of its package, or says that the build
/// reused one (C-11b).
fn store_object(file: &Path, temp: &Path) {
    if temp.join(CACHED_OBJECT).is_file() {
        eprintln!("unio build: the object file is reused from the cache of the package (target/unio-cache)");
        return;
    }
    let (Ok(key), Some(cache)) = (
        std::fs::read_to_string(temp.join(NEW_OBJECT)),
        unio::cache::dir_of(file),
    ) else {
        return;
    };
    if let Ok(object) = std::fs::read(temp.join(unio::native::object_file_name())) {
        let _ = unio::cache::write(&cache, unio::cache::ARTIFACTS, key.trim(), &object);
    }
}

/// The compile step of `unio build` in the worker: the object file in its temporary directory.
fn build_object(
    file: &Path,
    target: unio::native::Target,
    release: bool,
    toolchain: &str,
) -> ExitCode {
    let Some(dir) = worker::temp_dir() else {
        return ExitCode::from(worker::EXIT_SANDBOX);
    };
    // The cache of the package (C-11b): an object file of the same key is reused, and the key
    // of a new one is left for the tool, which stores it; the worker writes no cache.
    let key = artifact_key(file, target, release, toolchain);
    if let (Some(key), Some(cache)) = (&key, unio::cache::dir_of(file)) {
        if let unio::cache::Lookup::Hit(object) =
            unio::cache::read(&cache, unio::cache::ARTIFACTS, key)
        {
            let written = std::fs::write(dir.join(unio::native::object_file_name()), object)
                .and_then(|()| std::fs::write(dir.join(CACHED_OBJECT), key));
            if written.is_ok() {
                return ExitCode::SUCCESS;
            }
        }
    }
    if let Some(key) = &key {
        let _ = std::fs::write(dir.join(NEW_OBJECT), key);
    }
    match unio::native::compile_object_for(file, target, release, toolchain) {
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

/// The replay record of a counterexample (T-12): the scheme, the seeds, the values, and the
/// versions of the compiler and of the library it was found with.
fn property_json(p: &unio::interp::property::Report, file: &str, test: &str) -> String {
    let library = match unio::program::trusted_stdlib() {
        Some(dir) if unio::vfs::is_builtin(&dir) => {
            format!("embedded in unio {}", env!("CARGO_PKG_VERSION"))
        }
        Some(dir) => dir.display().to_string(),
        None => "none".to_string(),
    };
    format!(
        "{{\"scheme\": {}, \"seed\": {}, \"case_index\": {}, \"sample_seed\": {}, \"original\": {}, \"minimal\": {}, \"shrink_attempts\": {}, \"compiler\": {}, \"library\": {}, \"replay\": {}}}",
        json_text(unio::interp::property::SCHEME),
        p.seed,
        p.case_index,
        p.sample_seed,
        json_text(&p.original),
        json_text(&p.minimal),
        p.shrink_attempts,
        json_text(concat!("unio ", env!("CARGO_PKG_VERSION"))),
        json_text(&library),
        json_text(&format!("unio test --seed {} {} {file}", p.seed, json_text(test)))
    )
}

/// What the compile step of `unio test` found for one file.
enum Compiled {
    /// Nothing to report or run.
    Nothing,
    /// The file does not compile outside its tests: the diagnostics, as text.
    Broken(String),
    /// The main module to load, the tests that do not compile, by item, with their errors,
    /// and what the cache needs (C-11a, C-11b).
    Tests(PathBuf, Vec<(usize, String)>, TestInputs),
}

/// What the compile step found for the cache of the tests of one file (C-11a, C-11b): the
/// content hash of each test, by item, and the digest of every text the program is loaded
/// from.
#[derive(Default)]
struct TestInputs {
    hashes: Vec<(usize, String)>,
    sources: String,
}

impl TestInputs {
    fn hash(&self, item: usize) -> Option<&str> {
        self.hashes
            .iter()
            .find(|(i, _)| *i == item)
            .map(|(_, h)| h.as_str())
    }
}

/// The content hashes of the tests of the reported module of a program (C-11a).
fn test_hashes(program: &unio::program::Program) -> Vec<(usize, String)> {
    let Some(module) = program.modules.iter().position(|m| m.report) else {
        return Vec::new();
    };
    let hashes = unio::symbol::hashes(program);
    program.modules[module]
        .analysis
        .file
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            unio::ast::Item::Test(t) => {
                Some((i, unio::symbol::test_hash(program, &hashes, module, t)))
            }
            _ => None,
        })
        .collect()
}

/// The payload of a cached test result: its name, its output, and the time it took when it
/// ran (C-11b).
fn encode_cached_test(r: &unio::interp::TestResult) -> Vec<u8> {
    let mut out = worker::Enc::default();
    out.text("unio-test-result-v1")
        .text(&r.name)
        .text(&r.output)
        .text(&r.millis.to_string());
    out.0
}

fn decode_cached_test(payload: &[u8]) -> Option<unio::interp::TestResult> {
    let mut d = worker::Dec::new(payload);
    if d.text()? != "unio-test-result-v1" {
        return None;
    }
    let result = unio::interp::TestResult {
        name: d.text()?,
        output: d.text()?,
        millis: d.text()?.parse().ok()?,
        passed: true,
        message: String::new(),
        properties: Vec::new(),
        external_inputs: false,
    };
    d.end().then_some(result)
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
    // The examples of the documentation comments run as tests of their module (T-13): the
    // module is loaded with them appended, here and, from the sources returned, for the run.
    let mut overlay = Vec::new();
    let mut line_maps = Vec::new();
    for file in &files {
        let augmented = std::fs::read_to_string(file)
            .ok()
            .and_then(|source| unio::doctest::augment(&source));
        match (augmented, unio_runtime::sandbox::canonical(file)) {
            (Some((text, map)), Ok(canonical)) => {
                overlay.push((canonical, text));
                line_maps.push(map);
            }
            _ => line_maps.push(Vec::new()),
        }
    }
    if !overlay.is_empty() {
        unio::vfs::overlay(overlay);
    }
    for (file, line_map) in files.into_iter().zip(line_maps) {
        // As `unio::check_files_for_test`, with the sources of the same load for the run.
        let path = file.clone();
        let filter = filter.clone();
        let (analyses, main, found, hashes) = on_stack(FRONT_END_STACK, move || {
            let program = unio::load_for_test(&[path], filter.as_deref());
            let found = Sources::of(&program);
            let main = main_path(&program);
            let hashes = test_hashes(&program);
            (unio::analyses(program), main, found, hashes)
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
                    .map(|d| {
                        let line = unio::doctest::original_line(&line_map, d.span.start.line);
                        format!("{line}:{}: {}", d.code, d.message.get(lang))
                    })
                    .collect();
                (!mine.is_empty()).then(|| (*i, mine.join("\n")))
            })
            .collect();
        let Some(main) = main else {
            out.u32(0);
            continue;
        };
        let digest = unio::cache::sources_digest(&found.files);
        sources.add(found);
        out.u32(2).path(&main).u32(failing.len() as u32);
        for (item, message) in &failing {
            out.u32(*item as u32).text(message);
        }
        out.u32(hashes.len() as u32);
        for (item, hash) in &hashes {
            out.u32(*item as u32).text(hash);
        }
        out.text(&digest);
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
                let mut inputs = TestInputs::default();
                for _ in 0..d.u32()? {
                    inputs.hashes.push((d.u32()? as usize, d.text()?));
                }
                inputs.sources = d.text()?;
                Compiled::Tests(main, failing, inputs)
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
    seed: Option<i64>,
) -> ExitCode {
    let (filter, files) = test_files(arguments);
    if files.is_empty() {
        return usage("unio test needs files");
    }
    if worker::inside() {
        return test_compile(files, filter, lang);
    }
    let mut plan = match sandbox_cli::prepare(&files, sandbox, false) {
        Ok(plan) => plan,
        Err(code) => return ExitCode::from(code as u8),
    };
    // The time and memory of each test (T-12): the [limits] of the policy, or the defaults.
    let test_limits = plan.take_test_limits();
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
    // The cache of test results (C-11a, C-11b): the journals of earlier runs move into place,
    // the results of this run are looked up, and the journal of this run is opened, all before
    // any sandbox applies.
    let mut caches: std::collections::BTreeMap<PathBuf, Option<unio::cache::Journal>> =
        std::collections::BTreeMap::new();
    let mut cache_of: Vec<Option<PathBuf>> = Vec::new();
    for (file, compiled) in files.iter().zip(&compiled) {
        let dir = match compiled {
            Compiled::Tests(..) => unio::cache::dir_of(file),
            _ => None,
        };
        if let Some(dir) = &dir {
            caches.entry(dir.clone()).or_insert_with(|| {
                unio::cache::promote(dir);
                unio::cache::Journal::open(dir).ok()
            });
        }
        cache_of.push(dir);
    }
    let mut records = Vec::new();
    let (mut passed, mut failed) = (0, 0);
    for (index, (file, compiled)) in files.into_iter().zip(compiled).enumerate() {
        let display = file.display().to_string();
        let (path, failing, inputs) = match compiled {
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
            Compiled::Tests(path, failing, inputs) => (path, failing, inputs),
        };
        let limits = unio::interp::TestLimits::of(&test_limits[index]);
        // The key of each test, and the results the cache holds for this run.
        let keys: Vec<(usize, String)> = inputs
            .hashes
            .iter()
            .map(|(item, hash)| {
                let mut key = unio::cache::Key::new("test");
                key.field("toolchain", &unio::cache::toolchain())
                    .field("runner", "unio test")
                    .field("generator", unio::interp::property::SCHEME)
                    .field("seed", &format!("{seed:?}"))
                    .field("sandbox", &plan.describe(index))
                    .field("limits", &format!("{limits:?}"))
                    .field("stdlib", &format!("{:?}", unio::program::trusted_stdlib()))
                    .field("sources", &inputs.sources)
                    .field("file", &display)
                    .field("item", &item.to_string())
                    .field("test", hash);
                (*item, key.finish())
            })
            .collect();
        let key_of = |item: usize| keys.iter().find(|(i, _)| *i == item).map(|(_, k)| k);
        let mut cached: std::collections::HashMap<usize, unio::interp::TestResult> =
            std::collections::HashMap::new();
        if let Some(dir) = &cache_of[index] {
            for (item, key) in &keys {
                if failing.iter().any(|(j, _)| j == item) {
                    continue;
                }
                match unio::cache::read(dir, unio::cache::TESTS, key) {
                    unio::cache::Lookup::Hit(payload) => {
                        if let Some(result) = decode_cached_test(&payload) {
                            cached.insert(*item, result);
                        } else {
                            unio::cache::remove(dir, unio::cache::TESTS, key);
                        }
                    }
                    // Never trusted: the test runs, and its result replaces the entry.
                    unio::cache::Lookup::Damaged => {
                        unio::cache::remove(dir, unio::cache::TESTS, key)
                    }
                    unio::cache::Lookup::Missing => {}
                }
            }
        }
        // The lines of the examples of documentation comments, for the traces of their tests.
        let line_map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|source| unio::doctest::augment(&source))
            .map(|(_, map)| map)
            .unwrap_or_default();
        let filter = filter.clone();
        let plan = plan.clone();
        let results = on_run_stack(move || {
            // SB-1: each file's tests run in the sandbox of its package.
            let _sandbox = sandbox_cli::enter_or_exit(&plan, index);
            unio::interp::property::set_seed(seed);
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
                    results.push((
                        i,
                        unio::interp::TestResult {
                            name,
                            passed: false,
                            message: format!("the test does not compile:\n{message}\n"),
                            output: String::new(),
                            millis: 0,
                            properties: Vec::new(),
                            external_inputs: false,
                        },
                        false,
                    ));
                    continue;
                }
                if let Some(result) = cached.remove(&i) {
                    results.push((i, result, true));
                    continue;
                }
                results.push((
                    i,
                    unio::interp::run_test_limited(&interp, module, i, limits),
                    false,
                ));
            }
            results
        });
        for (item, mut r, from_cache) in results {
            // A result that passed and read nothing from outside is kept for the next run.
            if !from_cache && r.passed && !r.external_inputs && r.properties.is_empty() {
                if let (Some(dir), Some(key)) = (&cache_of[index], key_of(item)) {
                    if let Some(Some(journal)) = caches.get_mut(dir) {
                        let _ = journal.record(unio::cache::TESTS, key, &encode_cached_test(&r));
                    }
                }
            }
            if r.name.starts_with("doc: ") {
                for (augmented, original) in &line_map {
                    r.message = r
                        .message
                        .replace(&format!(":{augmented})"), &format!(":{original})"));
                }
            }
            if r.passed {
                passed += 1;
            } else {
                failed += 1;
            }
            if !json {
                if r.passed && from_cache {
                    println!("ok   {display}: {} (cached)", r.name);
                } else if r.passed {
                    println!("ok   {display}: {} ({} ms)", r.name, r.millis);
                } else {
                    println!("FAIL {display}: {}", r.name);
                    for line in r.message.lines().chain(r.output.lines()) {
                        println!("     {line}");
                    }
                    // What reproduces a property that failed (T-12).
                    for p in &r.properties {
                        println!(
                            "     property {}: seed {}, case {}, sample seed {}, original {}, minimal {} after {} shrink attempts",
                            unio::interp::property::SCHEME,
                            p.seed,
                            p.case_index,
                            p.sample_seed,
                            p.original,
                            p.minimal,
                            p.shrink_attempts
                        );
                        println!(
                            "     replay: unio test --seed {} {} {display}",
                            p.seed,
                            json_text(&r.name)
                        );
                    }
                }
            }
            records.push(format!(
                "{{\"file\": {}, \"name\": {}, \"status\": \"{}\", \"time_ms\": {}, \"output\": {}, \"message\": {}, \"properties\": [{}], \"hash\": {}, \"cached\": {}}}",
                json_text(&display),
                json_text(&r.name),
                if r.passed { "passed" } else { "failed" },
                r.millis,
                json_text(&r.output),
                json_text(&r.message),
                r.properties
                    .iter()
                    .map(|p| property_json(p, &display, &r.name))
                    .collect::<Vec<_>>()
                    .join(", "),
                inputs.hash(item).map_or("null".to_string(), json_text),
                from_cache
            ));
        }
    }
    for journal in caches.into_values().flatten() {
        let _ = journal.finish();
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
