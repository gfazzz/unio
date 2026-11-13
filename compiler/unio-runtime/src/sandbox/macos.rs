//! The restrictions of the operating system on macOS (SB-4): a Seatbelt profile applied with
//! `sandbox_init` and resource limits. Seatbelt restricts the whole process and every process it
//! starts, and it can be applied once; `unio test` therefore applies the union of the
//! permissions of its programs and the sources it still has to load, and the runtime keeps the
//! exact permissions of each program.

use super::{apply_rlimits, children, Limits, Permissions, Scope};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::OnceLock;

extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> i32;
    fn sandbox_free_error(error: *mut c_char);
}

static APPLIED: OnceLock<Result<(), String>> = OnceLock::new();

/// The files the applied profile lets a child execute (`process-exec`), as `profile` lists them.
static EXECUTABLE: OnceLock<Vec<std::path::PathBuf>> = OnceLock::new();

/// The files a child may execute under the profile of this process, once it is applied; the
/// runtime checks them under a limit of processes (RFC 0011, `verify_executables`).
pub fn executable() -> Option<&'static [std::path::PathBuf]> {
    EXECUTABLE.get().map(Vec::as_slice)
}

/// Applies the profile once per process; later calls report the result of the first.
pub fn apply(permissions: &Permissions, limits: &Limits) -> Result<(), String> {
    APPLIED
        .get_or_init(|| {
            apply_rlimits(limits)?;
            let execute = if permissions.process.is_empty() {
                Vec::new()
            } else {
                children::needs(&permissions.process, &[])
                    .execute
                    .into_iter()
                    .collect()
            };
            init(&profile(permissions))?;
            let _ = EXECUTABLE.set(execute);
            if !permissions.process.is_empty() {
                super::tree::set_marked();
            }
            Ok(())
        })
        .clone()
}

/// Applies a profile to this process, for the tests of other modules.
#[cfg(test)]
pub(super) fn init_profile(profile: &str) -> Result<(), String> {
    init(profile)
}

fn init(profile: &str) -> Result<(), String> {
    let profile = CString::new(profile).map_err(|_| "a path contains a zero byte".to_string())?;
    let mut error: *mut c_char = std::ptr::null_mut();
    // SAFETY: profile ends with 0; error receives a message that is freed below.
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if result == 0 {
        return Ok(());
    }
    let message = if error.is_null() {
        "sandbox_init failed".to_string()
    } else {
        // SAFETY: a non-null error is a C string owned by the sandbox library.
        let text = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: freed once, as sandbox_init requires.
        unsafe { sandbox_free_error(error) };
        text
    };
    Err(format!("Seatbelt: {message}"))
}

