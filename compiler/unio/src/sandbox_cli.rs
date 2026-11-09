//! The sandbox options of `unio run` and `unio test` (SB-1 to SB-5): `--policy FILE` or
//! `UNIO_POLICY`, and `--allow-weaker-sandbox`. The manifest of each program is the `unio.toml`
//! of its package, the nearest one above the file, as in `unio check`. A policy or a manifest
//! that cannot be read or parsed, and a sandbox that cannot be applied, stop the start with
//! code 103.

use std::path::{Path, PathBuf};

use unio_runtime::fault::EXIT_SANDBOX;
use unio_runtime::sandbox::{self, Declared, Guard, Permissions, Places, Policy, Sandbox, Scope};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub policy: Option<PathBuf>,
    pub allow_weaker: bool,
}

/// Takes one sandbox option at `arguments[i]`; returns how many arguments it used.
fn take_one(arguments: &[String], i: usize, options: &mut Options) -> Result<usize, String> {
    let argument = &arguments[i];
    if argument == "--allow-weaker-sandbox" {
        options.allow_weaker = true;
        Ok(1)
    } else if let Some(value) = argument.strip_prefix("--policy=") {
        options.policy = Some(PathBuf::from(value));
        Ok(1)
    } else if argument == "--policy" {
        let value = arguments
            .get(i + 1)
            .ok_or_else(|| "--policy needs a file".to_string())?;
        options.policy = Some(PathBuf::from(value));
        Ok(2)
    } else {
        Ok(0)
    }
}

/// The sandbox options of `unio test`, wherever they are, and the other arguments.
pub fn take(arguments: &[String]) -> Result<(Options, Vec<String>), String> {
    let mut options = Options::default();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < arguments.len() {
        match take_one(arguments, i, &mut options)? {
            0 => {
                rest.push(arguments[i].clone());
                i += 1;
            }
            n => i += n,
        }
    }
    Ok((options, rest))
}

/// The sandbox options of `unio run`: before the file, or between the file and `--`. Without
/// `--`, everything after the file belongs to the program.
pub fn take_run(arguments: &[String]) -> Result<(Options, Vec<String>), String> {
    let mut options = Options::default();
    let mut rest = Vec::new();
    let mut i = 0;
    let dash = arguments.iter().position(|a| a == "--");
    let mut seen_file = false;
    while i < arguments.len() {
        if Some(i) == dash {
            rest.extend_from_slice(&arguments[i..]);
            break;
        }
        let scanning = !seen_file || dash.is_some();
        let used = if scanning {
            take_one(arguments, i, &mut options)?
        } else {
            0
        };
        if used == 0 {
            seen_file = true;
            rest.push(arguments[i].clone());
            i += 1;
        } else {
            i += used;
        }
    }
    Ok((options, rest))
}

/// The sandboxes of the programs of one invocation.
#[derive(Clone, Debug)]
pub struct Plan {
    sandboxes: Vec<Sandbox>,
    process_wide: Permissions,
    allow_weaker: bool,
}

/// The directory with `unio.toml` nearest above the file.
fn package_root(file: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let dir = absolute.parent()?.to_path_buf();
    dir.ancestors()
        .find(|a| a.join("unio.toml").is_file())
        .map(Path::to_path_buf)
}

/// The policy of `--policy` or `UNIO_POLICY`, or the default policy without one.
pub fn read_policy(options: &Options) -> Result<Policy, String> {
    let path = match &options.policy {
        Some(path) => path.clone(),
        None => match std::env::var_os("UNIO_POLICY") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => return Ok(Policy::default()),
        },
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("the policy {} cannot be read: {e}", path.display()))?;
    let base = std::fs::canonicalize(&path)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    sandbox::parse_policy(&text, &Places::current(&base))
        .map_err(|e| format!("the policy {} is invalid: {e}", path.display()))
}

fn parse_manifest_at(text: &str, root: &Path) -> Result<Permissions, String> {
    sandbox::parse_manifest(text, &Places::current(root)).map_err(|e| {
        format!(
            "the manifest {} is invalid: {e}",
            root.join("unio.toml").display()
        )
    })
}

