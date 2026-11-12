//! Packages (T-6 to T-9, T-14) and the workers of package operations (SB-12), with the archive
//! cases of SB-15 and SB-14: dependencies from a directory, an archive, and a registry served by
//! a static HTTP server on 127.0.0.1; resolution and lock determinism; installation from the
//! lock without the network; hash mismatches; hostile archives refused by the unpack worker;
//! the download worker that cannot reach any other host; the review of T-9, which grants
//! nothing; and no code of a package run during installation or a check.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use unio::package::archive::{self, Entries, Item};

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

/// A static file server on 127.0.0.1: the layout of a registry (index JSON, archives,
/// signatures) as plain files, as any static server would serve them.
struct Server {
    port: u16,
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let files: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::default();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (f, r, s) = (files.clone(), requests.clone(), stop.clone());
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
                let body = f.lock().unwrap().get(&path).cloned();
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
            files,
            requests,
            stop,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/reg", self.port)
    }

    fn put(&self, path: &str, body: Vec<u8>) {
        self.files
            .lock()
            .unwrap()
            .insert(format!("/reg/{path}"), body);
    }

    /// Publishes versions of `owner/name`: (version, files, dependencies as (alias, package,
    /// requirement)).
    #[allow(clippy::type_complexity)]
    fn publish(
        &self,
        package: &str,
        versions: &[(&str, Vec<(&str, &str)>, Vec<(&str, &str, &str)>)],
    ) {
        let name = package.split('/').nth(1).unwrap();
        let mut entries = Vec::new();
        for (version, files, deps) in versions {
            let mut manifest = format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n");
            if !deps.is_empty() {
                manifest.push_str("\n[dependencies]\n");
                for (alias, p, req) in deps {
                    manifest.push_str(&format!(
                        "{alias} = {{ package = \"{p}\", version = \"{req}\" }}\n"
                    ));
                }
            }
            let mut list: Vec<(&str, String)> = vec![("unio.toml", manifest)];
            list.extend(files.iter().map(|(n, t)| (*n, t.to_string())));
            let bytes = tar(&list);
            let path = format!("archives/{package}/{version}.tar");
            let deps_json: Vec<String> = deps
                .iter()
                .map(|(a, p, r)| {
                    format!(r#"{{"alias": "{a}", "package": "{p}", "version": "{r}"}}"#)
                })
                .collect();
            entries.push(format!(
                r#"{{"version": "{version}", "archive": "{path}", "sha384": "{}", "dependencies": [{}]}}"#,
                sha(&bytes),
                deps_json.join(", ")
            ));
            self.put(&path, bytes);
        }
        let index = format!(
            r#"{{"format": "unio-registry-index-1", "package": "{package}", "versions": [{}]}}"#,
            entries.join(", ")
        );
        self.put(&format!("index/{package}.json"), index.into_bytes());
    }

    fn requested(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
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
        if cfg!(windows) {
            let policy = self.path("weaker.toml");
            fs::write(&policy, "allow_weaker = true\n").unwrap();
            command.env("UNIO_POLICY", policy);
        }
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

#[test]
fn a_registry_dependency_and_its_dependency_run_end_to_end() {
    let server = Server::start();
    let p = Project::new("registry");
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nallow_unsigned = true\n",
        server.url()
    ));
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
        all(&add).contains("without signature verification"),
        "{}",
        all(&add)
    );
    let lock = p.read("app/unio.lock");
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
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nallow_unsigned = true\n",
        server.url()
    ));
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
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nallow_unsigned = true\n",
        server.url()
    ));
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

#[test]
fn a_path_dependency_with_a_link_out_of_it_is_refused() {
    let p = Project::new("pathlink");
    p.util("util", "1.0.0");
    p.write("secret.txt", "SECRET");
    #[cfg(unix)]
    std::os::unix::fs::symlink(p.path("secret.txt"), p.path("util/leak.unio")).unwrap();
    #[cfg(unix)]
    refused(
        &p.unio(&["add", "util", "--path", "../util"]),
        "symbolic link",
    );
}

// ---------------------------------------------------------------- the download worker

/// The download worker can open no connection itself: the tool connects to the configured
/// registry and passes the connection, and the probe of another port on the same host is
/// denied by the operating system while the download goes on (SB-12).
#[cfg(unix)]
#[test]
fn the_download_worker_reaches_only_the_configured_registry() {
    let server = Server::start();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    other.set_nonblocking(true).unwrap();
    let other_address = other.local_addr().unwrap();
    let p = Project::new("download");
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nallow_unsigned = true\n",
        server.url()
    ));
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
fn https_and_unsigned_packages_are_refused_unless_the_operator_allows_them() {
    let server = Server::start();
    let p = Project::new("unsigned");
    p.config(&format!("[registry.main]\nurl = \"{}\"\n", server.url()));
    server.publish("acme/util", &[("1.0.0", vec![], vec![])]);
    let out = p.unio(&[
        "add",
        "util",
        "1",
        "--package",
        "acme/util",
        "--registry",
        "main",
    ]);
    refused(&out, "does not verify registry signatures yet");
    assert!(!p.path("app/unio.lock").exists());
    assert_eq!(p.cache_entries(), 0);
    p.config("[registry.main]\nurl = \"https://registry.example.org/\"\n");
    refused(&p.unio(&["install"]), "HTTPS is not supported yet");
}

#[test]
fn incompatible_requirements_fail_with_their_paths() {
    let server = Server::start();
    let p = Project::new("conflict");
    p.config(&format!(
        "[registry.main]\nurl = \"{}\"\nallow_unsigned = true\n",
        server.url()
    ));
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
    // Confirmed: installed, and still nothing granted. The program runs with the permissions
    // of the root package, which has none, so the dependency does not see the file.
    let out = p.unio(&["add", "util", "--path", "../util", "--unsafe-ok"]);
    ok(&out);
    assert!(all(&out).contains("nothing is granted"), "{}", all(&out));
    let run = p.unio(&["run", "main.unio"]);
    ok(&run);
    assert_eq!(text(&run.stdout), "HELLO!\n");
    // With a [permissions] table the compiler checks the call statically: the effect of the
    // dependency needs files.read of the root package (E0085).
    p.write(
        "app/unio.toml",
        &(p.read("app/unio.toml") + "\n[permissions]\nenv = []\n"),
    );
    refused(&p.unio(&["check", "main.unio"]), "E0085");
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
#[cfg(unix)]
#[test]
fn the_unpack_worker_writes_only_its_cache_entry() {
    let p = Project::new("unpackprobe");
    p.util("util", "1.0.0");
    p.write("home/secret.txt", "SECRET");
    let probes = [
        format!("write={}", p.path("cache/packages/planted").display()),
        format!("write={}", p.path("app/planted").display()),
        format!("read={}", p.path("home/secret.txt").display()),
        format!("read={}", p.path("app/unio.toml").display()),
        "exec=/bin/sh".to_string(),
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
