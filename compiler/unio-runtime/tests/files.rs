//! std.files against a real temporary directory, including the atomic commit protocol of
//! S-9c and a failed directory sync after a successful rename (S-9b).

use std::path::{Path, PathBuf};
use unio_runtime::{call, FailPoint, Outcome, Runtime, Unwind, Value};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("unio-runtime-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn p(path: &Path) -> Value {
    Value::path(path.to_string_lossy().into_owned())
}

fn kind_of(outcome: Outcome) -> String {
    match outcome {
        Err(Unwind::Error(e)) => e
            .field("kind")
            .and_then(|k| k.variant_name())
            .unwrap()
            .to_string(),
        other => panic!("expected an IoError, got {other:?}"),
    }
}

fn int_field(value: &Value, name: &str) -> i64 {
    match value.field(name) {
        Some(Value::Int(i)) => *i,
        other => panic!("{other:?}"),
    }
}

fn path_field(value: &Value, name: &str) -> PathBuf {
    match value.field(name) {
        Some(Value::Path(t)) => PathBuf::from(t.as_str()),
        other => panic!("{other:?}"),
    }
}

/// Runs the steps of AtomicFile.commit for a new content of the target.
fn commit(
    rt: &mut Runtime,
    target: &Path,
    content: &str,
    durable: bool,
) -> (i64, PathBuf, Outcome) {
    let temporary = call(rt, "std.files.system_open_temporary", &mut [p(target)]).unwrap();
    let descriptor = int_field(&temporary, "descriptor");
    let temporary_path = path_field(&temporary, "path");
    let outcome = (|| {
        call(
            rt,
            "std.files.system_write",
            &mut [Value::Int(descriptor), Value::text(content)],
        )?;
        call(
            rt,
            "std.files.system_sync_data",
            &mut [Value::Int(descriptor)],
        )?;
        call(rt, "std.files.system_close", &mut [Value::Int(descriptor)])?;
        call(
            rt,
            "std.files.system_rename",
            &mut [p(&temporary_path), p(target)],
        )?;
        if durable {
            call(
                rt,
                "std.files.system_sync_directory",
                &mut [p(target.parent().unwrap())],
            )?;
        }
        Ok(Value::Unit)
    })();
    (descriptor, temporary_path, outcome)
}

#[test]
fn the_commit_protocol_replaces_the_target() {
    let scratch = Scratch::new("commit");
    let target = scratch.path("data.txt");
    std::fs::write(&target, "old").unwrap();
    let mut rt = Runtime::new(vec![]);
    let (descriptor, temporary, outcome) = commit(&mut rt, &target, "new", true);
    outcome.unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert!(!temporary.exists());
    assert!(!rt.is_open(descriptor));
    let name = temporary
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with(".data.txt.unio-tmp-"), "{name}");
}

#[test]
fn a_failed_directory_sync_leaves_the_target_replaced_and_the_descriptor_closed_once() {
    let scratch = Scratch::new("sync-directory");
    let target = scratch.path("data.txt");
    std::fs::write(&target, "old").unwrap();
    let mut rt = Runtime::new(vec![]);
    rt.fail_point = Some(FailPoint::SyncDirectory);
    let releases = rt.releases.get();
    let (descriptor, temporary, outcome) = commit(&mut rt, &target, "new", true);
    assert_eq!(kind_of(outcome), "Other");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert!(!temporary.exists());
    assert_eq!(rt.releases.get(), releases + 1);
    // A second close does not close anything again.
    assert_eq!(
        kind_of(call(
            &mut rt,
            "std.files.system_close",
            &mut [Value::Int(descriptor)]
        )),
        "Other"
    );
    assert_eq!(rt.releases.get(), releases + 1);
    // The retried step succeeds.
    call(
        &mut rt,
        "std.files.system_sync_directory",
        &mut [p(&scratch.0)],
    )
    .unwrap();
}

