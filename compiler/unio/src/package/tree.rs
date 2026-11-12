//! The content of a package: its files read from a directory, and the content hash that names
//! its entry in the cache (T-7, T-14).
//!
//! The hash is SHA-384 over a versioned canonical form, with domain separation: the line
//! `unio-package-content-v1`, then for every file in the byte order of its relative name the
//! line `file <length> <name>` and its bytes. Directories are not hashed; the files decide the
//! content. It is the same however the files arrived (an archive or a directory) and wherever
//! they lie, so the cache is addressed by content. It authenticates nothing by itself
//! (docs/cryptography.md): it binds a lock entry to the files that were reviewed.

use std::path::Path;

use super::archive::{portable_name, Collector, Entries, Item, Limits};

pub fn content_hash(entries: &Entries) -> String {
    let mut data = b"unio-package-content-v1\n".to_vec();
    for (name, item) in entries {
        if let Item::File(bytes) = item {
            data.extend_from_slice(format!("file {} {name}\n", bytes.len()).as_bytes());
            data.extend_from_slice(bytes);
        }
    }
    format!("sha384:{}", crate::sha::sha384_hex(&data))
}

/// What is left out when a package is copied from the directory of a path dependency: names
/// that start with a dot (`.git`), and `target/` and `unio.lock` at the top, which belong to a
/// build or to the root of a resolution, not to the package.
fn left_out(parts: &[String]) -> bool {
    parts.iter().any(|p| p.starts_with('.'))
        || (parts.len() == 1 && (parts[0] == "target" || parts[0] == "unio.lock"))
}

/// The files of a directory, as an archive would give them, with the same refusals: a link,
/// a device, or a name outside the portable set is an error, never followed. `project`: the
/// directory of a path dependency, whose project files are left out; otherwise every file
/// counts (an entry of the cache).
pub fn read_dir(root: &Path, limits: &Limits, project: bool) -> Result<Entries, String> {
    let mut collector = Collector::new(*limits);
    let mut stack: Vec<(std::path::PathBuf, Vec<String>)> = vec![(root.to_path_buf(), Vec::new())];
    while let Some((dir, parts)) = stack.pop() {
        let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        names.sort();
        for name in names {
            let text = name
                .to_str()
                .ok_or_else(|| format!("a name in {} is not UTF-8", dir.display()))?;
            let mut child = parts.clone();
            child.push(text.to_string());
            if project && left_out(&child) {
                continue;
            }
            let joined = child.join("/");
            portable_name(&joined, limits)?;
            let path = dir.join(&name);
            let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{joined}: {e}"))?;
            let kind = meta.file_type();
            if kind.is_symlink() {
                return Err(format!("{joined} is a symbolic link"));
            } else if kind.is_dir() {
                collector.add(&child, Item::Directory)?;
                stack.push((path, child));
            } else if kind.is_file() {
                if meta.len() > limits.file_bytes {
                    return Err(format!(
                        "the file {joined} is larger than {} bytes",
                        limits.file_bytes
                    ));
                }
                let data = std::fs::read(&path).map_err(|e| format!("{joined}: {e}"))?;
                collector.add(&child, Item::File(data))?;
            } else {
                return Err(format!("{joined} is not a file or a directory"));
            }
        }
    }
    if !collector.entries.contains_key("unio.toml") {
        return Err(format!("{} has no unio.toml", root.display()));
    }
    Ok(collector.entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_depends_on_the_files_only() {
        let mut a = Entries::new();
        a.insert("unio.toml".into(), Item::File(b"x".to_vec()));
        let mut b = a.clone();
        b.insert("empty".into(), Item::Directory);
        assert_eq!(content_hash(&a), content_hash(&b));
        b.insert("f".into(), Item::File(Vec::new()));
        assert_ne!(content_hash(&a), content_hash(&b));
        // The name and the length are framed: moving a byte between name and data changes it.
        let mut c = Entries::new();
        c.insert("ab".into(), Item::File(b"c".to_vec()));
        let mut d = Entries::new();
        d.insert("a".into(), Item::File(b"bc".to_vec()));
        assert_ne!(content_hash(&c), content_hash(&d));
    }
}
