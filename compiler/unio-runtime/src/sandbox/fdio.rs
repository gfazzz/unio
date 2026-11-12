//! The system calls of the writer process (docs/runtime.md 9.2) on Linux and macOS: directories
//! opened one component at a time without following links, operations relative to a directory
//! descriptor, and descriptors passed over a Unix socket (SCM_RIGHTS). They are declared here by
//! hand, as in the rest of the runtime, so that the sandbox needs no dependency.

use std::ffi::{CString, OsStr};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use super::beneath::{Anchored, Kind};

extern "C" {
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn openat(dirfd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn renameat(from_dir: c_int, from: *const c_char, to_dir: c_int, to: *const c_char) -> c_int;
    fn unlinkat(dirfd: c_int, path: *const c_char, flags: c_int) -> c_int;
    fn linkat(
        from_dir: c_int,
        from: *const c_char,
        to_dir: c_int,
        to: *const c_char,
        flags: c_int,
    ) -> c_int;
    fn sendmsg(socket: c_int, message: *const MsgHdr, flags: c_int) -> isize;
    fn recvmsg(socket: c_int, message: *mut MsgHdr, flags: c_int) -> isize;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
}

#[cfg(target_os = "linux")]
extern "C" {
    fn mkdirat(dirfd: c_int, path: *const c_char, mode: u32) -> c_int;
}

#[cfg(target_vendor = "apple")]
extern "C" {
    fn mkdirat(dirfd: c_int, path: *const c_char, mode: u16) -> c_int;
    fn renameatx_np(
        from_dir: c_int,
        from: *const c_char,
        to_dir: c_int,
        to: *const c_char,
        flags: u32,
    ) -> c_int;
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod flags {
    pub const O_DIRECTORY: i32 = 0o200000;
    pub const O_NOFOLLOW: i32 = 0o400000;
}
#[cfg(all(target_os = "linux", not(target_arch = "x86_64")))]
mod flags {
    pub const O_DIRECTORY: i32 = 0o40000;
    pub const O_NOFOLLOW: i32 = 0o100000;
}

#[cfg(target_os = "linux")]
const O_CREAT: i32 = 0o100;
#[cfg(target_os = "linux")]
const O_EXCL: i32 = 0o200;
#[cfg(target_os = "linux")]
const O_CLOEXEC: i32 = 0o2000000;
#[cfg(target_os = "linux")]
const O_PATH: i32 = 0o10000000;
#[cfg(target_os = "linux")]
use flags::{O_DIRECTORY, O_NOFOLLOW};
/// A directory opened only to name it, which needs no right to read it.
#[cfg(target_os = "linux")]
const O_DIR_HANDLE: i32 = O_PATH | O_DIRECTORY | O_CLOEXEC;
#[cfg(target_os = "linux")]
const ELOOP: i32 = 40;

#[cfg(target_vendor = "apple")]
const O_CREAT: i32 = 0x200;
#[cfg(target_vendor = "apple")]
const O_EXCL: i32 = 0x800;
#[cfg(target_vendor = "apple")]
const O_NOFOLLOW: i32 = 0x100;
#[cfg(target_vendor = "apple")]
const O_DIRECTORY: i32 = 0x100000;
#[cfg(target_vendor = "apple")]
const O_CLOEXEC: i32 = 0x1000000;
#[cfg(target_vendor = "apple")]
const O_DIR_HANDLE: i32 = O_DIRECTORY | O_CLOEXEC;
#[cfg(target_vendor = "apple")]
const ELOOP: i32 = 62;

const O_RDONLY: i32 = 0;
const ENOTDIR: i32 = 20;
const O_WRONLY: i32 = 1;
const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;

fn c_text(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path contains a zero byte"))
}

fn check(result: c_int) -> io::Result<c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn owned(fd: c_int) -> io::Result<OwnedFd> {
    // SAFETY: a non-negative result of open is a new descriptor that nothing else owns.
    check(fd).map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Whether the error is a link where a directory or a new file was expected.
pub fn is_link_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ELOOP)
}

/// Opens an absolute, link-free directory one component at a time from `/`, refusing every
/// symbolic link on the way (ELOOP), so that a directory replaced by a link between the check
/// of a path and its use cannot lead outside it. The path must have no `.` or `..`.
pub fn open_directory(path: &Path) -> io::Result<OwnedFd> {
    let root = c_text(b"/")?;
    // SAFETY: root ends with 0.
    let mut fd = owned(unsafe { open(root.as_ptr(), O_DIR_HANDLE) })?;
    let mut prefix = std::path::PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => {
                prefix.push(part);
                let name = c_text(part.as_bytes())?;
                // SAFETY: the descriptor is open and name ends with 0.
                let next =
                    unsafe { openat(fd.as_raw_fd(), name.as_ptr(), O_DIR_HANDLE | O_NOFOLLOW) };
                fd = match owned(next) {
                    Ok(next) => next,
                    // With O_DIRECTORY a link is reported as not a directory; the decision is
                    // already made, this only names the reason.
                    Err(e)
                        if e.raw_os_error() == Some(ENOTDIR)
                            && std::fs::symlink_metadata(&prefix)
                                .is_ok_and(|m| m.file_type().is_symlink()) =>
                    {
                        return Err(io::Error::from_raw_os_error(ELOOP))
                    }
                    Err(e) => return Err(e),
                };
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the path is not absolute and normal",
                ))
            }
        }
    }
    Ok(fd)
}

