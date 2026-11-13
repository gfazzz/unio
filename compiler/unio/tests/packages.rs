//! Packages (T-6 to T-9, T-14) and the workers of package operations (SB-12), with the archive
//! cases of SB-15 and SB-14: dependencies from a directory, an archive, and a registry served by
//! a static HTTP server on 127.0.0.1; resolution and lock determinism; installation from the
//! lock without the network; hash mismatches; hostile archives refused by the unpack worker;
//! the download worker that cannot reach any other host; the review of T-9, which grants
//! nothing; and no code of a package run during installation or a check.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use unio::envelope::SecretKey;
use unio::package::archive::{self, Entries, Item};
use unio::package::registry;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn all(output: &Output) -> String {
    text(&output.stdout) + &text(&output.stderr)
}

fn sha(bytes: &[u8]) -> String {
    unio::sha::sha384_hex(bytes)
}

// ---------------------------------------------------------------- a static registry

/// A static file server on 127.0.0.1 over a registry directory (index JSON, archives,
/// signatures, trust files), as any static server would serve it. The registry is made with
/// the library of `unio publish`; its trust anchor and an endorsed signing key are fixed test
/// keys.
struct Server {
    port: u16,
    root: PathBuf,
    anchor: SecretKey,
    signer: SecretKey,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

static SERVERS: AtomicU32 = AtomicU32::new(0);

impl Server {
    fn start() -> Server {
        let n = SERVERS.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("unio-registry-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let anchor = SecretKey::from_seed(&[1; 32]);
        let signer = SecretKey::from_seed(&[2; 32]);
        registry::init(&root, &anchor).unwrap();
        registry::endorse(&root, &anchor, &signer.public(), None).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (dir, r, s) = (root.clone(), requests.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if s.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                r.lock().unwrap().push(path.clone());
                let body = path
                    .strip_prefix("/reg/")
                    .filter(|p| !p.split('/').any(|c| c == ".."))
                    .and_then(|p| fs::read(dir.join(p)).ok());
                let response = match body {
                    Some(body) => {
                        let mut out = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        out.extend_from_slice(&body);
                        out
                    }
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&response);
            }
        });
        Server {
            port,
            root,
            anchor,
            signer,
            requests,
            stop,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/reg", self.port)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn put(&self, relative: &str, body: Vec<u8>) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn get(&self, relative: &str) -> Vec<u8> {
        fs::read(self.path(relative)).unwrap()
    }

    /// Publishes versions of `owner/name` with the signing key: (version, files,
    /// dependencies as (alias, package, requirement)).
    #[allow(clippy::type_complexity)]
    fn publish(
        &self,
        package: &str,
        versions: &[(&str, Vec<(&str, &str)>, Vec<(&str, &str, &str)>)],
    ) {
        self.publish_with(&self.signer, package, versions);
    }

