//! The sandbox of the worker processes of the tools (requirements SB-10, SB-11, SB-14;
//! docs/runtime.md 9.7).
//!
//! `unio check`, `fmt`, `build`, `outline`, `symbol`, `parse`, and the compile steps of `unio
//! test` and `unio run` read a project that may be hostile. Their work runs in a worker: the
//! same executable, started by the tool with an empty environment except the variables that
//! select the language of messages, which applies the rules of this module to itself before it
//! reads anything of the project:
//!
//! - Linux: `no_new_privs`, Landlock (read the read trees, read and write the write trees,
//!   execute only the listed files, no TCP, signals only inside the sandbox), seccomp (no
//!   socket, no exec, no fork, `clone` only for threads, and the calls a program may never
//!   make), and resource limits: address space, CPU time, file size, no core files;
//! - macOS: a deny-by-default Seatbelt profile (read and write trees, no process, no network)
//!   and resource limits; the tool watches the memory footprint, which macOS does not limit;
//! - Windows: a Job Object with the memory limit and one active process, so the worker cannot
//!   start another one. The file system and the network are not restricted there yet
//!   (AppContainer is not implemented, SB-4); the tool says so.
//!
//! The wall-time limit is kept by the tool, which stops the worker.

use super::Scope;
use std::path::PathBuf;

/// The default memory limit of a worker (SB-11): 2 GiB.
pub const DEFAULT_MEMORY_BYTES: u64 = 2 << 30;
/// The default time limit of a worker (SB-11): 10 minutes.
pub const DEFAULT_WALL_SECONDS: u64 = 600;

/// The limits of one worker; `None` leaves a limit to the system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub memory_bytes: Option<u64>,
    pub cpu_seconds: Option<u64>,
    pub file_size_bytes: Option<u64>,
}

/// What a worker may do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Trees and files it may read.
    pub read: Vec<Scope>,
    /// Trees it may read and write: its own temporary directory.
    pub write: Vec<Scope>,
    /// Files it may execute; only the link step of `unio build` has any.
    pub execute: Vec<PathBuf>,
    /// Whether it may start processes: only the link step, which runs the C compiler.
    pub processes: bool,
    pub limits: Limits,
    /// The marker of the process tree the tool gave the link step: on macOS its profile carries
    /// it, so that the tool finds every process of the step to stop it (`tree.rs`).
    pub marker: Option<super::tree::Marker>,
}

/// What the link step of `unio build` may read and execute besides its own files.
#[derive(Clone, Debug, Default)]
pub struct Toolchain {
    pub read: Vec<Scope>,
    pub execute: Vec<PathBuf>,
}

/// The toolchain of the listed programs (the C compiler and the programs it runs to link), as
/// for the children of a program (docs/runtime.md 9.5): the library directories and the cache
/// of the dynamic loader may be read; exactly the programs, their interpreters and their
/// dynamic loader may be executed, and the libraries they need outside the library directories
/// may be read. On macOS the programs come from the developer tools (Xcode or the command line
/// tools), whose tree may be read and its libraries mapped, and the system libraries come from
/// the dyld shared cache and `/usr/lib`. Empty where no link step exists (Windows links with
/// the variables of Visual Studio and has no file system restrictions yet, SB-4).
pub fn toolchain(programs: &std::collections::BTreeSet<String>) -> Toolchain {
    #[cfg(target_os = "linux")]
    {
        let library_dirs: Vec<PathBuf> = super::linux::LIBRARY_DIRS
            .iter()
            .map(std::path::Path::new)
            .filter(|d| d.is_dir())
            .map(super::resolve)
            .collect();
        let mut read: Vec<Scope> = library_dirs
            .iter()
            .map(|d| Scope {
                root: d.clone(),
                subtree: true,
            })
            .collect();
        for file in super::linux::CHILD_FILES {
            read.push(Scope {
                root: PathBuf::from(file),
                subtree: false,
            });
        }
        let needs = super::children::needs(programs, &library_dirs);
        read.extend(needs.read.into_iter().map(|root| Scope {
            root,
            subtree: false,
        }));
        Toolchain {
            read,
            execute: needs.execute.into_iter().collect(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let needs = super::children::needs(programs, &[]);
        let mut read: Vec<Scope> = Vec::new();
        for path in &needs.execute {
            if let Some(root) = super::macos::developer_root(path) {
                let scope = Scope {
                    root,
                    subtree: true,
                };
                if !read.contains(&scope) {
                    read.push(scope);
                }
            }
        }
        read.extend(needs.read.into_iter().map(|root| Scope {
            root,
            subtree: false,
        }));
        Toolchain {
            read,
            execute: needs.execute.into_iter().collect(),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = programs;
        Toolchain::default()
    }
}

/// The restrictions the operating system cannot apply to a worker here (SB-5).
pub fn missing() -> Vec<String> {
    #[cfg(target_os = "linux")]
    return super::linux::missing();
    #[cfg(target_os = "macos")]
    return Vec::new();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return vec![
        "file system and network restrictions of the operating system for the worker \
         (AppContainer is not implemented)"
            .to_string(),
    ];
}

/// Applies the rules to the calling process. On Linux the restrictions hold for the calling
/// thread and everything it starts later, so the worker applies them on its main thread before
/// it starts any other.
pub fn apply(rules: &Rules) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let roots = |scopes: &[Scope]| scopes.iter().map(|s| s.root.clone()).collect::<Vec<_>>();
        super::linux::apply_worker(
            &roots(&rules.read),
            &roots(&rules.write),
            &rules.execute,
            rules.processes,
            &rules.limits,
        )
    }
    #[cfg(target_os = "macos")]
    {
        apply_unix_limits(&rules.limits)?;
        if rules.processes {
            return super::macos::apply_link_worker(
                &rules.read,
                &rules.write,
                super::working_directory(),
                &rules.execute,
                rules.marker.as_ref(),
            );
        }
        if !rules.execute.is_empty() {
            return Err("a worker that starts no process executes nothing".into());
        }
        super::macos::apply_worker(&rules.read, &rules.write, super::working_directory())
    }
    #[cfg(windows)]
    {
        windows::join_job(&rules.limits, rules.processes)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = rules;
        Ok(())
    }
}

/// The resource limits of a worker on both Unix systems: no core files, which could leak what
/// the worker read; CPU time; the size of written files, where exceeding it fails the write
/// with EFBIG instead of a signal.
#[cfg(unix)]
pub(crate) fn apply_unix_limits(limits: &Limits) -> Result<(), String> {
    extern "C" {
        fn signal(signal: i32, handler: usize) -> usize;
    }
    const SIGXFSZ: i32 = 25;
    const SIG_IGN: usize = 1;
    super::set_limit(super::RLIMIT_CORE, 0).map_err(|e| format!("RLIMIT_CORE: {e}"))?;
    if let Some(seconds) = limits.cpu_seconds {
        super::set_limit(super::RLIMIT_CPU, seconds).map_err(|e| format!("RLIMIT_CPU: {e}"))?;
    }
    if let Some(bytes) = limits.file_size_bytes {
        // SAFETY: ignoring SIGXFSZ makes a write over the limit fail with EFBIG.
        unsafe {
            signal(SIGXFSZ, SIG_IGN);
        }
        super::set_limit(super::RLIMIT_FSIZE, bytes).map_err(|e| format!("RLIMIT_FSIZE: {e}"))?;
    }
    Ok(())
}

/// Ends the calling process when the tool that started it ends, so that a worker never
/// outlives its tool (Linux; elsewhere the closed pipes end it). `false` when the tool has
/// already ended.
pub fn end_with_parent(parent: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn prctl(option: i32, ...) -> i32;
            fn getppid() -> i32;
        }
        const PR_SET_PDEATHSIG: i32 = 1;
        const SIGKILL: i32 = 9;
        // SAFETY: integer arguments only; getppid takes none.
        unsafe {
            prctl(PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0);
            i64::from(getppid()) == i64::from(parent)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = parent;
        true
    }
}

