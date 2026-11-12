//! The sandbox of Windows (requirements SB-1, SB-4, SB-10; docs/runtime.md 9.4): AppContainers
//! with a fresh SID for every run, access to files granted to that SID on exactly the scopes,
//! and Job Objects.
//!
//! - **Containers.** A [`Container`] is an AppContainer of its own, named after this process
//!   and a random token, so no two runs share a SID. It has no capability: no network, no
//!   library of the user. A process in it, and every process that process starts, runs with a
//!   token whose second access check sees only the SID of the container (and the groups of all
//!   application packages), so it can open only what an access control entry grants to that
//!   SID, or to all application packages, as the files of the system do.
//! - **Grants.** [`Container::grant`] adds an entry for the SID to the DACL of each scope: a
//!   tree is granted on its root with inheritance and the entry is propagated to everything
//!   below, a file or a directory alone on itself. The entries are removed again when the
//!   container is dropped. Propagation does not follow reparse points (links and junctions), so
//!   a link inside a scope does not carry the grant to its target, and it does not enter a
//!   directory whose DACL does not inherit. Changes of the DACLs are serialised between the
//!   processes of the user by a named mutex, since runs share directories (the standard library).
//! - **Programs** run on a thread of the tool or the executable, as on Linux: that thread
//!   impersonates a token of the container (a lowbox token), so its own opens are checked
//!   against the grants, and every child is created in the container. Writes of the runtime
//!   switch the thread to the token of a second container that holds the write grants (the
//!   writer of docs/runtime.md 9.2), so neither the program's own token nor its children can
//!   write.
//! - **Workers** of the tools are processes created in a container of their own.
//!
//! Processes are created here rather than by `std::process`, which cannot pass the attribute of
//! the container: [`spawn`] creates them suspended with `CreateProcessW`, the security
//! capabilities of the container, and exactly the standard handles inherited.

#![allow(clippy::upper_case_acronyms)]

use std::ffi::{c_void, OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, Ordering};

type HANDLE = *mut c_void;
type BOOL = i32;
type PSID = *mut c_void;

#[repr(C)]
struct ACL {
    revision: u8,
    sbz1: u8,
    size: u16,
    count: u16,
    sbz2: u16,
}

#[repr(C)]
struct AceHeader {
    kind: u8,
    flags: u8,
    size: u16,
}

#[repr(C)]
struct AllowedAce {
    header: AceHeader,
    mask: u32,
    sid_start: u32,
}

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: BOOL,
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_chars: u32,
    y_chars: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved2_size: u16,
    reserved2: *mut u8,
    stdin: HANDLE,
    stdout: HANDLE,
    stderr: HANDLE,
}

#[repr(C)]
struct StartupInfoEx {
    info: StartupInfo,
    attributes: *mut c_void,
}

#[repr(C)]
struct ProcessInformation {
    process: HANDLE,
    thread: HANDLE,
    pid: u32,
    tid: u32,
}

#[repr(C)]
struct SidAndAttributes {
    sid: PSID,
    attributes: u32,
}

#[repr(C)]
struct SecurityCapabilities {
    container: PSID,
    capabilities: *mut SidAndAttributes,
    count: u32,
    reserved: u32,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *mut c_void,
    attributes: u32,
    descriptor: *mut c_void,
    quality: *mut c_void,
}

#[link(name = "kernel32")]
extern "system" {
    fn CloseHandle(handle: HANDLE) -> BOOL;
    // As declared in mod.rs: a pseudo handle.
    fn GetCurrentProcess() -> isize;
    fn GetCurrentProcessId() -> u32;
    fn CreatePipe(
        read: *mut HANDLE,
        write: *mut HANDLE,
        attributes: *mut SecurityAttributes,
        size: u32,
    ) -> BOOL;
    fn SetHandleInformation(handle: HANDLE, mask: u32, flags: u32) -> BOOL;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: *mut SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: HANDLE,
    ) -> HANDLE;
    fn InitializeProcThreadAttributeList(
        list: *mut c_void,
        count: u32,
        flags: u32,
        size: *mut usize,
    ) -> BOOL;
    fn UpdateProcThreadAttribute(
        list: *mut c_void,
        flags: u32,
        attribute: usize,
        value: *mut c_void,
        size: usize,
        previous: *mut c_void,
        returned: *mut usize,
    ) -> BOOL;
    fn DeleteProcThreadAttributeList(list: *mut c_void);
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut SecurityAttributes,
        thread_attributes: *mut SecurityAttributes,
        inherit: BOOL,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *mut StartupInfoEx,
        information: *mut ProcessInformation,
    ) -> BOOL;
    fn ResumeThread(thread: HANDLE) -> u32;
    fn TerminateProcess(process: HANDLE, code: u32) -> BOOL;
    fn WaitForSingleObject(handle: HANDLE, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: HANDLE, code: *mut u32) -> BOOL;
    fn CreateMutexW(attributes: *mut SecurityAttributes, owner: BOOL, name: *const u16) -> HANDLE;
    fn ReleaseMutex(mutex: HANDLE) -> BOOL;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

#[link(name = "advapi32")]
extern "system" {
    fn FreeSid(sid: PSID) -> *mut c_void;
    fn GetLengthSid(sid: PSID) -> u32;
    fn EqualSid(a: PSID, b: PSID) -> BOOL;
    fn OpenProcessToken(process: HANDLE, access: u32, token: *mut HANDLE) -> BOOL;
    fn OpenThreadToken(thread: HANDLE, access: u32, as_self: BOOL, token: *mut HANDLE) -> BOOL;
    fn DuplicateTokenEx(
        token: HANDLE,
        access: u32,
        attributes: *mut SecurityAttributes,
        level: i32,
        kind: i32,
        new: *mut HANDLE,
    ) -> BOOL;
    fn SetThreadToken(thread: *mut HANDLE, token: HANDLE) -> BOOL;
    fn RevertToSelf() -> BOOL;
    fn GetTokenInformation(
        token: HANDLE,
        class: i32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> BOOL;
    fn GetSecurityInfo(
        handle: HANDLE,
        kind: i32,
        information: u32,
        owner: *mut PSID,
        group: *mut PSID,
        dacl: *mut *mut ACL,
        sacl: *mut *mut ACL,
        descriptor: *mut *mut c_void,
    ) -> u32;
    fn GetSecurityDescriptorControl(
        descriptor: *mut c_void,
        control: *mut u16,
        revision: *mut u32,
    ) -> BOOL;
    fn InitializeSecurityDescriptor(descriptor: *mut c_void, revision: u32) -> BOOL;
    fn SetSecurityDescriptorDacl(
        descriptor: *mut c_void,
        present: BOOL,
        dacl: *mut ACL,
        defaulted: BOOL,
    ) -> BOOL;
    fn SetSecurityDescriptorControl(descriptor: *mut c_void, mask: u16, bits: u16) -> BOOL;
    fn SetKernelObjectSecurity(handle: HANDLE, information: u32, descriptor: *mut c_void) -> BOOL;
    fn InitializeAcl(acl: *mut ACL, length: u32, revision: u32) -> BOOL;
    fn GetAce(acl: *mut ACL, index: u32, ace: *mut *mut c_void) -> BOOL;
    fn AddAce(acl: *mut ACL, revision: u32, start: u32, list: *mut c_void, length: u32) -> BOOL;
    fn AddAccessAllowedAceEx(
        acl: *mut ACL,
        revision: u32,
        flags: u32,
        mask: u32,
        sid: PSID,
    ) -> BOOL;
}

#[link(name = "userenv")]
extern "system" {
    fn CreateAppContainerProfile(
        name: *const u16,
        display: *const u16,
        description: *const u16,
        capabilities: *mut SidAndAttributes,
        count: u32,
        sid: *mut PSID,
    ) -> i32;
    fn DeleteAppContainerProfile(name: *const u16) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtCreateLowBoxToken(
        token: *mut HANDLE,
        existing: HANDLE,
        access: u32,
        attributes: *mut ObjectAttributes,
        package: PSID,
        capability_count: u32,
        capabilities: *mut SidAndAttributes,
        handle_count: u32,
        handles: *mut HANDLE,
    ) -> i32;
}

const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
const INFINITE: u32 = u32::MAX;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_ABANDONED: u32 = 0x80;
const HANDLE_FLAG_INHERIT: u32 = 1;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_SHARE_ALL: u32 = 0x7;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const CREATE_SUSPENDED: u32 = 0x4;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
const STARTF_USESTDHANDLES: u32 = 0x100;
const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
const PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES: usize = 0x0002_0009;

const TOKEN_ASSIGN_PRIMARY: u32 = 0x1;
const TOKEN_DUPLICATE: u32 = 0x2;
const TOKEN_IMPERSONATE: u32 = 0x4;
const TOKEN_QUERY: u32 = 0x8;
const TOKEN_ADJUST_DEFAULT: u32 = 0x80;
const TOKEN_ALL_ACCESS: u32 = 0x000F_01FF;
const SECURITY_IMPERSONATION: i32 = 2;
const TOKEN_IMPERSONATION: i32 = 2;
const TOKEN_IMPERSONATION_LEVEL: i32 = 9;
const TOKEN_IS_APP_CONTAINER: i32 = 29;

const SE_FILE_OBJECT: i32 = 1;
const DACL_SECURITY_INFORMATION: u32 = 4;
const SE_DACL_AUTO_INHERITED: u16 = 0x0400;
const SE_DACL_PROTECTED: u16 = 0x1000;
const ACL_REVISION: u32 = 2;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const OBJECT_INHERIT_ACE: u8 = 0x1;
const CONTAINER_INHERIT_ACE: u8 = 0x2;
const NO_PROPAGATE_INHERIT_ACE: u8 = 0x4;
const INHERIT_ONLY_ACE: u8 = 0x8;
const INHERITED_ACE: u8 = 0x10;

// Rights of files and directories.
const FILE_ADD_FILE: u32 = 0x2;
const FILE_ADD_SUBDIRECTORY: u32 = 0x4;
const FILE_TRAVERSE: u32 = 0x20;
const FILE_DELETE_CHILD: u32 = 0x40;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const SYNCHRONIZE: u32 = 0x0010_0000;
/// FILE_GENERIC_READ: data, attributes, extended attributes, the security descriptor.
const READ_FILE: u32 = 0x0012_0089;
/// FILE_GENERIC_READ and FILE_GENERIC_EXECUTE.
const READ_EXECUTE: u32 = 0x0012_00A9;
/// FILE_GENERIC_WRITE.
const WRITE_FILE: u32 = 0x0012_0116;
/// Reading, writing, and deleting, without changing the security descriptor or the owner.
const MODIFY: u32 = 0x0013_01FF;

/// The NTSTATUS of success.
const STATUS_SUCCESS: i32 = 0;

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

fn last_error(what: &str) -> io::Error {
    let e = io::Error::last_os_error();
    io::Error::new(e.kind(), format!("{what}: {e}"))
}

/// A token handle, closed when dropped.
pub struct Token(HANDLE);

// SAFETY: a token handle may be used from any thread.
unsafe impl Send for Token {}
unsafe impl Sync for Token {}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// ---------------------------------------------------------------- the lock of the grants

/// Serialises changes of DACLs between the processes of the user: a grant reads a DACL, adds an
/// entry and writes it back, which must not interleave with another run doing the same on a
/// shared directory.
struct AclLock(HANDLE);

impl AclLock {
    fn take() -> io::Result<AclLock> {
        let name = wide(OsStr::new("Local\\unio-sandbox-grants"));
        // SAFETY: a named mutex with default security; the name ends with 0.
        unsafe {
            let mutex = CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr());
            if mutex.is_null() {
                return Err(last_error("the lock of the grants cannot be created"));
            }
            // An abandoned mutex is owned now: its holder ended, and every DACL it wrote was
            // written as a whole.
            let waited = WaitForSingleObject(mutex, INFINITE);
            if waited != WAIT_OBJECT_0 && waited != WAIT_ABANDONED {
                CloseHandle(mutex);
                return Err(last_error("the lock of the grants cannot be taken"));
            }
            Ok(AclLock(mutex))
        }
    }
}