    #[allow(clippy::type_complexity)]
    fn publish_with(
        &self,
        key: &SecretKey,
        package: &str,
        versions: &[(&str, Vec<(&str, &str)>, Vec<(&str, &str, &str)>)],
    ) {
        let (owner, name) = package.split_once('/').unwrap();
        for (version, files, deps) in versions {
            let dir = self.root.with_extension(format!("src-{name}-{version}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let mut manifest = format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n");
            if !deps.is_empty() {
                manifest.push_str("\n[dependencies]\n");
                for (alias, p, req) in deps {
                    manifest.push_str(&format!(
                        "{alias} = {{ package = \"{p}\", version = \"{req}\" }}\n"
                    ));
                }
            }
            fs::write(dir.join("unio.toml"), manifest).unwrap();
            for (file, text) in files {
                fs::write(dir.join(file), text).unwrap();
            }
            registry::publish(&self.root, owner, &dir, key).unwrap();
            fs::remove_dir_all(&dir).unwrap();
        }
    }

    fn requested(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A ustar archive of the files, as `tar --format=ustar` makes one.
fn tar(files: &[(&str, String)]) -> Vec<u8> {
    let mut entries = Entries::new();
    for (name, text) in files {
        entries.insert(name.to_string(), Item::File(text.as_bytes().to_vec()));
    }
    archive::write(&entries)
}

/// An archive of raw entries: (name, type, data, link).
fn raw_tar(entries: &[(&str, u8, &[u8], &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, kind, data, link) in entries {
        out.extend_from_slice(&archive::header(name, *kind, data.len() as u64, link));
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

// ---------------------------------------------------------------- a project

struct Project {
    root: PathBuf,
}

const MAIN: &str = "from deps.util.strings import shout\n\npublic function main() returns Int:\n    print(shout(\"hello\"))\n    return 0\n";

impl Project {
    fn new(name: &str) -> Project {
        let dir = std::env::temp_dir().join(format!("unio-packages-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("cache")).unwrap();
        fs::create_dir_all(dir.join("home")).unwrap();
        let root = fs::canonicalize(dir).unwrap();
        let p = Project { root };
        p.write(
            "app/unio.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        );
        p.write("app/main.unio", MAIN);
        p.config("");
        p
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path(relative)).unwrap_or_default()
    }

    /// The packages configuration of the operator.
    fn config(&self, body: &str) {
        self.write("packages.toml", body);
    }

    /// Configures the registry of the server as `main`, with its trust anchor provisioned by
    /// the operator next to the configuration.
    fn use_registry(&self, server: &Server) {
        self.write("anchor.pub", &server.anchor.public().to_file());
        self.config(&format!(
            "[registry.main]\nurl = \"{}\"\nanchor = \"anchor.pub\"\n",
            server.url()
        ));
    }

    /// A package `util` in `dir` with `strings.shout`.
    fn util(&self, dir: &str, version: &str) {
        self.write(
            &format!("{dir}/unio.toml"),
            &format!("[package]\nname = \"util\"\nversion = \"{version}\"\n"),
        );
        self.write(
            &format!("{dir}/strings.unio"),
            "public function shout(text as Text) returns Text:\n    return text.to_upper() + \"!\"\n",
        );
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
        command
            .current_dir(self.path("app"))
            .env_remove("UNIO_POLICY")
            .env_remove("UNIO_WORKER_PROBE")
            .env("UNIO_CACHE", self.path("cache"))
            .env("UNIO_PACKAGES_CONFIG", self.path("packages.toml"))
            .env("HOME", self.path("home"))
            .stdin(Stdio::null())
            .args(args);
        command
    }

    fn unio(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn cache_entries(&self) -> usize {
        fs::read_dir(self.path("cache/packages"))
            .map(|d| d.count())
            .unwrap_or(0)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[track_caller]
fn ok(output: &Output) {
    assert!(output.status.success(), "failed:\n{}", all(output));
}

#[track_caller]
fn refused(output: &Output, expected: &str) {
    assert!(!output.status.success(), "succeeded:\n{}", all(output));
    assert!(
        all(output).contains(expected),
        "expected {expected:?}:\n{}",
        all(output)
    );
}

// ---------------------------------------------------------------- dependencies end to end

#[test]
fn a_path_dependency_is_locked_installed_and_imported() {
    let p = Project::new("path");
    p.util("util", "1.0.0");
    let before = p.unio(&["check", "main.unio"]);
    refused(&before, "E0048");
    ok(&p.unio(&["add", "util", "--path", "../util"]));
    assert!(p
        .read("app/unio.toml")
        .contains("util = { path = \"../util\" }"));
    let lock = p.read("app/unio.lock");
    assert!(lock.contains("id = \"path+../util\""), "{lock}");
    assert!(lock.contains("version = \"1.0.0\""), "{lock}");
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "HELLO!\n");
    // A change of the directory needs an explicit update; the lock is never changed silently.
    p.write(
        "util/extra.unio",
        "public function more() returns Int:\n    return 1\n",
    );
    fs::remove_dir_all(p.path("cache/packages")).unwrap();
    refused(&p.unio(&["install"]), "unio update util");
    assert_eq!(p.read("app/unio.lock"), lock);
    ok(&p.unio(&["update", "util"]));
    assert_ne!(p.read("app/unio.lock"), lock);
    ok(&p.unio(&["run", "main.unio"]));
    ok(&p.unio(&["remove", "util"]));
    assert!(!p.read("app/unio.lock").contains("path+../util"));
    refused(&p.unio(&["check", "main.unio"]), "E0048");
}

/// T-14: installation validates the target and runtime requirements of every package it
/// installs, and of the root package: `package.unio_version` must hold for this toolchain and
/// `package.targets`, when given, must name this machine. A package that does not meet them is
/// refused at `unio add` and `unio install` with what it asks for, and nothing is written; a
/// malformed key is E0088, at `unio check` as at installation.
#[test]
fn target_and_runtime_requirements_are_validated_at_installation() {
    let host = unio::package::manifest::host_target().expect("a supported machine");
    let other = unio::package::manifest::TARGETS
        .iter()
        .find(|t| **t != host)
        .unwrap();
    let this = unio::package::manifest::toolchain_version();
    let p = Project::new("requirements");
    let manifest = p.read("app/unio.toml");
    let with = |extra: &str| {
        p.util("util", "1.0.0");
        let base = p.read("util/unio.toml");
        p.write("util/unio.toml", &format!("{base}{extra}"));
    };
    let nothing_written = |p: &Project| {
        assert_eq!(p.read("app/unio.toml"), manifest);
        assert!(!p.path("app/unio.lock").exists());
        assert_eq!(p.cache_entries(), 0);
    };
    // A later toolchain than this one.
    with(&format!("unio_version = \">={}.0\"\n", this.major + 1));
    let out = p.unio(&["add", "util", "--path", "../util"]);
    refused(&out, "package.unio_version");
    refused(&out, "cannot be installed");
    nothing_written(&p);
    // Another machine.
    with(&format!("targets = [\"{other}\"]\n"));
    let out = p.unio(&["add", "util", "--path", "../util"]);
    refused(&out, "package.targets");
    refused(&out, host);
    nothing_written(&p);
    // A malformed key is a mistake of the manifest.
    with("targets = [\"x86_64-linux\"]\n");
    refused(&p.unio(&["add", "util", "--path", "../util"]), "E0088");
    nothing_written(&p);
    with("unio_version = \"soon\"\n");
    refused(&p.unio(&["add", "util", "--path", "../util"]), "E0088");
    nothing_written(&p);
    // Requirements this toolchain and machine meet: installed, run, and installed again.
    with(&format!(
        "unio_version = \"^{}.{}\"\ntargets = [\"{host}\", \"{other}\"]\n",
        this.major, this.minor
    ));
    ok(&p.unio(&["add", "util", "--path", "../util"]));
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "HELLO!\n");
    fs::remove_dir_all(p.path("cache/packages")).unwrap();
    ok(&p.unio(&["install"]));
    // The root package's own requirements hold at installation too.
    let added = p.read("app/unio.toml");
    p.write(
        "app/unio.toml",
        &added.replace(
            "version = \"0.1.0\"\n",
            &format!("version = \"0.1.0\"\ntargets = [\"{other}\"]\n"),
        ),
    );
    let out = p.unio(&["install"]);
    refused(&out, "package.targets");
    // And `unio check` reports a malformed key at its line as E0088.
    p.write(
        "app/unio.toml",
        &added.replace(
            "version = \"0.1.0\"\n",
            "version = \"0.1.0\"\ntargets = [\"vax\"]\n",
        ),
    );
    let out = p.unio(&["check", "main.unio"]);
    refused(&out, "E0088");
    refused(&out, "'vax' in package.targets is not a target");
}

#[test]
fn a_registry_dependency_and_its_dependency_run_end_to_end() {
    let server = Server::start();
    let p = Project::new("registry");
    p.use_registry(&server);
    server.publish(
        "acme/text",
        &[
            ("1.0.0", vec![("letters.unio", "public function upper(t as Text) returns Text:\n    return t.to_upper()\n")], vec![]),
            ("1.4.0", vec![("letters.unio", "public function upper(t as Text) returns Text:\n    return t.to_upper() + \"!\"\n")], vec![]),
        ],
    );
    server.publish(
        "acme/util",
        &[
            ("1.0.0", vec![("strings.unio", "public function shout(t as Text) returns Text:\n    return t\n")], vec![]),
            (
                "1.2.0",
                vec![("strings.unio", "from deps.text.letters import upper\n\npublic function shout(t as Text) returns Text:\n    return upper(t)\n")],
                vec![("text", "acme/text", "^1.1")],
            ),
            ("2.0.0", vec![("strings.unio", "public function shout(t as Text) returns Text:\n    return \"two\"\n")], vec![]),
        ],
    );
    let add = p.unio(&[
        "add",
        "util",
        "^1.0",
        "--package",
        "acme/util",
        "--registry",
        "main",
    ]);
    ok(&add);
    assert!(
        all(&add).contains("verified acme/util 1.2.0: ML-DSA-65 signature of the key"),
        "{}",
        all(&add)
    );
    let lock = p.read("app/unio.lock");
    assert!(
        lock.contains(&format!("signer = \"{}\"", server.signer.public().id)),
        "{lock}"
    );
    assert!(lock.contains("index_sequence = 3"), "{lock}");
    let origin = format!("registry+http://127.0.0.1:{}/reg", server.port);
    assert!(
        lock.contains(&format!("id = \"{origin}#acme/util\"")),
        "{lock}"
    );
    assert!(
        lock.contains(&format!("\"text|^1.1|{origin}#acme/text\"")),
        "{lock}"
    );
    assert!(lock.contains("version = \"1.2.0\""), "{lock}");
    assert!(lock.contains("version = \"1.4.0\""), "{lock}");
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "HELLO!\n");
    // Resolving again gives the same bytes: with the lock, and without it.
    ok(&p.unio(&["update"]));
    assert_eq!(p.read("app/unio.lock"), lock);
    fs::remove_file(p.path("app/unio.lock")).unwrap();
    ok(&p.unio(&["update"]));
    assert_eq!(p.read("app/unio.lock"), lock);
}

#[test]
fn a_locked_build_and_an_offline_install_touch_no_network() {
    let server = Server::start();
    let p = Project::new("offline");
    p.use_registry(&server);
    server.publish(
        "acme/util",
        &[(
            "1.0.0",
            vec![(
                "strings.unio",
                "public function shout(t as Text) returns Text:\n    return t + \"?\"\n",
            )],
            vec![],
        )],
    );
    ok(&p.unio(&[
        "add",
        "util",
        "1",
        "--package",
        "acme/util",
        "--registry",
        "main",
    ]));
    let downloads = server.requested().len();
    assert!(downloads >= 2, "{:?}", server.requested());
    // With the lock and the cache, nothing is downloaded: not by the check, the run, the
    // tests, nor by an install, offline or not.
    ok(&p.unio(&["check", "main.unio"]));
    ok(&p.unio(&["run", "main.unio"]));
    ok(&p.unio(&["install", "--offline"]));
    ok(&p.unio(&["install"]));
    assert_eq!(server.requested().len(), downloads);
    drop(server);
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "hello?\n");
    // Without the cache entry, the build says to install, and an offline install fails.
    fs::remove_dir_all(p.path("cache/packages")).unwrap();
    refused(&p.unio(&["run", "main.unio"]), "run `unio install`");
    refused(&p.unio(&["install", "--offline"]), "--offline forbids");
    assert_eq!(p.cache_entries(), 0);
}

#[test]
fn a_hash_mismatch_is_refused_before_unpacking() {
    let p = Project::new("hash");
    let bytes = tar(&[
        (
            "unio.toml",
            "[package]\nname = \"util\"\nversion = \"1.0.0\"\n".into(),
        ),
        (
            "strings.unio",
            "public function shout(t as Text) returns Text:\n    return t\n".into(),
        ),
    ]);
    fs::write(p.path("app/util.tar"), &bytes).unwrap();
    let manifest = p.read("app/unio.toml");
    let wrong = "0".repeat(96);
    let out = p.unio(&["add", "util", "--archive", "util.tar", "--sha384", &wrong]);
    refused(&out, "the unpack worker refused the package");
    refused(&out, "refused before unpacking");
    assert_eq!(p.read("app/unio.toml"), manifest);
    assert!(!p.path("app/unio.lock").exists());
    assert_eq!(p.cache_entries(), 0);
    // The right hash installs it.
    ok(&p.unio(&[
        "add",
        "util",
        "--archive",
        "util.tar",
        "--sha384",
        &sha(&bytes),
    ]));
    ok(&p.unio(&["run", "main.unio"]));
    // A registry index whose hash does not match its archive is refused too.
    let server = Server::start();
    p.use_registry(&server);
    server.publish("acme/other", &[("1.0.0", vec![], vec![])]);
    server.put(
        "archives/acme/other/1.0.0.tar",
        tar(&[(
            "unio.toml",
            "[package]\nname = \"other\"\nversion = \"1.0.0\"\n# changed\n".into(),
        )]),
    );
    let out = p.unio(&[
        "add",
        "other",
        "1",
        "--package",
        "acme/other",
        "--registry",
        "main",
    ]);
    refused(&out, "refused before unpacking");
}

// ---------------------------------------------------------------- hostile archives (SB-15)

#[test]
fn hostile_archives_are_refused_by_the_unpack_worker() {
    let p = Project::new("hostile");
    p.config("[limits]\nentries = 8\nfile_bytes = 4096\nunpacked_bytes = 6000\ndepth = 4\n");
    let toml: (&str, u8, &[u8], &str) = (
        "unio.toml",
        b'0',
        b"[package]\nname = \"evil\"\nversion = \"1.0.0\"\n",
        "",
    );
    let big = vec![b'x'; 4097];
    let half = vec![b'x'; 3500];
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "absolute",
            raw_tar(&[toml, ("/tmp/unio-evil", b'0', b"x", "")]),
            "absolute path",
        ),
        (
            "dotdot",
            raw_tar(&[toml, ("a/../../evil.txt", b'0', b"x", "")]),
            "leads out of the package",
        ),
        (
            "symlink",
            raw_tar(&[toml, ("link", b'2', b"", "/etc/passwd")]),
            "symbolic link",
        ),
        (
            "hardlink",
            raw_tar(&[toml, ("hard", b'1', b"", "/etc/passwd")]),
            "hard link",
        ),
        (
            "chardev",
            raw_tar(&[toml, ("dev", b'3', b"", "")]),
            "character device",
        ),
        (
            "blockdev",
            raw_tar(&[toml, ("disk", b'4', b"", "")]),
            "block device",
        ),
        ("fifo", raw_tar(&[toml, ("pipe", b'6', b"", "")]), "FIFO"),
        (
            "pax",
            raw_tar(&[toml, ("pax", b'x', b"", "")]),
            "extended pax header",
        ),
        (
            "longname",
            raw_tar(&[toml, ("long", b'L', b"", "")]),
            "GNU long name",
        ),
        (
            "duplicate",
            raw_tar(&[toml, ("a.unio", b'0', b"x", ""), ("a.unio", b'0', b"y", "")]),
            "appears twice",
        ),
        (
            "case",
            raw_tar(&[
                toml,
                ("Mod.unio", b'0', b"x", ""),
                ("mod.unio", b'0', b"y", ""),
            ]),
            "only in case",
        ),
        (
            "reserved",
            raw_tar(&[toml, ("aux.unio", b'0', b"x", "")]),
            "reserved on Windows",
        ),
        (
            "characters",
            raw_tar(&[toml, ("a:b", b'0', b"x", "")]),
            "portable set",
        ),
        (
            "count",
            raw_tar(&[
                toml,
                ("a", b'0', b"", ""),
                ("b", b'0', b"", ""),
                ("c", b'0', b"", ""),
                ("d", b'0', b"", ""),
                ("e", b'0', b"", ""),
                ("f", b'0', b"", ""),
                ("g", b'0', b"", ""),
                ("h", b'0', b"", ""),
            ]),
            "more than 8 entries",
        ),
        (
            "file-size",
            raw_tar(&[toml, ("big", b'0', &big, "")]),
            "larger than 4096 bytes",
        ),
        (
            "total-size",
            raw_tar(&[toml, ("one", b'0', &half, ""), ("two", b'0', &half, "")]),
            "together",
        ),
        (
            "depth",
            raw_tar(&[toml, ("a/b/c/d/e.unio", b'0', b"x", "")]),
            "deeper than 4",
        ),
        (
            "file-as-directory",
            raw_tar(&[toml, ("x", b'0', b"x", ""), ("x/y", b'0', b"x", "")]),
            "a file and a directory",
        ),
    ];
    // A size that claims far more data than the archive holds: the "bomb" of an uncompressed
    // format is refused before anything is allocated for it.
    let mut bomb = raw_tar(&[toml]);
    let at = bomb.len() - 1024;
    bomb.splice(at..at, archive::header("bomb", b'0', 1 << 40, ""));
    let mut cases = cases;
    cases.push(("bomb", bomb, "past the end of the archive"));
    let mut trailing = raw_tar(&[toml]);
    trailing.extend_from_slice(&archive::header("late", b'0', 0, ""));
    cases.push(("trailing", trailing, "follows the end"));
    let manifest = p.read("app/unio.toml");
    for (name, bytes, expected) in cases {
        let file = format!("{name}.tar");
        fs::write(p.path(&format!("app/{file}")), &bytes).unwrap();
        let out = p.unio(&["add", "evil", "--archive", &file, "--sha384", &sha(&bytes)]);
        refused(&out, "the unpack worker refused the package");
        refused(&out, expected);
        assert_eq!(p.read("app/unio.toml"), manifest, "{name}");
        assert!(!p.path("app/unio.lock").exists(), "{name}");
        assert_eq!(p.cache_entries(), 0, "{name}");
        assert!(
            !p.path("evil.txt").exists() && !p.path("cache/evil.txt").exists(),
            "{name}"
        );
    }
    // Nothing is left in the staging area of the cache either.
    assert_eq!(fs::read_dir(p.path("cache/staging")).unwrap().count(), 0);
}

/// A path dependency that holds a link out of its directory is refused, and nothing of the
/// target is installed. On Unix the link is a symbolic link to a file; on Windows it is a
/// directory junction, which needs no privilege and is a link like a symbolic link (a
/// name-surrogate reparse point, which `symlink_metadata` reports as one). Where the link
/// cannot be made the test fails under UNIO_REQUIRE_PREREQUISITES=1, as in CI. The Windows run
/// is on CI only.
#[test]
fn a_path_dependency_with_a_link_out_of_it_is_refused() {
    let p = Project::new("pathlink");
    p.util("util", "1.0.0");
    p.write("outside/secret.unio", "SECRET");
    #[cfg(unix)]
    std::os::unix::fs::symlink(p.path("outside/secret.unio"), p.path("util/leak.unio")).unwrap();
    #[cfg(windows)]
    {
        // `mklink` does not take the verbatim prefix that `canonicalize` gives.
        let plain = |path: PathBuf| {
            let text = path.display().to_string();
            text.strip_prefix(r"\\?\").map(String::from).unwrap_or(text)
        };
        let made = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(plain(p.path("util/leak")))
            .arg(plain(p.path("outside")))
            .output();
        let made = made.is_ok_and(|o| o.status.success());
        if !made || !fs::symlink_metadata(p.path("util/leak")).is_ok_and(|m| m.is_symlink()) {
            common::missing_prerequisite("a directory junction cannot be made with mklink /J");
            return;
        }
    }
    #[cfg(any(unix, windows))]
    {
        let out = p.unio(&["add", "util", "--path", "../util"]);
        refused(&out, "symbolic link");
        assert!(!all(&out).contains("SECRET"), "{}", all(&out));
        assert_eq!(p.cache_entries(), 0, "{}", all(&out));
        assert!(!p.read("app/unio.toml").contains("util"));
    }
}

// ---------------------------------------------------------------- the download worker

/// The download worker can open no connection itself: the tool connects to the configured
/// registry and passes the connection, and the probe of another port on the same host is
/// denied by the operating system while the download goes on (SB-12). On Linux and macOS the
/// worker's seccomp filter or Seatbelt profile denies it; on Windows its AppContainer has no
/// network capability, so it reaches no address, not even the loopback, and the tool passes
/// the socket it connected (docs/runtime.md 9.7). The Windows run is on CI only.
#[test]
fn the_download_worker_reaches_only_the_configured_registry() {
    let server = Server::start();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    other.set_nonblocking(true).unwrap();
    let other_address = other.local_addr().unwrap();
    let p = Project::new("download");
    p.use_registry(&server);
    server.publish(
        "acme/util",
        &[(
            "1.0.0",
            vec![(
                "strings.unio",
                "public function shout(t as Text) returns Text:\n    return t\n",
            )],
            vec![],
        )],
    );
    let probe = format!("connect={other_address};udp");
    let out = p
        .command(&[
            "add",
            "util",
            "1",
            "--package",
            "acme/util",
            "--registry",
            "main",
        ])
        .env("UNIO_WORKER_PROBE", &probe)
        .output()
        .unwrap();
    ok(&out);
    let err = text(&out.stderr);
    let line = format!("unio worker probe: connect={other_address}: ");
    let results: Vec<&str> = err
        .lines()
        .filter_map(|l| l.strip_prefix(line.as_str()))
        .collect();
    assert!(!results.is_empty(), "no probe ran:\n{err}");
    assert!(results.iter().all(|r| r.starts_with("denied")), "{err}");
    assert!(other.accept().is_err(), "a worker reached the other port");
    assert!(server.requested().iter().any(|r| r.ends_with(".tar")));
    // An archive on a host that no configured registry has is refused before any download.
    let out = p.unio(&[
        "add",
        "far",
        "--archive",
        &format!("http://{other_address}/x.tar"),
        "--sha384",
        &"a".repeat(96),
    ]);
    refused(&out, "is not the host of a configured registry");
    assert!(
        other.accept().is_err(),
        "the tool connected to the other port"
    );
}

#[test]
fn https_unsigned_and_unanchored_registries_are_refused() {
    let server = Server::start();
    let p = Project::new("unsigned");
    server.publish("acme/util", &[("1.0.0", vec![], vec![])]);
    let add = [
        "add",
        "util",
        "1",
        "--package",
        "acme/util",
        "--registry",
        "main",
    ];
    // No trust anchor provisioned by the operator: refused before any download.
    p.config(&format!("[registry.main]\nurl = \"{}\"\n", server.url()));
    refused(&p.unio(&add), "has no anchor");
    // The option that accepted unverified packages is gone.
    p.write("anchor.pub", &server.anchor.public().to_file());
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nanchor = \"anchor.pub\"\nallow_unsigned = true\n",
        server.url()
    ));
    refused(&p.unio(&add), "allow_unsigned is no longer accepted");
    // An anchor file that is missing.
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nanchor = \"missing.pub\"\n",
        server.url()
    ));
    refused(&p.unio(&add), "the trust anchor of the registry 'main'");
    // An index of format 1, without signatures.
    p.use_registry(&server);
    let index = String::from_utf8(server.get("index/acme/util.json")).unwrap();
    server.put(
        "index/acme/util.json",
        index.replace("index-2", "index-1").into_bytes(),
    );
    refused(&p.unio(&add), "the index of acme/util is refused");
    assert!(!p.path("app/unio.lock").exists());
    assert_eq!(p.cache_entries(), 0);
    p.config("[registry.main]\nurl = \"https://registry.example.org/\"\nanchor = \"anchor.pub\"\n");
    refused(&p.unio(&["install"]), "HTTPS is not supported");
}

#[test]
fn incompatible_requirements_fail_with_their_paths() {
    let server = Server::start();
    let p = Project::new("conflict");
    p.use_registry(&server);
    server.publish(
        "acme/d",
        &[("1.0.0", vec![], vec![]), ("2.0.0", vec![], vec![])],
    );
    server.publish("acme/b", &[("1.0.0", vec![], vec![("d", "acme/d", "^1")])]);
    server.publish("acme/c", &[("1.0.0", vec![], vec![("d", "acme/d", "^2")])]);
    ok(&p.unio(&["add", "b", "1", "--package", "acme/b", "--registry", "main"]));
    let out = p.unio(&["add", "c", "1", "--package", "acme/c", "--registry", "main"]);
    refused(&out, "acme/b 1.0.0 requires ^1");
    refused(&out, "acme/c 1.0.0 requires ^2");
}

// ---------------------------------------------------------------- the review of T-9

#[test]
fn the_review_of_t9_is_shown_and_grants_nothing() {
    let p = Project::new("review");
    p.write(
        "util/unio.toml",
        "[package]\nname = \"util\"\nversion = \"1.0.0\"\nunsafe_modules = [\"raw\"]\n\n[permissions]\nfiles.read = [\"$CWD/**\"]\n",
    );
    p.write(
        "util/strings.unio",
        "import std.files\n\npublic function shout(text as Text) returns Text:\n    if files.exists(Path(\"secret.txt\")):\n        return \"LEAKED\"\n    return text.to_upper() + \"!\"\n",
    );
    p.write(
        "util/raw.unio",
        "unsafe module\n\npublic function same(x as Int) returns Int:\n    return x\n",
    );
    p.write("app/secret.txt", "SECRET");
    let manifest = p.read("app/unio.toml");
    // Without a confirmation (no terminal, no --unsafe-ok): the review is shown, nothing changes.
    let out = p.unio(&["add", "util", "--path", "../util"]);
    refused(&out, "needs your review before it is installed (T-9)");
    refused(&out, "listed for review: raw");
    refused(&out, "contain no external functions");
    refused(&out, "files.read $CWD/**");
    refused(&out, "it grants nothing");
    assert_eq!(p.read("app/unio.toml"), manifest);
    assert!(!p.path("app/unio.lock").exists());
    assert_eq!(p.cache_entries(), 0);
    // Confirmed: installed, and still nothing granted. The compiler checks the call
    // statically: the effect of the dependency needs files.read of the root package, which
    // has no `[permissions]` table and so permits nothing (E0085, SB-7), as with a table
    // without files.read.
    let out = p.unio(&["add", "util", "--path", "../util", "--unsafe-ok"]);
    ok(&out);
    assert!(all(&out).contains("nothing is granted"), "{}", all(&out));
    let installed = p.read("app/unio.toml");
    refused(&p.unio(&["check", "main.unio"]), "E0085");
    p.write(
        "app/unio.toml",
        &(installed.clone() + "\n[permissions]\nenv = []\n"),
    );
    refused(&p.unio(&["check", "main.unio"]), "E0085");
    // The program runs with the permissions of the root package, which reach only `public/`,
    // so the dependency does not see the file.
    p.write(
        "app/unio.toml",
        &(installed + "\n[permissions]\nfiles.read = [\"$CWD/public/**\"]\n"),
    );
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "HELLO!\n");
    // An external function in the marked module of a package is still E0086 (RFC 0004).
    p.write(
        "util/raw.unio",
        "unsafe module\n\nexternal function poke(x as Int) returns Int\n",
    );
    p.write(
        "app/unio.toml",
        &manifest.replace(
            "version = \"0.1.0\"",
            "version = \"0.1.0\"\n\n[dependencies]\nutil = { path = \"../util\" }",
        ),
    );
    ok(&p.unio(&["update", "util", "--unsafe-ok"]));
    p.write(
        "app/main.unio",
        "import deps.util.raw\n\npublic function main() returns Int:\n    return 0\n",
    );
    refused(&p.unio(&["check", "main.unio"]), "E0086");
}

// ---------------------------------------------------------------- SB-14

/// No code of a package runs while it is installed, checked, or tested: its `main` and its
/// constants would write a marker, which never appears.
#[test]
fn the_compiler_never_runs_package_code() {
    let p = Project::new("noexec");
    let marker = p.path("app/ran.txt");
    p.write(
        "util/unio.toml",
        "[package]\nname = \"util\"\nversion = \"1.0.0\"\n\n[permissions]\nfiles.write = [\"$CWD/**\"]\n",
    );
    p.write(
        "util/strings.unio",
        "import std.files\n\nlet MARK as Text = mark()\n\nfunction mark() returns Text:\n    try:\n        files.write_text(Path(\"ran.txt\"), \"ran\")\n    on IoError as e:\n        return \"no {e.reason}\"\n    return \"yes\"\n\npublic function shout(text as Text) returns Text:\n    return text\n\npublic function main() returns Int:\n    print(mark())\n    return 0\n\ntest \"it marks\":\n    assert mark() == \"yes\"\n",
    );
    ok(&p.unio(&["add", "util", "--path", "../util", "--unsafe-ok"]));
    ok(&p.unio(&["install", "--unsafe-ok"]));
    ok(&p.unio(&["check", "main.unio"]));
    p.write(
        "app/main_test.unio",
        "import deps.util.strings\n\ntest \"nothing\":\n    assert 1 == 1\n",
    );
    ok(&p.unio(&["test", "main_test.unio"]));
    assert!(!marker.exists(), "package code ran");
}

/// Two origins with the same name and even the same files are two packages: their types are
/// not the same (T-14), although the cache keeps one entry for the one content.
#[test]
fn two_origins_with_one_name_do_not_share_types() {
    let p = Project::new("identity");
    for dir in ["one", "two"] {
        p.write(
            &format!("{dir}/unio.toml"),
            "[package]\nname = \"boxes\"\nversion = \"1.0.0\"\n",
        );
        p.write(
            &format!("{dir}/box.unio"),
            "public struct Box:\n    public value as Int\n\npublic function make() returns Box:\n    return Box(value = 1)\n",
        );
    }
    p.write(
        "app/main.unio",
        "import deps.a.box as a\nimport deps.b.box as b\n\nfunction take(x as a.Box) returns Int:\n    return x.value\n\npublic function main() returns Int:\n    print(\"{take(a.make())}\")\n    print(\"{take(b.make())}\")\n    return 0\n",
    );
    ok(&p.unio(&["add", "a", "--path", "../one"]));
    ok(&p.unio(&["add", "b", "--path", "../two"]));
    assert_eq!(p.cache_entries(), 1);
    let out = p.unio(&["check", "main.unio"]);
    refused(&out, "main.unio:9");
    assert!(!all(&out).contains("main.unio:8"), "{}", all(&out));
}

/// The unpack worker reads only the package it unpacks and writes only the staging directory
/// of its cache entry: the probes past the code of the worker are denied by the system (SB-12).
/// On Linux and macOS by Landlock and seccomp or by Seatbelt; on Windows by the AppContainer,
/// which holds grants on exactly the scopes of the worker, and by its Job Object, which allows
/// one active process, so the worker starts none (docs/runtime.md 9.7). The Windows run is on
/// CI only.
#[test]
fn the_unpack_worker_writes_only_its_cache_entry() {
    let p = Project::new("unpackprobe");
    p.util("util", "1.0.0");
    p.write("home/secret.txt", "SECRET");
    // A program that exists on the system, so that only the sandbox can deny its start.
    let shell = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join("cmd.exe")
    } else {
        PathBuf::from("/bin/sh")
    };
    assert!(shell.is_file(), "{} is missing", shell.display());
    let probes = [
        format!("write={}", p.path("cache/packages/planted").display()),
        format!("write={}", p.path("app/planted").display()),
        format!("read={}", p.path("home/secret.txt").display()),
        format!("read={}", p.path("app/unio.toml").display()),
        format!("exec={}", shell.display()),
    ];
    let out = p
        .command(&["add", "util", "--path", "../util"])
        .env("UNIO_WORKER_PROBE", probes.join(";"))
        .output()
        .unwrap();
    ok(&out);
    let err = text(&out.stderr);
    for probe in &probes {
        let prefix = format!("unio worker probe: {probe}: ");
        let results: Vec<&str> = err
            .lines()
            .filter_map(|l| l.strip_prefix(prefix.as_str()))
            .collect();
        assert!(!results.is_empty(), "no result for {probe}:\n{err}");
        assert!(
            results.iter().all(|r| r.starts_with("denied")),
            "{probe}:\n{err}"
        );
    }
    assert!(!p.path("cache/packages/planted").exists() && !p.path("app/planted").exists());
    assert!(!err.contains("SECRET"));
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
}

// ---------------------------------------------------------------- signed registries (T-8, T-8a)

const ADD_UTIL: [&str; 7] = [
    "add",
    "util",
    "1",
    "--package",
    "acme/util",
    "--registry",
    "main",
];

const SHOUT: &str = "public function shout(t as Text) returns Text:\n    return t.to_upper()\n";

/// A secret key file of a test key, outside the package directory.
fn key_file(p: &Project, name: &str, key: &SecretKey) -> String {
    p.write(&format!("keys/{name}.key"), &key.to_file());
    p.write(&format!("keys/{name}.pub"), &key.public().to_file());
    p.path(&format!("keys/{name}.key")).display().to_string()
}

#[track_caller]
fn nothing_installed(p: &Project) {
    assert!(!p.path("app/unio.lock").exists(), "a lock was written");
    assert_eq!(p.cache_entries(), 0, "a cache entry was made");
    let staging = fs::read_dir(p.path("cache/staging")).map_or(0, |d| d.count());
    assert_eq!(staging, 0, "something was left in the staging area");
}

/// The whole path with the commands of the operator and the publisher: keys, a registry
/// directory, `unio publish`, a static HTTP server on 127.0.0.1, and a verified install.
#[test]
fn publish_serve_and_install_verified_end_to_end() {
    let p = Project::new("publish");
    let unio = |dir: &str, args: &[&str]| {
        let mut c = p.command(args);
        c.current_dir(p.path(dir));
        c.output().unwrap()
    };
    // Keys are never made inside a package.
    refused(
        &unio("app", &["key", "new", "anchor", "--dir", "secrets"]),
        "never kept in a project",
    );
    ok(&unio(".", &["key", "new", "anchor", "--dir", "keys"]));
    ok(&unio(".", &["key", "new", "signer", "--dir", "keys"]));
    assert!(SecretKey::parse_file(&fs::read(p.path("keys/anchor.key")).unwrap()).is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(p.path("keys/signer.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the secret key is readable by others");
    }
    refused(
        &unio(".", &["key", "new", "signer", "--dir", "keys"]),
        "already exists",
    );
    let reg = p.path("www/reg").display().to_string();
    ok(&unio(
        ".",
        &["registry", "init", &reg, "--anchor", "keys/anchor.key"],
    ));
    ok(&unio(
        ".",
        &[
            "key",
            "endorse",
            "keys/signer.pub",
            "--key",
            "keys/anchor.key",
            "--registry",
            &reg,
        ],
    ));
    p.util("util", "1.0.0");
    let publish = [
        "publish",
        "--registry",
        &reg,
        "--owner",
        "acme",
        "--key",
        "../keys/signer.key",
    ];
    let out = unio("util", &publish);
    ok(&out);
    assert!(
        all(&out).contains("published acme/util 1.0.0"),
        "{}",
        all(&out)
    );
    let tar = fs::read(p.path("www/reg/archives/acme/util/1.0.0.tar")).unwrap();
    let sig = fs::read(p.path("www/reg/archives/acme/util/1.0.0.tar.sig")).unwrap();
    // The same content again changes nothing; other content under the version is refused.
    ok(&unio("util", &publish));
    p.write("util/strings.unio", SHOUT);
    refused(&unio("util", &publish), "never replaced or removed");
    assert_eq!(
        fs::read(p.path("www/reg/archives/acme/util/1.0.0.tar")).unwrap(),
        tar
    );
    // A secret key in the package is never published.
    p.write(
        "util/unio.toml",
        "[package]\nname = \"util\"\nversion = \"1.1.0\"\n",
    );
    fs::copy(p.path("keys/signer.key"), p.path("util/stray.key")).unwrap();
    refused(&unio("util", &publish), "secret key file");
    fs::remove_file(p.path("util/stray.key")).unwrap();
    ok(&unio("util", &publish));
    // Archives are deterministic: another registry gets the same bytes and, with deterministic
    // ML-DSA, the same signature.
    p.util("copy/util", "1.0.0");
    let other = p.path("www/other").display().to_string();
    ok(&unio(
        ".",
        &["registry", "init", &other, "--anchor", "keys/anchor.key"],
    ));
    let publish2 = [
        "publish",
        "--registry",
        &other,
        "--owner",
        "acme",
        "--key",
        "../../keys/signer.key",
    ];
    ok(&unio("copy/util", &publish2));
    assert_eq!(
        fs::read(p.path("www/other/archives/acme/util/1.0.0.tar")).unwrap(),
        tar
    );
    assert_eq!(
        fs::read(p.path("www/other/archives/acme/util/1.0.0.tar.sig")).unwrap(),
        sig
    );
    // Served by a plain static server, installed verified.
    let server = Server::start();
    fs::remove_dir_all(&server.root).unwrap();
    fs::rename(p.path("www/reg"), &server.root).unwrap();
    fs::copy(p.path("keys/anchor.pub"), p.path("anchor.pub")).unwrap();
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nanchor = \"anchor.pub\"\n",
        server.url()
    ));
    let out = p.unio(&ADD_UTIL);
    ok(&out);
    assert!(
        all(&out).contains("verified acme/util 1.1.0"),
        "{}",
        all(&out)
    );
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "HELLO\n");
}