#[test]
fn a_failed_close_still_releases_the_descriptor() {
    let scratch = Scratch::new("close");
    let target = scratch.path("data.txt");
    let mut rt = Runtime::new(vec![]);
    let temporary = call(
        &mut rt,
        "std.files.system_open_temporary",
        &mut [p(&target)],
    )
    .unwrap();
    let descriptor = int_field(&temporary, "descriptor");
    rt.fail_point = Some(FailPoint::Close);
    assert_eq!(
        kind_of(call(
            &mut rt,
            "std.files.system_close",
            &mut [Value::Int(descriptor)]
        )),
        "Other"
    );
    assert!(!rt.is_open(descriptor));
    call(
        &mut rt,
        "std.files.system_remove",
        &mut [p(&path_field(&temporary, "path"))],
    )
    .unwrap();
}

#[test]
fn a_failed_rename_leaves_the_target_unchanged() {
    let scratch = Scratch::new("rename-fails");
    let target = scratch.path("data.txt");
    std::fs::write(&target, "old").unwrap();
    let mut rt = Runtime::new(vec![]);
    rt.fail_point = Some(FailPoint::Rename);
    let (_, temporary, outcome) = commit(&mut rt, &target, "new", false);
    assert_eq!(kind_of(outcome), "Other");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    assert!(temporary.exists());
    call(&mut rt, "std.files.system_remove", &mut [p(&temporary)]).unwrap();
    assert!(!temporary.exists());
}

#[test]
fn reading_in_chunks_and_whole_files() {
    let scratch = Scratch::new("read");
    let file = scratch.path("text.txt");
    std::fs::write(&file, "Привет").unwrap();
    let mut rt = Runtime::new(vec![]);
    assert_eq!(
        call(&mut rt, "std.files.read_text", &mut [p(&file)]).unwrap(),
        Value::text("Привет")
    );
    let descriptor = call(&mut rt, "std.files.system_open_read", &mut [p(&file)]).unwrap();
    let mut collected = Vec::new();
    loop {
        let step = call(
            &mut rt,
            "std.files.system_read_chunk",
            &mut [descriptor.clone(), p(&file), Value::Int(5)],
        )
        .unwrap();
        if let Some(Value::Bytes(b)) = step.field("data") {
            assert!(b.len() <= 5);
            collected.extend_from_slice(b);
        }
        if step.field("end") == Some(&Value::Bool(true)) {
            break;
        }
    }
    assert_eq!(collected, "Привет".as_bytes());
    call(&mut rt, "std.files.system_close", &mut [descriptor]).unwrap();
    std::fs::write(&file, [0xff, 0xfe]).unwrap();
    assert_eq!(
        kind_of(call(&mut rt, "std.files.read_text", &mut [p(&file)])),
        "InvalidData"
    );
    assert_eq!(
        call(&mut rt, "std.files.read_bytes", &mut [p(&file)]).unwrap(),
        Value::bytes(vec![0xff, 0xfe])
    );
    assert_eq!(
        kind_of(call(
            &mut rt,
            "std.files.read_text",
            &mut [p(&scratch.path("missing"))]
        )),
        "NotFound"
    );
}

#[test]
fn directories_listing_and_renaming_without_replacing() {
    let scratch = Scratch::new("directories");
    let mut rt = Runtime::new(vec![]);
    let sub = scratch.path("sub");
    call(&mut rt, "std.files.create_directory", &mut [p(&sub)]).unwrap();
    assert_eq!(
        kind_of(call(&mut rt, "std.files.create_directory", &mut [p(&sub)])),
        "AlreadyExists"
    );
    assert_eq!(
        kind_of(call(
            &mut rt,
            "std.files.create_directory",
            &mut [p(&scratch.path("no/such"))]
        )),
        "NotFound"
    );
    for name in ["b.txt", "a.txt"] {
        std::fs::write(scratch.path(name), name).unwrap();
    }
    let listed = call(&mut rt, "std.files.list_directory", &mut [p(&scratch.0)]).unwrap();
    assert_eq!(
        listed,
        Value::list(vec![
            p(&scratch.path("a.txt")),
            p(&scratch.path("b.txt")),
            p(&sub)
        ])
    );
    assert_eq!(
        call(&mut rt, "std.files.is_directory", &mut [p(&sub)]).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        call(
            &mut rt,
            "std.files.exists",
            &mut [p(&scratch.path("c.txt"))]
        )
        .unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        kind_of(call(
            &mut rt,
            "std.files.rename",
            &mut [p(&scratch.path("a.txt")), p(&scratch.path("b.txt"))]
        )),
        "AlreadyExists"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path("b.txt")).unwrap(),
        "b.txt"
    );
    call(
        &mut rt,
        "std.files.rename",
        &mut [p(&scratch.path("a.txt")), p(&scratch.path("c.txt"))],
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.path("c.txt")).unwrap(),
        "a.txt"
    );
    assert!(!scratch.path("a.txt").exists());
    call(
        &mut rt,
        "std.files.rename",
        &mut [p(&sub), p(&scratch.path("moved"))],
    )
    .unwrap();
    assert!(scratch.path("moved").is_dir());
    let resolved = call(
        &mut rt,
        "std.files.resolve",
        &mut [p(&scratch.path("moved"))],
    )
    .unwrap();
    assert!(matches!(resolved, Value::Path(_)));
    let durable = call(
        &mut rt,
        "std.files.system_supports_durability",
        &mut [p(&scratch.path("x"))],
    )
    .unwrap();
    assert!(matches!(durable, Value::Bool(_)));
}

