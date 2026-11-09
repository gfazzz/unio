//! What a child process needs from the operating system to run a program the manifest lists
//! under `process` (docs/runtime.md 9.5 and 9.6). The rules of the sandbox grant a child the
//! right to execute exactly these files and to read the libraries they load, and nothing else:
//! a child that may run `sh` cannot run `/bin/cat`.
//!
//! For every path a listed name resolves to (as [`super::program_paths`] resolves it):
//!
//! - the file itself may be executed and read;
//! - a script that starts with `#!` adds its interpreter, which may be executed; when the
//!   interpreter is `env`, the program `env` is asked to run is resolved through `PATH` and
//!   added as well. A listed script therefore runs its interpreter, and only that one;
//! - on Linux an ELF file adds its program interpreter (`PT_INTERP`, the dynamic loader), which
//!   the kernel executes, and the shared libraries it needs (`DT_NEEDED`, found through its
//!   `DT_RUNPATH` or `DT_RPATH`, `/etc/ld.so.cache`, and the library directories), which may be
//!   read; the libraries they need are followed in turn. A library inside the library
//!   directories is already readable (see `LIBRARY_DIRS` in `linux.rs`), so only libraries
//!   outside them are added one by one;
//! - on macOS `/bin/sh` runs the shell that `/private/var/select/sh` names, which is added;
//!   libraries come from the dyld shared cache and `/usr/lib`, which the profile grants.
//!
//! Resolution is done once, before the sandbox applies, and touches only the listed files and
//! their libraries; the depth is bounded, so a loop of interpreters or libraries ends.

use super::{program_paths, resolve};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The files a child may execute and the files it may read to run the listed programs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// Executed by the kernel: the listed programs, the interpreters of scripts, and on Linux
    /// the dynamic loader. Each may also be read.
    pub execute: BTreeSet<PathBuf>,
    /// Read only: shared libraries outside the library directories.
    pub read: BTreeSet<PathBuf>,
}

/// How deep interpreters and libraries are followed.
const DEPTH: usize = 8;

/// What the children need to run the listed programs.
pub fn needs(programs: &BTreeSet<String>, library_dirs: &[PathBuf]) -> Needs {
    let mut needs = Needs::default();
    for program in programs {
        for path in program_paths(program) {
            add_program(&mut needs, &path, library_dirs, 0);
        }
    }
    needs
}

fn add_program(needs: &mut Needs, path: &Path, library_dirs: &[PathBuf], depth: usize) {
    if depth > DEPTH || !needs.execute.insert(path.to_path_buf()) {
        return;
    }
    let Ok(mut file) = File::open(path) else {
        return;
    };
    let mut head = [0u8; 256];
    let n = read_up_to(&mut file, &mut head);
    let head = &head[..n];
    if let Some(line) = head.strip_prefix(b"#!") {
        for interpreter in shebang(line) {
            add_program(needs, &interpreter, library_dirs, depth + 1);
        }
        return;
    }
    #[cfg(target_os = "macos")]
    if path == Path::new("/bin/sh") {
        let shell = resolve(Path::new("/private/var/select/sh"));
        if shell.is_file() {
            add_program(needs, &shell, library_dirs, depth + 1);
        }
    }
    #[cfg(target_os = "linux")]
    if head.starts_with(b"\x7fELF") {
        if let Some(elf) = elf::Dynamic::read(&mut file) {
            if let Some(interpreter) = &elf.interpreter {
                let interpreter = resolve(Path::new(interpreter));
                needs.execute.insert(interpreter);
            }
            add_libraries(needs, path, &elf, library_dirs, depth + 1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = library_dirs;
}

/// The interpreters a `#!` line names: the interpreter itself, and for `env` the program it
/// runs, found through `PATH`. Linux passes the rest of the line as one argument, macOS splits
/// it; both are covered by taking the first word that is not an option or an assignment.
fn shebang(line: &[u8]) -> Vec<PathBuf> {
    let line = line.split(|&b| b == b'\n').next().unwrap_or_default();
    let line = String::from_utf8_lossy(line);
    let mut words = line.split_ascii_whitespace();
    let Some(interpreter) = words.next() else {
        return Vec::new();
    };
    let interpreter = resolve(Path::new(interpreter));
    let mut out = vec![interpreter.clone()];
    if interpreter.file_name().is_some_and(|n| n == "env") {
        if let Some(name) = words.find(|w| !w.starts_with('-') && !w.contains('=')) {
            out.extend(program_paths(name));
        }
    }
    out
}

fn read_up_to(file: &mut File, buffer: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
    }
    filled
}

/// Adds the libraries an ELF object needs that lie outside the library directories, and what
/// they need in turn.
#[cfg(target_os = "linux")]
fn add_libraries(
    needs: &mut Needs,
    object: &Path,
    elf: &elf::Dynamic,
    library_dirs: &[PathBuf],
    depth: usize,
) {
    if depth > DEPTH {
        return;
    }
    let origin = object.parent().unwrap_or(Path::new("/"));
    let search: &[String] = if elf.runpath.is_empty() {
        &elf.rpath
    } else {
        &elf.runpath
    };
    let search: Vec<PathBuf> = search
        .iter()
        .map(|dir| {
            let origin = origin.to_string_lossy();
            PathBuf::from(
                dir.replace("${ORIGIN}", &origin)
                    .replace("$ORIGIN", &origin),
            )
        })
        .collect();
    for name in &elf.needed {
        for library in find_library(name, &search, library_dirs) {
            if library_dirs.iter().any(|dir| library.starts_with(dir)) {
                continue;
            }
            if !needs.read.insert(library.clone()) {
                continue;
            }
            let Ok(mut file) = File::open(&library) else {
                continue;
            };
            if let Some(inner) = elf::Dynamic::read(&mut file) {
                add_libraries(needs, &library, &inner, library_dirs, depth + 1);
            }
        }
    }
}

/// Where the dynamic loader finds a library: a name with a slash is a path; otherwise the
/// search path of the object, then the cache of `ldconfig`, then the library directories.
#[cfg(target_os = "linux")]
fn find_library(name: &str, search: &[PathBuf], library_dirs: &[PathBuf]) -> Vec<PathBuf> {
    if name.contains('/') {
        return vec![resolve(Path::new(name))];
    }
    for dir in search {
        let path = dir.join(name);
        if path.is_file() {
            return vec![resolve(&path)];
        }
    }
    let cached: Vec<PathBuf> = elf::cache()
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, path)| resolve(Path::new(path)))
        .collect();
    if !cached.is_empty() {
        return cached;
    }
    library_dirs
        .iter()
        .map(|dir| dir.join(name))
        .filter(|p| p.is_file())
        .map(|p| resolve(&p))
        .take(1)
        .collect()
}