type Tamper = Box<dyn Fn(&Server)>;

/// Every change an attacker on the path or on the server can make without the signing keys
/// is refused, before anything of an archive is unpacked, and nothing is installed.
#[test]
fn tampered_and_untrusted_registry_files_are_refused_before_unpacking() {
    let other_tar = tar(&[(
        "unio.toml",
        "[package]\nname = \"util\"\nversion = \"1.0.0\"\n# evil\n".into(),
    )]);
    let stranger = SecretKey::from_seed(&[9; 32]);
    let ot = other_tar.clone();
    let cases: Vec<(&str, Tamper, &str)> = vec![
        (
            "tampered archive",
            Box::new(move |s| s.put("archives/acme/util/1.0.0.tar", ot.clone())),
            "refused before unpacking",
        ),
        (
            "tampered archive and index",
            Box::new(move |s| {
                let old = sha(&s.get("archives/acme/util/1.0.0.tar"));
                s.put("archives/acme/util/1.0.0.tar", other_tar.clone());
                let index = String::from_utf8(s.get("index/acme/util.json")).unwrap();
                s.put(
                    "index/acme/util.json",
                    index.replace(&old, &sha(&other_tar)).into_bytes(),
                );
            }),
            "the payload has the digest",
        ),
        (
            "signed by a key nobody endorsed",
            Box::new(move |s| {
                fs::remove_dir_all(s.path("index")).unwrap();
                s.publish_with(
                    &stranger,
                    "acme/util",
                    &[("1.0.0", vec![("strings.unio", SHOUT)], vec![])],
                );
            }),
            "which is not trusted",
        ),
        (
            "revoked key",
            Box::new(|s| {
                registry::revoke(&s.root, &s.anchor, &s.signer.public().id).unwrap();
            }),
            "it is revoked",
        ),
        (
            "unknown algorithm",
            Box::new(|s| {
                let sig = String::from_utf8(s.get("archives/acme/util/1.0.0.tar.sig")).unwrap();
                s.put(
                    "archives/acme/util/1.0.0.tar.sig",
                    sig.replace("ml-dsa-65", "ml-dsa-87").into_bytes(),
                );
            }),
            "is not accepted",
        ),
        (
            "unknown index algorithm",
            Box::new(|s| {
                let sig = String::from_utf8(s.get("index/acme/util.json.sig")).unwrap();
                s.put(
                    "index/acme/util.json.sig",
                    sig.replace("algorithm ml-dsa-65", "algorithm ed25519")
                        .into_bytes(),
                );
            }),
            "only ml-dsa-65",
        ),
        (
            "truncated signature",
            Box::new(|s| {
                let mut sig = s.get("archives/acme/util/1.0.0.tar.sig");
                sig.truncate(sig.len() - 100);
                s.put("archives/acme/util/1.0.0.tar.sig", sig);
            }),
            "refused before unpacking",
        ),
        (
            "a changed byte of the signature",
            Box::new(|s| {
                let mut sig = s.get("archives/acme/util/1.0.0.tar.sig");
                let at = sig.len() - 20;
                sig[at] = if sig[at] == b'0' { b'1' } else { b'0' };
                s.put("archives/acme/util/1.0.0.tar.sig", sig);
            }),
            "is not valid",
        ),
        (
            "no archive signature",
            Box::new(|s| fs::remove_file(s.path("archives/acme/util/1.0.0.tar.sig")).unwrap()),
            "404",
        ),
        (
            "no index signature",
            Box::new(|s| fs::remove_file(s.path("index/acme/util.json.sig")).unwrap()),
            "404",
        ),
        (
            "the signature of another version",
            Box::new(|s| {
                s.publish(
                    "acme/util",
                    &[("0.9.0", vec![("strings.unio", SHOUT)], vec![])],
                );
                let sig = s.get("archives/acme/util/0.9.0.tar.sig");
                s.put("archives/acme/util/1.0.0.tar.sig", sig);
            }),
            "refused before unpacking",
        ),
        (
            "the index signature of another package",
            Box::new(|s| {
                s.publish("acme/other", &[("1.0.0", vec![], vec![])]);
                let sig = s.get("index/acme/other.json.sig");
                s.put("index/acme/util.json.sig", sig);
            }),
            "the index of acme/util is refused",
        ),
        (
            "a tampered endorsement",
            Box::new(|s| {
                let keys = String::from_utf8(s.get("trust/keys")).unwrap();
                s.put(
                    "trust/keys",
                    keys.replacen("expires never", "expires 2999-12-31", 1)
                        .into_bytes(),
                );
            }),
            "the endorsement of the key",
        ),
        (
            "a revocation list not signed by the anchor",
            Box::new(|s| {
                let list = unio::package::trust::Revocations {
                    sequence: 9,
                    revoked: Default::default(),
                };
                s.put(
                    "trust/revocations",
                    list.file(&s.signer).unwrap().into_bytes(),
                );
            }),
            "not by the trust anchor",
        ),
    ];
    for (name, tamper, expected) in cases {
        let server = Server::start();
        server.publish(
            "acme/util",
            &[("1.0.0", vec![("strings.unio", SHOUT)], vec![])],
        );
        tamper(&server);
        let p = Project::new(&format!("tamper-{}", name.replace(' ', "-")));
        p.use_registry(&server);
        let out = p.unio(&ADD_UTIL);
        refused(&out, expected);
        assert!(
            !all(&out).contains("verified acme/util"),
            "{name}:\n{}",
            all(&out)
        );
        nothing_installed(&p);
    }
}