#[test]
fn renaming_never_replaces_a_file_or_a_directory() {
    let scratch = Scratch::new("rename-no-replace");
    let mut rt = Runtime::new(vec![]);
    for name in ["dir", "empty", "full"] {
        std::fs::create_dir(scratch.path(name)).unwrap();
    }
    std::fs::write(scratch.path("dir/inside"), "dir").unwrap();
    std::fs::write(scratch.path("full/inside"), "full").unwrap();
    std::fs::write(scratch.path("file"), "file").unwrap();
    // rename(2) alone would replace an empty directory with a directory.
    for (source, target) in [
        ("dir", "empty"),
        ("dir", "full"),
        ("dir", "file"),
        ("file", "empty"),
        ("file", "full/inside"),
    ] {
        assert_eq!(
            kind_of(call(
                &mut rt,
                "std.files.rename",
                &mut [p(&scratch.path(source)), p(&scratch.path(target))]
            )),
            "AlreadyExists",
            "{source} -> {target}"
        );
    }
    assert!(scratch.path("empty").is_dir() && scratch.path("dir/inside").is_file());
    assert_eq!(
        std::fs::read_to_string(scratch.path("full/inside")).unwrap(),
        "full"
    );
    call(
        &mut rt,
        "std.files.rename",
        &mut [p(&scratch.path("dir")), p(&scratch.path("new"))],
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.path("new/inside")).unwrap(),
        "dir"
    );
    assert!(!scratch.path("dir").exists());
}

fn exhausted(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Err(Unwind::Exhausted(unio_runtime::Exhaustion::Heap))
    )
}

#[test]
fn reading_more_than_the_heap_limit_is_exhaustion_before_allocation() {
    let scratch = Scratch::new("read-budget");
    let file = scratch.path("big.bin");
    std::fs::write(&file, vec![b'x'; 300_000]).unwrap();
    let mut rt = Runtime::new(vec![]);
    // The reserve for `release` counts within the limit (C-17): 100,000 bytes for the program.
    rt.heap_limit = Some(unio_runtime::reserve::RESERVE_BYTES + 100_000);
    let descriptor = call(&mut rt, "std.files.system_open_read", &mut [p(&file)]).unwrap();
    // A buffer of 2^50 bytes cannot be allocated at all: it must be refused, not attempted.
    let outcome = call(
        &mut rt,
        "std.files.system_read_chunk",
        &mut [descriptor.clone(), p(&file), Value::Int(1 << 50)],
    );
    assert!(exhausted(outcome));
    assert!(exhausted(call(
        &mut rt,
        "std.files.read_bytes",
        &mut [p(&file)]
    )));
    assert!(exhausted(call(
        &mut rt,
        "std.files.read_text",
        &mut [p(&file)]
    )));
    // Within the limit, a large max reads what there is.
    rt.heap_limit = Some(1 << 30);
    let step = call(
        &mut rt,
        "std.files.system_read_chunk",
        &mut [descriptor.clone(), p(&file), Value::Int(1 << 50)],
    );
    assert!(exhausted(step), "a buffer over the limit is refused");
    let step = call(
        &mut rt,
        "std.files.system_read_chunk",
        &mut [descriptor.clone(), p(&file), Value::Int(1 << 20)],
    )
    .unwrap();
    assert!(matches!(step.field("data"), Some(Value::Bytes(b)) if b.len() == 300_000));
    call(&mut rt, "std.files.system_close", &mut [descriptor]).unwrap();
    // Without a heap limit, an allocation the system cannot make is exhaustion, not an abort.
    rt.heap_limit = None;
    let descriptor = call(&mut rt, "std.files.system_open_read", &mut [p(&file)]).unwrap();
    assert!(exhausted(call(
        &mut rt,
        "std.files.system_read_chunk",
        &mut [descriptor.clone(), p(&file), Value::Int(i64::MAX)],
    )));
    call(&mut rt, "std.files.system_close", &mut [descriptor]).unwrap();
}

