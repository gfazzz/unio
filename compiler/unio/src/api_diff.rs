//! `unio api-diff` (T-15): the public API of a package, or the comparison of two versions of
//! it with the semantic-version verdict, the affected callers of other packages, and the
//! unambiguous migrations. The work runs in a worker that reads only the given packages, their
//! locked dependencies, and the standard library (SB-11).

use std::path::PathBuf;
use std::process::ExitCode;

use unio::sandbox_cli;

use crate::{isolated, usage, worker};

pub const USAGE: &str = "unio api-diff [--json] [--callers DIR]... OLD [NEW]";

/// `unio api-diff [--json] [--callers DIR]... OLD [NEW]`: with one package, its API; with two,
/// the changes from OLD to NEW. The exit code is 1 when NEW has a version too small for its
/// changes, 2 for a usage mistake or a package that does not check.
pub fn command(all: &[String], arguments: &[String]) -> ExitCode {
    let (sandbox, arguments) = match sandbox_cli::take(arguments) {
        Ok(taken) => taken,
        Err(problem) => return usage(&problem),
    };
    let mut json = false;
    let mut callers = Vec::new();
    let mut packages = Vec::new();
    let mut i = 0;
    while i < arguments.len() {
        let a = &arguments[i];
        if a == "--json" {
            json = true;
        } else if a == "--callers" {
            match arguments.get(i + 1) {
                Some(dir) => callers.push(PathBuf::from(dir)),
                None => return usage("--callers needs a directory"),
            }
            i += 1;
        } else if let Some(dir) = a.strip_prefix("--callers=") {
            callers.push(PathBuf::from(dir));
        } else if a.starts_with('-') {
            return usage(&format!("unknown option '{a}'"));
        } else {
            packages.push(PathBuf::from(a));
        }
        i += 1;
    }
    if packages.is_empty() || packages.len() > 2 {
        return usage(&format!("{USAGE}: one or two package directories"));
    }
    if packages.len() == 1 && !callers.is_empty() {
        return usage("--callers needs two packages to compare");
    }
    let manifests: Vec<PathBuf> = packages
        .iter()
        .chain(&callers)
        .map(|d| d.join("unio.toml"))
        .collect();
    isolated(
        all,
        || worker::Spec::for_files(&manifests, &sandbox),
        || work(&packages, &callers, json),
    )
}

fn work(packages: &[PathBuf], callers: &[PathBuf], json: bool) -> ExitCode {
    let mut apis = Vec::new();
    for dir in packages {
        match unio::api::read(dir) {
            Ok(api) => apis.push(api),
            Err(problem) => {
                eprintln!("unio api-diff: {problem}");
                return ExitCode::from(2);
            }
        }
    }
    if let [api] = apis.as_slice() {
        if json {
            println!("{}", unio::api::api_json(api));
        } else {
            print!("{}", unio::api::api_text(api));
        }
        return ExitCode::SUCCESS;
    }
    let (old, new) = (&apis[0], &apis[1]);
    let mut changes = unio::api::compare(old, new);
    for dir in callers {
        let mut found = changes.clone();
        if let Err(problem) = unio::api::find_callers(dir, &new.name, &mut found) {
            eprintln!("unio api-diff: {problem}");
            return ExitCode::from(2);
        }
        for (change, more) in changes.iter_mut().zip(found) {
            change.callers.extend(more.callers);
            change.edits.extend(more.edits);
        }
    }
    if json {
        println!("{}", unio::api::diff_json(old, new, &changes));
    } else {
        print!("{}", unio::api::diff_text(old, new, &changes));
    }
    match unio::api::verdict(old, new, &changes) {
        (_, None) => ExitCode::SUCCESS,
        (_, Some(_)) => ExitCode::from(1),
    }
}