#[cfg(target_os = "linux")]
mod elf {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::OnceLock;

    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const PT_INTERP: u32 = 3;
    const DT_NEEDED: u64 = 1;
    const DT_STRTAB: u64 = 5;
    const DT_STRSZ: u64 = 10;
    const DT_RPATH: u64 = 15;
    const DT_RUNPATH: u64 = 29;
    /// Bounds on what is read from a file, so that a damaged or hostile file cannot make the
    /// start allocate much.
    const MAX_HEADERS: usize = 512;
    const MAX_TABLE: u64 = 1 << 20;

    /// What the dynamic loader reads from an ELF object.
    #[derive(Debug, Default)]
    pub struct Dynamic {
        pub interpreter: Option<String>,
        pub needed: Vec<String>,
        pub rpath: Vec<String>,
        pub runpath: Vec<String>,
    }

    fn at(file: &mut File, offset: u64, length: u64) -> Option<Vec<u8>> {
        if length > MAX_TABLE {
            return None;
        }
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut buffer = vec![0u8; usize::try_from(length).ok()?];
        file.read_exact(&mut buffer).ok()?;
        Some(buffer)
    }

    fn word(bytes: &[u8], offset: usize, wide: bool) -> Option<u64> {
        if wide {
            let b = bytes.get(offset..offset + 8)?;
            Some(u64::from_le_bytes(b.try_into().ok()?))
        } else {
            let b = bytes.get(offset..offset + 4)?;
            Some(u64::from(u32::from_le_bytes(b.try_into().ok()?)))
        }
    }

    fn half(bytes: &[u8], offset: usize) -> Option<usize> {
        let b = bytes.get(offset..offset + 2)?;
        Some(usize::from(u16::from_le_bytes(b.try_into().ok()?)))
    }

