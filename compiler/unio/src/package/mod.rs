//! Packages (T-6 to T-9, T-14, SB-12): manifests, versions, resolution, the lock, archives, the
//! content-addressed cache, and the client side of a registry. The commands (`unio add`,
//! `remove`, `update`, `install`) and their download and unpack workers are in the `unio`
//! executable; this library gives them, and the loader of programs, what they share.
//!
//! The identity of a package (T-14) is its origin: `registry+<registry URL>#<owner>/<name>`,
//! `path+<directory relative to the root package>`, or `archive+<file or URL>`. Its lock entry
//! binds the identity to an exact version and the content hash of its files, which names its
//! entry in the cache: `<cache>/packages/<hex digest>/`.

pub mod archive;
pub mod config;
pub mod edit;
pub mod http;
pub mod index;
pub mod lock;
pub mod manifest;
pub mod registry;
pub mod resolve;
pub mod semver;
pub mod signature;
pub mod tree;
pub mod trust;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The cache a worker was given by its tool, which has no environment of its own.
static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Makes the directory the cache of packages for this process (a worker of the tools).
pub fn use_cache(dir: Option<PathBuf>) {
    let _ = CACHE.set(dir);
}

/// The cache the tool found, canonical, as it was when first asked for.
static DEFAULT_CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The cache of packages: the one the tool gave this worker, or [`config::default_cache`],
/// canonical when it exists. The tool finds it once, before any sandbox applies, so that the
/// paths of the cache entries it computes later, inside the sandbox of a program, are the
/// same canonical paths the worker loaded the modules from (on macOS the temporary directory
/// is a link).
pub fn cache_root() -> Option<PathBuf> {
    match CACHE.get() {
        Some(dir) => dir.clone(),
        None => DEFAULT_CACHE
            .get_or_init(|| {
                config::default_cache().map(|d| {
                    let d = if d.is_absolute() {
                        d
                    } else {
                        std::env::current_dir().map(|c| c.join(&d)).unwrap_or(d)
                    };
                    unio_runtime::sandbox::canonical(&d).unwrap_or(d)
                })
            })
            .clone(),
    }
}

/// The directory of a cache entry, by the content hash of the package (`sha384:<hex>`).
pub fn entry_dir(cache: &Path, content: &str) -> Option<PathBuf> {
    let hex = lock::digest_hex(content)?;
    Some(cache.join("packages").join(hex))
}

/// What the loader needs to resolve `deps.<alias>.<module>` (T-14): for the root package and
/// for each locked package, its aliases with the identity and the cache entry of each.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    /// Alias to (identity, entry) of the root package.
    pub root: HashMap<String, Target>,
    /// Identity to its aliases.
    pub packages: HashMap<String, HashMap<String, Target>>,
    /// Identity to its name and version, for messages and traces.
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub entry: PathBuf,
}

/// Reads `unio.lock` of a root package through the loader's files (`crate::vfs`), or `None`
/// without one. An unreadable or malformed lock is an error for the import that needs it.
pub fn resolved(package: &Path) -> Result<Option<Resolved>, String> {
    let path = package.join(lock::FILE);
    if !crate::vfs::is_file(&path) {
        return Ok(None);
    }
    let text = crate::vfs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lock = lock::Lock::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(cache) = cache_root() else {
        return Err("no cache of packages: set UNIO_CACHE or HOME".into());
    };
    let target = |id: &str| -> Option<Target> {
        let entry = lock.entry(id)?;
        Some(Target {
            id: id.to_string(),
            entry: entry_dir(&cache, &entry.content)?,
        })
    };
    let table = |edges: &[lock::Edge]| -> HashMap<String, Target> {
        edges
            .iter()
            .filter_map(|e| Some((e.alias.clone(), target(&e.id)?)))
            .collect()
    };
    Ok(Some(Resolved {
        root: table(&lock.root),
        packages: lock
            .packages
            .iter()
            .map(|e| (e.id.clone(), table(&e.dependencies)))
            .collect(),
        labels: lock
            .packages
            .iter()
            .map(|e| (e.id.clone(), format!("{}@{}", e.name, e.version)))
            .collect(),
    }))
}

/// The cache entries of the packages a root package locks, for the read scopes of a worker
/// (SB-11): only the locked dependencies, never the whole cache. A malformed lock gives none;
/// the compiler reports it.
pub fn locked_entries(package: &Path, cache: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(package.join(lock::FILE)) else {
        return Vec::new();
    };
    let Ok(lock) = lock::Lock::parse(&text) else {
        return Vec::new();
    };
    lock.packages
        .iter()
        .filter_map(|e| entry_dir(cache, &e.content))
        .filter(|d| d.is_dir())
        .collect()
}
