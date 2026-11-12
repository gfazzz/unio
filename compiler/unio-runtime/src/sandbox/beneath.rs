//! The platform-independent part of a read below a scope (release condition 5 of docs/plan.md
//! section 12, docs/runtime.md 9.2): which directory a checked path is opened from, and the
//! walk from that directory one component at a time, in which no component may be a link.
//!
//! The runtime check resolves a path as the system would ([`super::resolve`]) and compares the
//! result with the scopes. The result has no `.`, no `..`, and no link in its existing part. If
//! the operation then opened that path by its name, another process could put a link on it in
//! between, and the system would follow it. Instead the operation starts at a handle of the
//! directory of a scope (its [`Anchored::anchor`]), which is itself opened without following a
//! link, and opens each remaining component relative to the handle of the one before, refusing
//! a link at every step. A link found there was put on the path after the check, so the
//! operation fails rather than follow it: what it reaches is always the checked path.
//!
//! Each platform supplies the steps through [`Beneath`]: Linux and macOS with directory
//! descriptors and `openat` with `O_NOFOLLOW` (`fdio.rs`), Windows with handles opened relative
//! to a root handle (`NtCreateFile` with `RootDirectory` and `FILE_OPEN_REPARSE_POINT`, refusing
//! a handle whose attributes name a reparse point).

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::Scope;

/// A checked path split at the directory it is opened from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchored {
    /// The root of the covering scope when that scope is a subtree and the path lies below its
    /// root; otherwise the directory that holds the path. Absolute and link-free when the check
    /// was made.
    pub anchor: PathBuf,
    /// The names from the anchor to the path; empty when the path is the anchor itself (only
    /// for a root of the file system, which has no directory above it).
    pub rest: Vec<OsString>,
}

/// Splits a path that the runtime resolved and found inside one of the scopes. `None` when no
/// scope covers it, or when it is not absolute and normal (a check never produces one).
pub fn anchor(target: &Path, scopes: &[Scope]) -> Option<Anchored> {
    let normal = target.is_absolute()
        && target
            .components()
            .all(|c| !matches!(c, Component::CurDir | Component::ParentDir));
    if !normal {
        return None;
    }
    let scope = scopes.iter().find(|s| s.covers(target))?;
    if scope.subtree && target != scope.root {
        let rest = target.strip_prefix(&scope.root).ok()?;
        return Some(Anchored {
            anchor: scope.root.clone(),
            rest: names(rest),
        });
    }
    Some(match (target.parent(), target.file_name()) {
        (Some(parent), Some(name)) => Anchored {
            anchor: parent.to_path_buf(),
            rest: vec![name.to_os_string()],
        },
        _ => Anchored {
            anchor: target.to_path_buf(),
            rest: Vec::new(),
        },
    })
}

fn names(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect()
}

/// The steps of a walk below an anchor, on one platform. Every step must refuse a link
/// (a symbolic link, and on Windows any reparse point that redirects a name) instead of
/// following it, and must name only what the system holds under that one name in the directory
/// of the handle it is given.
pub trait Beneath {
    /// A handle of a directory or of the final object.
    type Handle;
    /// Opens the anchor, an absolute path without links, so that a link on it fails.
    fn open_anchor(&mut self, anchor: &Path) -> io::Result<Self::Handle>;
    /// Opens a directory in the directory of `dir`.
    fn open_directory(&mut self, dir: &Self::Handle, name: &OsString) -> io::Result<Self::Handle>;
    /// Opens the final object in the directory of `dir`, as the operation needs it.
    fn open_last(&mut self, dir: &Self::Handle, name: &OsString) -> io::Result<Self::Handle>;
    /// The final object when it is the anchor itself (a root of the file system).
    fn reopen_anchor(&mut self, anchor: Self::Handle) -> io::Result<Self::Handle>;
}

/// Opens the checked path through the steps of the platform: the anchor, each directory
/// relative to the one before, and the last name as the operation needs it.
pub fn walk<B: Beneath>(steps: &mut B, anchored: &Anchored) -> io::Result<B::Handle> {
    let mut handle = steps.open_anchor(&anchored.anchor)?;
    let Some((last, directories)) = anchored.rest.split_last() else {
        return steps.reopen_anchor(handle);
    };
    for name in directories {
        handle = steps.open_directory(&handle, name)?;
    }
    steps.open_last(&handle, last)
}

