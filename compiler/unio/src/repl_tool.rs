//! `unio repl` (T-13, docs/language-contracts.md section 5): a checked interactive session.
//!
//! Every input is checked as the session module of the package around the working directory
//! (`unio::repl`) in a worker of the tools that executes nothing (SB-11). An input that checks
//! runs in a separate process, the evaluator, in the sandbox of `unio run` with the manifest of
//! the package and the limits of the policy; the session stops it after the wall-clock limit of
//! the policy, or after [`DEFAULT_SECONDS`] without one. `:type` checks an expression and shows
//! its type without running anything. A rejected input keeps its fixes under its revision: a
//! fix applies only while the session is still at that revision, and the fixed input is
//! checked again before it runs.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use unio::repl::{self, Binding, InputFix, Mode};
use unio::sandbox_cli;
use unio::Lang;

use crate::{on_run_stack, on_stack, usage, worker, Sources, FRONT_END_STACK};

/// The wall-clock limit of one evaluation when the policy sets none.
pub const DEFAULT_SECONDS: u64 = 60;

const COMPILE: &str = "--compile";
const EVALUATE: &str = "--evaluate";

const HELP: &str = "Enter declarations (function, struct, enum, import, ...), statements, or an
expression; a line that ends with ':' starts a block that an empty line ends.
  :type EXPR   the type of an expression, without running it
  :fix [N]     apply fix N (default 1) of the input just rejected, and check it again
  :show        the declarations and the kept bindings
  :reset       forget the declarations and the bindings
  :quit        end the session (also the end of the input)
";