/// What a read-class operation opens at the end of a walk below a scope ([`open_checked`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// A file, for reading its content.
    Read,
    /// A directory, for listing its names.
    Directory,
}

/// Opens a path that the runtime checked, split at its scope ([`super::beneath::anchor`]), so
/// that a link put on any component after the check makes the open fail instead of leading it
/// out of the scope (release condition 5, docs/runtime.md 9.2).
///
/// Linux: the anchor is opened one component at a time from `/` ([`open_directory`]) and the
/// rest relative to it, with `openat2` and `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS |
/// RESOLVE_NO_MAGICLINKS` where the kernel has it (5.6 and later) and otherwise one component
/// at a time with `O_NOFOLLOW` ([`super::beneath::walk`]). macOS: the whole path is opened with
/// `O_NOFOLLOW_ANY` (macOS 11 and later), which fails on a link in any component; the path has
/// no `.` and `..`, so it names exactly the components the check saw.
pub fn open_checked(anchored: &Anchored, want: Want) -> io::Result<OwnedFd> {
    let flags = match want {
        Want::Read => O_RDONLY | O_CLOEXEC,
        Want::Directory => O_RDONLY | O_DIRECTORY | O_CLOEXEC,
    };
    open_with(anchored, flags)
}

/// What the checked path is, without following a link in any component and without opening
/// its content (`exists`, `is_directory`, `resolve`).
#[cfg(target_os = "linux")]
pub fn kind_checked(anchored: &Anchored) -> io::Result<Kind> {
    // O_PATH names the object without opening it, so a device or a pipe is not touched; with
    // O_NOFOLLOW a link as the last name is opened as the link itself, and refused here.
    let file = std::fs::File::from(open_with(anchored, O_PATH | O_CLOEXEC)?);
    let kind = file.metadata()?.file_type();
    if kind.is_symlink() {
        Err(io::Error::from_raw_os_error(ELOOP))
    } else if kind.is_dir() {
        Ok(Kind::Directory)
    } else {
        Ok(Kind::Other)
    }
}

#[cfg(target_os = "linux")]
fn open_with(anchored: &Anchored, flags: i32) -> io::Result<OwnedFd> {
    let anchor = open_directory(&anchored.anchor)?;
    if !anchored.rest.is_empty() {
        let mut rest = std::path::PathBuf::new();
        rest.extend(&anchored.rest);
        if let Some(done) = openat2_beneath(&anchor, rest.as_os_str(), flags | O_NOFOLLOW) {
            return done;
        }
    }
    open_walking(anchored, anchor, flags)
}

/// The walk without `openat2`: one component at a time below the opened anchor.
#[cfg(target_os = "linux")]
fn open_walking(anchored: &Anchored, anchor: OwnedFd, flags: i32) -> io::Result<OwnedFd> {
    let mut steps = Steps {
        last: flags,
        anchor: Some(anchor),
    };
    super::beneath::walk(&mut steps, anchored)
}

/// The steps of [`super::beneath::walk`] with descriptors: the anchor as [`open_directory`]
/// opens it (or as it was already opened), each directory with `O_PATH` and `O_NOFOLLOW`, the
/// last name with the flags of the operation and `O_NOFOLLOW`.
#[cfg(target_os = "linux")]
struct Steps {
    last: i32,
    anchor: Option<OwnedFd>,
}

