//! External functions of `std.files` (requirements S-9, S-9a, S-9c): reading, directories,
//! renaming without replacing, and the system calls of the atomic commit protocol of
//! `AtomicFile`, which `stdlib/std/files.unio` drives step by step.
//!
//! In a sandbox on Linux and macOS every write-class operation goes to the writer process of
//! the sandbox (`sandbox/writer.rs`, docs/runtime.md 9.2), which checks the write scopes again
//! and works relative to a directory it opened without following links: the program itself has
//! no right to write files. The runtime check comes first in both cases, so a denied operation
//! reports the same error. Without a sandbox, and on Windows, the operations run here.
//!
//! Descriptors are numbers in a table of the runtime. `system_close` removes the descriptor
//! from the table before the operating system reports the result of closing it, so a released
//! descriptor is never closed again even when closing fails.

use super::{int, io_error, io_error_value, os_error, text, ExternFn};
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

/// A path argument, with the memory of the copies the operation makes of it checked against the
/// heap limit first (C-18): the native path, its resolution through the file system, the
/// zero-terminated text for the system call, and the request to the writer process, and for
/// the resolution a part for each component. The copies of a path of ordinary length are a
/// small fixed cost, like the chunk buffer of a read, and are not counted; a longer path whose
/// copies do not fit ends the program with code 102 instead of allocating them past the limit.
fn path(runtime: &Runtime, arguments: &[Value], index: usize) -> Result<Text, Unwind> {
    let p = super::path(runtime, arguments, index)?;
    let text = p.as_str();
    let components = text.bytes().filter(|&b| b == b'/' || b == b'\\').count() + 1;
    let copies = text
        .len()
        .saturating_mul(PATH_COPIES)
        .saturating_add(components.saturating_mul(COMPONENT_BYTES));
    if copies > UNCOUNTED_PATH_BYTES {
        runtime.reserve_heap(copies)?;
    }
    Ok(p)
}

/// The copies of a path an operation may hold at once.
const PATH_COPIES: usize = 4;
/// The memory of one component in the resolution of a path: its entry and its name.
const COMPONENT_BYTES: usize = 64;
/// The copies of a path that are not counted: those of a path of up to 4,096 bytes (PATH_MAX
/// of Linux) with ordinary components.
const UNCOUNTED_PATH_BYTES: usize = 32 * 1024;

/// The data of a Bytes argument, borrowed: writing it needs no copy.
fn bytes_of<'a>(
    runtime: &Runtime,
    arguments: &'a [Value],
    index: usize,
) -> Result<&'a [u8], Unwind> {
    match arguments.get(index) {
        Some(Value::Bytes(b)) => Ok(&b[..]),
        // Any other argument is the internal fault that `bytes` reports for it.
        _ => super::bytes(runtime, arguments, index).map(|_| &[][..]),
    }
}

fn native(p: &Text) -> PathBuf {
    PathBuf::from(p.as_str())
}

fn path_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Records an operation for the crash trials (`file_trace`): absolute paths, then raw data.
fn trace(operation: &str, paths: &[&Path], data: Option<&[u8]>) {
    if !crate::file_trace::enabled() {
        return;
    }
    let mut arguments: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| crate::file_trace::path_bytes(&crate::sandbox::absolute(p)))
        .collect();
    arguments.extend(data.map(<[u8]>::to_vec));
    let arguments: Vec<&[u8]> = arguments.iter().map(Vec::as_slice).collect();
    crate::file_trace::record(operation, &arguments);
}

