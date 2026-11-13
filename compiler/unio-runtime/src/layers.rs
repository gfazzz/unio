//! The layers of the runtime and the hooks between them (requirement S-12, docs/runtime.md
//! section 11).
//!
//! - **core**: the entry of a program and its exit code, faults and their traces, checked
//!   arithmetic, and the externals of `stdlib/core` (`fault.rs`, `externs/core.rs`,
//!   `externs/math.rs`). It makes no call to the operating system and reaches the other
//!   layers only through the [`Hooks`] below.
//! - **alloc**: the heap with its limit and reference counting, the reserve for `release`
//!   (`heap.rs`, `reserve.rs`, `value.rs`), and the externals of `stdlib/alloc`
//!   (`externs/alloc.rs`, `externs/unicode.rs`, `externs/float_text.rs`).
//! - **std**: standard output and error, files, processes, clocks, entropy, and the sandbox
//!   (`externs/system.rs`, `files.rs`, `durability.rs`, `process.rs`, `clock.rs`, `sandbox/`,
//!   `deadline.rs`, `file_trace.rs`), and the hooks of the application profile
//!   ([`application`]).
//!
//! The test `the_core_layer_reaches_the_system_only_through_hooks`
//! (`compiler/unio-runtime/tests/bindings.rs`) checks the sources of the core layer, and
//! `a_core_program_makes_no_system_call` (`compiler/unio/tests/layers.rs`) runs core programs
//! under a seccomp filter that ends the process at any system call other than a write to
//! standard output or error, memory, and exit.

use std::sync::OnceLock;

/// What the core runtime needs from the layers above it. Each hook is a plain function, so
/// that calling one needs no allocation, also on the path of exhaustion.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// alloc: a fault starts; its message, its trace and the unwinding come from the reserve
    /// for `release` from here on (C-17).
    pub open_reserve: fn(),
    /// alloc: whether the reserve for `release` is open on this thread.
    pub reserve_is_open: fn() -> bool,
    /// alloc: a fault was received by `expect_fault`; the budget of the heap is as before it.
    pub close_reserve: fn(),
    /// std: bytes to standard output; a closed stream loses them.
    pub write_output: fn(&[u8]),
    /// std: bytes to standard error: diagnostics of faults and of exhaustion.
    pub write_error: fn(&[u8]),
    /// std: ends the process with the code (exhaustion, C-18).
    pub exit: fn(i32) -> !,
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Installs the hooks of a profile before the first program runs; `Err` with the hooks that
/// were already in place. In release 0.1 the application profile is the only one, and its
/// hooks are installed by the first use when no profile installed others.
pub fn install(hooks: Hooks) -> Result<(), Hooks> {
    HOOKS.set(hooks)?;
    Ok(())
}

/// The hooks in place.
#[inline]
pub fn hooks() -> &'static Hooks {
    HOOKS.get_or_init(application)
}

/// A writer of the core layer onto a hook, so that a fault is written line by line without
/// building its text (C-17).
pub struct HookWriter(pub fn(&[u8]));

