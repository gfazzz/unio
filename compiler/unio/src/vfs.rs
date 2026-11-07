//! The files a program is loaded from. Normally the file system; in an executable made by
//! `unio build`, the sources embedded in it (docs/runtime.md 8.4), so that the executable
//! rebuilds the same declarations without reading its sources or the standard library at run
//! time, and the paths in fault traces are those of the build.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct Embedded {
    stdlib: Option<PathBuf>,
    files: HashMap<PathBuf, String>,
}

static EMBEDDED: OnceLock<Embedded> = OnceLock::new();

/// Makes the embedded files the only files programs are loaded from.
pub fn install(stdlib: Option<PathBuf>, files: Vec<(PathBuf, String)>) {
    let _ = EMBEDDED.set(Embedded {
        stdlib,
        files: files.into_iter().collect(),
    });
}

/// The standard library directory recorded by the build, when the files are embedded.
pub(crate) fn embedded_stdlib() -> Option<Option<PathBuf>> {
    EMBEDDED.get().map(|e| e.stdlib.clone())
}

pub(crate) fn is_file(path: &Path) -> bool {
    match EMBEDDED.get() {
        Some(e) => e.files.contains_key(path),
        None => path.is_file(),
    }
}

pub(crate) fn read_to_string(path: &Path) -> std::io::Result<String> {
    match EMBEDDED.get() {
        Some(e) => e
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound)),
        None => std::fs::read_to_string(path),
    }
}

/// Embedded paths are already canonical.
pub(crate) fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    match EMBEDDED.get() {
        Some(e) if e.files.contains_key(path) => Ok(path.to_path_buf()),
        Some(_) => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        None => std::fs::canonicalize(path),
    }
}

/// The `.unio` files directly in a directory, sorted.
pub(crate) fn unio_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = match EMBEDDED.get() {
        Some(e) => e
            .files
            .keys()
            .filter(|p| p.parent() == Some(dir))
            .cloned()
            .collect(),
        None => match std::fs::read_dir(dir) {
            Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
            Err(_) => Vec::new(),
        },
    };
    found.retain(|p| p.extension().is_some_and(|e| e == "unio"));
    found.sort();
    found
}