/// Records an operation on a descriptor of the runtime for the crash trials.
fn trace_descriptor(operation: &str, descriptor: i64, data: &[&[u8]]) {
    if !crate::file_trace::enabled() {
        return;
    }
    let number = descriptor.to_string();
    let mut arguments: Vec<&[u8]> = vec![number.as_bytes()];
    arguments.extend_from_slice(data);
    crate::file_trace::record(operation, &arguments);
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

/// The path a read-class operation uses, once the sandbox permits it (files.read): in a sandbox
/// the path resolved as the system resolves it, so that the operation opens exactly what was
/// checked; without one, the path as given.
fn readable(p: &Text) -> Result<PathBuf, Unwind> {
    crate::sandbox::read_target(p.as_str()).ok_or_else(|| {
        io_error(
            p.as_str(),
            "PermissionDenied",
            crate::sandbox::denied("files.read", p.as_str()),
        )
    })
}

/// The entry a write-class operation changes, once the sandbox permits it (files.write): in a
/// sandbox its directory resolved and its last name as given ([`crate::sandbox::entry_path`]).
fn writable(p: &Text) -> Result<PathBuf, Unwind> {
    crate::sandbox::write_target(p.as_str()).ok_or_else(|| {
        io_error(
            p.as_str(),
            "PermissionDenied",
            crate::sandbox::denied("files.write", p.as_str()),
        )
    })
}

/// The error of an operation the writer did not do, reported for the path of the program.
#[cfg(unix)]
fn writer_error(p: &str, failure: crate::sandbox::writer::Failure) -> Unwind {
    use crate::sandbox::writer::Failure;
    match failure {
        Failure::Refused(reason) => io_error(p, "PermissionDenied", reason),
        Failure::Os(0, reason) => io_error(p, "Other", reason),
        Failure::Os(errno, _) => os_error(p, &std::io::Error::from_raw_os_error(errno)),
        other => io_error(p, "Other", other.to_string()),
    }
}

/// The path the writer is asked about (see [`crate::sandbox::writer::target_path`]).
#[cfg(unix)]
fn writer_path(p: &str) -> Result<PathBuf, Unwind> {
    crate::sandbox::writer::target_path(p)
        .ok_or_else(|| io_error(p, "InvalidInput", "the path does not name a file"))
}

/// Runs `f` with the writer of the sandbox, if the program has one.
#[cfg(unix)]
fn through_writer<R>(
    f: impl FnOnce(&crate::sandbox::writer::Session) -> Result<R, Unwind>,
) -> Option<Result<R, Unwind>> {
    crate::sandbox::with_writer(f)
}

/// Whether the path exists and is accessible.
/// A path the sandbox hides is reported as absent (S-8).
fn exists(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    Ok(Value::Bool(
        crate::sandbox::read_target(p.as_str())
            .is_some_and(|t| crate::sandbox::entry_kind(&t).is_ok()),
    ))
}

/// Whether the path is an accessible directory.
fn is_directory(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    Ok(Value::Bool(
        crate::sandbox::read_target(p.as_str()).is_some_and(|t| {
            crate::sandbox::entry_kind(&t)
                .is_ok_and(|k| k == crate::sandbox::beneath::Kind::Directory)
        }),
    ))
}

/// The whole file as text; kind InvalidData if it is not UTF-8.
fn read_text(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    let target = readable(&p)?;
    let data = read_whole(runtime, &p, &target)?;
    let text = String::from_utf8(data)
        .map_err(|_| io_error(p.as_str(), "InvalidData", "the file is not valid UTF-8"))?;
    let value = Value::text(text);
    runtime.check_heap()?;
    Ok(value)
}

/// The whole file as bytes.
fn read_bytes(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    let target = readable(&p)?;
    let data = read_whole(runtime, &p, &target)?;
    let value = Value::bytes(data);
    runtime.check_heap()?;
    Ok(value)
}

/// The content of a file, with its size checked against the heap limit before the memory is
/// allocated (C-18). A file that grows while it is read is read to its end in steps that are
/// checked the same way.
fn read_whole(runtime: &Runtime, p: &Text, target: &Path) -> Result<Vec<u8>, Unwind> {
    let mut file =
        crate::sandbox::open_for_reading(target).map_err(|e| os_error(p.as_str(), &e))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut data = Vec::new();
    super::reserve(
        runtime,
        &mut data,
        usize::try_from(size).unwrap_or(usize::MAX),
    )?;
    let mut chunk = [0u8; 8192];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => return Ok(data),
            Ok(n) => {
                if data.capacity() - data.len() < n {
                    // The buffer the runtime holds is not counted yet, and growing it may copy
                    // it into a new block before the old one is freed: both are checked.
                    let more = data.len().max(n);
                    runtime.reserve_heap(
                        data.capacity()
                            .saturating_add(data.len())
                            .saturating_add(more),
                    )?;
                    crate::reserve::fallible(|| data.try_reserve_exact(more))
                        .map_err(|_| Unwind::Exhausted(crate::fault::Exhaustion::Heap))?;
                }
                data.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(os_error(p.as_str(), &e)),
        }
    }
}

