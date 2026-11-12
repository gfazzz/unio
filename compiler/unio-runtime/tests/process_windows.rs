//! std.process.run on Windows: the process tree of a child is a Job Object that the child joins
//! while it is still suspended, so every process it starts, even at once, is in the job and is
//! stopped with it (docs/runtime.md 9.5).

#![cfg(windows)]

use unio_runtime::{call, Outcome, Runtime, Unwind, Value};

/// The role of the test binary when it runs itself as the child or the grandchild.
const ROLE: &str = "UNIO_PROCESS_TREE_ROLE";
/// The file where the grandchild writes its process number.
const FILE: &str = "UNIO_PROCESS_TREE_FILE";
const NAME: &str = "a_process_the_child_starts_at_once_is_stopped_with_its_job";

fn run(program: &str, arguments: &[&str], timeout: i64) -> Outcome {
    let mut rt = Runtime::new(vec![]);
    let arguments = Value::list(arguments.iter().map(|a| Value::text(*a)).collect());
    call(
        &mut rt,
        "std.process.run",
        &mut [Value::text(program), arguments, Value::Int(timeout)],
    )
}

fn kind(outcome: Outcome) -> String {
    match outcome {
        Err(Unwind::Error(e)) => e
            .field("kind")
            .and_then(|k| k.variant_name())
            .unwrap()
            .to_string(),
        other => panic!("expected a ProcessError, got {other:?}"),
    }
}

type Handle = *mut std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn TerminateProcess(handle: Handle, code: u32) -> i32;
}
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_TERMINATE: u32 = 0x1;
const WAIT_OBJECT_0: u32 = 0;

/// Whether the process has ended within five seconds; a process that cannot be opened is gone.
/// One that still runs is terminated, so that a failure leaves nothing behind.
fn gone(pid: u32) -> bool {
    // SAFETY: a handle of the process, closed after use.
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE | PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return true;
        }
        let ended = WaitForSingleObject(handle, 5000) == WAIT_OBJECT_0;
        if !ended {
            TerminateProcess(handle, 1);
        }
        CloseHandle(handle);
        ended
    }
}

/// The child starts the grandchild as its first action and waits; the grandchild writes its
/// number and waits. At the deadline the job is terminated: the grandchild, which only the job
/// connects to the call, ends with it.
#[test]
fn a_process_the_child_starts_at_once_is_stopped_with_its_job() {
    let exe = std::env::current_exe().unwrap();
    let arguments = ["--exact", NAME, "--nocapture", "--test-threads=1"];
    match std::env::var(ROLE).as_deref() {
        Ok("child") => {
            let _grandchild = std::process::Command::new(&exe)
                .args(arguments)
                .env(ROLE, "grandchild")
                .spawn();
            std::thread::sleep(std::time::Duration::from_secs(60));
            return;
        }
        Ok("grandchild") => {
            let file = std::path::PathBuf::from(std::env::var_os(FILE).unwrap());
            let partial = file.with_extension("tmp");
            std::fs::write(&partial, std::process::id().to_string()).unwrap();
            std::fs::rename(&partial, &file).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(60));
            return;
        }
        _ => {}
    }
    let dir = std::env::temp_dir().join(format!("unio-process-job-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pid");
    // Without a sandbox the child inherits the environment of this process.
    std::env::set_var(ROLE, "child");
    std::env::set_var(FILE, &file);
    let exe_text = exe.to_string_lossy().into_owned();
    let outcome = run(&exe_text, &arguments, 5);
    std::env::remove_var(ROLE);
    let record = std::fs::read_to_string(&file);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(kind(outcome), "TimedOut");
    let pid: u32 = record
        .expect("the grandchild did not start in time")
        .trim()
        .parse()
        .unwrap();
    assert!(gone(pid), "the grandchild {pid} is outside the job");
}
