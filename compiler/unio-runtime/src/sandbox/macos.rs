//! The restrictions of the operating system on macOS (SB-4): a Seatbelt profile applied with
//! `sandbox_init` and resource limits. Seatbelt restricts the whole process and every process it
//! starts, and it can be applied once; `unio test` therefore applies the union of the
//! permissions of its programs and the sources it still has to load, and the runtime keeps the
//! exact permissions of each program.

use super::{apply_rlimits, program_paths, Limits, Permissions, Scope};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::OnceLock;

extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> i32;
    fn sandbox_free_error(error: *mut c_char);
}

static APPLIED: OnceLock<Result<(), String>> = OnceLock::new();

/// Applies the profile once per process; later calls report the result of the first.
pub fn apply(permissions: &Permissions, limits: &Limits) -> Result<(), String> {
    APPLIED
        .get_or_init(|| {
            apply_rlimits(limits)?;
            init(&profile(permissions))
        })
        .clone()
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

/// The profile: everything is denied except what the permissions grant. Metadata stays
/// readable, because resolving a path reads the metadata of its parents; the runtime hides
/// files outside the read scopes from `exists` and `is_directory`.
pub fn profile(permissions: &Permissions) -> String {
    let mut p = String::from(
        "(version 1)\n(deny default)\n(allow file-read-metadata)\n(allow sysctl-read)\n\
         (allow signal (target self))\n\
         (allow file-read* file-write-data (literal \"/dev/null\"))\n\
         (allow file-read* (literal \"/dev/urandom\") (literal \"/dev/random\"))\n",
    );
    for scope in &permissions.read {
        p.push_str(&format!("(allow file-read* {})\n", filter(scope)));
    }
    for scope in &permissions.write {
        p.push_str(&format!("(allow file-write* {})\n", filter(scope)));
        if !scope.subtree {
            // The temporary files of the atomic commit protocol next to the target, and the
            // directory itself, which is opened to sync it.
            if let (Some(dir), Some(name)) = (scope.root.parent(), scope.root.file_name()) {
                let pattern = format!(
                    "^{}/\\.{}\\.unio-tmp-",
                    regex_literal(&dir.to_string_lossy()),
                    regex_literal(&name.to_string_lossy())
                );
                p.push_str(&format!("(allow file-write* (regex #\"{pattern}\"))\n"));
                p.push_str(&format!(
                    "(allow file-read-data (literal {}))\n",
                    quoted(dir)
                ));
            }
        }
    }
    if !permissions.process.is_empty() {
        p.push_str("(allow process-fork)\n(allow signal (target same-sandbox))\n");
        p.push_str(
            "(allow file-read* file-map-executable process-exec (subpath \"/usr/lib\") (subpath \"/usr/libexec\") \
             (subpath \"/System\") (subpath \"/Library/Apple\") (subpath \"/private/var/db/dyld\") \
             (literal \"/private/etc/localtime\"))\n",
        );
        for program in &permissions.process {
            for path in program_paths(program) {
                p.push_str(&format!(
                    "(allow file-read* file-map-executable process-exec (literal {}))\n",
                    quoted(&path)
                ));
            }
        }
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
        assert!(p.contains("(allow file-write* (literal \"/h/a.json\"))"));
        assert!(p.contains("\\.a\\.json\\.unio-tmp-"), "{p}");
        assert!(!p.contains("process-fork"));
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
        assert!(std::fs::write(root.join("open/out.txt"), "x").is_ok());
        assert!(std::fs::write(root.join("closed/c.txt"), "x").is_err());
        assert!(std::net::TcpStream::connect("127.0.0.1:9").is_err());
        println!("seatbelt checks passed");
    }
}
