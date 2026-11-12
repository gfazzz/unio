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
//! - Windows: the child is created suspended, assigned to a new Job Object with kill-on-close
//!   and without breakaway, and only then resumed, so its first instruction already runs in the
//!   job and every process it starts is in the job too. A job that cannot be created,
//!   configured, or assigned fails the start: the suspended child is terminated before it ran.

use std::process::{Child, Command};

/// Prepares the command of a child so that it starts in a tree of its own: a new process group
/// (Unix) whose descendants cannot leave it (Linux), or suspended until it is in its Job Object
/// (Windows). [`Tree::start`] must follow the spawn.
pub fn prepare(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        super::keep_in_tree(command);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_SUSPENDED: u32 = 0x4;
        command.creation_flags(CREATE_SUSPENDED);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = command;
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
    pub fn start(child: &mut Child, marker: Option<Marker>) -> std::io::Result<Tree> {
        match imp::Tree::start(child, marker) {
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
}

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
    }

    impl Tree {
        pub fn start(child: &mut Child, marker: Option<Marker>) -> std::io::Result<Tree> {
            let group = i32::try_from(child.id()).unwrap_or(0);
            if group <= 0 {
                return Err(std::io::Error::other("the child has no process number"));
            }
            register(group);
            Ok(Tree {
                group,
                live: true,
                marker,
            })
        }

        pub fn stop(&mut self) {
            if self.live {
                stop_group(self.group, self.marker.as_ref());
                unregister(self.group);
                self.live = false;
            }
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
    use super::Marker;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

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
    struct ThreadEntry {
        size: u32,
        usage: u32,
        thread: u32,
        owner: u32,
        base_priority: i32,
        delta_priority: i32,
        flags: u32,
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
        fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> Handle;
        fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
        fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
        fn OpenThread(access: u32, inherit: i32, thread: u32) -> Handle;
        fn ResumeThread(thread: Handle) -> u32;
    }
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    /// The flags of the job: kill-on-close only. Neither JOB_OBJECT_LIMIT_BREAKAWAY_OK
    /// (0x800) nor JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK (0x1000) is set, so a process in the
    /// job cannot start a child outside it with CREATE_BREAKAWAY_FROM_JOB.
    const JOB_FLAGS: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    const _: () = assert!(JOB_FLAGS & (0x800 | 0x1000) == 0);
    const TH32CS_SNAPTHREAD: u32 = 0x4;
    const THREAD_SUSPEND_RESUME: u32 = 0x2;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    /// A Job Object that holds the child and every process it starts; closing it stops them
    /// all (kill-on-close).
    pub struct Tree {
        job: Handle,
    }

    // SAFETY: the job handle is owned by the tree and may be used from any thread.
    unsafe impl Send for Tree {}

    fn last_error(what: &str) -> std::io::Error {
        let e = std::io::Error::last_os_error();
        std::io::Error::new(e.kind(), format!("{what}: {e}"))
    }

    impl Tree {
        pub fn start(child: &mut Child, _: Option<Marker>) -> std::io::Result<Tree> {
            // SAFETY: a new unnamed job with default security; the limits structure is the
            // documented JOBOBJECT_EXTENDED_LIMIT_INFORMATION; the process handle belongs to
            // the child, which is alive and suspended.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() {
                    return Err(last_error("the Job Object cannot be created"));
                }
                let tree = Tree { job };
                let mut limits = ExtendedLimits::default();
                limits.basic.flags = JOB_FLAGS;
                if SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&mut limits as *mut ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                ) == 0
                {
                    return Err(last_error("the Job Object cannot be configured"));
                }
                if AssignProcessToJobObject(job, child.as_raw_handle()) == 0 {
                    return Err(last_error("the child cannot be put in its Job Object"));
                }
                resume(child.id())?;
                Ok(tree)
            }
        }

        pub fn stop(&mut self) {
            if !self.job.is_null() {
                // SAFETY: the job handle is owned by this tree and closed once.
                unsafe {
                    TerminateJobObject(self.job, 1);
                    CloseHandle(self.job);
                }
                self.job = std::ptr::null_mut();
            }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// Resumes the threads of a process created suspended: its one main thread.
    fn resume(process: u32) -> std::io::Result<()> {
        // SAFETY: a snapshot of the threads of the system, walked with an entry of the
        // documented THREADENTRY32 layout; each thread handle is closed after use.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
                return Err(last_error("the threads of the child cannot be listed"));
            }
            let mut entry = ThreadEntry {
                size: std::mem::size_of::<ThreadEntry>() as u32,
                usage: 0,
                thread: 0,
                owner: 0,
                base_priority: 0,
                delta_priority: 0,
                flags: 0,
            };
            let mut resumed = 0;
            let mut failed = None;
            let mut more = Thread32First(snapshot, &mut entry) != 0;
            while more {
                if entry.owner == process {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.thread);
                    if thread.is_null() || ResumeThread(thread) == u32::MAX {
                        failed = Some(last_error("the child cannot be resumed"));
                    } else {
                        resumed += 1;
                    }
                    if !thread.is_null() {
                        CloseHandle(thread);
                    }
                }
                more = Thread32Next(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            match failed {
                Some(e) => Err(e),
                None if resumed == 0 => Err(std::io::Error::other(
                    "the child cannot be resumed: it has no thread",
                )),
                None => Ok(()),
            }
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
        pub fn start(_: &mut Child, _: Option<Marker>) -> std::io::Result<Tree> {
            Ok(Tree)
        }

        pub fn stop(&mut self) {}
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