#[cfg(target_os = "linux")]
impl super::beneath::Beneath for Steps {
    type Handle = OwnedFd;
    fn open_anchor(&mut self, anchor: &Path) -> io::Result<OwnedFd> {
        match self.anchor.take() {
            Some(fd) => Ok(fd),
            None => open_directory(anchor),
        }
    }
    fn open_directory(&mut self, dir: &OwnedFd, name: &std::ffi::OsString) -> io::Result<OwnedFd> {
        open_in(dir, name, O_DIR_HANDLE | O_NOFOLLOW)
    }
    fn open_last(&mut self, dir: &OwnedFd, name: &std::ffi::OsString) -> io::Result<OwnedFd> {
        open_in(dir, name, self.last | O_NOFOLLOW)
    }
    fn reopen_anchor(&mut self, anchor: OwnedFd) -> io::Result<OwnedFd> {
        if self.last & O_PATH != 0 {
            return Ok(anchor);
        }
        // `.` is never a link; the directory is opened again with the rights of the operation.
        open_in(&anchor, OsStr::new("."), self.last)
    }
}

#[cfg(target_os = "linux")]
fn open_in(dir: &OwnedFd, name: &OsStr, flags: i32) -> io::Result<OwnedFd> {
    let name = c_text(name.as_bytes())?;
    // SAFETY: the descriptor is open and name ends with 0.
    owned(unsafe { openat(dir.as_raw_fd(), name.as_ptr(), flags) })
}

/// Whether the kernel lacks `openat2` (Linux before 5.6, or a filter that refuses it); then
/// every open walks one component at a time.
#[cfg(target_os = "linux")]
static NO_OPENAT2: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `openat2` below the directory, refusing every link and every way out of it; `None` where
/// the kernel cannot do it, so that the caller walks instead.
#[cfg(target_os = "linux")]
fn openat2_beneath(dir: &OwnedFd, rest: &OsStr, flags: i32) -> Option<io::Result<OwnedFd>> {
    use std::sync::atomic::Ordering;
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    extern "C" {
        fn syscall(number: i64, ...) -> i64;
    }
    // The same number on every architecture, as for all calls from 424 on.
    const SYS_OPENAT2: i64 = 437;
    const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;
    const ENOSYS: i32 = 38;
    const EPERM: i32 = 1;
    const EINVAL: i32 = 22;
    const E2BIG: i32 = 7;
    const EAGAIN: i32 = 11;
    if NO_OPENAT2.load(Ordering::Relaxed) {
        return None;
    }
    let name = match c_text(rest.as_bytes()) {
        Ok(name) => name,
        Err(e) => return Some(Err(e)),
    };
    let how = OpenHow {
        flags: flags as u32 as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    for _ in 0..8 {
        // SAFETY: the descriptor is open, name ends with 0, and how lives during the call with
        // the size passed.
        let fd = unsafe {
            syscall(
                SYS_OPENAT2,
                dir.as_raw_fd(),
                name.as_ptr(),
                &how as *const OpenHow,
                std::mem::size_of::<OpenHow>(),
            )
        };
        if fd >= 0 {
            // SAFETY: a non-negative result is a new descriptor that nothing else owns.
            return Some(Ok(unsafe { OwnedFd::from_raw_fd(fd as c_int) }));
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            // A rename elsewhere on the file system during the lookup: the kernel asks for a
            // retry.
            Some(EAGAIN) => continue,
            Some(ENOSYS | EPERM | E2BIG) => {
                NO_OPENAT2.store(true, Ordering::Relaxed);
                return None;
            }
            Some(EINVAL) => return None,
            _ => return Some(Err(error)),
        }
    }
    None
}

#[cfg(target_vendor = "apple")]
const O_NOFOLLOW_ANY: i32 = 0x2000_0000;

#[cfg(target_vendor = "apple")]
fn open_with(anchored: &Anchored, flags: i32) -> io::Result<OwnedFd> {
    let path = c_text(checked_path(anchored)?.as_os_str().as_bytes())?;
    // SAFETY: path ends with 0.
    owned(unsafe { open(path.as_ptr(), flags | O_NOFOLLOW | O_NOFOLLOW_ANY) })
}

/// The whole checked path, once the system is known to refuse a link in any component of it.
#[cfg(target_vendor = "apple")]
fn checked_path(anchored: &Anchored) -> io::Result<std::path::PathBuf> {
    nofollow_any()?;
    let mut path = anchored.anchor.clone();
    path.extend(&anchored.rest);
    Ok(path)
}

/// What the checked path is: `fstatat` with `AT_SYMLINK_NOFOLLOW_ANY`, which fails on a link
/// in any component and does not open the object.
#[cfg(target_vendor = "apple")]
pub fn kind_checked(anchored: &Anchored) -> io::Result<Kind> {
    extern "C" {
        #[cfg_attr(target_arch = "x86_64", link_name = "fstatat$INODE64")]
        fn fstatat(dirfd: c_int, path: *const c_char, buf: *mut u8, flags: c_int) -> c_int;
    }
    const AT_FDCWD: c_int = -2;
    const AT_SYMLINK_NOFOLLOW: c_int = 0x20;
    const AT_SYMLINK_NOFOLLOW_ANY: c_int = 0x0800;
    const S_IFMT: u16 = 0o170000;
    const S_IFDIR: u16 = 0o040000;
    const S_IFLNK: u16 = 0o120000;
    let path = c_text(checked_path(anchored)?.as_os_str().as_bytes())?;
    // struct stat (64-bit inodes) is 144 bytes; st_mode is a u16 after the 32-bit st_dev.
    let mut buffer = [0u64; 32];
    // SAFETY: path ends with 0 and the buffer is larger than struct stat.
    check(unsafe {
        fstatat(
            AT_FDCWD,
            path.as_ptr(),
            buffer.as_mut_ptr() as *mut u8,
            AT_SYMLINK_NOFOLLOW | AT_SYMLINK_NOFOLLOW_ANY,
        )
    })?;
    // SAFETY: the offset is inside the buffer.
    let mode =
        unsafe { std::ptr::read_unaligned((buffer.as_ptr() as *const u8).add(4) as *const u16) };
    match mode & S_IFMT {
        S_IFLNK => Err(io::Error::from_raw_os_error(ELOOP)),
        S_IFDIR => Ok(Kind::Directory),
        _ => Ok(Kind::Other),
    }
}

/// `O_NOFOLLOW_ANY` and `AT_SYMLINK_NOFOLLOW_ANY` exist from macOS 11 (Darwin 20); an older
/// kernel would ignore them, so a read below a scope fails there instead of following a link.
#[cfg(target_vendor = "apple")]
fn nofollow_any() -> io::Result<()> {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let supported = *SUPPORTED.get_or_init(|| {
        extern "C" {
            fn uname(name: *mut [[u8; 256]; 5]) -> c_int;
        }
        let mut name = [[0u8; 256]; 5];
        // SAFETY: struct utsname of Darwin is five fields of 256 bytes.
        if unsafe { uname(&mut name) } != 0 {
            return false;
        }
        let release = &name[2];
        let digits: Vec<u8> = release
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .copied()
            .collect();
        std::str::from_utf8(&digits)
            .ok()
            .and_then(|d| d.parse::<u32>().ok())
            .is_some_and(|major| major >= 20)
    });
    if supported {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "reading in a sandbox needs macOS 11 or later (O_NOFOLLOW_ANY)",
        ))
    }
}