#[test]
fn repeating_a_text_over_the_heap_limit_is_exhaustion() {
    let mut rt = Runtime::new(vec![]);
    rt.heap_limit = Some(unio_runtime::reserve::RESERVE_BYTES + 100_000);
    assert!(exhausted(call(
        &mut rt,
        "Text.repeat",
        &mut [Value::text("ab"), Value::Int(1 << 40)]
    )));
}

/// Runs `f` on a thread with only the runtime checks of a sandbox whose read and write scope is
/// `scope/**`: the weaker sandbox of SB-5, where nothing but the runtime stands between the
/// program and the file system, and writes run in the runtime as on Windows.
#[cfg(any(unix, windows))]
fn in_checks_only_sandbox(scope: PathBuf, f: impl FnOnce(&mut Runtime) + Send + 'static) {
    use unio_runtime::sandbox::{self, Limits, Permissions, Sandbox, Scope};
    std::thread::spawn(move || {
        let scope = Scope {
            root: scope,
            subtree: true,
        };
        let permissions = Permissions {
            read: vec![scope.clone()],
            write: vec![scope],
            ..Permissions::default()
        };
        let _guard = sandbox::enter_checks_only(Sandbox::new(permissions, Limits::default()));
        let mut rt = Runtime::new(vec![]);
        f(&mut rt);
    })
    .join()
    .unwrap();
}

#[cfg(unix)]
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Audit finding: `open/link/../secret.txt` is `open/secret.txt` when `..` is taken lexically,
/// but the system follows the link first and then goes up from its target, outside the scope.
/// The check must authorize what the system opens, for every operation, and the operation must
/// open what was authorized. The converse, a path that only looks outside, is permitted.
#[cfg(unix)]
#[test]
fn a_link_followed_by_dot_dot_does_not_lead_a_file_operation_out_of_its_scope() {
    let scratch = Scratch::new("link-dot-dot");
    for dir in ["open/sub", "elsewhere/deep", "outside"] {
        std::fs::create_dir_all(scratch.path(dir)).unwrap();
    }
    std::fs::write(scratch.path("elsewhere/secret.txt"), "SECRET").unwrap();
    std::fs::write(scratch.path("open/secret.txt"), "inside").unwrap();
    std::fs::write(scratch.path("open/a.txt"), "alpha").unwrap();
    std::os::unix::fs::symlink(scratch.path("elsewhere/deep"), scratch.path("open/link")).unwrap();
    std::os::unix::fs::symlink(scratch.path("open/sub"), scratch.path("outside/back")).unwrap();
    let root = std::fs::canonicalize(&scratch.0).unwrap();
    let check_root = root.clone();
    in_checks_only_sandbox(root.join("open"), move |rt| {
        let root = check_root;
        let escape = root.join("open/link/../secret.txt");
        let up = root.join("open/link/..");
        for name in [
            "std.files.read_text",
            "std.files.read_bytes",
            "std.files.system_open_read",
        ] {
            assert_eq!(
                kind_of(call(rt, name, &mut [p(&escape)])),
                "PermissionDenied",
                "{name}"
            );
        }
        for name in ["std.files.list_directory", "std.files.resolve"] {
            assert_eq!(
                kind_of(call(rt, name, &mut [p(&up)])),
                "PermissionDenied",
                "{name}"
            );
        }
        for name in ["std.files.exists", "std.files.is_directory"] {
            let target = if name.ends_with("exists") {
                &escape
            } else {
                &up
            };
            assert!(
                matches!(call(rt, name, &mut [p(target)]), Ok(Value::Bool(false))),
                "{name}"
            );
        }
        // Writes: a new file, a directory, and a rename into the place outside.
        let outside = root.join("open/link/../new.txt");
        assert_eq!(
            kind_of(call(
                rt,
                "std.files.system_open_temporary",
                &mut [p(&outside)]
            )),
            "PermissionDenied"
        );
        assert_eq!(
            kind_of(call(
                rt,
                "std.files.create_directory",
                &mut [p(&root.join("open/link/../made"))]
            )),
            "PermissionDenied"
        );
        assert_eq!(
            kind_of(call(
                rt,
                "std.files.rename",
                &mut [p(&root.join("open/a.txt")), p(&outside)]
            )),
            "PermissionDenied"
        );
        // The converse: outside/back is open/sub, so outside/back/.. is open itself.
        let inside = root.join("outside/back/../a.txt");
        assert!(matches!(
            call(rt, "std.files.read_text", &mut [p(&inside)]),
            Ok(Value::Text(t)) if t.as_str() == "alpha"
        ));
        assert!(matches!(
            call(rt, "std.files.exists", &mut [p(&inside)]),
            Ok(Value::Bool(true))
        ));
        let written = root.join("outside/back/../w.txt");
        let (_, _, outcome) = commit(rt, &written, "written", false);
        outcome.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("open/w.txt")).unwrap(),
            "written"
        );
    });
    assert_eq!(names_in(&root.join("elsewhere")), ["deep", "secret.txt"]);
    assert_eq!(names_in(&root.join("outside")), ["back"]);
}