/// A string literal of the profile language.
fn quoted(path: &Path) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A regular expression of the profile language that matches the text exactly.
fn regex_literal(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\.^$|?*+()[]{}\"".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn filter(scope: &Scope) -> String {
    if scope.subtree {
        format!("(subpath {})", quoted(&scope.root))
    } else {
        format!("(literal {})", quoted(&scope.root))
    }
}

/// The profile of a program: everything is denied except what the permissions grant. Metadata stays
/// readable, because resolving a path reads the metadata of its parents; the runtime hides
/// files outside the read scopes from `exists` and `is_directory`.
pub fn profile(permissions: &Permissions) -> String {
    let mut p = String::from(
        "(version 1)\n(deny default)\n(allow file-read-metadata)\n(allow sysctl-read)\n\
         (allow signal (target self))\n\
         (allow signal (target children))\n\
         (allow file-read* file-write-data (literal \"/dev/null\"))\n\
         (allow file-read* (literal \"/dev/urandom\") (literal \"/dev/random\"))\n",
    );
    for scope in &permissions.read {
        p.push_str(&format!("(allow file-read* {})\n", filter(scope)));
    }
    // The write scopes grant nothing here: the program and its children write no file; the
    // writer process writes for the program (docs/runtime.md 9.2, writer_profile).
    // A child may execute exactly the listed programs and the interpreters of listed scripts
    // (children.rs), and read and map the system libraries: the dyld shared cache and /usr/lib.
    // Nothing else may be executed (docs/runtime.md 9.5 and 9.6). Listing the descendants of a
    // child lets the runtime stop those that left its process group, and the marker of the
    // profile finds those whose parent has ended (9.5, `tree.rs`).
    if !permissions.process.is_empty() {
        p.push_str("(allow process-fork)\n(allow signal (target same-sandbox))\n");
        p.push_str(
            "(allow file-read* file-map-executable (subpath \"/usr/lib\") \
             (subpath \"/System\") (subpath \"/Library/Apple\") (subpath \"/private/var/db/dyld\") \
             (subpath \"/private/preboot/Cryptexes\") \
             (literal \"/private/etc/localtime\"))\n\
             (allow process-info* (target self))\n\
             (allow process-info-listpids)\n\
             (allow file-read-data (literal \"/\"))\n",
        );
        let needs = children::needs(&permissions.process, &[]);
        for path in needs.execute {
            p.push_str(&format!(
                "(allow file-read* file-map-executable process-exec (literal {}))\n",
                quoted(&path)
            ));
        }
        p.push_str(&super::tree::own_marker().rules());
    }
    p
}

/// Applies the profile of the writer process to it; it is a process of its own, so the profile
/// of the program has not been applied to it.
pub fn apply_writer(scopes: &[Scope]) -> Result<(), String> {
    apply_rlimits(&Limits::default())?;
    init(&writer_profile(scopes))
}

/// The profile of the writer process: it may write the write scopes, the temporary files of
/// the atomic commit next to a scope of one file, and open the directories on the way to them,
/// which it walks one component at a time; it reads no file, runs nothing, and has no network.
pub fn writer_profile(scopes: &[Scope]) -> String {
    let mut p = String::from(
        "(version 1)\n(deny default)\n(allow file-read-metadata)\n(allow sysctl-read)\n",
    );
    let mut ancestors: Vec<&Path> = Vec::new();
    for scope in scopes {
        p.push_str(&format!(
            "(allow file-read* file-write* {})\n",
            filter(scope)
        ));
        let mut dir = scope.root.parent();
        while let Some(d) = dir {
            if !ancestors.contains(&d) {
                ancestors.push(d);
            }
            dir = d.parent();
        }
        if !scope.subtree {
            if let (Some(dir), Some(name)) = (scope.root.parent(), scope.root.file_name()) {
                let pattern = format!(
                    "^{}/\\.{}\\.unio-tmp-",
                    regex_literal(&dir.to_string_lossy()),
                    regex_literal(&name.to_string_lossy())
                );
                p.push_str(&format!(
                    "(allow file-read* file-write* (regex #\"{pattern}\"))\n"
                ));
            }
        }
    }
    for dir in ancestors {
        p.push_str(&format!(
            "(allow file-read-data (literal {}))\n",
            quoted(dir)
        ));
    }
    p
}

/// Applies the profile of a worker of the tools to this process (SB-11, docs/runtime.md 9.7).
pub fn apply_worker(read: &[Scope], write: &[Scope], cwd: &Path) -> Result<(), String> {
    init(&worker_profile(read, write, cwd))
}

/// The profile of a worker: it may read the read trees, read and write the write trees, and
/// list its working directory, which `getcwd` needs to resolve relative paths; it may not
/// start or execute a process, has no network, and may signal only itself.
pub fn worker_profile(read: &[Scope], write: &[Scope], cwd: &Path) -> String {
    let mut p = String::from(
        "(version 1)\n(deny default)\n(allow file-read-metadata)\n(allow sysctl-read)\n\
         (allow signal (target self))\n\
         (allow file-read* file-write-data (literal \"/dev/null\"))\n\
         (allow file-read* (literal \"/dev/urandom\") (literal \"/dev/random\"))\n",
    );
    p.push_str(&format!(
        "(allow file-read-data (literal {}))\n",
        quoted(cwd)
    ));
    for scope in read {
        p.push_str(&format!("(allow file-read* {})\n", filter(scope)));
    }
    for scope in write {
        p.push_str(&format!(
            "(allow file-read* file-write* {})\n",
            filter(scope)
        ));
    }
    p
}

/// The tree of the developer tools a program of the link step belongs to, which the link step
/// may read and whose libraries it may map: `Xcode.app/Contents/Developer`, the command line
/// tools, or the directory above `usr/bin` of another toolchain; `None` for a system program.
pub fn developer_root(program: &Path) -> Option<std::path::PathBuf> {
    for dir in program.ancestors().skip(1) {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        let parent = dir
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned());
        if name.as_deref() == Some("Developer") && parent.as_deref() == Some("Contents") {
            return Some(dir.to_path_buf());
        }
        if dir == Path::new("/Library/Developer/CommandLineTools") {
            return Some(dir.to_path_buf());
        }
    }
    let bin = program.parent()?;
    let usr = bin.parent()?;
    let root = usr.parent()?;
    let system = [Path::new("/"), Path::new("/usr"), Path::new("/private")];
    (bin.file_name()? == "bin" && usr.file_name()? == "usr" && !system.contains(&root))
        .then(|| root.to_path_buf())
}

