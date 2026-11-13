//! The package commands (T-6 to T-9, T-14, SB-12): `unio add`, `remove`, `update`, and
//! `install`, with the download and unpack workers they start.
//!
//! The tool reads the manifest and the lock of the root package and the configuration of the
//! operator, decides what to fetch, starts the workers, shows the review of T-9, and writes
//! `unio.toml`, `unio.lock`, and the finished cache entries. Everything that comes from outside
//! the root package is handled in a worker: the download worker speaks HTTP over a connection
//! the tool opened to a configured registry and parses the registry index; the unpack worker
//! checks the hash (and the signature hook) of an archive, unpacks it into the staging
//! directory of its cache entry, reads the manifest of the package, and finds its marked
//! modules. Nothing of a package is ever executed (SB-14): no build scripts exist, and parsing
//! a module to find its `unsafe module` marker runs none of it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use unio::package::archive::Limits;
use unio::package::config::{Config, Registry};
use unio::package::http::Url;
use unio::package::index::Index;
use unio::package::lock::{self, Edge, Entry, Lock};
use unio::package::manifest::{self, Dependency, Manifest, Source};
use unio::package::resolve::{self, Candidate, Provider, Requirement};
use unio::package::semver::{Req, Version};
use unio::package::trust::{self, Bundle, Trust};
use unio::package::{edit, signature, tree};
use unio::sandbox_cli;
use unio_runtime::sandbox::Scope;

use crate::worker;

/// The largest registry index the download worker accepts.
const INDEX_LIMIT: u64 = 8 << 20;
/// The largest detached signature the download worker accepts.
const SIGNATURE_LIMIT: u64 = 1 << 20;
/// The file of the cache that keeps the highest sequences seen (rollback protection).
const STATE_FILE: &str = "trust/state";

// ---------------------------------------------------------------- what the unpack worker reports

/// What the unpack worker found in a package: its manifest as normalized data, its marked
/// modules, and its content hash.
#[derive(Clone, Debug, Default)]
struct Report {
    name: String,
    version: String,
    content: String,
    archive: String,
    unsafe_listed: Vec<String>,
    unsafe_marked: Vec<String>,
    permissions: Vec<String>,
    /// (alias, kind, a, b, requirement): kind 0 path (a), 1 registry (a registry name or
    /// empty, b package), 2 archive (a location, b SHA-384).
    dependencies: Vec<(String, u32, String, String, String)>,
    /// For a registry package: the key whose signature was verified, and the sequence of the
    /// revocation list it was verified with.
    signer: String,
    revocations: u64,
    /// The target and runtime requirements of its manifest (T-14): `package.unio_version`, or
    /// empty, and `package.targets`, when it is given.
    unio_version: String,
    targets: Option<Vec<String>>,
}

fn encode_list(out: &mut worker::Enc, items: &[String]) {
    out.u32(items.len() as u32);
    for item in items {
        out.text(item);
    }
}

fn decode_list(d: &mut worker::Dec) -> Option<Vec<String>> {
    let n = d.u32()?;
    (0..n).map(|_| d.text()).collect()
}

impl Report {
    fn encode(&self) -> Vec<u8> {
        let mut out = worker::Enc::default();
        out.text("unio-unpack-report-3")
            .text(&self.name)
            .text(&self.version)
            .text(&self.content)
            .text(&self.archive);
        encode_list(&mut out, &self.unsafe_listed);
        encode_list(&mut out, &self.unsafe_marked);
        encode_list(&mut out, &self.permissions);
        out.u32(self.dependencies.len() as u32);
        for (alias, kind, a, b, req) in &self.dependencies {
            out.text(alias).u32(*kind).text(a).text(b).text(req);
        }
        out.text(&self.signer).text(&self.revocations.to_string());
        out.text(&self.unio_version);
        match &self.targets {
            Some(targets) => {
                out.u32(1);
                encode_list(&mut out, targets);
            }
            None => {
                out.u32(0);
            }
        }
        out.0
    }

    fn decode(data: &[u8]) -> Option<Report> {
        let mut d = worker::Dec::new(data);
        if d.text()? != "unio-unpack-report-3" {
            return None;
        }
        let mut r = Report {
            name: d.text()?,
            version: d.text()?,
            content: d.text()?,
            archive: d.text()?,
            unsafe_listed: decode_list(&mut d)?,
            unsafe_marked: decode_list(&mut d)?,
            permissions: decode_list(&mut d)?,
            dependencies: Vec::new(),
            signer: String::new(),
            revocations: 0,
            unio_version: String::new(),
            targets: None,
        };
        let n = d.u32()?;
        for _ in 0..n {
            r.dependencies
                .push((d.text()?, d.u32()?, d.text()?, d.text()?, d.text()?));
        }
        r.signer = d.text()?;
        r.revocations = d.text()?.parse().ok()?;
        r.unio_version = d.text()?;
        r.targets = match d.u32()? {
            0 => None,
            _ => Some(decode_list(&mut d)?),
        };
        d.end().then_some(r)
    }

    /// The modules to review: those the manifest lists and those that carry the marker.
    fn unsafe_modules(&self) -> Vec<String> {
        let set: BTreeSet<String> = self
            .unsafe_listed
            .iter()
            .chain(&self.unsafe_marked)
            .cloned()
            .collect();
        set.into_iter().collect()
    }

    /// The target and runtime requirements of the package's manifest, checked against this
    /// toolchain and machine (T-14).
    fn requirements_met(&self) -> Result<(), String> {
        let req = if self.unio_version.is_empty() {
            None
        } else {
            Some(Req::parse(&self.unio_version)?)
        };
        unio::package::manifest::requirements_met(req.as_ref(), self.targets.as_deref())
    }

    fn manifest_dependencies(&self) -> Result<Vec<Dependency>, String> {
        self.dependencies
            .iter()
            .map(|(alias, kind, a, b, req)| {
                let source = match kind {
                    0 => Source::Path(a.clone()),
                    1 => Source::Registry {
                        registry: (!a.is_empty()).then(|| a.clone()),
                        package: b.clone(),
                    },
                    _ => Source::Archive {
                        location: a.clone(),
                        sha384: b.clone(),
                    },
                };
                let req = if req.is_empty() {
                    None
                } else {
                    Some(Req::parse(req)?)
                };
                Ok(Dependency {
                    alias: alias.clone(),
                    source,
                    req,
                })
            })
            .collect()
    }
}

fn req_text(req: &Option<Req>) -> String {
    req.as_ref()
        .map(|r| r.text().replace(' ', ""))
        .unwrap_or_default()
}

fn limits_text(l: &Limits) -> String {
    format!(
        "{},{},{},{},{}",
        l.archive_bytes, l.unpacked_bytes, l.file_bytes, l.entries, l.depth
    )
}

fn parse_limits(text: &str) -> Option<Limits> {
    let n: Vec<u64> = text
        .split(',')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let [archive_bytes, unpacked_bytes, file_bytes, entries, depth] = n.as_slice() else {
        return None;
    };
    Some(Limits {
        archive_bytes: *archive_bytes,
        unpacked_bytes: *unpacked_bytes,
        file_bytes: *file_bytes,
        entries: *entries as usize,
        depth: *depth as usize,
    })
}

// ---------------------------------------------------------------- inside the workers

/// The unpack worker, after its rules apply (`worker.rs`): `dir DIR STAGING LIMITS`,
/// `archive FILE SHA384 STAGING LIMITS PACKAGE|- VERSION|- REGISTRY|-`, or
/// `inspect ENTRY LIMITS`. For a package of a registry its input holds the trust bundle of the
/// registry and the signature envelope of the archive. It writes the report on standard output.
pub fn unpack_worker(arguments: &[String]) -> ExitCode {
    let arguments = arguments.to_vec();
    // Parsing a module may recurse deeply; the stack of the front end, as in other workers.
    let thread = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || unpack(&arguments));
    let result = match thread.map(|t| t.join()) {
        Ok(Ok(result)) => result,
        _ => Err("the worker failed".to_string()),
    };
    match result {
        Ok(report) => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if out
                .write_all(&report.encode())
                .and_then(|()| out.flush())
                .is_err()
            {
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("unio: the unpack worker refused the package: {problem}");
            ExitCode::from(1)
        }
    }
}

