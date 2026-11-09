//! The `unio` command (front-end subset): check, fmt, outline, symbol, parse --ast, explain.

use std::process::ExitCode;

use unio::Lang;

use unio::sandbox_cli;

const USAGE: &str = "Usage:
    unio check [--diagnostics=json] [--lang=en|ru] [--profile=application|core] FILE...
    unio fmt [--check] FILE...
    unio run [--policy FILE] [--allow-weaker-sandbox] FILE [-- ARGS...]
    unio build [--release] FILE [-o OUT]
    unio test [--json] [--policy FILE] [--allow-weaker-sandbox] [FILTER] FILE...
    unio outline [--json] FILE
    unio symbol [--json] MODULE.NAME
    unio parse --ast FILE
    unio explain CODE

The language of messages is taken from --lang, then from UNIO_LANG, and is English otherwise.";

fn main() -> ExitCode {
    // Started as the writer process of a sandbox, this serves and ends here (docs/runtime.md 9.2).
    #[cfg(unix)]
    unio_runtime::sandbox::writer::serve_if_requested();
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("run") {
        return run(&arguments[1..]);
    }
    if arguments.first().map(String::as_str) == Some("build") {
        return build(&arguments[1..]);
    }
    let (sandbox, arguments) = if arguments.first().map(String::as_str) == Some("test") {
        match sandbox_cli::take(&arguments) {
            Ok(taken) => taken,
            Err(problem) => return usage(&problem),
        }
    } else {
        (sandbox_cli::Options::default(), arguments)
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
    match command.as_str() {
        "check" => check(files, json, lang, profile),
        "symbol" if files.len() == 1 => symbol(&files[0], json),
        "parse" if ast && files.len() == 1 => parse(&files[0], lang),
        "fmt" => fmt(files, check_only, lang),
        "test" => test(files, json, lang, &sandbox),
        "outline" if files.len() == 1 => outline(&files[0], json, lang),
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
                } else if let Err(e) = std::fs::write(path, formatted) {
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
    std::thread::Builder::new()
        .stack_size(RUN_STACK)
        .spawn(f)
        .expect("a thread for the interpreter")
        .join()
        .expect("the interpreter thread")
}

/// `unio build [--release] FILE [-o OUT]`: one executable (C-5, docs/runtime.md 8.4).
fn build(arguments: &[String]) -> ExitCode {
    let mut release = false;
    let mut out = None;
    let mut file = None;
    let mut i = 0;
    while i < arguments.len() {
        let a = &arguments[i];
        if a == "--release" {
            release = true;
        } else if a == "-o" {
            match arguments.get(i + 1) {
                Some(o) => out = Some(std::path::PathBuf::from(o)),
                None => return usage("-o needs a file"),
            }
            i += 1;
        } else if let Some(o) = a.strip_prefix("-o") {
            out = Some(std::path::PathBuf::from(o));
        } else if a.starts_with('-') {
            return usage(&format!("unknown option '{a}'"));
        } else if file.is_none() {
            file = Some(std::path::PathBuf::from(a));
        } else {
            return usage("unio build takes one file");
        }
        i += 1;
    }
    let Some(file) = file else {
        return usage("unio build needs a file");
    };
    let out = out.unwrap_or_else(|| std::path::PathBuf::from(file.file_stem().unwrap_or_default()));
    match unio::native::build(&file, &out, release) {
        Ok(()) => ExitCode::SUCCESS,
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

fn run(arguments: &[String]) -> ExitCode {
    let (sandbox, arguments) = match sandbox_cli::take_run(arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    let (file, rest) = match arguments.split_first() {
        Some((file, rest)) => (std::path::PathBuf::from(file), rest),
        None => return usage("unio run needs a file"),
    };
    let program_args: Vec<String> = match rest.first().map(String::as_str) {
        Some("--") => rest[1..].to_vec(),
        Some(_) => rest.to_vec(),
        None => Vec::new(),
    };
    let plan = match sandbox_cli::prepare(std::slice::from_ref(&file), &sandbox, false) {
        Ok(plan) => plan,
        Err(code) => return ExitCode::from(code as u8),
    };
    // The program is loaded and checked once: a file with errors is reported and not run.
    let code = on_run_stack(move || {
        let program = match unio::load_for_run(&[file], unio::effects::Profile::Application) {
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
        // SB-1: the sandbox applies after the sources are loaded and before main runs.
        let _sandbox = sandbox_cli::enter_or_exit(&plan, 0);
        let mut rt = unio_runtime::Runtime::new(program_args);
        unio::interp::run_main(&interp, &mut rt, module)
    });
    ExitCode::from(code.clamp(0, 255) as u8)
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

/// `unio test [--json] [FILTER] FILE...` (T-4, T-5, T-5a).
fn test(arguments: &[String], json: bool, lang: Lang, sandbox: &sandbox_cli::Options) -> ExitCode {
    let (filter, files): (Option<String>, Vec<std::path::PathBuf>) = match arguments.split_first() {
        Some((first, rest))
            if !first.ends_with(".unio") && !std::path::Path::new(first).is_file() =>
        {
            (
                Some(first.clone()),
                rest.iter().map(std::path::PathBuf::from).collect(),
            )
        }
        _ => (
            None,
            arguments.iter().map(std::path::PathBuf::from).collect(),
        ),
    };
    if files.is_empty() {
        return usage("unio test needs files");
    }
    let plan = match sandbox_cli::prepare(&files, sandbox, true) {
        Ok(plan) => plan,
        Err(code) => return ExitCode::from(code as u8),
    };
    let mut records = Vec::new();
    let (mut passed, mut failed) = (0, 0);
    for (index, file) in files.into_iter().enumerate() {
        let display = file.display().to_string();
        let analyses = unio::check_files_for_test(std::slice::from_ref(&file), filter.as_deref());
        let Some(analysis) = analyses.into_iter().next() else {
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
        let outside: Vec<&&unio::Diagnostic> = errors
            .iter()
            .filter(|d| {
                !tests
                    .iter()
                    .any(|(_, s, e)| (*s..=*e).contains(&d.span.start.line))
            })
            .collect();
        if !outside.is_empty() || analysis.has_errors() && tests.is_empty() {
            eprint!("{}", analysis.render_text(lang));
            failed += 1;
            records.push(format!(
                "{{\"file\": {}, \"name\": null, \"status\": \"failed\", \"message\": {}}}",
                json_text(&display),
                json_text("the file does not compile")
            ));
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
        let path = file.clone();
        let filter = filter.clone();
        let plan = plan.clone();
        let results = on_run_stack(move || {
            let program = unio::load_for_test(&[path], filter.as_deref());
            // SB-1: each file's tests run in the sandbox of its package.
            let _sandbox = sandbox_cli::enter_or_exit(&plan, index);
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
