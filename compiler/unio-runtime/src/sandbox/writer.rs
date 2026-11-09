//! The writer process of the sandbox (docs/runtime.md 9.2): the one process that writes files
//! for a sandboxed program on Linux and macOS.
//!
//! The program process gets no right to write files from the operating system, so neither do
//! the children it starts. Every write-class operation of `std.files` (the atomic commit of
//! `AtomicFile`, `create_directory`, `rename`) is a request to the writer, which:
//!
//! - is started before the sandbox of the program applies, from the same executable, with an
//!   empty environment, and receives the effective write scopes (the manifest narrowed by the
//!   policy, SB-3a) once, before any code of the program runs; it never takes them from the
//!   program again;
//! - applies its own, narrower sandbox: write access to the write scopes and their directories
//!   only, no reading of other files, no network, no exec, no new processes;
//! - checks every path against those scopes itself, opens its directory one component at a
//!   time without following links, and works relative to that directory descriptor from the
//!   check to the last step, so that replacing a directory by a link in between cannot lead a
//!   write elsewhere;
//! - creates each temporary file with O_CREAT | O_EXCL | O_NOFOLLOW under a name that is unique
//!   to the operation, passes the program only a descriptor to write into (SCM_RIGHTS), and
//!   commits or removes a temporary file only for the operation id it issued for it;
//! - ends when the program ends: the socket closes, and on Linux a parent-death signal.
//!
//! The protocol is versioned and length-prefixed: a frame is a 32-bit little-endian length and
//! then a body of a version byte, an operation or status byte, and fields (u64 little-endian,
//! byte strings with a u32 length). A request the writer cannot decode is refused; a frame
//! over the size limit ends the connection.

use super::fdio;
use super::Scope;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The version of the protocol; a request of another version is refused.
pub const VERSION: u8 = 1;
/// The variable that starts an executable as a writer: `<version>:<parent pid>`.
pub const ENV: &str = "UNIO_SANDBOX_WRITER";
/// The descriptor of the socket in the writer process.
const SOCKET_FD: i32 = 3;
/// The largest frame; a longer one ends the connection.
pub const MAX_FRAME: usize = 1 << 20;
/// Atomic files that may be open at once in one program.
const MAX_OPEN: usize = 256;
/// Committed operations whose directory can still be synced by id.
const KEEP_COMMITTED: usize = 16;

/// Operations.
pub mod op {
    pub const CONFIG: u8 = 1;
    pub const DURABLE: u8 = 2;
    pub const OPEN: u8 = 3;
    pub const COMMIT: u8 = 4;
    pub const ABORT: u8 = 5;
    pub const SYNC: u8 = 6;
    pub const SYNC_DIRECTORY: u8 = 7;
    pub const MAKE_DIRECTORY: u8 = 8;
    pub const RENAME: u8 = 9;
}

/// Statuses of a reply.
pub mod status {
    pub const OK: u8 = 0;
    pub const REFUSED: u8 = 1;
    pub const OS_ERROR: u8 = 2;
    pub const MALFORMED: u8 = 3;
}

// ---------------------------------------------------------------- encoding

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn path(&mut self, p: &Path) -> &mut Self {
        self.bytes(p.as_os_str().as_bytes())
    }
    /// The body as a frame: its length first.
    fn frame(self) -> Vec<u8> {
        let mut frame = (self.0.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&self.0);
        frame
    }
}

struct In<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let part = self.data.get(self.at..end)?;
        self.at = end;
        Some(part)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn i32(&mut self) -> Option<i32> {
        self.take(4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn path(&mut self) -> Option<PathBuf> {
        self.bytes()
            .map(|b| PathBuf::from(OsStr::from_bytes(b).to_os_string()))
    }
    fn end(&self) -> bool {
        self.at == self.data.len()
    }
}

/// A request of the protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The write scopes; only the first message of a connection, sent before the program runs.
    Config(Vec<Scope>),
    /// Whether the file system of the target keeps the guarantees of S-9 (S-9a).
    Durable(PathBuf),
    /// A new temporary file next to the target; the reply carries an operation id, the path of
    /// the temporary file, and a descriptor open for writing it.
    Open(PathBuf),
    /// Replaces the target of the operation with its temporary file.
    Commit(u64, PathBuf),
    /// Removes the temporary file of the operation.
    Abort(u64),
    /// Syncs the directory of a committed operation.
    Sync(u64),
    /// Syncs a directory that holds a write target.
    SyncDirectory(PathBuf),
    /// Creates one directory.
    MakeDirectory(PathBuf),
    /// Renames without replacing an existing target.
    Rename(PathBuf, PathBuf),
}