fn unpack(arguments: &[String]) -> Result<Report, String> {
    let bad = || "the unpack worker was started with bad arguments".to_string();
    let limits = |text: &String| parse_limits(text).ok_or_else(bad);
    match arguments {
        [mode, dir, staging, l] if mode == "dir" => {
            let entries = tree::read_dir(Path::new(dir), &limits(l)?, true)?;
            finish(&entries, Some(Path::new(staging)), None)
        }
        [mode, entry, l] if mode == "inspect" => {
            let entries = tree::read_dir(Path::new(entry), &limits(l)?, false)?;
            finish(&entries, None, None)
        }
        [mode, file, sha, staging, l, package, version, registry] if mode == "archive" => {
            let limits = limits(l)?;
            let size = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
            if size > limits.archive_bytes {
                return Err(format!(
                    "the archive is larger than {} bytes",
                    limits.archive_bytes
                ));
            }
            let bytes = std::fs::read(file).map_err(|e| format!("the archive: {e}"))?;
            // The hash before anything of the archive is read (SB-12).
            let digest = unio::sha::sha384_hex(&bytes);
            if &digest != sha {
                return Err(format!(
                    "the SHA-384 of the archive is {digest}, not the expected {sha}: refused \
                     before unpacking"
                ));
            }
            // The signature of a registry package, before anything of the archive is parsed
            // (T-8a, SB-12).
            let verified = if package != "-" {
                let parts = trust::unframe(worker::input(), 2)
                    .ok_or("the unpack worker was given malformed trust data")?;
                let bundle = Bundle::decode(parts[0])
                    .ok_or("the unpack worker was given a malformed trust bundle")?;
                let signature = (!parts[1].is_empty()).then_some(parts[1]);
                let verified = signature::check(&signature::Request {
                    package,
                    version,
                    registry,
                    archive: &bytes,
                    signature,
                    trust: &bundle,
                })?;
                eprintln!(
                    "unio: verified {package} {version}: ML-DSA-65 signature of the key {}",
                    verified.key
                );
                Some(verified)
            } else {
                None
            };
            let entries = unio::package::archive::read(&bytes, &limits)?;
            let mut report = finish(
                &entries,
                Some(Path::new(staging)),
                Some(format!("sha384:{digest}")),
            )?;
            if let Some(v) = verified {
                report.signer = v.key;
                report.revocations = v.revocations;
            }
            Ok(report)
        }
        _ => Err(bad()),
    }
}

fn finish(
    entries: &unio::package::archive::Entries,
    staging: Option<&Path>,
    archive: Option<String>,
) -> Result<Report, String> {
    use unio::package::archive::Item;
    let Some(Item::File(text)) = entries.get("unio.toml") else {
        return Err("the package has no unio.toml".into());
    };
    let text = String::from_utf8(text.clone()).map_err(|_| "unio.toml is not UTF-8")?;
    let manifest = Manifest::parse(&text).map_err(|e| format!("its unio.toml: {e}"))?;
    // The target and runtime requirements are checked by the tool, which installs (T-14).
    if manifest.name.is_empty() {
        return Err("its unio.toml has no package.name".into());
    }
    let version = manifest
        .version
        .as_ref()
        .ok_or("its unio.toml has no package.version")?;
    // The modules that carry the marker, found by parsing (never running) them (T-9, SB-14).
    let mut marked = Vec::new();
    for (name, item) in entries {
        let (Some(module), Item::File(bytes)) = (name.strip_suffix(".unio"), item) else {
            continue;
        };
        let source = String::from_utf8_lossy(bytes);
        let analysis = unio::check_source(name, &source);
        if analysis.file.unsafe_module {
            marked.push(module.replace('/', "."));
        }
    }
    if let Some(staging) = staging {
        unio::package::archive::write_tree(entries, staging)?;
    }
    let p = &manifest.permissions;
    let mut permissions = Vec::new();
    for (kind, list) in [
        ("files.read", &p.read),
        ("files.write", &p.write),
        ("env", &p.env),
        ("process", &p.process),
    ] {
        permissions.extend(list.iter().map(|e| format!("{kind} {e}")));
    }
    let dependencies = manifest
        .dependencies
        .iter()
        .map(|d| {
            let (kind, a, b) = match &d.source {
                Source::Path(path) => (0, path.clone(), String::new()),
                Source::Registry { registry, package } => {
                    (1, registry.clone().unwrap_or_default(), package.clone())
                }
                Source::Archive { location, sha384 } => (2, location.clone(), sha384.clone()),
            };
            (d.alias.clone(), kind, a, b, req_text(&d.req))
        })
        .collect();
    Ok(Report {
        name: manifest.name.clone(),
        version: version.to_string(),
        content: tree::content_hash(entries),
        archive: archive.unwrap_or_default(),
        unsafe_listed: manifest.unsafe_modules.clone(),
        unsafe_marked: marked,
        permissions,
        dependencies,
        signer: String::new(),
        revocations: 0,
        unio_version: manifest
            .unio_version
            .as_ref()
            .map_or(String::new(), |r| r.text().to_string()),
        targets: manifest.targets.clone(),
    })
}

/// The download worker, after its rules apply (`worker.rs`): `get HOST PORT PATH LIMIT NAME`
/// writes the body into the download area as NAME; `index HOST PORT PATH LIMIT PACKAGE`
/// verifies the signature of the registry index with the trust bundle, the envelope, and the
/// lowest acceptable sequence of its input, before it parses the index, and writes its
/// normalized lines on standard output.
pub fn download_worker(stream: std::net::TcpStream, temp: &Path, arguments: &[String]) -> ExitCode {
    let fail = |problem: String| {
        eprintln!("unio: the download worker: {problem}");
        ExitCode::from(1)
    };
    let [mode, host, port, path, limit, extra] = arguments else {
        return fail("started with bad arguments".into());
    };
    let (Ok(port), Ok(limit)) = (port.parse::<u16>(), limit.parse::<u64>()) else {
        return fail("started with bad arguments".into());
    };
    let url = Url {
        host: host.clone(),
        port,
        path: path.clone(),
    };
    let shown = format!("{}{}", url.origin(), url.path);
    let body = match unio::package::http::get(stream, &url, limit) {
        Ok(body) => body,
        Err(problem) => return fail(format!("{shown}: {problem}")),
    };
    match mode.as_str() {
        "get"
            if [
                "archive",
                "signature",
                "keys",
                "revocations",
                "index-signature",
            ]
            .contains(&extra.as_str()) =>
        {
            match std::fs::write(temp.join(extra), &body) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(format!("{shown}: writing the download: {e}")),
            }
        }
        "index" => match verified_index(&body, extra) {
            Ok(lines) => {
                print!("{lines}");
                ExitCode::SUCCESS
            }
            Err(problem) => fail(format!("{shown}: {problem}")),
        },
        _ => fail("started with bad arguments".into()),
    }
}

/// The index of a package, verified before it is parsed (T-8a): its envelope must be signed by
/// a key the trust bundle establishes, for this package, over exactly these bytes, with a
/// sequence no lower than the floor. Returns the lines for the tool.
fn verified_index(body: &[u8], package: &str) -> Result<String, String> {
    let parts = trust::unframe(worker::input(), 3).ok_or("started with malformed trust data")?;
    let bundle = Bundle::decode(parts[0]).ok_or("started with a malformed trust bundle")?;
    let floor: u64 = std::str::from_utf8(parts[2])
        .ok()
        .and_then(|f| f.parse().ok())
        .ok_or("started with a malformed floor")?;
    let refused = |why: String| format!("the index of {package} is refused: {why}");
    if parts[1].is_empty() {
        return Err(refused("it has no signature".into()));
    }
    let trust = Trust::load(&bundle, &trust::today()).map_err(&refused)?;
    let verified = trust
        .verify(parts[1], "registry-index", package, body)
        .map_err(&refused)?;
    if verified.sequence < floor {
        return Err(refused(format!(
            "its sequence {} is older than {floor}, which this project or machine has already \
             seen: a rollback to an older index",
            verified.sequence
        )));
    }
    let text = std::str::from_utf8(body).map_err(|_| refused("it is not UTF-8".into()))?;
    let index = Index::parse(text, package).map_err(&refused)?;
    if index.sequence != verified.sequence {
        return Err(refused(format!(
            "the document says sequence {} and its signature {}",
            index.sequence, verified.sequence
        )));
    }
    Ok(format!(
        "digest sha384:{}\nsigner {}\nrevocations {}\n{}",
        unio::sha::sha384_hex(body),
        verified.key,
        verified.revocations,
        index.encode()
    ))
}

