//! External functions of `std.files` (requirements S-9, S-9a, S-9c): reading, directories,
//! renaming without replacing, and the system calls of the atomic commit protocol of
//! `AtomicFile`, which `stdlib/std/files.unio` drives step by step.
//!
//! Descriptors are numbers in a table of the runtime. `system_close` removes the descriptor
//! from the table before the operating system reports the result of closing it, so a released
//! descriptor is never closed again even when closing fails.

use super::{bytes, int, io_error, io_error_value, os_error, path, text, ExternFn};
use crate::fault::{Outcome, Unwind};
use crate::runtime::{FailPoint, Runtime};
use crate::value::{Text, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

pub const BINDINGS: &[(&str, ExternFn)] = &[
    ("std.files.exists", exists),
    ("std.files.is_directory", is_directory),
    ("std.files.read_text", read_text),
    ("std.files.system_open_read", system_open_read),
    ("std.files.system_read_chunk", system_read_chunk),
    ("std.files.resolve", resolve),
    ("std.files.list_directory", list_directory),
    ("std.files.read_bytes", read_bytes),
    ("std.files.rename", rename),
    ("std.files.create_directory", create_directory),
    (
        "std.files.system_supports_durability",
        system_supports_durability,
    ),
    ("std.files.system_open_temporary", system_open_temporary),
    ("std.files.system_write", system_write),
    ("std.files.system_write_bytes", system_write_bytes),
    ("std.files.system_sync_data", system_sync_data),
    ("std.files.system_close", system_close),
    ("std.files.system_rename", system_rename),
    ("std.files.system_sync_directory", system_sync_directory),
    ("std.files.system_remove", system_remove),
];

fn native(p: &Text) -> PathBuf {
    PathBuf::from(p.as_str())
}

fn path_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn injected(p: &str) -> Unwind {
    io_error(p, "Other", "injected failure")
}

/// The error of an operation the sandbox denies (SB-6): kind PermissionDenied, naming the
/// permission that is missing.
fn permit(allowed: bool, permission: &str, p: &str) -> Result<(), Unwind> {
    if allowed {
        Ok(())
    } else {
        Err(io_error(
            p,
            "PermissionDenied",
            crate::sandbox::denied(permission, p),
        ))
    }
}

fn permit_read(p: &Text) -> Result<(), Unwind> {
    permit(
        crate::sandbox::may_read(p.as_str()),
        "files.read",
        p.as_str(),
    )
}

fn permit_write(p: &Text) -> Result<(), Unwind> {
    permit(
        crate::sandbox::may_write(p.as_str()),
        "files.write",
        p.as_str(),
    )
}

/// Whether the path exists and is accessible.
/// A path the sandbox hides is reported as absent (S-8).
fn exists(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    Ok(Value::Bool(
        crate::sandbox::may_read(p.as_str()) && fs::metadata(native(&p)).is_ok(),
    ))
}

/// Whether the path is an accessible directory.
fn is_directory(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    Ok(Value::Bool(
        crate::sandbox::may_read(p.as_str()) && fs::metadata(native(&p)).is_ok_and(|m| m.is_dir()),
    ))
}

/// The whole file as text; kind InvalidData if it is not UTF-8.
fn read_text(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    permit_read(&p)?;
    let data = fs::read(native(&p)).map_err(|e| os_error(p.as_str(), &e))?;
    let text = String::from_utf8(data)
        .map_err(|_| io_error(p.as_str(), "InvalidData", "the file is not valid UTF-8"))?;
    let value = Value::text(text);
    runtime.check_heap()?;
    Ok(value)
}

/// The whole file as bytes.
fn read_bytes(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    permit_read(&p)?;
    let data = fs::read(native(&p)).map_err(|e| os_error(p.as_str(), &e))?;
    let value = Value::bytes(data);
    runtime.check_heap()?;
    Ok(value)
}

/// Opens a file for reading; no content is read ahead.
fn system_open_read(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    permit_read(&p)?;
    let file = File::open(native(&p)).map_err(|e| os_error(p.as_str(), &e))?;
    Ok(Value::Int(runtime.add_descriptor(file)))
}

fn read_step(data: Vec<u8>, end: bool, problem: Option<Value>) -> Value {
    Value::record(
        "ReadStep",
        vec![
            ("data", Value::bytes(data)),
            ("end", Value::Bool(end)),
            ("problem", Value::option(problem)),
        ],
    )
}

/// Blocks until data, the end, or an error; retries interrupted reads; never returns more than
/// max_bytes. Bytes read before an error are delivered together with it.
fn system_read_chunk(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let p = path(runtime, arguments, 1)?;
    let max = usize::try_from(int(runtime, arguments, 2)?)
        .unwrap_or(0)
        .max(1);
    let file = match runtime.descriptor(descriptor) {
        Some(f) => f,
        None => {
            let problem = io_error_value(p.as_str(), "Other", "the descriptor is not open");
            return Ok(read_step(Vec::new(), false, Some(problem)));
        }
    };
    let mut buffer = vec![0u8; max];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(read_step(Vec::new(), true, None)),
            Ok(n) => {
                buffer.truncate(n);
                return Ok(read_step(buffer, false, None));
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                let problem = io_error_value(p.as_str(), super::io_kind(&e), e.to_string());
                return Ok(read_step(Vec::new(), false, Some(problem)));
            }
        }
    }
}