impl Request {
    /// The request as a frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Out::default();
        out.u8(VERSION);
        match self {
            Request::Config(scopes) => {
                out.u8(op::CONFIG).u32(scopes.len() as u32);
                for scope in scopes {
                    out.u8(u8::from(scope.subtree)).path(&scope.root);
                }
            }
            Request::Durable(p) => {
                out.u8(op::DURABLE).path(p);
            }
            Request::Open(p) => {
                out.u8(op::OPEN).path(p);
            }
            Request::Commit(id, p) => {
                out.u8(op::COMMIT).u64(*id).path(p);
            }
            Request::Abort(id) => {
                out.u8(op::ABORT).u64(*id);
            }
            Request::Sync(id) => {
                out.u8(op::SYNC).u64(*id);
            }
            Request::SyncDirectory(p) => {
                out.u8(op::SYNC_DIRECTORY).path(p);
            }
            Request::MakeDirectory(p) => {
                out.u8(op::MAKE_DIRECTORY).path(p);
            }
            Request::Rename(a, b) => {
                out.u8(op::RENAME).path(a).path(b);
            }
        }
        out.frame()
    }

    /// Decodes the body of a frame; anything else than exactly one known request is an error.
    pub fn decode(body: &[u8]) -> Result<Request, String> {
        let mut r = In { data: body, at: 0 };
        let version = r.u8().ok_or("an empty request")?;
        if version != VERSION {
            return Err(format!(
                "protocol version {version} is not supported; the writer speaks version {VERSION}"
            ));
        }
        let code = r.u8().ok_or("a request without an operation")?;
        let malformed = || format!("a malformed request of operation {code}");
        let request = match code {
            op::CONFIG => {
                let count = r.u32().ok_or_else(malformed)?;
                let mut scopes = Vec::new();
                for _ in 0..count {
                    let subtree = match r.u8().ok_or_else(malformed)? {
                        0 => false,
                        1 => true,
                        _ => return Err(malformed()),
                    };
                    let root = r.path().ok_or_else(malformed)?;
                    scopes.push(Scope { root, subtree });
                }
                Request::Config(scopes)
            }
            op::DURABLE => Request::Durable(r.path().ok_or_else(malformed)?),
            op::OPEN => Request::Open(r.path().ok_or_else(malformed)?),
            op::COMMIT => Request::Commit(
                r.u64().ok_or_else(malformed)?,
                r.path().ok_or_else(malformed)?,
            ),
            op::ABORT => Request::Abort(r.u64().ok_or_else(malformed)?),
            op::SYNC => Request::Sync(r.u64().ok_or_else(malformed)?),
            op::SYNC_DIRECTORY => Request::SyncDirectory(r.path().ok_or_else(malformed)?),
            op::MAKE_DIRECTORY => Request::MakeDirectory(r.path().ok_or_else(malformed)?),
            op::RENAME => Request::Rename(
                r.path().ok_or_else(malformed)?,
                r.path().ok_or_else(malformed)?,
            ),
            _ => return Err(format!("unknown operation {code}")),
        };
        if !r.end() {
            return Err(malformed());
        }
        Ok(request)
    }
}

/// Why the writer did not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The writer refused the operation: the path is outside the write scopes, a directory of
    /// it is a link, or the operation id is not one it issued.
    Refused(String),
    /// The operating system failed the operation with this error number.
    Os(i32, String),
    /// The writer could not decode the request.
    Malformed(String),
    /// The connection to the writer failed.
    Broken(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Refused(m) | Failure::Os(_, m) => write!(f, "{m}"),
            Failure::Malformed(m) => write!(f, "the writer refused a malformed request: {m}"),
            Failure::Broken(m) => write!(f, "the writer process does not answer: {m}"),
        }
    }
}

/// A reply of the writer.
enum Reply {
    Ok(Vec<u8>, Option<OwnedFd>),
    Refused(String),
    Os(io::Error),
    Malformed(String),
}

impl Reply {
    fn ok() -> Reply {
        Reply::Ok(Vec::new(), None)
    }

