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
/// opened only to name the directory cannot be synced.
pub fn sync_directory(dir: &OwnedFd) -> io::Result<()> {
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
    // sync_all is fsync on Linux and F_FULLFSYNC on Apple platforms, where a file system that
    // refuses it for a directory still orders the rename with a plain fsync.
    match file.sync_all() {
        Ok(()) => Ok(()),
        #[cfg(target_vendor = "apple")]
        Err(_) => {
            extern "C" {
                fn fsync(fd: c_int) -> c_int;
            }
            // SAFETY: the descriptor belongs to `file`, which stays open during the call.
            check(unsafe { fsync(file.as_raw_fd()) }).map(drop)
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
