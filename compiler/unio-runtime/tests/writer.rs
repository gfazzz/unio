//! The writer process of the sandbox through its protocol (docs/runtime.md 9.2), served in a
//! thread: it checks the write scopes itself, whatever the program sends, works relative to
//! the directory it opened, and commits or removes a temporary file only for the operation that
//! created it. Its own restrictions of the operating system are tested in `sandbox/linux.rs`
//! and, as a process, in the sandbox suite of the `unio` crate.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use unio_runtime::sandbox::writer::{self, op, status, Failure, Handle, Request, VERSION};
use unio_runtime::sandbox::Scope;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("unio-writer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("open")).unwrap();
        std::fs::create_dir_all(path.join("closed")).unwrap();
        Scratch(std::fs::canonicalize(path).unwrap())
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

fn file(root: PathBuf) -> Scope {
    Scope {
        root,
        subtree: false,
    }
}

fn tree(root: PathBuf) -> Scope {
    Scope {
        root,
        subtree: true,
    }
}

fn start(scopes: &[Scope]) -> Handle {
    let (ours, theirs) = UnixStream::pair().unwrap();
    writer::serve_in_thread(theirs);
    Handle::connect(ours, scopes).unwrap()
}

#[track_caller]
fn refused<T: std::fmt::Debug>(result: Result<T, Failure>) {
    match result {
        Err(Failure::Refused(reason)) => {
            assert!(reason.contains("denied by the sandbox"), "{reason}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[track_caller]
fn malformed<T: std::fmt::Debug>(result: Result<T, Failure>) {
    assert!(
        matches!(result, Err(Failure::Malformed(_))),
        "expected a malformed request, got {result:?}"
    );
}

/// Writes the content through a fresh operation and commits it.
fn write(w: &Handle, target: &Path, content: &str) -> Result<(), Failure> {
    let (id, _, mut f) = w.open(target)?;
    f.write_all(content.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);
    w.commit(id, target)
}

#[test]
fn the_writer_refuses_every_path_outside_its_scopes() {
    let s = Scratch::new("scopes");
    std::fs::write(s.path("open/neighbour.txt"), "neighbour").unwrap();
    std::fs::write(s.path("open/out.txt"), "old").unwrap();
    let w = start(&[file(s.path("open/out.txt"))]);
    refused(w.open(&s.path("open/neighbour.txt")));
    refused(w.open(&s.path("open/new.txt")));
    refused(w.open(&s.path("closed/x.txt")));
    refused(w.make_directory(&s.path("open/dir")));
    refused(w.rename(&s.path("open/out.txt"), &s.path("open/moved.txt")));
    refused(w.rename(&s.path("open/neighbour.txt"), &s.path("open/out.txt")));
    refused(w.durable(&s.path("closed/x.txt")));
    refused(w.sync_directory(&s.path("closed")));
    // Paths that are not absolute and normal are not paths of the protocol.
    malformed(w.open(Path::new("open/out.txt")));
    malformed(w.open(&s.path("open/../open/out.txt")));
    malformed(w.open(Path::new("/")));
    // The one permitted file is written and its directory synced.
    write(&w, &s.path("open/out.txt"), "new").unwrap();
    w.sync_directory(&s.path("open")).unwrap();
    w.durable(&s.path("open/out.txt")).unwrap();
    assert_eq!(
        std::fs::read_to_string(s.path("open/out.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(s.path("open/neighbour.txt")).unwrap(),
        "neighbour"
    );
    let names: Vec<String> = std::fs::read_dir(s.path("open"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(std::fs::read_dir(s.path("closed"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn only_the_operation_that_created_a_temporary_file_commits_or_removes_it() {
    let s = Scratch::new("operations");
    let target = s.path("open/out.txt");
    std::fs::write(&target, "old").unwrap();
    let scopes = [file(target.clone()), file(s.path("open/other.txt"))];
    let w = start(&scopes);
    let other = start(&scopes);
    let (id, temporary, mut f) = w.open(&target).unwrap();
    let (second, second_temporary, _) = w.open(&target).unwrap();
    assert_ne!(id, second);
    assert_ne!(
        temporary, second_temporary,
        "temporary names are unique per operation"
    );
    let name = temporary
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with(".out.txt.unio-tmp-"), "{name}");
    f.write_all(b"new").unwrap();
    drop(f);
    // Forged commits: an id this writer never issued, an id of another writer, and a target
    // that is not the operation's, even one the scopes permit.
    refused(w.commit(id ^ 1, &target));
    refused(w.commit(0, &target));
    refused(other.commit(id, &target));
    refused(other.abort(id));
    refused(w.commit(id, &s.path("open/other.txt")));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    w.commit(id, &target).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert!(!temporary.exists());
    // A finished operation is no longer open.
    refused(w.commit(id, &target));
    refused(w.abort(id));
    w.sync(id).unwrap();
    refused(w.sync(second ^ 1));
    // The second operation is removed by its own id only.
    assert!(second_temporary.exists());
    w.abort(second).unwrap();
    assert!(!second_temporary.exists());
    refused(w.abort(second));
}

#[test]
fn a_directory_replaced_by_a_link_between_requests_does_not_lead_the_write_outside() {
    let s = Scratch::new("substitution");
    std::fs::create_dir(s.path("open/sub")).unwrap();
    let w = start(&[tree(s.path("open"))]);
    let target = s.path("open/sub/out.txt");
    let (id, _, mut f) = w.open(&target).unwrap();
    f.write_all(b"inside").unwrap();
    drop(f);
    // Between the requests the directory becomes a link to a directory outside the scopes.
    std::fs::rename(s.path("open/sub"), s.path("open/sub-old")).unwrap();
    std::os::unix::fs::symlink(s.path("closed"), s.path("open/sub")).unwrap();
    w.commit(id, &target).unwrap();
    assert_eq!(
        std::fs::read_to_string(s.path("open/sub-old/out.txt")).unwrap(),
        "inside",
        "the commit happens in the directory that was checked"
    );
    // A new request does not follow the link.
    refused(w.open(&target));
    refused(w.make_directory(&s.path("open/sub/dir")));
    refused(w.rename(&s.path("open/sub-old/out.txt"), &s.path("open/sub/out.txt")));
    refused(w.sync_directory(&s.path("open/sub")));
    // A link as the last name is replaced as an entry, never followed.
    std::os::unix::fs::symlink(s.path("closed/secret.txt"), s.path("open/link")).unwrap();
    std::fs::write(s.path("closed/secret.txt"), "secret").unwrap();
    write(&w, &s.path("open/link"), "replaced").unwrap();
    assert!(!s.path("open/link").is_symlink());
    assert_eq!(
        std::fs::read_to_string(s.path("closed/secret.txt")).unwrap(),
        "secret"
    );
    let closed: Vec<_> = std::fs::read_dir(s.path("closed"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(closed.len(), 1, "{closed:?}");
}

#[test]
fn directories_and_renames_go_through_the_same_checks() {
    let s = Scratch::new("directories");
    std::fs::write(s.path("open/a.txt"), "a").unwrap();
    std::fs::write(s.path("open/b.txt"), "b").unwrap();
    let w = start(&[tree(s.path("open"))]);
    w.make_directory(&s.path("open/dir")).unwrap();
    assert!(matches!(
        w.make_directory(&s.path("open/dir")),
        Err(Failure::Os(17, _))
    ));
    assert!(matches!(
        w.rename(&s.path("open/a.txt"), &s.path("open/b.txt")),
        Err(Failure::Os(17, _))
    ));
    assert_eq!(std::fs::read_to_string(s.path("open/b.txt")).unwrap(), "b");
    w.rename(&s.path("open/a.txt"), &s.path("open/dir/a.txt"))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(s.path("open/dir/a.txt")).unwrap(),
        "a"
    );
    refused(w.rename(&s.path("open/b.txt"), &s.path("closed/b.txt")));
    assert!(s.path("open/b.txt").exists());
}

/// Sends a frame as it is and returns the status of the reply.
fn status_of(w: &Handle, frame: &[u8]) -> u8 {
    let (body, _) = w.exchange(frame).unwrap();
    assert_eq!(body[0], VERSION);
    body[1]
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

#[test]
fn malformed_requests_are_refused() {
    let s = Scratch::new("malformed");
    let target = s.path("open/out.txt");
    let w = start(&[file(target.clone())]);
    let path = target.to_string_lossy().into_owned();
    let mut open = vec![VERSION, op::OPEN];
    open.extend_from_slice(&(path.len() as u32).to_le_bytes());
    open.extend_from_slice(path.as_bytes());
    for (what, body) in [
        ("an empty body", vec![]),
        ("another version", [&[VERSION + 1], &open[1..]].concat()),
        ("an unknown operation", vec![VERSION, 99]),
        ("a cut path", open[..open.len() - 1].to_vec()),
        ("bytes after the request", [&open[..], &[0]].concat()),
        (
            "a path longer than the frame",
            vec![VERSION, op::OPEN, 255, 255, 255, 255],
        ),
        (
            "a second configuration",
            Request::Config(vec![tree(PathBuf::from("/"))]).encode()[4..].to_vec(),
        ),
    ] {
        assert_eq!(status_of(&w, &frame(&body)), status::MALFORMED, "{what}");
    }
    // The connection still serves well-formed requests.
    assert_eq!(status_of(&w, &frame(&open)), status::OK);
    // A frame over the limit cannot be skipped: the writer refuses it and ends the connection.
    let huge = ((writer::MAX_FRAME + 1) as u32).to_le_bytes();
    assert_eq!(status_of(&w, &huge), status::MALFORMED);
    assert!(w.exchange(&frame(&open)).is_err());
}

#[test]
fn the_first_message_must_be_the_configuration() {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let served = writer::serve_in_thread(theirs);
    let w = Handle::connect(ours, &[]);
    assert!(w.is_ok(), "an empty configuration is a configuration");
    drop(w);
    served.join().unwrap().unwrap();
    let (mut ours, theirs) = UnixStream::pair().unwrap();
    let served = writer::serve_in_thread(theirs);
    ours.write_all(&Request::Open(PathBuf::from("/x")).encode())
        .unwrap();
    let mut reply = Vec::new();
    std::io::Read::read_to_end(&mut ours, &mut reply).unwrap();
    assert_eq!(reply[5], status::MALFORMED);
    served.join().unwrap().unwrap();
}