/// Rotation by the old key and by the anchor, revocation that breaks a chain, expiry, and the
/// recovery after a compromised signing key (`unio registry resign`).
#[test]
fn key_rotation_revocation_expiry_and_recovery() {
    let server = Server::start();
    let p = Project::new("rotation");
    p.use_registry(&server);
    let anchor = key_file(&p, "anchor", &server.anchor);
    let old = key_file(&p, "old", &server.signer);
    let new_key = SecretKey::from_seed(&[3; 32]);
    let new = key_file(&p, "new", &new_key);
    let reg = server.root.display().to_string();
    // Rotation: the old key endorses the new one, which signs from now on.
    ok(&p.unio(&[
        "key",
        "endorse",
        &p.path("keys/new.pub").display().to_string(),
        "--key",
        &old,
        "--registry",
        &reg,
    ]));
    server.publish_with(
        &new_key,
        "acme/util",
        &[("1.0.0", vec![("strings.unio", SHOUT)], vec![])],
    );
    ok(&p.unio(&ADD_UTIL));
    // The anchor revokes the old key: the new one, endorsed only by it, is no longer trusted.
    let old_id = server.signer.public().id;
    ok(&p.unio(&[
        "key",
        "revoke",
        &old_id,
        "--key",
        &anchor,
        "--registry",
        &reg,
    ]));
    refused(
        &p.unio(&["key", "revoke", &old_id, "--key", &new, "--registry", &reg]),
        "only the anchor revokes",
    );
    let fresh = Project::new("rotation-fresh");
    fresh.use_registry(&server);
    refused(&fresh.unio(&ADD_UTIL), "neither the trust anchor");
    nothing_installed(&fresh);
    // The anchor endorses the new key itself: trusted again.
    ok(&p.unio(&[
        "key",
        "endorse",
        &p.path("keys/new.pub").display().to_string(),
        "--key",
        &anchor,
        "--registry",
        &reg,
    ]));
    ok(&fresh.unio(&ADD_UTIL));
    // An expired endorsement.
    let short = SecretKey::from_seed(&[4; 32]);
    key_file(&p, "short", &short);
    ok(&p.unio(&[
        "key",
        "endorse",
        &p.path("keys/short.pub").display().to_string(),
        "--key",
        &anchor,
        "--registry",
        &reg,
        "--expires",
        "2000-01-01",
    ]));
    server.publish_with(&short, "acme/other", &[("1.0.0", vec![], vec![])]);
    let other = [
        "add",
        "other",
        "1",
        "--package",
        "acme/other",
        "--registry",
        "main",
    ];
    refused(&fresh.unio(&other), "expired on 2000-01-01");
    // Recovery: the new key is compromised and revoked; a replacement endorsed by the anchor
    // signs every archive and index again.
    ok(&p.unio(&[
        "key",
        "revoke",
        &p.path("keys/new.pub").display().to_string(),
        "--key",
        &anchor,
        "--registry",
        &reg,
    ]));
    let third = Project::new("rotation-third");
    third.use_registry(&server);
    refused(&third.unio(&ADD_UTIL), "it is revoked");
    let spare = SecretKey::from_seed(&[5; 32]);
    let spare_file = key_file(&p, "spare", &spare);
    ok(&p.unio(&[
        "key",
        "endorse",
        &p.path("keys/spare.pub").display().to_string(),
        "--key",
        &anchor,
        "--registry",
        &reg,
    ]));
    ok(&p.unio(&["registry", "resign", &reg, "--key", &spare_file]));
    let out = third.unio(&ADD_UTIL);
    ok(&out);
    assert!(all(&out).contains(&spare.public().id), "{}", all(&out));
    // The anchor cannot be revoked by itself.
    refused(
        &p.unio(&[
            "key",
            "revoke",
            &server.anchor.public().id,
            "--key",
            &anchor,
            "--registry",
            &reg,
        ]),
        "cannot revoke itself",
    );
}

