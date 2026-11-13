//! The incremental build and test cache (C-11a, C-11b; docs/tools.md, "The cache").
//!
//! An entry is found by a content-addressed key: SHA-384 over the versioned, framed fields of
//! every input that decides it, never a source hash alone. An artifact key of `unio build`
//! binds the scheme, the compiler (its version and the hash of the sources of the compiler,
//! the code generator, the runtime and the embedded standard library, `UNIO_SOURCE_HASH`), the
//! target and its ABI, the profile, the identity of the linker and the SDK, and the texts of
//! every file the program is loaded from: its modules, those of the standard library and of its
//! dependencies, the manifests with their permissions, and the lock. A test key binds the same
//! program inputs and additionally the content hash of the test (C-11a), the runner and the
//! scheme of property generation, the seed of `--seed`, the policy, the permissions and limits
//! the test runs with, and the place of the standard library. Neither binds the time or the
//! environment of the build.
//!
//! The cache lives in the package, in `target/unio-cache/` below the directory of `unio.toml`
//! (the build directory of SB-11): `artifacts/` holds the object files of `unio build`, `tests/`
//! the results of tests that passed and read nothing from outside the program. A file outside a
//! package has no build directory and is not cached. Nothing is shared between packages or
//! users, and nothing goes to the network.
//!
//! An entry records its scheme, its kind, its key, the length and the SHA-384 of its payload;
//! an entry that is incomplete, damaged, or written for another key does not match, is never
//! trusted, and is made again. Entries are written to a temporary file and renamed into place.
//! The results of one `unio test` are appended to a journal that the tool opens before the
//! sandbox of the tests applies (`tests/pending/`), since the sandbox may forbid creating files
//! afterwards; the next `unio test` checks every record of a journal and moves it into place.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The version of the format of keys and entries; part of every key and every entry.
pub const SCHEME: &str = "unio-cache-v1";

/// The kinds of entries, each a directory of the cache.
pub const ARTIFACTS: &str = "artifacts";
pub const TESTS: &str = "tests";

/// The largest entry read back: larger files are not entries of this cache.
const MAX_ENTRY: u64 = 1 << 30;

/// The identity of this compiler in keys: its version and the hash of the sources of the
/// compiler, the runtime and the embedded standard library (`build.rs`).
pub fn toolchain() -> String {
    format!(
        "unio {} sources {}",
        env!("CARGO_PKG_VERSION"),
        env!("UNIO_SOURCE_HASH")
    )
}

/// The framed fields of a key, in order.
#[derive(Default)]
pub struct Key(Vec<u8>);

impl Key {
    pub fn new(domain: &str) -> Key {
        let mut key = Key::default();
        key.field("scheme", SCHEME).field("domain", domain);
        key
    }

    /// Adds a named field; names and values are framed with their lengths, so that no two
    /// sequences of fields encode alike.
    pub fn field(&mut self, name: &str, value: &str) -> &mut Key {
        self.bytes(name, value.as_bytes())
    }

    pub fn bytes(&mut self, name: &str, value: &[u8]) -> &mut Key {
        for part in [name.as_bytes(), value] {
            self.0.extend_from_slice(&(part.len() as u64).to_be_bytes());
            self.0.extend_from_slice(part);
        }
        self
    }

    /// The key: `unio-cache-v1:sha384:<96 hexadecimal digits>`.
    pub fn finish(&self) -> String {
        format!("{SCHEME}:sha384:{}", crate::sha::sha384_hex(&self.0))
    }
}

/// The digest of the texts a program is loaded from, by path, in the order of their paths.
pub fn sources_digest(files: &[(PathBuf, String)]) -> String {
    let mut sorted: Vec<&(PathBuf, String)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut key = Key::new("sources");
    for (path, text) in sorted {
        key.field("path", &path.to_string_lossy())
            .field("text", text);
    }
    key.finish()
}

/// The directory of `unio.toml` nearest above the file, by the canonical path of the file.
pub fn package_root(file: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(file).ok()?;
    absolute
        .parent()?
        .ancestors()
        .find(|dir| dir.join("unio.toml").is_file())
        .map(Path::to_path_buf)
}

/// The cache of the package of a file, if the file belongs to a package.
pub fn dir_of(file: &Path) -> Option<PathBuf> {
    package_root(file).map(|root| root.join("target").join("unio-cache"))
}