/// The memory footprint of a process in bytes, as the system accounts it (macOS), for the tool
/// to stop a worker over its memory limit; `None` elsewhere or when it cannot be read.
pub fn footprint(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct RusageInfoV0 {
            uuid: [u8; 16],
            user_time: u64,
            system_time: u64,
            pkg_idle_wkups: u64,
            interrupt_wkups: u64,
            pageins: u64,
            wired_size: u64,
            resident_size: u64,
            phys_footprint: u64,
            proc_start_abstime: u64,
            proc_exit_abstime: u64,
        }
        extern "C" {
            fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
        }
        const RUSAGE_INFO_V0: i32 = 0;
        let mut info = std::mem::MaybeUninit::<RusageInfoV0>::zeroed();
        // SAFETY: the buffer is a zeroed rusage_info_v0, which the call fills.
        let result = unsafe { proc_pid_rusage(pid as i32, RUSAGE_INFO_V0, info.as_mut_ptr()) };
        if result != 0 {
            return None;
        }
        // SAFETY: the call succeeded and filled the structure.
        Some(unsafe { info.assume_init() }.phys_footprint)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(windows)]
mod windows {
    use super::Limits;

    type Handle = *mut std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        minimum_working_set: usize,
        maximum_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut std::ffi::c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: i32,
            info: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        // As declared in mod.rs: a pseudo handle.
        fn GetCurrentProcess() -> isize;
    }
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_PROCESS_TIME: u32 = 0x2;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x8;
    const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x100;
    const JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION: u32 = 0x400;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    /// Puts the worker in a new Job Object with its limits: the memory of the process, its CPU
    /// time, and, unless it may run processes, one active process, so that it cannot start
    /// another. Neither breakaway flag is set. The handle stays open for the life of the
    /// worker; closing it at exit ends anything left in the job.
    pub fn join_job(limits: &Limits, processes: bool) -> Result<(), String> {
        // SAFETY: a new unnamed job with default security; the limits structure is the
        // documented JOBOBJECT_EXTENDED_LIMIT_INFORMATION; the pseudo handle of this process.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return Err(format!("Job Object: {}", std::io::Error::last_os_error()));
            }
            let mut info = ExtendedLimits::default();
            info.basic.flags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            if !processes {
                info.basic.flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                info.basic.active_processes = 1;
            }
            if let Some(bytes) = limits.memory_bytes {
                info.basic.flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.process_memory = usize::try_from(bytes).unwrap_or(usize::MAX);
            }
            if let Some(seconds) = limits.cpu_seconds {
                info.basic.flags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
                // In units of 100 ns.
                info.basic.per_process_user_time =
                    i64::try_from(seconds.saturating_mul(10_000_000)).unwrap_or(i64::MAX);
            }
            let set = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&mut info as *mut ExtendedLimits).cast(),
                std::mem::size_of::<ExtendedLimits>() as u32,
            );
            if set == 0 || AssignProcessToJobObject(job, GetCurrentProcess() as Handle) == 0 {
                return Err(format!("Job Object: {}", std::io::Error::last_os_error()));
            }
            // The handle is kept until the process ends.
            Ok(())
        }
    }
}