impl Drop for AclLock {
    fn drop(&mut self) {
        // SAFETY: the mutex is owned by this thread and the handle by this value.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

// ---------------------------------------------------------------- containers

/// What a grant permits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Reading a file, or a directory and everything in it: listing and reading, not executing.
    Read,
    /// Reading and executing one file: a program a child may start.
    Execute,
    /// Reading, writing, creating, and removing a directory and everything in it.
    Modify,
    /// Writing the data of one existing file in place (`unio fmt`).
    WriteFile,
    /// Creating, replacing, and removing entries of one directory, without reading or listing
    /// it: the writer of a scope of one file in that directory (docs/runtime.md 9.2).
    Entries,
    /// Entering one directory, without listing it: the working directory of a process.
    Enter,
}

/// One entry: its inheritance flags and its rights.
type Entry = (u8, u32);

impl Access {
    /// The entries on the root of the grant; `subtree` grants inherit to everything below.
    fn entries(self, subtree: bool) -> Vec<Entry> {
        let inherit = if subtree {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        };
        match self {
            // Directories may be entered and listed; files read but not executed.
            Access::Read if subtree => vec![
                (CONTAINER_INHERIT_ACE, READ_EXECUTE),
                (OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE, READ_FILE),
            ],
            Access::Read => vec![(0, READ_FILE)],
            Access::Execute => vec![(0, READ_EXECUTE)],
            Access::Modify => vec![(inherit, MODIFY)],
            Access::WriteFile => vec![(0, READ_FILE | WRITE_FILE)],
            // A file or directory the writer creates in the directory (a temporary file of a
            // commit) inherits the rights to write it: the access of a new file is checked
            // against the entries it inherits.
            Access::Entries => vec![
                (
                    0,
                    FILE_ADD_FILE
                        | FILE_ADD_SUBDIRECTORY
                        | FILE_DELETE_CHILD
                        | FILE_TRAVERSE
                        | FILE_READ_ATTRIBUTES
                        | SYNCHRONIZE,
                ),
                (
                    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE,
                    MODIFY,
                ),
            ],
            Access::Enter => vec![(0, FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)],
        }
    }
}

/// The entries a child inherits from the entries of its directory, by the rules of NTFS.
fn inherit(entries: &[Entry], directory: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    for &(flags, mask) in entries {
        if directory {
            if flags & NO_PROPAGATE_INHERIT_ACE != 0 {
                if flags & CONTAINER_INHERIT_ACE != 0 {
                    out.push((INHERITED_ACE, mask));
                }
            } else if flags & CONTAINER_INHERIT_ACE != 0 {
                out.push((
                    INHERITED_ACE | (flags & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)),
                    mask,
                ));
            } else if flags & OBJECT_INHERIT_ACE != 0 {
                out.push((INHERITED_ACE | OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE, mask));
            }
        } else if flags & OBJECT_INHERIT_ACE != 0 {
            out.push((INHERITED_ACE, mask));
        }
    }
    out
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A random token for the name of a container.
fn random_token() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::SeqCst));
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    hasher.finish()
}

/// One grant that a container holds, removed when the container is dropped.
struct Grant {
    path: PathBuf,
    subtree: bool,
    /// The entries of the directory may have inherited the grant (`Access::Entries`): they are
    /// cleaned too, one level deep.
    entries: bool,
}

/// An AppContainer of its own, with no capability, and the grants it holds.
pub struct Container {
    name: String,
    sid: PSID,
    grants: Vec<Grant>,
}

// SAFETY: the SID is owned by the container and only read.
unsafe impl Send for Container {}
unsafe impl Sync for Container {}

impl Container {
    /// A new container with a fresh SID: its name is that of this process, a counter, and a
    /// random token, so that no other run has the same SID.
    pub fn new() -> io::Result<Container> {
        // SAFETY: no arguments.
        let pid = unsafe { GetCurrentProcessId() };
        let name = format!("unio.{pid}.{:016x}", random_token());
        let wide_name = wide(OsStr::new(&name));
        let mut sid: PSID = std::ptr::null_mut();
        // SAFETY: the strings end with 0; no capabilities; the SID is freed with FreeSid.
        let result = unsafe {
            CreateAppContainerProfile(
                wide_name.as_ptr(),
                wide_name.as_ptr(),
                wide_name.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut sid,
            )
        };
        if result != 0 || sid.is_null() {
            return Err(io::Error::other(format!(
                "the AppContainer cannot be created (HRESULT 0x{result:08x})"
            )));
        }
        Ok(Container {
            name,
            sid,
            grants: Vec::new(),
        })
    }