/// An older index or an older revocation list, though validly signed, is refused once a newer
/// one was seen: by the floor of this machine and by the floor of the lock.
#[test]
fn rollback_of_an_index_or_of_the_revocation_list_is_refused() {
    let server = Server::start();
    server.publish(
        "acme/util",
        &[("1.0.0", vec![("strings.unio", SHOUT)], vec![])],
    );
    let old_index = server.get("index/acme/util.json");
    let old_sig = server.get("index/acme/util.json.sig");
    let old_revocations = server.get("trust/revocations");
    server.publish(
        "acme/util",
        &[("1.1.0", vec![("strings.unio", SHOUT)], vec![])],
    );
    registry::revoke(
        &server.root,
        &server.anchor,
        &SecretKey::from_seed(&[8; 32]).public().id,
    )
    .unwrap();
    let new_index = server.get("index/acme/util.json");
    let new_sig = server.get("index/acme/util.json.sig");
    let p = Project::new("rollback");
    p.use_registry(&server);
    ok(&p.unio(&ADD_UTIL));
    let lock = p.read("app/unio.lock");
    assert!(lock.contains("index_sequence = 2"), "{lock}");
    // The old index, served again: refused by the floor of this machine...
    server.put("index/acme/util.json", old_index.clone());
    server.put("index/acme/util.json.sig", old_sig.clone());
    refused(&p.unio(&["update"]), "a rollback to an older index");
    // ...and by the lock, without the state of this machine.
    fs::remove_file(p.path("cache/trust/state")).unwrap();
    refused(&p.unio(&["update"]), "a rollback to an older index");
    assert_eq!(p.read("app/unio.lock"), lock);
    // Without the lock, the floor of this machine still holds.
    server.put("index/acme/util.json", new_index);
    server.put("index/acme/util.json.sig", new_sig);
    ok(&p.unio(&["update"]));
    ok(&p.unio(&["remove", "util"]));
    server.put("index/acme/util.json", old_index);
    server.put("index/acme/util.json.sig", old_sig);
    refused(&p.unio(&ADD_UTIL), "a rollback to an older index");
    // An older revocation list.
    server.publish(
        "acme/util",
        &[("1.2.0", vec![("strings.unio", SHOUT)], vec![])],
    );
    server.put("trust/revocations", old_revocations);
    refused(&p.unio(&ADD_UTIL), "rollback of the trust metadata");
}