/// `unio repl [--policy FILE] [--allow-weaker-sandbox] [--lang=en|ru]`.
pub fn command(arguments: &[String]) -> ExitCode {
    let (sandbox, arguments) = match sandbox_cli::take(arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    let mut lang = std::env::var("UNIO_LANG")
        .ok()
        .and_then(|v| Lang::parse(&v))
        .unwrap_or(Lang::En);
    let mut step = None;
    for a in &arguments {
        if a == COMPILE || a == EVALUATE {
            step = Some(a.as_str());
        } else if let Some(value) = a.strip_prefix("--lang=") {
            match Lang::parse(value) {
                Some(l) => lang = l,
                None => return usage(&format!("unknown language '{value}'; use en or ru")),
            }
        } else {
            return usage(&format!("unio repl takes no argument '{a}'"));
        }
    }
    match step {
        Some(COMPILE) if worker::inside() => compile_step(),
        Some(EVALUATE) => evaluate_step(&sandbox),
        Some(_) => usage("an internal step of unio repl"),
        None => Session::start(sandbox, lang).map_or_else(|code| code, |mut s| s.run()),
    }
}

// ---------------------------------------------------------------- the compile step

struct Request {
    path: PathBuf,
    mode: Mode,
    declarations: Vec<String>,
    bindings: Vec<Binding>,
    input: String,
    lang: Lang,
}

impl Request {
    fn encode(&self) -> Vec<u8> {
        let mut out = worker::Enc::default();
        out.path(&self.path).u32(self.mode.code());
        out.u32(self.declarations.len() as u32);
        for d in &self.declarations {
            out.text(d);
        }
        out.u32(self.bindings.len() as u32);
        for b in &self.bindings {
            out.text(&b.name)
                .u32(u32::from(b.var))
                .text(&b.ty)
                .text(&b.literal);
        }
        out.text(&self.input).u32(u32::from(self.lang == Lang::Ru));
        out.0
    }

    fn decode(data: &[u8]) -> Option<Request> {
        let mut d = worker::Dec::new(data);
        let path = d.path()?;
        let mode = Mode::from_code(d.u32()?)?;
        let mut declarations = Vec::new();
        for _ in 0..d.u32()? {
            declarations.push(d.text()?);
        }
        let mut bindings = Vec::new();
        for _ in 0..d.u32()? {
            bindings.push(Binding {
                name: d.text()?,
                var: d.u32()? != 0,
                ty: d.text()?,
                literal: d.text()?,
            });
        }
        let input = d.text()?;
        let lang = if d.u32()? != 0 { Lang::Ru } else { Lang::En };
        d.end().then_some(Request {
            path,
            mode,
            declarations,
            bindings,
            input,
            lang,
        })
    }
}

/// What the compile step returned.
struct Compiled {
    errors: bool,
    rendered: String,
    fixes: Vec<InputFix>,
    bad_bindings: Vec<String>,
    types: Vec<(String, bool, String)>,
    value_type: Option<String>,
    module_name: String,
    /// The line of the session module where the input starts.
    first_line: usize,
    sources: Option<Sources>,
}

fn compile_step() -> ExitCode {
    let Some(request) = Request::decode(worker::input()) else {
        eprintln!("unio repl: the compile step got a malformed request");
        return ExitCode::from(worker::EXIT_ABNORMAL);
    };
    let out = on_stack(FRONT_END_STACK, move || {
        let (checked, program) = repl::check_input(
            &request.path,
            &request.declarations,
            &request.bindings,
            &request.input,
            request.mode,
            request.lang,
        );
        let mut out = worker::Enc::default();
        out.u32(u32::from(checked.errors)).text(&checked.rendered);
        out.u32(checked.fixes.len() as u32);
        for f in &checked.fixes {
            out.u32(f.start as u32)
                .u32(f.end as u32)
                .text(&f.new_text)
                .text(&f.description);
        }
        out.u32(checked.bad_bindings.len() as u32);
        for b in &checked.bad_bindings {
            out.text(b);
        }
        out.u32(checked.types.len() as u32);
        for (name, var, ty) in &checked.types {
            out.text(name).u32(u32::from(*var)).text(ty);
        }
        match &checked.value_type {
            Some(ty) => out.u32(1).text(ty),
            None => out.u32(0),
        };
        out.text(&checked.module_name)
            .u32(checked.candidate.first_line as u32);
        if !checked.errors {
            Sources::of(&program).encode(&mut out);
        }
        out.0
    });
    crate::emit(&worker::Enc(out))
}

fn decode_compiled(data: &[u8]) -> Option<Compiled> {
    let mut d = worker::Dec::new(data);
    let errors = d.u32()? != 0;
    let rendered = d.text()?;
    let mut fixes = Vec::new();
    for _ in 0..d.u32()? {
        fixes.push(InputFix {
            start: d.u32()? as usize,
            end: d.u32()? as usize,
            new_text: d.text()?,
            description: d.text()?,
        });
    }
    let mut bad_bindings = Vec::new();
    for _ in 0..d.u32()? {
        bad_bindings.push(d.text()?);
    }
    let mut types = Vec::new();
    for _ in 0..d.u32()? {
        types.push((d.text()?, d.u32()? != 0, d.text()?));
    }
    let value_type = match d.u32()? {
        0 => None,
        _ => Some(d.text()?),
    };
    let module_name = d.text()?;
    let first_line = d.u32()? as usize;
    let sources = if errors {
        None
    } else {
        Some(Sources::decode(&mut d)?)
    };
    d.end().then_some(Compiled {
        errors,
        rendered,
        fixes,
        bad_bindings,
        types,
        value_type,
        module_name,
        first_line,
        sources,
    })
}

// ---------------------------------------------------------------- the evaluator

/// What the evaluator returned.
struct Evaluated {
    failure: Option<String>,
    output: String,
    errors: String,
    shown: Option<String>,
    /// The top-level bindings with their values as source, `None` when they cannot be kept.
    values: Vec<(String, Option<String>)>,
}

/// The evaluator: reads the program from its standard input, applies the sandbox of
/// `unio run`, runs the input, and writes what happened to its standard output.
fn evaluate_step(options: &sandbox_cli::Options) -> ExitCode {
    let mut data = Vec::new();
    if std::io::stdin().lock().read_to_end(&mut data).is_err() {
        return ExitCode::from(worker::EXIT_ABNORMAL);
    }
    let mut d = worker::Dec::new(&data);
    let (Some(path), Some(module_name), Some(sources)) =
        (d.path(), d.text(), Sources::decode(&mut d))
    else {
        eprintln!("unio repl: the evaluator got malformed data");
        return ExitCode::from(worker::EXIT_ABNORMAL);
    };
    // The sandbox of `unio run` for the session module: the manifest of its package and the
    // policy (SB-1).
    let plan = match sandbox_cli::prepare(std::slice::from_ref(&path), options, false) {
        Ok(plan) => plan,
        Err(code) => return ExitCode::from(code as u8),
    };
    sources.install();
    let result = on_run_stack(move || {
        let _sandbox = sandbox_cli::enter_or_exit(&plan, 0);
        let program = match unio::load_for_run(
            std::slice::from_ref(&path),
            unio::effects::Profile::Application,
        ) {
            Ok(program) => program,
            Err(analyses) => {
                let mut text = String::new();
                for analysis in analyses.iter().filter(|a| a.has_errors()) {
                    text.push_str(&analysis.render_text(Lang::En));
                }
                return Err(text);
            }
        };
        let module = program
            .modules
            .iter()
            .position(|m| m.path == path)
            .ok_or_else(|| "the session module was not loaded".to_string())?;
        let function = program
            .funcs
            .iter()
            .position(|f| f.module == module && f.name == repl::INPUT_FUNCTION)
            .ok_or_else(|| "the input was not loaded".to_string())?;
        let interp = unio::interp::Interp::new(program);
        let run = unio::interp::run_input(&interp, module, function, repl::VALUE);
        let values = run
            .values
            .iter()
            .filter(|(n, _)| n != repl::VALUE)
            .map(|(n, v)| (n.clone(), repl::literal(v, &module_name)))
            .collect();
        let mut out = worker::Enc::default();
        match &run.failure {
            Some(f) => out.u32(1).text(f),
            None => out.u32(0),
        };
        out.text(&run.output).text(&run.errors);
        match &run.shown {
            Some(s) => out.u32(1).text(s),
            None => out.u32(0),
        };
        let values: Vec<(String, Option<String>)> = values;
        out.u32(values.len() as u32);
        for (name, literal) in &values {
            out.text(name);
            match literal {
                Some(l) => out.u32(1).text(l),
                None => out.u32(0),
            };
        }
        Ok(out.0)
    });
    match result {
        Ok(data) => crate::emit(&worker::Enc(data)),
        Err(text) => {
            eprint!("{text}");
            ExitCode::from(1)
        }
    }
}

fn decode_evaluated(data: &[u8]) -> Option<Evaluated> {
    let mut d = worker::Dec::new(data);
    let failure = match d.u32()? {
        0 => None,
        _ => Some(d.text()?),
    };
    let output = d.text()?;
    let errors = d.text()?;
    let shown = match d.u32()? {
        0 => None,
        _ => Some(d.text()?),
    };
    let mut values = Vec::new();
    for _ in 0..d.u32()? {
        let name = d.text()?;
        let literal = match d.u32()? {
            0 => None,
            _ => Some(d.text()?),
        };
        values.push((name, literal));
    }
    d.end().then_some(Evaluated {
        failure,
        output,
        errors,
        shown,
        values,
    })
}

// ---------------------------------------------------------------- the session

/// A trace with the lines of the session module given as lines of the input.
fn input_lines(trace: &str, first_line: usize) -> String {
    let marker = format!("{}:", repl::SESSION_FILE);
    let mut out = String::new();
    let mut rest = trace;
    while let Some(at) = rest.find(&marker) {
        out.push_str(&rest[..at]);
        let after = &rest[at + marker.len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        match digits.parse::<usize>() {
            Ok(line) if line >= first_line => {
                out.push_str(&format!("input:{}", line - first_line + 1))
            }
            _ => {
                out.push_str(&marker);
                out.push_str(&digits);
            }
        }
        rest = &after[digits.len()..];
    }
    out.push_str(rest);
    out
}

/// A rejected input and its fixes, bound to its revision.
struct Rejected {
    revision: u32,
    input: String,
    fixes: Vec<InputFix>,
}

struct Session {
    sandbox: sandbox_cli::Options,
    lang: Lang,
    path: PathBuf,
    seconds: u64,
    declarations: Vec<String>,
    bindings: Vec<Binding>,
    revision: u32,
    rejected: Option<Rejected>,
}

/// The package root around the working directory, or the directory itself.
fn package_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cwd = unio_runtime::sandbox::canonical(&cwd).unwrap_or(cwd);
    cwd.ancestors()
        .find(|a| a.join("unio.toml").is_file())
        .map(Path::to_path_buf)
        .unwrap_or(cwd)
}

impl Session {
    fn start(sandbox: sandbox_cli::Options, lang: Lang) -> Result<Session, ExitCode> {
        let policy = sandbox_cli::read_policy(&sandbox).map_err(|problem| {
            eprintln!("unio: {problem}");
            ExitCode::from(worker::EXIT_SANDBOX)
        })?;
        let path = package_root().join(repl::SESSION_FILE);
        if path.exists() {
            eprintln!(
                "unio repl: {} exists; the session module takes that name, so rename the file",
                path.display()
            );
            return Err(ExitCode::from(2));
        }
        Ok(Session {
            sandbox,
            lang,
            path,
            seconds: policy.limits.wall_seconds.unwrap_or(DEFAULT_SECONDS),
            declarations: Vec::new(),
            bindings: Vec::new(),
            revision: 0,
            rejected: None,
        })
    }

    fn sandbox_arguments(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(policy) = &self.sandbox.policy {
            out.push("--policy".to_string());
            out.push(policy.to_string_lossy().into_owned());
        }
        if self.sandbox.allow_weaker {
            out.push("--allow-weaker-sandbox".to_string());
        }
        out
    }

    fn run(&mut self) -> ExitCode {
        let interactive = std::io::stdin().is_terminal();
        if interactive {
            println!("Unio {} REPL; :help for help", env!("CARGO_PKG_VERSION"));
        }
        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        loop {
            if interactive {
                print!("unio> ");
                let _ = std::io::stdout().flush();
            }
            let Some(Ok(line)) = lines.next() else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(command) = trimmed.strip_prefix(':') {
                if !self.command(command) {
                    break;
                }
                continue;
            }
            let mut input = line.clone();
            if trimmed.ends_with(':') {
                // A block: until an empty line or the end.
                loop {
                    if interactive {
                        print!("....> ");
                        let _ = std::io::stdout().flush();
                    }
                    match lines.next() {
                        Some(Ok(more)) if !more.trim().is_empty() => {
                            input.push('\n');
                            input.push_str(&more);
                        }
                        _ => break,
                    }
                }
            }
            self.submit(&input);
            let _ = std::io::stdout().flush();
        }
        ExitCode::SUCCESS
    }

    /// Runs a command; false ends the session.
    fn command(&mut self, command: &str) -> bool {
        let (name, rest) = command.split_once(' ').unwrap_or((command, ""));
        match name {
            "q" | "quit" | "exit" => return false,
            "help" | "h" => print!("{HELP}"),
            "type" | "t" => self.type_of(rest.trim()),
            "fix" => self.fix(rest.trim()),
            "show" => {
                for d in &self.declarations {
                    println!("{d}");
                }
                for b in &self.bindings {
                    let word = if b.var { "var" } else { "let" };
                    println!("{word} {} as {} = {}", b.name, b.ty, b.literal);
                }
            }
            "reset" => {
                self.declarations.clear();
                self.bindings.clear();
                self.rejected = None;
            }
            _ => println!("unknown command ':{name}'; :help lists the commands"),
        }
        true
    }

    /// Checks an input in a worker that executes nothing.
    fn compile(&self, input: &str, mode: Mode) -> Option<Compiled> {
        let request = Request {
            path: self.path.clone(),
            mode,
            declarations: self.declarations.clone(),
            bindings: self.bindings.clone(),
            input: input.to_string(),
            lang: self.lang,
        };
        let mut spec =
            match worker::Spec::for_files(std::slice::from_ref(&self.path), &self.sandbox) {
                Ok(spec) => spec,
                Err(_) => return None,
            };
        spec.input = request.encode();
        let temp = match worker::TempDir::new() {
            Ok(temp) => temp,
            Err(problem) => {
                eprintln!("unio: {problem}");
                return None;
            }
        };
        let mut arguments = vec!["repl".to_string(), COMPILE.to_string()];
        arguments.extend(self.sandbox_arguments());
        match worker::start("tool", &arguments, &spec, &temp, true, &[]) {
            Ok(finished) if finished.code == 0 => {
                let compiled = decode_compiled(&finished.data);
                if compiled.is_none() {
                    eprintln!("unio: the worker returned malformed data");
                }
                compiled
            }
            _ => None,
        }
    }

    fn type_of(&mut self, expression: &str) {
        if expression.is_empty() {
            println!(":type needs an expression");
            return;
        }
        let Some(compiled) = self.compile(expression, Mode::Expression) else {
            return;
        };
        if compiled.errors {
            print!("{}", compiled.rendered);
        } else {
            println!(
                "{expression} : {}",
                compiled.value_type.unwrap_or_else(|| "nothing".into())
            );
        }
    }

    fn fix(&mut self, which: &str) {
        let Some(rejected) = &self.rejected else {
            println!("no rejected input has fixes");
            return;
        };
        // `N` or `rREV.N`: a fix of another revision is refused.
        let (revision, number) = match which.strip_prefix('r').and_then(|r| r.split_once('.')) {
            Some((rev, n)) => (rev.parse::<u32>().ok(), n.parse::<usize>().ok()),
            None if which.is_empty() => (Some(rejected.revision), Some(1)),
            None => (Some(rejected.revision), which.parse::<usize>().ok()),
        };
        let (Some(revision), Some(number)) = (revision, number) else {
            println!(":fix takes a number, or rREVISION.NUMBER");
            return;
        };
        if revision != rejected.revision || rejected.revision != self.revision {
            println!(
                "the fix belongs to revision r{revision}, and the session is at r{}: enter the input again",
                self.revision
            );
            return;
        }
        let Some(fix) = number.checked_sub(1).and_then(|i| rejected.fixes.get(i)) else {
            println!("revision r{revision} has no fix {number}");
            return;
        };
        let Some(fixed) = repl::apply_fix(&rejected.input, fix) else {
            println!("the fix does not apply to the input");
            return;
        };
        println!("{fixed}");
        self.submit(&fixed);
    }

    fn submit(&mut self, input: &str) {
        // A rejection stays known after later inputs, so that its fixes are refused by
        // revision rather than forgotten.
        self.revision += 1;
        let mode = repl::mode_of(input);
        let mut compiled = match self.compile(input, mode) {
            Some(c) => c,
            None => return,
        };
        // A kept value that no longer checks is dropped, and the input is checked again.
        if !compiled.bad_bindings.is_empty() {
            for name in &compiled.bad_bindings {
                println!("note: '{name}' is no longer kept: its value does not check as source");
            }
            self.bindings
                .retain(|b| !compiled.bad_bindings.contains(&b.name));
            compiled = match self.compile(input, mode) {
                Some(c) => c,
                None => return,
            };
        }
        if compiled.errors {
            print!("{}", compiled.rendered);
            for (i, fix) in compiled.fixes.iter().enumerate() {
                println!("fix {} [r{}]: {}", i + 1, self.revision, fix.description);
            }
            if !compiled.fixes.is_empty() {
                self.rejected = Some(Rejected {
                    revision: self.revision,
                    input: input.to_string(),
                    fixes: compiled.fixes,
                });
            }
            return;
        }
        if mode == Mode::Declarations {
            self.declarations.push(input.trim_end().to_string());
            return;
        }
        let Some(sources) = compiled.sources.take() else {
            return;
        };
        let Some(evaluated) = self.evaluate(&compiled.module_name, &sources) else {
            return;
        };
        print!("{}", evaluated.output);
        eprint!("{}", evaluated.errors);
        if let Some(failure) = &evaluated.failure {
            eprint!("{}", input_lines(failure, compiled.first_line));
            println!(
                "note: the input ended early; what it did before is not undone, and its bindings are not kept"
            );
            return;
        }
        if let (Some(shown), Some(ty)) = (&evaluated.shown, &compiled.value_type) {
            if ty != "nothing" {
                println!("= {shown} : {ty}");
            }
        }
        for (name, literal) in evaluated.values {
            let declared = compiled.types.iter().find(|(n, _, _)| *n == name);
            let kept = self.bindings.iter().find(|b| b.name == name).cloned();
            let (var, ty) = match (declared, &kept) {
                (Some((_, var, ty)), _) => (*var, ty.clone()),
                (None, Some(b)) => (b.var, b.ty.clone()),
                (None, None) => continue,
            };
            self.bindings.retain(|b| b.name != name);
            match literal {
                Some(literal) => self.bindings.push(Binding {
                    name,
                    var,
                    ty,
                    literal,
                }),
                None => println!(
                    "note: '{name}' is not kept: a value of type {ty} cannot be written as source (a function, a resource, bytes, a path, or a type declared outside the session)"
                ),
            }
        }
    }

    /// Runs the checked program in the evaluator within the wall-clock limit.
    fn evaluate(&self, module_name: &str, sources: &Sources) -> Option<Evaluated> {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("unio repl: the evaluator cannot start: {e}");
                return None;
            }
        };
        let mut arguments = vec!["repl".to_string(), EVALUATE.to_string()];
        arguments.extend(self.sandbox_arguments());
        let mut child = match Command::new(exe)
            .args(&arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                eprintln!("unio repl: the evaluator cannot start: {e}");
                return None;
            }
        };
        let mut request = worker::Enc::default();
        request.path(&self.path).text(module_name);
        sources.encode(&mut request);
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&request.0);
        }
        let mut stdout = child.stdout.take().expect("a piped standard output");
        let mut stderr = child.stderr.take().expect("a piped standard error");
        let out = std::thread::spawn(move || {
            let mut data = Vec::new();
            let _ = stdout.read_to_end(&mut data);
            data
        });
        let err = std::thread::spawn(move || {
            let mut data = Vec::new();
            let _ = stderr.read_to_end(&mut data);
            data
        });
        let deadline = Instant::now() + Duration::from_secs(self.seconds);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break None,
            }
        };
        let data = out.join().unwrap_or_default();
        let errors = err.join().unwrap_or_default();
        eprint!("{}", String::from_utf8_lossy(&errors));
        match status {
            None => {
                eprintln!(
                    "unio repl: the input was stopped after {} s, the wall-clock limit of the session; what it did before is not undone",
                    self.seconds
                );
                None
            }
            Some(status) if status.success() => {
                let evaluated = decode_evaluated(&data);
                if evaluated.is_none() {
                    eprintln!("unio repl: the evaluator returned malformed data");
                }
                evaluated
            }
            Some(status) => {
                eprintln!(
                    "unio repl: the evaluator ended with code {}",
                    status.code().unwrap_or(-1)
                );
                None
            }
        }
    }
}
