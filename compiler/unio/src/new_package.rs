//! `unio new NAME [--bin | --lib]` (table 6.1): a new package in the directory NAME, with its
//! manifest, one module, and a test, that `unio check`, `unio test`, and `unio fmt --check`
//! accept as written (docs/packages.md section 1, docs/guide.md chapter 2).
//!
//! A program (`--bin`, the default) has the module `main.unio` with `public function main()`;
//! a library (`--lib`) has one module named after the package (`-` becomes `_`), imported by an
//! application as `deps.<alias>.<module>`. Each module has a documentation comment and a `test`
//! block next to the code, and the library has an example in a documentation comment, which
//! `unio test` runs too. The manifest grants no permissions. Nothing of an existing directory
//! is changed: the command stops when NAME exists.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::usage;

/// What the package is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Program,
    Library,
}

/// `unio new NAME [--bin | --lib]`.
pub fn command(arguments: &[String]) -> ExitCode {
    let mut kind = None;
    let mut path = None;
    for a in arguments {
        let wanted = match a.as_str() {
            "--bin" => Kind::Program,
            "--lib" => Kind::Library,
            _ if a.starts_with('-') => return usage(&format!("unknown option '{a}' of unio new")),
            _ if path.is_some() => return usage("unio new takes one name"),
            _ => {
                path = Some(PathBuf::from(a));
                continue;
            }
        };
        if kind.is_some_and(|k| k != wanted) {
            return usage("unio new takes --bin or --lib, not both");
        }
        kind = Some(wanted);
    }
    let Some(path) = path else {
        return usage("unio new needs the name of the package");
    };
    let kind = kind.unwrap_or(Kind::Program);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !unio::package::manifest::valid_name(&name) {
        return usage(&format!(
            "'{name}' is not a valid package name: lowercase letters, digits, '-' and '_', starting with a letter"
        ));
    }
    let module = name.replace('-', "_");
    if kind == Kind::Library {
        if let Some(problem) = module_name_problem(&module) {
            return usage(&problem);
        }
    }
    match create(&path, &name, &module, kind) {
        Ok(files) => {
            let what = match kind {
                Kind::Program => "program",
                Kind::Library => "library",
            };
            println!(
                "unio new: created the {what} package {name} in {}",
                path.display()
            );
            for file in files {
                println!("    {}", file.display());
            }
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("unio new: {problem}");
            ExitCode::from(1)
        }
    }
}

/// Why the module of a library cannot be named `module`: a keyword cannot be imported, and
/// `std` and `deps` start the names of other modules (L-22, T-14).
fn module_name_problem(module: &str) -> Option<String> {
    if unio::lexer::KEYWORDS.contains(&module) {
        return Some(format!(
            "the module of the library would be named '{module}', a keyword of Unio, which cannot be imported; choose another name"
        ));
    }
    if module == "std" || module == "deps" {
        return Some(format!(
            "the module of the library would be named '{module}', which starts the names of other modules; choose another name"
        ));
    }
    None
}

/// The manifest: the name, the first version, and no permissions.
fn manifest(name: &str) -> String {
    format!(
        "# The manifest of the package: its name, its version, its dependencies, and its permissions.
# A program may read its arguments and standard input and write to standard output and standard
# error; anything else is granted in [permissions], such as files.read = [\"$CWD/**\"].

[package]
name = \"{name}\"
version = \"0.1.0\"

[permissions]
"
    )
}

fn program(name: &str) -> String {
    format!(
        "## The program {name}.
##
## Usage:
##     unio run main.unio [NAME]

import std.args

## The greeting for a name.
function greeting(name as Text) returns Text:
    return \"Hello, {{name}}!\"

public function main():
    print(greeting(args.list().first() or \"world\"))

test \"the greeting names whom it greets\":
    assert greeting(\"world\") == \"Hello, world!\"
"
    )
}

fn library(name: &str) -> String {
    format!(
        "## The library {name}.

## The greeting for a name.
##
## Example:
##     assert greeting(\"Anna\") == \"Hello, Anna!\"
public function greeting(name as Text) returns Text:
    return \"Hello, {{name}}!\"

test \"the greeting names whom it greets\":
    assert greeting(\"world\") == \"Hello, world!\"
"
    )
}

/// Creates the directory and its files; nothing when the directory exists.
fn create(dir: &Path, name: &str, module: &str, kind: Kind) -> Result<Vec<PathBuf>, String> {
    if std::fs::symlink_metadata(dir).is_ok() {
        return Err(format!(
            "{} already exists; choose a new directory",
            dir.display()
        ));
    }
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let source = match kind {
        Kind::Program => ("main.unio".to_string(), program(name)),
        Kind::Library => (format!("{module}.unio"), library(name)),
    };
    let files = [
        ("unio.toml".to_string(), manifest(name)),
        source,
        // `unio doc` writes into target/ of the package.
        (".gitignore".to_string(), "/target/\n".to_string()),
    ];
    let mut written = Vec::new();
    for (file, text) in files {
        let path = dir.join(&file);
        write_new(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

fn write_new(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(text.as_bytes())
}