    fn encode(self) -> (Vec<u8>, Option<OwnedFd>) {
        let mut out = Out::default();
        out.u8(VERSION);
        let mut fd = None;
        match self {
            Reply::Ok(payload, descriptor) => {
                out.u8(status::OK);
                out.0.extend_from_slice(&payload);
                fd = descriptor;
            }
            Reply::Refused(message) => {
                out.u8(status::REFUSED).bytes(message.as_bytes());
            }
            Reply::Os(error) => {
                out.u8(status::OS_ERROR)
                    .i32(error.raw_os_error().unwrap_or(0))
                    .bytes(error.to_string().as_bytes());
            }
            Reply::Malformed(message) => {
                out.u8(status::MALFORMED).bytes(message.as_bytes());
            }
        }
        (out.frame(), fd)
    }
}

/// Decodes the body of a reply into its payload.
fn decode_reply(body: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut r = In { data: body, at: 0 };
    let broken = || Failure::Broken("a malformed reply".into());
    if r.u8().ok_or_else(broken)? != VERSION {
        return Err(Failure::Broken(
            "a reply of another protocol version".into(),
        ));
    }
    let code = r.u8().ok_or_else(broken)?;
    let text = |r: &mut In| {
        r.bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .ok_or_else(broken)
    };
    match code {
        status::OK => Ok(body[2..].to_vec()),
        status::REFUSED => Err(Failure::Refused(text(&mut r)?)),
        status::OS_ERROR => {
            let errno = r.i32().ok_or_else(broken)?;
            Err(Failure::Os(errno, text(&mut r)?))
        }
        status::MALFORMED => Err(Failure::Malformed(text(&mut r)?)),
        _ => Err(broken()),
    }
}

// ---------------------------------------------------------------- the writer

/// Operation ids: unpredictable, so that an id cannot be guessed, and never reused.
struct Ids(u64);

impl Ids {
    fn next(&mut self) -> u64 {
        loop {
            // splitmix64
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            if z != 0 {
                return z;
            }
        }
    }
}

/// A seed from the system's random source, mixed with the time and the process id.
fn seed() -> u64 {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    u64::from_le_bytes(bytes) ^ time ^ (u64::from(std::process::id()) << 32)
}

/// An atomic file between Open and Commit: the directory it was opened in, the name of its
/// temporary file there, and its target.
struct Operation {
    dir: OwnedFd,
    /// The path of `dir`, for the trace of the crash trials.
    parent: PathBuf,
    temporary: OsString,
    name: OsString,
    target: PathBuf,
}

/// The state of one writer: the scopes it was given once, and the operations it issued.
pub struct Server {
    scopes: Vec<Scope>,
    open: HashMap<u64, Operation>,
    /// Committed operations: the id, the directory, and its path.
    committed: VecDeque<(u64, OwnedFd, PathBuf)>,
    ids: Ids,
    /// /proc/self/mountinfo, read before the writer's own sandbox applies (Linux).
    mountinfo: Option<String>,
}

fn refused_scope(path: &Path) -> Reply {
    Reply::Refused(format!(
        "denied by the sandbox: the permissions do not include files.write for {} (checked by the writer)",
        path.display()
    ))
}

/// The parent and the name of an absolute, normal path: no `.`, no `..`, a name at the end.
fn split(path: &Path) -> Result<(PathBuf, OsString), Reply> {
    let malformed =
        || Reply::Malformed(format!("{} is not an absolute normal path", path.display()));
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err(malformed());
    }
    let mut parent = PathBuf::from("/");
    let mut name = None;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => {
                if let Some(previous) = name.replace(part.to_os_string()) {
                    parent.push(previous);
                }
            }
            _ => return Err(malformed()),
        }
    }
    match name {
        Some(name) => Ok((parent, name)),
        None => Err(malformed()),
    }
}

/// The directory opened without following links; a link on the way is a refusal.
fn walk(dir: &Path) -> Result<OwnedFd, Reply> {
    fdio::open_directory(dir).map_err(|e| {
        if fdio::is_link_error(&e) {
            Reply::Refused(format!(
                "denied by the sandbox: a directory of {} is a symbolic link, which the writer does not follow",
                dir.display()
            ))
        } else {
            Reply::Os(e)
        }
    })
}