    /// The name of the container.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Grants the container `access` to the path, with everything below it when `subtree`.
    /// A path that does not exist is skipped: it grants nothing, as a scope whose root is
    /// missing grants nothing on the other systems. The entry is removed when the container is
    /// dropped.
    pub fn grant(&mut self, path: &Path, access: Access, subtree: bool) -> io::Result<()> {
        let path = verbatim(path);
        if std::fs::symlink_metadata(&path).is_err() {
            return Ok(());
        }
        let subtree = subtree && std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        let root_entries = if subtree {
            access.entries(true)
        } else if access == Access::Read
            && std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir())
        {
            // A directory alone may be listed and entered.
            vec![(0, READ_EXECUTE)]
        } else {
            access.entries(false)
        };
        let _lock = AclLock::take()?;
        self.grants.push(Grant {
            path: path.clone(),
            subtree,
            entries: access == Access::Entries,
        });
        // SAFETY: the SID is valid for the life of the container.
        unsafe { add_entries(&path, self.sid, &root_entries) }
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        if subtree {
            propagate(&path, self.sid, &root_entries, &mut |dir, sid, entries| {
                // SAFETY: as above.
                unsafe { add_entries(dir, sid, entries) }
            });
        }
        Ok(())
    }

    /// Removes every grant of the container.
    fn revoke(&mut self) {
        if self.grants.is_empty() {
            return;
        }
        let Ok(_lock) = AclLock::take() else {
            return;
        };
        for grant in std::mem::take(&mut self.grants) {
            // SAFETY: the SID is valid for the life of the container.
            let _ = unsafe { remove_entries(&grant.path, self.sid) };
            if grant.subtree {
                propagate(&grant.path, self.sid, &[], &mut |path, sid, _| {
                    // SAFETY: as above.
                    unsafe { remove_entries(path, sid) }
                });
            } else if grant.entries {
                for entry in std::fs::read_dir(&grant.path)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    // SAFETY: as above.
                    let _ = unsafe { remove_entries(&entry.path(), self.sid) };
                }
            }
        }
    }

    /// A token of the container made from the token of this process: a primary token when
    /// `primary`, else one a thread can impersonate.
    pub fn token(&self, primary: bool) -> io::Result<Token> {
        // SAFETY: the process token is opened and closed here; the attributes are zeroed with
        // their length set, as NtCreateLowBoxToken expects; the SID is valid.
        unsafe {
            let mut process: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(
                GetCurrentProcess() as HANDLE,
                TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
                &mut process,
            ) == 0
            {
                return Err(last_error("the token of the process cannot be opened"));
            }
            let process = Token(process);
            let mut attributes = ObjectAttributes {
                length: std::mem::size_of::<ObjectAttributes>() as u32,
                root: std::ptr::null_mut(),
                name: std::ptr::null_mut(),
                attributes: 0,
                descriptor: std::ptr::null_mut(),
                quality: std::ptr::null_mut(),
            };
            let mut lowbox: HANDLE = std::ptr::null_mut();
            let status = NtCreateLowBoxToken(
                &mut lowbox,
                process.0,
                TOKEN_ALL_ACCESS,
                &mut attributes,
                self.sid,
                0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            );
            if status != STATUS_SUCCESS {
                return Err(io::Error::other(format!(
                    "the token of the AppContainer cannot be made (NTSTATUS 0x{status:08x})"
                )));
            }
            let lowbox = Token(lowbox);
            if primary {
                return Ok(lowbox);
            }
            let mut impersonation: HANDLE = std::ptr::null_mut();
            if DuplicateTokenEx(
                lowbox.0,
                TOKEN_IMPERSONATE | TOKEN_QUERY | TOKEN_DUPLICATE,
                std::ptr::null_mut(),
                SECURITY_IMPERSONATION,
                TOKEN_IMPERSONATION,
                &mut impersonation,
            ) == 0
            {
                return Err(last_error(
                    "the token of the AppContainer cannot be duplicated",
                ));
            }
            Ok(Token(impersonation))
        }
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        self.revoke();
        let name = wide(OsStr::new(&self.name));
        // SAFETY: the SID came from CreateAppContainerProfile; the name ends with 0. Deleting
        // the profile removes its folder and its keys.
        unsafe {
            DeleteAppContainerProfile(name.as_ptr());
            FreeSid(self.sid);
        }
    }
}

/// The path in the form `\\?\C:\...`, which has no length limit and which `canonicalize`
/// returns; a path that cannot be made absolute is kept.
fn verbatim(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Opens the entry itself, not the target of a link, to read and write its DACL.
fn open_for_dacl(path: &Path) -> io::Result<OwnedHandle> {
    let name = wide(path.as_os_str());
    // SAFETY: the name ends with 0; the handle is owned by the result.
    unsafe {
        let handle = CreateFileW(
            name.as_ptr(),
            READ_CONTROL | WRITE_DAC,
            FILE_SHARE_ALL,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedHandle::from_raw_handle(handle))
    }
}

/// The DACL of an open entry and its control bits; `None` for a NULL DACL, which grants
/// everything to everyone and so needs no entry.
struct Dacl {
    descriptor: *mut c_void,
    acl: *mut ACL,
    control: u16,
}

impl Drop for Dacl {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by GetSecurityInfo.
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}

unsafe fn read_dacl(handle: HANDLE) -> io::Result<Dacl> {
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: *mut c_void = std::ptr::null_mut();
    let result = GetSecurityInfo(
        handle,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut acl,
        std::ptr::null_mut(),
        &mut descriptor,
    );
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let mut control = 0u16;
    let mut revision = 0u32;
    GetSecurityDescriptorControl(descriptor, &mut control, &mut revision);
    Ok(Dacl {
        descriptor,
        acl,
        control,
    })
}

/// Whether the entry is an allowed entry for the SID.
unsafe fn is_entry_of(ace: *mut c_void, sid: PSID) -> bool {
    let header = &*(ace as *const AceHeader);
    if header.kind != ACCESS_ALLOWED_ACE_TYPE {
        return false;
    }
    let allowed = ace as *mut AllowedAce;
    let ace_sid = std::ptr::addr_of_mut!((*allowed).sid_start) as PSID;
    EqualSid(ace_sid, sid) != 0
}

/// The DACL with `entries` added for the SID, or, without entries, with every entry of the SID
/// removed. Explicit entries go after the explicit denied ones and inherited entries at the end,
/// as the canonical order asks; an entry the DACL already has is not added again, so grants of
/// one container on one object add up (a tree to read that is also the working directory).
/// `None` when nothing changes.
unsafe fn rebuilt(old: *mut ACL, sid: PSID, entries: &[Entry]) -> Option<Vec<u64>> {
    let count = (*old).count as u32;
    let mut kept: Vec<*mut c_void> = Vec::new();
    let mut removed = false;
    let mut present: Vec<Entry> = Vec::new();
    for i in 0..count {
        let mut ace: *mut c_void = std::ptr::null_mut();
        if GetAce(old, i, &mut ace) == 0 {
            return None;
        }
        if is_entry_of(ace, sid) {
            if entries.is_empty() {
                removed = true;
                continue;
            }
            let allowed = &*(ace as *const AllowedAce);
            present.push((allowed.header.flags, allowed.mask));
        }
        kept.push(ace);
    }
    let entries: Vec<Entry> = entries
        .iter()
        .copied()
        .filter(|e| !present.contains(e))
        .collect();
    let entries = entries.as_slice();
    if !removed && entries.is_empty() {
        return None;
    }
    let sid_length = GetLengthSid(sid);
    let mut size = std::mem::size_of::<ACL>() as u32;
    for &ace in &kept {
        size += (*(ace as *const AceHeader)).size as u32;
    }
    size += entries.len() as u32 * (8 + sid_length);
    let size = (size + 7) & !7;
    let mut buffer = vec![0u64; size as usize / 8];
    let acl = buffer.as_mut_ptr() as *mut ACL;
    let revision = u32::from((*old).revision).max(ACL_REVISION);
    if InitializeAcl(acl, size, revision) == 0 {
        return None;
    }
    let explicit = |ace: *mut c_void| (*(ace as *const AceHeader)).flags & INHERITED_ACE == 0;
    let denied = |ace: *mut c_void| (*(ace as *const AceHeader)).kind == ACCESS_DENIED_ACE_TYPE;
    let mut index = 0;
    // Explicit denied entries first.
    while index < kept.len() && explicit(kept[index]) && denied(kept[index]) {
        let ace = kept[index];
        AddAce(
            acl,
            revision,
            u32::MAX,
            ace,
            (*(ace as *const AceHeader)).size as u32,
        );
        index += 1;
    }
    for &(flags, mask) in entries.iter().filter(|(f, _)| f & INHERITED_ACE == 0) {
        AddAccessAllowedAceEx(acl, revision, u32::from(flags), mask, sid);
    }
    for &ace in &kept[index..] {
        AddAce(
            acl,
            revision,
            u32::MAX,
            ace,
            (*(ace as *const AceHeader)).size as u32,
        );
    }
    for &(flags, mask) in entries.iter().filter(|(f, _)| f & INHERITED_ACE != 0) {
        AddAccessAllowedAceEx(acl, revision, u32::from(flags), mask, sid);
    }
    Some(buffer)
}

/// Writes a DACL to an open entry with its control bits, without propagating it: the caller
/// walks the tree itself.
unsafe fn write_dacl(handle: HANDLE, acl: *mut ACL, control: u16) -> io::Result<()> {
    let mut descriptor = [0u64; 8];
    let sd = descriptor.as_mut_ptr() as *mut c_void;
    if InitializeSecurityDescriptor(sd, 1) == 0 || SetSecurityDescriptorDacl(sd, 1, acl, 0) == 0 {
        return Err(io::Error::last_os_error());
    }
    let bits = control & (SE_DACL_AUTO_INHERITED | SE_DACL_PROTECTED);
    SetSecurityDescriptorControl(sd, SE_DACL_AUTO_INHERITED | SE_DACL_PROTECTED, bits);
    if SetKernelObjectSecurity(handle, DACL_SECURITY_INFORMATION, sd) == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// What one step of a walk did with an entry: its children are walked too. A DACL that does
/// not inherit (protected, or with explicit entries only, as files of the temporary directory
/// often have) still gets the entry of the grant: the access check reads every entry whatever
/// its flags, so a scope covers everything below its root, as on the other systems.
enum Step {
    Continue,
}

unsafe fn change_entries(path: &Path, sid: PSID, entries: &[Entry]) -> io::Result<Step> {
    let handle = open_for_dacl(path)?;
    let raw = std::os::windows::io::AsRawHandle::as_raw_handle(&handle);
    let dacl = read_dacl(raw)?;
    if !dacl.acl.is_null() {
        if let Some(mut acl) = rebuilt(dacl.acl, sid, entries) {
            write_dacl(raw, acl.as_mut_ptr() as *mut ACL, dacl.control)?;
        }
    }
    Ok(Step::Continue)
}

unsafe fn add_entries(path: &Path, sid: PSID, entries: &[Entry]) -> io::Result<Step> {
    change_entries(path, sid, entries)
}

unsafe fn remove_entries(path: &Path, sid: PSID) -> io::Result<Step> {
    change_entries(path, sid, &[])
}

/// One step of a walk: an entry, the SID, and the entries it gets.
type StepFn<'a> = dyn FnMut(&Path, PSID, &[Entry]) -> io::Result<Step> + 'a;

/// Applies `step` to everything below the directory, depth first, with the entries each child
/// inherits. A reparse point (a link or a junction) is neither changed nor entered, so a grant
/// never reaches outside the tree. Errors of single entries are skipped: an entry that cannot
/// be changed is one the container cannot open, and a directory whose DACL cannot be read is
/// not entered.
fn propagate(dir: &Path, sid: PSID, entries: &[Entry], step: &mut StepFn) {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in listing.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() || is_reparse_point(&entry) {
            continue;
        }
        let path = entry.path();
        let is_dir = kind.is_dir();
        let mine = if entries.is_empty() {
            Vec::new()
        } else {
            inherit(entries, is_dir)
        };
        if !entries.is_empty() && mine.is_empty() {
            continue;
        }
        let stepped = step(&path, sid, &mine);
        if is_dir && matches!(stepped, Ok(Step::Continue)) {
            propagate(&path, sid, &mine, step);
        }
    }
}

fn is_reparse_point(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    entry
        .metadata()
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

// ---------------------------------------------------------------- canonical paths

/// The volumes of the drive letters, as the NT names of their devices, read outside any
/// AppContainer: inside one the mount manager, which turns a device into a drive letter, cannot
/// be reached.
static VOLUMES: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

/// The drive letters and the devices of their volumes (`C:` and `\Device\HarddiskVolume3`), as
/// this process sees them.
pub fn volumes() -> Vec<(String, String)> {
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryDosDeviceW(name: *const u16, target: *mut u16, length: u32) -> u32;
    }
    let mut found = Vec::new();
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", letter as char);
        let name = wide(OsStr::new(&drive));
        let mut buffer = vec![0u16; 1024];
        // SAFETY: the name ends with 0; the buffer holds 1024 units.
        let n = unsafe { QueryDosDeviceW(name.as_ptr(), buffer.as_mut_ptr(), 1024) } as usize;
        if n > 0 {
            // The first of the targets, which end with 0 each.
            let end = buffer[..n].iter().position(|&c| c == 0).unwrap_or(n);
            found.push((drive, String::from_utf16_lossy(&buffer[..end])));
        }
    }
    found
}

