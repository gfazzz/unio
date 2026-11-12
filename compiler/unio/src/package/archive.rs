//! The archive of a package (SB-12, SB-15): an uncompressed POSIX ustar archive, as
//! `tar --format=ustar -cf NAME.tar -C DIR .` makes it.
//!
//! Why this format: ustar is a standard that existing tools write, its header is a fixed
//! block of 512 bytes that the standard library of Rust parses without a dependency, and an
//! uncompressed archive cannot expand: every byte of every file is present in the archive, so
//! the unpacked size never exceeds the size of the archive, which the limits bound before
//! anything is read, and no decompression-ratio attack (a "zip bomb") exists. Compression is
//! not required by the specification; a compressed format would be a new version of the format
//! with its own ratio limit.
//!
//! What is accepted: regular files and directories, with portable relative names (ASCII
//! letters, digits, `.`, `_`, `-`, `+`), optionally all below one top-level directory. What is
//! refused, before anything is written: absolute names, `..`, backslashes and other characters
//! outside that set, names that Windows reserves or cannot keep (`CON`, a trailing dot), links
//! (symbolic and hard), devices, FIFOs, extended headers of pax and GNU (long names), duplicate
//! names, names that differ only in case (they collide on case-insensitive file systems, and a
//! package must unpack the same everywhere), a file where a directory is needed, too many
//! entries, too deep a tree, files and archives over the size limits, sizes that run past the
//! end of the archive, bad header checksums, and data after the end of the archive.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const BLOCK: usize = 512;

/// The limits of an archive and of the package unpacked from it or copied from a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub archive_bytes: u64,
    pub unpacked_bytes: u64,
    pub file_bytes: u64,
    pub entries: usize,
    pub depth: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            archive_bytes: 64 << 20,
            unpacked_bytes: 64 << 20,
            file_bytes: 16 << 20,
            entries: 10_000,
            depth: 32,
        }
    }
}

/// The longest name of an entry, in bytes.
pub const MAX_NAME: usize = 255;

/// An entry of a package: a file with its bytes, or a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    File(Vec<u8>),
    Directory,
}

/// The files and directories of a package by their relative names (`/` separated), sorted.
pub type Entries = BTreeMap<String, Item>;

fn reserved(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit())
}

/// The components of a portable relative name; `./` at the start is dropped.
pub fn portable_name(name: &str, limits: &Limits) -> Result<Vec<String>, String> {
    if name.is_empty() {
        return Err("an entry without a name".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!(
            "the name of an entry is longer than {MAX_NAME} bytes"
        ));
    }
    if name.starts_with('/') {
        return Err(format!("the absolute path {name:?}"));
    }
    if name.contains('\\') {
        return Err(format!("the name {name:?} contains a backslash"));
    }
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    let mut parts: Vec<String> = Vec::new();
    for (i, part) in trimmed.split('/').enumerate() {
        if part == "." && i == 0 {
            continue;
        }
        if part == ".." {
            return Err(format!(
                "the name {name:?} leads out of the package with .."
            ));
        }
        if part.is_empty() || part == "." {
            return Err(format!("the name {name:?} has an empty component"));
        }
        if !part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
        {
            return Err(format!(
                "the name {name:?} has characters outside the portable set (ASCII letters, \
                 digits, '.', '_', '-', '+')"
            ));
        }
        if part.ends_with('.') {
            return Err(format!("the name {name:?} ends a component with a dot"));
        }
        if reserved(part.split('.').next().unwrap_or(part)) {
            return Err(format!("the name {name:?} is reserved on Windows"));
        }
        parts.push(part.to_string());
    }
    if parts.len() > limits.depth {
        return Err(format!(
            "the name {name:?} is deeper than {} directories",
            limits.depth
        ));
    }
    Ok(parts)
}

/// Adds an entry, refusing duplicates, case collisions, and files in the place of directories.
pub struct Collector {
    pub entries: Entries,
    folded: BTreeSet<String>,
    total: u64,
    limits: Limits,
}

impl Collector {
    pub fn new(limits: Limits) -> Collector {
        Collector {
            entries: Entries::new(),
            folded: BTreeSet::new(),
            total: 0,
            limits,
        }
    }