/// The bytes the heap counts while `f` holds its result.
fn counted_by(f: impl FnOnce() -> Value) -> (Value, usize) {
    let before = unio_runtime::heap::live_bytes();
    let value = f();
    let after = unio_runtime::heap::live_bytes();
    (value, after.saturating_sub(before))
}

/// Audit finding: the temporary memory of file operations must be checked against the heap
/// limit before it is allocated. Each operation here would succeed if only its result were
/// counted, but holds more while it runs; with `free` bytes left it must end with exhaustion
/// (code 102) instead of allocating past the limit.
#[test]
fn the_temporary_memory_of_file_operations_counts_against_the_heap_limit() {
    let scratch = Scratch::new("temporary-budget");
    let mut rt = Runtime::new(vec![]);
    let with_free = |rt: &mut Runtime, free: usize| {
        rt.heap_limit = Some(unio_runtime::heap::live_bytes() + free);
    };

    // A directory listing holds every name before it makes the list.
    let many = scratch.path("many");
    std::fs::create_dir(&many).unwrap();
    for i in 0..2000 {
        std::fs::write(
            many.join(format!("an-entry-with-a-longish-name-{i:05}")),
            "",
        )
        .unwrap();
    }
    let (list, result) =
        counted_by(|| call(&mut rt, "std.files.list_directory", &mut [p(&many)]).unwrap());
    drop(list);
    with_free(&mut rt, result + 8 * 1024);
    assert!(exhausted(call(
        &mut rt,
        "std.files.list_directory",
        &mut [p(&many)]
    )));
    rt.heap_limit = None;

    // A path's copies: the native path, its resolution, the text for the system call.
    let long = Value::path(format!(
        "{}/{}",
        scratch.0.display(),
        "a/".repeat(32 * 1024)
    ));
    with_free(&mut rt, 32 * 1024);
    for name in [
        "std.files.exists",
        "std.files.read_bytes",
        "std.files.create_directory",
    ] {
        assert!(
            exhausted(call(&mut rt, name, &mut [long.clone()])),
            "{name} with a long path"
        );
    }
    rt.heap_limit = None;
}

/// Writing bytes the program holds makes no copy of them: measured in the reserve for
/// `release`, which records the most it served at once (C-17).
#[test]
fn writing_bytes_makes_no_copy_of_them() {
    use unio_runtime::reserve;
    let scratch = Scratch::new("write-no-copy");
    let mut rt = Runtime::new(vec![]);
    let data = Value::bytes(vec![b'x'; 256 * 1024]);
    let temporary = call(
        &mut rt,
        "std.files.system_open_temporary",
        &mut [p(&scratch.path("w.bin"))],
    )
    .unwrap();
    let descriptor = Value::Int(int_field(&temporary, "descriptor"));
    reserve::open();
    reserve::reset_peak();
    let written = call(
        &mut rt,
        "std.files.system_write_bytes",
        &mut [descriptor.clone(), data.clone()],
    );
    let peak = reserve::peak();
    reserve::close();
    written.unwrap();
    assert!(peak < 4096, "writing 256 KiB allocated {peak} bytes");
    call(&mut rt, "std.files.system_close", &mut [descriptor]).unwrap();
}