/// Applies the profile of the link step of `unio build` to this process (docs/runtime.md 9.7).
pub fn apply_link_worker(
    read: &[Scope],
    write: &[Scope],
    cwd: &Path,
    execute: &[std::path::PathBuf],
    marker: Option<&super::tree::Marker>,
) -> Result<(), String> {
    let mut profile = link_profile(read, write, cwd, execute);
    if let Some(marker) = marker {
        profile.push_str(&marker.rules());
    }
    init(&profile)
}

/// The profile of the link step: that of a worker, and it may start processes that execute
/// exactly the listed programs (the compiler and the linker it runs), map the libraries of the
/// read trees (the developer tools) and of the system, and signal its children. It still has
/// no network and writes only its temporary directory. [`apply_link_worker`] adds the marker
/// the tool gave, by which the tool finds every process of the link step to stop it.
pub fn link_profile(
    read: &[Scope],
    write: &[Scope],
    cwd: &Path,
    execute: &[std::path::PathBuf],
) -> String {
    let mut p = worker_profile(read, write, cwd);
    p.push_str(
        "(allow process-fork)\n(allow signal (target children))\n\
         (allow process-info* (target self))\n\
         (allow file-read-data (literal \"/\"))\n\
         (allow file-read* file-map-executable (subpath \"/usr/lib\") \
         (subpath \"/System\") (subpath \"/Library/Apple\") (subpath \"/private/var/db/dyld\") \
         (subpath \"/private/preboot/Cryptexes\") \
         (literal \"/private/etc/localtime\"))\n",
    );
    for scope in read.iter().filter(|s| s.subtree) {
        p.push_str(&format!("(allow file-map-executable {})\n", filter(scope)));
    }
    for path in execute {
        p.push_str(&format!(
            "(allow file-read* file-map-executable process-exec (literal {}))\n",
            quoted(path)
        ));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    #[test]
    fn the_profile_denies_by_default_and_grants_the_scopes() {
        let permissions = Permissions {
            read: vec![Scope {
                root: PathBuf::from("/w"),
                subtree: true,
            }],
            write: vec![Scope {
                root: PathBuf::from("/h/a.json"),
                subtree: false,
            }],
            env: BTreeSet::new(),
            process: BTreeSet::new(),
        };
        let p = profile(&permissions);
        assert!(p.contains("(deny default)"));
        assert!(p.contains("(allow file-read* (subpath \"/w\"))"));
        // Only /dev/null may be written; every file write goes through the writer process.
        let writes: Vec<&str> = p.lines().filter(|l| l.contains("file-write")).collect();
        assert!(
            writes.iter().all(|l| l.contains("\"/dev/null\"")),
            "the program writes no file itself: {p}"
        );
        assert!(p.contains("(allow signal (target children))"), "{p}");
        assert!(!p.contains("process-fork"));
        let mut with_sh = permissions.clone();
        with_sh.process.insert("sh".into());
        let p = profile(&with_sh);
        assert!(
            p.ends_with(&crate::sandbox::tree::own_marker().rules()),
            "the marker goes last: {p}"
        );
        let exec: Vec<&str> = p.lines().filter(|l| l.contains("process-exec")).collect();
        assert!(!exec.is_empty(), "{p}");
        assert!(
            exec.iter()
                .all(|l| l.contains("(literal ") && !l.contains("(subpath ")),
            "only listed files may be executed: {p}"
        );
        assert!(
            !p.contains("/usr/libexec") && !p.contains("/usr/share"),
            "{p}"
        );
        let w = writer_profile(&permissions.write);
        assert!(w.contains("(deny default)"));
        assert!(w.contains("(allow file-read* file-write* (literal \"/h/a.json\"))"));
        assert!(w.contains("\\.a\\.json\\.unio-tmp-"), "{w}");
        assert!(w.contains("(allow file-read-data (literal \"/h\"))"), "{w}");
        assert!(!w.contains("process-exec") && !w.contains("network"));
        let k = worker_profile(
            &permissions.read,
            &[Scope {
                root: PathBuf::from("/t"),
                subtree: true,
            }],
            Path::new("/c"),
        );
        assert!(k.contains("(deny default)"));
        assert!(k.contains("(allow file-read* (subpath \"/w\"))"), "{k}");
        assert!(
            k.contains("(allow file-read* file-write* (subpath \"/t\"))"),
            "{k}"
        );
        assert!(
            !k.contains("process-fork") && !k.contains("process-exec") && !k.contains("network")
        );
    }

    #[test]
    fn the_link_step_executes_only_the_listed_programs() {
        let clang = PathBuf::from(
            "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
        );
        assert_eq!(
            developer_root(&clang),
            Some(PathBuf::from("/Applications/Xcode.app/Contents/Developer"))
        );
        assert_eq!(
            developer_root(Path::new("/Library/Developer/CommandLineTools/usr/bin/ld")),
            Some(PathBuf::from("/Library/Developer/CommandLineTools"))
        );
        assert_eq!(
            developer_root(Path::new("/opt/llvm/usr/bin/clang")),
            Some(PathBuf::from("/opt/llvm"))
        );
        assert_eq!(developer_root(Path::new("/usr/bin/cc")), None);
        let read = [Scope {
            root: PathBuf::from("/Applications/Xcode.app/Contents/Developer"),
            subtree: true,
        }];
        let write = [Scope {
            root: PathBuf::from("/t"),
            subtree: true,
        }];
        let p = link_profile(&read, &write, Path::new("/c"), &[clang]);
        assert!(p.contains("(deny default)"), "{p}");
        assert!(p.contains("(allow process-fork)"), "{p}");
        let exec: Vec<&str> = p.lines().filter(|l| l.contains("process-exec")).collect();
        assert_eq!(exec.len(), 1, "{p}");
        assert!(exec[0].contains("(literal ") && exec[0].ends_with("/usr/bin/clang\"))"));
        let writes: Vec<&str> = p.lines().filter(|l| l.contains("file-write")).collect();
        assert!(
            writes
                .iter()
                .all(|l| l.contains("\"/t\"") || l.contains("\"/dev/null\"")),
            "{p}"
        );
        assert!(!p.contains("network"), "{p}");
    }

    /// Seatbelt applies to the whole process, so it is tried in a child process: the test
    /// binary runs itself with one test and a variable that makes that test apply the profile.
    #[test]
    fn seatbelt_denies_what_the_scopes_do_not_grant() {
        const VARIABLE: &str = "UNIO_SEATBELT_CHILD";
        let Some(dir) = std::env::var_os(VARIABLE) else {
            let dir = std::env::temp_dir().join(format!("unio-seatbelt-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("open")).unwrap();
            std::fs::create_dir_all(dir.join("closed")).unwrap();
            std::fs::write(dir.join("open/a.txt"), "a").unwrap();
            std::fs::write(dir.join("closed/b.txt"), "b").unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sandbox::macos::tests::seatbelt_denies_what_the_scopes_do_not_grant",
                    "--nocapture",
                ])
                .env(VARIABLE, &dir)
                .output()
                .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            let text = String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{text}");
            assert!(text.contains("seatbelt checks passed"), "{text}");
            return;
        };
        let root = std::fs::canonicalize(dir).unwrap();
        let permissions = Permissions {
            read: vec![Scope {
                root: root.join("open"),
                subtree: true,
            }],
            write: vec![Scope {
                root: root.join("open/out.txt"),
                subtree: false,
            }],
            env: BTreeSet::new(),
            process: BTreeSet::new(),
        };
        apply(&permissions, &Limits::default()).unwrap();
        assert!(std::fs::read_to_string(root.join("open/a.txt")).is_ok());
        assert!(std::fs::read_to_string(root.join("closed/b.txt")).is_err());
        // The program writes no file itself; the writer process does (docs/runtime.md 9.2).
        assert!(std::fs::write(root.join("open/out.txt"), "x").is_err());
        assert!(std::fs::write(root.join("closed/c.txt"), "x").is_err());
        assert!(std::net::TcpStream::connect("127.0.0.1:9").is_err());
        println!("seatbelt checks passed");
    }
}