impl Server {
    pub fn new(scopes: Vec<Scope>) -> Server {
        Server {
            scopes,
            open: HashMap::new(),
            committed: VecDeque::new(),
            ids: Ids(seed()),
            mountinfo: if cfg!(target_os = "linux") {
                std::fs::read_to_string("/proc/self/mountinfo").ok()
            } else {
                None
            },
        }
    }

    fn permits(&self, path: &Path) -> bool {
        self.scopes.iter().any(|s| s.covers(path))
    }

    /// A directory may be synced if it lies in a write scope or holds a file of one.
    fn permits_directory(&self, dir: &Path) -> bool {
        self.scopes
            .iter()
            .any(|s| s.covers(dir) || s.root.parent() == Some(dir))
    }

    /// The parent and name of a path the scopes permit, and the parent opened.
    fn target(&self, path: &Path) -> Result<(PathBuf, OsString, OwnedFd), Reply> {
        let (parent, name) = split(path)?;
        if !self.permits(path) {
            return Err(refused_scope(path));
        }
        let dir = walk(&parent)?;
        Ok((parent, name, dir))
    }

    fn handle(&mut self, request: Request) -> Reply {
        match self.handle_checked(request) {
            Ok(reply) | Err(reply) => reply,
        }
    }

    fn handle_checked(&mut self, request: Request) -> Result<Reply, Reply> {
        match request {
            Request::Config(_) => Err(Reply::Malformed(
                "the scopes are given once, before the program runs".into(),
            )),
            Request::Durable(path) => {
                let (parent, _, _) = self.target(&path)?;
                let name = self.file_system(&parent);
                let supported = name.is_some_and(|n| {
                    super::super::externs::durability::SUPPORTED
                        .contains(&n.to_ascii_lowercase().as_str())
                });
                let mut out = Out::default();
                out.u8(u8::from(supported));
                Ok(Reply::Ok(out.0, None))
            }
            Request::Open(path) => {
                let (parent, name, dir) = self.target(&path)?;
                if self.open.len() >= MAX_OPEN {
                    return Err(Reply::Refused(format!(
                        "the writer keeps at most {MAX_OPEN} atomic files open at once"
                    )));
                }
                for _ in 0..8 {
                    let id = self.ids.next();
                    if self.open.contains_key(&id) {
                        continue;
                    }
                    let mut temporary = b".".to_vec();
                    temporary.extend_from_slice(name.as_bytes());
                    temporary.extend_from_slice(
                        format!(".unio-tmp-{}-{id:016x}", std::process::id()).as_bytes(),
                    );
                    let temporary = OsString::from_vec(temporary);
                    match fdio::create_new(&dir, &temporary) {
                        Ok(file) => {
                            let created = parent.join(&temporary);
                            trace("create", &[&created]);
                            let mut out = Out::default();
                            out.u64(id).path(&created);
                            self.open.insert(
                                id,
                                Operation {
                                    dir,
                                    parent,
                                    temporary,
                                    name,
                                    target: path,
                                },
                            );
                            return Ok(Reply::Ok(out.0, Some(file)));
                        }
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(Reply::Os(e)),
                    }
                }
                Err(Reply::Os(io::Error::from(io::ErrorKind::AlreadyExists)))
            }
            Request::Commit(id, target) => {
                let operation = self.operation(id)?;
                if operation.target != target {
                    return Err(Reply::Refused(format!(
                        "denied by the sandbox: operation {id:016x} does not write {}",
                        target.display()
                    )));
                }
                fdio::replace_in(&operation.dir, &operation.temporary, &operation.name)
                    .map_err(Reply::Os)?;
                trace(
                    "rename",
                    &[
                        &operation.parent.join(&operation.temporary),
                        &operation.target,
                    ],
                );
                let operation = self
                    .open
                    .remove(&id)
                    .expect("the operation was found above");
                self.committed
                    .push_back((id, operation.dir, operation.parent));
                while self.committed.len() > KEEP_COMMITTED {
                    self.committed.pop_front();
                }
                Ok(Reply::ok())
            }
            Request::Abort(id) => {
                self.operation(id)?;
                let operation = self
                    .open
                    .remove(&id)
                    .expect("the operation was found above");
                match fdio::remove_in(&operation.dir, &operation.temporary) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => Err(Reply::Os(e)),
                    removed => {
                        if removed.is_ok() {
                            trace("unlink", &[&operation.parent.join(&operation.temporary)]);
                        }
                        Ok(Reply::ok())
                    }
                }
            }
            Request::Sync(id) => {
                let Some((_, dir, path)) = self.committed.iter().find(|(i, _, _)| *i == id) else {
                    return Err(Reply::Refused(format!(
                        "denied by the sandbox: operation {id:016x} is not a committed atomic file of this program"
                    )));
                };
                fdio::sync_directory(dir, path).map_err(Reply::Os)?;
                Ok(Reply::ok())
            }
            Request::SyncDirectory(dir) => {
                split(&dir)?;
                if !self.permits_directory(&dir) {
                    return Err(refused_scope(&dir));
                }
                let fd = walk(&dir)?;
                fdio::sync_directory(&fd, &dir).map_err(Reply::Os)?;
                Ok(Reply::ok())
            }
            Request::MakeDirectory(path) => {
                let (parent, name, dir) = self.target(&path)?;
                fdio::make_directory_in(&dir, &name).map_err(Reply::Os)?;
                trace("mkdir", &[&path]);
                // The entry of the new directory is synced, so that a durable write into it
                // survives power loss (S-9, docs/runtime.md 10). The directory exists either
                // way, so the result of the sync is not reported: a file system that cannot
                // sync a directory gives no guarantee after power loss in any case (S-9a).
                let _ = fdio::sync_directory(&dir, &parent);
                Ok(Reply::ok())
            }
            Request::Rename(source, target) => {
                let (_, from, from_dir) = self.target(&source)?;
                let (_, to, to_dir) = self.target(&target)?;
                fdio::rename_without_replacing(&from_dir, &from, &to_dir, &to)
                    .map_err(Reply::Os)?;
                trace("rename-noreplace", &[&source, &target]);
                Ok(Reply::ok())
            }
        }
    }

    /// The open operation with this id, which only this writer issued.
    fn operation(&self, id: u64) -> Result<&Operation, Reply> {
        self.open.get(&id).ok_or_else(|| {
            Reply::Refused(format!(
                "denied by the sandbox: operation {id:016x} is not an open atomic file of this program"
            ))
        })
    }

    #[cfg(target_os = "linux")]
    fn file_system(&self, dir: &Path) -> Option<String> {
        super::super::externs::durability::file_system_from_mountinfo(
            self.mountinfo.as_deref()?,
            dir.to_str()?,
        )
    }

    #[cfg(not(target_os = "linux"))]
    fn file_system(&self, dir: &Path) -> Option<String> {
        let _ = &self.mountinfo;
        super::super::externs::durability::file_system_name(dir)
    }
}

