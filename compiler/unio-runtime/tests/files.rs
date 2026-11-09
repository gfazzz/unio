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