    pub fn add(&mut self, parts: &[String], item: Item) -> Result<(), String> {
        if parts.is_empty() {
            // The top directory itself (`./`).
            return match item {
                Item::Directory => Ok(()),
                Item::File(_) => Err("a file without a name".into()),
            };
        }
        if self.entries.len() >= self.limits.entries {
            return Err(format!("more than {} entries", self.limits.entries));
        }
        if let Item::File(data) = &item {
            let size = data.len() as u64;
            if size > self.limits.file_bytes {
                return Err(format!(
                    "the file {} is larger than {} bytes",
                    parts.join("/"),
                    self.limits.file_bytes
                ));
            }
            self.total += size;
            if self.total > self.limits.unpacked_bytes {
                return Err(format!(
                    "the files are larger than {} bytes together",
                    self.limits.unpacked_bytes
                ));
            }
        }
        // Every parent is a directory, not a file.
        for end in 1..parts.len() {
            let parent = parts[..end].join("/");
            match self.entries.get(&parent) {
                Some(Item::File(_)) => {
                    return Err(format!("{parent} is a file and a directory"));
                }
                Some(Item::Directory) => {}
                None => {
                    self.insert(parent, Item::Directory)?;
                }
            }
        }
        let name = parts.join("/");
        match (self.entries.get(&name), &item) {
            (Some(Item::Directory), Item::Directory) => return Ok(()),
            (Some(_), _) => return Err(format!("the name {name} appears twice")),
            (None, _) => {}
        }
        self.insert(name, item)
    }

    fn insert(&mut self, name: String, item: Item) -> Result<(), String> {
        if !self.folded.insert(name.to_ascii_lowercase()) {
            return Err(format!(
                "the name {name} differs from another only in case, which collides on \
                 case-insensitive file systems"
            ));
        }
        self.entries.insert(name, item);
        Ok(())
    }
}

fn octal(field: &[u8], what: &str) -> Result<u64, String> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        return Err(format!(
            "the {what} of an entry uses the base-256 extension"
        ));
    }
    let text: String = field
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as char)
        .collect();
    let text = text.trim_matches(' ');
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|_| format!("the {what} of an entry is not octal"))
}

fn field_text(field: &[u8]) -> Result<String, String> {
    let bytes: Vec<u8> = field.iter().take_while(|b| **b != 0).copied().collect();
    String::from_utf8(bytes).map_err(|_| "a name that is not UTF-8".to_string())
}

/// Reads an archive into its entries, refusing everything the module documentation lists.
/// The caller has checked the hash of the bytes before (SB-12).
pub fn read(archive: &[u8], limits: &Limits) -> Result<Entries, String> {
    if archive.len() as u64 > limits.archive_bytes {
        return Err(format!(
            "the archive is larger than {} bytes",
            limits.archive_bytes
        ));
    }
    if !archive.len().is_multiple_of(BLOCK) {
        return Err("the archive is not a whole number of 512-byte blocks".into());
    }
    let mut collector = Collector::new(*limits);
    let mut headers = 0usize;
    let mut at = 0usize;
    loop {
        let Some(header) = archive.get(at..at + BLOCK) else {
            return Err("the archive ends without its end marker".into());
        };
        if header.iter().all(|b| *b == 0) {
            if archive[at..].iter().any(|b| *b != 0) {
                return Err("data follows the end of the archive".into());
            }
            break;
        }
        headers += 1;
        if headers > limits.entries {
            return Err(format!("more than {} entries", limits.entries));
        }
        let expected = octal(&header[148..156], "checksum")?;
        let sum: u64 = header
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    u64::from(*b)
                }
            })
            .sum();
        if sum != expected {
            return Err("a header has a wrong checksum".into());
        }
        if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
            return Err("an entry is not in the POSIX ustar format".into());
        }
        let size = octal(&header[124..136], "size")?;
        let typeflag = header[156];
        let mut name = field_text(&header[0..100])?;
        let prefix = field_text(&header[345..500])?;
        if !prefix.is_empty() {
            name = format!("{prefix}/{name}");
        }
        let data_start = at + BLOCK;
        let blocks = size.div_ceil(BLOCK as u64);
        let data_end = (data_start as u64)
            .checked_add(blocks * BLOCK as u64)
            .filter(|end| *end <= archive.len() as u64)
            .ok_or_else(|| {
                format!("the entry {name:?} declares {size} bytes, past the end of the archive")
            })? as usize;
        let refuse = |what: &str| Err(format!("the entry {name:?} is {what}"));
        match typeflag {
            b'0' | 0 => {
                let parts = portable_name(&name, limits)?;
                if name.ends_with('/') {
                    return refuse("a file whose name ends with /");
                }
                let data = archive[data_start..data_start + size as usize].to_vec();
                collector.add(&parts, Item::File(data))?;
            }
            b'5' => {
                if size != 0 {
                    return refuse("a directory with data");
                }
                let parts = portable_name(&name, limits)?;
                collector.add(&parts, Item::Directory)?;
            }
            b'1' => return refuse("a hard link"),
            b'2' => return refuse("a symbolic link"),
            b'3' => return refuse("a character device"),
            b'4' => return refuse("a block device"),
            b'6' => return refuse("a FIFO"),
            b'x' | b'g' => return refuse("an extended pax header"),
            b'L' | b'K' => return refuse("a GNU long name"),
            other => return refuse(&format!("of the unsupported type {:?}", other as char)),
        }
        at = data_end;
    }
    strip_top_directory(collector.entries)
}

