//! Helpers shared by the integration tests.

use std::fs;
use std::path::{Path, PathBuf};

/// The root of the repository: compiler/unio/../..
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Every `.unio` file below the directory, sorted.
pub fn unio_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "unio") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Windows has no sandbox of the operating system for programs or for the workers of the tools
/// yet, so `unio` refuses there with code 103 unless the operator allows a weaker sandbox
/// (SB-5, SB-10). The functional tests act as that operator through the external policy; the
/// refusal itself is tested separately without it.
#[allow(dead_code)]
pub fn allow_weaker_on_windows(command: &mut std::process::Command) {
    if cfg!(windows) {
        command.env("UNIO_POLICY", weaker_policy());
    }
}

/// A policy file that only allows the weaker sandbox.
#[allow(dead_code)]
pub fn weaker_policy() -> PathBuf {
    let policy = std::env::temp_dir().join(format!("unio-weaker-{}.toml", std::process::id()));
    fs::write(&policy, "allow_weaker = true\n").unwrap();
    policy
}