/// Opens a file for reading; no content is read ahead.
fn system_open_read(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    let target = readable(&p)?;
    let file = crate::sandbox::open_for_reading(&target).map_err(|e| os_error(p.as_str(), &e))?;
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
/// max_bytes. Bytes read before an error are delivered together with it. The buffer of
/// max_bytes is checked against the heap limit before it is allocated, so a max_bytes that does
/// not fit ends the program with code 102 (C-18) instead of an abort.
fn system_read_chunk(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let p = path(runtime, arguments, 1)?;
    let max = usize::try_from(int(runtime, arguments, 2)?)
        .unwrap_or(0)
        .max(1);
    if runtime.descriptor(descriptor).is_none() {
        let problem = io_error_value(p.as_str(), "Other", "the descriptor is not open");
        return Ok(read_step(Vec::new(), false, Some(problem)));
    }
    let mut buffer = super::buffer(runtime, max)?;
    let file = runtime
        .descriptor(descriptor)
        .expect("the descriptor was checked above");
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(read_step(Vec::new(), true, None)),
            Ok(n) => {
                buffer.truncate(n);
                buffer.shrink_to_fit();
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
    let target = readable(&p)?;
    let resolved =
        crate::sandbox::resolve_existing(&target).map_err(|e| os_error(p.as_str(), &e))?;
    Ok(Value::path(path_text(&resolved)))
}

/// The entries of a directory, sorted by name, without "." and "..". Names that are not valid
/// Unicode are written with replacement characters.
fn list_directory(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    let target = readable(&p)?;
    let directory = native(&p);
    // The names are held before the list is made, so each is checked against the heap limit
    // as it is read (C-18): its text, its place in the list of names, and room for that list
    // to grow, which may copy it.
    let mut names: Vec<String> = Vec::new();
    let mut held = 0usize;
    let mut exhausted = None;
    crate::sandbox::list_directory(&target, &mut |name| {
        let name = name.to_string_lossy().into_owned();
        held = held.saturating_add(name.len() + 3 * std::mem::size_of::<String>());
        if let Err(unwind) = runtime.reserve_heap(held) {
            exhausted = Some(unwind);
            return false;
        }
        names.push(name);
        true
    })
    .map_err(|e| os_error(p.as_str(), &e))?;
    if let Some(unwind) = exhausted {
        return Err(unwind);
    }
    names.sort();
    // Each entry of the list is a path below the directory, which the heap counts when it is
    // made; the names are still held until then.
    let per_entry = directory.as_os_str().len() + 1 + ENTRY_BYTES;
    runtime.reserve_heap(held.saturating_add(names.len().saturating_mul(per_entry)))?;
    let entries = names
        .into_iter()
        .map(|name| Value::path(path_text(&directory.join(name))))
        .collect();
    let list = Value::list(entries);
    runtime.check_heap()?;
    Ok(list)
}

/// The memory of one entry of a listed directory besides its text: the value in the list and
/// the storage of the text.
const ENTRY_BYTES: usize = 96;

/// Gives a file or directory a new path and fails with AlreadyExists instead of replacing an
/// existing target. The check and the rename are one step of the operating system:
/// renameat2 with RENAME_NOREPLACE on Linux, renamex_np with RENAME_EXCL on macOS, and
/// MoveFileExW without MOVEFILE_REPLACE_EXISTING on Windows. Where the system or the file
/// system has no such step, a file is linked under the new name, which fails atomically when
/// the name exists, and then unlinked from the old one; a directory then fails with kind
/// Unsupported rather than racing with another process that could create the target.
fn rename(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let source = path(runtime, arguments, 0)?;
    let target = path(runtime, arguments, 1)?;
    let from = writable(&source)?;
    let to = writable(&target)?;
    #[cfg(unix)]
    if let Some(done) = through_writer(|w| {
        let (from, to) = (writer_path(source.as_str())?, writer_path(target.as_str())?);
        w.handle()
            .rename(&from, &to)
            .map_err(|f| writer_error(target.as_str(), f))
    }) {
        return done.map(|()| Value::Unit);
    }
    crate::sandbox::writing(|| {
        #[cfg(windows)]
        if let Some(done) = crate::sandbox::rename_checked(&from, &to, false) {
            return done;
        }
        rename_without_replacing(&from, &to)
    })
    .map_err(|e| os_error(target.as_str(), &e))?;
    trace("rename-noreplace", &[&from, &to], None);
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
    match rename_exclusive(source, target) {
        Err(e) if no_exclusive_rename(&e) => {}
        done => return done,
    }
    if !fs::symlink_metadata(source)?.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "this file system cannot rename a directory without the risk of replacing a target",
        ));
    }
    match fs::hard_link(source, target) {
        Ok(()) => {
            if let Err(e) = fs::remove_file(source) {
                let _ = fs::remove_file(target);
                return Err(e);
            }
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(e),
        Err(e) => Err(std::io::Error::new(
            ErrorKind::Unsupported,
            format!(
                "this file system cannot rename a file without the risk of replacing a target: {e}"
            ),
        )),
    }
}