/// A package whose files are all below one directory, without `unio.toml` at the top, is that
/// directory: `tar -cf x.tar pkg` works as well as `tar -cf x.tar -C pkg .`.
fn strip_top_directory(entries: Entries) -> Result<Entries, String> {
    if entries.contains_key("unio.toml") {
        return Ok(entries);
    }
    let tops: BTreeSet<&str> = entries
        .keys()
        .map(|k| k.split('/').next().unwrap_or(k))
        .collect();
    let single = tops.len() == 1;
    let top = tops.into_iter().next().map(str::to_string);
    if let (true, Some(top)) = (single, top) {
        if entries.get(&top) == Some(&Item::Directory)
            && entries.contains_key(&format!("{top}/unio.toml"))
        {
            let prefix = format!("{top}/");
            return Ok(entries
                .into_iter()
                .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|r| (r.to_string(), v)))
                .collect());
        }
    }
    Err("the archive has no unio.toml at its top or in its one top-level directory".into())
}

/// Writes the entries below `dest`, an empty directory: directories first, files created new
/// (never through an existing name), with no execute or special bits.
pub fn write_tree(entries: &Entries, dest: &Path) -> Result<(), String> {
    for (name, item) in entries {
        let path = dest.join(name);
        match item {
            Item::Directory => {
                std::fs::create_dir_all(&path).map_err(|e| format!("{name}: {e}"))?;
            }
            Item::File(data) => {
                use std::io::Write;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("{name}: {e}"))?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|e| format!("{name}: {e}"))?;
                file.write_all(data)
                    .and_then(|()| file.sync_all())
                    .map_err(|e| format!("{name}: {e}"))?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- writing

/// One header of a ustar archive with any type and name, for tests of hostile archives as
/// well as for [`write`].
pub fn header(name: &str, typeflag: u8, size: u64, link: &str) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    let (prefix, base) = if name.len() > 100 {
        match name[..name.len().min(156)].rfind('/') {
            Some(i) => (&name[..i], &name[i + 1..]),
            None => ("", name),
        }
    } else {
        ("", name)
    };
    let put = |h: &mut [u8; BLOCK], at: usize, len: usize, bytes: &[u8]| {
        let n = bytes.len().min(len);
        h[at..at + n].copy_from_slice(&bytes[..n]);
    };
    put(&mut h, 0, 100, base.as_bytes());
    put(&mut h, 100, 8, b"0000644\0");
    put(&mut h, 108, 8, b"0000000\0");
    put(&mut h, 116, 8, b"0000000\0");
    put(&mut h, 124, 12, format!("{size:011o}\0").as_bytes());
    put(&mut h, 136, 12, b"00000000000\0");
    h[156] = typeflag;
    put(&mut h, 157, 100, link.as_bytes());
    put(&mut h, 257, 6, b"ustar\0");
    put(&mut h, 263, 2, b"00");
    put(&mut h, 345, 155, prefix.as_bytes());
    h[148..156].copy_from_slice(b"        ");
    let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
    put(&mut h, 148, 8, format!("{sum:06o}\0 ").as_bytes());
    h
}

/// A ustar archive of the entries, in their sorted order, with fixed metadata (mode 0644 or
/// 0755, owner 0, time 0), so that the same files give the same bytes.
pub fn write(entries: &Entries) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, item) in entries {
        match item {
            Item::Directory => out.extend_from_slice(&header(&format!("{name}/"), b'5', 0, "")),
            Item::File(data) => {
                out.extend_from_slice(&header(name, b'0', data.len() as u64, ""));
                out.extend_from_slice(data);
                out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
            }
        }
    }
    out.extend_from_slice(&[0u8; BLOCK * 2]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> Entries {
        let mut c = Collector::new(Limits::default());
        for (name, text) in list {
            let parts = portable_name(name, &Limits::default()).unwrap();
            c.add(&parts, Item::File(text.as_bytes().to_vec())).unwrap();
        }
        c.entries
    }

    fn raw(entries: &[([u8; BLOCK], &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (h, data) in entries {
            out.extend_from_slice(h);
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
        }
        out.extend_from_slice(&[0u8; BLOCK * 2]);
        out
    }

    #[test]
    fn an_archive_round_trips() {
        let entries = files(&[("unio.toml", "[package]\n"), ("src/a.unio", "x")]);
        let bytes = write(&entries);
        assert_eq!(read(&bytes, &Limits::default()).unwrap(), entries);
        assert_eq!(write(&entries), bytes);
    }

    #[test]
    fn one_top_level_directory_is_the_package() {
        let bytes = raw(&[
            (header("pkg/", b'5', 0, ""), b""),
            (header("pkg/unio.toml", b'0', 1, ""), b"x"),
        ]);
        let entries = read(&bytes, &Limits::default()).unwrap();
        assert!(entries.contains_key("unio.toml"));
    }

    #[test]
    fn hostile_entries_are_refused() {
        let toml = (header("unio.toml", b'0', 1, ""), &b"x"[..]);
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (
                raw(&[toml, (header("/etc/passwd", b'0', 1, ""), b"x")]),
                "absolute",
            ),
            (raw(&[toml, (header("a/../../x", b'0', 1, ""), b"x")]), ".."),
            (
                raw(&[toml, (header("l", b'2', 0, "/etc"), b"")]),
                "symbolic link",
            ),
            (
                raw(&[toml, (header("h", b'1', 0, "unio.toml"), b"")]),
                "hard link",
            ),
            (
                raw(&[toml, (header("d", b'3', 0, ""), b"")]),
                "character device",
            ),
            (
                raw(&[toml, (header("b", b'4', 0, ""), b"")]),
                "block device",
            ),
            (raw(&[toml, (header("f", b'6', 0, ""), b"")]), "FIFO"),
            (raw(&[toml, (header("p", b'x', 0, ""), b"")]), "pax"),
            (
                raw(&[toml, (header("unio.toml", b'0', 1, ""), b"y")]),
                "twice",
            ),
            (
                raw(&[
                    toml,
                    (header("A.unio", b'0', 1, ""), b"y"),
                    (header("a.unio", b'0', 1, ""), b"y"),
                ]),
                "case",
            ),
            (
                raw(&[toml, (header("a\\b", b'0', 1, ""), b"y")]),
                "backslash",
            ),
            (raw(&[toml, (header("c:x", b'0', 1, ""), b"y")]), "portable"),
            (
                raw(&[toml, (header("NUL.txt", b'0', 1, ""), b"y")]),
                "reserved",
            ),
            (
                raw(&[
                    toml,
                    (header("x", b'0', 1, ""), b"y"),
                    (header("x/y", b'0', 1, ""), b"y"),
                ]),
                "file and a directory",
            ),
        ];
        for (bytes, expected) in cases {
            let error = read(&bytes, &Limits::default()).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn sizes_and_counts_are_limited() {
        let limits = Limits {
            entries: 3,
            file_bytes: 10,
            unpacked_bytes: 15,
            depth: 2,
            archive_bytes: 8192,
        };
        let toml = (header("unio.toml", b'0', 1, ""), &b"x"[..]);
        let many = raw(&[
            toml,
            (header("a", b'0', 1, ""), b"x"),
            (header("b", b'0', 1, ""), b"x"),
            (header("c", b'0', 1, ""), b"x"),
        ]);
        assert!(read(&many, &limits).unwrap_err().contains("entries"));
        let big = raw(&[toml, (header("a", b'0', 11, ""), &[1u8; 11][..])]);
        assert!(read(&big, &limits).unwrap_err().contains("larger"));
        let total = raw(&[
            toml,
            (header("a", b'0', 8, ""), &[1u8; 8][..]),
            (header("b", b'0', 8, ""), &[1u8; 8][..]),
        ]);
        assert!(read(&total, &limits).unwrap_err().contains("together"));
        let deep = raw(&[toml, (header("a/b/c", b'0', 1, ""), b"x")]);
        assert!(read(&deep, &limits).unwrap_err().contains("deeper"));
        // A size that claims more data than the archive has: refused, nothing allocated.
        let mut lying = raw(&[toml]);
        let h = header("bomb", b'0', 1 << 40, "");
        lying.splice(BLOCK * 2..BLOCK * 2, h);
        assert!(read(&lying, &Limits::default())
            .unwrap_err()
            .contains("past the end"));
        let huge = vec![0u8; 16384];
        assert!(read(&huge, &limits).unwrap_err().contains("larger than"));
        let mut bad = raw(&[toml]);
        bad[0] = b'y';
        assert!(read(&bad, &limits).unwrap_err().contains("checksum"));
        let mut trailing = raw(&[toml]);
        trailing.extend_from_slice(&header("late", b'0', 0, ""));
        assert!(read(&trailing, &limits).is_err());
    }
}