/// Records an operation of the writer for the crash trials (`file_trace`); only observes.
fn trace(operation: &str, paths: &[&Path]) {
    if !crate::file_trace::enabled() {
        return;
    }
    let paths: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| p.as_os_str().as_bytes().to_vec())
        .collect();
    let paths: Vec<&[u8]> = paths.iter().map(Vec::as_slice).collect();
    crate::file_trace::record(operation, &paths);
}

fn read_frame(stream: &mut UnixStream) -> io::Result<Option<Result<Vec<u8>, String>>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match stream.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let length = u32::from_le_bytes(head) as usize;
    if length > MAX_FRAME {
        return Ok(Some(Err(format!(
            "a frame of {length} bytes is over the limit of {MAX_FRAME}"
        ))));
    }
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body)?;
    Ok(Some(Ok(body)))
}

fn send_reply(stream: &mut UnixStream, reply: Reply) -> io::Result<()> {
    let (frame, fd) = reply.encode();
    let sent = fdio::send_with_descriptor(
        stream.as_raw_fd(),
        &frame,
        fd.as_ref().map(|f| f.as_raw_fd()),
    )?;
    // The writer keeps no descriptor of a temporary file: the program writes it.
    drop(fd);
    stream.write_all(&frame[sent..])
}

/// Serves requests until the program closes the connection. A frame over the limit ends the
/// connection after a refusal, since the stream can no longer be read in step.
pub fn serve(mut stream: UnixStream, server: &mut Server) -> io::Result<()> {
    loop {
        let body = match read_frame(&mut stream)? {
            None => return Ok(()),
            Some(Err(problem)) => {
                send_reply(&mut stream, Reply::Malformed(problem))?;
                return Ok(());
            }
            Some(Ok(body)) => body,
        };
        let reply = match Request::decode(&body) {
            Ok(request) => server.handle(request),
            Err(problem) => Reply::Malformed(problem),
        };
        send_reply(&mut stream, reply)?;
    }
}