/// Records the volumes for [`canonicalize`]: those of this process, before it enters an
/// AppContainer, or those the tool gave a worker. The first record stays.
pub fn remember_volumes(table: Option<Vec<(String, String)>>) {
    let _ = VOLUMES.get_or_init(|| table.unwrap_or_else(volumes));
}

/// The path as the file system resolves it, as `std::fs::canonicalize` gives it: absolute, in
/// the form `\\?\C:\...`, with links resolved and the names normalized. Inside an AppContainer
/// the drive letter of the result cannot be had from the system (the mount manager is out of
/// reach), so the normalized name is taken with the NT name of its volume and the volume is
/// turned into its drive letter by the table of [`remember_volumes`]; a volume without a letter
/// in it fails.
pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let nt = final_name(path)?;
            // A process that recorded no table (outside an AppContainer) reads its own now.
            let table = VOLUMES.get_or_init(volumes).as_slice();
            for (drive, device) in table {
                if let Some(rest) = nt.strip_prefix(device.as_str()) {
                    if rest.is_empty() || rest.starts_with('\\') {
                        return Ok(PathBuf::from(format!("\\\\?\\{drive}{rest}")));
                    }
                }
            }
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("the volume of {nt} has no known drive letter"),
            ))
        }
        done => done,
    }
}

/// The normalized name of the file with the NT name of its volume.
fn final_name(path: &Path) -> io::Result<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFinalPathNameByHandleW(file: HANDLE, path: *mut u16, length: u32, flags: u32) -> u32;
    }
    const VOLUME_NAME_NT: u32 = 0x2;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let name = wide(absolute.as_os_str());
    // SAFETY: the name ends with 0; the handle is owned by `file` and closed when dropped; the
    // buffer holds `buffer.len()` units, and the call writes at most that many.
    unsafe {
        let handle = CreateFileW(
            name.as_ptr(),
            0,
            FILE_SHARE_ALL,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = OwnedHandle::from_raw_handle(handle);
        let raw = std::os::windows::io::AsRawHandle::as_raw_handle(&file);
        let mut buffer = vec![0u16; 512];
        loop {
            let n = GetFinalPathNameByHandleW(
                raw,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                VOLUME_NAME_NT,
            ) as usize;
            if n == 0 {
                return Err(io::Error::last_os_error());
            }
            if n < buffer.len() {
                return Ok(String::from_utf16_lossy(&buffer[..n]));
            }
            buffer = vec![0u16; n + 1];
        }
    }
}

/// The name of the file system that holds the directory, asked through a handle of the
/// directory (`GetVolumeInformationByHandleW`): a process of an AppContainer may not open the
/// root of the volume, which `GetVolumeInformationW` needs.
pub fn file_system_name(directory: &Path) -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumeInformationByHandleW(
            file: HANDLE,
            volume_name: *mut u16,
            volume_name_size: u32,
            serial_number: *mut u32,
            maximum_component_length: *mut u32,
            flags: *mut u32,
            file_system_name: *mut u16,
            file_system_name_size: u32,
        ) -> BOOL;
    }
    let wide_name = wide(directory.as_os_str());
    let mut name = vec![0u16; 64];
    // SAFETY: the name ends with 0; the handle is owned by `file`; the buffer is as long as the
    // length passed.
    unsafe {
        let handle = CreateFileW(
            wide_name.as_ptr(),
            0,
            FILE_SHARE_ALL,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let file = OwnedHandle::from_raw_handle(handle);
        if GetVolumeInformationByHandleW(
            std::os::windows::io::AsRawHandle::as_raw_handle(&file),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        ) == 0
        {
            return None;
        }
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16(&name[..end]).ok()
}

// ---------------------------------------------------------------- opening below a scope

/// The error of a link found where the check saw none: `ERROR_STOPPED_ON_SYMLINK`, as `ELOOP`
/// on Unix (release condition 5, docs/runtime.md 9.2).
pub const STOPPED_ON_LINK: i32 = 681;

/// Whether the error is a link found below a scope after the check.
pub fn is_link_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(STOPPED_ON_LINK)
}

