//! A power cut on real file systems: block writes recorded and replayed (S-9b, C-19;
//! docs/runtime.md 10.5).
//!
//! `tests/power_loss.rs` replays the system calls of a run on a model of a file system. This
//! trial records the blocks that the kernel's ext4 and XFS write to a device while Unio programs
//! write atomic files through `std.files` and the writer process, and replays them:
//!
//! 1. A file system is made on an image (`base.img`) with the old content of every target.
//! 2. The image is put behind a recorder and mounted. The recorder is either the device-mapper
//!    target `log-writes` of the kernel (`UNIO_POWER_CUT=dm-log-writes`, as xfstests uses it),
//!    or, where the kernel has no device mapper, a loop device over a file served by a small
//!    FUSE server in this test (`UNIO_POWER_CUT=fuse`), which sees every write and every flush
//!    the loop device passes to its file. Both write the same log, in the on-disk format of
//!    `drivers/md/dm-log-writes.c`: each write with its sector and data, each flush and FUA,
//!    and marks. The harness adds a mark each time the program reports that a commit with
//!    `durable = true` has returned.
//! 3. A Unio program replaces files with `write_text` and `open_atomic`, with and without
//!    `durable`, writes in parts, leaves an atomic file without a commit, and writes into a
//!    directory it has just created; the harness then writes one file in place, the protocol
//!    that S-9 forbids, as a negative control.
//! 4. The log is read back by the replay program of this file (the reader of
//!    `read_log`, which follows `src/log-writes/log-writes.c` of xfstests) and applied to a
//!    copy of the base image one entry at a time. After every entry (so at every flush and FUA
//!    and at every point between them) a copy of the image is mounted, which runs the journal
//!    recovery of the file system as after a power cut, and checked: each target holds its old
//!    content or the complete content of one commit, never a torn or empty one; after the mark
//!    of a durable commit it holds that content or a later one; nothing but the targets, the
//!    files that were there, the new directory, and temporary files `.<name>.unio-tmp-*` is in
//!    the directory. The written-in-place file must be seen torn at some point, or the trial
//!    has not shown that it can see a torn write. The log replayed in full must give the device
//!    the run left, so that no write went unrecorded.
//!
//! The trial needs root, loop devices, `mount`, and `mkfs.ext4` and `mkfs.xfs`; it is
//! `#[ignore]`d and run by the job `power-cut` of `.github/workflows/ci.yml` with
//! `UNIO_POWER_CUT` set. A replayed prefix is one state that storage may hold after a power
//! cut; storage may also keep any subset of the writes after the last flush, which a prefix
//! does not cover, and storage that acknowledges writes it has not stored is beyond any
//! software check (S-9a).

#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// Selects the recorder: `dm-log-writes` or `fuse`.
const ENV: &str = "UNIO_POWER_CUT";
/// The file systems to trial, separated by commas (default `ext4,xfs`).
const ENV_FS: &str = "UNIO_POWER_CUT_FS";
/// The directory for the images (default `/dev/shm`, else the temporary directory).
const ENV_DIR: &str = "UNIO_POWER_CUT_DIR";

// ---------------------------------------------------------------- the log of dm-log-writes

/// The on-disk format of `drivers/md/dm-log-writes.c`: a super block in the first sector,
/// then for each entry a sector with its header (and the text of a mark), then its data.
const MAGIC: u64 = 0x006a_7366_7773_6872;
const VERSION: u64 = 1;
const FLUSH: u64 = 1 << 0;
const FUA: u64 = 1 << 1;
const DISCARD: u64 = 1 << 2;
const MARK: u64 = 1 << 3;
/// The sector size of the logs this harness writes (that of a loop device).
const SECTOR: u64 = 512;

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// One entry of the log.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    /// The byte offset on the device.
    offset: u64,
    flags: u64,
    /// The data of a write; for a discard, its length in zeros.
    data: Vec<u8>,
    discard: u64,
    mark: Option<String>,
}

/// Writes a log in the format of dm-log-writes (the FUSE recorder, and the test of the reader).
struct LogWriter {
    file: File,
    position: u64,
    entries: u64,
}

impl LogWriter {
    fn create(path: &Path) -> LogWriter {
        LogWriter {
            file: File::create(path).unwrap(),
            position: SECTOR,
            entries: 0,
        }
    }

    fn entry(&mut self, offset: u64, flags: u64, data: &[u8], mark: &[u8]) {
        assert!(offset.is_multiple_of(SECTOR) && (data.len() as u64).is_multiple_of(SECTOR));
        assert!(32 + mark.len() <= SECTOR as usize);
        let mut header = vec![0u8; SECTOR as usize];
        header[0..8].copy_from_slice(&(offset / SECTOR).to_le_bytes());
        header[8..16].copy_from_slice(&(data.len() as u64 / SECTOR).to_le_bytes());
        header[16..24].copy_from_slice(&flags.to_le_bytes());
        header[24..32].copy_from_slice(&(mark.len() as u64).to_le_bytes());
        header[32..32 + mark.len()].copy_from_slice(mark);
        self.file.write_all_at(&header, self.position).unwrap();
        self.position += SECTOR;
        self.file.write_all_at(data, self.position).unwrap();
        self.position += data.len() as u64;
        self.entries += 1;
    }