/// `unio install` from a lock verifies the signature again: a revoked key, or a lock edited to
/// point at another archive, is refused.
#[test]
fn install_from_the_lock_verifies_the_signature_again() {
    let server = Server::start();
    server.publish(
        "acme/util",
        &[
            ("1.0.0", vec![("strings.unio", SHOUT)], vec![]),
            (
                "2.0.0",
                vec![(
                    "strings.unio",
                    "public function shout(t as Text) returns Text:\n    return \"pwned\"\n",
                )],
                vec![],
            ),
        ],
    );
    let p = Project::new("relock");
    p.use_registry(&server);
    ok(&p.unio(&ADD_UTIL));
    let lock = p.read("app/unio.lock");
    fs::remove_dir_all(p.path("cache/packages")).unwrap();
    // A lock edited to fetch the archive of 2.0.0 under 1.0.0, with its hash.
    let v1 = sha(&server.get("archives/acme/util/1.0.0.tar"));
    let v2 = sha(&server.get("archives/acme/util/2.0.0.tar"));
    let edited = lock.replace(&v1, &v2).replace(
        "archives/acme/util/1.0.0.tar\"",
        "archives/acme/util/2.0.0.tar\"",
    );
    p.write("app/unio.lock", &edited);
    refused(&p.unio(&["install"]), "refused before unpacking");
    let edited = edited.replace("1.0.0.tar.sig", "2.0.0.tar.sig");
    p.write("app/unio.lock", &edited);
    refused(&p.unio(&["install"]), "refused before unpacking");
    assert_eq!(p.cache_entries(), 0);
    // The real lock, after the signing key was revoked.
    p.write("app/unio.lock", &lock);
    registry::revoke(&server.root, &server.anchor, &server.signer.public().id).unwrap();
    refused(&p.unio(&["install"]), "it is revoked");
    assert_eq!(p.cache_entries(), 0);
}