/// Calls `each` with the names in a directory opened by [`open_checked`] with
/// [`Want::Directory`], without `.` and `..`, read from the descriptor itself, so that the
/// directory listed is the one opened; stops early when `each` returns false.
pub fn list_names(dir: OwnedFd, each: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()> {
    extern "C" {
        #[cfg_attr(
            all(target_vendor = "apple", target_arch = "x86_64"),
            link_name = "fdopendir$INODE64"
        )]
        fn fdopendir(fd: c_int) -> *mut c_void;
        #[cfg_attr(
            all(target_vendor = "apple", target_arch = "x86_64"),
            link_name = "readdir$INODE64"
        )]
        fn readdir(dir: *mut c_void) -> *const u8;
        fn closedir(dir: *mut c_void) -> c_int;
    }
    #[cfg(target_os = "linux")]
    extern "C" {
        #[link_name = "__errno_location"]
        fn errno() -> *mut c_int;
    }
    #[cfg(target_vendor = "apple")]
    extern "C" {
        #[link_name = "__error"]
        fn errno() -> *mut c_int;
    }
    // struct dirent: on Linux d_ino, d_off (8 bytes each), d_reclen (2), d_type (1); on macOS
    // d_ino, d_seekoff (8 each), d_reclen, d_namlen (2 each), d_type (1); then the name.
    #[cfg(target_os = "linux")]
    const NAME: usize = 19;
    #[cfg(target_vendor = "apple")]
    const NAME: usize = 21;
    use std::os::fd::IntoRawFd;
    let fd = dir.into_raw_fd();
    // SAFETY: the descriptor is open; on success the stream owns it.
    let stream = unsafe { fdopendir(fd) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: the stream did not take the descriptor, which is still ours.
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
        return Err(error);
    }
    let result = loop {
        // SAFETY: errno is the thread's own; it is cleared to tell the end from an error.
        unsafe { *errno() = 0 };
        // SAFETY: the stream is open.
        let entry = unsafe { readdir(stream) };
        if entry.is_null() {
            // SAFETY: as above.
            let code = unsafe { *errno() };
            break if code == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(code))
            };
        }
        // SAFETY: the name of an entry ends with 0 inside the entry.
        let name = unsafe { std::ffi::CStr::from_ptr(entry.add(NAME) as *const c_char) };
        let name = name.to_bytes();
        if name != b"." && name != b".." && !each(OsStr::from_bytes(name)) {
            break Ok(());
        }
    };
    // SAFETY: the stream is open and closes the descriptor with it.
    unsafe { closedir(stream) };
    result
}