/// Serves on the stream in a thread of this process, without the writer's own sandbox: for the
/// tests of the protocol. The first message must be the configuration, as in the process.
pub fn serve_in_thread(mut stream: UnixStream) -> std::thread::JoinHandle<io::Result<()>> {
    std::thread::spawn(move || {
        let scopes = match configuration(&mut stream)? {
            Ok(scopes) => scopes,
            Err(problem) => return send_reply(&mut stream, Reply::Malformed(problem)),
        };
        let mut server = Server::new(scopes);
        send_reply(&mut stream, Reply::ok())?;
        serve(stream, &mut server)
    })
}

fn configuration(stream: &mut UnixStream) -> io::Result<Result<Vec<Scope>, String>> {
    Ok(match read_frame(stream)? {
        None => Err("the connection closed before the configuration".into()),
        Some(Err(problem)) => Err(problem),
        Some(Ok(body)) => match Request::decode(&body) {
            Ok(Request::Config(scopes)) => Ok(scopes),
            Ok(_) => Err("the first message must be the configuration".into()),
            Err(problem) => Err(problem),
        },
    })
}

static CAN_SERVE: AtomicBool = AtomicBool::new(false);

/// The hook at the start of `main` of every executable that runs sandboxed programs: `unio`
/// and the executables of `unio build`. Started as a writer (the variable [`ENV`]), it serves
/// and ends the process; otherwise it returns, and the executable may start writers, since it
/// can be one.
pub fn serve_if_requested() {
    CAN_SERVE.store(true, Ordering::SeqCst);
    let Some(value) = std::env::var_os(ENV) else {
        return;
    };
    std::process::exit(run_writer(&value.to_string_lossy()));
}

/// Whether this executable can be started as a writer.
pub fn can_serve() -> bool {
    CAN_SERVE.load(Ordering::SeqCst)
}

fn run_writer(value: &str) -> i32 {
    let fail = |message: &str| {
        eprintln!("unio: the writer process: {message}");
        crate::fault::EXIT_SANDBOX
    };
    let Some((version, parent)) = value.split_once(':') else {
        return fail("started without its protocol version");
    };
    if version != VERSION.to_string() {
        return fail("started with another protocol version");
    }
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn prctl(option: i32, ...) -> i32;
            fn getppid() -> i32;
        }
        const PR_SET_PDEATHSIG: i32 = 1;
        const SIGKILL: i32 = 9;
        // SAFETY: integer arguments only.
        unsafe { prctl(PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0) };
        // The parent may have ended before the signal was asked for.
        // SAFETY: getppid takes no arguments.
        if parent.parse::<i32>().ok() != Some(unsafe { getppid() }) {
            return fail("its program has already ended");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = parent;
    // SAFETY: the starter put the writer's end of the socket at this descriptor.
    let mut stream = unsafe { UnixStream::from_raw_fd(SOCKET_FD) };
    let scopes = match configuration(&mut stream) {
        Ok(Ok(scopes)) => scopes,
        Ok(Err(problem)) => {
            let _ = send_reply(&mut stream, Reply::Malformed(problem.clone()));
            return fail(&problem);
        }
        Err(e) => return fail(&e.to_string()),
    };
    // Everything the writer reads from outside its scopes is read now.
    let mut server = Server::new(scopes);
    if let Err(problem) = super::apply_writer_os(&server.scopes) {
        let _ = send_reply(
            &mut stream,
            Reply::Refused(format!("the writer cannot apply its sandbox: {problem}")),
        );
        return fail(&problem);
    }
    if send_reply(&mut stream, Reply::ok()).is_err() {
        return fail("the program closed the connection");
    }
    match serve(stream, &mut server) {
        Ok(()) => 0,
        Err(e) => fail(&e.to_string()),
    }
}

// ---------------------------------------------------------------- the program's side