/// Resolves an existing path through the file system, including links.
fn resolve(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    permit_read(&p)?;
    let resolved = fs::canonicalize(native(&p)).map_err(|e| os_error(p.as_str(), &e))?;
    Ok(Value::path(path_text(&resolved)))
}

/// The entries of a directory, sorted by name, without "." and "..". Names that are not valid
/// Unicode are written with replacement characters.
fn list_directory(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    permit_read(&p)?;
    let directory = native(&p);
    let mut names: Vec<String> = fs::read_dir(&directory)
        .map_err(|e| os_error(p.as_str(), &e))?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()
        .map_err(|e| os_error(p.as_str(), &e))?;
    names.sort();
    let entries = names
        .into_iter()
        .map(|name| Value::path(path_text(&directory.join(name))))
        .collect();
    Ok(Value::list(entries))
}

/// Gives a file or directory a new path and fails with AlreadyExists instead of replacing an
/// existing target. A file is renamed by linking the new name, which fails atomically when it
/// exists, and then unlinking the old one; a directory, or a file system without links, is
/// checked and then renamed, which leaves a short window in which another process could create
/// the target.
fn rename(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let source = path(runtime, arguments, 0)?;
    let target = path(runtime, arguments, 1)?;
    permit_write(&source)?;
    permit_write(&target)?;
    rename_without_replacing(&native(&source), &native(&target))
        .map_err(|e| os_error(target.as_str(), &e))?;
    Ok(Value::Unit)
}

