//! The files a program is loaded from. Normally the file system; for `unio run` and `unio test`,
//! the files the worker of the tool read, kept in memory (docs/runtime.md 9.7), so that the
//! program is loaded again in its sandbox without reading its sources or the standard library.
//! An executable made by `unio build` loads nothing: it reads its program image (8.4).
//!
//! The standard library is also embedded in the compiler itself ([`BUILTIN_ROOT`]): an installed
//! `unio` needs no file of the library on disk (plan section 12, release condition 1). Its
//! modules have paths below that root, which no file system path can be, and are always read
//! from the compiler's own memory, never from a disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct Embedded {
    stdlib: Option<PathBuf>,
    files: HashMap<PathBuf, String>,
}

static EMBEDDED: OnceLock<Embedded> = OnceLock::new();

mod builtin {
    // The files of `stdlib/` at the build, written by `build.rs`: (path below the library, text).
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}

/// The directory of the standard library embedded in the compiler. It is relative and starts
/// with a component that no file name on Windows may contain, so it never names a directory a
/// program or a project made; it ends with `stdlib`, which the layers of modules are found
/// after ([`crate::program::Program::module_layer`]).
pub const BUILTIN_ROOT: &str = "<unio>/stdlib";

/// The root of the embedded standard library, if this compiler was built with one.
pub fn builtin_stdlib() -> Option<PathBuf> {
    (!builtin::FILES.is_empty()).then(|| PathBuf::from(BUILTIN_ROOT))
}

/// Whether the path is the embedded library or below it.
pub fn is_builtin(path: &Path) -> bool {
    path.starts_with(BUILTIN_ROOT)
}

/// The text of a file of the embedded library.
fn builtin_file(path: &Path) -> Option<&'static str> {
    let relative = path.strip_prefix(BUILTIN_ROOT).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let name = parts.join("/");
    builtin::FILES
        .iter()
        .find(|(file, _)| *file == name)
        .map(|(_, text)| *text)
}

/// The `.unio` files of the embedded library directly in a directory of it.
fn builtin_files_in(dir: &Path) -> Vec<PathBuf> {
    let root = Path::new(BUILTIN_ROOT);
    builtin::FILES
        .iter()
        .map(|(file, _)| root.join(file))
        .filter(|p| p.parent() == Some(dir))
        .collect()
}

/// The standard library a worker of the tools was given by the tool (docs/runtime.md 9.7):
/// the worker sees no `UNIO_STDLIB`, since it receives no environment but the language of
/// messages.
static STDLIB: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Makes the directory the standard library of every program loaded from the file system;
/// `None`: there is none, as for the tool.
pub fn use_stdlib(dir: Option<PathBuf>) {
    let _ = STDLIB.set(dir);
}

/// The unsaved buffers of the editor that the language server gave its worker (SB-13,
/// docs/runtime.md 9.7): by canonical path, they take the place of the files on disk.
static OVERLAY: OnceLock<HashMap<PathBuf, String>> = OnceLock::new();

/// Makes these texts the contents of their paths for every program loaded afterwards; other
/// files are read as before. The paths are canonical.
pub fn overlay(files: Vec<(PathBuf, String)>) {
    let _ = OVERLAY.set(files.into_iter().collect());
}

fn overlaid(path: &Path) -> Option<&'static String> {
    OVERLAY.get().and_then(|o| o.get(path))
}

/// Makes the embedded files the only files programs are loaded from.
pub fn install(stdlib: Option<PathBuf>, files: Vec<(PathBuf, String)>) {
    let _ = EMBEDDED.set(Embedded {
        stdlib,
        files: files.into_iter().collect(),
    });
}

/// The standard library directory recorded by the build, when the files are embedded.
/// Also the directory given by [`use_stdlib`].
pub(crate) fn embedded_stdlib() -> Option<Option<PathBuf>> {
    EMBEDDED
        .get()
        .map(|e| e.stdlib.clone())
        .or_else(|| STDLIB.get().cloned())
}

pub(crate) fn is_file(path: &Path) -> bool {
    if overlaid(path).is_some() {
        return true;
    }
    match EMBEDDED.get() {
        Some(e) => e.files.contains_key(path),
        None if is_builtin(path) => builtin_file(path).is_some(),
        None => path.is_file(),
    }
}

pub(crate) fn read_to_string(path: &Path) -> std::io::Result<String> {
    if let Some(text) = overlaid(path) {
        return Ok(text.clone());
    }
    match EMBEDDED.get() {
        Some(e) => e
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound)),
        None if is_builtin(path) => builtin_file(path)
            .map(str::to_string)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound)),
        None => std::fs::read_to_string(path),
    }
}

/// Embedded paths are already canonical, and so are those of the embedded library.
pub(crate) fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    if overlaid(path).is_some() {
        return Ok(path.to_path_buf());
    }
    match EMBEDDED.get() {
        Some(e) if e.files.contains_key(path) => Ok(path.to_path_buf()),
        _ if is_builtin(path) => Ok(path.to_path_buf()),
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
        None if is_builtin(dir) => builtin_files_in(dir),
        None => match std::fs::read_dir(dir) {
            Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
            Err(_) => Vec::new(),
        },
    };
    if let Some(overlay) = OVERLAY.get() {
        let unsaved: Vec<PathBuf> = overlay
            .keys()
            .filter(|p| p.parent() == Some(dir) && !found.contains(p))
            .cloned()
            .collect();
        found.extend(unsaved);
    }
    found.retain(|p| p.extension().is_some_and(|e| e == "unio"));
    found.sort();
    found
}