fn link_error() -> io::Error {
    io::Error::from_raw_os_error(STOPPED_ON_LINK)
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct IoStatus {
    status: usize,
    information: usize,
}

#[repr(C)]
struct HandleInformation {
    attributes: u32,
    creation: u64,
    access: u64,
    write: u64,
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtCreateFile(
        handle: *mut HANDLE,
        access: u32,
        attributes: *mut ObjectAttributes,
        status: *mut IoStatus,
        allocation: *mut i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *mut c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(
        handle: HANDLE,
        status: *mut IoStatus,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetFileInformationByHandle(file: HANDLE, information: *mut HandleInformation) -> BOOL;
    fn GetFileInformationByHandleEx(
        file: HANDLE,
        class: u32,
        buffer: *mut c_void,
        size: u32,
    ) -> BOOL;
}

const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const FILE_DIRECTORY_FILE: u32 = 0x1;
const FILE_WRITE_THROUGH: u32 = 0x2;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;
const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const DELETE: u32 = 0x0001_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_RENAME_INFORMATION: u32 = 10;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const FILE_FULL_DIRECTORY_INFO: u32 = 14;
const FILE_FULL_DIRECTORY_RESTART_INFO: u32 = 15;
const ERROR_NO_MORE_FILES: i32 = 18;
/// Entering a directory and reading its attributes, all a step of a walk needs.
const STEP: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

fn status_error(status: i32) -> io::Error {
    // SAFETY: a conversion of a number.
    io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32)
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    std::os::windows::io::AsRawHandle::as_raw_handle(handle)
}

/// Opens one name in the directory of `dir` (`NtCreateFile` with `RootDirectory`), never
/// following a reparse point: with `FILE_OPEN_REPARSE_POINT` a link or junction is opened as
/// itself. The name must be one component.
fn open_in(
    dir: &OwnedHandle,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty()
        || units.len() > 32_000
        || units
            .iter()
            .any(|&c| c == u16::from(b'\\') || c == u16::from(b'/'))
        || name == OsStr::new(".")
        || name == OsStr::new("..")
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not one name"));
    }
    let mut text = UnicodeString {
        length: (units.len() * 2) as u16,
        maximum: (units.len() * 2) as u16,
        buffer: units.as_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root: raw(dir),
        name: (&mut text as *mut UnicodeString).cast(),
        attributes: OBJ_CASE_INSENSITIVE,
        descriptor: std::ptr::null_mut(),
        quality: std::ptr::null_mut(),
    };
    let mut status = IoStatus {
        status: 0,
        information: 0,
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: every pointer refers to a local that lives through the call; the name is not
    // terminated, its length is given; the handle is owned by the result.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &mut attributes,
            &mut status,
            std::ptr::null_mut(),
            0,
            FILE_SHARE_ALL,
            disposition,
            options
                | FILE_OPEN_REPARSE_POINT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | FILE_OPEN_FOR_BACKUP_INTENT,
            std::ptr::null_mut(),
            0,
        )
    };
    if result < 0 {
        return Err(status_error(result));
    }
    // SAFETY: a new handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// The attributes of an open object; a reparse point (a link or a junction put there after
/// the check) is refused.
fn attributes_without_link(handle: &OwnedHandle) -> io::Result<u32> {
    // SAFETY: the structure is the documented BY_HANDLE_FILE_INFORMATION, filled by the call.
    let attributes = unsafe {
        let mut information: HandleInformation = std::mem::zeroed();
        if GetFileInformationByHandle(raw(handle), &mut information) == 0 {
            return Err(io::Error::last_os_error());
        }
        information.attributes
    };
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(link_error());
    }
    Ok(attributes)
}

/// The path of an open object as `\\?\C:\...`, from its normalized NT name and the table of
/// drive letters ([`remember_volumes`]).
fn path_of(handle: &OwnedHandle) -> io::Result<PathBuf> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFinalPathNameByHandleW(file: HANDLE, path: *mut u16, length: u32, flags: u32) -> u32;
    }
    const VOLUME_NAME_NT: u32 = 0x2;
    let mut buffer = vec![0u16; 512];
    let nt = loop {
        // SAFETY: the buffer holds `buffer.len()` units, and the call writes at most that many.
        let n = unsafe {
            GetFinalPathNameByHandleW(
                raw(handle),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                VOLUME_NAME_NT,
            )
        } as usize;
        if n == 0 {
            return Err(io::Error::last_os_error());
        }
        if n < buffer.len() {
            break String::from_utf16_lossy(&buffer[..n]);
        }
        buffer = vec![0u16; n + 1];
    };
    // A process that recorded no table (outside an AppContainer) reads its own now.
    let table = VOLUMES.get_or_init(volumes).as_slice();
    for (drive, device) in table {
        if let Some(rest) = nt.strip_prefix(device.as_str()) {
            if rest.is_empty() || rest.starts_with('\\') {
                return Ok(PathBuf::from(format!("\\\\?\\{drive}{rest}")));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("the volume of {nt} has no known drive letter"),
    ))
}

/// Opens the anchor of a walk, an absolute, link-free directory, as itself, and confirms that
/// the handle names exactly that path: a directory above it replaced by a junction after the
/// check would lead the open elsewhere, and its final name would differ.
fn open_anchor_directory(anchor: &Path) -> io::Result<OwnedHandle> {
    let name = wide(anchor.as_os_str());
    // SAFETY: the name ends with 0; the handle is owned by the result.
    let handle = unsafe {
        let handle = CreateFileW(
            name.as_ptr(),
            STEP,
            FILE_SHARE_ALL,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        OwnedHandle::from_raw_handle(handle)
    };
    if attributes_without_link(&handle)? & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ));
    }
    let found = path_of(&handle)?;
    let same = |a: &Path, b: &Path| {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    };
    if !same(&found, anchor) && !same(&found, &verbatim(anchor)) {
        return Err(link_error());
    }
    Ok(handle)
}

/// What the last step of a walk opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// A file, for reading its content.
    Read,
    /// A directory, for listing its names.
    Directory,
    /// Anything, for its attributes only.
    Kind,
    /// A directory, to open, create, rename, or remove names in it (the writes).
    Parent,
}

/// The steps of [`super::beneath::walk`] on Windows: the anchor by its path and confirmed by
/// its final name, then each name relative to the handle of the one before, every step opened
/// as itself and refused when it is a reparse point.
pub struct Steps {
    pub want: Want,
}

impl super::beneath::Beneath for Steps {
    type Handle = OwnedHandle;

    fn open_anchor(&mut self, anchor: &Path) -> io::Result<OwnedHandle> {
        open_anchor_directory(anchor)
    }

    fn open_directory(&mut self, dir: &OwnedHandle, name: &OsString) -> io::Result<OwnedHandle> {
        let handle = open_in(dir, name, STEP, FILE_OPEN, 0)?;
        if attributes_without_link(&handle)? & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            ));
        }
        Ok(handle)
    }

    fn open_last(&mut self, dir: &OwnedHandle, name: &OsString) -> io::Result<OwnedHandle> {
        let (access, options) = match self.want {
            Want::Read => (READ_FILE, FILE_NON_DIRECTORY_FILE),
            Want::Directory => (READ_EXECUTE, FILE_DIRECTORY_FILE),
            Want::Kind => (STEP & !FILE_TRAVERSE, 0),
            Want::Parent => (STEP, FILE_DIRECTORY_FILE),
        };
        let handle = open_in(dir, name, access, FILE_OPEN, options)?;
        attributes_without_link(&handle)?;
        Ok(handle)
    }

    fn reopen_anchor(&mut self, anchor: OwnedHandle) -> io::Result<OwnedHandle> {
        Ok(anchor)
    }
}

/// Opens a checked path as `want` asks ([`Steps`]).
pub fn open_checked(anchored: &super::beneath::Anchored, want: Want) -> io::Result<OwnedHandle> {
    super::beneath::walk(&mut Steps { want }, anchored)
}

/// What the checked path is, without following a reparse point in any component.
pub fn kind_checked(anchored: &super::beneath::Anchored) -> io::Result<super::beneath::Kind> {
    let handle = open_checked(anchored, Want::Kind)?;
    Ok(
        if attributes_without_link(&handle)? & FILE_ATTRIBUTE_DIRECTORY != 0 {
            super::beneath::Kind::Directory
        } else {
            super::beneath::Kind::Other
        },
    )
}

/// Calls `each` with the names in an open directory, without `.` and `..`, until it returns
/// false (`GetFileInformationByHandleEx` with `FileFullDirectoryInfo`).
pub fn list_names(dir: &OwnedHandle, each: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()> {
    let mut buffer = vec![0u64; 8192];
    let mut class = FILE_FULL_DIRECTORY_RESTART_INFO;
    loop {
        // SAFETY: the buffer holds the size passed; the call fills it with entries of the
        // documented FILE_FULL_DIR_INFO layout.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                raw(dir),
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        };
        if ok == 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(ERROR_NO_MORE_FILES) {
                Ok(())
            } else {
                Err(e)
            };
        }
        class = FILE_FULL_DIRECTORY_INFO;
        let bytes = buffer.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            // SAFETY: each entry lies in the buffer the call filled: the next offset, then the
            // length of the name at byte 60 and the name at byte 68.
            let (next, name) = unsafe {
                let entry = bytes.add(offset);
                let next = std::ptr::read_unaligned(entry as *const u32) as usize;
                let length = std::ptr::read_unaligned(entry.add(60) as *const u32) as usize / 2;
                let name = std::slice::from_raw_parts(entry.add(68) as *const u16, length);
                (next, OsString::from_wide(name))
            };
            if name != "." && name != ".." && !each(&name) {
                return Ok(());
            }
            if next == 0 {
                break;
            }
            offset += next;
        }
    }
}