/// A file whose size the system does not report (a pipe) grows the buffer step by step; each
/// step is checked with the buffer the runtime already holds and the copy that growing makes.
#[cfg(unix)]
#[test]
fn reading_a_file_of_unknown_size_counts_the_buffer_it_grows() {
    extern "C" {
        fn mkfifo(path: *const std::os::raw::c_char, mode: u32) -> i32;
    }
    let scratch = Scratch::new("read-pipe");
    let pipe = scratch.path("pipe");
    let name = std::ffi::CString::new(pipe.to_string_lossy().into_owned()).unwrap();
    // SAFETY: the path ends with 0.
    assert_eq!(unsafe { mkfifo(name.as_ptr(), 0o600) }, 0);
    let feed = |pipe: PathBuf| {
        std::thread::spawn(move || {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(pipe) {
                let _ = f.write_all(&vec![b'y'; 100 * 1024]);
            }
        })
    };
    let mut rt = Runtime::new(vec![]);
    // The content is 100 KiB, and the buffer that holds it 128 KiB: with 150 KiB free it fits,
    // but growing from 64 KiB to 128 KiB holds both blocks for a moment.
    let writer = feed(pipe.clone());
    rt.heap_limit = Some(unio_runtime::heap::live_bytes() + 150 * 1024);
    let outcome = call(&mut rt, "std.files.read_bytes", &mut [p(&pipe)]);
    rt.heap_limit = None;
    writer.join().unwrap();
    assert!(exhausted(outcome));
    // With room for both, the whole content is read.
    let writer = feed(pipe.clone());
    rt.heap_limit = Some(unio_runtime::heap::live_bytes() + 256 * 1024);
    let read = call(&mut rt, "std.files.read_bytes", &mut [p(&pipe)]);
    rt.heap_limit = None;
    writer.join().unwrap();
    assert!(matches!(read, Ok(Value::Bytes(b)) if b.len() == 100 * 1024));
}