fn read_manifest(file: &Path) -> Result<(Permissions, Option<PathBuf>), String> {
    let Some(root) = package_root(file) else {
        return Ok((Permissions::default(), None));
    };
    let manifest = root.join("unio.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("the manifest {} cannot be read: {e}", manifest.display()))?;
    let permissions = parse_manifest_at(&text, &root)?;
    Ok((permissions, Some(root)))
}

/// The manifest of a program for `unio build`: its text and directory, checked as `unio run`
/// would check it (C-5, SB-1).
pub fn manifest_of(file: &Path) -> Result<Option<(String, PathBuf)>, String> {
    let Some(root) = package_root(file) else {
        return Ok(None);
    };
    let manifest = root.join("unio.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("the manifest {} cannot be read: {e}", manifest.display()))?;
    parse_manifest_at(&text, &root)?;
    Ok(Some((text, root)))
}

/// The sandbox of an executable made by `unio build`: the permissions its manifest declared,
/// kept by the build, narrowed by the policy of `UNIO_POLICY` (the arguments belong to the
/// program, so there is no `--policy`). Paths with `$CWD`, `$HOME` and `$TEMP` are those of the
/// run; the build refused relative entries (docs/runtime.md 8.4).
pub fn prepare_embedded(declared: Option<&Declared>) -> Result<Plan, i32> {
    let fail = |message: String| {
        eprintln!("unio: {message}");
        EXIT_SANDBOX
    };
    let policy = read_policy(&Options::default()).map_err(fail)?;
    let permissions = match declared {
        Some(declared) => {
            let places = Places::current(sandbox::working_directory());
            declared
                .expand(&places)
                .map_err(|e| fail(format!("the permissions of this executable: {e}")))?
        }
        None => Permissions::default(),
    };
    let (permissions, warnings) = sandbox::effective(&permissions, &policy);
    for warning in warnings {
        eprintln!("unio: warning: {warning}");
    }
    let mut process_wide = Permissions::default();
    process_wide.add(&permissions);
    let mut sandboxes = vec![Sandbox::new(permissions, policy.limits.clone())];
    start_writers(&mut sandboxes)?;
    Ok(Plan {
        sandboxes,
        process_wide,
        allow_weaker: policy.allow_weaker,
    })
}

/// The directory of the standard library, which `unio test` still reads after the first file
/// (see `program.rs`).
/// The library the checker reads, so that the sandbox grants exactly that directory.
fn stdlib_dir(_start: &Path) -> Option<PathBuf> {
    crate::program::trusted_stdlib()
}

fn tree(root: PathBuf) -> Scope {
    Scope {
        root: sandbox::resolve(&root),
        subtree: true,
    }
}

/// Reads the policy and the manifests of the files and works out their permissions. With
/// `loads_later`, as in `unio test`, the sources of the later files and the standard library
/// stay readable for a mechanism that restricts the whole process (Seatbelt).
pub fn prepare(files: &[PathBuf], options: &Options, loads_later: bool) -> Result<Plan, i32> {
    let fail = |message: String| {
        eprintln!("unio: {message}");
        EXIT_SANDBOX
    };
    let policy = read_policy(options).map_err(fail)?;
    let mut warnings: Vec<String> = Vec::new();
    let mut sandboxes = Vec::new();
    let mut process_wide = Permissions::default();
    for file in files {
        let (manifest, root) = read_manifest(file).map_err(fail)?;
        let (permissions, found) = sandbox::effective(&manifest, &policy);
        for warning in found {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        process_wide.add(&permissions);
        if loads_later {
            let dir = root.or_else(|| file.parent().map(Path::to_path_buf));
            if let Some(dir) = dir {
                process_wide.add(&Permissions {
                    read: vec![tree(if dir.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        dir
                    })],
                    ..Permissions::default()
                });
            }
            if let Some(stdlib) = stdlib_dir(file) {
                process_wide.add(&Permissions {
                    read: vec![tree(stdlib)],
                    ..Permissions::default()
                });
            }
        }
        sandboxes.push(Sandbox::new(permissions, policy.limits.clone()));
    }
    for warning in warnings {
        eprintln!("unio: warning: {warning}");
    }
    start_writers(&mut sandboxes)?;
    Ok(Plan {
        sandboxes,
        process_wide,
        allow_weaker: options.allow_weaker || policy.allow_weaker,
    })
}

/// Starts the writer processes that must start before any restriction applies (macOS, where
/// Seatbelt restricts the whole process once), or stops with code 103.
fn start_writers(sandboxes: &mut [Sandbox]) -> Result<(), i32> {
    sandbox::start_writers(sandboxes).map_err(|refused| {
        eprintln!("unio: {}", refused.0);
        EXIT_SANDBOX
    })
}

/// Applies the sandbox of the program at `index` to the current thread before it runs, or
/// ends the process with code 103 (SB-5). The guard keeps the runtime checks and the limits.
pub fn enter_or_exit(plan: &Plan, index: usize) -> Guard {
    let sandbox = plan.sandboxes[index].clone();
    match sandbox::enter(sandbox, &plan.process_wide, plan.allow_weaker) {
        Ok(guard) => {
            if !guard.missing.is_empty() {
                eprintln!(
                    "unio: warning: the operator allowed a weaker sandbox; missing: {}",
                    guard.missing.join("; ")
                );
            }
            guard
        }
        Err(refused) => {
            eprintln!("unio: {}", refused.0);
            std::process::exit(EXIT_SANDBOX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn options_of_run_stop_at_the_program_arguments() {
        let (o, rest) = take_run(&words("--policy p.toml a.unio -- --policy x")).unwrap();
        assert_eq!(o.policy, Some(PathBuf::from("p.toml")));
        assert_eq!(rest, words("a.unio -- --policy x"));
        let (o, rest) = take_run(&words("a.unio --allow-weaker-sandbox -- b")).unwrap();
        assert!(o.allow_weaker);
        assert_eq!(rest, words("a.unio -- b"));
        let (o, rest) = take_run(&words("a.unio --allow-weaker-sandbox b")).unwrap();
        assert!(!o.allow_weaker);
        assert_eq!(rest, words("a.unio --allow-weaker-sandbox b"));
        assert!(take_run(&words("--policy")).is_err());
    }

    #[test]
    fn options_of_test_anywhere() {
        let (o, rest) = take(&words(
            "--json --policy=p.toml a.unio --allow-weaker-sandbox",
        ))
        .unwrap();
        assert_eq!(o.policy, Some(PathBuf::from("p.toml")));
        assert!(o.allow_weaker);
        assert_eq!(rest, words("--json a.unio"));
    }
}