fn hex_of(key: &str) -> Option<&str> {
    let hex = key.strip_prefix(SCHEME)?.strip_prefix(":sha384:")?;
    (hex.len() == 96 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// The file of the entry of `key`.
pub fn entry_path(dir: &Path, kind: &str, key: &str) -> Option<PathBuf> {
    Some(dir.join(kind).join(hex_of(key)?))
}

/// An entry as it is stored: its header and its payload.
fn encode(kind: &str, key: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "{SCHEME}\n{kind}\n{key}\n{}\n{}\n",
        payload.len(),
        crate::sha::sha384_hex(payload)
    )
    .into_bytes();
    out.extend_from_slice(payload);
    out
}

/// The payload of a stored entry, if it is whole and was written for this kind and key.
fn decode(kind: &str, key: &str, data: &[u8]) -> Option<Vec<u8>> {
    let mut rest = data;
    let mut line = || -> Option<String> {
        let end = rest.iter().position(|b| *b == b'\n')?;
        let text = std::str::from_utf8(&rest[..end]).ok()?.to_string();
        rest = &rest[end + 1..];
        Some(text)
    };
    if line()? != SCHEME || line()? != kind || line()? != key {
        return None;
    }
    let length: usize = line()?.parse().ok()?;
    let digest = line()?;
    if rest.len() != length || crate::sha::sha384_hex(rest) != digest {
        return None;
    }
    Some(rest.to_vec())
}

/// What a lookup found.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    Hit(Vec<u8>),
    Missing,
    /// A file that is no whole entry of this key: never trusted.
    Damaged,
}

/// Looks up the entry of `key`.
pub fn read(dir: &Path, kind: &str, key: &str) -> Lookup {
    let Some(path) = entry_path(dir, kind, key) else {
        return Lookup::Missing;
    };
    let Ok(file) = File::open(&path) else {
        return Lookup::Missing;
    };
    let mut data = Vec::new();
    if file.take(MAX_ENTRY).read_to_end(&mut data).is_err() {
        return Lookup::Damaged;
    }
    match decode(kind, key, &data) {
        Some(payload) => Lookup::Hit(payload),
        None => Lookup::Damaged,
    }
}