    fn string(table: &[u8], offset: u64) -> Option<String> {
        let rest = table.get(usize::try_from(offset).ok()?..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    struct Segment {
        kind: u32,
        offset: u64,
        address: u64,
        size: u64,
    }

    impl Dynamic {
        /// Reads a little-endian ELF object of 32 or 64 bits; `None` for anything else.
        pub fn read(file: &mut File) -> Option<Dynamic> {
            let header = at(file, 0, 64).or_else(|| at(file, 0, 52))?;
            if !header.starts_with(b"\x7fELF") || header.get(5) != Some(&1) {
                return None;
            }
            let wide = match header.get(4) {
                Some(1) => false,
                Some(2) => true,
                _ => return None,
            };
            let (phoff, phentsize, phnum) = if wide {
                (
                    word(&header, 0x20, true)?,
                    half(&header, 0x36)?,
                    half(&header, 0x38)?,
                )
            } else {
                (
                    word(&header, 0x1C, false)?,
                    half(&header, 0x2A)?,
                    half(&header, 0x2C)?,
                )
            };
            if phnum > MAX_HEADERS || phentsize < if wide { 56 } else { 32 } {
                return None;
            }
            let table = at(file, phoff, (phentsize * phnum) as u64)?;
            let mut segments = Vec::new();
            for i in 0..phnum {
                let e = i * phentsize;
                let kind = u32::try_from(word(&table, e, false)?).ok()?;
                let (offset, address, size) = if wide {
                    (
                        word(&table, e + 8, true)?,
                        word(&table, e + 16, true)?,
                        word(&table, e + 32, true)?,
                    )
                } else {
                    (
                        word(&table, e + 4, false)?,
                        word(&table, e + 8, false)?,
                        word(&table, e + 16, false)?,
                    )
                };
                segments.push(Segment {
                    kind,
                    offset,
                    address,
                    size,
                });
            }
            let mut out = Dynamic::default();
            if let Some(s) = segments.iter().find(|s| s.kind == PT_INTERP) {
                let bytes = at(file, s.offset, s.size)?;
                out.interpreter = string(&bytes, 0);
            }
            let Some(dynamic) = segments.iter().find(|s| s.kind == PT_DYNAMIC) else {
                return Some(out);
            };
            let entries = at(file, dynamic.offset, dynamic.size)?;
            let step = if wide { 16 } else { 8 };
            let mut tags = Vec::new();
            for e in (0..entries.len() / step).map(|i| i * step) {
                let tag = word(&entries, e, wide)?;
                if tag == 0 {
                    break;
                }
                tags.push((tag, word(&entries, e + step / 2, wide)?));
            }
            let value = |wanted: u64| tags.iter().find(|(t, _)| *t == wanted).map(|(_, v)| *v);
            let (Some(strtab), Some(strsz)) = (value(DT_STRTAB), value(DT_STRSZ)) else {
                return Some(out);
            };
            // The string table is named by its address; the loaded segment that holds it
            // gives its place in the file.
            let load = segments.iter().find(|s| {
                s.kind == PT_LOAD
                    && s.address <= strtab
                    && strtab < s.address.saturating_add(s.size)
            })?;
            let strings = at(file, load.offset + (strtab - load.address), strsz)?;
            for (tag, v) in &tags {
                let list = match *tag {
                    DT_NEEDED => {
                        out.needed.extend(string(&strings, *v));
                        continue;
                    }
                    DT_RPATH => &mut out.rpath,
                    DT_RUNPATH => &mut out.runpath,
                    _ => continue,
                };
                if let Some(paths) = string(&strings, *v) {
                    list.extend(paths.split(':').filter(|p| !p.is_empty()).map(String::from));
                }
            }
            Some(out)
        }
    }

    /// The names and paths of `/etc/ld.so.cache` in the format of glibc 2.7 and later
    /// (`glibc-ld.so.cache1.1`); empty when the file is missing or in another format.
    pub fn cache() -> &'static [(String, String)] {
        static CACHE: OnceLock<Vec<(String, String)>> = OnceLock::new();
        CACHE.get_or_init(|| {
            const MAGIC: &[u8] = b"glibc-ld.so.cache1.1";
            let Ok(bytes) = std::fs::read("/etc/ld.so.cache") else {
                return Vec::new();
            };
            let Some(start) = find(&bytes, MAGIC) else {
                return Vec::new();
            };
            let file = &bytes[start..];
            let Some(count) = word(file, MAGIC.len(), false) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for i in 0..usize::try_from(count).unwrap_or(0).min(1 << 16) {
                let e = 48 + 24 * i;
                let (Some(key), Some(value)) = (word(file, e + 4, false), word(file, e + 8, false))
                else {
                    break;
                };
                if let (Some(k), Some(v)) = (string(file, key), string(file, value)) {
                    out.push((k, v));
                }
            }
            out
        })
    }

    /// The new format follows the old one in files written for both.
    fn find(bytes: &[u8], magic: &[u8]) -> Option<usize> {
        bytes.windows(magic.len()).position(|w| w == magic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shebang_names_its_interpreter_and_the_program_of_env() {
        let sh = shebang(b"/bin/sh -e\nexit 0\n");
        assert_eq!(sh, vec![resolve(Path::new("/bin/sh"))]);
        let env = shebang(b"/usr/bin/env -S LANG=C sh -e\n");
        assert_eq!(env[0], resolve(Path::new("/usr/bin/env")));
        assert_eq!(&env[1..], program_paths("sh").as_slice());
        assert!(shebang(b"\n").is_empty());
    }

    /// The shell of the system is an ELF program on Linux: its loader is executable and its
    /// libraries are found, so that the child can start.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_listed_program_brings_its_loader_and_nothing_else_to_execute() {
        let programs: BTreeSet<String> = ["sh".to_string()].into();
        let dirs = vec![resolve(Path::new("/usr/lib")), resolve(Path::new("/lib"))];
        let needs = needs(&programs, &dirs);
        let sh = program_paths("sh");
        assert!(!sh.is_empty());
        for path in &sh {
            assert!(needs.execute.contains(path), "{needs:?}");
        }
        let mut file = File::open(&sh[0]).unwrap();
        if let Some(elf) = elf::Dynamic::read(&mut file) {
            assert!(elf.needed.iter().any(|n| n.starts_with("libc.")), "{elf:?}");
            if let Some(loader) = elf.interpreter {
                assert!(
                    needs.execute.contains(&resolve(Path::new(&loader))),
                    "{needs:?}"
                );
            }
        }
        assert!(needs.execute.len() <= sh.len() * 2, "{needs:?}");
        assert!(!needs.execute.contains(&resolve(Path::new("/bin/cat"))));
    }
}