/// What an existing object is, for `exists` and `is_directory`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn scope(root: &str, subtree: bool) -> Scope {
        Scope {
            root: PathBuf::from(root),
            subtree,
        }
    }

    fn os(names: &[&str]) -> Vec<OsString> {
        names.iter().map(OsString::from).collect()
    }

    #[cfg(unix)]
    #[test]
    fn paths_are_anchored_at_the_scope_that_covers_them() {
        let scopes = [scope("/data/open", true), scope("/etc/one.conf", false)];
        let below = anchor(Path::new("/data/open/a/b.txt"), &scopes).unwrap();
        assert_eq!(below.anchor, Path::new("/data/open"));
        assert_eq!(below.rest, os(&["a", "b.txt"]));
        // The root of a subtree, and a scope of one file, open from the directory above.
        let root = anchor(Path::new("/data/open"), &scopes).unwrap();
        assert_eq!(root.anchor, Path::new("/data"));
        assert_eq!(root.rest, os(&["open"]));
        let one = anchor(Path::new("/etc/one.conf"), &scopes).unwrap();
        assert_eq!(one.anchor, Path::new("/etc"));
        assert_eq!(one.rest, os(&["one.conf"]));
        // Nothing outside the scopes, and nothing that is not normal.
        assert_eq!(anchor(Path::new("/data/other"), &scopes), None);
        assert_eq!(anchor(Path::new("/data/open/../x"), &scopes), None);
        assert_eq!(anchor(Path::new("data/open/x"), &scopes), None);
        let whole = anchor(Path::new("/"), &[scope("/", true)]).unwrap();
        assert_eq!(whole.anchor, Path::new("/"));
        assert!(whole.rest.is_empty());
    }

    /// A file system of names: directories, files, and links, with each step logged.
    struct Fake {
        entries: Vec<(&'static str, char)>,
        log: Vec<String>,
    }

    impl Fake {
        fn kind(&self, path: &Path) -> io::Result<char> {
            self.entries
                .iter()
                .find(|(p, _)| Path::new(p) == path)
                .map(|(_, k)| *k)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    impl Beneath for Fake {
        type Handle = PathBuf;
        fn open_anchor(&mut self, anchor: &Path) -> io::Result<PathBuf> {
            self.log.push(format!("anchor {}", anchor.display()));
            match self.kind(anchor)? {
                'd' => Ok(anchor.to_path_buf()),
                _ => Err(io::Error::other("link")),
            }
        }
        fn open_directory(&mut self, dir: &PathBuf, name: &OsString) -> io::Result<PathBuf> {
            let path = dir.join(name);
            self.log.push(format!("dir {}", path.display()));
            match self.kind(&path)? {
                'd' => Ok(path),
                _ => Err(io::Error::other("link")),
            }
        }
        fn open_last(&mut self, dir: &PathBuf, name: &OsString) -> io::Result<PathBuf> {
            let path = dir.join(name);
            self.log.push(format!("last {}", path.display()));
            match self.kind(&path)? {
                'l' => Err(io::Error::other("link")),
                _ => Ok(path),
            }
        }
        fn reopen_anchor(&mut self, anchor: PathBuf) -> io::Result<PathBuf> {
            Ok(anchor)
        }
    }

    #[test]
    fn the_walk_opens_each_name_from_the_one_before_and_refuses_links() {
        let mut fake = Fake {
            entries: vec![
                ("/s", 'd'),
                ("/s/a", 'd'),
                ("/s/a/f", 'f'),
                ("/s/l", 'l'),
                ("/s/a/fl", 'l'),
            ],
            log: Vec::new(),
        };
        let anchored = |rest: &[&str]| Anchored {
            anchor: PathBuf::from("/s"),
            rest: os(rest),
        };
        assert_eq!(
            walk(&mut fake, &anchored(&["a", "f"])).unwrap(),
            Path::new("/s/a/f")
        );
        // The names are joined with the separator of the system.
        let s = Path::new("/s");
        assert_eq!(
            fake.log,
            [
                "anchor /s".to_string(),
                format!("dir {}", s.join("a").display()),
                format!("last {}", s.join("a").join("f").display()),
            ]
        );
        assert!(walk(&mut fake, &anchored(&["l", "f"])).is_err());
        assert!(walk(&mut fake, &anchored(&["a", "fl"])).is_err());
        assert!(walk(&mut fake, &anchored(&["a", "missing"])).is_err());
        assert_eq!(walk(&mut fake, &anchored(&[])).unwrap(), Path::new("/s"));
    }
}