/// Creates a new file in the directory and opens it for writing; never follows a link and
/// never opens an existing file.
pub fn create_new(dir: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
    let name = c_text(name.as_bytes())?;
    // SAFETY: the descriptor is open, name ends with 0, and the mode is passed as an int.
    owned(unsafe {
        openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
            0o666 as c_int,
        )
    })
}

/// Replaces `to` with `from`, both in the same directory; the entry is moved, links are not
/// followed.
pub fn replace_in(dir: &OwnedFd, from: &OsStr, to: &OsStr) -> io::Result<()> {
    let (from, to) = (c_text(from.as_bytes())?, c_text(to.as_bytes())?);
    // SAFETY: the descriptor is open and both names end with 0.
    check(unsafe { renameat(dir.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr()) })
        .map(drop)
}

/// Removes a file entry of the directory.
pub fn remove_in(dir: &OwnedFd, name: &OsStr) -> io::Result<()> {
    let name = c_text(name.as_bytes())?;
    // SAFETY: the descriptor is open and name ends with 0.
    check(unsafe { unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) }).map(drop)
}

/// Creates one directory in the directory.
pub fn make_directory_in(dir: &OwnedFd, name: &OsStr) -> io::Result<()> {
    let name = c_text(name.as_bytes())?;
    // SAFETY: the descriptor is open and name ends with 0.
    check(unsafe { mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o777) }).map(drop)
}

/// Syncs the directory: it is opened again through `.`, which is never a link, since a handle
/// opened only to name the directory cannot be synced. `path` names it in the trace of the
/// crash trials (`file_trace`).
///
/// `sync_all` is fsync on Linux and fcntl(F_FULLFSYNC) on Apple platforms, where a plain fsync
/// does not reach storage (S-9a). A file system that refuses F_FULLFSYNC for a directory still
/// gets a plain fsync, which orders the rename, but the error is reported: the rename is not
/// confirmed to survive power loss, and `commit` reports kind DurabilityUnconfirmed (S-9c).
pub fn sync_directory(dir: &OwnedFd, path: &Path) -> io::Result<()> {
    let dot = c_text(b".")?;
    // SAFETY: the descriptor is open and dot ends with 0.
    let fd = owned(unsafe {
        openat(
            dir.as_raw_fd(),
            dot.as_ptr(),
            O_RDONLY | O_DIRECTORY | O_CLOEXEC,
        )
    })?;
    let file = std::fs::File::from(fd);
    let trace = path.as_os_str().as_bytes();
    match file.sync_all() {
        Ok(()) => {
            crate::file_trace::record(
                "fsync-dir",
                &[crate::file_trace::FILE_SYNC.as_bytes(), trace],
            );
            Ok(())
        }
        #[cfg(target_vendor = "apple")]
        Err(full) => {
            extern "C" {
                fn fsync(fd: c_int) -> c_int;
            }
            // SAFETY: the descriptor belongs to `file`, which stays open during the call.
            if unsafe { fsync(file.as_raw_fd()) } == 0 {
                crate::file_trace::record("fsync-dir", &[b"fsync", trace]);
            }
            Err(io::Error::new(
                full.kind(),
                format!("F_FULLFSYNC of the directory failed, so the rename may not reach storage: {full}"),
            ))
        }
        #[cfg(not(target_vendor = "apple"))]
        Err(e) => Err(e),
    }
}

