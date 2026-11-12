//! Trials of the file guarantees of S-9 against crashes (S-9b, docs/runtime.md 10).
//!
//! **Simulated power loss.** A Unio program runs with the test hook `UNIO_TEST_FILE_TRACE=1`
//! (`unio-runtime/src/file_trace.rs`), which records every operation that decides what a file
//! system may keep after power loss: the creation of a temporary file, each write into it, each
//! sync of a file or directory with the call that did it, each rename, removal, and new
//! directory, from the program and from the writer process of the sandbox. The trace is replayed
//! on a model file system, in the manner of ALICE and CrashMonkey: at every crash point (every
//! prefix of the trace), every state the model allows the storage to keep is built, and the
//! guarantees are checked in each:
//!
//! - guarantees 1 and 2: the target holds its old content or the complete content of one
//!   commit that reached its rename, never a mix, a prefix, or zeros;
//! - guarantee 3: once a commit with `durable = true` has returned, the target holds that
//!   content or a later one;
//! - no other name appears in the directory than the targets, the files that were there, new
//!   directories, and the temporary files `.<name>.unio-tmp-*` that S-9 allows after a crash,
//!   and the files that were there keep their content.
//!
//! The model keeps a file's data only up to its last sync by a call that reaches storage on the
//! platform (`fsync` on Linux, `fcntl(F_FULLFSYNC)` on macOS, where a plain `fsync` does not
//! count, `FlushFileBuffers` on Windows); data written after it may be lost from any write on.
//! An entry of a directory (create, rename, unlink, mkdir) is kept for certain only once its
//! directory is synced after it, or, on Windows, once a rename with `MOVEFILE_WRITE_THROUGH`
//! has returned at or after it. Entries that are not certain are kept in two ways: *ordered*,
//! a prefix in the order of the operations, as the journals of ext4, XFS, APFS, and NTFS keep
//! them; and *weak*, any subset, which is all POSIX promises. The weak model also lets a file
//! keep its new length with zeros instead of its unsynced data. A rename of one entry is
//! atomic in both. The trace of a real run is also replayed without a crash and compared with
//! the file system the run left, so that a missing record cannot go unnoticed.
//!
//! The harness is first shown to catch broken protocols: the real trace is changed into a
//! protocol without the data sync, with the rename before the data sync, without the directory
//! sync, without the sync of a new directory's parent, and with a plain `fsync` on macOS, and
//! each must fail; Windows is checked on the trace of its code path.
//!
//! **Killed processes.** A program writes in a loop and is killed with SIGKILL (on Windows
//! `TerminateProcess`) at random points, and on Linux the writer process alone is killed; the
//! target must then hold one complete content, and only temporary files may be left beside it.
//!
//! What is not trialled: a real power cut, and recorded block writes (dm-log-writes, dm-flakey,
//! a loop device), which need root and are not available in CI. Storage that acknowledges
//! writes it has not stored is beyond any software check (S-9a).

// The trials of real traces need the writer process and run on Linux and macOS; on Windows only
// the model and the killed programs are checked, so the helpers of the others stay unused there.
#![cfg_attr(windows, allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------- the trace

/// One recorded operation.
#[derive(Clone, Debug, PartialEq)]
enum Op {
    /// A new empty file.
    Create(String),
    /// A descriptor of the program for the file at the path.
    Open(u64, String),
    /// Data appended through the descriptor.
    Write(u64, Vec<u8>),
    /// A sync of the file's data, by the named call.
    Sync(u64, String),
    Close(u64),
    /// A rename that replaces the target.
    Rename(String, String),
    /// MoveFileExW with MOVEFILE_WRITE_THROUGH (Windows).
    RenameWriteThrough(String, String),
    /// A rename that never replaces (`std.files.rename`).
    RenameNoReplace(String, String),
    Unlink(String),
    Mkdir(String),
    /// A sync of a directory, by the named call.
    SyncDir(String, String),
}