/// Release condition 5 (docs/plan.md section 12, runtime.md 9.2): another process swaps a
/// directory and a file of the scope for links that lead outside it, in a tight loop, while the
/// program reads, lists, asks about, and resolves paths through them. The check sees the link
/// or the real entry, but whatever it saw, the operation must not reach what lies outside:
/// nothing outside is read, listed, reported as existing, or named. Only the runtime checks
/// stand between the program and the file system here (the weaker sandbox), so that the rules
/// of the operating system do not hide the race. On Windows the links are symbolic links where
/// the account may make them, else the directory is a junction (and the file is not swapped),
/// and the writes race too: the runtime writes there itself, relative to the directory of the
/// scope, so a write through the swapped directory must not land outside.
#[cfg(any(unix, windows))]
#[test]
fn a_link_swapped_in_after_the_check_reveals_nothing_outside_the_scope() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let scratch = Scratch::new("swap-race");
    let root = std::fs::canonicalize(&scratch.0).unwrap();
    for dir in ["open/d/sub", "outside/only-outside-dir"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("open/d/f.txt"), "inside").unwrap();
    std::fs::write(root.join("open/f.txt"), "inside").unwrap();
    std::fs::write(root.join("outside/f.txt"), "SECRET").unwrap();
    std::fs::write(root.join("outside/only-outside.txt"), "SECRET").unwrap();
    link_directory(&root.join("outside"), &root.join("open/d-link"));
    link_file(&root.join("outside/f.txt"), &root.join("open/f-link"));
    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let (stop, open) = (Arc::clone(&stop), root.join("open"));
        std::thread::spawn(move || {
            let at = |name: &str| open.join(name);
            while !stop.load(Ordering::Relaxed) {
                // open/d and open/f.txt are the real entries, then the links, then real again.
                let _ = std::fs::rename(at("d"), at("d-real"));
                let _ = std::fs::rename(at("d-link"), at("d"));
                let _ = std::fs::rename(at("f.txt"), at("f-real"));
                let _ = std::fs::rename(at("f-link"), at("f.txt"));
                std::thread::yield_now();
                let _ = std::fs::rename(at("d"), at("d-link"));
                let _ = std::fs::rename(at("d-real"), at("d"));
                let _ = std::fs::rename(at("f.txt"), at("f-link"));
                let _ = std::fs::rename(at("f-real"), at("f.txt"));
                std::thread::yield_now();
            }
        })
    };
    let leaks = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (found, check_root) = (Arc::clone(&leaks), root.clone());
    in_checks_only_sandbox(root.join("open"), move |rt| {
        let root = check_root;
        let outside = root.join("outside");
        let leak = |what: String| found.lock().unwrap().push(what);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut rounds = 0u64;
        while std::time::Instant::now() < deadline || rounds < 2000 {
            rounds += 1;
            for file in ["open/d/f.txt", "open/f.txt"] {
                let file = root.join(file);
                for name in ["std.files.read_text", "std.files.read_bytes"] {
                    match call(rt, name, &mut [p(&file)]) {
                        Ok(Value::Text(t)) if t.as_str() == "SECRET" => leak(name.to_string()),
                        Ok(Value::Bytes(b)) if b.as_slice() == b"SECRET" => leak(name.to_string()),
                        _ => {}
                    }
                }
                if let Ok(Value::Int(d)) = call(rt, "std.files.system_open_read", &mut [p(&file)]) {
                    let read = call(
                        rt,
                        "std.files.system_read_chunk",
                        &mut [Value::Int(d), p(&file), Value::Int(64)],
                    );
                    if let Ok(step) = read {
                        if matches!(step.field("data"), Some(Value::Bytes(b)) if b.as_slice() == b"SECRET")
                        {
                            leak("system_open_read".into());
                        }
                    }
                    let _ = call(rt, "std.files.system_close", &mut [Value::Int(d)]);
                }
                if let Ok(Value::Path(r)) = call(rt, "std.files.resolve", &mut [p(&file)]) {
                    if Path::new(r.as_str()).starts_with(&outside) {
                        leak(format!("resolve {r:?}"));
                    }
                }
            }
            let d = root.join("open/d");
            if let Ok(entries) = call(rt, "std.files.list_directory", &mut [p(&d)]) {
                let names = format!("{entries:?}");
                if names.contains("only-outside") {
                    leak(format!("list_directory {names}"));
                }
            }
            if let Ok(Value::Path(r)) = call(rt, "std.files.resolve", &mut [p(&d)]) {
                if Path::new(r.as_str()).starts_with(&outside) {
                    leak(format!("resolve {r:?}"));
                }
            }
            if cfg!(windows) {
                let target = root.join("open/d/written.txt");
                let _ = call(
                    rt,
                    "std.files.write_text",
                    &mut [p(&target), Value::text("x")],
                );
                let made = root.join("open/d/made");
                let _ = call(rt, "std.files.create_directory", &mut [p(&made)]);
            }
            for (name, path) in [
                ("std.files.exists", "open/d/only-outside.txt"),
                ("std.files.is_directory", "open/d/only-outside-dir"),
            ] {
                if matches!(
                    call(rt, name, &mut [p(&root.join(path))]),
                    Ok(Value::Bool(true))
                ) {
                    leak(name.into());
                }
            }
        }
    });
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();
    let mut leaks = leaks.lock().unwrap().clone();
    for written in ["written.txt", "made"] {
        if root.join("outside").join(written).exists() {
            leaks.push(format!("a write made outside/{written}"));
        }
    }
    leaks.sort();
    leaks.dedup();
    assert!(leaks.is_empty(), "outside the scope: {leaks:?}");
}

/// A link to a directory: a symbolic link, or on Windows, where making one needs a privilege
/// the account may lack, a junction.
#[cfg(any(unix, windows))]
fn link_directory(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(target, link).is_err() {
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()
            .unwrap();
        assert!(status.success(), "a junction cannot be made");
    }
}

/// A link to a file; on Windows without the privilege of symbolic links a copy stands in, so
/// that the swap still moves an entry but reaches nothing outside.
#[cfg(any(unix, windows))]
fn link_file(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_file(target, link).is_err() {
        eprintln!("no symbolic link for a file here: the file is not swapped for a link");
        std::fs::write(link, "inside").unwrap();
    }
}