/// A registry that removes or replaces a locked version, even with a valid signature, is
/// refused on update (T-8: no deletion of published versions).
#[test]
fn a_registry_that_removes_a_locked_version_is_refused() {
    let server = Server::start();
    server.publish(
        "acme/util",
        &[("1.0.0", vec![("strings.unio", SHOUT)], vec![])],
    );
    let p = Project::new("removed");
    p.use_registry(&server);
    ok(&p.unio(&ADD_UTIL));
    server.publish(
        "acme/util",
        &[("1.0.1", vec![("strings.unio", SHOUT)], vec![])],
    );
    let published = String::from_utf8(server.get("index/acme/util.json")).unwrap();
    let mut index2 = unio::package::index::Index::parse(&published, "acme/util").unwrap();
    index2.versions.retain(|v| v.version.to_string() != "1.0.0");
    index2.sequence += 1;
    let json = index2.to_json();
    let sealed = unio::envelope::seal(
        &server.signer,
        "registry-index",
        "acme/util",
        index2.sequence,
        json.as_bytes(),
    )
    .unwrap();
    server.put("index/acme/util.json", json.into_bytes());
    server.put("index/acme/util.json.sig", sealed.into_bytes());
    refused(
        &p.unio(&["update"]),
        "no longer lists the locked version 1.0.0",
    );
}