/// The directory that holds a checked path of a write and the last name of the path.
pub fn open_parent(anchored: &super::beneath::Anchored) -> io::Result<(OwnedHandle, OsString)> {
    let Some((last, directories)) = anchored.rest.split_last() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a root is not written",
        ));
    };
    let mut steps = Steps { want: Want::Parent };
    let mut handle = super::beneath::Beneath::open_anchor(&mut steps, &anchored.anchor)?;
    for name in directories {
        handle = super::beneath::Beneath::open_directory(&mut steps, &handle, name)?;
    }
    Ok((handle, last.clone()))
}

/// Creates a new file in the directory for writing; fails if the name exists.
pub fn create_new_in(dir: &OwnedHandle, name: &OsStr) -> io::Result<std::fs::File> {
    let handle = open_in(
        dir,
        name,
        WRITE_FILE | FILE_READ_ATTRIBUTES | DELETE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE,
    )?;
    Ok(std::fs::File::from(handle))
}

/// Creates a new directory in the directory; fails if the name exists.
pub fn make_directory_in(dir: &OwnedHandle, name: &OsStr) -> io::Result<()> {
    open_in(dir, name, STEP, FILE_CREATE, FILE_DIRECTORY_FILE).map(drop)
}

/// Gives the entry `from` of `dir` the name `to` in the directory `target` (`NtSetInformationFile`
/// with `FileRenameInformation`, relative to the handle of `target`): with `replace` an existing
/// target is replaced, the rename written through as `MOVEFILE_WRITE_THROUGH` does; without it
/// an existing target fails the rename. The entry is opened as itself, never through a link.
pub fn rename_in(
    dir: &OwnedHandle,
    from: &OsStr,
    target: &OwnedHandle,
    to: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let entry = open_in(
        dir,
        from,
        DELETE | SYNCHRONIZE,
        FILE_OPEN,
        FILE_WRITE_THROUGH,
    )?;
    let units: Vec<u16> = to.encode_wide().collect();
    if units.is_empty()
        || units
            .iter()
            .any(|&c| c == u16::from(b'\\') || c == u16::from(b'/'))
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not one name"));
    }
    // FILE_RENAME_INFORMATION: ReplaceIfExists (a BOOLEAN, padded to 8 bytes), RootDirectory,
    // FileNameLength in bytes, then the name.
    let mut buffer = vec![0u64; 3 + units.len().div_ceil(4) + 1];
    let size = 20 + units.len() * 2;
    // SAFETY: the buffer holds the header and the name; the handles are open.
    unsafe {
        let base = buffer.as_mut_ptr() as *mut u8;
        *base = u8::from(replace);
        std::ptr::write_unaligned(base.add(8) as *mut HANDLE, raw(target));
        std::ptr::write_unaligned(base.add(16) as *mut u32, (units.len() * 2) as u32);
        std::ptr::copy_nonoverlapping(units.as_ptr(), base.add(20) as *mut u16, units.len());
        let mut status = IoStatus {
            status: 0,
            information: 0,
        };
        let result = NtSetInformationFile(
            raw(&entry),
            &mut status,
            base.cast(),
            size as u32,
            FILE_RENAME_INFORMATION,
        );
        if result < 0 {
            return Err(status_error(result));
        }
    }
    Ok(())
}

/// Removes the file of this name in the directory, as itself.
pub fn remove_in(dir: &OwnedHandle, name: &OsStr) -> io::Result<()> {
    let entry = open_in(
        dir,
        name,
        DELETE | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE,
    )?;
    let mut delete: u8 = 1;
    let mut status = IoStatus {
        status: 0,
        information: 0,
    };
    // SAFETY: FILE_DISPOSITION_INFORMATION is one BOOLEAN; the handle is open.
    let result = unsafe {
        NtSetInformationFile(
            raw(&entry),
            &mut status,
            (&mut delete as *mut u8).cast(),
            1,
            FILE_DISPOSITION_INFORMATION,
        )
    };
    if result < 0 {
        return Err(status_error(result));
    }
    Ok(())
}

// ---------------------------------------------------------------- the thread of a program

/// Makes the calling thread impersonate the token and checks that it does, at the level that
/// opens files, inside an AppContainer.
pub fn impersonate(token: &Token) -> io::Result<()> {
    // SAFETY: a null thread pointer means the calling thread; the token is an impersonation
    // token; the thread token is opened as self and closed here.
    unsafe {
        if SetThreadToken(std::ptr::null_mut(), token.0) == 0 {
            return Err(last_error(
                "the thread cannot take the token of its AppContainer",
            ));
        }
        let mut current: HANDLE = std::ptr::null_mut();
        if OpenThreadToken(current_thread(), TOKEN_QUERY, 1, &mut current) == 0 {
            RevertToSelf();
            return Err(last_error("the token of the thread cannot be read"));
        }
        let current = Token(current);
        let level = query_u32(current.0, TOKEN_IMPERSONATION_LEVEL);
        let container = query_u32(current.0, TOKEN_IS_APP_CONTAINER);
        if level != Some(SECURITY_IMPERSONATION as u32) || container != Some(1) {
            RevertToSelf();
            return Err(io::Error::other(format!(
                "the thread does not run in its AppContainer (level {level:?}, AppContainer \
                 {container:?})"
            )));
        }
    }
    Ok(())
}

/// Ends the impersonation of the calling thread.
pub fn revert() {
    // SAFETY: no arguments; reverting a thread that does not impersonate does nothing.
    unsafe {
        RevertToSelf();
    }
}

fn current_thread() -> HANDLE {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> HANDLE;
    }
    // SAFETY: no arguments; a pseudo handle.
    unsafe { GetCurrentThread() }
}

unsafe fn query_u32(token: HANDLE, class: i32) -> Option<u32> {
    let mut value = 0u32;
    let mut returned = 0u32;
    (GetTokenInformation(
        token,
        class,
        (&mut value as *mut u32).cast(),
        4,
        &mut returned,
    ) != 0)
        .then_some(value)
}

/// Whether this process runs in an AppContainer: the check of a worker that must have been
/// started in one (SB-5: it fails closed otherwise).
pub fn in_app_container() -> bool {
    // SAFETY: the process token is opened for querying and closed.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess() as HANDLE, TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let token = Token(token);
        query_u32(token.0, TOKEN_IS_APP_CONTAINER) == Some(1)
    }
}

// ---------------------------------------------------------------- processes

/// What a standard handle of a child is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stdio {
    Null,
    Piped,
    Inherit,
}

/// A process to create.
pub struct Command<'a> {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The whole environment of the child; `None` inherits that of this process.
    pub env: Option<Vec<(OsString, OsString)>>,
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
    /// The container to create the process in; `None` creates it as this process is.
    pub container: Option<&'a Container>,
    /// Inheritable handles the child receives besides its standard handles, with the same
    /// values (the connection of the download worker of the package operations).
    pub inherit: Vec<isize>,
}

impl<'a> Command<'a> {
    pub fn new(program: impl Into<PathBuf>) -> Command<'a> {
        Command {
            program: program.into(),
            args: Vec::new(),
            env: None,
            stdin: Stdio::Inherit,
            stdout: Stdio::Inherit,
            stderr: Stdio::Inherit,
            container: None,
            inherit: Vec::new(),
        }
    }
}

/// A child process, created suspended: [`Child::resume`] starts it, after it was put in its
/// Job Object.
pub struct Child {
    process: OwnedHandle,
    thread: Option<OwnedHandle>,
    pid: u32,
    pub stdin: Option<std::fs::File>,
    pub stdout: Option<std::fs::File>,
    pub stderr: Option<std::fs::File>,
    status: Option<ExitStatus>,
}

impl Child {
    pub fn id(&self) -> u32 {
        self.pid
    }

    pub fn as_raw_handle(&self) -> HANDLE {
        std::os::windows::io::AsRawHandle::as_raw_handle(&self.process)
    }