/// A connection to a writer, from the program process.
pub struct Handle {
    stream: Mutex<UnixStream>,
    child: Mutex<Option<std::process::Child>>,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "writer {:?}", self.pid())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Ok(stream) = self.stream.lock() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Handle {
    /// Starts this executable as the writer of the scopes.
    pub fn start(scopes: &[Scope]) -> Result<Handle, String> {
        if !can_serve() {
            return Err(
                "this executable cannot start the writer process of the sandbox".to_string(),
            );
        }
        #[cfg(target_os = "linux")]
        let exe = PathBuf::from("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let exe = std::env::current_exe().map_err(|e| format!("the executable: {e}"))?;
        Handle::start_with(&exe, scopes)
    }

    /// Starts `exe` as the writer of the scopes and waits until it has applied its sandbox.
    pub fn start_with(exe: &Path, scopes: &[Scope]) -> Result<Handle, String> {
        use std::os::unix::process::CommandExt;
        extern "C" {
            fn dup2(from: i32, to: i32) -> i32;
        }
        let problem = |e: io::Error| format!("the writer process cannot start: {e}");
        let (ours, theirs) = UnixStream::pair().map_err(problem)?;
        let fd = theirs.as_raw_fd();
        let mut command = std::process::Command::new(exe);
        command
            .env_clear()
            .env(ENV, format!("{VERSION}:{}", std::process::id()))
            .current_dir("/")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit());
        // The trace of the crash trials only observes; the writer records on the standard
        // error it inherits anyway (`file_trace`).
        if crate::file_trace::enabled() {
            command.env(crate::file_trace::ENV, "1");
        }
        // SAFETY: the closure only calls dup2 and fcntl, which are async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                if fd == SOCKET_FD {
                    fdio::keep_on_exec(fd)
                } else if dup2(fd, SOCKET_FD) < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command.spawn().map_err(problem)?;
        drop(theirs);
        let handle = Handle {
            stream: Mutex::new(ours),
            child: Mutex::new(Some(child)),
        };
        handle
            .request(&Request::Config(scopes.to_vec()))
            .map_err(|f| f.to_string())?;
        Ok(handle)
    }

    /// A connection over a stream whose other end a writer serves, as [`serve_in_thread`]; the
    /// configuration is sent here.
    pub fn connect(stream: UnixStream, scopes: &[Scope]) -> Result<Handle, Failure> {
        let handle = Handle {
            stream: Mutex::new(stream),
            child: Mutex::new(None),
        };
        handle.request(&Request::Config(scopes.to_vec()))?;
        Ok(handle)
    }

    /// The process id of the writer, if this handle started one.
    pub fn pid(&self) -> Option<u32> {
        self.child
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|c| c.id()))
    }

    /// Sends one frame as it is and returns the body of the reply and a descriptor that came
    /// with it: for the tests of malformed requests.
    pub fn exchange(&self, frame: &[u8]) -> io::Result<(Vec<u8>, Option<OwnedFd>)> {
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| io::Error::other("the connection is poisoned"))?;
        stream.write_all(frame)?;
        let mut head = [0u8; 4];
        let mut got = 0;
        let mut fd = None;
        while got < 4 {
            let (n, received) =
                fdio::receive_with_descriptor(stream.as_raw_fd(), &mut head[got..])?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            if received.is_some() {
                fd = received;
            }
            got += n;
        }
        let length = u32::from_le_bytes(head) as usize;
        if length > MAX_FRAME {
            return Err(io::Error::other("a reply over the size limit"));
        }
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body)?;
        Ok((body, fd))
    }

    /// Sends a request; the payload of the reply and the descriptor that came with it.
    pub fn request(&self, request: &Request) -> Result<(Vec<u8>, Option<OwnedFd>), Failure> {
        let (body, fd) = self
            .exchange(&request.encode())
            .map_err(|e| Failure::Broken(e.to_string()))?;
        Ok((decode_reply(&body)?, fd))
    }

    fn unit(&self, request: &Request) -> Result<(), Failure> {
        self.request(request).map(drop)
    }

    pub fn durable(&self, target: &Path) -> Result<bool, Failure> {
        let (payload, _) = self.request(&Request::Durable(target.to_path_buf()))?;
        Ok(payload.first() == Some(&1))
    }

    /// A new temporary file for the target: the operation id, its path, and the file.
    pub fn open(&self, target: &Path) -> Result<(u64, PathBuf, std::fs::File), Failure> {
        let (payload, fd) = self.request(&Request::Open(target.to_path_buf()))?;
        let mut r = In {
            data: &payload,
            at: 0,
        };
        let broken = || Failure::Broken("a malformed reply to open".into());
        let id = r.u64().ok_or_else(broken)?;
        let path = r.path().ok_or_else(broken)?;
        let fd = fd.ok_or_else(|| Failure::Broken("the reply to open has no descriptor".into()))?;
        Ok((id, path, std::fs::File::from(fd)))
    }

    pub fn commit(&self, id: u64, target: &Path) -> Result<(), Failure> {
        self.unit(&Request::Commit(id, target.to_path_buf()))
    }

    pub fn abort(&self, id: u64) -> Result<(), Failure> {
        self.unit(&Request::Abort(id))
    }

    pub fn sync(&self, id: u64) -> Result<(), Failure> {
        self.unit(&Request::Sync(id))
    }

    pub fn sync_directory(&self, dir: &Path) -> Result<(), Failure> {
        self.unit(&Request::SyncDirectory(dir.to_path_buf()))
    }

    pub fn make_directory(&self, path: &Path) -> Result<(), Failure> {
        self.unit(&Request::MakeDirectory(path.to_path_buf()))
    }

    pub fn rename(&self, source: &Path, target: &Path) -> Result<(), Failure> {
        self.unit(&Request::Rename(source.to_path_buf(), target.to_path_buf()))
    }
}