/// Renames without replacing an existing target, as `std.files.rename` promises: renameat2
/// with RENAME_NOREPLACE on Linux, renameatx_np with RENAME_EXCL on macOS. Where neither works,
/// a file is linked under the new name, which fails atomically if the name exists, and then
/// unlinked from the old one; a directory then fails with kind Unsupported.
pub fn rename_without_replacing(
    from_dir: &OwnedFd,
    from: &OsStr,
    to_dir: &OwnedFd,
    to: &OsStr,
) -> io::Result<()> {
    let (from_c, to_c) = (c_text(from.as_bytes())?, c_text(to.as_bytes())?);
    match rename_exclusive(from_dir, &from_c, to_dir, &to_c) {
        Err(e) if no_exclusive_rename(&e) => {}
        done => return done,
    }
    // SAFETY: the descriptor is open and the name ends with 0.
    let entry = owned(unsafe {
        openat(
            from_dir.as_raw_fd(),
            from_c.as_ptr(),
            O_RDONLY | O_NOFOLLOW | O_CLOEXEC | NONBLOCK,
        )
    })?;
    if !std::fs::File::from(entry).metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this file system cannot rename a directory without the risk of replacing a target",
        ));
    }
    // SAFETY: both descriptors are open and both names end with 0.
    let linked = check(unsafe {
        linkat(
            from_dir.as_raw_fd(),
            from_c.as_ptr(),
            to_dir.as_raw_fd(),
            to_c.as_ptr(),
            0,
        )
    });
    match linked {
        Ok(_) => {
            if let Err(e) = remove_in(from_dir, from) {
                let _ = remove_in(to_dir, to);
                return Err(e);
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "this file system cannot rename a file without the risk of replacing a target: {e}"
            ),
        )),
    }
}

#[cfg(target_os = "linux")]
const NONBLOCK: i32 = 0o4000;
#[cfg(target_vendor = "apple")]
const NONBLOCK: i32 = 0x4;

