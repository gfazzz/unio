//! The restrictions of the operating system on Linux (SB-4): `no_new_privs`, Landlock for the
//! file system and TCP, seccomp-bpf for system calls, and resource limits. Landlock and seccomp
//! restrict the calling thread and everything it starts later: the threads of the program and
//! every child process (SB-1). The interfaces are declared here by hand, as in the rest of the
//! runtime, so that the sandbox needs no dependency.

use super::{apply_rlimits, program_paths, Limits, Permissions, Scope};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

extern "C" {
    fn syscall(number: i64, ...) -> i64;
    fn prctl(option: i32, ...) -> i32;
    fn open(path: *const std::os::raw::c_char, flags: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
}

const SYS_LANDLOCK_CREATE_RULESET: i64 = 444;
const SYS_LANDLOCK_ADD_RULE: i64 = 445;
const SYS_LANDLOCK_RESTRICT_SELF: i64 = 446;
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: i32 = 1;

const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
const REMOVE_DIR: u64 = 1 << 4;
const REMOVE_FILE: u64 = 1 << 5;
const MAKE_DIR: u64 = 1 << 7;
const MAKE_REG: u64 = 1 << 8;
const REFER: u64 = 1 << 13;
const TRUNCATE: u64 = 1 << 14;
const IOCTL_DEV: u64 = 1 << 15;
/// The rights that apply to a file rather than a directory.
const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;

const NET_BIND_TCP: u64 = 1 << 0;
const NET_CONNECT_TCP: u64 = 1 << 1;
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
const SCOPE_SIGNAL: u64 = 1 << 1;

const READ: u64 = READ_FILE | READ_DIR;
const WRITE: u64 = WRITE_FILE | REMOVE_DIR | REMOVE_FILE | MAKE_DIR | MAKE_REG | REFER | TRUNCATE
    // Syncing the directory of a replaced file opens it for reading.
    | READ_DIR;

const O_PATH: i32 = 0o10000000;
const O_CLOEXEC: i32 = 0o2000000;

const PR_SET_NO_NEW_PRIVS: i32 = 38;
const PR_GET_SECCOMP: i32 = 21;
const PR_SET_SECCOMP: i32 = 22;
const SECCOMP_MODE_FILTER: i32 = 2;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// The Landlock ABI of the kernel; 0 when Landlock is missing or disabled.
pub fn landlock_abi() -> i64 {
    // SAFETY: with the version flag the call reads no memory and returns the ABI or -1.
    let abi = unsafe {
        syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    abi.max(0)
}

fn seccomp_available() -> bool {
    // SAFETY: PR_GET_SECCOMP takes no pointer; it fails with EINVAL without seccomp.
    unsafe { prctl(PR_GET_SECCOMP, 0, 0, 0, 0) >= 0 }
}

/// The restrictions this kernel cannot apply (SB-5).
pub fn missing() -> Vec<String> {
    let mut missing = Vec::new();
    let abi = landlock_abi();
    if abi == 0 {
        missing.push("file system restrictions (Landlock is not available in this kernel)".into());
    } else if abi < 3 {
        missing.push(format!(
            "truncation of files outside the write scopes (Landlock ABI {abi} < 3)"
        ));
    }
    if !seccomp_available() {
        missing.push(
            "system call and network restrictions (seccomp is not available in this kernel)".into(),
        );
    }
    missing
}

/// Applies every restriction the kernel supports to the current thread.
pub fn apply(permissions: &Permissions, limits: &Limits) -> Result<(), String> {
    apply_rlimits(limits)?;
    restrict(&rules(permissions), &[])
}

/// The restrictions of the writer process (docs/runtime.md 9.2): it may write only in the
/// write scopes and their directories, read no file, run nothing, start no process or thread,
/// and open no socket.
pub fn apply_writer(scopes: &[Scope]) -> Result<(), String> {
    apply_rlimits(&Limits::default())?;
    let rules: Vec<(PathBuf, u64)> = scopes.iter().map(|s| (write_root(s), WRITE)).collect();
    restrict(&rules, nr::WRITER_DENIED)
}

fn restrict(rules: &[(PathBuf, u64)], denied: &[u32]) -> Result<(), String> {
    // SAFETY: PR_SET_NO_NEW_PRIVS takes integer arguments only.
    if unsafe { prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!("no_new_privs: {}", std::io::Error::last_os_error()));
    }
    let abi = landlock_abi();
    if abi > 0 {
        landlock(abi, rules)?;
    }
    if seccomp_available() {
        seccomp(denied)?;
    }
    Ok(())
}

/// The paths and rights of the Landlock rules of a program. The write scopes grant nothing
/// here: the program and its children write no file; the writer process writes for the program
/// and checks the scopes itself (docs/runtime.md 9.2).
fn rules(permissions: &Permissions) -> Vec<(PathBuf, u64)> {
    let mut rules = Vec::new();
    for scope in &permissions.read {
        rules.push((scope.root.clone(), READ));
    }
    // A child of the sandbox can only start: it inherits the rules, so the programs, their
    // libraries and the files of the dynamic linker must be readable and executable. What a
    // child reads or writes beyond that is bounded by the scopes of the program (SB-1).
    if !permissions.process.is_empty() {
        let mut system: Vec<PathBuf> = [
            "/usr",
            "/bin",
            "/sbin",
            "/lib",
            "/lib32",
            "/lib64",
            "/libx32",
            "/opt",
            "/nix/store",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        if let Some(path) = std::env::var_os("PATH") {
            system.extend(std::env::split_paths(&path));
        }
        for dir in system {
            rules.push((dir, READ | EXECUTE));
        }
        for file in [
            "/etc/ld.so.cache",
            "/etc/ld.so.conf",
            "/etc/ld.so.conf.d",
            "/etc/localtime",
            "/etc/nsswitch.conf",
            "/etc/passwd",
            "/etc/group",
            "/etc/alternatives",
        ] {
            rules.push((PathBuf::from(file), READ | EXECUTE));
        }
        for program in &permissions.process {
            for path in program_paths(program) {
                rules.push((path, READ_FILE | EXECUTE));
            }
        }
        rules.push((PathBuf::from("/dev/null"), READ_FILE | WRITE_FILE));
    }
    rules
}

/// Landlock rules apply to a directory and everything below it: the writer process gets a
/// write scope of one file on its directory, since the atomic commit creates a temporary file
/// next to the target and renames it; the writer itself permits only the file.
fn write_root(scope: &Scope) -> PathBuf {
    if scope.subtree {
        scope.root.clone()
    } else {
        scope
            .root
            .parent()
            .map_or_else(|| scope.root.clone(), Path::to_path_buf)
    }
}

fn handled_fs(abi: i64) -> u64 {
    let mut handled = (1 << 13) - 1;
    if abi >= 2 {
        handled |= REFER;
    }
    if abi >= 3 {
        handled |= TRUNCATE;
    }
    if abi >= 5 {
        handled |= IOCTL_DEV;
    }
    handled
}

fn landlock(abi: i64, rules: &[(PathBuf, u64)]) -> Result<(), String> {
    let error = |what: &str| format!("Landlock {what}: {}", std::io::Error::last_os_error());
    let handled = handled_fs(abi);
    let attr = RulesetAttr {
        handled_access_fs: handled,
        handled_access_net: if abi >= 4 {
            NET_BIND_TCP | NET_CONNECT_TCP
        } else {
            0
        },
        scoped: if abi >= 6 {
            SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL
        } else {
            0
        },
    };
    let size: usize = match abi {
        1..=3 => 8,
        4 | 5 => 16,
        _ => 24,
    };
    // SAFETY: attr outlives the call and size never exceeds its size.
    let ruleset = unsafe { syscall(SYS_LANDLOCK_CREATE_RULESET, &attr, size, 0u32) };
    if ruleset < 0 {
        return Err(error("ruleset"));
    }
    let ruleset = ruleset as i32;
    let result = (|| {
        for (path, rights) in rules {
            let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
                continue;
            };
            // SAFETY: c_path ends with 0; O_PATH opens without reading.
            let fd = unsafe { open(c_path.as_ptr(), O_PATH | O_CLOEXEC) };
            if fd < 0 {
                // A scope whose root does not exist grants nothing in the operating system.
                continue;
            }
            let mut allowed = rights & handled;
            if !path.is_dir() {
                allowed &= FILE_RIGHTS;
            }
            let rule = PathBeneathAttr {
                allowed_access: allowed,
                parent_fd: fd,
            };
            // SAFETY: rule outlives the call; the descriptors are open.
            let added = unsafe {
                syscall(
                    SYS_LANDLOCK_ADD_RULE,
                    ruleset,
                    LANDLOCK_RULE_PATH_BENEATH,
                    &rule,
                    0u32,
                )
            };
            // SAFETY: fd was opened above and is closed once.
            unsafe { close(fd) };
            if added != 0 {
                return Err(format!(
                    "Landlock rule for {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }
        }
        // SAFETY: the ruleset descriptor is open; no_new_privs is set.
        if unsafe { syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset, 0u32) } != 0 {
            return Err(error("restrict_self"));
        }
        Ok(())
    })();
    // SAFETY: the ruleset descriptor is closed once.
    unsafe { close(ruleset) };
    result
}

// ---------------------------------------------------------------- seccomp

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

const LD_W_ABS: u16 = 0x20;
const JEQ_K: u16 = 0x15;
const JGE_K: u16 = 0x35;
const JSET_K: u16 = 0x45;
const RET_K: u16 = 0x06;

const RET_ALLOW: u32 = 0x7fff_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const EPERM: u32 = 1;
const ENOSYS: u32 = 38;

const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
/// The low 32 bits of argument `i` on a little-endian machine.
const fn offset_arg(i: u32) -> u32 {
    16 + 8 * i
}

/// The namespace flags of clone: a new namespace could hide the rules of the sandbox.
const CLONE_NAMESPACES: u32 =
    0x0002_0000 | 0x0200_0000 | 0x0400_0000 | 0x0800_0000 | 0x1000_0000 | 0x2000_0000 | 0x4000_0000;
const TIOCSTI: u32 = 0x5412;
const TIOCLINUX: u32 = 0x541C;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const ARCH: u32 = 0xC000_003E;
    pub const SOCKET: u32 = 41;
    pub const CLONE: u32 = 56;
    pub const IOCTL: u32 = 16;
    /// What the writer process does not need besides [`DENIED`]: exec, new processes and
    /// threads, and sockets of any kind.
    pub const WRITER_DENIED: &[u32] = &[
        59, 322, // execve, execveat
        56, 57, 58, // clone, fork, vfork
        53, 42, 43, 288, 49, 50, // socketpair, connect, accept, accept4, bind, listen
    ];
    /// System calls a Unio program never needs and a sandboxed one must not make.
    pub const DENIED: &[u32] = &[
        101, // ptrace
        310, 311, // process_vm_readv, process_vm_writev
        246, 320, // kexec_load, kexec_file_load
        175, 313, 176, // init_module, finit_module, delete_module
        165, 166, 155, 161, // mount, umount2, pivot_root, chroot
        167, 168, 169, // swapon, swapoff, reboot
        308, 272, // setns, unshare
        321, 298, 323, // bpf, perf_event_open, userfaultfd
        248, 249, 250, // add_key, request_key, keyctl
        303, 304, // name_to_handle_at, open_by_handle_at
        425, 426, 427, // io_uring_setup, io_uring_enter, io_uring_register
        172, 173, // iopl, ioperm
        163, 164, 227, 179, // acct, settimeofday, clock_settime, quotactl
        300, 212, 312, // fanotify_init, lookup_dcookie, kcmp
        428, 429, 430, 431, 432, 433, 442, // open_tree .. fspick, mount_setattr
    ];
}

#[cfg(target_arch = "aarch64")]
mod nr {
    pub const ARCH: u32 = 0xC000_00B7;
    pub const SOCKET: u32 = 198;
    pub const CLONE: u32 = 220;
    pub const IOCTL: u32 = 29;
    pub const WRITER_DENIED: &[u32] = &[
        221, 281, // execve, execveat
        220, // clone
        199, 203, 202, 242, 200, 201, // socketpair, connect, accept, accept4, bind, listen
    ];
    pub const DENIED: &[u32] = &[
        117, // ptrace
        270, 271, // process_vm_readv, process_vm_writev
        104, 294, // kexec_load, kexec_file_load
        105, 273, 106, // init_module, finit_module, delete_module
        40, 39, 41, 51, // mount, umount2, pivot_root, chroot
        224, 225, 142, // swapon, swapoff, reboot
        268, 97, // setns, unshare
        280, 241, 282, // bpf, perf_event_open, userfaultfd
        217, 218, 219, // add_key, request_key, keyctl
        264, 265, // name_to_handle_at, open_by_handle_at
        425, 426, 427, // io_uring
        89, 170, 112, 60, // acct, settimeofday, clock_settime, quotactl
        262, 18, 272, // fanotify_init, lookup_dcookie, kcmp
        428, 429, 430, 431, 432, 433, 442, // new mount interface
    ];
}

const CLONE3: u32 = 435;

fn stmt(code: u16, k: u32) -> SockFilter {
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// The filter: the native architecture only; no sockets at all (the network is never
/// permitted and a Unix socket could reach a privileged service); no new namespaces; no
/// terminal injection; and none of the calls of [`nr::DENIED`].
fn filter(extra: &[u32]) -> Vec<SockFilter> {
    let mut f = vec![
        stmt(LD_W_ABS, OFFSET_ARCH),
        jump(JEQ_K, nr::ARCH, 1, 0),
        stmt(RET_K, RET_KILL_PROCESS),
        stmt(LD_W_ABS, OFFSET_NR),
    ];
    if cfg!(target_arch = "x86_64") {
        // The x32 interface of x86_64.
        f.push(jump(JGE_K, 0x4000_0000, 0, 1));
        f.push(stmt(RET_K, RET_ERRNO | EPERM));
    }
    for &number in nr::DENIED.iter().chain([nr::SOCKET].iter()).chain(extra) {
        f.push(jump(JEQ_K, number, 0, 1));
        f.push(stmt(RET_K, RET_ERRNO | EPERM));
    }
    // clone3 passes its flags in memory, which a filter cannot read: the C library then falls
    // back to clone, whose flags are checked.
    f.push(jump(JEQ_K, CLONE3, 0, 1));
    f.push(stmt(RET_K, RET_ERRNO | ENOSYS));
    f.push(jump(JEQ_K, nr::CLONE, 0, 3));
    f.push(stmt(LD_W_ABS, offset_arg(0)));
    f.push(jump(JSET_K, CLONE_NAMESPACES, 0, 1));
    f.push(stmt(RET_K, RET_ERRNO | EPERM));
    // The number is no longer in the accumulator after the clone check, so ioctl is checked
    // after reloading it.
    f.push(stmt(LD_W_ABS, OFFSET_NR));
    f.push(jump(JEQ_K, nr::IOCTL, 0, 4));
    f.push(stmt(LD_W_ABS, offset_arg(1)));
    f.push(jump(JEQ_K, TIOCSTI, 1, 0));
    f.push(jump(JEQ_K, TIOCLINUX, 0, 1));
    f.push(stmt(RET_K, RET_ERRNO | EPERM));
    f.push(stmt(RET_K, RET_ALLOW));
    f
}

fn seccomp(extra: &[u32]) -> Result<(), String> {
    let filter = filter(extra);
    let program = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };
    // SAFETY: program points to filter, which lives until the call returns; the kernel copies it.
    if unsafe {
        prctl(
            PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            &program as *const SockFprog,
        )
    } != 0
    {
        return Err(format!("seccomp: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::spawn(f).join().unwrap()
    }

    #[test]
    fn the_filter_ends_with_allow_and_has_no_jump_out_of_it() {
        let f = filter(nr::WRITER_DENIED);
        assert!(f.len() < 200);
        assert_eq!(f.last().map(|s| (s.code, s.k)), Some((RET_K, RET_ALLOW)));
        for (i, s) in f.iter().enumerate() {
            if s.code & 0x07 == 0x05 {
                assert!(i + 1 + (s.jt.max(s.jf) as usize) < f.len());
            }
        }
    }

    /// The restrictions of the operating system hold for code that does not go through the
    /// runtime checks: Rust's own file and socket calls on the sandboxed thread.
    #[test]
    fn landlock_and_seccomp_deny_what_the_scopes_do_not_grant() {
        if landlock_abi() == 0 || !seccomp_available() {
            eprintln!("skipped: Landlock or seccomp is not available");
            return;
        }
        let dir = std::env::temp_dir().join(format!("unio-landlock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("open")).unwrap();
        std::fs::create_dir_all(dir.join("closed")).unwrap();
        std::fs::write(dir.join("open/a.txt"), "a").unwrap();
        std::fs::write(dir.join("closed/b.txt"), "b").unwrap();
        let root = std::fs::canonicalize(&dir).unwrap();
        let results = on_thread(move || {
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
            (
                std::fs::read_to_string(root.join("open/a.txt")).is_ok(),
                std::fs::read_to_string(root.join("closed/b.txt")).map_err(|e| e.kind()),
                std::fs::write(root.join("open/out.txt"), "x").is_ok(),
                std::fs::write(root.join("closed/c.txt"), "x").map_err(|e| e.kind()),
                std::net::TcpStream::connect("127.0.0.1:9").map_err(|e| e.kind()),
                std::net::UdpSocket::bind("127.0.0.1:0").is_err(),
            )
        });
        let _ = std::fs::remove_dir_all(&dir);
        let denied = Err(std::io::ErrorKind::PermissionDenied);
        assert!(results.0, "a granted file can be read");
        assert_eq!(results.1.map(|_| ()), denied);
        assert!(
            !results.2,
            "the program writes no file itself: the writer process does (docs/runtime.md 9.2)"
        );
        assert_eq!(results.3, denied);
        assert_eq!(results.4.map(|_| ()), denied);
        assert!(results.5);
    }

    /// The writer process may create and replace files only in the directories of its scopes,
    /// reads nothing else, and can neither run a program nor open a socket.
    #[test]
    fn the_writer_is_confined_to_its_write_scopes() {
        if landlock_abi() == 0 || !seccomp_available() {
            eprintln!("skipped: Landlock or seccomp is not available");
            return;
        }
        let dir = std::env::temp_dir().join(format!("unio-landlock-writer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("open")).unwrap();
        std::fs::create_dir_all(dir.join("closed")).unwrap();
        std::fs::write(dir.join("closed/b.txt"), "b").unwrap();
        let root = std::fs::canonicalize(&dir).unwrap();
        let results = on_thread(move || {
            apply_writer(&[Scope {
                root: root.join("open/out.txt"),
                subtree: false,
            }])
            .unwrap();
            (
                std::fs::write(root.join("open/.out.txt.unio-tmp-1"), "x").is_ok(),
                std::fs::rename(
                    root.join("open/.out.txt.unio-tmp-1"),
                    root.join("open/out.txt"),
                )
                .is_ok(),
                std::fs::write(root.join("closed/c.txt"), "x").is_err(),
                std::fs::read_to_string(root.join("closed/b.txt")).is_err(),
                std::process::Command::new("/bin/true").status().is_err(),
                std::net::UdpSocket::bind("127.0.0.1:0").is_err(),
            )
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(results, (true, true, true, true, true, true));
    }
}