    /// Lets the main thread of the suspended child run.
    pub fn resume(&mut self) -> io::Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // SAFETY: the thread handle belongs to the child and is closed when dropped.
        if unsafe { ResumeThread(std::os::windows::io::AsRawHandle::as_raw_handle(&thread)) }
            == u32::MAX
        {
            return Err(last_error("the child cannot be resumed"));
        }
        Ok(())
    }

    fn wait_for(&mut self, milliseconds: u32) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        // SAFETY: the process handle is owned by the child.
        unsafe {
            match WaitForSingleObject(self.as_raw_handle(), milliseconds) {
                WAIT_OBJECT_0 => {}
                0x102 => return Ok(None),
                _ => return Err(last_error("the child cannot be waited for")),
            }
            let mut code = 0u32;
            if GetExitCodeProcess(self.as_raw_handle(), &mut code) == 0 {
                return Err(last_error("the status of the child cannot be read"));
            }
            let status = <ExitStatus as std::os::windows::process::ExitStatusExt>::from_raw(code);
            self.status = Some(status);
            Ok(Some(status))
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_for(0)
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.stdin = None;
        self.wait_for(INFINITE)
            .map(|s| s.expect("an infinite wait ends with the process"))
    }

    pub fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: the process handle is owned by the child.
        if unsafe { TerminateProcess(self.as_raw_handle(), 1) } == 0 {
            return Err(last_error("the child cannot be terminated"));
        }
        Ok(())
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // A child that was never resumed never ran: it does not stay behind suspended.
        if self.thread.is_some() {
            let _ = self.kill();
        }
    }
}

/// Appends one argument quoted as the C runtime of Microsoft reads it back.
fn append_argument(out: &mut Vec<u16>, argument: &OsStr, always: bool) {
    let units: Vec<u16> = argument.encode_wide().collect();
    let quote = always
        || units.is_empty()
        || units
            .iter()
            .any(|&c| c == u16::from(b' ') || c == u16::from(b'\t') || c == u16::from(b'"'));
    if quote {
        out.push(u16::from(b'"'));
    }
    let mut backslashes = 0usize;
    for &c in &units {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                out.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        out.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        out.push(u16::from(b'"'));
    }
}

/// The command line of a program and its arguments.
pub fn command_line(program: &Path, args: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    append_argument(&mut line, program.as_os_str(), true);
    for argument in args {
        line.push(u16::from(b' '));
        append_argument(&mut line, argument, false);
    }
    line.push(0);
    line
}

fn environment_block(env: &[(OsString, OsString)]) -> Vec<u16> {
    let mut sorted: Vec<&(OsString, OsString)> = env.iter().collect();
    sorted.sort_by_key(|(k, _)| k.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (key, value) in sorted {
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// Where a program of this name is, as a child would be started: a name with a directory as
/// written, a bare name through the directories of `PATH`; `.exe` is added to a name without an
/// extension.
pub fn find_program(program: &OsStr, path: Option<&OsStr>) -> Option<PathBuf> {
    let name = Path::new(program);
    let with_extension = |p: PathBuf| {
        if p.extension().is_some() {
            p
        } else {
            let mut s = p.into_os_string();
            s.push(".exe");
            PathBuf::from(s)
        }
    };
    let text = program.to_string_lossy();
    if text.contains('\\') || text.contains('/') || name.is_absolute() {
        let candidate = with_extension(name.to_path_buf());
        return candidate.is_file().then_some(candidate);
    }
    let path = path
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| with_extension(d.join(name)))
        .find(|c| c.is_file())
}

/// An inheritable handle of `NUL`.
fn null_handle(write: bool) -> io::Result<OwnedHandle> {
    let name = wide(OsStr::new("NUL"));
    let mut attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: std::ptr::null_mut(),
        inherit: 1,
    };
    // SAFETY: the name ends with 0; the handle is owned by the result.
    unsafe {
        let handle = CreateFileW(
            name.as_ptr(),
            if write { GENERIC_WRITE } else { GENERIC_READ },
            FILE_SHARE_ALL,
            &mut attributes,
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("NUL cannot be opened"));
        }
        Ok(OwnedHandle::from_raw_handle(handle))
    }
}

/// What the child gets for a discarded standard handle. Inside an AppContainer `NUL` cannot be
/// opened, so the standard input is a pipe whose other end is closed at once (the child reads
/// its end), and an output that `NUL` cannot take is a pipe that a thread of this process
/// drains.
fn empty(output: bool) -> io::Result<OwnedHandle> {
    if !output {
        let (child, _closed) = pipe(true)?;
        return Ok(child);
    }
    if let Ok(null) = null_handle(true) {
        return Ok(null);
    }
    let (child, mine) = pipe(false)?;
    let mut drained = std::fs::File::from(mine);
    std::thread::Builder::new()
        .name("unio-discard".into())
        .spawn(move || {
            let _ = io::copy(&mut drained, &mut io::sink());
        })?;
    Ok(child)
}

/// A pipe: the end of the child, inheritable, and the end of this process, not inheritable.
fn pipe(child_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: std::ptr::null_mut(),
        inherit: 1,
    };
    let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: both handles are owned by the result.
    unsafe {
        if CreatePipe(&mut read, &mut write, &mut attributes, 0) == 0 {
            return Err(last_error("a pipe cannot be created"));
        }
        let (read, write) = (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        );
        let (child, mine) = if child_reads {
            (read, write)
        } else {
            (write, read)
        };
        if SetHandleInformation(
            std::os::windows::io::AsRawHandle::as_raw_handle(&mine),
            HANDLE_FLAG_INHERIT,
            0,
        ) == 0
        {
            return Err(last_error("a pipe cannot be set up"));
        }
        Ok((child, mine))
    }
}

/// An inheritable duplicate of a standard handle of this process.
fn inherited(which: u32) -> io::Result<OwnedHandle> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> HANDLE;
        fn DuplicateHandle(
            source_process: HANDLE,
            source: HANDLE,
            target_process: HANDLE,
            target: *mut HANDLE,
            access: u32,
            inherit: BOOL,
            options: u32,
        ) -> BOOL;
    }
    const DUPLICATE_SAME_ACCESS: u32 = 0x2;
    // SAFETY: the standard handle is only duplicated; the duplicate is owned by the result.
    unsafe {
        let handle = GetStdHandle(which);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return empty(which != STD_INPUT);
        }
        let mut duplicate: HANDLE = std::ptr::null_mut();
        if DuplicateHandle(
            GetCurrentProcess() as HANDLE,
            handle,
            GetCurrentProcess() as HANDLE,
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        ) == 0
        {
            return empty(which != STD_INPUT);
        }
        Ok(OwnedHandle::from_raw_handle(duplicate))
    }
}

const STD_INPUT: u32 = -10i32 as u32;
const STD_OUTPUT: u32 = -11i32 as u32;
const STD_ERROR: u32 = -12i32 as u32;

/// Makes a handle of this process inheritable, so that [`Command::inherit`] can pass it.
pub fn make_inheritable(handle: isize) -> io::Result<()> {
    // SAFETY: the flag of a handle the caller owns.
    if unsafe { SetHandleInformation(handle as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }
        == 0
    {
        return Err(last_error("the handle cannot be passed"));
    }
    Ok(())
}

/// Starts the sockets of this process (`WSAStartup`) for a socket it received rather than
/// made, which the standard library starts only when it makes one.
pub fn start_sockets() -> io::Result<()> {
    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAStartup(version: u16, data: *mut u8) -> i32;
    }
    let mut data = [0u8; 512];
    // SAFETY: the buffer is larger than WSADATA, which the call fills.
    match unsafe { WSAStartup(0x0202, data.as_mut_ptr()) } {
        0 => Ok(()),
        code => Err(io::Error::from_raw_os_error(code)),
    }
}