#[cfg(unix)]
fn c_path(p: &Path) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(ErrorKind::InvalidInput, "the path contains a zero byte"))
}

/// renameat2(AT_FDCWD, source, AT_FDCWD, target, RENAME_NOREPLACE), through the system call
/// itself, which needs no particular version of the C library.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn rename_exclusive(source: &Path, target: &Path) -> std::io::Result<()> {
    extern "C" {
        fn syscall(number: i64, ...) -> i64;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_RENAMEAT2: i64 = 316;
    #[cfg(target_arch = "aarch64")]
    const SYS_RENAMEAT2: i64 = 276;
    const AT_FDCWD: i32 = -100;
    const RENAME_NOREPLACE: u32 = 1;
    let (from, to) = (c_path(source)?, c_path(target)?);
    // SAFETY: both paths end with 0 and live during the call.
    let result = unsafe {
        syscall(
            SYS_RENAMEAT2,
            AT_FDCWD,
            from.as_ptr(),
            AT_FDCWD,
            to.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// renamex_np(source, target, RENAME_EXCL).
#[cfg(target_vendor = "apple")]
fn rename_exclusive(source: &Path, target: &Path) -> std::io::Result<()> {
    extern "C" {
        fn renamex_np(
            from: *const std::os::raw::c_char,
            to: *const std::os::raw::c_char,
            flags: u32,
        ) -> i32;
    }
    const RENAME_EXCL: u32 = 0x4;
    let (from, to) = (c_path(source)?, c_path(target)?);
    // SAFETY: both paths end with 0 and live during the call.
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(
    windows,
    target_vendor = "apple",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
fn rename_exclusive(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(ErrorKind::Unsupported))
}

/// Whether the error says that the system or the file system has no exclusive rename: ENOSYS
/// and EINVAL from renameat2 (EINVAL is also what an unsupported flag gives), ENOTSUP and
/// EOPNOTSUPP from either call.
#[cfg(not(windows))]
fn no_exclusive_rename(error: &std::io::Error) -> bool {
    #[cfg(target_os = "linux")]
    const CODES: &[i32] = &[38, 22, 95];
    #[cfg(target_vendor = "apple")]
    const CODES: &[i32] = &[78, 45, 102];
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    const CODES: &[i32] = &[];
    error.kind() == ErrorKind::Unsupported
        || error
            .raw_os_error()
            .is_some_and(|code| CODES.contains(&code))
}

/// Creates one directory; AlreadyExists if it exists, NotFound if its parent does not.
fn create_directory(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    let target = writable(&p)?;
    #[cfg(unix)]
    if let Some(done) = through_writer(|w| {
        w.handle()
            .make_directory(&writer_path(p.as_str())?)
            .map_err(|f| writer_error(p.as_str(), f))
    }) {
        return done.map(|()| Value::Unit);
    }
    crate::sandbox::writing(|| {
        #[cfg(windows)]
        if let Some(done) = crate::sandbox::make_directory_checked(&target) {
            return done;
        }
        fs::create_dir(&target)
    })
    .map_err(|e| os_error(p.as_str(), &e))?;
    trace("mkdir", &[&target], None);
    // The entry of the new directory is synced, so that a durable write into it survives power
    // loss (S-9, docs/runtime.md 10). The directory exists either way, so a failed sync is not
    // reported here: it is remembered, and a durable commit below the directory syncs the
    // parent again first or reports kind DurabilityUnconfirmed. Windows has no sync of a
    // directory (NTFS journals the entry).
    #[cfg(unix)]
    {
        let created = crate::sandbox::resolve(&target);
        if let Some(parent) = created.parent() {
            let synced = if super::durability::parent_sync_fails(false) {
                Err(super::durability::injected_sync_failure())
            } else {
                sync_directory(parent)
            };
            if synced.is_err() {
                let parent = parent.to_path_buf();
                UNSYNCED.with(|u| u.borrow_mut().record(created, parent));
            }
        }
    }
    Ok(Value::Unit)
}

/// Whether the file system that holds the path keeps the guarantees of S-9 (S-9a). In a
/// sandbox the writer answers for a target it may write, since it is the one that writes it and
/// the program may not read the table of mounts.
fn system_supports_durability(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let p = path(runtime, arguments, 0)?;
    #[cfg(unix)]
    if crate::sandbox::may_write(p.as_str()) {
        if let Some(answer) = through_writer(|w| {
            let target = writer_path(p.as_str())?;
            Ok(w.handle().durable(&target).unwrap_or(false))
        }) {
            return answer.map(Value::Bool);
        }
    }
    Ok(Value::Bool(crate::sandbox::writing(|| {
        super::durability::supports_durability(&native(&p))
    })))
}

/// Creates .<name>.unio-tmp-<id> next to the target and opens it for writing.
fn system_open_temporary(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let target = path(runtime, arguments, 0)?;
    let target_path = writable(&target)?;
    #[cfg(unix)]
    if let Some(opened) = through_writer(|w| {
        w.open(&writer_path(target.as_str())?)
            .map_err(|f| writer_error(target.as_str(), f))
    }) {
        let (temporary, file) = opened?;
        let descriptor = runtime.add_descriptor(file);
        trace_descriptor(
            "open",
            descriptor,
            &[&crate::file_trace::path_bytes(&temporary)],
        );
        return Ok(Value::record(
            "TemporaryFile",
            vec![
                ("descriptor", Value::Int(descriptor)),
                ("path", Value::path(path_text(&temporary))),
            ],
        ));
    }
    let directory = match target_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = directory.join(format!(".{name}.unio-tmp-{}", runtime.next_temporary_id()));
    let file = crate::sandbox::writing(|| {
        #[cfg(windows)]
        if let Some(done) = crate::sandbox::create_new_checked(&temporary) {
            return done;
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
    })
    .map_err(|e| os_error(target.as_str(), &e))?;
    trace("create", &[&temporary], None);
    let descriptor = runtime.add_descriptor(file);
    trace_descriptor(
        "open",
        descriptor,
        &[&crate::file_trace::path_bytes(&crate::sandbox::absolute(
            &temporary,
        ))],
    );
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
    trace_descriptor("write", descriptor, &[data]);
    Ok(Value::Unit)
}

fn system_write(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let t = text(runtime, arguments, 1)?;
    write_all(runtime, descriptor, t.as_str().as_bytes())
}

fn system_write_bytes(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let descriptor = int(runtime, arguments, 0)?;
    let data = bytes_of(runtime, arguments, 1)?;
    write_all(runtime, descriptor, data)
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
    trace_descriptor(
        "fsync",
        descriptor,
        &[crate::file_trace::FILE_SYNC.as_bytes()],
    );
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
    trace_descriptor("close", descriptor, &[]);
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
    let from = crate::sandbox::temporary_target(temporary.as_str()).ok_or_else(|| {
        io_error(
            temporary.as_str(),
            "PermissionDenied",
            crate::sandbox::denied("files.write", temporary.as_str()),
        )
    })?;
    let to = writable(&target)?;
    if runtime.fails_at(FailPoint::Rename) {
        return Err(injected(target.as_str()));
    }
    #[cfg(unix)]
    if let Some(done) = through_writer(|w| {
        w.commit(temporary.as_str(), &writer_path(target.as_str())?)
            .map_err(|f| writer_error(target.as_str(), f))
    }) {
        return done.map(|()| Value::Unit);
    }
    crate::sandbox::writing(|| {
        #[cfg(windows)]
        if let Some(done) = crate::sandbox::rename_checked(&from, &to, true) {
            return done;
        }
        replace(&from, &to)
    })
    .map_err(|e| os_error(target.as_str(), &e))?;
    // MOVEFILE_WRITE_THROUGH returns after the rename is on the disk (S-9a).
    let operation = if cfg!(windows) {
        "rename-write-through"
    } else {
        "rename"
    };
    trace(operation, &[&from, &to], None);
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
    #[cfg(unix)]
    if let Some(done) = through_writer(|w| {
        let dir = crate::sandbox::resolve(&crate::sandbox::absolute(&native(&directory)));
        w.sync_directory(&dir)
            .map_err(|f| writer_error(directory.as_str(), f))
    }) {
        return done.map(|()| Value::Unit);
    }
    let dir = match crate::sandbox::read_target(directory.as_str()) {
        Some(dir) => dir,
        None => crate::sandbox::resolve(&native(&directory)),
    };
    UNSYNCED
        .with(|u| {
            u.borrow_mut()
                .settle(&crate::sandbox::resolve(&dir), &mut |parent| {
                    if super::durability::parent_sync_fails(true) {
                        Err(super::durability::injected_sync_failure())
                    } else {
                        sync_directory(parent)
                    }
                })
        })
        .map_err(|e| os_error(directory.as_str(), &e))?;
    sync_directory(&dir).map_err(|e| os_error(directory.as_str(), &e))?;
    Ok(Value::Unit)
}

thread_local! {
    /// The new directories of this thread whose parent could not be synced, when the runtime
    /// creates them itself (no writer process).
    static UNSYNCED: std::cell::RefCell<super::durability::UnsyncedParents> =
        std::cell::RefCell::default();
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    File::open(directory)?.sync_all()?;
    trace_directory_sync(crate::file_trace::FILE_SYNC, directory);
    Ok(())
}

/// On Apple platforms `sync_all` is fcntl(F_FULLFSYNC). Where a file system refuses it for a
/// directory, a plain fsync of the directory still orders the rename, but it does not reach
/// storage, so the error is reported: `commit` then fails with kind DurabilityUnconfirmed.
#[cfg(target_vendor = "apple")]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn fsync(fd: i32) -> i32;
    }
    let file = File::open(directory)?;
    let full = match file.sync_all() {
        Ok(()) => {
            trace_directory_sync(crate::file_trace::FILE_SYNC, directory);
            return Ok(());
        }
        Err(e) => e,
    };
    // SAFETY: the descriptor belongs to `file`, which stays open during the call.
    if unsafe { fsync(file.as_raw_fd()) } == 0 {
        trace_directory_sync("fsync", directory);
    }
    Err(std::io::Error::new(
        full.kind(),
        format!("F_FULLFSYNC of the directory failed, so the rename may not reach storage: {full}"),
    ))
}

/// Records a sync of a directory and the call that did it, for the crash trials.
#[cfg(unix)]
fn trace_directory_sync(call: &str, directory: &Path) {
    if crate::file_trace::enabled() {
        let path = crate::file_trace::path_bytes(&crate::sandbox::absolute(directory));
        crate::file_trace::record("fsync-dir", &[call.as_bytes(), &path]);
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
    #[cfg(unix)]
    if through_writer(|w| {
        w.abort(p.as_str());
        Ok(())
    })
    .is_some()
    {
        return Ok(Value::Unit);
    }
    if let Some(entry) = crate::sandbox::temporary_target(p.as_str()) {
        let removed = crate::sandbox::writing(|| {
            #[cfg(windows)]
            if let Some(done) = crate::sandbox::remove_checked(&entry) {
                return done;
            }
            fs::remove_file(&entry)
        });
        if removed.is_ok() {
            trace("unlink", &[&entry], None);
        }
    }
    Ok(Value::Unit)
}
