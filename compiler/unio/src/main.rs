//! The `unio` command (front-end subset): check, fmt, outline, symbol, parse --ast, explain.

use std::process::ExitCode;

use unio::Lang;

const USAGE: &str = "Usage:
    unio check [--diagnostics=json] [--lang=en|ru] [--profile=application|core] FILE...
    unio fmt [--check] FILE...
    unio outline [--json] FILE
    unio symbol [--json] MODULE.NAME
    unio parse --ast FILE
    unio explain CODE

The language of messages is taken from --lang, then from UNIO_LANG, and is English otherwise.";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
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