/// The path the writer is asked about: absolute, without `.` and `..`, its directory resolved
/// through the file system as the runtime checks resolve it, and its last name kept as given,
/// so that the entry itself is written and not what a link there points to.
pub fn target_path(path: &str) -> Option<PathBuf> {
    let absolute = super::absolute(Path::new(path));
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    let name = lexical.file_name()?.to_os_string();
    let parent = super::resolve(lexical.parent()?);
    Some(parent.join(name))
}

/// The writer of one running program and the operations it has issued to it.
pub(crate) struct Session {
    handle: Arc<Handle>,
    /// The temporary files of open operations, by the path the program was given.
    temporaries: RefCell<HashMap<String, (u64, PathBuf)>>,
    /// Recently committed operations by the directory of their target, newest last.
    committed: RefCell<VecDeque<(PathBuf, u64)>>,
}

impl Session {
    pub(crate) fn new(handle: Arc<Handle>) -> Session {
        Session {
            handle,
            temporaries: RefCell::new(HashMap::new()),
            committed: RefCell::new(VecDeque::new()),
        }
    }

    pub(crate) fn handle(&self) -> &Handle {
        &self.handle
    }

    pub(crate) fn open(&self, target: &Path) -> Result<(PathBuf, std::fs::File), Failure> {
        let (id, temporary, file) = self.handle.open(target)?;
        self.temporaries.borrow_mut().insert(
            temporary.to_string_lossy().into_owned(),
            (id, target.to_path_buf()),
        );
        Ok((temporary, file))
    }

    /// Commits the operation of the temporary file onto the target.
    pub(crate) fn commit(&self, temporary: &str, target: &Path) -> Result<(), Failure> {
        let Some((id, _)) = self.temporaries.borrow().get(temporary).cloned() else {
            return Err(Failure::Refused(format!(
                "denied by the sandbox: {temporary} is not a temporary file of an open atomic file"
            )));
        };
        self.handle.commit(id, target)?;
        self.temporaries.borrow_mut().remove(temporary);
        if let Some(dir) = target.parent() {
            let mut committed = self.committed.borrow_mut();
            committed.push_back((dir.to_path_buf(), id));
            while committed.len() > KEEP_COMMITTED {
                committed.pop_front();
            }
        }
        Ok(())
    }

    /// Removes the temporary file of an open operation; anything else is left alone.
    pub(crate) fn abort(&self, temporary: &str) {
        let found = self.temporaries.borrow_mut().remove(temporary);
        if let Some((id, _)) = found {
            let _ = self.handle.abort(id);
        }
    }

    /// Syncs the directory: through the newest commit into it, whose directory the writer
    /// still holds open, or else by its path.
    pub(crate) fn sync_directory(&self, dir: &Path) -> Result<(), Failure> {
        let id = self
            .committed
            .borrow()
            .iter()
            .rev()
            .find(|(d, _)| d == dir)
            .map(|(_, id)| *id);
        if let Some(id) = id {
            match self.handle.sync(id) {
                // The writer no longer holds the directory of that commit.
                Err(Failure::Refused(_)) => {}
                done => return done,
            }
        }
        self.handle.sync_directory(dir)
    }
}