impl std::io::Write for HookWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        (self.0)(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------- the application profile

/// The hooks of the application profile: the reserve of `reserve.rs` (alloc), and standard
/// output, standard error and the exit of the operating system (std).
pub fn application() -> Hooks {
    Hooks {
        open_reserve: crate::reserve::open,
        reserve_is_open: crate::reserve::is_open,
        close_reserve: crate::reserve::close,
        write_output: system_output,
        write_error: system_error,
        exit: system_exit,
    }
}

fn system_output(bytes: &[u8]) {
    use std::io::Write;
    let _ = std::io::stdout().write_all(bytes);
}

fn system_error(bytes: &[u8]) {
    use std::io::Write;
    let _ = std::io::stderr().write_all(bytes);
}

fn system_exit(code: i32) -> ! {
    std::process::exit(code)
}

// ---------------------------------------------------------------- the check of S-12

/// The environment variable of the test hook of S-12: an executable made by `unio build` run
/// with `UNIO_TEST_CORE_SECCOMP=1` installs, after its start and right before `main`, a
/// seccomp filter that ends the process with SIGSYS at any system call except a write to
/// standard output or error, the memory calls of the allocator, waiting on and waking a futex
/// (the thread of the start waiting for `main` to end), and exit. Like the other test hooks it
/// only takes authority away. Linux only; elsewhere it is ignored.
pub const TEST_CORE_SECCOMP: &str = "UNIO_TEST_CORE_SECCOMP";

/// Installs the filter of [`TEST_CORE_SECCOMP`] on every thread of the process when the
/// variable is `1`; the start reads the variable before (std), so this reads nothing.
pub fn enter_core_only(requested: bool) -> Result<(), String> {
    if !requested {
        return Ok(());
    }
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    {
        core_filter::install()
    }
    #[cfg(not(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )))]
    {
        Ok(())
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod core_filter {
    #[repr(C)]
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

    extern "C" {
        fn prctl(option: i32, ...) -> i32;
        fn syscall(number: i64, ...) -> i64;
    }

    const PR_SET_NO_NEW_PRIVS: i32 = 38;
    const SECCOMP_SET_MODE_FILTER: i64 = 1;
    const SECCOMP_FILTER_FLAG_TSYNC: i64 = 1;

    const LD_W_ABS: u16 = 0x20;
    const JEQ_K: u16 = 0x15;
    const RET_K: u16 = 0x06;
    const RET_ALLOW: u32 = 0x7fff_0000;
    const RET_KILL_PROCESS: u32 = 0x8000_0000;
    const OFFSET_NR: u32 = 0;
    const OFFSET_ARCH: u32 = 4;
    /// The low 32 bits of the first argument on a little-endian machine.
    const OFFSET_ARG0: u32 = 16;

    #[cfg(target_arch = "x86_64")]
    mod nr {
        pub const ARCH: u32 = 0xC000_003E;
        pub const SECCOMP: i64 = 317;
        pub const WRITE: u32 = 1;
        pub const WRITEV: u32 = 20;
        /// brk, mmap, munmap, mremap, madvise, mprotect; futex; rt_sigprocmask, sigaltstack
        /// (the end of a thread); exit, exit_group.
        pub const ALLOWED: &[u32] = &[12, 9, 11, 25, 28, 10, 202, 14, 131, 60, 231];
    }

    #[cfg(target_arch = "aarch64")]
    mod nr {
        pub const ARCH: u32 = 0xC000_00B7;
        pub const SECCOMP: i64 = 277;
        pub const WRITE: u32 = 64;
        pub const WRITEV: u32 = 66;
        pub const ALLOWED: &[u32] = &[214, 222, 215, 216, 233, 226, 98, 135, 132, 93, 94];
    }

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

    pub fn install() -> Result<(), String> {
        let mut f = vec![
            stmt(LD_W_ABS, OFFSET_ARCH),
            jump(JEQ_K, nr::ARCH, 1, 0),
            stmt(RET_K, RET_KILL_PROCESS),
            stmt(LD_W_ABS, OFFSET_NR),
        ];
        for &number in nr::ALLOWED {
            f.push(jump(JEQ_K, number, 0, 1));
            f.push(stmt(RET_K, RET_ALLOW));
        }
        // write and writev only to descriptors 1 and 2.
        f.push(jump(JEQ_K, nr::WRITE, 1, 0));
        f.push(jump(JEQ_K, nr::WRITEV, 0, 4));
        f.push(stmt(LD_W_ABS, OFFSET_ARG0));
        f.push(jump(JEQ_K, 1, 1, 0));
        f.push(jump(JEQ_K, 2, 0, 1));
        f.push(stmt(RET_K, RET_ALLOW));
        f.push(stmt(RET_K, RET_KILL_PROCESS));
        let program = SockFprog {
            len: f.len() as u16,
            filter: f.as_ptr(),
        };
        // SAFETY: PR_SET_NO_NEW_PRIVS takes integers only; seccomp(2) receives a pointer to
        // `program`, which points into `f`, both alive until the call returns, and the kernel
        // copies the filter.
        unsafe {
            if prctl(PR_SET_NO_NEW_PRIVS, 1i64, 0i64, 0i64, 0i64) != 0 {
                return Err(format!("no_new_privs: {}", std::io::Error::last_os_error()));
            }
            if syscall(
                nr::SECCOMP,
                SECCOMP_SET_MODE_FILTER,
                SECCOMP_FILTER_FLAG_TSYNC,
                &program as *const SockFprog,
            ) != 0
            {
                return Err(format!("seccomp: {}", std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }
}