/// renameat2 with RENAME_NOREPLACE, through the system call itself, which needs no
/// particular version of the C library.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn rename_exclusive(
    from_dir: &OwnedFd,
    from: &CString,
    to_dir: &OwnedFd,
    to: &CString,
) -> io::Result<()> {
    extern "C" {
        fn syscall(number: i64, ...) -> i64;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_RENAMEAT2: i64 = 316;
    #[cfg(target_arch = "aarch64")]
    const SYS_RENAMEAT2: i64 = 276;
    const RENAME_NOREPLACE: u32 = 1;
    // SAFETY: both descriptors are open and both names end with 0.
    let result = unsafe {
        syscall(
            SYS_RENAMEAT2,
            from_dir.as_raw_fd(),
            from.as_ptr(),
            to_dir.as_raw_fd(),
            to.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(
    target_os = "linux",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
fn rename_exclusive(_: &OwnedFd, _: &CString, _: &OwnedFd, _: &CString) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(target_vendor = "apple")]
fn rename_exclusive(
    from_dir: &OwnedFd,
    from: &CString,
    to_dir: &OwnedFd,
    to: &CString,
) -> io::Result<()> {
    const RENAME_EXCL: u32 = 0x4;
    // SAFETY: both descriptors are open and both names end with 0.
    check(unsafe {
        renameatx_np(
            from_dir.as_raw_fd(),
            from.as_ptr(),
            to_dir.as_raw_fd(),
            to.as_ptr(),
            RENAME_EXCL,
        )
    })
    .map(drop)
}

/// ENOSYS and EINVAL from renameat2 (EINVAL is also what an unsupported flag gives), ENOTSUP
/// and EOPNOTSUPP from either call.
fn no_exclusive_rename(error: &io::Error) -> bool {
    #[cfg(target_os = "linux")]
    const CODES: &[i32] = &[38, 22, 95];
    #[cfg(target_vendor = "apple")]
    const CODES: &[i32] = &[78, 45, 102];
    error.kind() == io::ErrorKind::Unsupported
        || error
            .raw_os_error()
            .is_some_and(|code| CODES.contains(&code))
}

/// Makes a descriptor survive exec, for the one descriptor a child is meant to inherit.
pub fn keep_on_exec(fd: RawFd) -> io::Result<()> {
    // SAFETY: F_SETFD takes an int argument.
    check(unsafe { fcntl(fd, F_SETFD, 0) }).map(drop)
}

fn close_on_exec(fd: RawFd) {
    // SAFETY: F_SETFD takes an int argument.
    unsafe { fcntl(fd, F_SETFD, FD_CLOEXEC) };
}

// ---------------------------------------------------------------- descriptors over a socket

#[repr(C)]
struct IoVec {
    base: *mut c_void,
    len: usize,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct MsgHdr {
    name: *mut c_void,
    name_len: u32,
    iov: *mut IoVec,
    iov_len: usize,
    control: *mut c_void,
    control_len: usize,
    flags: c_int,
}

#[cfg(target_vendor = "apple")]
#[repr(C)]
struct MsgHdr {
    name: *mut c_void,
    name_len: u32,
    iov: *mut IoVec,
    iov_len: c_int,
    control: *mut c_void,
    control_len: u32,
    flags: c_int,
}

#[cfg(target_os = "linux")]
mod cmsg {
    /// struct cmsghdr: a size_t length, then level and type.
    pub const HEADER: usize = 16;
    pub const SOL_SOCKET: i32 = 1;
    pub const RECV_FLAGS: i32 = 0x4000_0000; // MSG_CMSG_CLOEXEC
    pub type Len = usize;
}

#[cfg(target_vendor = "apple")]
mod cmsg {
    /// struct cmsghdr: a socklen_t length, then level and type.
    pub const HEADER: usize = 12;
    pub const SOL_SOCKET: i32 = 0xffff;
    pub const RECV_FLAGS: i32 = 0;
    pub type Len = u32;
}

const SCM_RIGHTS: i32 = 1;
const MSG_CTRUNC: i32 = if cfg!(target_os = "linux") { 0x8 } else { 0x20 };

/// Sends the bytes; the descriptor, if any, travels with the first byte (SCM_RIGHTS).
pub fn send_with_descriptor(socket: RawFd, data: &[u8], fd: Option<RawFd>) -> io::Result<usize> {
    let mut control = [0u64; 4];
    let mut iov = IoVec {
        base: data.as_ptr() as *mut c_void,
        len: data.len(),
    };
    let mut header = MsgHdr {
        name: std::ptr::null_mut(),
        name_len: 0,
        iov: &mut iov,
        iov_len: 1,
        control: std::ptr::null_mut(),
        control_len: 0,
        flags: 0,
    };
    if let Some(fd) = fd {
        let bytes = control.as_mut_ptr() as *mut u8;
        let length = cmsg::HEADER + 4;
        // SAFETY: control has 32 bytes, more than one header and one int; the writes are
        // unaligned-safe and inside it.
        unsafe {
            std::ptr::write_unaligned(bytes as *mut cmsg::Len, length as cmsg::Len);
            std::ptr::write_unaligned(
                bytes.add(std::mem::size_of::<cmsg::Len>()) as *mut i32,
                cmsg::SOL_SOCKET,
            );
            std::ptr::write_unaligned(
                bytes.add(std::mem::size_of::<cmsg::Len>() + 4) as *mut i32,
                SCM_RIGHTS,
            );
            std::ptr::write_unaligned(bytes.add(cmsg::HEADER) as *mut i32, fd);
        }
        header.control = bytes as *mut c_void;
        // CMSG_SPACE(sizeof(int)): the length rounded up to the alignment of the header.
        let align = std::mem::size_of::<cmsg::Len>();
        header.control_len = (length.div_ceil(align) * align) as _;
    }
    loop {
        // SAFETY: the header, the vector, and the control buffer live during the call.
        let sent = unsafe { sendmsg(socket, &header, 0) };
        if sent >= 0 {
            return Ok(sent as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Receives up to `buffer.len()` bytes and a descriptor that travels with them; the descriptor
/// is closed on exec, so that a child of the program never inherits it.
pub fn receive_with_descriptor(
    socket: RawFd,
    buffer: &mut [u8],
) -> io::Result<(usize, Option<OwnedFd>)> {
    let mut control = [0u64; 8];
    let mut iov = IoVec {
        base: buffer.as_mut_ptr() as *mut c_void,
        len: buffer.len(),
    };
    let mut header = MsgHdr {
        name: std::ptr::null_mut(),
        name_len: 0,
        iov: &mut iov,
        iov_len: 1,
        control: control.as_mut_ptr() as *mut c_void,
        control_len: std::mem::size_of_val(&control) as _,
        flags: 0,
    };
    let received = loop {
        // SAFETY: the header, the vector, and the control buffer live during the call.
        let received = unsafe { recvmsg(socket, &mut header, cmsg::RECV_FLAGS) };
        if received >= 0 {
            break received as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    let mut fds = Vec::new();
    let total = header.control_len as usize;
    let bytes = control.as_ptr() as *const u8;
    let align = std::mem::size_of::<cmsg::Len>();
    let mut offset = 0;
    while offset + cmsg::HEADER <= total {
        // SAFETY: the reads are inside the control buffer, whose length the kernel reported.
        let (length, level, kind) = unsafe {
            (
                std::ptr::read_unaligned(bytes.add(offset) as *const cmsg::Len) as usize,
                std::ptr::read_unaligned(bytes.add(offset + align) as *const i32),
                std::ptr::read_unaligned(bytes.add(offset + align + 4) as *const i32),
            )
        };
        if length < cmsg::HEADER || offset + length > total {
            break;
        }
        if level == cmsg::SOL_SOCKET && kind == SCM_RIGHTS {
            let mut at = offset + cmsg::HEADER;
            while at + 4 <= offset + length {
                // SAFETY: inside the message, as checked above.
                let fd = unsafe { std::ptr::read_unaligned(bytes.add(at) as *const i32) };
                close_on_exec(fd);
                // SAFETY: the kernel installed the descriptor for this process.
                fds.push(unsafe { OwnedFd::from_raw_fd(fd) });
                at += 4;
            }
        }
        offset += length.div_ceil(align) * align;
    }
    if header.flags & MSG_CTRUNC != 0 {
        return Err(io::Error::other("a descriptor was cut off"));
    }
    if fds.len() > 1 {
        return Err(io::Error::other("more than one descriptor arrived"));
    }
    Ok((received, fds.pop()))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Both ways of opening below a scope, `openat2` and the walk of one component at a time,
    /// open the checked entries and refuse a link in any component, the last one included.
    #[test]
    fn opening_below_a_scope_refuses_links_with_and_without_openat2() {
        let dir = std::env::temp_dir().join(format!("unio-fdio-beneath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scope/sub")).unwrap();
        std::fs::create_dir_all(dir.join("outside")).unwrap();
        std::fs::write(dir.join("scope/sub/f.txt"), "inside").unwrap();
        std::fs::write(dir.join("outside/f.txt"), "SECRET").unwrap();
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("scope/dir-link")).unwrap();
        std::os::unix::fs::symlink(dir.join("outside/f.txt"), dir.join("scope/file-link")).unwrap();
        let root = std::fs::canonicalize(dir.join("scope")).unwrap();
        let at = |rest: &[&str]| Anchored {
            anchor: root.clone(),
            rest: rest.iter().map(std::ffi::OsString::from).collect(),
        };
        let read = |fd: OwnedFd| std::io::read_to_string(std::fs::File::from(fd)).unwrap();
        let walking = |rest: &[&str], flags: i32| {
            open_walking(&at(rest), open_directory(&root).unwrap(), flags)
        };
        let file = O_RDONLY | O_CLOEXEC;
        assert_eq!(
            read(open_checked(&at(&["sub", "f.txt"]), Want::Read).unwrap()),
            "inside"
        );
        assert_eq!(read(walking(&["sub", "f.txt"], file).unwrap()), "inside");
        for rest in [&["dir-link", "f.txt"][..], &["file-link"][..]] {
            assert!(open_checked(&at(rest), Want::Read).is_err(), "{rest:?}");
            assert!(walking(rest, file).is_err(), "{rest:?}");
            assert!(kind_checked(&at(rest)).is_err(), "{rest:?}");
            assert!(walking(rest, O_PATH | O_CLOEXEC)
                .map(|fd| std::fs::File::from(fd).metadata().unwrap().is_symlink())
                .unwrap_or(true));
        }
        assert!(open_checked(&at(&["dir-link"]), Want::Directory).is_err());
        assert_eq!(kind_checked(&at(&["sub"])).unwrap(), Kind::Directory);
        assert_eq!(kind_checked(&at(&["sub", "f.txt"])).unwrap(), Kind::Other);
        // The scope itself, from the directory above it, and the root of the file system.
        let above = Anchored {
            anchor: root.parent().unwrap().to_path_buf(),
            rest: vec!["scope".into()],
        };
        let mut names = Vec::new();
        list_names(open_checked(&above, Want::Directory).unwrap(), &mut |n| {
            names.push(n.to_os_string());
            true
        })
        .unwrap();
        names.sort();
        assert_eq!(names, ["dir-link", "file-link", "sub"]);
        let slash = Anchored {
            anchor: PathBuf::from("/"),
            rest: Vec::new(),
        };
        assert_eq!(kind_checked(&slash).unwrap(), Kind::Directory);
        assert!(open_checked(&slash, Want::Directory).is_ok());
        // A link as the anchor itself is refused too.
        let linked = Anchored {
            anchor: dir.join("scope/dir-link"),
            rest: vec!["f.txt".into()],
        };
        assert!(open_checked(&linked, Want::Read).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