// ---------------------------------------------------------------- the tool's side

enum Fail {
    /// Already reported, with this exit code (a worker, or a limit of the policy).
    Code(u8),
    Message(String),
}

impl From<String> for Fail {
    fn from(message: String) -> Fail {
        Fail::Message(message)
    }
}

impl From<&str> for Fail {
    fn from(message: &str) -> Fail {
        Fail::Message(message.to_string())
    }
}

type Res<T> = Result<T, Fail>;

/// Where a package came from, which decides how its own dependencies are named.
#[derive(Clone, Debug)]
enum Origin {
    Root,
    Dir(PathBuf),
    Registry(Registry),
    Archive,
}

/// How to obtain a package.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fetch {
    Dir(PathBuf),
    File { path: PathBuf, sha: String },
    Url { url: Url, sha: String },
    Registry { registry: Registry, package: String },
}

/// A staging directory of the cache, removed unless it became an entry.
struct Staging(Option<PathBuf>);

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

struct Staged {
    report: Report,
    staging: Staging,
    fetch: Option<String>,
    signature: Option<String>,
    index: Option<String>,
    index_sequence: Option<u64>,
}

/// The highest sequences seen on this machine, kept in the cache (`trust/state`): of the
/// revocation list of each registry (`revocations <registry URL>`) and of the index of each
/// package (`index <identity>`). Together with the lock, they are the floors below which a
/// signed file is a rollback (T-8a).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State(BTreeMap<String, u64>);

impl State {
    fn load(cache: &Path) -> State {
        let text = std::fs::read_to_string(cache.join(STATE_FILE)).unwrap_or_default();
        State(
            text.lines()
                .filter_map(|l| {
                    let (key, n) = l.rsplit_once(' ')?;
                    Some((key.to_string(), n.parse().ok()?))
                })
                .collect(),
        )
    }

    fn get(&self, key: &str) -> u64 {
        self.0.get(key).copied().unwrap_or(0)
    }

    fn raise(&mut self, key: String, n: u64) {
        let e = self.0.entry(key).or_insert(0);
        *e = (*e).max(n);
    }

    fn save(&self, cache: &Path) -> Result<(), String> {
        let mut text =
            String::from("# The highest sequences of signed registry files seen here (unio).\n");
        for (key, n) in &self.0 {
            text.push_str(&format!("{key} {n}\n"));
        }
        let path = cache.join(STATE_FILE);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        write_atomically(&path, &text)
    }
}

/// A verified index: the index, the hash of its document, and its signer.
#[derive(Clone)]
struct Verified {
    index: Index,
    digest: String,
}

struct Session {
    root: PathBuf,
    manifest_text: String,
    manifest: Manifest,
    old: Option<Lock>,
    config: Config,
    cache: PathBuf,
    options: sandbox_cli::Options,
    offline: bool,
    unsafe_ok: bool,
    fetch: BTreeMap<String, Fetch>,
    origins: BTreeMap<String, Origin>,
    indexes: BTreeMap<String, Verified>,
    /// The trust bundle of each registry, by its canonical URL, as read for this command.
    bundles: BTreeMap<String, Vec<u8>>,
    /// The floors of rollback protection as read, and as raised by what this command saw.
    state: State,
    seen: State,
    staged: BTreeMap<String, Staged>,
    reports: BTreeMap<String, Report>,
    /// Failures of the provider during resolution, kept to report the code of a worker.
    failure: Option<Fail>,
}

/// `PATH` with `.` and `..` resolved lexically.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `target` relative to `base`, with `/`, both absolute and lexical.
fn relative_to(base: &Path, target: &Path) -> String {
    let b: Vec<_> = base.components().collect();
    let t: Vec<_> = target.components().collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return target.to_string_lossy().replace('\\', "/");
    }
    let mut parts: Vec<String> = vec!["..".to_string(); b.len() - common];
    parts.extend(
        t[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.unio-tmp-{}", std::process::id()));
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            format!("{}: {e}", path.display())
        })
}