#[cfg(windows)]
fn rename_without_replacing(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both strings end with 0; flags 0 never replace an existing target.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn rename_without_replacing(source: &Path, target: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_file() {
        match fs::hard_link(source, target) {
            Ok(()) => {
                if let Err(e) = fs::remove_file(source) {
                    let _ = fs::remove_file(target);
                    return Err(e);
                }
                return Ok(());
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {}
        }
    }
    if fs::symlink_metadata(target).is_ok() {
        return Err(std::io::Error::new(
            ErrorKind::AlreadyExists,
            "the target already exists",
        ));
    }
    fs::rename(source, target)
}

/// Creates one directory; AlreadyExists if it exists, NotFound if its parent does not.
fn create_directory(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    permit_write(&p)?;
    fs::create_dir(native(&p)).map_err(|e| os_error(p.as_str(), &e))?;
    Ok(Value::Unit)
}

/// Whether the file system that holds the path keeps the guarantees of S-9 (S-9a).
fn system_supports_durability(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    Ok(Value::Bool(super::durability::supports_durability(
        &native(&p),
    )))
}

/// Creates .<name>.unio-tmp-<id> next to the target and opens it for writing.
fn system_open_temporary(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let target = path(runtime, arguments, 0)?;
    permit_write(&target)?;
    let target_path = native(&target);
    let directory = match target_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = directory.join(format!(".{name}.unio-tmp-{}", runtime.next_temporary_id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| os_error(target.as_str(), &e))?;
    let descriptor = runtime.add_descriptor(file);
    Ok(Value::record(
        "TemporaryFile",
        vec![
            ("descriptor", Value::Int(descriptor)),
            ("path", Value::path(path_text(&temporary))),
        ],
    ))
}

fn write_all(runtime: &mut Runtime, descriptor: i64, data: &[u8]) -> Outcome {
    let label = format!("descriptor {descriptor}");
    if runtime.fails_at(FailPoint::Write) {
        return Err(injected(&label));
    }
    let file = runtime
        .descriptor(descriptor)
        .ok_or_else(|| io_error(&label, "Other", "the descriptor is not open"))?;
    if let Some(limit) = crate::sandbox::file_size_limit() {
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        if size.saturating_add(data.len() as u64) > limit {
            return Err(io_error(
                &label,
                "Other",
                format!("the policy limits written files to {limit} bytes"),
            ));
        }
    }
    file.write_all(data).map_err(|e| os_error(&label, &e))?;
    Ok(Value::Unit)
}

fn system_write(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let t = text(runtime, arguments, 1)?;
    write_all(runtime, descriptor, t.as_str().as_bytes())
}

fn system_write_bytes(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let data = bytes(runtime, arguments, 1)?;
    write_all(runtime, descriptor, &data)
}

/// Syncs the data of the file to storage: `File::sync_all` is fsync on Linux,
/// fcntl(F_FULLFSYNC) on Apple platforms, and FlushFileBuffers on Windows.
fn system_sync_data(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let label = format!("descriptor {descriptor}");
    if runtime.fails_at(FailPoint::SyncData) {
        return Err(injected(&label));
    }
    let file = runtime
        .descriptor(descriptor)
        .ok_or_else(|| io_error(&label, "Other", "the descriptor is not open"))?;
    file.sync_all().map_err(|e| os_error(&label, &e))?;
    Ok(Value::Unit)
}

/// Releases the descriptor even when closing reports an error; a released descriptor is
/// never closed again, and closing one that is not open is an error that closes nothing.
fn system_close(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let label = format!("descriptor {descriptor}");
    let file = runtime
        .release_descriptor(descriptor)
        .ok_or_else(|| io_error(&label, "Other", "the descriptor is not open"))?;
    let closed = close(file);
    if runtime.fails_at(FailPoint::Close) {
        return Err(injected(&label));
    }
    closed.map_err(|e| os_error(&label, &e))?;
    Ok(Value::Unit)
}

/// Closes the file and reports the result of closing, which dropping a `File` hides.
#[cfg(unix)]
fn close(file: File) -> std::io::Result<()> {
    use std::os::unix::io::IntoRawFd;
    extern "C" {
        fn close(fd: i32) -> i32;
    }
    let fd = file.into_raw_fd();
    // SAFETY: the descriptor came from a File that no longer owns it; it is closed once.
    if unsafe { close(fd) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn close(file: File) -> std::io::Result<()> {
    drop(file);
    Ok(())
}

/// Replaces the target with the temporary file: rename on Linux and macOS, MoveFileExW with
/// MOVEFILE_REPLACE_EXISTING and MOVEFILE_WRITE_THROUGH on Windows.
fn system_rename(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let temporary = path(runtime, arguments, 0)?;
    let target = path(runtime, arguments, 1)?;
    permit(
        crate::sandbox::may_write_temporary(temporary.as_str()),
        "files.write",
        temporary.as_str(),
    )?;
    permit_write(&target)?;
    if runtime.fails_at(FailPoint::Rename) {
        return Err(injected(target.as_str()));
    }
    replace(&native(&temporary), &native(&target)).map_err(|e| os_error(target.as_str(), &e))?;
    Ok(Value::Unit)
}

#[cfg(windows)]
fn replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both strings end with 0.
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(temporary, target)
}

/// Syncs the directory, so that the rename survives power loss: fsync of the directory on Linux
/// and macOS; on Windows MOVEFILE_WRITE_THROUGH has done this already.
fn system_sync_directory(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let directory = path(runtime, arguments, 0)?;
    permit(
        crate::sandbox::may_sync_directory(directory.as_str()),
        "files.write",
        directory.as_str(),
    )?;
    if runtime.fails_at(FailPoint::SyncDirectory) {
        return Err(injected(directory.as_str()));
    }
    sync_directory(&native(&directory)).map_err(|e| os_error(directory.as_str(), &e))?;
    Ok(Value::Unit)
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    File::open(directory)?.sync_all()
}

/// On Apple platforms `sync_all` is fcntl(F_FULLFSYNC); where a file system refuses it for a
/// directory, a plain fsync of the directory still orders the rename.
#[cfg(target_vendor = "apple")]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn fsync(fd: i32) -> i32;
    }
    let file = File::open(directory)?;
    if file.sync_all().is_ok() {
        return Ok(());
    }
    // SAFETY: the descriptor belongs to `file`, which stays open during the call.
    if unsafe { fsync(file.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn sync_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Removes the file; errors are ignored.
fn system_remove(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    if crate::sandbox::may_write_temporary(p.as_str()) {
        let _ = fs::remove_file(native(&p));
    }
    Ok(Value::Unit)
}