fn unhex(text: &str) -> Vec<u8> {
    if text == "-" {
        return Vec::new();
    }
    assert!(text.len().is_multiple_of(2), "odd hex {text}");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

/// The operations in the standard error of a run; any other line is ignored, an unknown
/// operation fails the test.
fn parse(stderr: &str) -> Vec<Op> {
    let mut ops = Vec::new();
    for line in stderr.lines() {
        let mut words = line.split(' ');
        if words.next() != Some(unio_runtime::file_trace::PREFIX) {
            continue;
        }
        let name = words.next().expect("an operation");
        let args: Vec<Vec<u8>> = words.map(unhex).collect();
        let text = |i: usize| utf8(args[i].clone());
        let number = |i: usize| text(i).parse::<u64>().unwrap();
        ops.push(match (name, args.len()) {
            ("create", 1) => Op::Create(text(0)),
            ("open", 2) => Op::Open(number(0), text(1)),
            ("write", 2) => Op::Write(number(0), args[1].clone()),
            ("fsync", 2) => Op::Sync(number(0), text(1)),
            ("close", 1) => Op::Close(number(0)),
            ("rename", 2) => Op::Rename(text(0), text(1)),
            ("rename-write-through", 2) => Op::RenameWriteThrough(text(0), text(1)),
            ("rename-noreplace", 2) => Op::RenameNoReplace(text(0), text(1)),
            ("unlink", 1) => Op::Unlink(text(0)),
            ("mkdir", 1) => Op::Mkdir(text(0)),
            ("fsync-dir", 2) => Op::SyncDir(text(0), text(1)),
            other => panic!("an unknown operation in the trace: {other:?} in {line}"),
        });
    }
    ops
}

/// The directory and the name of a path of the trace.
fn split(path: &str) -> (&str, &str) {
    let at = path.rfind(['/', '\\']).expect("an absolute path");
    (&path[..at.max(1)], &path[at + 1..])
}

// ---------------------------------------------------------------- the model

#[derive(Clone, Copy, Debug, PartialEq)]
enum Platform {
    Linux,
    Apple,
    Windows,
}

impl Platform {
    fn host() -> Platform {
        if cfg!(target_vendor = "apple") {
            Platform::Apple
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// Whether a sync by this call reaches storage (S-9a).
    fn reaches_storage(self, call: &str) -> bool {
        match self {
            Platform::Linux => call == "fsync",
            Platform::Apple => call == "fullfsync",
            Platform::Windows => call == "flush",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Persistence {
    /// Uncertain entries are kept as a prefix in the order of the operations (journaled).
    Ordered,
    /// Any subset of uncertain entries is kept (POSIX).
    Weak,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Node {
    File(usize),
    Dir,
}

/// A change of directory entries that is kept or lost as a whole.
#[derive(Clone, Debug)]
struct Entry {
    /// The index of the operation in the trace.
    at: usize,
    changes: Vec<(String, Option<Node>)>,
    /// The directories that must be synced after it to keep it.
    dirs: BTreeSet<String>,
    synced: BTreeSet<String>,
    /// Kept for certain by a write-through rename (Windows).
    written_through: bool,
}

impl Entry {
    fn certain(&self) -> bool {
        self.written_through || self.dirs.is_subset(&self.synced)
    }
}

/// The state that existed before the trace, all of it on storage.
#[derive(Clone, Debug)]
struct World {
    /// The directory of the trial; everything the trace touches is below it.
    root: String,
    names: BTreeMap<String, Node>,
    contents: Vec<Vec<u8>>,
}

impl World {
    /// The files and directories below `root` as they are on disk.
    fn of(root: &Path) -> World {
        let mut world = World {
            root: root.to_string_lossy().into_owned(),
            names: BTreeMap::new(),
            contents: Vec::new(),
        };
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = path.to_string_lossy().into_owned();
                if path.is_dir() {
                    world.names.insert(name, Node::Dir);
                    stack.push(path);
                } else {
                    world.contents.push(fs::read(&path).unwrap());
                    world
                        .names
                        .insert(name, Node::File(world.contents.len() - 1));
                }
            }
        }
        world
    }
}

/// The volatile state after a prefix of the trace: what a reader saw before the crash, and what
/// storage holds for certain.
struct Volatile {
    names: BTreeMap<String, Node>,
    /// The writes of each file in order; a file that existed before has one.
    writes: Vec<Vec<Vec<u8>>>,
    /// How many writes of each file are on storage for certain.
    synced: Vec<usize>,
    descriptors: BTreeMap<u64, usize>,
    entries: Vec<Entry>,
}

impl Volatile {
    fn replay(world: &World, ops: &[Op], platform: Platform) -> Volatile {
        let mut v = Volatile {
            names: world.names.clone(),
            writes: world.contents.iter().map(|c| vec![c.clone()]).collect(),
            synced: vec![1; world.contents.len()],
            descriptors: BTreeMap::new(),
            entries: Vec::new(),
        };
        for (at, op) in ops.iter().enumerate() {
            v.apply(at, op, platform);
        }
        v
    }

    fn entry(&mut self, at: usize, changes: Vec<(String, Option<Node>)>, through: bool) {
        let dirs = changes
            .iter()
            .map(|(p, _)| split(p).0.to_string())
            .collect();
        for (path, node) in &changes {
            match node {
                Some(n) => self.names.insert(path.clone(), *n),
                None => self.names.remove(path),
            };
        }
        if through {
            // NTFS keeps its log in order: a write-through rename keeps every entry before it.
            for earlier in &mut self.entries {
                earlier.written_through = true;
            }
        }
        self.entries.push(Entry {
            at,
            changes,
            dirs,
            synced: BTreeSet::new(),
            written_through: through,
        });
    }

    fn node(&self, path: &str, at: usize) -> Node {
        *self
            .names
            .get(path)
            .unwrap_or_else(|| panic!("operation {at} names {path}, which does not exist"))
    }

    fn apply(&mut self, at: usize, op: &Op, platform: Platform) {
        match op {
            Op::Create(path) => {
                self.writes.push(Vec::new());
                self.synced.push(0);
                let file = Node::File(self.writes.len() - 1);
                self.entry(at, vec![(path.clone(), Some(file))], false);
            }
            Op::Open(d, path) => match self.node(path, at) {
                Node::File(i) => {
                    self.descriptors.insert(*d, i);
                }
                Node::Dir => panic!("operation {at} opens a directory"),
            },
            Op::Write(d, data) => {
                let i = self.descriptors[d];
                self.writes[i].push(data.clone());
            }
            Op::Sync(d, call) => {
                if platform.reaches_storage(call) {
                    let i = self.descriptors[d];
                    self.synced[i] = self.writes[i].len();
                }
            }
            // A descriptor stays known after it is closed, so that a broken protocol that syncs
            // later can be modelled; an Open of the same number replaces it.
            Op::Close(_) => {}
            Op::Rename(from, to)
            | Op::RenameNoReplace(from, to)
            | Op::RenameWriteThrough(from, to) => {
                let node = self.node(from, at);
                let through =
                    matches!(op, Op::RenameWriteThrough(..)) && platform == Platform::Windows;
                self.entry(
                    at,
                    vec![(from.clone(), None), (to.clone(), Some(node))],
                    through,
                );
            }
            Op::Unlink(path) => {
                self.node(path, at);
                self.entry(at, vec![(path.clone(), None)], false);
            }
            Op::Mkdir(path) => self.entry(at, vec![(path.clone(), Some(Node::Dir))], false),
            Op::SyncDir(call, dir) => {
                if platform.reaches_storage(call) {
                    for entry in &mut self.entries {
                        if entry.dirs.contains(dir) {
                            entry.synced.insert(dir.clone());
                        }
                    }
                }
            }
        }
    }

    /// The contents storage may hold for a file.
    fn contents(&self, file: usize, persistence: Persistence) -> Vec<Vec<u8>> {
        let writes = &self.writes[file];
        let synced = self.synced[file];
        let mut out = Vec::new();
        for kept in synced..=writes.len() {
            out.push(writes[..kept].concat());
        }
        if persistence == Persistence::Weak && writes.len() > synced {
            let mut zeros = writes[..synced].concat();
            zeros.resize(writes.concat().len(), 0);
            out.push(zeros);
        }
        out
    }

    /// The content a reader sees now.
    fn content(&self, file: usize) -> Vec<u8> {
        self.writes[file].concat()
    }

    /// Every set of entries storage may hold, as the indices of the entries kept.
    fn kept_entries(&self, persistence: Persistence) -> Vec<Vec<usize>> {
        let n = self.entries.len();
        match persistence {
            Persistence::Ordered => {
                let first = self
                    .entries
                    .iter()
                    .rposition(Entry::certain)
                    .map_or(0, |i| i + 1);
                (first..=n).map(|cut| (0..cut).collect()).collect()
            }
            Persistence::Weak => {
                let uncertain: Vec<usize> =
                    (0..n).filter(|&i| !self.entries[i].certain()).collect();
                assert!(
                    uncertain.len() <= 16,
                    "too many uncertain entries to enumerate"
                );
                (0u32..1 << uncertain.len())
                    .map(|mask| {
                        (0..n)
                            .filter(|&i| match uncertain.iter().position(|&u| u == i) {
                                Some(bit) => mask & (1 << bit) != 0,
                                None => true,
                            })
                            .collect()
                    })
                    .collect()
            }
        }
    }

    /// The names storage holds when it keeps these entries, without names whose directory it
    /// did not keep.
    fn names_kept(&self, world: &World, kept: &[usize]) -> BTreeMap<String, Node> {
        let mut names = world.names.clone();
        for &i in kept {
            for (path, node) in &self.entries[i].changes {
                match node {
                    Some(n) => names.insert(path.clone(), *n),
                    None => names.remove(path),
                };
            }
        }
        let visible = |path: &str| {
            let mut dir = split(path).0;
            while dir.len() > world.root.len() {
                if names.get(dir) != Some(&Node::Dir) {
                    return false;
                }
                dir = split(dir).0;
            }
            true
        };
        names
            .iter()
            .filter(|(p, _)| visible(p))
            .map(|(p, n)| (p.clone(), *n))
            .collect()
    }
}

/// One commit onto a target, found in the trace.
#[derive(Debug)]
struct Commit {
    content: Vec<u8>,
    /// The index of its rename.
    renamed: usize,
    /// The index after which its call has returned: the directory sync of a durable commit,
    /// the rename otherwise.
    returned: usize,
    durable: bool,
}

/// What a trial expects of one target: its content before, and the content and `durable` of
/// each of its commits, in order.
struct Target {
    path: String,
    commits: Vec<(Vec<u8>, bool)>,
}

/// The commits onto the target in the trace, checked against what the program wrote.
fn commits_of(world: &World, ops: &[Op], target: &Target, platform: Platform) -> Vec<Commit> {
    let mut found = Vec::new();
    for (at, op) in ops.iter().enumerate() {
        let to = match op {
            Op::Rename(_, to) | Op::RenameWriteThrough(_, to) => to,
            _ => continue,
        };
        if *to != target.path {
            continue;
        }
        let before = Volatile::replay(world, &ops[..at], platform);
        let Node::File(file) = before.node(split_rename(op).0, at) else {
            panic!("a directory replaced the target");
        };
        let (content, durable) = target
            .commits
            .get(found.len())
            .cloned()
            .unwrap_or_else(|| panic!("more commits onto {} than expected", target.path));
        assert_eq!(
            before.content(file),
            content,
            "commit {} onto {} wrote other data than the program",
            found.len(),
            target.path
        );
        let dir = split(&target.path).0;
        let returned = if durable {
            ops.iter()
                .enumerate()
                .skip(at + 1)
                .find(|(_, o)| matches!(o, Op::SyncDir(_, d) if d == dir))
                .map_or(at, |(i, _)| i)
        } else {
            at
        };
        found.push(Commit {
            content,
            renamed: at,
            returned,
            durable,
        });
    }
    assert_eq!(
        found.len(),
        target.commits.len(),
        "the trace has another number of commits onto {}",
        target.path
    );
    found
}

fn split_rename(op: &Op) -> (&str, &str) {
    match op {
        Op::Rename(a, b) | Op::RenameWriteThrough(a, b) | Op::RenameNoReplace(a, b) => (a, b),
        _ => unreachable!(),
    }
}

fn shown(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() > 40 {
        format!("{:?}... ({} bytes)", &text[..40], bytes.len())
    } else {
        format!("{text:?}")
    }
}

/// Replays every prefix of the trace and checks the guarantees in every state the model allows.
/// The number of states checked, or the first violation.
fn check(
    world: &World,
    ops: &[Op],
    targets: &[Target],
    platform: Platform,
    persistence: Persistence,
) -> Result<usize, String> {
    let commits: Vec<Vec<Commit>> = targets
        .iter()
        .map(|t| commits_of(world, ops, t, platform))
        .collect();
    let target_names: BTreeSet<&str> = targets.iter().map(|t| t.path.as_str()).collect();
    let temporary = |path: &str| {
        let (dir, name) = split(path);
        targets.iter().any(|t| {
            let (tdir, tname) = split(&t.path);
            dir == tdir && name.starts_with(&format!(".{tname}.unio-tmp-"))
        })
    };
    let made: BTreeSet<&str> = ops
        .iter()
        .filter_map(|o| match o {
            Op::Mkdir(p) => Some(p.as_str()),
            _ => None,
        })
        .collect();
    let mut states = 0;
    for crash in 0..=ops.len() {
        let v = Volatile::replay(world, &ops[..crash], platform);
        let at = |kept: &[usize]| {
            let last = ops[..crash].last().map(|o| format!("{o:?}"));
            let lost: Vec<usize> = (0..v.entries.len())
                .filter(|i| !kept.contains(i))
                .map(|i| v.entries[i].at)
                .collect();
            format!(
                "crash after {crash} of {} operations (last {last:?}), {persistence:?} model on \
                 {platform:?}, entries of operations {lost:?} lost",
                ops.len()
            )
        };
        for kept in v.kept_entries(persistence) {
            let names = v.names_kept(world, &kept);
            for (path, node) in &names {
                if world.names.contains_key(path)
                    || target_names.contains(path.as_str())
                    || made.contains(path.as_str())
                    || temporary(path)
                {
                    continue;
                }
                return Err(format!(
                    "{}: an unexpected name {path} ({node:?})",
                    at(&kept)
                ));
            }
            for (path, node) in &world.names {
                if target_names.contains(path.as_str()) {
                    continue;
                }
                if let Node::File(old) = node {
                    match names.get(path) {
                        Some(Node::File(f)) if f == old => {}
                        other => {
                            return Err(format!(
                                "{}: the neighbour {path} became {other:?}",
                                at(&kept)
                            ))
                        }
                    }
                }
            }
            for (target, commits) in targets.iter().zip(&commits) {
                let renamed: Vec<&Commit> = commits.iter().filter(|c| c.renamed < crash).collect();
                let floor = commits
                    .iter()
                    .rposition(|c| c.durable && c.returned < crash);
                let mut allowed: Vec<Option<&[u8]>> = renamed
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| floor.is_none_or(|f| *i >= f))
                    .map(|(_, c)| Some(c.content.as_slice()))
                    .collect();
                if floor.is_none() {
                    allowed.push(match world.names.get(&target.path) {
                        Some(Node::File(f)) => Some(world.contents[*f].as_slice()),
                        Some(Node::Dir) => panic!("the target is a directory"),
                        None => None,
                    });
                }
                let held: Vec<Option<Vec<u8>>> = match names.get(&target.path) {
                    None => vec![None],
                    Some(Node::File(f)) => {
                        v.contents(*f, persistence).into_iter().map(Some).collect()
                    }
                    Some(Node::Dir) => {
                        return Err(format!("{}: {} is a directory", at(&kept), target.path))
                    }
                };
                for content in held {
                    states += 1;
                    if !allowed.contains(&content.as_deref()) {
                        let what = content.as_deref().map_or("no file".into(), shown);
                        let expected: Vec<String> = allowed
                            .iter()
                            .map(|a| a.map_or("no file".into(), shown))
                            .collect();
                        return Err(format!(
                            "{}: {} holds {what}, expected one of {expected:?}",
                            at(&kept),
                            target.path
                        ));
                    }
                }
            }
        }
    }
    Ok(states)
}

// ---------------------------------------------------------------- broken protocols

/// The trace without the data syncs: the rename may reach storage before the data.
fn without_data_sync(ops: &[Op]) -> Vec<Op> {
    ops.iter()
        .filter(|o| !matches!(o, Op::Sync(..)))
        .cloned()
        .collect()
}

/// The trace with each data sync moved after the rename of its file.
fn rename_before_sync(ops: &[Op]) -> Vec<Op> {
    let mut path_of = BTreeMap::new();
    let mut held: BTreeMap<String, Op> = BTreeMap::new();
    let mut out = Vec::new();
    for op in ops {
        match op {
            Op::Open(d, p) => {
                path_of.insert(*d, p.clone());
                out.push(op.clone());
            }
            Op::Sync(d, _) => {
                held.insert(path_of[d].clone(), op.clone());
            }
            Op::Rename(from, _) | Op::RenameWriteThrough(from, _) => {
                out.push(op.clone());
                out.extend(held.remove(from));
            }
            _ => out.push(op.clone()),
        }
    }
    out
}

/// The trace without directory syncs.
fn without_directory_sync(ops: &[Op]) -> Vec<Op> {
    ops.iter()
        .filter(|o| !matches!(o, Op::SyncDir(..)))
        .cloned()
        .collect()
}

/// The trace without the sync of the parent that follows a new directory.
fn without_parent_sync(ops: &[Op]) -> Vec<Op> {
    let mut out: Vec<Op> = Vec::new();
    for op in ops {
        if let (Op::SyncDir(_, dir), Some(Op::Mkdir(made))) = (op, out.last()) {
            if split(made).0 == dir {
                continue;
            }
        }
        out.push(op.clone());
    }
    out
}

/// The trace with every sync done by `call`.
fn synced_by(ops: &[Op], call: &str) -> Vec<Op> {
    ops.iter()
        .map(|o| match o {
            Op::Sync(d, _) => Op::Sync(*d, call.into()),
            Op::SyncDir(_, p) => Op::SyncDir(call.into(), p.clone()),
            other => other.clone(),
        })
        .collect()
}

/// The trace of the Windows code path of `std.files` (externs/files.rs): FlushFileBuffers, then
/// MoveFileExW with MOVEFILE_WRITE_THROUGH, and no sync of a directory.
fn as_on_windows(ops: &[Op], write_through: bool) -> Vec<Op> {
    ops.iter()
        .filter(|o| !matches!(o, Op::SyncDir(..)))
        .map(|o| match o {
            Op::Sync(d, _) => Op::Sync(*d, "flush".into()),
            Op::Rename(a, b) if write_through => Op::RenameWriteThrough(a.clone(), b.clone()),
            other => other.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------- running programs

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-power-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

/// A package whose program may read and write everything below `data/`.
fn package(name: &str, program: &str) -> PathBuf {
    let dir = scratch(name);
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(
        dir.join("unio.toml"),
        "[package]\nname = \"trial\"\n\n[permissions]\nfiles.read = [\"$CWD/data/**\"]\nfiles.write = [\"$CWD/data/**\"]\n",
    )
    .unwrap();
    fs::write(dir.join("main.unio"), program).unwrap();
    dir
}

fn command(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .env_remove(unio_runtime::file_trace::ENV)
        .arg("run");
    command.arg("main.unio");
    command
}

fn durable_here(dir: &Path) -> bool {
    unio_runtime::externs::durability::supports_durability(&dir.join("probe"))
}

/// The commits of the trial program, with `durable` only where the file system supports it.
fn trial_program(data: &Path, durable: bool) -> (String, Vec<Target>) {
    let d = |s: &str| data.join(s).to_string_lossy().into_owned();
    let flag = if durable { "true" } else { "false" };
    let program = format!(
        "import std.files

function run() fails with IoError:
    try files.write_text(Path(\"{a}\"), \"first\")
    try files.write_text(Path(\"{a}\"), \"second, durable\", durable = {flag})
    using file = try files.open_atomic(Path(\"{a}\"), durable = {flag}):
        try file.write(\"third \")
        try file.write(\"in \")
        try file.write(\"parts\")
        try file.commit()
    using file = try files.open_atomic(Path(\"{a}\")):
        try file.write(\"never committed\")
    try files.write_text(Path(\"{b}\"), \"a new file\", durable = {flag})
    try files.create_directory(Path(\"{dir}\"))
    try files.write_text(Path(\"{c}\"), \"in a new directory\", durable = {flag})
    try files.write_text(Path(\"{a}\"), \"last, not durable\")

public function main():
    try run() on IoError as e:
        print(\"error {{e.reason}}\")
        return
    print(\"done\")
",
        a = d("a.txt"),
        b = d("b.txt"),
        dir = d("new"),
        c = d("new/c.txt"),
    );
    let text = |s: &str| s.as_bytes().to_vec();
    let targets = vec![
        Target {
            path: d("a.txt"),
            commits: vec![
                (text("first"), false),
                (text("second, durable"), durable),
                (text("third in parts"), durable),
                (text("last, not durable"), false),
            ],
        },
        Target {
            path: d("b.txt"),
            commits: vec![(text("a new file"), durable)],
        },
        Target {
            path: d("new/c.txt"),
            commits: vec![(text("in a new directory"), durable)],
        },
    ];
    (program, targets)
}

/// Runs the trial program with the trace on: the world before, the trace, the targets.
fn traced_run(name: &str) -> (World, Vec<Op>, Vec<Target>, PathBuf) {
    traced_run_with(name, &[])
}

/// [`traced_run`] with more variables for the run, such as a test hook that makes syncs fail.
fn traced_run_with(name: &str, envs: &[(&str, &str)]) -> (World, Vec<Op>, Vec<Target>, PathBuf) {
    let dir = package(name, "");
    let data = dir.join("data");
    fs::write(data.join("a.txt"), "old").unwrap();
    fs::write(data.join("neighbour.txt"), "neighbour").unwrap();
    let durable = durable_here(&data);
    let (program, targets) = trial_program(&data, durable);
    fs::write(dir.join("main.unio"), program).unwrap();
    let world = World::of(&data);
    let output = command(&dir)
        .env(unio_runtime::file_trace::ENV, "1")
        .envs(envs.iter().copied())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("done"),
        "the trial program failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let ops = parse(&stderr);
    assert!(
        ops.len() > 30,
        "the trace is too short to be a trace of the program: {ops:?}"
    );
    // The trace without a crash must give the file system the run left, so that no operation
    // that changes it goes unrecorded.
    let v = Volatile::replay(&world, &ops, Platform::host());
    let now = World::of(&data);
    let replayed: BTreeMap<&String, Option<Vec<u8>>> = v
        .names
        .iter()
        .map(|(p, n)| {
            (
                p,
                match n {
                    Node::File(f) => Some(v.content(*f)),
                    Node::Dir => None,
                },
            )
        })
        .collect();
    let on_disk: BTreeMap<&String, Option<Vec<u8>>> = now
        .names
        .iter()
        .map(|(p, n)| {
            (
                p,
                match n {
                    Node::File(f) => Some(now.contents[*f].clone()),
                    Node::Dir => None,
                },
            )
        })
        .collect();
    assert_eq!(
        replayed, on_disk,
        "the trace does not replay to the file system"
    );
    (world, ops, targets, dir)
}

/// The data sync, the rename, and the directory sync are the calls of the platform, and the
/// directory sync after a durable commit comes from the writer process where there is one.
fn calls_of_the_platform(ops: &[Op], durable: bool) {
    let call = unio_runtime::file_trace::FILE_SYNC;
    assert!(ops.iter().any(|o| matches!(o, Op::Sync(..))));
    for op in ops {
        match op {
            Op::Sync(_, c) => assert_eq!(c, call, "{op:?}"),
            Op::SyncDir(c, _) => assert_eq!(c, call, "{op:?}"),
            _ => {}
        }
    }
    assert_eq!(
        ops.iter().any(|o| matches!(o, Op::SyncDir(..))),
        durable || cfg!(unix),
        "a directory sync is recorded"
    );
}

#[cfg(unix)]
#[test]
fn every_crash_point_of_a_real_trace_keeps_the_guarantees() {
    let (world, ops, targets, dir) = traced_run("real");
    let durable = targets[1].commits[0].1;
    calls_of_the_platform(&ops, durable);
    let platform = Platform::host();
    for persistence in [Persistence::Ordered, Persistence::Weak] {
        let states = check(&world, &ops, &targets, platform, persistence)
            .unwrap_or_else(|violation| panic!("{violation}\ntrace: {ops:#?}"));
        eprintln!(
            "{persistence:?} model on {platform:?}: {} crash points, {states} states",
            ops.len() + 1
        );
        assert!(states > 2 * ops.len());
    }
    // The same protocol under the rules of the other platforms: macOS with F_FULLFSYNC.
    let apple = synced_by(&ops, "fullfsync");
    for persistence in [Persistence::Ordered, Persistence::Weak] {
        check(&world, &apple, &targets, Platform::Apple, persistence).unwrap();
    }
    let _ = fs::remove_dir_all(dir);
}

#[track_caller]
fn caught(result: Result<usize, String>, what: &str) {
    match result {
        Err(violation) => eprintln!("{what}: caught: {violation}"),
        Ok(states) => panic!("{what}: the harness did not catch it in {states} states"),
    }
}

/// The negative trials: the harness catches each broken protocol in the trace of a real run.
#[cfg(unix)]
#[test]
fn the_harness_catches_broken_protocols() {
    let (world, ops, targets, dir) = traced_run("broken");
    let durable = targets[1].commits[0].1;
    let platform = Platform::host();
    for persistence in [Persistence::Ordered, Persistence::Weak] {
        caught(
            check(
                &world,
                &without_data_sync(&ops),
                &targets,
                platform,
                persistence,
            ),
            &format!("no data sync ({persistence:?})"),
        );
        caught(
            check(
                &world,
                &rename_before_sync(&ops),
                &targets,
                platform,
                persistence,
            ),
            &format!("rename before the data sync ({persistence:?})"),
        );
        // macOS: a plain fsync does not reach storage.
        caught(
            check(
                &world,
                &synced_by(&ops, "fsync"),
                &targets,
                Platform::Apple,
                persistence,
            ),
            &format!("plain fsync on macOS ({persistence:?})"),
        );
    }
    if durable {
        for persistence in [Persistence::Ordered, Persistence::Weak] {
            caught(
                check(
                    &world,
                    &without_directory_sync(&ops),
                    &targets,
                    platform,
                    persistence,
                ),
                &format!("no directory sync after a durable commit ({persistence:?})"),
            );
        }
        // A new directory whose parent is not synced may be lost with the durable file in it,
        // unless the journal keeps the entries in order.
        let unsynced_parent = without_parent_sync(&ops);
        assert!(unsynced_parent.len() < ops.len());
        check(
            &world,
            &unsynced_parent,
            &targets,
            platform,
            Persistence::Ordered,
        )
        .unwrap();
        caught(
            check(
                &world,
                &unsynced_parent,
                &targets,
                platform,
                Persistence::Weak,
            ),
            "a new directory without the sync of its parent (Weak)",
        );
    }
    let _ = fs::remove_dir_all(dir);
}

/// Audit finding: when the sync of the parent after `create_directory` fails, the new directory
/// may be lost after power loss with every file in it, so a later durable commit into it does
/// not survive by its own syncs. The test hook makes that sync fail (`once`): the durable commit
/// must sync the parent again before it returns, and the guarantees hold in every crash state of
/// the weak model. When the parent cannot be synced at all (`always`), the commit reports kind
/// DurabilityUnconfirmed instead of success.
#[cfg(unix)]
#[test]
fn a_durable_commit_confirms_a_new_directory_whose_parent_sync_failed() {
    use unio_runtime::externs::durability::FAIL_PARENT_SYNC;
    let (world, ops, targets, dir) = traced_run_with("parent-once", &[(FAIL_PARENT_SYNC, "once")]);
    let durable = targets[1].commits[0].1;
    let platform = Platform::host();
    if durable {
        // The sync after the mkdir failed, so the trace has none right after it.
        assert_eq!(without_parent_sync(&ops).len(), ops.len(), "{ops:#?}");
        for persistence in [Persistence::Ordered, Persistence::Weak] {
            check(&world, &ops, &targets, platform, persistence)
                .unwrap_or_else(|violation| panic!("{violation}\ntrace: {ops:#?}"));
        }
    }
    let _ = fs::remove_dir_all(dir);

    let dir = package("parent-always", "");
    let data = dir.join("data");
    let made = data.join("made");
    let program = format!(
        "import std.files
from std.files import IoErrorKind

public function main():
    try files.create_directory(Path(\"{made}\")) on IoError as e:
        print(\"error {{e.reason}}\")
        return
    try files.write_text(Path(\"{target}\"), \"inside\", durable = true) on IoError as e:
        if e.kind == IoErrorKind.DurabilityUnconfirmed:
            print(\"unconfirmed {{e.reason}}\")
        else:
            print(\"error {{e.reason}}\")
        return
    print(\"confirmed\")
",
        made = made.to_string_lossy(),
        target = made.join("c.txt").to_string_lossy(),
    );
    fs::write(dir.join("main.unio"), program).unwrap();
    let output = command(&dir)
        .env(FAIL_PARENT_SYNC, "always")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if durable_here(&data) {
        assert!(
            stdout.starts_with("unconfirmed") && stdout.contains("is not confirmed"),
            "stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        // The target is replaced all the same (guarantees 1 and 2).
        assert_eq!(fs::read_to_string(made.join("c.txt")).unwrap(), "inside");
    }
    let _ = fs::remove_dir_all(dir);
}

/// Windows has no writer process and no sync of a directory: the code path of externs/files.rs
/// (FlushFileBuffers, MoveFileExW with MOVEFILE_WRITE_THROUGH) is checked on the trace of a run,
/// changed into the calls that path makes, under the assumption that NTFS keeps its log in
/// order and a write-through rename returns once it and every entry before it are on storage.
#[cfg(unix)]
#[test]
fn the_windows_protocol_keeps_the_guarantees_under_its_assumptions() {
    let (world, ops, targets, dir) = traced_run("windows");
    let durable = targets[1].commits[0].1;
    for persistence in [Persistence::Ordered, Persistence::Weak] {
        check(
            &world,
            &as_on_windows(&ops, true),
            &targets,
            Platform::Windows,
            persistence,
        )
        .unwrap();
        caught(
            check(
                &world,
                &without_data_sync(&as_on_windows(&ops, true)),
                &targets,
                Platform::Windows,
                persistence,
            ),
            &format!("Windows without FlushFileBuffers ({persistence:?})"),
        );
        if durable {
            caught(
                check(
                    &world,
                    &as_on_windows(&ops, false),
                    &targets,
                    Platform::Windows,
                    persistence,
                ),
                &format!("Windows without MOVEFILE_WRITE_THROUGH ({persistence:?})"),
            );
        }
    }
    let _ = fs::remove_dir_all(dir);
}

/// A hand-written trace of the textbook mistake, for the model alone: the target is written in
/// place, so a crash can leave part of the new content.
#[test]
fn the_model_catches_a_write_in_place() {
    let root = "/trial".to_string();
    let target = format!("{root}/a.txt");
    let world = World {
        root: root.clone(),
        names: BTreeMap::new(),
        contents: Vec::new(),
    };
    let ops = vec![
        Op::Create(target.clone()),
        Op::Open(1, target.clone()),
        Op::Write(1, b"new ".to_vec()),
        Op::Write(1, b"content".to_vec()),
        Op::Sync(1, "fsync".into()),
        Op::Close(1),
    ];
    // Without a rename there is no commit; a reader may see the file with part of the content.
    let targets = [Target {
        path: target.clone(),
        commits: vec![],
    }];
    caught(
        check(
            &world,
            &ops,
            &targets,
            Platform::Linux,
            Persistence::Ordered,
        ),
        "a write in place",
    );
    // The same data through a temporary file and a rename keeps the guarantees.
    let temporary = format!("{root}/.a.txt.unio-tmp-1");
    let ops = vec![
        Op::Create(temporary.clone()),
        Op::Open(1, temporary.clone()),
        Op::Write(1, b"new ".to_vec()),
        Op::Write(1, b"content".to_vec()),
        Op::Sync(1, "fsync".into()),
        Op::Close(1),
        Op::Rename(temporary, target.clone()),
        Op::SyncDir("fsync".into(), root.clone()),
    ];
    let targets = [Target {
        path: target,
        commits: vec![(b"new content".to_vec(), true)],
    }];
    for persistence in [Persistence::Ordered, Persistence::Weak] {
        check(&world, &ops, &targets, Platform::Linux, persistence).unwrap();
    }
}

// ---------------------------------------------------------------- killed processes

/// A small pseudo-random generator; the seed is printed, and `UNIO_TRIAL_SEED` repeats a run.
struct Random(u64);

impl Random {
    fn new() -> Random {
        let seed = std::env::var("UNIO_TRIAL_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64
            });
        eprintln!("UNIO_TRIAL_SEED={seed}");
        Random(seed)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) % n
    }
}

/// A program that replaces `data/out.txt` in a loop until a write fails; content `i` is
/// `begin i`, a line of a size that depends on `i`, and `end i`.
fn loop_program(durable: bool) -> String {
    format!(
        "import std.files

function content(i as Int) returns Text:
    return \"begin {{i}}\\n\" + \"x\".repeat(1000 + (i * 7919) % 300000) + \"\\nend {{i}}\\n\"

public function main():
    var i = 0
    while true:
        try files.write_text(Path(\"data/out.txt\"), content(i), durable = {durable}) on IoError as e:
            print(\"stopped at {{i}}: {{e.reason}}\")
            return
        i = i + 1
"
    )
}

/// The number of the complete content the file holds, or why it is not one.
fn complete_content(data: &[u8]) -> Result<u64, String> {
    let text = std::str::from_utf8(data).map_err(|_| "not UTF-8".to_string())?;
    let rest = text
        .strip_prefix("begin ")
        .ok_or_else(|| format!("no beginning: {}", shown(data)))?;
    let (number, rest) = rest.split_once('\n').ok_or("no first line")?;
    let i: u64 = number
        .parse()
        .map_err(|_| format!("a bad number {number:?}"))?;
    let middle = rest
        .strip_suffix(&format!("\nend {i}\n"))
        .ok_or_else(|| format!("content {i} has no end ({} bytes)", data.len()))?;
    let size = 1000 + (i * 7919) % 300000;
    if middle.len() as u64 != size || middle.bytes().any(|b| b != b'x') {
        return Err(format!(
            "content {i} is torn: {} bytes of {size}",
            middle.len()
        ));
    }
    Ok(i)
}

/// The check after a kill tells a complete content from a torn one.
#[test]
fn the_check_after_a_kill_rejects_torn_contents() {
    let whole = |i: u64| {
        let size = (1000 + (i * 7919) % 300000) as usize;
        format!("begin {i}\n{}\nend {i}\n", "x".repeat(size)).into_bytes()
    };
    assert_eq!(complete_content(&whole(7)), Ok(7));
    let data = whole(7);
    assert!(complete_content(&data[..data.len() - 1]).is_err());
    assert!(complete_content(&data[..data.len() / 2]).is_err());
    assert!(complete_content(&[]).is_err());
    let mut zeros = data.clone();
    zeros[100] = 0;
    assert!(complete_content(&zeros).is_err());
    let mut mixed = whole(7);
    mixed.truncate(20);
    mixed.extend_from_slice(&whole(8)[20..]);
    assert!(complete_content(&mixed).is_err());
}

/// The target holds one complete content, and only temporary files of it are beside it.
fn check_after_kill(dir: &Path) -> (u64, usize) {
    let data = dir.join("data");
    let content = fs::read(data.join("out.txt")).unwrap();
    let number = complete_content(&content).unwrap_or_else(|e| panic!("after the kill: {e}"));
    let mut left = 0;
    for entry in fs::read_dir(&data).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "out.txt" {
            continue;
        }
        assert!(
            name.starts_with(".out.txt.unio-tmp-"),
            "an unexpected file after the kill: {name}"
        );
        left += 1;
        // A writer that outlives its program by a moment may still rename it.
        let _ = fs::remove_file(entry.path());
    }
    (number, left)
}

fn wait_for_first_write(dir: &Path, child: &mut std::process::Child) {
    let started = std::time::Instant::now();
    while !dir.join("data/out.txt").exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the program ended before its first write: {status}");
        }
        assert!(
            started.elapsed().as_secs() < 60,
            "the program did not write"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn durable_choices(dir: &Path) -> Vec<bool> {
    if durable_here(&dir.join("data")) {
        vec![false, true]
    } else {
        vec![false]
    }
}

#[test]
fn killing_the_program_during_writes_keeps_one_complete_content() {
    let mut random = Random::new();
    let dir = package("kill-program", "");
    let mut in_progress = 0;
    for durable in durable_choices(&dir) {
        fs::write(dir.join("main.unio"), loop_program(durable)).unwrap();
        for trial in 0..12 {
            let _ = fs::remove_file(dir.join("data/out.txt"));
            let mut child = command(&dir).spawn().unwrap();
            wait_for_first_write(&dir, &mut child);
            std::thread::sleep(std::time::Duration::from_millis(random.below(300)));
            child.kill().unwrap();
            child.wait().unwrap();
            let (number, left) = check_after_kill(&dir);
            eprintln!(
                "durable {durable}, trial {trial}: content {number}, {left} temporary files left"
            );
            if number > 0 {
                in_progress += 1;
            }
        }
    }
    assert!(in_progress > 0, "no kill landed after the first commit");
    let _ = fs::remove_dir_all(dir);
}

/// The processes whose parent is `parent` and whose environment starts them as a writer.
#[cfg(target_os = "linux")]
fn writers_of(parent: u32) -> Vec<i32> {
    fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let stat = fs::read_to_string(e.path().join("stat")).ok()?;
            let ppid = stat.rsplit_once(") ")?.1.split(' ').nth(1)?.to_string();
            let environ = fs::read(e.path().join("environ")).ok()?;
            (ppid == parent.to_string()
                && String::from_utf8_lossy(&environ).contains("UNIO_SANDBOX_WRITER"))
            .then(|| e.file_name().to_string_lossy().parse().ok())?
        })
        .collect()
}

/// The writer process alone is killed at random points: the program's next write fails, and
/// the target keeps one complete content.
#[cfg(target_os = "linux")]
#[test]
fn killing_the_writer_during_writes_keeps_one_complete_content() {
    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let mut random = Random::new();
    let dir = package("kill-writer", "");
    for durable in durable_choices(&dir) {
        fs::write(dir.join("main.unio"), loop_program(durable)).unwrap();
        for trial in 0..8 {
            let _ = fs::remove_file(dir.join("data/out.txt"));
            let mut child = command(&dir)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            wait_for_first_write(&dir, &mut child);
            let writers = writers_of(child.id());
            assert_eq!(writers.len(), 1, "one writer runs: {writers:?}");
            std::thread::sleep(std::time::Duration::from_millis(random.below(300)));
            // SAFETY: a plain system call with integer arguments.
            assert_eq!(unsafe { kill(writers[0], 9) }, 0);
            let output = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("stopped at"),
                "the program did not report the failed write: {stdout}"
            );
            let (number, left) = check_after_kill(&dir);
            eprintln!(
                "durable {durable}, trial {trial}: content {number}, {left} temporary files left"
            );
        }
    }
    let _ = fs::remove_dir_all(dir);
}
