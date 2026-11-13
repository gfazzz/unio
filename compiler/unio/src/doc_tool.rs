//! `unio doc [--open] [-o DIR] [PACKAGE]` (table 6.1, L-3, L-9b): the documentation of a
//! package as HTML pages (`unio::doc`).
//!
//! The package is that of PACKAGE, a directory or a file in it (by default the working
//! directory): the nearest directory with `unio.toml`. A worker reads and checks it with the
//! scopes of SB-11 and returns the pages (SB-10); a package with errors gets none, and its
//! diagnostics are shown. The tool writes the pages into `target/doc/` of the package, or into
//! DIR: it never writes through a link in the package, replaces a file of a page only by a new
//! file, and removes only pages of an earlier run that `unio doc` made. It prints the path of
//! the index page; `--open` then opens it with the program in `BROWSER`, or with the opener of
//! the system (`xdg-open` on Linux, `open` on macOS, `rundll32.exe url.dll,FileProtocolHandler`
//! on Windows), outside the sandbox, as the operator's own command.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use unio::sandbox_cli;
use unio::Lang;

use crate::{emit, malformed, usage, worker};

/// The test variable that makes `--open` print the command instead of running it.
const PRINT_OPEN: &str = "UNIO_TEST_PRINT_OPEN";

pub fn command(arguments: &[String]) -> ExitCode {
    let (sandbox, arguments) = match sandbox_cli::take(arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    let mut open = false;
    let mut out = None;
    let mut package = None;
    let mut i = 0;
    while i < arguments.len() {
        let a = &arguments[i];
        if a == "--open" {
            open = true;
        } else if a == "-o" {
            i += 1;
            match arguments.get(i) {
                Some(dir) => out = Some(PathBuf::from(dir)),
                None => return usage("-o needs a directory"),
            }
        } else if a.starts_with('-') {
            return usage(&format!("unknown option '{a}' of unio doc"));
        } else if package.is_none() {
            package = Some(PathBuf::from(a));
        } else {
            return usage("unio doc takes one package");
        }
        i += 1;
    }
    if worker::inside() {
        let Some(root) = package else {
            return usage("the worker of unio doc needs the package");
        };
        return pages_in_worker(&root);
    }
    let cwd = unio_runtime::sandbox::working_directory().to_path_buf();
    let given = package.unwrap_or_else(|| PathBuf::from("."));
    let root = match package_root(&cwd.join(&given)) {
        Some(root) => root,
        None => {
            return usage(&format!(
                "{}: no such package, directory, or file",
                given.display()
            ))
        }
    };
    let spec = match worker::Spec::for_directory(&root, &sandbox) {
        Ok(spec) => spec,
        Err(code) => return ExitCode::from(code),
    };
    let temp = match worker::TempDir::new() {
        Ok(temp) => temp,
        Err(problem) => {
            eprintln!("unio: {problem}");
            return ExitCode::from(worker::EXIT_SANDBOX);
        }
    };
    let request = ["doc".to_string(), root.to_string_lossy().into_owned()];
    let data = match worker::start("tool", &request, &spec, &temp, true, &[]) {
        Ok(finished) if finished.code == 0 => finished.data,
        Ok(finished) => return ExitCode::from(finished.code),
        Err(code) => return ExitCode::from(code),
    };
    let Some(pages) = decode(&data) else {
        return malformed();
    };
    let dir = match out {
        Some(dir) => cwd.join(dir),
        None => root.join("target").join("doc"),
    };
    if let Err(problem) = write(&root, &dir, &pages) {
        eprintln!("unio doc: {problem}");
        return ExitCode::from(1);
    }
    let index = dir.join(unio::doc::INDEX);
    println!("{}", index.display());
    if open {
        return open_index(&index);
    }
    ExitCode::SUCCESS
}

/// The package of a path: the nearest directory with `unio.toml` at or above it, or the
/// directory itself (of a file, its directory) without one, as the compiler takes it.
fn package_root(path: &Path) -> Option<PathBuf> {
    let path = lexical(path);
    let meta = std::fs::metadata(&path).ok()?;
    let dir = if meta.is_dir() {
        path
    } else {
        path.parent()?.to_path_buf()
    };
    let root = dir
        .ancestors()
        .find(|a| a.join("unio.toml").is_file())
        .unwrap_or(&dir)
        .to_path_buf();
    Some(root)
}

/// A path without `.` and with `..` taken away lexically, so that pages name no `./`.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// In the worker: the pages of the package on standard output, or its diagnostics.
fn pages_in_worker(root: &Path) -> ExitCode {
    match unio::doc::generate(root) {
        Ok(pages) => {
            let mut out = worker::Enc::default();
            out.u32(pages.len() as u32);
            for page in &pages {
                out.text(&page.name).text(&page.html);
            }
            emit(&out)
        }
        Err(analyses) => {
            let lang = std::env::var("UNIO_LANG")
                .ok()
                .and_then(|v| Lang::parse(&v))
                .unwrap_or(Lang::En);
            for analysis in analyses.iter().filter(|a| a.has_errors()) {
                eprint!("{}", analysis.render_text(lang));
            }
            ExitCode::from(1)
        }
    }
}

fn decode(data: &[u8]) -> Option<Vec<(String, String)>> {
    let mut d = worker::Dec::new(data);
    let n = d.u32()?;
    let mut pages = Vec::new();
    for _ in 0..n {
        let name = d.text()?;
        // A page is a file of the output directory, never a path.
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return None;
        }
        pages.push((name, d.text()?));
    }
    d.end().then_some(pages)
}

