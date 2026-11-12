//! The process tree of a child: a child of `std.process.run` (docs/runtime.md 9.5) or a worker
//! of the tools (9.7), and every process it starts. When the call or the worker ends, at its
//! time limit, and on exhaustion, the whole tree is stopped, so that no descendant outlives it.
//!
//! - Linux: the child leads a new process group, and a seccomp filter installed before it runs
//!   the program denies `setsid` and `setpgid` to it and to every process it starts, so no
//!   descendant can leave the group; SIGKILL to the group stops the tree.
//! - macOS: the child leads a new process group, which a descendant can leave (`setsid`,
//!   `setpgid`). The tree is frozen with SIGSTOP and stopped with SIGKILL: the group, the
//!   descendants found through their parents, and, where the child runs under a Seatbelt
//!   profile that carries a [`Marker`], every process whose profile carries that marker. A
//!   profile cannot be left or changed by the processes it applies to and is inherited by every
//!   process they start, so the marker finds a descendant that left the group and whose parent
//!   has ended (reparented to launchd). Without a profile (a program without a sandbox, as in
//!   the tests of the runtime), such a descendant is not found.
//! - Windows: the child is created suspended (`windows::spawn`), assigned to a new Job Object
//!   with kill-on-close and without breakaway, and only then resumed, so its first instruction
//!   already runs in the job and every process it starts is in the job too. A job that cannot
//!   be created, configured, or assigned fails the start: the suspended child is terminated
//!   before it ran.
//!
//! The limit of processes of a program (`limits.processes`) counts every process of the trees
//! of its children (docs/runtime.md 9.5): on Linux a seccomp filter of the program makes each
//! creation wait for the runtime, which fails it past the limit (`linux.rs`); on Windows the
//! job holds at most the processes left and reports each new one, and a tree that started more
//! is stopped; on macOS the tree is searched for new processes while the call waits, and a
//! tree found over the limit is stopped.

#[cfg(windows)]
pub use super::windows::Child;
#[cfg(unix)]
pub use std::process::Child;
#[cfg(not(any(unix, windows)))]
pub use std::process::Child;

/// Prepares the command of a child so that it starts in a tree of its own: a new process group
/// (Unix) whose descendants cannot leave it (Linux). [`Tree::start`] must follow the spawn. On
/// Windows children are created suspended by `windows::spawn` instead.
#[cfg(unix)]
pub fn prepare(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    super::keep_in_tree(command);
}

/// A unique mark of the Seatbelt profile of a process tree on macOS: the profile allows reading
/// one preference domain and denies reading another, both named after a random token. No
/// other profile treats these two domains differently, and a process without a profile may
/// read both, so `sandbox_check` tells the processes of the tree from all others. The marker
/// names preference domains, not paths: `sandbox_check` reports a path that does not exist as
/// denied for every process, with or without a profile, so a pair of paths that exist nowhere
/// would mark no process. Reading a preference goes through `cfprefsd`, which the profiles do
/// not let a process reach (`mach-lookup` is denied), so the allowed domain grants nothing.
/// On other systems the marker is carried but not used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker(String);

impl Marker {
    /// A new marker with a random token.
    pub fn new() -> Marker {
        use std::hash::{BuildHasher, Hasher};
        let mut token = String::new();
        for part in 0..2u64 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            hasher.write_u64(part);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            hasher.write_u128(nanos);
            token.push_str(&format!("{:016x}", hasher.finish()));
        }
        Marker(token)
    }

    /// A marker from its token, as [`Marker::token`] gives it; `None` for anything else.
    pub fn from_token(token: &str) -> Option<Marker> {
        (token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Marker(token.to_string()))
    }

    pub fn token(&self) -> &str {
        &self.0
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn domain(&self, side: &str) -> String {
        format!("unio.tree.{}.{side}", self.0)
    }

    /// The rules of the profile that carry the marker; they go last, so that no other rule
    /// overrides them.
    pub fn rules(&self) -> String {
        format!(
            "(allow user-preference-read (preference-domain \"{}\"))\n\
             (deny user-preference-read (preference-domain \"{}\"))\n",
            self.domain("member"),
            self.domain("outside")
        )
    }
}