    fn write(&mut self, offset: u64, data: &[u8]) {
        self.entry(offset, 0, data, b"");
    }

    fn flush(&mut self) {
        self.entry(0, FLUSH, &[], b"");
    }

    fn mark(&mut self, text: &str) {
        self.entry(0, MARK, &[], text.as_bytes());
    }

    fn finish(&mut self) {
        let mut sup = vec![0u8; SECTOR as usize];
        sup[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        sup[8..16].copy_from_slice(&VERSION.to_le_bytes());
        sup[16..24].copy_from_slice(&self.entries.to_le_bytes());
        sup[24..28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
        self.file.write_all_at(&sup, 0).unwrap();
        self.file.sync_all().unwrap();
    }
}

/// Reads a log of dm-log-writes, as `log_replay_next_entry` of xfstests does.
fn read_log(path: &Path) -> Result<Vec<Entry>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut sup = [0u8; 28];
    file.read_exact_at(&mut sup, 0).map_err(|e| e.to_string())?;
    if le64(&sup, 0) != MAGIC {
        return Err(format!(
            "not a log of dm-log-writes: magic {:#x}",
            le64(&sup, 0)
        ));
    }
    if le64(&sup, 8) != VERSION {
        return Err(format!("version {} of the log", le64(&sup, 8)));
    }
    let count = le64(&sup, 16);
    let sector = u64::from(le32(&sup, 24));
    if sector < 512 || !sector.is_power_of_two() {
        return Err(format!("a sector size of {sector}"));
    }
    let mut position = sector;
    let mut entries = Vec::new();
    let mut header = vec![0u8; sector as usize];
    for i in 0..count {
        file.read_exact_at(&mut header, position)
            .map_err(|e| format!("entry {i}: {e}"))?;
        position += sector;
        let flags = le64(&header, 16);
        let sectors = le64(&header, 8);
        let mut entry = Entry {
            offset: le64(&header, 0) * sector,
            flags,
            data: Vec::new(),
            discard: 0,
            mark: None,
        };
        if flags & MARK != 0 {
            let length = le64(&header, 24) as usize;
            if 32 + length > sector as usize {
                return Err(format!("entry {i}: a mark of {length} bytes"));
            }
            entry.mark = Some(String::from_utf8_lossy(&header[32..32 + length]).into_owned());
        } else if flags & DISCARD != 0 {
            entry.discard = sectors * sector;
        } else if le64(&header, 24) > 0 {
            // Inline data after the header (`write_inline_data`, the DAX writes of kernels
            // before 5.19): it starts in the header's sector and fills whole sectors.
            let length = le64(&header, 24);
            let mut bytes = vec![0u8; (32 + length).div_ceil(sector) as usize * sector as usize];
            file.read_exact_at(&mut bytes, position - sector)
                .map_err(|e| format!("entry {i}: {e}"))?;
            entry.data = bytes[32..32 + length as usize].to_vec();
            position += bytes.len() as u64 - sector;
        } else {
            entry.data = vec![0u8; (sectors * sector) as usize];
            file.read_exact_at(&mut entry.data, position)
                .map_err(|e| format!("entry {i}: {e}"))?;
            position += sectors * sector;
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// Applies one entry to an image.
fn apply(image: &File, entry: &Entry) {
    if entry.discard > 0 {
        image
            .write_all_at(&vec![0u8; entry.discard as usize], entry.offset)
            .unwrap();
    } else if !entry.data.is_empty() {
        image.write_all_at(&entry.data, entry.offset).unwrap();
    }
}

/// The log reader gives back what the writer wrote.
#[test]
fn the_log_reader_reads_the_format_of_dm_log_writes() {
    let dir = std::env::temp_dir().join(format!("unio-power-cut-log-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("log");
    let mut log = LogWriter::create(&path);
    log.mark("start");
    log.write(4096, &[7u8; 1024]);
    log.flush();
    log.entry(8192, FUA, &[9u8; 512], b"");
    log.entry(1024, DISCARD, &[], b"");
    log.mark("end");
    log.finish();
    let entries = read_log(&path).unwrap();
    let kinds: Vec<(u64, u64, usize, Option<&str>)> = entries
        .iter()
        .map(|e| (e.offset, e.flags, e.data.len(), e.mark.as_deref()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (0, MARK, 0, Some("start")),
            (4096, 0, 1024, None),
            (0, FLUSH, 0, None),
            (8192, FUA, 512, None),
            (1024, DISCARD, 0, None),
            (0, MARK, 0, Some("end")),
        ]
    );
    assert!(entries[1].data.iter().all(|&b| b == 7));
    // A discard of no length and a log that is not one.
    fs::write(dir.join("not-a-log"), vec![0u8; 4096]).unwrap();
    assert!(read_log(&dir.join("not-a-log")).is_err());
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------- system helpers

extern "C" {
    fn geteuid() -> u32;
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    fn mount(
        source: *const std::ffi::c_char,
        target: *const std::ffi::c_char,
        fstype: *const std::ffi::c_char,
        flags: std::ffi::c_ulong,
        data: *const std::ffi::c_void,
    ) -> i32;
    fn umount2(target: *const std::ffi::c_char, flags: i32) -> i32;
}

const SEEK_DATA: i32 = 3;
const SEEK_HOLE: i32 = 4;

/// Runs a program and returns its standard output; a failure fails the trial.
fn run(program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(
        output.status.success(),
        "{program} {args:?}: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Unmounts a file system, trying again while it is busy: an XFS state just checked can stay
/// busy for a moment (umount gave status 32 on the runner, run 43456804573). A mount that does
/// not go away fails the trial with the last error.
fn unmount(point: &Path) {
    let mut last = String::new();
    for _ in 0..50 {
        let _ = Command::new("sync").status();
        let output = Command::new("umount").arg(point).output().unwrap();
        if output.status.success() {
            return;
        }
        last = format!(
            "{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("umount {}: {last}", point.display());
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Copies an image and keeps its holes.
fn sparse_copy(from: &Path, to: &Path) {
    let source = File::open(from).unwrap();
    let size = source.metadata().unwrap().len();
    let target = File::create(to).unwrap();
    target.set_len(size).unwrap();
    let fd = source.as_raw_fd();
    let mut buffer = vec![0u8; 1 << 20];
    let mut position = 0u64;
    while position < size {
        // SAFETY: plain system calls on a descriptor this function holds.
        let data = unsafe { lseek(fd, position as i64, SEEK_DATA) };
        if data < 0 {
            break;
        }
        let hole = unsafe { lseek(fd, data, SEEK_HOLE) } as u64;
        let mut at = data as u64;
        while at < hole {
            let n = ((hole - at) as usize).min(buffer.len());
            source.read_exact_at(&mut buffer[..n], at).unwrap();
            target.write_all_at(&buffer[..n], at).unwrap();
            at += n as u64;
        }
        position = hole;
    }
}

/// Whether two images hold the same bytes.
fn same_images(a: &Path, b: &Path) -> Result<(), String> {
    let (a, b) = (File::open(a).unwrap(), File::open(b).unwrap());
    let size = a.metadata().unwrap().len();
    if size != b.metadata().unwrap().len() {
        return Err("the sizes differ".into());
    }
    let (mut x, mut y) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    let mut at = 0u64;
    while at < size {
        let n = ((size - at) as usize).min(x.len());
        a.read_exact_at(&mut x[..n], at).unwrap();
        b.read_exact_at(&mut y[..n], at).unwrap();
        if x[..n] != y[..n] {
            let i = (0..n).find(|&i| x[i] != y[i]).unwrap();
            return Err(format!("they differ at byte {}", at + i as u64));
        }
        at += n as u64;
    }
    Ok(())
}

fn c_text(text: &str) -> std::ffi::CString {
    std::ffi::CString::new(text).unwrap()
}

// ---------------------------------------------------------------- the FUSE recorder

/// A FUSE server for one file, `disk`, whose content is a file of the test. Every write and
/// every fsync the kernel sends is applied and recorded in the log before it is answered, so
/// a write that has completed is in the log before any flush that may depend on it. Without a
/// write-back cache, FUSE sends each write of the loop device through at once; the loop device
/// turns a flush into an fsync of its file, and a FUA write into a write and a flush.
mod fuse {
    use super::*;

    const LOOKUP: u32 = 1;
    const FORGET: u32 = 2;
    const GETATTR: u32 = 3;
    const SETATTR: u32 = 4;
    const OPEN: u32 = 14;
    const READ: u32 = 15;
    const WRITE: u32 = 16;
    const STATFS: u32 = 17;
    const RELEASE: u32 = 18;
    const FSYNC: u32 = 20;
    const FLUSH: u32 = 25;
    const INIT: u32 = 26;
    const OPENDIR: u32 = 27;
    const READDIR: u32 = 28;
    const RELEASEDIR: u32 = 29;
    const FSYNCDIR: u32 = 30;
    const ACCESS: u32 = 34;
    const INTERRUPT: u32 = 36;
    const BATCH_FORGET: u32 = 42;
    const BIG_WRITES: u32 = 1 << 5;
    const MAX_WRITE: u32 = 128 * 1024;
    const ENOENT: i32 = 2;
    const ENOSYS: i32 = 38;
    const ROOT: u64 = 1;
    const DISK: u64 = 2;

    fn attr(node: u64, size: u64) -> Vec<u8> {
        let mut a = Vec::with_capacity(88);
        a.extend(node.to_le_bytes());
        a.extend(size.to_le_bytes());
        a.extend(size.div_ceil(512).to_le_bytes());
        a.extend([0u8; 24]); // atime, mtime, ctime
        a.extend([0u8; 12]); // their nanoseconds
        let (mode, links) = if node == ROOT {
            (0o040755u32, 2u32)
        } else {
            (0o100600, 1)
        };
        for v in [mode, links, 0, 0, 0, 4096, 0] {
            a.extend(v.to_le_bytes());
        }
        a
    }

    fn attr_out(node: u64, size: u64) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(3600u64.to_le_bytes());
        out.extend([0u8; 8]);
        out.extend(attr(node, size));
        out
    }

    pub struct Recorder {
        pub mount: PathBuf,
        pub disk: PathBuf,
        pub log: Arc<Mutex<LogWriter>>,
        server: Option<std::thread::JoinHandle<()>>,
    }

    impl Recorder {
        pub fn start(disk: &Path, log: &Path, mount_point: &Path) -> Recorder {
            fs::create_dir_all(mount_point).unwrap();
            let device = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/fuse")
                .unwrap_or_else(|e| panic!("/dev/fuse: {e}"));
            let options = c_text(&format!(
                "fd={},rootmode=40000,user_id=0,group_id=0",
                device.as_raw_fd()
            ));
            let (source, target, kind) = (
                c_text("unio-recorder"),
                c_text(text(mount_point)),
                c_text("fuse"),
            );
            // SAFETY: valid C strings; MS_NOSUID | MS_NODEV.
            let result = unsafe {
                mount(
                    source.as_ptr(),
                    target.as_ptr(),
                    kind.as_ptr(),
                    2 | 4,
                    options.as_ptr().cast(),
                )
            };
            assert_eq!(
                result,
                0,
                "mounting the FUSE recorder: {}",
                std::io::Error::last_os_error()
            );
            let log = Arc::new(Mutex::new(LogWriter::create(log)));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(disk)
                .unwrap();
            let shared = Arc::clone(&log);
            let server = std::thread::spawn(move || serve(device, file, shared));
            Recorder {
                mount: mount_point.to_path_buf(),
                disk: mount_point.join("disk"),
                log,
                server: Some(server),
            }
        }

        pub fn stop(&mut self) {
            let target = c_text(text(&self.mount));
            // SAFETY: a valid C string; MNT_DETACH, so that the server answers until the loop
            // device has let its file go.
            unsafe { umount2(target.as_ptr(), 2) };
            if let Some(server) = self.server.take() {
                server.join().unwrap();
            }
            self.log.lock().unwrap().finish();
        }
    }

    fn serve(mut device: File, disk: File, log: Arc<Mutex<LogWriter>>) {
        let mut buffer = vec![0u8; (MAX_WRITE as usize) + (1 << 16)];
        loop {
            let n = match device.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => match e.raw_os_error() {
                    // An interrupted request, or a signal.
                    Some(2) | Some(4) | Some(11) => continue,
                    // The file system is gone.
                    Some(19) => return,
                    _ => panic!("reading /dev/fuse: {e}"),
                },
            };
            let request = &buffer[..n];
            let opcode = le32(request, 4);
            let unique = le64(request, 8);
            let node = le64(request, 16);
            let body = &request[40..];
            let size = disk.metadata().unwrap().len();
            let reply: Option<Result<Vec<u8>, i32>> = match opcode {
                INIT => {
                    let minor = le32(body, 4).min(31);
                    let mut out = Vec::new();
                    for v in [7u32, minor, le32(body, 8), BIG_WRITES] {
                        out.extend(v.to_le_bytes());
                    }
                    out.extend(16u16.to_le_bytes());
                    out.extend(12u16.to_le_bytes());
                    out.extend(MAX_WRITE.to_le_bytes());
                    out.extend(1u32.to_le_bytes());
                    out.resize(64, 0);
                    Some(Ok(out))
                }
                LOOKUP => {
                    let name = body.split(|&b| b == 0).next().unwrap();
                    if node == ROOT && name == b"disk" {
                        let mut out = Vec::new();
                        out.extend(DISK.to_le_bytes());
                        out.extend(0u64.to_le_bytes());
                        out.extend(3600u64.to_le_bytes());
                        out.extend(3600u64.to_le_bytes());
                        out.extend([0u8; 8]);
                        out.extend(attr(DISK, size));
                        Some(Ok(out))
                    } else {
                        Some(Err(ENOENT))
                    }
                }
                GETATTR | SETATTR => Some(Ok(attr_out(node, size))),
                OPEN | OPENDIR => {
                    let mut out = 1u64.to_le_bytes().to_vec();
                    out.extend([0u8; 8]);
                    Some(Ok(out))
                }
                READ => {
                    let (offset, length) = (le64(body, 8), le32(body, 16) as u64);
                    let end = (offset + length).min(size).max(offset);
                    let mut data = vec![0u8; (end - offset) as usize];
                    disk.read_exact_at(&mut data, offset).unwrap();
                    Some(Ok(data))
                }
                WRITE => {
                    let (offset, length) = (le64(body, 8), le32(body, 16) as usize);
                    let data = &body[40..40 + length];
                    disk.write_all_at(data, offset).unwrap();
                    log.lock().unwrap().write(offset, data);
                    let mut out = (length as u32).to_le_bytes().to_vec();
                    out.extend([0u8; 4]);
                    Some(Ok(out))
                }
                FSYNC => {
                    log.lock().unwrap().flush();
                    Some(Ok(Vec::new()))
                }
                STATFS => {
                    let mut out = Vec::new();
                    for v in [size / 4096, 0, 0, 2, 0] {
                        out.extend(v.to_le_bytes());
                    }
                    for v in [4096u32, 255, 4096, 0] {
                        out.extend(v.to_le_bytes());
                    }
                    out.resize(80, 0);
                    Some(Ok(out))
                }
                READDIR => Some(Ok(Vec::new())),
                RELEASE | RELEASEDIR | FLUSH | FSYNCDIR | ACCESS => Some(Ok(Vec::new())),
                FORGET | BATCH_FORGET | INTERRUPT => None,
                _ => Some(Err(ENOSYS)),
            };
            if let Some(reply) = reply {
                let (error, payload) = match reply {
                    Ok(p) => (0i32, p),
                    Err(e) => (-e, Vec::new()),
                };
                let mut out = Vec::with_capacity(16 + payload.len());
                out.extend(((16 + payload.len()) as u32).to_le_bytes());
                out.extend(error.to_le_bytes());
                out.extend(unique.to_le_bytes());
                out.extend(payload);
                // A request interrupted meanwhile is answered with ENOENT; nothing to do.
                let _ = device.write(&out);
            }
        }
    }
}

// ---------------------------------------------------------------- the recorders

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    DmLogWrites,
    Fuse,
}

enum Recorder {
    Dm {
        name: String,
        data_loop: String,
        log_loop: String,
        data: PathBuf,
        log: PathBuf,
    },
    Fuse {
        server: fuse::Recorder,
        loop_device: String,
        data: PathBuf,
        log: PathBuf,
    },
}

impl Recorder {
    /// Puts a copy of the base image behind the recorder.
    fn start(kind: Kind, base: &Path, work: &Path) -> Recorder {
        let data = work.join("device.img");
        let log = work.join("log.img");
        sparse_copy(base, &data);
        match kind {
            Kind::DmLogWrites => {
                File::create(&log).unwrap().set_len(4 << 30).unwrap();
                let data_loop = run("losetup", &["--find", "--show", text(&data)])
                    .trim()
                    .to_string();
                let log_loop = run("losetup", &["--find", "--show", text(&log)])
                    .trim()
                    .to_string();
                let sectors = run("blockdev", &["--getsz", &data_loop]).trim().to_string();
                let name = format!("unio-power-cut-{}", std::process::id());
                let table = format!("0 {sectors} log-writes {data_loop} {log_loop}");
                run("dmsetup", &["create", &name, "--table", &table]);
                Recorder::Dm {
                    name,
                    data_loop,
                    log_loop,
                    data,
                    log,
                }
            }
            Kind::Fuse => {
                let server = fuse::Recorder::start(&data, &log, &work.join("fuse"));
                let loop_device = run("losetup", &["--find", "--show", text(&server.disk)])
                    .trim()
                    .to_string();
                Recorder::Fuse {
                    server,
                    loop_device,
                    data,
                    log,
                }
            }
        }
    }

    /// The block device to make the file system on.
    fn device(&self) -> String {
        match self {
            Recorder::Dm { name, .. } => format!("/dev/mapper/{name}"),
            Recorder::Fuse { loop_device, .. } => loop_device.clone(),
        }
    }

    fn mark(&self, text: &str) {
        match self {
            Recorder::Dm { name, .. } => {
                run("dmsetup", &["message", name, "0", "mark", text]);
            }
            Recorder::Fuse { server, .. } => server.log.lock().unwrap().mark(text),
        }
    }

    /// Stops recording: the log, and the image the device holds at the end.
    fn finish(self) -> (PathBuf, PathBuf) {
        match self {
            Recorder::Dm {
                name,
                data_loop,
                log_loop,
                data,
                log,
            } => {
                run("dmsetup", &["remove", &name]);
                run("losetup", &["--detach", &data_loop]);
                run("losetup", &["--detach", &log_loop]);
                (log, data)
            }
            Recorder::Fuse {
                mut server,
                loop_device,
                data,
                log,
            } => {
                run("losetup", &["--detach", &loop_device]);
                server.stop();
                (log, data)
            }
        }
    }
}

// ---------------------------------------------------------------- the program and its check

/// The rounds in which `a.txt` is replaced; even rounds are durable.
const ROUNDS: u64 = 6;
/// The round of `b.txt`, written in parts through `open_atomic`: about 290 KB.
const B_ROUND: u64 = 37;

fn content_size(i: u64) -> usize {
    (1000 + (i * 7919) % 300000) as usize
}

/// The content number `i` of the file `name`, which tells its number and its completeness.
fn content(name: &str, i: u64) -> Vec<u8> {
    format!(
        "begin {name} {i}\n{}\nend {name} {i}\n",
        "x".repeat(content_size(i))
    )
    .into_bytes()
}

/// The number of the complete content a file holds, or why it is not one.
fn which_content(name: &str, data: &[u8]) -> Result<u64, String> {
    let prefix = format!("begin {name} ");
    let head = &data[..data.len().min(64)];
    let head = String::from_utf8_lossy(head);
    let number = head
        .strip_prefix(&prefix)
        .and_then(|rest| rest.split('\n').next())
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| format!("{} bytes without a beginning ({head:?})", data.len()))?;
    let expected = content(name, number);
    if data == expected.as_slice() {
        return Ok(number);
    }
    let first = (0..data.len().min(expected.len()))
        .find(|&i| data[i] != expected[i])
        .unwrap_or(data.len().min(expected.len()));
    Err(format!(
        "content {number} is torn: {} bytes of {}, the first difference at byte {first}",
        data.len(),
        expected.len()
    ))
}

#[test]
fn which_content_tells_complete_contents_from_torn_ones() {
    let whole = content("a", 3);
    assert_eq!(which_content("a", &whole), Ok(3));
    assert!(which_content("a", &whole[..whole.len() - 1]).is_err());
    assert!(which_content("a", &whole[..100]).is_err());
    assert!(which_content("a", b"").is_err());
    assert!(which_content("b", &whole).is_err());
    let mut zeros = whole.clone();
    zeros[5000] = 0;
    assert!(which_content("a", &zeros).is_err());
}

fn program() -> String {
    let mut steps = String::new();
    for i in 1..=ROUNDS {
        let durable = i % 2 == 0;
        steps += &format!(
            "    try files.write_text(Path(\"data/a.txt\"), content(\"a\", {i}), durable = {durable})\n"
        );
        if durable {
            steps += &format!("    print(\"durable a {i}\")\n");
        }
    }
    format!(
        "import std.files

function content(name as Text, i as Int) returns Text:
    return \"begin {{name}} {{i}}\\n\" + \"x\".repeat(1000 + (i * 7919) % 300000) + \"\\nend {{name}} {{i}}\\n\"

function run() fails with IoError:
{steps}    using file = try files.open_atomic(Path(\"data/b.txt\"), durable = true):
        try file.write(\"begin b {B_ROUND}\\n\")
        try file.write(\"x\".repeat({size}))
        try file.write(\"\\nend b {B_ROUND}\\n\")
        try file.commit()
    print(\"durable b {B_ROUND}\")
    using file = try files.open_atomic(Path(\"data/a.txt\")):
        try file.write(\"never committed\")
    try files.create_directory(Path(\"data/new\"))
    try files.write_text(Path(\"data/new/c.txt\"), content(\"c\", 1), durable = true)
    print(\"durable c 1\")

public function main():
    try run() on IoError as e:
        print(\"error {{e.reason}}\")
        return
    print(\"done\")
",
        size = content_size(B_ROUND),
    )
}

/// What the durable commits that have returned require: the least content of each file.
#[derive(Default, Clone)]
struct Required {
    a: u64,
    b: bool,
    c: bool,
}

impl Required {
    fn add(&mut self, mark: &str) {
        let words: Vec<&str> = mark.split('-').collect();
        match words.as_slice() {
            ["durable", "a", i] => self.a = i.parse().unwrap(),
            ["durable", "b", _] => self.b = true,
            ["durable", "c", _] => self.c = true,
            _ => {}
        }
    }
}

/// What one replayed state holds.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Seen {
    a: u64,
    b: bool,
    c: bool,
    /// The file written in place: its content, or why it is torn.
    in_place: Result<u64, String>,
}

fn is_temporary(name: &str, of: &str) -> bool {
    name.starts_with(&format!(".{of}.unio-tmp-"))
}

/// Checks the guarantees of S-9 in a mounted state.
fn check_state(root: &Path, required: &Required) -> Result<Seen, String> {
    let data = root.join("data");
    let names = |dir: &Path| -> Result<Vec<String>, String> {
        let mut found: Vec<String> = fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        Ok(found)
    };
    for name in names(&data)? {
        let known = ["a.txt", "b.txt", "neighbour.txt", "in-place.txt", "new"];
        if !known.contains(&name.as_str())
            && !is_temporary(&name, "a.txt")
            && !is_temporary(&name, "b.txt")
        {
            return Err(format!("an unexpected name in the directory: {name}"));
        }
    }
    let neighbour = fs::read(data.join("neighbour.txt")).map_err(|e| e.to_string())?;
    if neighbour != b"neighbour" {
        return Err(format!("neighbour.txt changed: {neighbour:?}"));
    }
    let a = fs::read(data.join("a.txt")).map_err(|e| format!("a.txt: {e}"))?;
    let a = which_content("a", &a).map_err(|e| format!("a.txt: {e}"))?;
    if a > ROUNDS {
        return Err(format!("a.txt holds content {a}, never written"));
    }
    if a < required.a {
        return Err(format!(
            "a.txt holds content {a} after the durable commit of content {} returned",
            required.a
        ));
    }
    let b = match fs::read(data.join("b.txt")) {
        Ok(bytes) => {
            let i = which_content("b", &bytes).map_err(|e| format!("b.txt: {e}"))?;
            if i != B_ROUND {
                return Err(format!("b.txt holds content {i}"));
            }
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(format!("b.txt: {e}")),
    };
    if required.b && !b {
        return Err("b.txt is missing after its durable commit returned".into());
    }
    let new = data.join("new");
    let c = if new.is_dir() {
        for name in names(&new)? {
            if name != "c.txt" && !is_temporary(&name, "c.txt") {
                return Err(format!("an unexpected name in new/: {name}"));
            }
        }
        match fs::read(new.join("c.txt")) {
            Ok(bytes) => {
                which_content("c", &bytes).map_err(|e| format!("new/c.txt: {e}"))?;
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(format!("new/c.txt: {e}")),
        }
    } else {
        false
    };
    if required.c && !c {
        return Err("new/c.txt is missing after its durable commit returned".into());
    }
    let in_place = fs::read(data.join("in-place.txt"))
        .map_err(|e| e.to_string())
        .and_then(|bytes| which_content("p", &bytes));
    Ok(Seen { a, b, c, in_place })
}

/// Makes the base image: a file system with the package and the old contents.
fn make_base(file_system: &str, base: &Path, mount_point: &Path) {
    let size: u64 = if file_system == "xfs" {
        300 << 20
    } else {
        64 << 20
    };
    File::create(base).unwrap().set_len(size).unwrap();
    match file_system {
        // No lazy initialisation, so that no kernel thread writes during the trial.
        "ext4" => run(
            "mkfs.ext4",
            &[
                "-q",
                "-F",
                "-E",
                "lazy_itable_init=0,lazy_journal_init=0",
                text(base),
            ],
        ),
        "xfs" => run("mkfs.xfs", &["-q", "-f", text(base)]),
        other => panic!("no trial for the file system {other}"),
    };
    fs::create_dir_all(mount_point).unwrap();
    run("mount", &["-o", "loop", text(base), text(mount_point)]);
    let data = mount_point.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        mount_point.join("unio.toml"),
        "[package]\nname = \"trial\"\n\n[permissions]\nfiles.read = [\"$CWD/data/**\"]\nfiles.write = [\"$CWD/data/**\"]\n",
    )
    .unwrap();
    fs::write(mount_point.join("main.unio"), program()).unwrap();
    fs::write(data.join("a.txt"), content("a", 0)).unwrap();
    fs::write(data.join("neighbour.txt"), "neighbour").unwrap();
    fs::write(data.join("in-place.txt"), content("p", 0)).unwrap();
    unmount(mount_point);
}

/// Runs the program on the mounted device and marks each durable commit it reports.
fn run_program(recorder: &Recorder, root: &Path) {
    assert!(
        unio_runtime::externs::durability::supports_durability(&root.join("data/a.txt")),
        "durable = true is not supported on {}",
        root.display()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(root)
        .env_remove("UNIO_POLICY")
        .args(["run", "main.unio"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = Vec::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        // The commit has returned: its flush is in the log, and the mark comes after it.
        // A message of dm-log-writes is split at spaces, so the mark has none.
        if line.starts_with("durable ") {
            recorder.mark(&line.replace(' ', "-"));
        }
        lines.push(line);
    }
    let status = child.wait().unwrap();
    assert!(
        status.success() && lines.last().map(String::as_str) == Some("done"),
        "the program failed ({status}): {lines:?}"
    );
}

/// The negative control: the file is written in place, in two parts, each synced.
fn write_in_place(root: &Path) {
    let whole = content("p", 1);
    let mut file = File::create(root.join("data/in-place.txt")).unwrap();
    let half = whole.len() / 2;
    file.write_all(&whole[..half]).unwrap();
    file.sync_all().unwrap();
    file.write_all(&whole[half..]).unwrap();
    file.sync_all().unwrap();
}

struct Summary {
    entries: usize,
    flushes: usize,
    states: usize,
    a_contents: BTreeSet<u64>,
    torn_in_place: usize,
}

/// Replays the log at every entry and checks each state.
fn replay(file_system: &str, base: &Path, log: &[Entry], work: &Path) -> Result<Summary, String> {
    let image = work.join("replay.img");
    let copy = work.join("state.img");
    let mount_point = work.join("state");
    fs::create_dir_all(&mount_point).unwrap();
    sparse_copy(base, &image);
    let file = OpenOptions::new().write(true).open(&image).unwrap();
    let mut required = Required::default();
    let mut summary = Summary {
        entries: log.len(),
        flushes: log.iter().filter(|e| e.flags & (FLUSH | FUA) != 0).count(),
        states: 0,
        a_contents: BTreeSet::new(),
        torn_in_place: 0,
    };
    for k in 0..=log.len() {
        if k > 0 {
            let entry = &log[k - 1];
            if let Some(mark) = &entry.mark {
                required.add(mark);
                continue;
            }
            apply(&file, entry);
            // Several writes in a row without a flush: every one of them is a state.
        }
        sparse_copy(&image, &copy);
        let mut options = vec!["-o".to_string(), "loop".to_string()];
        if file_system == "xfs" {
            options[1] += ",nouuid";
        }
        let mounted = Command::new("mount")
            .args(&options)
            .args(["-t", file_system, text(&copy), text(&mount_point)])
            .output()
            .unwrap();
        if !mounted.status.success() {
            return Err(format!(
                "the state after entry {k} of {} does not mount: {}",
                log.len(),
                String::from_utf8_lossy(&mounted.stderr)
            ));
        }
        let result = check_state(&mount_point, &required);
        unmount(&mount_point);
        let seen = result.map_err(|e| format!("after entry {k} of {}: {e}", log.len()))?;
        summary.states += 1;
        summary.a_contents.insert(seen.a);
        if seen.in_place.is_err() {
            summary.torn_in_place += 1;
        }
    }
    let _ = fs::remove_file(&copy);
    let _ = fs::remove_file(&image);
    Ok(summary)
}

fn work_dir(file_system: &str) -> PathBuf {
    let root = std::env::var_os(ENV_DIR)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if Path::new("/dev/shm").is_dir() {
                PathBuf::from("/dev/shm")
            } else {
                std::env::temp_dir()
            }
        });
    let dir = root.join(format!(
        "unio-power-cut-{}-{file_system}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn trial(kind: Kind, file_system: &str) {
    let work = work_dir(file_system);
    let base = work.join("base.img");
    let root = work.join("mnt");
    make_base(file_system, &base, &root);
    let recorder = Recorder::start(kind, &base, &work);
    recorder.mark("start");
    let device = recorder.device();
    run("mount", &["-t", file_system, &device, text(&root)]);
    run_program(&recorder, &root);
    write_in_place(&root);
    recorder.mark("end");
    unmount(&root);
    let (log_path, final_image) = recorder.finish();
    let log = read_log(&log_path).unwrap();
    // The whole log on the base image gives the device the run left: no write is missing.
    let full = work.join("full.img");
    sparse_copy(&base, &full);
    let file = OpenOptions::new().write(true).open(&full).unwrap();
    for entry in &log {
        apply(&file, entry);
    }
    drop(file);
    same_images(&full, &final_image)
        .unwrap_or_else(|e| panic!("the replayed log is not the device the run left: {e}"));
    let _ = fs::remove_file(&full);
    let _ = fs::remove_file(&log_path);
    let _ = fs::remove_file(&final_image);
    let summary = replay(file_system, &base, &log, &work)
        .unwrap_or_else(|e| panic!("{kind:?} on {file_system}: a replayed state breaks S-9: {e}"));
    eprintln!(
        "{kind:?} on {file_system}: {} entries, {} flushes or FUA writes, {} states mounted and \
         checked; a.txt seen with contents {:?}; the file written in place torn in {} states",
        summary.entries, summary.flushes, summary.states, summary.a_contents, summary.torn_in_place
    );
    assert!(
        summary.a_contents.len() > 2,
        "the replay did not pass through the commits of a.txt: {:?}",
        summary.a_contents
    );
    assert!(
        summary.torn_in_place > 0,
        "no replayed state showed the write in place torn: the trial cannot see a torn write"
    );
    let _ = fs::remove_dir_all(&work);
}

/// The trial: see the comment at the top of this file.
#[test]
#[ignore = "needs root, loop devices, and UNIO_POWER_CUT=dm-log-writes or fuse (job power-cut)"]
fn a_power_cut_at_every_logged_write_keeps_the_guarantees() {
    let kind = match std::env::var(ENV).as_deref() {
        Ok("dm-log-writes") => Kind::DmLogWrites,
        Ok("fuse") => Kind::Fuse,
        other => panic!("set {ENV} to dm-log-writes or fuse, not {other:?}"),
    };
    // SAFETY: a plain system call.
    assert_eq!(unsafe { geteuid() }, 0, "the trial needs root");
    let systems = std::env::var(ENV_FS).unwrap_or_else(|_| "ext4,xfs".to_string());
    for file_system in systems.split(',').filter(|s| !s.is_empty()) {
        trial(kind, file_system);
    }
}