/// Creates the process suspended: in the container of the command when it names one, with
/// exactly its three standard handles inherited and no other handle of this process.
pub fn spawn(command: &Command) -> io::Result<Child> {
    let mut theirs: Vec<OwnedHandle> = Vec::new();
    let mut mine: [Option<std::fs::File>; 3] = [None, None, None];
    for (index, (stdio, which)) in [
        (command.stdin, STD_INPUT),
        (command.stdout, STD_OUTPUT),
        (command.stderr, STD_ERROR),
    ]
    .into_iter()
    .enumerate()
    {
        let handle = match stdio {
            Stdio::Null => empty(index != 0)?,
            Stdio::Inherit => inherited(which)?,
            Stdio::Piped => {
                let (child, parent) = pipe(index == 0)?;
                mine[index] = Some(std::fs::File::from(parent));
                child
            }
        };
        theirs.push(handle);
    }
    let raw: Vec<HANDLE> = theirs
        .iter()
        .map(std::os::windows::io::AsRawHandle::as_raw_handle)
        .collect();
    // Two attributes: the handles to inherit and, in a container, its capabilities.
    let mut size = 0usize;
    // SAFETY: the first call only reports the size of the list.
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut size);
    }
    let mut list = vec![0u64; size.div_ceil(8)];
    let list_ptr = list.as_mut_ptr() as *mut c_void;
    let mut handle_list = raw.clone();
    handle_list.extend(command.inherit.iter().map(|&h| h as HANDLE));
    let mut capabilities = SecurityCapabilities {
        container: command.container.map_or(std::ptr::null_mut(), |c| c.sid),
        capabilities: std::ptr::null_mut(),
        count: 0,
        reserved: 0,
    };
    let mut line = command_line(&command.program, &command.args);
    let application = wide(command.program.as_os_str());
    // A process of an AppContainer does not start without `LOCALAPPDATA` in its environment
    // (ERROR_ENVVAR_NOT_FOUND); the system replaces the value with the folder of the
    // container's own profile below it (`<value>\Packages\<name>\AC`), and sets `TEMP` and
    // `TMP` to that folder's `Temp`, so it must be the real one.
    let env = command.env.as_ref().map(|vars| {
        let mut vars = vars.clone();
        let has = vars
            .iter()
            .any(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("LOCALAPPDATA"));
        if command.container.is_some() && !has {
            if let Some(value) = std::env::var_os("LOCALAPPDATA") {
                vars.push((OsString::from("LOCALAPPDATA"), value));
            }
        }
        vars
    });
    let mut environment = env.as_deref().map(environment_block);
    let mut information = ProcessInformation {
        process: std::ptr::null_mut(),
        thread: std::ptr::null_mut(),
        pid: 0,
        tid: 0,
    };
    // SAFETY: the list has the size the system asked for and is deleted below; every pointer
    // given to it (the handle list, the capabilities) lives until CreateProcessW returns; the
    // strings end with 0; the environment block ends with two zeros.
    let created = unsafe {
        if InitializeProcThreadAttributeList(list_ptr, 2, 0, &mut size) == 0 {
            return Err(last_error("the attributes of the child cannot be set up"));
        }
        let mut ok = UpdateProcThreadAttribute(
            list_ptr,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            handle_list.as_mut_ptr().cast(),
            handle_list.len() * std::mem::size_of::<HANDLE>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) != 0;
        if ok && command.container.is_some() {
            ok = UpdateProcThreadAttribute(
                list_ptr,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
                (&mut capabilities as *mut SecurityCapabilities).cast(),
                std::mem::size_of::<SecurityCapabilities>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) != 0;
        }
        if !ok {
            let e = last_error("the attributes of the child cannot be set up");
            DeleteProcThreadAttributeList(list_ptr);
            return Err(e);
        }
        let mut startup = StartupInfoEx {
            info: std::mem::zeroed(),
            attributes: list_ptr,
        };
        startup.info.cb = std::mem::size_of::<StartupInfoEx>() as u32;
        startup.info.flags = STARTF_USESTDHANDLES;
        startup.info.stdin = raw[0];
        startup.info.stdout = raw[1];
        startup.info.stderr = raw[2];
        let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT;
        let result = CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            flags,
            environment
                .as_mut()
                .map_or(std::ptr::null_mut(), |e| e.as_mut_ptr().cast()),
            std::ptr::null(),
            &mut startup,
            &mut information,
        );
        let error = io::Error::last_os_error();
        DeleteProcThreadAttributeList(list_ptr);
        if result == 0 {
            Err(error)
        } else {
            Ok(())
        }
    };
    drop(theirs);
    created?;
    let [stdin, stdout, stderr] = mine;
    // SAFETY: both handles were returned by CreateProcessW and are owned from here on.
    unsafe {
        Ok(Child {
            process: OwnedHandle::from_raw_handle(information.process),
            thread: Some(OwnedHandle::from_raw_handle(information.thread)),
            pid: information.pid,
            stdin,
            stdout,
            stderr,
            status: None,
        })
    }
}

// ---------------------------------------------------------------- the sandbox of a program

/// The AppContainers of a running program and the tokens of its thread (docs/runtime.md 9.4).
///
/// The reader holds grants on the read scopes and the listed programs; the thread impersonates
/// its token, and every child is created in it. The writer holds the write grants: a tree of a
/// write scope with everything in it, and for a scope of one file the right to create, replace,
/// and remove entries of its directory, as the writer process has on Linux and macOS. The
/// thread takes the writer's token only for the write-class operations of the runtime, which
/// have checked the scopes first; a child never has it.
pub struct Program {
    reader: Container,
    writer: Option<Container>,
    read_token: Token,
    write_token: Option<Token>,
}

/// Grants an entry, or warns when it cannot be granted: a missing grant only takes access
/// away, so the program still starts, and the operation it needed fails as denied.
fn grant_or_warn(container: &mut Container, path: &Path, access: Access, subtree: bool) {
    if let Err(e) = container.grant(path, access, subtree) {
        eprintln!(
            "unio: warning: {} cannot be granted to the AppContainer of the program ({access:?}): \
             {e}",
            path.display()
        );
    }
}

impl Program {
    /// Sets up the containers of the permissions and makes the calling thread impersonate the
    /// reader.
    pub fn enter(permissions: &super::Permissions) -> io::Result<Program> {
        remember_volumes(None);
        let mut reader = Container::new()?;
        for scope in &permissions.read {
            grant_or_warn(&mut reader, &scope.root, Access::Read, scope.subtree);
            // A read of the root of a scope, or of a scope of one file, opens it from the
            // directory above (`beneath::anchor`), which it may enter but not list.
            if let Some(parent) = scope.root.parent() {
                let _ = reader.grant(parent, Access::Enter, false);
            }
        }
        // A child whose working directory it cannot enter does not start a process itself.
        let _ = reader.grant(super::working_directory(), Access::Enter, false);
        let path = std::env::var_os("PATH");
        for program in &permissions.process {
            // A program of the system that every application package may run needs no grant,
            // and cannot be given one by a user who does not own it: no warning then.
            if let Some(found) = find_program(OsStr::new(program), path.as_deref()) {
                let _ = reader.grant(&found, Access::Execute, false);
            }
        }
        let (writer, write_token) = if permissions.write.is_empty() {
            (None, None)
        } else {
            let mut writer = Container::new()?;
            for scope in &permissions.write {
                if scope.subtree {
                    grant_or_warn(&mut writer, &scope.root, Access::Modify, true);
                } else if let Some(dir) = scope.root.parent() {
                    grant_or_warn(&mut writer, dir, Access::Entries, false);
                }
            }
            let token = writer.token(false)?;
            (Some(writer), Some(token))
        };
        let read_token = reader.token(false)?;
        impersonate(&read_token)?;
        Ok(Program {
            reader,
            writer,
            read_token,
            write_token,
        })
    }

    /// The container of the children of the program.
    pub fn container(&self) -> &Container {
        &self.reader
    }

    /// Runs `f` with the token of the writer, then returns to the token of the program.
    pub fn as_writer<R>(&self, f: impl FnOnce() -> R) -> R {
        let Some(token) = &self.write_token else {
            return f();
        };
        struct Back<'a>(&'a Token);
        impl Drop for Back<'_> {
            fn drop(&mut self) {
                if impersonate(self.0).is_err() {
                    // The thread cannot return to its own restrictions: it must not go on
                    // with more rights than the program has.
                    std::process::exit(crate::fault::EXIT_SANDBOX);
                }
            }
        }
        if impersonate(token).is_err() {
            return f();
        }
        let _back = Back(&self.read_token);
        f()
    }

    /// Runs `f` with the token of this process, to create a child in the container: the
    /// child's pipes and its creation belong to the runtime, which has checked the program.
    pub fn without_token<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Back<'a>(&'a Token);
        impl Drop for Back<'_> {
            fn drop(&mut self) {
                if impersonate(self.0).is_err() {
                    std::process::exit(crate::fault::EXIT_SANDBOX);
                }
            }
        }
        revert();
        let _back = Back(&self.read_token);
        f()
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        // The thread leaves the containers before their grants are removed.
        revert();
        let _ = &self.writer;
    }
}

/// Text of a wide string, for messages.
pub fn text_of(units: &[u16]) -> String {
    OsString::from_wide(units).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_as_the_c_runtime_reads_them() {
        let line = command_line(
            Path::new(r"C:\a b\p.exe"),
            &[
                OsString::from("plain"),
                OsString::from("two words"),
                OsString::from(r#"quote"inside"#),
                OsString::from(r"trailing\"),
                OsString::from(r"space and trailing\"),
                OsString::from(""),
            ],
        );
        assert_eq!(
            text_of(&line[..line.len() - 1]),
            r#""C:\a b\p.exe" plain "two words" "quote\"inside" trailing\ "space and trailing\\" """#
        );
    }

    #[test]
    fn children_inherit_the_entries_of_their_directory_by_the_rules_of_ntfs() {
        let root = Access::Read.entries(true);
        let dir = inherit(&root, true);
        let file = inherit(&root, false);
        assert_eq!(
            dir,
            vec![
                (INHERITED_ACE | CONTAINER_INHERIT_ACE, READ_EXECUTE),
                (
                    INHERITED_ACE | OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE,
                    READ_FILE
                ),
            ]
        );
        assert_eq!(file, vec![(INHERITED_ACE, READ_FILE)]);
        assert_eq!(inherit(&dir, false), file);
        assert_eq!(inherit(&Access::Modify.entries(false), true), vec![]);
    }
}