impl Default for Marker {
    fn default() -> Marker {
        Marker::new()
    }
}

/// The marker of the profile of this process, the same for all its children.
pub fn own_marker() -> &'static Marker {
    static OWN: std::sync::OnceLock<Marker> = std::sync::OnceLock::new();
    OWN.get_or_init(Marker::new)
}

static MARKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Records that the profile of this process carries [`own_marker`], so that the trees of its
/// children are stopped by it too (macOS).
pub fn set_marked() {
    MARKED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// [`own_marker`] when the profile of this process carries it.
pub fn applied_marker() -> Option<&'static Marker> {
    MARKED
        .load(std::sync::atomic::Ordering::SeqCst)
        .then(own_marker)
}

/// A started child and its process tree.
pub struct Tree {
    imp: imp::Tree,
}

impl Tree {
    /// Takes control of the tree of a child spawned with a command [`prepare`]d. On Windows it
    /// puts the suspended child in its Job Object and resumes it; when that fails the child is
    /// terminated and reaped, and the error is returned: the child never ran. `marker` is the
    /// marker of the Seatbelt profile the child runs under, if any (macOS).
    ///
    /// Within a program with a limit of processes, the tree counts its processes against it.
    pub fn start(child: &mut Child, marker: Option<Marker>) -> std::io::Result<Tree> {
        match imp::Tree::start(child, marker, super::process_budget()) {
            Ok(imp) => Ok(Tree { imp }),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }

    /// Stops every process of the tree. On Unix it must run before the child is reaped, so that
    /// the number of the process group still belongs to this tree.
    pub fn stop(&mut self) {
        self.imp.stop();
    }

    /// Whether the tree has started more processes than the limit of the program allows; such
    /// a tree is stopped (Windows, macOS). On Linux a creation past the limit fails instead, so
    /// a tree is never over it.
    pub fn over_limit(&mut self) -> bool {
        self.imp.over_limit()
    }
}

/// The processes of a tree the limit still allows besides those already counted.
pub type Budget = Option<(u64, std::sync::Arc<std::sync::atomic::AtomicU64>)>;

/// Whether the child has ended, without reaping it on Unix (waitid with WNOWAIT), so that its
/// tree can be stopped before its number is free again.
pub fn has_exited(child: &mut Child) -> std::io::Result<bool> {
    imp::has_exited(child)
}

/// Stops the trees of children that still run; for the watchdog of the time limits of a
/// program, which ends the process. On Windows kill-on-close of the jobs stops them when the
/// process ends.
pub fn stop_all() {
    #[cfg(unix)]
    imp::stop_registered();
}

/// Makes SIGINT, SIGTERM, SIGHUP, and SIGQUIT stop the process groups of the running trees
/// before they end this process (Unix): a child in a group of its own does not receive the
/// signals of the terminal. Installed for a signal only while it has its default action. On
/// macOS the handler stops the groups only, since searching for descendants is not safe in a
/// signal handler. On Windows the jobs end with the process (kill-on-close).
pub fn stop_on_termination() {
    #[cfg(unix)]
    imp::stop_on_termination();
}

#[cfg(unix)]
mod imp {
    use super::Marker;
    use std::process::Child;
    use std::sync::atomic::{AtomicI32, Ordering};

    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
        fn waitid(idtype: u32, id: u32, info: *mut u8, options: i32) -> i32;
        fn signal(signal: i32, handler: usize) -> usize;
        fn raise(signal: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    const P_PID: u32 = 1;
    const WEXITED: i32 = 4;
    const WNOHANG: i32 = 1;
    #[cfg(target_os = "linux")]
    const WNOWAIT: i32 = 0x0100_0000;
    #[cfg(not(target_os = "linux"))]
    const WNOWAIT: i32 = 0x20;

    /// The process groups of the running trees, in slots a signal handler can read.
    static GROUPS: [AtomicI32; 256] = [const { AtomicI32::new(0) }; 256];

    fn register(group: i32) {
        for slot in &GROUPS {
            if slot
                .compare_exchange(0, group, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
    }

    fn unregister(group: i32) {
        for slot in &GROUPS {
            if slot
                .compare_exchange(group, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
    }

    /// The process group of the child, whose number is the number of the child.
    pub struct Tree {
        group: i32,
        live: bool,
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        marker: Option<Marker>,
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        budget: super::Budget,
        /// The processes of the tree found so far (macOS).
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        seen: Vec<i32>,
    }

    impl Tree {
        pub fn start(
            child: &mut Child,
            marker: Option<Marker>,
            budget: super::Budget,
        ) -> std::io::Result<Tree> {
            let group = i32::try_from(child.id()).unwrap_or(0);
            if group <= 0 {
                return Err(std::io::Error::other("the child has no process number"));
            }
            register(group);
            Ok(Tree {
                group,
                live: true,
                marker,
                budget,
                seen: vec![group],
            })
        }

        pub fn stop(&mut self) {
            if self.live {
                stop_group(self.group, self.marker.as_ref());
                unregister(self.group);
                self.live = false;
            }
        }

        /// Linux: the filter of the program fails every creation past the limit.
        #[cfg(not(target_os = "macos"))]
        pub fn over_limit(&mut self) -> bool {
            false
        }

        /// macOS: the descendants are searched through their parents and the marker, and each
        /// process not seen before counts; a tree over the limit is stopped. A process that
        /// starts and ends between two searches is not seen.
        #[cfg(target_os = "macos")]
        pub fn over_limit(&mut self) -> bool {
            use std::sync::atomic::Ordering;
            let Some((limit, counted)) = &self.budget else {
                return false;
            };
            if !self.live {
                return false;
            }
            let own = std::process::id() as i32;
            let mut next = 0;
            while next < self.seen.len() && self.seen.len() < 4096 {
                let parent = self.seen[next];
                next += 1;
                for pid in macos::children_of(parent) {
                    if pid > 0 && pid != own && !self.seen.contains(&pid) {
                        self.seen.push(pid);
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            if let Some(marker) = &self.marker {
                for pid in macos::all_pids() {
                    if pid > 1
                        && pid != own
                        && !self.seen.contains(&pid)
                        && macos::carries(pid, marker)
                    {
                        self.seen.push(pid);
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            if counted.load(Ordering::SeqCst) > *limit {
                self.stop();
                return true;
            }
            false
        }
    }

    /// On Linux no descendant can leave the group (the filter of `sandbox::keep_in_tree`), so
    /// SIGKILL to the group stops them all.
    #[cfg(not(target_os = "macos"))]
    fn stop_group(group: i32, _: Option<&Marker>) {
        // SAFETY: kill with a negative number signals the process group; no memory effects.
        unsafe {
            kill(-group, SIGKILL);
        }
    }

    /// On macOS, where a descendant can leave the group: the group is frozen with SIGSTOP; the
    /// descendants are found through their parents and, with a marker, through their profile,
    /// and frozen in turn, until two rounds find nothing new, so that none starts another
    /// process meanwhile; then all of them and the group get SIGKILL.
    #[cfg(target_os = "macos")]
    fn stop_group(group: i32, marker: Option<&Marker>) {
        const SIGSTOP: i32 = 17;
        const MOST: usize = 4096;
        // SAFETY: kill has no memory effects.
        unsafe {
            kill(-group, SIGSTOP);
        }
        let own = std::process::id() as i32;
        let mut found = vec![group];
        let mut next = 0;
        let mut quiet = 0;
        for _ in 0..64 {
            let before = found.len();
            while next < found.len() && found.len() < MOST {
                let parent = found[next];
                next += 1;
                for pid in macos::children_of(parent) {
                    if pid > 0 && pid != own && !found.contains(&pid) {
                        // SAFETY: as above.
                        unsafe {
                            kill(pid, SIGSTOP);
                        }
                        found.push(pid);
                    }
                }
            }
            if let Some(marker) = marker {
                for pid in macos::all_pids() {
                    if pid > 1 && pid != own && !found.contains(&pid) && macos::carries(pid, marker)
                    {
                        // SAFETY: as above.
                        unsafe {
                            kill(pid, SIGSTOP);
                        }
                        found.push(pid);
                    }
                }
            }
            if found.len() == before && next == found.len() {
                quiet += 1;
                if quiet >= 2 || marker.is_none() {
                    break;
                }
            } else {
                quiet = 0;
            }
            if found.len() >= MOST {
                break;
            }
        }
        // SAFETY: as above.
        unsafe {
            kill(-group, SIGKILL);
            for &pid in &found[1..] {
                kill(pid, SIGKILL);
            }
        }
    }

    pub fn has_exited(child: &mut Child) -> std::io::Result<bool> {
        let mut info = [0u8; 256];
        // SAFETY: info is larger than siginfo_t on every Unix the runtime supports; waitid
        // writes at most one siginfo_t.
        let result = unsafe {
            waitid(
                P_PID,
                child.id(),
                info.as_mut_ptr(),
                WEXITED | WNOHANG | WNOWAIT,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // si_signo is the first int of siginfo_t; it stays 0 when no child has ended.
        Ok(i32::from_ne_bytes([info[0], info[1], info[2], info[3]]) != 0)
    }

    pub fn stop_registered() {
        let marker = super::applied_marker();
        for slot in &GROUPS {
            let group = slot.load(Ordering::SeqCst);
            if group > 0 {
                stop_group(group, marker);
            }
        }
    }

    const SIG_DFL: usize = 0;

    extern "C" fn on_termination(sig: i32) {
        for slot in &GROUPS {
            let group = slot.load(Ordering::SeqCst);
            if group > 0 {
                // SAFETY: kill is async-signal-safe.
                unsafe {
                    kill(-group, SIGKILL);
                }
            }
        }
        // SAFETY: signal and raise are async-signal-safe; the default action ends the process.
        unsafe {
            signal(sig, SIG_DFL);
            raise(sig);
        }
    }

    pub fn stop_on_termination() {
        static INSTALLED: std::sync::Once = std::sync::Once::new();
        INSTALLED.call_once(|| {
            // SIGHUP, SIGINT, SIGQUIT, SIGTERM have the same numbers on Linux and macOS.
            for sig in [1, 2, 3, 15] {
                // SAFETY: the handler only makes async-signal-safe calls; a disposition other
                // than the default is put back at once.
                unsafe {
                    let previous = signal(sig, on_termination as extern "C" fn(i32) as usize);
                    if previous != SIG_DFL {
                        signal(sig, previous);
                    }
                }
            }
        });
    }

    #[cfg(target_os = "macos")]
    pub(super) mod macos {
        use super::super::Marker;
        use std::ffi::CString;

        extern "C" {
            fn proc_listpids(kind: u32, info: u32, buffer: *mut i32, size: i32) -> i32;
            fn sandbox_check(
                pid: i32,
                operation: *const std::os::raw::c_char,
                kind: i32,
                ...
            ) -> i32;
        }
        const PROC_ALL_PIDS: u32 = 1;
        const PROC_PPID_ONLY: u32 = 6;
        /// `SANDBOX_FILTER_PREFERENCE_DOMAIN` of libsandbox: the argument names a preference
        /// domain, as `cfprefsd` checks its clients.
        const SANDBOX_FILTER_PREFERENCE_DOMAIN: i32 = 6;
        /// `SANDBOX_CHECK_NO_REPORT` of libsandbox: a denied check is not logged.
        const SANDBOX_CHECK_NO_REPORT: i32 = 0x4000_0000;

        fn list(kind: u32, info: u32) -> Vec<i32> {
            let mut buffer = vec![0i32; 4096];
            loop {
                let size = i32::try_from(buffer.len() * 4).unwrap_or(i32::MAX);
                // SAFETY: the buffer holds size bytes; the call writes at most that many and
                // returns how many it wrote.
                let bytes = unsafe { proc_listpids(kind, info, buffer.as_mut_ptr(), size) };
                let count = usize::try_from(bytes).unwrap_or(0) / 4;
                if count < buffer.len() || buffer.len() >= 1 << 20 {
                    buffer.truncate(count.min(buffer.len()));
                    return buffer;
                }
                buffer = vec![0i32; buffer.len() * 4];
            }
        }

        pub fn children_of(parent: i32) -> Vec<i32> {
            list(PROC_PPID_ONLY, parent as u32)
        }

        pub fn all_pids() -> Vec<i32> {
            list(PROC_ALL_PIDS, 0)
        }

        /// What `sandbox_check` answers for reading the preference domain in the process: 0
        /// allowed, 1 denied, -1 an error.
        fn check(pid: i32, domain: &str) -> i32 {
            let operation = c"user-preference-read";
            let Ok(domain) = CString::new(domain) else {
                return -1;
            };
            // SAFETY: two C strings that live through the call; the variadic argument is the
            // domain the filter type names.
            unsafe {
                sandbox_check(
                    pid,
                    operation.as_ptr(),
                    SANDBOX_FILTER_PREFERENCE_DOMAIN | SANDBOX_CHECK_NO_REPORT,
                    domain.as_ptr(),
                )
            }
        }

        /// Whether the profile of the process carries the marker: the domain `member` is
        /// allowed and the domain `outside` denied.
        pub fn carries(pid: i32, marker: &Marker) -> bool {
            check(pid, &marker.domain("member")) == 0 && check(pid, &marker.domain("outside")) == 1
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{Budget, Child, Marker};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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

    #[repr(C)]
    struct CompletionPort {
        key: *mut std::ffi::c_void,
        port: Handle,
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
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
        fn GetQueuedCompletionStatus(
            port: Handle,
            bytes: *mut u32,
            key: *mut usize,
            overlapped: *mut *mut std::ffi::c_void,
            milliseconds: u32,
        ) -> i32;
        fn PostQueuedCompletionStatus(
            port: Handle,
            bytes: u32,
            key: usize,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
        fn DuplicateHandle(
            source_process: Handle,
            source: Handle,
            target_process: Handle,
            target: *mut Handle,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        // As declared in mod.rs: a pseudo handle.
        fn GetCurrentProcess() -> isize;
    }
    const JOB_OBJECT_BASIC_LIMIT_INFORMATION: i32 = 2;
    const JOB_OBJECT_ASSOCIATE_COMPLETION_PORT_INFORMATION: i32 = 7;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x8;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_MSG_NEW_PROCESS: u32 = 6;
    const DUPLICATE_SAME_ACCESS: u32 = 0x2;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    /// The key of the packet that ends the thread that watches a job.
    const QUIT: usize = 1;
    /// The flags of the job: kill-on-close only. Neither JOB_OBJECT_LIMIT_BREAKAWAY_OK
    /// (0x800) nor JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK (0x1000) is set, so a process in the
    /// job cannot start a child outside it with CREATE_BREAKAWAY_FROM_JOB.
    const JOB_FLAGS: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    const _: () = assert!(JOB_FLAGS & (0x800 | 0x1000) == 0);

    /// A Job Object that holds the child and every process it starts; closing it stops them
    /// all (kill-on-close). With a limit of processes, the job holds at most the processes the
    /// limit leaves at once, and a thread counts every new process from the messages of the
    /// job: when the count reaches the limit, no further process may join the job; a tree that
    /// started more (in the moment before that) is terminated.
    pub struct Tree {
        job: Handle,
        port: Handle,
        exceeded: Arc<AtomicBool>,
    }

    // SAFETY: the handles are owned by the tree and may be used from any thread.
    unsafe impl Send for Tree {}

    fn last_error(what: &str) -> std::io::Error {
        let e = std::io::Error::last_os_error();
        std::io::Error::new(e.kind(), format!("{what}: {e}"))
    }

    unsafe fn set_active_limit(job: Handle, processes: u32) -> bool {
        let mut limits = BasicLimits {
            flags: JOB_FLAGS | JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            active_processes: processes.max(1),
            ..BasicLimits::default()
        };
        SetInformationJobObject(
            job,
            JOB_OBJECT_BASIC_LIMIT_INFORMATION,
            (&mut limits as *mut BasicLimits).cast(),
            std::mem::size_of::<BasicLimits>() as u32,
        ) != 0
    }

    impl Tree {
        pub fn start(
            child: &mut Child,
            _: Option<Marker>,
            budget: Budget,
        ) -> std::io::Result<Tree> {
            // SAFETY: a new unnamed job with default security; the limits structure is the
            // documented JOBOBJECT_EXTENDED_LIMIT_INFORMATION; the process handle belongs to
            // the child, which is alive and suspended.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return Err(last_error("the Job Object cannot be created"));
                }
                let mut tree = Tree {
                    job,
                    port: std::ptr::null_mut(),
                    exceeded: Arc::new(AtomicBool::new(false)),
                };
                let mut limits = ExtendedLimits::default();
                limits.basic.flags = JOB_FLAGS;
                if let Some((limit, counted)) = &budget {
                    // The child itself was counted when the call started it.
                    let left = limit.saturating_sub(counted.load(Ordering::SeqCst)) + 1;
                    limits.basic.flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                    limits.basic.active_processes = u32::try_from(left).unwrap_or(u32::MAX);
                }
                if SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&mut limits as *mut ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                {
                    return Err(last_error("the Job Object cannot be configured"));
                }
                if let Some((limit, counted)) = budget {
                    tree.watch(limit, counted, child.id())?;
                }
                if AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                    return Err(last_error("the child cannot be put in its Job Object"));
                }
                child.resume()?;
                Ok(tree)
            }
        }

        /// Starts the thread that counts the new processes of the job.
        unsafe fn watch(
            &mut self,
            limit: u64,
            counted: Arc<std::sync::atomic::AtomicU64>,
            child: u32,
        ) -> std::io::Result<()> {
            let port = CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1);
            if port.is_null() {
                return Err(last_error(
                    "the messages of the Job Object cannot be received",
                ));
            }
            self.port = port;
            let mut association = CompletionPort {
                key: std::ptr::null_mut(),
                port,
            };
            if SetInformationJobObject(
                self.job,
                JOB_OBJECT_ASSOCIATE_COMPLETION_PORT_INFORMATION,
                (&mut association as *mut CompletionPort).cast(),
                std::mem::size_of::<CompletionPort>() as u32,
            ) == 0
            {
                return Err(last_error(
                    "the messages of the Job Object cannot be received",
                ));
            }
            // The thread keeps handles of its own, so that stopping the tree never closes a
            // handle it still uses.
            let mut job: Handle = std::ptr::null_mut();
            let mut own_port: Handle = std::ptr::null_mut();
            let process = GetCurrentProcess() as Handle;
            if DuplicateHandle(
                process,
                self.job,
                process,
                &mut job,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            ) == 0
                || DuplicateHandle(
                    process,
                    port,
                    process,
                    &mut own_port,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                ) == 0
            {
                return Err(last_error("the Job Object cannot be watched"));
            }
            let exceeded = Arc::clone(&self.exceeded);
            let (job, own_port) = (job as usize, own_port as usize);
            let spawned = std::thread::Builder::new()
                .name("unio-job".into())
                .spawn(move || {
                    let (job, port) = (job as Handle, own_port as Handle);
                    loop {
                        let mut message = 0u32;
                        let mut key = 0usize;
                        let mut pid: *mut std::ffi::c_void = std::ptr::null_mut();
                        // SAFETY: the port is owned by this thread; the outputs are locals.
                        let got = unsafe {
                            GetQueuedCompletionStatus(
                                port,
                                &mut message,
                                &mut key,
                                &mut pid,
                                u32::MAX,
                            )
                        };
                        if got == 0 || key == QUIT {
                            break;
                        }
                        if message != JOB_OBJECT_MSG_NEW_PROCESS || pid as usize as u32 == child {
                            continue;
                        }
                        let now = counted.fetch_add(1, Ordering::SeqCst) + 1;
                        // SAFETY: the job handle is owned by this thread.
                        unsafe {
                            if now >= limit {
                                // No further process may join the job.
                                set_active_limit(job, 1);
                            }
                            if now > limit {
                                exceeded.store(true, Ordering::SeqCst);
                                TerminateJobObject(job, 1);
                            }
                        }
                    }
                    // SAFETY: both handles are owned by this thread.
                    unsafe {
                        CloseHandle(job);
                        CloseHandle(port);
                    }
                });
            if spawned.is_err() {
                return Err(std::io::Error::other("the Job Object cannot be watched"));
            }
            Ok(())
        }

        pub fn stop(&mut self) {
            // SAFETY: the handles are owned by this tree and closed once.
            unsafe {
                if !self.job.is_null() {
                    TerminateJobObject(self.job, 1);
                    CloseHandle(self.job);
                    self.job = std::ptr::null_mut();
                }
                if !self.port.is_null() {
                    PostQueuedCompletionStatus(self.port, 0, QUIT, std::ptr::null_mut());
                    CloseHandle(self.port);
                    self.port = std::ptr::null_mut();
                }
            }
        }

        pub fn over_limit(&mut self) -> bool {
            self.exceeded.load(Ordering::SeqCst)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            self.stop();
        }
    }

    pub fn has_exited(child: &mut Child) -> std::io::Result<bool> {
        // On Windows waiting does not reuse the identity of the job: the handle is ours.
        Ok(child.try_wait()?.is_some())
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::Marker;
    use std::process::Child;

    pub struct Tree;

    impl Tree {
        pub fn start(_: &mut Child, _: Option<Marker>, _: super::Budget) -> std::io::Result<Tree> {
            Ok(Tree)
        }

        pub fn stop(&mut self) {}

        pub fn over_limit(&mut self) -> bool {
            false
        }
    }

    pub fn has_exited(child: &mut Child) -> std::io::Result<bool> {
        Ok(child.try_wait()?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_names_two_paths_of_its_own_token() {
        let a = Marker::new();
        let b = Marker::new();
        assert_ne!(a, b);
        assert_eq!(Marker::from_token(a.token()), Some(a.clone()));
        assert_eq!(Marker::from_token("../x"), None);
        let rules = a.rules();
        assert!(rules.contains(&format!("\"unio.tree.{}.member\"", a.token())));
        assert!(rules.contains("(deny user-preference-read"), "{rules}");
    }

    /// The profile of a child carries the marker, and `sandbox_check` finds it: the test
    /// binary runs itself with one test and a variable that makes that test apply a profile
    /// that denies by default, as the profile of a program does, with the marker, and wait.
    /// This process has no profile and does not carry the marker. Inside the profile, where
    /// the runtime of a program stops its trees, the child finds the marker on itself and not
    /// on this process.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_process_under_a_marked_profile_is_found_by_its_marker() {
        const VARIABLE: &str = "UNIO_TREE_MARKER_CHILD";
        if let Ok(token) = std::env::var(VARIABLE) {
            let marker = Marker::from_token(&token).unwrap();
            let profile = format!(
                "(version 1)\n(deny default)\n(allow file-read-metadata)\n\
                 (allow process-info* (target self))\n(allow process-info-listpids)\n\
                 (allow file-read* file-map-executable (subpath \"/usr/lib\") \
                 (subpath \"/System\"))\n{}",
                marker.rules()
            );
            super::super::macos::init_profile(&profile).unwrap();
            let parent: i32 = std::env::var("UNIO_TREE_MARKER_PARENT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            let itself = imp::macos::carries(std::process::id() as i32, &marker);
            let other = imp::macos::carries(parent, &marker);
            println!("marked inside: itself {itself}, parent {other}");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            return;
        }
        use std::io::{BufRead, BufReader, Write};
        let marker = Marker::new();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sandbox::tree::tests::a_process_under_a_marked_profile_is_found_by_its_marker",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(VARIABLE, marker.token())
            .env("UNIO_TREE_MARKER_PARENT", std::process::id().to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut seen = String::new();
        let mut line = String::new();
        while out.read_line(&mut line).unwrap_or(0) > 0 {
            if line.contains("marked") {
                seen = line.trim().to_string();
                break;
            }
            line.clear();
        }
        let pid = child.id() as i32;
        let found = imp::macos::carries(pid, &marker);
        let other = imp::macos::carries(pid, &Marker::new());
        let own = imp::macos::carries(std::process::id() as i32, &marker);
        let _ = child.stdin.take().unwrap().write_all(b"\n");
        let _ = child.wait();
        assert!(!seen.is_empty(), "the child did not apply the profile");
        assert!(
            seen.ends_with("marked inside: itself true, parent false"),
            "inside the profile: {seen}"
        );
        assert!(found, "the marker of the child was not found");
        assert!(!other, "another marker was found in the child");
        assert!(!own, "a process without a profile carries the marker");
    }
}