#[cfg(unix)]
fn inheritable(fd: i32) -> Result<(), String> {
    extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    const F_GETFD: i32 = 1;
    const F_SETFD: i32 = 2;
    const FD_CLOEXEC: i32 = 1;
    // SAFETY: integer arguments only, on a descriptor this process owns.
    unsafe {
        let flags = fcntl(fd, F_GETFD);
        if flags < 0 || fcntl(fd, F_SETFD, flags & !FD_CLOEXEC) < 0 {
            return Err(format!(
                "the connection cannot be passed to the download worker: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

impl Session {
    fn open(options: sandbox_cli::Options, offline: bool, unsafe_ok: bool) -> Res<Session> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
        let root = cwd
            .ancestors()
            .find(|a| a.join("unio.toml").is_file())
            .ok_or("no unio.toml in this directory or above it")?
            .to_path_buf();
        let manifest_text = std::fs::read_to_string(root.join("unio.toml"))
            .map_err(|e| format!("{}: {e}", root.join("unio.toml").display()))?;
        let manifest = Manifest::parse(&manifest_text)
            .map_err(|e| format!("{}: {e}", root.join("unio.toml").display()))?;
        let lock_path = root.join(lock::FILE);
        let old = match std::fs::read_to_string(&lock_path) {
            Ok(text) => {
                Some(Lock::parse(&text).map_err(|e| format!("{}: {e}", lock_path.display()))?)
            }
            Err(_) => None,
        };
        let config = Config::load()?;
        let cache = unio::package::cache_root()
            .ok_or("no cache of packages: set UNIO_CACHE, or HOME for the default")?;
        std::fs::create_dir_all(cache.join("packages"))
            .and_then(|()| std::fs::create_dir_all(cache.join("staging")))
            .map_err(|e| format!("the cache {}: {e}", cache.display()))?;
        let cache = std::fs::canonicalize(&cache).unwrap_or(cache);
        let state = State::load(&cache);
        Ok(Session {
            root,
            manifest_text,
            manifest,
            old,
            config,
            cache,
            options,
            offline,
            unsafe_ok,
            fetch: BTreeMap::new(),
            origins: BTreeMap::new(),
            indexes: BTreeMap::new(),
            bundles: BTreeMap::new(),
            seen: state.clone(),
            state,
            staged: BTreeMap::new(),
            reports: BTreeMap::new(),
            failure: None,
        })
    }

    fn registry(&self, name: &str) -> Res<Registry> {
        self.config.registries.get(name).cloned().ok_or_else(|| {
            Fail::Message(format!(
                "the registry '{name}' is not configured: the operator lists registries in the \
                 packages configuration ({})",
                self.config
                    .file
                    .as_ref()
                    .map_or("UNIO_PACKAGES_CONFIG".to_string(), |f| f
                        .display()
                        .to_string())
            ))
        })
    }

    /// The identity of a dependency declared by a package of this origin, with how to fetch
    /// it and the origin of its own dependencies (T-14).
    fn identify(&self, parent: &Origin, dep: &Dependency) -> Res<(String, Fetch, Origin)> {
        let base = match parent {
            Origin::Root => Some(self.root.clone()),
            Origin::Dir(dir) => Some(dir.clone()),
            _ => None,
        };
        match &dep.source {
            Source::Path(path) => {
                let base = base.ok_or_else(|| {
                    format!(
                        "the dependency '{}' is a path, which only the root package and path \
                         dependencies may use",
                        dep.alias
                    )
                })?;
                let dir = lexical(&base.join(path));
                let id = format!("path+{}", relative_to(&self.root, &dir));
                Ok((id, Fetch::Dir(dir.clone()), Origin::Dir(dir)))
            }
            Source::Registry { registry, package } => {
                let registry = match (registry, parent) {
                    (Some(name), _) => self.registry(name)?,
                    (None, Origin::Registry(r)) => r.clone(),
                    (None, _) => self.registry("default")?,
                };
                let id = format!("registry+{}#{package}", registry.url.canonical());
                Ok((
                    id,
                    Fetch::Registry {
                        registry: registry.clone(),
                        package: package.clone(),
                    },
                    Origin::Registry(registry),
                ))
            }
            Source::Archive { location, sha384 } => {
                if location.contains("://") {
                    let url = Url::parse(location)?;
                    let id = format!("archive+{}{}", url.origin(), url.path);
                    let fetch = Fetch::Url {
                        url,
                        sha: sha384.clone(),
                    };
                    return Ok((id, fetch, Origin::Archive));
                }
                let base = base.ok_or_else(|| {
                    format!(
                        "the dependency '{}' is a local archive, which only the root package \
                         and path dependencies may use",
                        dep.alias
                    )
                })?;
                let file = lexical(&base.join(location));
                let id = format!("archive+{}", relative_to(&self.root, &file));
                let fetch = Fetch::File {
                    path: file,
                    sha: sha384.clone(),
                };
                Ok((id, fetch, Origin::Archive))
            }
        }
    }

    /// Records how to fetch an identity; two different sources under one identity are an error.
    fn register(&mut self, id: &str, fetch: Fetch, origin: Origin) -> Res<()> {
        if let Some(known) = self.fetch.get(id) {
            if *known != fetch {
                return Err(format!(
                    "{id} is named twice with different contents (another hash or registry)"
                )
                .into());
            }
            return Ok(());
        }
        self.fetch.insert(id.to_string(), fetch);
        self.origins.insert(id.to_string(), origin);
        Ok(())
    }

    fn requirements(&mut self, parent: &Origin, deps: &[Dependency]) -> Res<Vec<Requirement>> {
        let mut out = Vec::new();
        for dep in deps {
            let (id, fetch, origin) = self.identify(parent, dep)?;
            self.register(&id, fetch, origin)?;
            out.push(Requirement {
                alias: dep.alias.clone(),
                id,
                req: dep.req.clone(),
            });
        }
        Ok(out)
    }

    /// How to fetch a locked package again, from its lock entry.
    fn fetch_of_entry(&self, entry: &Entry) -> Res<(Fetch, Origin)> {
        if let Some(rel) = entry.id.strip_prefix("path+") {
            let dir = lexical(&self.root.join(rel));
            return Ok((Fetch::Dir(dir.clone()), Origin::Dir(dir)));
        }
        let sha = entry
            .archive
            .as_deref()
            .and_then(lock::digest_hex)
            .map(str::to_string);
        if let Some(location) = entry.id.strip_prefix("archive+") {
            let sha = sha.ok_or_else(|| format!("{} has no archive hash in the lock", entry.id))?;
            if location.contains("://") {
                let url = Url::parse(location)?;
                return Ok((Fetch::Url { url, sha }, Origin::Archive));
            }
            let path = lexical(&self.root.join(location));
            return Ok((Fetch::File { path, sha }, Origin::Archive));
        }
        if let Some(rest) = entry.id.strip_prefix("registry+") {
            let (origin, package) = rest
                .rsplit_once('#')
                .ok_or_else(|| format!("{} is not a registry identity", entry.id))?;
            let registry = self.config.registry_at(origin).cloned().ok_or_else(|| {
                format!(
                    "{} comes from the registry {origin}, which the packages configuration of \
                     the operator does not list",
                    entry.id
                )
            })?;
            return Ok((
                Fetch::Registry {
                    registry: registry.clone(),
                    package: package.to_string(),
                },
                Origin::Registry(registry),
            ));
        }
        Err(format!("{} is not an identity this unio knows", entry.id).into())
    }

    // ------------------------------------------------------------ workers

    fn new_staging(&self) -> Res<Staging> {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = self
            .cache
            .join("staging")
            .join(format!("{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Staging(Some(dir)))
    }

    /// Runs the unpack worker with these arguments and scopes.
    fn unpack(
        &self,
        arguments: Vec<String>,
        read: Vec<Scope>,
        read_files: Vec<PathBuf>,
        write: Option<&Path>,
        input: Vec<u8>,
    ) -> Res<Report> {
        let mut spec = worker::Spec::for_packages(&self.options).map_err(Fail::Code)?;
        spec.input = input;
        spec.read = read;
        spec.read_files = read_files;
        spec.write = write
            .map(|w| Scope {
                root: unio_runtime::sandbox::resolve(w),
                subtree: true,
            })
            .into_iter()
            .collect();
        let temp = worker::TempDir::new()?;
        let finished =
            worker::start("unpack", &arguments, &spec, &temp, true, &[]).map_err(Fail::Code)?;
        if finished.code != 0 {
            return Err(Fail::Code(finished.code));
        }
        Report::decode(&finished.data)
            .ok_or_else(|| "the unpack worker returned malformed data".into())
    }

    /// Runs the download worker for one URL of a configured registry; returns its download
    /// area and its standard output.
    fn download(
        &self,
        url: &Url,
        mode: &str,
        limit: u64,
        extra: &str,
        input: Vec<u8>,
    ) -> Res<(worker::TempDir, Vec<u8>)> {
        if self.offline {
            return Err(format!(
                "{}{} is needed, and --offline forbids downloading it",
                url.origin(),
                url.path
            )
            .into());
        }
        if !self.config.allows(url) {
            return Err(format!(
                "{} is not the host of a configured registry; the download worker may reach only \
                 those (SB-12)",
                url.origin()
            )
            .into());
        }
        use std::net::ToSocketAddrs;
        let addresses: Vec<std::net::SocketAddr> = (url.host.as_str(), url.port)
            .to_socket_addrs()
            .map_err(|e| format!("{}: {e}", url.origin()))?
            .collect();
        let mut spec = worker::Spec::for_packages(&self.options).map_err(Fail::Code)?;
        spec.input = input;
        // The connection is opened here, to the configured host only, and passed to the
        // worker, which can open none (seccomp and Landlock, Seatbelt, an AppContainer without
        // capabilities).
        let stream = {
            let mut last = None;
            let mut found = None;
            for address in &addresses {
                match std::net::TcpStream::connect_timeout(
                    address,
                    std::time::Duration::from_secs(30),
                ) {
                    Ok(stream) => {
                        found = Some(stream);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            let stream = found.ok_or_else(|| {
                format!(
                    "{}: {}",
                    url.origin(),
                    last.map_or("no address".to_string(), |e| e.to_string())
                )
            })?;
            let timeout = Some(std::time::Duration::from_secs(120));
            let _ = stream.set_read_timeout(timeout);
            let _ = stream.set_write_timeout(timeout);
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                inheritable(stream.as_raw_fd())?;
                spec.socket = Some(stream.as_raw_fd().into());
            }
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawSocket;
                let handle = stream.as_raw_socket() as isize;
                unio_runtime::sandbox::windows::make_inheritable(handle).map_err(|e| {
                    format!("the connection cannot be passed to the download worker: {e}")
                })?;
                spec.socket = Some(handle as i64);
            }
            stream
        };
        let temp = worker::TempDir::new()?;
        let arguments = vec![
            mode.to_string(),
            url.host.clone(),
            url.port.to_string(),
            url.path.clone(),
            limit.to_string(),
            extra.to_string(),
        ];
        let finished = worker::start("download", &arguments, &spec, &temp, true, &[]);
        drop(stream);
        let finished = finished.map_err(Fail::Code)?;
        if finished.code != 0 {
            return Err(Fail::Code(finished.code));
        }
        Ok((temp, finished.data))
    }

    /// Downloads one file of a registry as it is, for a worker to check: the tool never
    /// parses it.
    fn fetch_raw(&self, url: &Url, limit: u64, name: &str) -> Res<Vec<u8>> {
        let (area, _) = self.download(url, "get", limit, name, Vec::new())?;
        std::fs::read(area.path().join(name))
            .map_err(|e| Fail::Message(format!("the download of {}: {e}", url.path)))
    }

    /// The trust bundle of a registry (T-8a): the operator's anchor and the registry's
    /// endorsements and revocation list, with the floor of its sequence. The workers verify it.
    fn bundle(&mut self, registry: &Registry) -> Res<Vec<u8>> {
        let canonical = registry.url.canonical();
        if let Some(bundle) = self.bundles.get(&canonical) {
            return Ok(bundle.clone());
        }
        let anchor = std::fs::read(&registry.anchor).map_err(|e| {
            format!(
                "the trust anchor of the registry '{}' ({}): {e}; the operator provisions it \
                 (packages.md, section 5)",
                registry.name,
                registry.anchor.display()
            )
        })?;
        let keys = self.fetch_raw(
            &registry.url.join("trust/keys")?,
            trust::MAX_KEYS_FILE,
            "keys",
        )?;
        let revocations = self.fetch_raw(
            &registry.url.join("trust/revocations")?,
            trust::MAX_REVOCATIONS_FILE,
            "revocations",
        )?;
        let bundle = Bundle {
            anchor,
            keys,
            revocations,
            revocations_floor: self.state.get(&format!("revocations {canonical}")),
        }
        .encode();
        self.bundles.insert(canonical, bundle.clone());
        Ok(bundle)
    }

    fn index(&mut self, id: &str, registry: &Registry, package: &str) -> Res<Index> {
        if let Some(v) = self.indexes.get(id) {
            return Ok(v.index.clone());
        }
        let bundle = self.bundle(registry)?;
        let url = registry.url.join(&format!("index/{package}.json"))?;
        let signature = self.fetch_raw(
            &registry.url.join(&format!("index/{package}.json.sig"))?,
            SIGNATURE_LIMIT,
            "index-signature",
        )?;
        let locked = self.old.as_ref().and_then(|l| l.entry(id)).cloned();
        let floor = self
            .state
            .get(&format!("index {id}"))
            .max(locked.as_ref().and_then(|e| e.index_sequence).unwrap_or(0));
        let input = trust::frame(&[&bundle, &signature, floor.to_string().as_bytes()]);
        let (_temp, data) = self.download(&url, "index", INDEX_LIMIT, package, input)?;
        let text = String::from_utf8(data)
            .map_err(|_| "the download worker returned malformed data".to_string())?;
        let mut lines = text.splitn(4, '\n');
        let malformed = || Fail::from("the download worker returned malformed data");
        let mut field = |name: &str| -> Res<String> {
            lines
                .next()
                .and_then(|l| l.strip_prefix(name))
                .map(str::to_string)
                .ok_or_else(malformed)
        };
        let digest = field("digest ")?;
        let _signer = field("signer ")?;
        let revocations: u64 = field("revocations ")?.parse().map_err(|_| malformed())?;
        let index = Index::decode(lines.next().unwrap_or(""))?;
        // A published version is never removed or changed (T-8): the locked one must still be
        // there with the same archive.
        if let Some(entry) = &locked {
            let same = index.versions.iter().any(|v| {
                v.version.to_string() == entry.version
                    && entry.archive.as_deref().and_then(lock::digest_hex)
                        == Some(v.sha384.as_str())
            });
            if !same {
                return Err(format!(
                    "the registry index of {package} no longer lists the locked version {} with \
                     the archive unio.lock records: a published version was removed or replaced, \
                     which a registry never does (T-8); refused",
                    entry.version
                )
                .into());
            }
        }
        self.seen.raise(
            format!("revocations {}", registry.url.canonical()),
            revocations,
        );
        self.seen.raise(format!("index {id}"), index.sequence);
        self.indexes.insert(
            id.to_string(),
            Verified {
                index: index.clone(),
                digest,
            },
        );
        Ok(index)
    }

    /// Unpacks a package that is not from a registry into a staging directory.
    fn stage(&mut self, id: &str) -> Res<Report> {
        if let Some(s) = self.staged.get(id) {
            return Ok(s.report.clone());
        }
        let fetch = self
            .fetch
            .get(id)
            .cloned()
            .ok_or_else(|| format!("{id} has no known source"))?;
        let staging = self.new_staging()?;
        let dir = staging.0.clone().unwrap();
        let limits = limits_text(&self.config.limits);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let (report, fetch_url) = match &fetch {
            Fetch::Dir(source) => {
                if !source.join("unio.toml").is_file() {
                    return Err(format!("{id}: {} has no unio.toml", source.display()).into());
                }
                let read = vec![Scope {
                    root: unio_runtime::sandbox::resolve(source),
                    subtree: true,
                }];
                let args = vec!["dir".into(), s(source), s(&dir), limits];
                (
                    self.unpack(args, read, Vec::new(), Some(&dir), Vec::new())?,
                    None,
                )
            }
            Fetch::File { path, sha } => {
                if !path.is_file() {
                    return Err(
                        format!("{id}: the archive {} does not exist", path.display()).into(),
                    );
                }
                let file = unio_runtime::sandbox::resolve(path);
                let args = archive_args(&file, sha, &dir, &limits, None);
                let report = self.unpack(args, Vec::new(), vec![file], Some(&dir), Vec::new())?;
                (report, None)
            }
            Fetch::Url { url, sha } => {
                let (area, _) = self.download(
                    url,
                    "get",
                    self.config.limits.archive_bytes,
                    "archive",
                    Vec::new(),
                )?;
                let file = area.path().join("archive");
                let args = archive_args(&file, sha, &dir, &limits, None);
                let report = self.unpack(args, Vec::new(), vec![file], Some(&dir), Vec::new())?;
                (report, Some(format!("{}{}", url.origin(), url.path)))
            }
            Fetch::Registry { .. } => {
                return Err(format!("{id}: a registry package is staged by its version").into())
            }
        };
        self.staged.insert(
            id.to_string(),
            Staged {
                report: report.clone(),
                staging,
                fetch: fetch_url,
                signature: None,
                index: None,
                index_sequence: None,
            },
        );
        Ok(report)
    }

    /// Downloads and unpacks one version of a registry package into a staging directory, and
    /// checks that the archive is what its index entry says.
    fn stage_registry(&mut self, id: &str, version: &str, locked: Option<&Entry>) -> Res<Report> {
        if let Some(s) = self.staged.get(id) {
            return Ok(s.report.clone());
        }
        let Some(Fetch::Registry { registry, package }) = self.fetch.get(id).cloned() else {
            return Err(format!("{id} is not a registry package").into());
        };
        // From the lock: its URLs and hash. Otherwise from the verified index.
        let (archive_url, sha, signature_url, index_digest, index_sequence, deps) = match locked {
            Some(entry) => {
                let fetch = entry
                    .fetch
                    .as_deref()
                    .ok_or_else(|| format!("{id} has no archive URL in the lock"))?;
                let sha = entry
                    .archive
                    .as_deref()
                    .and_then(lock::digest_hex)
                    .ok_or_else(|| format!("{id} has no archive hash in the lock"))?;
                let signature = entry.signature.as_deref().ok_or_else(|| {
                    format!(
                        "{id} has no signature URL in the lock (a lock of an unverified \
                         package); run `unio update` to lock it with its signature"
                    )
                })?;
                (
                    Url::parse(fetch)?,
                    sha.to_string(),
                    Url::parse(signature)?,
                    entry.index.clone(),
                    entry.index_sequence,
                    None,
                )
            }
            None => {
                let index = self.index(id, &registry, &package)?;
                let digest = self.indexes.get(id).map(|v| v.digest.clone());
                let v = index
                    .versions
                    .iter()
                    .find(|v| v.version.to_string() == version)
                    .ok_or_else(|| format!("{id} has no version {version} in its index"))?;
                let sequence = Some(index.sequence);
                (
                    registry.url.join(&v.archive)?,
                    v.sha384.clone(),
                    registry.url.join(&v.signature)?,
                    digest,
                    sequence,
                    Some(v.dependencies.clone()),
                )
            }
        };
        let bundle = self.bundle(&registry)?;
        let (area, _) = self.download(
            &archive_url,
            "get",
            self.config.limits.archive_bytes,
            "archive",
            Vec::new(),
        )?;
        let file = area.path().join("archive");
        let signature = self.fetch_raw(&signature_url, SIGNATURE_LIMIT, "signature")?;
        let staging = self.new_staging()?;
        let dir = staging.0.clone().unwrap();
        let limits = limits_text(&self.config.limits);
        let canonical = registry.url.canonical();
        let args = archive_args(
            &file,
            &sha,
            &dir,
            &limits,
            Some((&canonical, &package, version)),
        );
        let input = trust::frame(&[&bundle, &signature]);
        let report = self.unpack(args, Vec::new(), vec![file], Some(&dir), input)?;
        self.seen
            .raise(format!("revocations {canonical}"), report.revocations);
        let short = package.rsplit('/').next().unwrap_or(&package);
        if report.name != short || report.version != version {
            return Err(format!(
                "the archive of {package} {version} holds the package {} {}, not what the \
                 registry index says",
                report.name, report.version
            )
            .into());
        }
        if let Some(deps) = deps {
            let from_index: BTreeSet<(String, String, String)> = deps
                .iter()
                .map(|d| {
                    (
                        d.alias.clone(),
                        d.package.clone(),
                        d.req.text().replace(' ', ""),
                    )
                })
                .collect();
            let from_archive: BTreeSet<(String, String, String)> = report
                .dependencies
                .iter()
                .filter(|(_, kind, a, ..)| *kind == 1 && a.is_empty())
                .map(|(alias, _, _, b, req)| (alias.clone(), b.clone(), req.clone()))
                .collect();
            if from_index != from_archive || report.dependencies.len() != from_archive.len() {
                return Err(format!(
                    "the dependencies in the archive of {package} {version} differ from its \
                     registry index entry; refused"
                )
                .into());
            }
        }
        self.staged.insert(
            id.to_string(),
            Staged {
                report: report.clone(),
                staging,
                fetch: Some(format!("{}{}", archive_url.origin(), archive_url.path)),
                signature: Some(format!("{}{}", signature_url.origin(), signature_url.path)),
                index: index_digest,
                index_sequence,
            },
        );
        Ok(report)
    }

    // ------------------------------------------------------------ resolution

    fn frozen(&mut self) -> Res<BTreeMap<String, Candidate>> {
        let Some(old) = self.old.clone() else {
            return Ok(BTreeMap::new());
        };
        let mut frozen = BTreeMap::new();
        for entry in &old.packages {
            let deps = entry
                .dependencies
                .iter()
                .map(|e| {
                    Ok(Requirement {
                        alias: e.alias.clone(),
                        id: e.id.clone(),
                        req: if e.req.is_empty() {
                            None
                        } else {
                            Some(Req::parse(&e.req)?)
                        },
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            frozen.insert(
                entry.id.clone(),
                Candidate {
                    version: Version::parse(&entry.version)?,
                    deps,
                },
            );
        }
        Ok(frozen)
    }

    /// Resolves the manifest with the packages of `unlocked` (or every package) free and the
    /// others kept at their locked versions, and returns the new lock and the packages that
    /// are new or changed.
    fn resolve(&mut self, unlocked: Option<BTreeSet<String>>) -> Res<(Lock, Vec<String>)> {
        let deps = self.manifest.dependencies.clone();
        let root = self.requirements(&Origin::Root, &deps)?;
        // The fetch of locked packages, for those not in the cache.
        if let Some(old) = self.old.clone() {
            for entry in &old.packages {
                if !self.fetch.contains_key(&entry.id) {
                    if let Ok((fetch, origin)) = self.fetch_of_entry(entry) {
                        self.fetch.insert(entry.id.clone(), fetch);
                        self.origins.insert(entry.id.clone(), origin);
                    }
                }
            }
        }
        let frozen = self.frozen()?;
        let unlocked = unlocked.unwrap_or_else(|| {
            frozen
                .keys()
                .chain(root.iter().map(|r| &r.id))
                .cloned()
                .collect()
        });
        let aliases: BTreeMap<String, String> = root
            .iter()
            .map(|r| (r.id.clone(), r.alias.clone()))
            .collect();
        let chosen = resolve::resolve(&root, self, &frozen, &unlocked, &aliases);
        if let Some(failure) = self.failure.take() {
            return Err(failure);
        }
        let chosen = chosen?;
        let old = self.old.clone();
        let mut lock = Lock {
            root: root
                .iter()
                .map(|r| Edge {
                    alias: r.alias.clone(),
                    req: req_text(&r.req),
                    id: r.id.clone(),
                })
                .collect(),
            packages: Vec::new(),
        };
        let mut changed = Vec::new();
        for (id, candidate) in &chosen {
            let version = candidate.version.to_string();
            let previous = old.as_ref().and_then(|l| l.entry(id));
            let staged = self.staged.contains_key(id);
            let kept = previous.filter(|p| p.version == version && !staged);
            let entry = match kept {
                Some(previous) => previous.clone(),
                None => {
                    if matches!(self.fetch.get(id), Some(Fetch::Registry { .. })) {
                        self.stage_registry(id, &version, None)?;
                    }
                    let staged = self
                        .staged
                        .get(id)
                        .ok_or_else(|| format!("{id} was chosen but not fetched"))?;
                    let r = &staged.report;
                    let entry = Entry {
                        id: id.clone(),
                        name: r.name.clone(),
                        version: r.version.clone(),
                        content: r.content.clone(),
                        archive: (!r.archive.is_empty()).then(|| r.archive.clone()),
                        fetch: staged.fetch.clone(),
                        signature: staged.signature.clone(),
                        index: staged.index.clone(),
                        index_sequence: staged.index_sequence,
                        signer: (!r.signer.is_empty()).then(|| r.signer.clone()),
                        unsafe_modules: r.unsafe_modules(),
                        permissions: r.permissions.clone(),
                        dependencies: candidate
                            .deps
                            .iter()
                            .map(|d| Edge {
                                alias: d.alias.clone(),
                                req: req_text(&d.req),
                                id: d.id.clone(),
                            })
                            .collect(),
                    };
                    if previous.is_none_or(|p| p.content != entry.content) {
                        changed.push(id.clone());
                    }
                    entry
                }
            };
            lock.packages.push(entry);
        }
        lock.normalize();
        Ok((lock, changed))
    }

    // ------------------------------------------------------------ installation

    /// Makes sure every package of the lock is in the cache or staged, verified against the
    /// lock, and returns the reports of the packages to review.
    fn obtain(&mut self, lock: &Lock, review_all: bool, changed: &[String]) -> Res<Vec<String>> {
        let mut to_review = Vec::new();
        for entry in &lock.packages {
            let in_cache = unio::package::entry_dir(&self.cache, &entry.content)
                .is_some_and(|d| d.join("unio.toml").is_file());
            let report = if self.staged.contains_key(&entry.id) {
                self.staged[&entry.id].report.clone()
            } else if in_cache {
                if !review_all {
                    continue;
                }
                // Read again in a worker: it checks the entry against its content hash and
                // gives what the review shows.
                let dir = unio::package::entry_dir(&self.cache, &entry.content).unwrap();
                let read = vec![Scope {
                    root: unio_runtime::sandbox::resolve(&dir),
                    subtree: true,
                }];
                let args = vec![
                    "inspect".into(),
                    dir.to_string_lossy().into_owned(),
                    limits_text(&self.config.limits),
                ];
                self.unpack(args, read, Vec::new(), None, Vec::new())?
            } else {
                if !self.fetch.contains_key(&entry.id) {
                    let (fetch, origin) = self.fetch_of_entry(entry)?;
                    self.fetch.insert(entry.id.clone(), fetch);
                    self.origins.insert(entry.id.clone(), origin);
                }
                if matches!(self.fetch.get(&entry.id), Some(Fetch::Registry { .. })) {
                    self.stage_registry(&entry.id, &entry.version, Some(entry))?
                } else {
                    self.stage(&entry.id)?
                }
            };
            if report.content != entry.content {
                let alias = lock
                    .root
                    .iter()
                    .find(|e| e.id == entry.id)
                    .map_or(entry.id.clone(), |e| e.alias.clone());
                return Err(format!(
                    "{} {} does not have the content that unio.lock records ({} instead of {}); \
                     it changed since it was locked or the cache entry is damaged; run \
                     `unio update {alias}` to lock its new content",
                    entry.id, entry.version, report.content, entry.content
                )
                .into());
            }
            if report.version != entry.version {
                return Err(format!(
                    "{} is version {} instead of the locked {}",
                    entry.id, report.version, entry.version
                )
                .into());
            }
            // The target and runtime requirements of the package hold here (T-14).
            report.requirements_met().map_err(|problem| {
                format!(
                    "{} {} cannot be installed: {problem}; nothing was installed",
                    entry.id, entry.version
                )
            })?;
            if review_all || changed.contains(&entry.id) {
                to_review.push(entry.id.clone());
            }
            self.reports.insert(entry.id.clone(), report);
        }
        Ok(to_review)
    }

    /// The permissions a dependency declares that this package does not: entries of
    /// `files.read` and `files.write` not covered by an equal entry or a `/**` subtree of the
    /// root, and names of `env` and `process` the root does not list (T-9).
    fn wider(&self, report: &Report) -> Vec<String> {
        let p = &self.manifest.permissions;
        let covered = |list: &[String], entry: &str| {
            list.iter().any(|r| {
                r == entry
                    || r.strip_suffix("/**")
                        .is_some_and(|base| entry == base || entry.starts_with(&format!("{base}/")))
            })
        };
        report
            .permissions
            .iter()
            .filter(|line| {
                let (kind, entry) = line.split_once(' ').unwrap_or((line, ""));
                match kind {
                    "files.read" => !covered(&p.read, entry),
                    "files.write" => !covered(&p.write, entry),
                    "env" => !p.env.iter().any(|e| e == entry),
                    "process" => !p.process.iter().any(|e| e == entry),
                    _ => true,
                }
            })
            .cloned()
            .collect()
    }

    /// The review of T-9 for the packages that need it, and the confirmation. `Ok(())` when
    /// nothing needs it or it was confirmed.
    fn review(&self, lock: &Lock, ids: &[String]) -> Res<()> {
        let mut needed = false;
        for id in ids {
            let (Some(report), Some(entry)) = (self.reports.get(id), lock.entry(id)) else {
                continue;
            };
            let unsafe_modules = report.unsafe_modules();
            let wider = self.wider(report);
            let listed: BTreeSet<&String> = report.unsafe_listed.iter().collect();
            let marked: BTreeSet<&String> = report.unsafe_marked.iter().collect();
            for m in marked.difference(&listed) {
                eprintln!(
                    "unio: warning: {}@{}: the module {m} carries `unsafe module` but is not \
                     in package.unsafe_modules of its unio.toml (T-6)",
                    entry.name, entry.version
                );
            }
            for m in listed.difference(&marked) {
                eprintln!(
                    "unio: warning: {}@{}: package.unsafe_modules lists {m}, which does not \
                     carry `unsafe module` (T-6)",
                    entry.name, entry.version
                );
            }
            if unsafe_modules.is_empty() && wider.is_empty() {
                continue;
            }
            needed = true;
            eprintln!(
                "unio: the dependency {}@{} ({id}) needs your review before it is installed \
                 (T-9):",
                entry.name, entry.version
            );
            if !unsafe_modules.is_empty() {
                eprintln!(
                    "  modules marked `unsafe module`, listed for review: {}",
                    unsafe_modules.join(", ")
                );
                eprintln!(
                    "    In Unio 0.1 these modules contain no external functions (E0086 rejects \
                     them) and receive no wider permissions (RFC 0004)."
                );
            }
            if !wider.is_empty() {
                eprintln!(
                    "  permissions it declares beyond this package's own: {}",
                    wider.join(", ")
                );
                eprintln!(
                    "    A dependency runs with the permissions of this package only: its own \
                     [permissions] are shown for review and grant nothing."
                );
            }
            eprintln!("  Confirming asks for this review; it grants nothing.");
        }
        if !needed {
            return Ok(());
        }
        if self.unsafe_ok {
            eprintln!("unio: confirmed with --unsafe-ok; nothing is granted by it");
            return Ok(());
        }
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprint!("Type yes to confirm that you reviewed them: ");
            let mut answer = String::new();
            let _ = std::io::stdin().read_line(&mut answer);
            if answer.trim() == "yes" {
                return Ok(());
            }
        }
        Err(
            "the installation needs the confirmation of T-9: review the modules and permissions \
             above, then run again with --unsafe-ok (it grants nothing); nothing was changed"
                .into(),
        )
    }

    /// Turns the staging directories of the lock's packages into cache entries.
    fn commit(&mut self, lock: &Lock) -> Res<()> {
        for entry in &lock.packages {
            let Some(staged) = self.staged.get_mut(&entry.id) else {
                continue;
            };
            let target = unio::package::entry_dir(&self.cache, &entry.content)
                .ok_or("a content hash of the lock is malformed")?;
            if target.join("unio.toml").is_file() {
                continue;
            }
            let _ = std::fs::remove_dir_all(&target);
            let from = staged
                .staging
                .0
                .take()
                .ok_or("a staging directory is gone")?;
            if let Err(e) = std::fs::rename(&from, &target) {
                let _ = std::fs::remove_dir_all(&from);
                return Err(format!("{}: {e}", target.display()).into());
            }
        }
        Ok(())
    }

    /// Records the sequences this command saw as the new floors of this machine.
    fn save_state(&self) -> Res<()> {
        if self.seen != self.state {
            self.seen.save(&self.cache)?;
        }
        Ok(())
    }

    fn write_lock(&self, lock: &Lock) -> Res<()> {
        write_atomically(&self.root.join(lock::FILE), &lock.write())?;
        Ok(())
    }
}

fn archive_args(
    file: &Path,
    sha: &str,
    staging: &Path,
    limits: &str,
    registry: Option<(&str, &str, &str)>,
) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let (registry, package, version) = registry.unwrap_or(("-", "-", "-"));
    vec![
        "archive".into(),
        s(file),
        sha.to_string(),
        s(staging),
        limits.to_string(),
        package.to_string(),
        version.to_string(),
        registry.to_string(),
    ]
}

impl Provider for Session {
    fn candidates(&mut self, id: &str) -> Result<Vec<Candidate>, String> {
        let result = self.provide(id);
        match result {
            Ok(c) => Ok(c),
            Err(Fail::Message(m)) => Err(m),
            Err(code) => {
                self.failure = Some(code);
                Err("a worker failed".into())
            }
        }
    }
}

impl Session {
    fn provide(&mut self, id: &str) -> Res<Vec<Candidate>> {
        let fetch = self
            .fetch
            .get(id)
            .cloned()
            .ok_or_else(|| format!("{id} has no known source"))?;
        let origin = self.origins.get(id).cloned().unwrap_or(Origin::Archive);
        if let Fetch::Registry { registry, package } = fetch {
            let index = self.index(id, &registry, &package)?;
            let mut out = Vec::new();
            for v in &index.versions {
                let deps: Vec<Dependency> = v
                    .dependencies
                    .iter()
                    .map(|d| Dependency {
                        alias: d.alias.clone(),
                        source: Source::Registry {
                            registry: None,
                            package: d.package.clone(),
                        },
                        req: Some(d.req.clone()),
                    })
                    .collect();
                out.push(Candidate {
                    version: v.version.clone(),
                    deps: self.requirements(&origin, &deps)?,
                });
            }
            return Ok(out);
        }
        let report = self.stage(id)?;
        let deps = report.manifest_dependencies()?;
        Ok(vec![Candidate {
            version: Version::parse(&report.version)?,
            deps: self.requirements(&origin, &deps)?,
        }])
    }
}

// ---------------------------------------------------------------- the commands

struct Options {
    sandbox: sandbox_cli::Options,
    offline: bool,
    unsafe_ok: bool,
    positional: Vec<String>,
    named: BTreeMap<String, String>,
}

fn options(arguments: &[String], with_values: &[&str]) -> Result<Options, String> {
    let (sandbox, rest) = sandbox_cli::take(arguments)?;
    let mut o = Options {
        sandbox,
        offline: false,
        unsafe_ok: false,
        positional: Vec::new(),
        named: BTreeMap::new(),
    };
    let mut i = 0;
    while i < rest.len() {
        let a = &rest[i];
        if a == "--offline" {
            o.offline = true;
        } else if a == "--unsafe-ok" {
            o.unsafe_ok = true;
        } else if let Some(name) = a.strip_prefix("--") {
            let (name, value) = match name.split_once('=') {
                Some((n, v)) => (n.to_string(), v.to_string()),
                None => {
                    i += 1;
                    let value = rest
                        .get(i)
                        .ok_or_else(|| format!("--{name} needs a value"))?;
                    (name.to_string(), value.clone())
                }
            };
            if !with_values.contains(&name.as_str()) {
                return Err(format!("unknown option '--{name}'"));
            }
            o.named.insert(name, value);
        } else {
            o.positional.push(a.clone());
        }
        i += 1;
    }
    Ok(o)
}

/// `unio add`, `remove`, `update`, and `install`.
pub fn command(name: &str, arguments: &[String]) -> ExitCode {
    let with_values: &[&str] = if name == "add" {
        &["path", "archive", "sha384", "registry", "package"]
    } else {
        &[]
    };
    let options = match options(arguments, with_values) {
        Ok(o) => o,
        Err(problem) => {
            eprintln!("unio {name}: {problem}");
            return ExitCode::from(2);
        }
    };
    let result = match name {
        "add" => add(options),
        "remove" => remove(options),
        "update" => update(options),
        _ => install(options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fail::Code(code)) => ExitCode::from(code),
        Err(Fail::Message(message)) => {
            eprintln!("unio {name}: error: {message}");
            ExitCode::from(1)
        }
    }
}

fn open(o: &Options) -> Res<Session> {
    Session::open(o.sandbox.clone(), o.offline, o.unsafe_ok)
}

/// Resolves, fetches, reviews, and writes: the common end of `add`, `remove`, and `update`.
fn settle(
    s: &mut Session,
    unlocked: Option<BTreeSet<String>>,
    manifest: Option<String>,
) -> Res<()> {
    root_requirements(s)?;
    let (lock, changed) = s.resolve(unlocked)?;
    let to_review = s.obtain(&lock, false, &changed)?;
    s.review(&lock, &to_review)?;
    s.commit(&lock)?;
    if let Some(text) = manifest {
        write_atomically(&s.root.join("unio.toml"), &text)?;
    }
    s.write_lock(&lock)?;
    s.save_state()?;
    for id in &changed {
        if let Some(e) = lock.entry(id) {
            eprintln!("unio: locked {} {} ({id})", e.name, e.version);
        }
    }
    Ok(())
}

/// The target and runtime requirements of the root package hold on this machine (T-14).
fn root_requirements(s: &Session) -> Res<()> {
    s.manifest.requirements_met().map_err(|problem| {
        Fail::Message(format!(
            "the package {} cannot be installed here: {problem}",
            s.root.join("unio.toml").display()
        ))
    })
}

fn add(o: Options) -> Res<()> {
    let (alias, version) = match o.positional.as_slice() {
        [alias] => (alias.clone(), None),
        [alias, version] => (alias.clone(), Some(version.clone())),
        _ => return Err("usage: unio add ALIAS [VERSION] (--path DIR | --package OWNER/NAME [--registry NAME] | --archive FILE|URL --sha384 HEX)".into()),
    };
    if !manifest::valid_alias(&alias) {
        return Err(
            format!("'{alias}' is not a valid alias: lowercase letters, digits and '_'").into(),
        );
    }
    let mut s = open(&o)?;
    if s.manifest.dependency(&alias).is_some() {
        return Err(format!(
            "'{alias}' is already a dependency; use `unio update {alias}` or `unio remove {alias}`"
        )
        .into());
    }
    let q = edit::quote;
    let mut fields = Vec::new();
    let n = &o.named;
    match (n.get("path"), n.get("package"), n.get("archive")) {
        (Some(path), None, None) if !n.contains_key("registry") && !n.contains_key("sha384") => {
            fields.push(format!("path = {}", q(path)));
        }
        (None, Some(package), None) if !n.contains_key("sha384") => {
            let registry = n.get("registry").cloned().unwrap_or_else(|| "default".into());
            fields.push(format!("registry = {}", q(&registry)));
            fields.push(format!("package = {}", q(package)));
            if version.is_none() {
                return Err("a registry dependency needs a VERSION requirement, such as ^1.0".into());
            }
        }
        (None, None, Some(archive)) if !n.contains_key("registry") => {
            let sha = n
                .get("sha384")
                .ok_or("--archive needs --sha384, the hash of the archive")?;
            fields.push(format!("archive = {}", q(archive)));
            fields.push(format!("sha384 = {}", q(sha)));
        }
        _ => {
            return Err("give one source: --path DIR, --package OWNER/NAME [--registry NAME], or --archive FILE|URL --sha384 HEX".into())
        }
    }
    if let Some(v) = &version {
        fields.push(format!("version = {}", q(v)));
    }
    let text = edit::add(
        &s.manifest_text,
        &alias,
        &format!("{{ {} }}", fields.join(", ")),
    );
    s.manifest = Manifest::parse(&text).map_err(|e| format!("the new dependency: {e}"))?;
    let dep = s.manifest.dependency(&alias).cloned().unwrap();
    let (id, ..) = s.identify(&Origin::Root, &dep)?;
    let unlocked: BTreeSet<String> = [id].into();
    settle(&mut s, Some(unlocked), Some(text))
}

fn remove(o: Options) -> Res<()> {
    let [alias] = o.positional.as_slice() else {
        return Err("usage: unio remove ALIAS".into());
    };
    let mut s = open(&o)?;
    let text = edit::remove(&s.manifest_text, alias)
        .ok_or_else(|| format!("'{alias}' is not a dependency in unio.toml"))?;
    s.manifest = Manifest::parse(&text)?;
    settle(&mut s, Some(BTreeSet::new()), Some(text))
}

fn update(o: Options) -> Res<()> {
    let mut s = open(&o)?;
    let unlocked = if o.positional.is_empty() {
        None
    } else {
        let mut ids = BTreeSet::new();
        for alias in &o.positional {
            let dep = s
                .manifest
                .dependency(alias)
                .cloned()
                .ok_or_else(|| format!("'{alias}' is not a dependency in unio.toml"))?;
            ids.insert(s.identify(&Origin::Root, &dep)?.0);
        }
        Some(ids)
    };
    settle(&mut s, unlocked, None)
}

/// `unio install`: the exact packages of `unio.lock` into the cache. It never resolves and
/// never changes the lock; a lock that does not match the manifest is an error.
fn install(o: Options) -> Res<()> {
    if !o.positional.is_empty() {
        return Err("usage: unio install [--offline] [--unsafe-ok]".into());
    }
    let mut s = open(&o)?;
    root_requirements(&s)?;
    let lock = s
        .old
        .clone()
        .ok_or("there is no unio.lock; run `unio update` to make one")?;
    // The lock must match the manifest exactly; it is never updated silently.
    let mut mismatch = Vec::new();
    let mut aliases = BTreeSet::new();
    for dep in s.manifest.dependencies.clone() {
        aliases.insert(dep.alias.clone());
        let (id, fetch, origin) = s.identify(&Origin::Root, &dep)?;
        let edge = lock.root.iter().find(|e| e.alias == dep.alias);
        match edge {
            Some(e) if e.id == id && e.req == req_text(&dep.req) => {
                s.register(&id, fetch, origin)?;
            }
            Some(e) if e.id != id => mismatch.push(format!(
                "'{}' is {id} in unio.toml but {} in unio.lock",
                dep.alias, e.id
            )),
            Some(_) => mismatch.push(format!(
                "the version requirement of '{}' changed",
                dep.alias
            )),
            None => mismatch.push(format!("'{}' is not locked", dep.alias)),
        }
    }
    for e in &lock.root {
        if !aliases.contains(&e.alias) {
            mismatch.push(format!("'{}' is locked but not in unio.toml", e.alias));
        }
    }
    // Every edge holds for the locked versions.
    for (from, edge) in lock
        .root
        .iter()
        .map(|e| ("the root package".to_string(), e))
        .chain(
            lock.packages
                .iter()
                .flat_map(|p| p.dependencies.iter().map(move |e| (p.id.clone(), e))),
        )
    {
        if edge.req.is_empty() {
            continue;
        }
        let entry = lock.entry(&edge.id).expect("checked by the parser");
        let version = Version::parse(&entry.version)?;
        if !Req::parse(&edge.req)?.matches(&version) {
            mismatch.push(format!(
                "{from} requires {} {} but {} is locked",
                edge.id, edge.req, entry.version
            ));
        }
    }
    if !mismatch.is_empty() {
        return Err(format!(
            "unio.lock does not match unio.toml: {}; run `unio update` to resolve again",
            mismatch.join("; ")
        )
        .into());
    }
    let to_review = s.obtain(&lock, true, &[])?;
    s.review(&lock, &to_review)?;
    s.commit(&lock)?;
    s.save_state()?;
    eprintln!(
        "unio: {} locked package(s) installed in {}",
        lock.packages.len(),
        s.cache.display()
    );
    Ok(())
}
