//! The test hook that records the file operations of the atomic commit protocol (S-9, S-9b) for
//! the crash trials of `compiler/unio/tests/power_loss.rs` (docs/runtime.md 10).
//!
//! With the environment variable [`ENV`] set to `1`, every operation that changes what a file
//! system may keep after power loss is written, after it succeeded, as one line to standard
//! error: the creation of a temporary file, each write into it, each sync of a file or of a
//! directory with the call that did it (`fsync`, `fullfsync` for `fcntl(F_FULLFSYNC)`, `flush`
//! for `FlushFileBuffers`), each rename, removal, and new directory. The program records what
//! it does itself; the writer process of the sandbox (docs/runtime.md 9.2) records what it does
//! for the program, on the standard error it already inherits. The program waits for each reply
//! of the writer, which records before it replies, so the lines of both processes are in the
//! order of the operations.
//!
//! The hook only observes: it opens nothing, grants nothing, and changes no operation. The
//! program passes it to the writer, which starts with an empty environment, only when it is set.
//!
//! A line is `unio-file-trace`, the operation, and its arguments, separated by spaces; every
//! argument is hexadecimal, so that paths and data need no quoting.

use std::io::Write;
use std::sync::OnceLock;

/// The variable that turns the hook on.
pub const ENV: &str = "UNIO_TEST_FILE_TRACE";
/// The first word of a line of the trace.
pub const PREFIX: &str = "unio-file-trace";

/// Whether the hook is on in this process.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os(ENV).is_some_and(|v| v == "1"))
}

/// The call that syncs a file's data on this platform: `File::sync_all` is `fsync` on Linux,
/// `fcntl(F_FULLFSYNC)` on Apple platforms, and `FlushFileBuffers` on Windows.
pub const FILE_SYNC: &str = if cfg!(target_vendor = "apple") {
    "fullfsync"
} else if cfg!(windows) {
    "flush"
} else {
    "fsync"
};

/// Records one operation, if the hook is on.
pub fn record(operation: &str, arguments: &[&[u8]]) {
    if !enabled() {
        return;
    }
    let mut line = String::from(PREFIX);
    line.push(' ');
    line.push_str(operation);
    for argument in arguments {
        line.push(' ');
        if argument.is_empty() {
            line.push('-');
        }
        for byte in *argument {
            line.push_str(&format!("{byte:02x}"));
        }
    }
    line.push('\n');
    // One write per line; an error of standard error is not the program's business.
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

/// The bytes of a path as the trace records it.
pub fn path_bytes(path: &std::path::Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}