/// L-31 across packages: an implementation belongs to the package of the type or of the
/// interface. A path dependency may implement its own interface for its own type in another of
/// its modules, and the root package may implement an interface of a dependency for a type of
/// its own; an implementation of the interface of one path dependency for the type of another,
/// written in the root package, owns neither and is E0044 (docs/semantics.md 3.6).
#[test]
fn an_implementation_belongs_to_the_package_of_the_type_or_the_interface() {
    let p = Project::new("ownership");
    p.write(
        "shapes/unio.toml",
        "[package]\nname = \"shapes\"\nversion = \"1.0.0\"\n",
    );
    p.write(
        "shapes/point.unio",
        "public struct Point:\n    public x as Int\n",
    );
    p.write(
        "shapes/show.unio",
        "from point import Point\n\nextend Point with Display:\n    function to_text(self) returns Text:\n        return \"point {self.x}\"\n",
    );
    p.write(
        "views/unio.toml",
        "[package]\nname = \"views\"\nversion = \"1.0.0\"\n",
    );
    p.write(
        "views/named.unio",
        "public interface Named:\n    function name(self) returns Text\n",
    );
    p.write(
        "app/main.unio",
        "from deps.shapes.point import Point\nimport deps.shapes.show\nfrom deps.views.named import Named\n\nstruct Label:\n    text as Text\n\nextend Label with Named:\n    function name(self) returns Text:\n        return self.text\n\nfunction describe(n as Named) returns Text:\n    return n.name()\n\npublic function main() returns Int:\n    print(\"{Point(x = 2)}\")\n    print(describe(Label(text = \"label\")))\n    return 0\n",
    );
    ok(&p.unio(&["add", "shapes", "--path", "../shapes"]));
    ok(&p.unio(&["add", "views", "--path", "../views"]));
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "point 2\nlabel\n");
    p.write(
        "app/bridge.unio",
        "from deps.shapes.point import Point\nfrom deps.views.named import Named\n\nextend Point with Named:\n    function name(self) returns Text:\n        return \"point\"\n",
    );
    let out = p.unio(&["check", "bridge.unio"]);
    refused(&out, "E0044");
    assert!(all(&out).contains("bridge.unio:4"), "{}", all(&out));
    assert!(all(&out).contains("only in the package"), "{}", all(&out));
}