/// A directory of the package that the tool creates or writes into: a real directory, never a
/// link, which the project could point anywhere.
fn real_directory(dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "{} is a link; unio doc does not write through a link of the package",
            dir.display()
        )),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(format!("{} is not a directory", dir.display())),
        Err(_) => std::fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display())),
    }
}

fn write(root: &Path, dir: &Path, pages: &[(String, String)]) -> Result<(), String> {
    if dir.starts_with(root) {
        // Inside the package, every directory on the way must be a real one.
        let mut at = root.to_path_buf();
        for component in dir.strip_prefix(root).unwrap_or(dir).components() {
            at.push(component.as_os_str());
            real_directory(&at)?;
        }
    } else {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    for (name, html) in pages {
        let path = dir.join(name);
        replace(&path, html).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    // Pages of an earlier run whose module is gone; nothing else is removed.
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut stale: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".html") || pages.iter().any(|(page, _)| *page == name) {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_file()) && made_by_unio_doc(&entry.path()) {
                stale.push(entry.path());
            }
        }
        for path in stale {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

fn made_by_unio_doc(path: &Path) -> bool {
    let mut start = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(512).read_to_end(&mut start))
        .is_ok()
        && String::from_utf8_lossy(&start).contains(unio::doc::GENERATOR)
}

/// Writes a page as a new file: what was at its path, a link included, is removed first, so
/// nothing is written through it.
fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            return Err(std::io::Error::other(
                "a directory is in the place of a page",
            ))
        }
        Ok(_) => std::fs::remove_file(path)?,
        Err(_) => {}
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(text.as_bytes())
}

/// The command that opens the index: `BROWSER`, else the opener of the system.
fn opener(index: &Path) -> (String, Vec<String>) {
    let page = index.to_string_lossy().into_owned();
    if let Some(browser) = std::env::var("BROWSER")
        .ok()
        .filter(|b| !b.trim().is_empty())
    {
        return (browser, vec![page]);
    }
    if cfg!(target_os = "macos") {
        ("open".into(), vec![page])
    } else if cfg!(windows) {
        (
            "rundll32.exe".into(),
            vec!["url.dll,FileProtocolHandler".into(), page],
        )
    } else {
        ("xdg-open".into(), vec![page])
    }
}

fn open_index(index: &Path) -> ExitCode {
    let (program, args) = opener(index);
    if std::env::var(PRINT_OPEN).is_ok_and(|v| v == "1") {
        println!("unio doc: would run {program} {}", args.join(" "));
        return ExitCode::SUCCESS;
    }
    match std::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "unio doc: the pages are written, but {program} that opens them cannot be run: {e}; set BROWSER to a browser"
            );
            ExitCode::from(1)
        }
    }
}