/// Stores the entry of `key`, replacing a damaged one: a temporary file renamed into place.
pub fn write(dir: &Path, kind: &str, key: &str, payload: &[u8]) -> std::io::Result<()> {
    let path = entry_path(dir, kind, key)
        .ok_or_else(|| std::io::Error::other(format!("'{key}' is not a key of the cache")))?;
    let parent = path.parent().expect("an entry has a directory");
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        hex_of(key).unwrap_or("entry"),
        std::process::id()
    ));
    let written = File::create(&temporary).and_then(|mut f| {
        f.write_all(&encode(kind, key, payload))?;
        f.sync_all()
    });
    written
        .and_then(|()| std::fs::rename(&temporary, &path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
}

/// Removes a damaged entry, so that it is made again.
pub fn remove(dir: &Path, kind: &str, key: &str) {
    if let Some(path) = entry_path(dir, kind, key) {
        let _ = std::fs::remove_file(path);
    }
}

// ---------------------------------------------------------------- the journal of `unio test`

const JOURNAL: &str = "unio-cache-journal-v1\n";
const END: &[u8] = b"end\n";

/// The directory of the journals of the tests.
fn pending(dir: &Path) -> PathBuf {
    dir.join(TESTS).join("pending")
}

/// A journal of test results, opened before the sandbox of the tests applies; the records are
/// written after the tests, into the file already open.
pub struct Journal {
    file: File,
    records: usize,
}

impl Journal {
    pub fn open(dir: &Path) -> std::io::Result<Journal> {
        let pending = pending(dir);
        std::fs::create_dir_all(&pending)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = pending.join(format!("{}-{nanos}", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(JOURNAL.as_bytes())?;
        Ok(Journal { file, records: 0 })
    }

    /// Appends the entry of `key`.
    pub fn record(&mut self, kind: &str, key: &str, payload: &[u8]) -> std::io::Result<()> {
        let entry = encode(kind, key, payload);
        self.file
            .write_all(&(entry.len() as u64).to_be_bytes())
            .and_then(|()| self.file.write_all(&entry))?;
        self.records += 1;
        Ok(())
    }

    /// Marks the journal complete.
    pub fn finish(mut self) -> std::io::Result<()> {
        self.file
            .write_all(&u64::MAX.to_be_bytes())
            .and_then(|()| self.file.write_all(END))
            .and_then(|()| self.file.sync_all())
    }
}

/// Moves the records of every journal of the package's cache into place. A record that is not
/// whole is dropped; a journal that is complete, or older than a day (its run ended without
/// completing it), is removed, and one that another run may still write is left.
pub fn promote(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(pending(dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let mut complete = false;
        if let Some(mut rest) = data.strip_prefix(JOURNAL.as_bytes()) {
            while rest.len() >= 8 {
                let length = u64::from_be_bytes(rest[..8].try_into().expect("eight bytes"));
                rest = &rest[8..];
                if length == u64::MAX {
                    complete = rest == END;
                    break;
                }
                let Some(record) = usize::try_from(length).ok().and_then(|n| rest.get(..n)) else {
                    break;
                };
                rest = &rest[record.len()..];
                store_record(dir, record);
            }
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(24 * 3600));
        if complete || stale || !data.starts_with(JOURNAL.as_bytes()) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Stores one record of a journal if it is a whole entry.
fn store_record(dir: &Path, record: &[u8]) {
    let mut lines = record.splitn(4, |b| *b == b'\n');
    let (Some(_), Some(kind), Some(key)) = (lines.next(), lines.next(), lines.next()) else {
        return;
    };
    let (Ok(kind), Ok(key)) = (std::str::from_utf8(kind), std::str::from_utf8(key)) else {
        return;
    };
    if kind != TESTS {
        return;
    }
    if let Some(payload) = decode(kind, key, record) {
        let _ = write(dir, kind, key, &payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("unio-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn keys_frame_their_fields() {
        let mut a = Key::new("t");
        a.field("x", "ab").field("y", "c");
        let mut b = Key::new("t");
        b.field("x", "a").field("y", "bc");
        assert_ne!(a.finish(), b.finish());
        assert!(hex_of(&a.finish()).is_some());
    }

    #[test]
    fn an_entry_is_found_only_whole_and_for_its_key() {
        let dir = scratch("entries");
        let key = Key::new("t").finish();
        let other = {
            let mut k = Key::new("t");
            k.field("more", "1");
            k.finish()
        };
        assert_eq!(read(&dir, TESTS, &key), Lookup::Missing);
        write(&dir, TESTS, &key, b"payload").unwrap();
        assert_eq!(read(&dir, TESTS, &key), Lookup::Hit(b"payload".to_vec()));
        // A copy under another key, a flipped byte, and a truncation are damaged.
        let path = entry_path(&dir, TESTS, &key).unwrap();
        std::fs::copy(&path, entry_path(&dir, TESTS, &other).unwrap()).unwrap();
        assert_eq!(read(&dir, TESTS, &other), Lookup::Damaged);
        let mut data = std::fs::read(&path).unwrap();
        let last = data.len() - 1;
        data[last] ^= 1;
        std::fs::write(&path, &data).unwrap();
        assert_eq!(read(&dir, TESTS, &key), Lookup::Damaged);
        std::fs::write(&path, &data[..data.len() - 3]).unwrap();
        assert_eq!(read(&dir, TESTS, &key), Lookup::Damaged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_journal_is_promoted_when_complete() {
        let dir = scratch("journal");
        let key = Key::new("t").finish();
        let mut journal = Journal::open(&dir).unwrap();
        journal.record(TESTS, &key, b"result").unwrap();
        journal.finish().unwrap();
        // An incomplete journal of another run stays; its whole records are taken.
        let mut open = Journal::open(&dir).unwrap();
        let second = {
            let mut k = Key::new("t");
            k.field("n", "2");
            k.finish()
        };
        open.record(TESTS, &second, b"second").unwrap();
        promote(&dir);
        assert_eq!(read(&dir, TESTS, &key), Lookup::Hit(b"result".to_vec()));
        assert_eq!(read(&dir, TESTS, &second), Lookup::Hit(b"second".to_vec()));
        assert_eq!(std::fs::read_dir(pending(&dir)).unwrap().count(), 1);
        drop(open);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
