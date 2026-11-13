//! The language server `unio lsp` (T-10) and the isolation of its analyses (SB-13): scripted
//! sessions over pipes, as an editor drives the server.
//!
//! The first test is one session: diagnostics with codes and ranges from an unsaved buffer,
//! cleared after the fix; go to definition across modules; hover with the type of a local and
//! the signature and effects of a function; completion of a local and of a member; formatting;
//! rename across the package and its refusal for the standard library; shutdown and exit.
//!
//! The SB-13 tests open hostile projects, as `tests/workers.rs` does for the other tools
//! (SB-15): the probes of `UNIO_WORKER_PROBE` show that the worker of an analysis cannot read
//! outside the package, write, start a process, or use the network, and an analysis over the
//! time or memory limit of `[compile]` gives a diagnostic while the server keeps answering.

// The helpers of the file-system tests are not used on Windows.
#![cfg_attr(windows, allow(dead_code))]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use unio::lsp::json::Json;
use unio::lsp::path_to_uri;

const SECRET: &str = "SECRET-OF-THE-OPERATOR";

/// A project in a temporary directory: a package `pkg/`, a directory `outside/`, and a home
/// directory with a key.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let dir = std::env::temp_dir().join(format!("unio-lsp-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::create_dir_all(dir.join("outside")).unwrap();
        fs::create_dir_all(dir.join("home/.ssh")).unwrap();
        fs::write(dir.join("pkg/unio.toml"), "[package]\nname = \"demo\"\n").unwrap();
        fs::write(
            dir.join("outside/secret.unio"),
            format!("public function leak() returns Text:\n    return \"{SECRET}\"\n"),
        )
        .unwrap();
        fs::write(dir.join("home/.ssh/id_rsa"), SECRET).unwrap();
        Project {
            root: fs::canonicalize(dir).unwrap(),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.path(relative);
        fs::write(&path, text).unwrap();
        path
    }

    fn policy(&self, body: &str) -> PathBuf {
        self.write("policy.toml", body)
    }

    /// `unio lsp` in the package, with the home directory of the project.
    fn server(&self, policy: Option<&Path>, envs: &[(&str, &str)]) -> Client {
        let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
        command
            .arg("lsp")
            .current_dir(self.path("pkg"))
            .env_remove("UNIO_POLICY")
            .env_remove("UNIO_WORKER_PROBE")
            .env_remove("UNIO_LANG")
            .env("HOME", self.path("home"));
        let policy = match policy {
            Some(p) => Some(p.to_path_buf()),
            None if cfg!(windows) => Some(self.policy("")),
            None => None,
        };
        if let Some(policy) = policy {
            command.env("UNIO_POLICY", policy);
        }
        for (k, v) in envs {
            command.env(k, v);
        }
        Client::start(command)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The editor's side of a session.
struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: mpsc::Receiver<Json>,
    /// Notifications and responses not consumed yet.
    pending: Vec<Json>,
    stderr: Arc<Mutex<String>>,
    next: i64,
}

fn frame(message: &Json) -> Vec<u8> {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

impl Client {
    fn start(mut command: Command) -> Client {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (send, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut input = BufReader::new(stdout);
            loop {
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if input.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.strip_prefix("Content-Length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                if input.read_exact(&mut body).is_err() {
                    return;
                }
                let message = Json::parse(&String::from_utf8(body).unwrap()).unwrap();
                if send.send(message).is_err() {
                    return;
                }
            }
        });
        let log = Arc::new(Mutex::new(String::new()));
        let sink = log.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..n]));
            }
        });
        Client {
            child,
            stdin,
            messages,
            pending: Vec::new(),
            stderr: log,
            next: 1,
        }
    }

    fn send(&mut self, message: Json) {
        self.stdin.write_all(&frame(&message)).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Json) {
        self.send(Json::object(vec![
            ("jsonrpc", Json::str("2.0")),
            ("method", Json::str(method)),
            ("params", params),
        ]));
    }

    /// Sends a request and gives its id.
    fn ask(&mut self, method: &str, params: Json) -> i64 {
        let id = self.next;
        self.next += 1;
        self.send(Json::object(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", Json::int(id)),
            ("method", Json::str(method)),
            ("params", params),
        ]));
        id
    }

    /// The first message that satisfies the test, waiting at most `limit`.
    fn wait(&mut self, limit: Duration, what: &str, test: impl Fn(&Json) -> bool) -> Json {
        if let Some(i) = self.pending.iter().position(&test) {
            return self.pending.remove(i);
        }
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.messages.recv_timeout(left) {
                Ok(message) if test(&message) => return message,
                Ok(message) => self.pending.push(message),
                Err(_) => panic!(
                    "no {what} within {limit:?}; received {:?}\nstderr:\n{}",
                    self.pending,
                    self.log()
                ),
            }
        }
    }

    fn response(&mut self, id: i64) -> Json {
        self.wait(Duration::from_secs(60), &format!("response {id}"), |m| {
            m.get("id").and_then(Json::as_i64) == Some(id) && m.get("method").is_none()
        })
    }

    /// The result of a request; panics on an error response.
    fn call(&mut self, method: &str, params: Json) -> Json {
        let id = self.ask(method, params);
        let response = self.response(id);
        match response.get("result") {
            Some(result) => result.clone(),
            None => panic!("{method} failed: {response}\nstderr:\n{}", self.log()),
        }
    }

    /// The next diagnostics published for the URI, as an array.
    fn diagnostics(&mut self, uri: &str, limit: Duration) -> Vec<Json> {
        let message = self.wait(limit, &format!("diagnostics of {uri}"), |m| {
            m.get("method").and_then(Json::as_str) == Some("textDocument/publishDiagnostics")
                && m.at(&["params", "uri"]).and_then(Json::as_str) == Some(uri)
        });
        message
            .at(&["params", "diagnostics"])
            .and_then(Json::as_array)
            .unwrap()
            .to_vec()
    }

    fn log(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn initialize(&mut self, root: &Path) -> Json {
        let result = self.call(
            "initialize",
            Json::object(vec![
                ("processId", Json::Null),
                ("rootUri", Json::str(path_to_uri(root))),
                ("capabilities", Json::object(vec![])),
            ]),
        );
        self.notify("initialized", Json::object(vec![]));
        result
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            Json::object(vec![(
                "textDocument",
                Json::object(vec![
                    ("uri", Json::str(uri)),
                    ("languageId", Json::str("unio")),
                    ("version", Json::int(1)),
                    ("text", Json::str(text)),
                ]),
            )]),
        );
    }

    fn change(&mut self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            Json::object(vec![
                (
                    "textDocument",
                    Json::object(vec![
                        ("uri", Json::str(uri)),
                        ("version", Json::int(version)),
                    ]),
                ),
                (
                    "contentChanges",
                    Json::Array(vec![Json::object(vec![("text", Json::str(text))])]),
                ),
            ]),
        );
    }

    /// Shuts the server down and waits for it to exit with code 0.
    fn finish(mut self) -> String {
        let result = self.call("shutdown", Json::Null);
        assert_eq!(result, Json::Null);
        self.notify("exit", Json::Null);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(0), "stderr:\n{}", self.log());
                break;
            }
            assert!(Instant::now() < deadline, "the server did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The stderr thread ends with the process.
        std::thread::sleep(Duration::from_millis(50));
        self.log()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn position(text: &str, needle: &str, after: usize) -> Json {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {text}"))
        + after;
    let (line, character) = unio::lsp::position_of(text, at);
    Json::object(vec![
        ("line", Json::int(line)),
        ("character", Json::int(character)),
    ])
}

fn at(uri: &str, text: &str, needle: &str, after: usize) -> Json {
    Json::object(vec![
        ("textDocument", Json::object(vec![("uri", Json::str(uri))])),
        ("position", position(text, needle, after)),
    ])
}

fn line_of(range: &Json) -> i64 {
    range.at(&["start", "line"]).and_then(Json::as_i64).unwrap()
}

fn labels(completion: &Json) -> Vec<String> {
    completion
        .get("items")
        .and_then(Json::as_array)
        .unwrap()
        .iter()
        .map(|i| i.get("label").and_then(Json::as_str).unwrap().to_string())
        .collect()
}

const GEOMETRY: &str = "## A point of the plane.
public struct Point:
    public x as Int
    public y as Int

## The squared length of the vector to the point.
public function norm(p as Point) returns Int:
    return p.x * p.x + p.y * p.y
";

const MAIN_SAVED: &str = "import geometry

public function main():
    print(\"saved\")
";

const MAIN_BROKEN: &str = "import geometry

public function main():
    let origin = geometry.Point(x = 1, y = 2)
    let total = geometry.norm(origin)
    print(\"{total} {missing}\")
";

const MAIN_FIXED: &str = "import geometry

public function main():
    let origin = geometry.Point(x = 1, y = 2)
    let total = geometry.norm(origin)
    print(\"{total} {origin.x}\")
";

#[test]
fn a_scripted_session_covers_every_feature() {
    let p = Project::new("session");
    p.write("pkg/geometry.unio", GEOMETRY);
    let main = p.write("pkg/main.unio", MAIN_SAVED);
    let uri = path_to_uri(&main);
    let geometry_uri = path_to_uri(&p.path("pkg/geometry.unio"));
    let mut c = p.server(None, &[]);
    let init = c.initialize(&p.path("pkg"));
    for capability in [
        "hoverProvider",
        "definitionProvider",
        "documentFormattingProvider",
        "renameProvider",
        "inlayHintProvider",
    ] {
        assert_eq!(
            init.at(&["capabilities", capability]),
            Some(&Json::Bool(true)),
            "{init}"
        );
    }

    // The unsaved buffer is analysed, not the file on disk: an unknown name with its code,
    // range, and the link to its explanation.
    c.open(&uri, MAIN_BROKEN);
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(d.get("code").and_then(Json::as_str), Some("E0056"), "{d}");
    let range = d.get("range").unwrap();
    assert_eq!(line_of(range), 5, "{d}");
    assert_eq!(
        range.at(&["start", "character"]).and_then(Json::as_i64),
        Some(20)
    );
    assert_eq!(
        range.at(&["end", "character"]).and_then(Json::as_i64),
        Some(27)
    );
    assert_eq!(
        d.at(&["codeDescription", "href"]).and_then(Json::as_str),
        Some("unio-explain:/E0056")
    );
    assert!(d
        .get("message")
        .and_then(Json::as_str)
        .unwrap()
        .contains("missing"));
    let explanation = c.call(
        "unio/explain",
        Json::object(vec![("code", Json::str("E0056"))]),
    );
    assert!(
        explanation.as_str().unwrap().starts_with("E0056:"),
        "{explanation}"
    );

    // The fix clears them.
    c.change(&uri, 2, MAIN_FIXED);
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    // Go to definition across modules.
    let found = c.call(
        "textDocument/definition",
        at(&uri, MAIN_FIXED, "norm(origin)", 1),
    );
    assert_eq!(
        found.get("uri").and_then(Json::as_str),
        Some(geometry_uri.as_str()),
        "{found}"
    );
    assert_eq!(line_of(found.get("range").unwrap()), 6, "{found}");
    let field = c.call(
        "textDocument/definition",
        at(&uri, MAIN_FIXED, "origin.x}", 7),
    );
    assert_eq!(line_of(field.get("range").unwrap()), 2, "{field}");
    let local = c.call(
        "textDocument/definition",
        at(&uri, MAIN_FIXED, "norm(origin)", 6),
    );
    assert_eq!(local.get("uri").and_then(Json::as_str), Some(uri.as_str()));
    assert_eq!(line_of(local.get("range").unwrap()), 3, "{local}");

    // Hover: the type of a local, the signature, documentation and effects of a function.
    let hover = c.call("textDocument/hover", at(&uri, MAIN_FIXED, "total =", 1));
    let text = hover
        .at(&["contents", "value"])
        .and_then(Json::as_str)
        .unwrap();
    assert!(text.contains("total as Int"), "{text}");
    let hover = c.call(
        "textDocument/hover",
        at(&uri, MAIN_FIXED, "norm(origin)", 2),
    );
    let text = hover
        .at(&["contents", "value"])
        .and_then(Json::as_str)
        .unwrap();
    assert!(
        text.contains("function norm(p as Point) returns Int")
            && text.contains("squared length")
            && text.contains("effects:"),
        "{text}"
    );
    let hover = c.call("textDocument/hover", at(&uri, MAIN_FIXED, "print(", 1));
    let text = hover
        .at(&["contents", "value"])
        .and_then(Json::as_str)
        .unwrap();
    assert!(
        text.contains("effects:") && text.contains("print"),
        "{text}"
    );

    // Type hints: the inferred types of the bindings written without one.
    let hints = c.call(
        "textDocument/inlayHint",
        Json::object(vec![
            (
                "textDocument",
                Json::object(vec![("uri", Json::str(uri.as_str()))]),
            ),
            (
                "range",
                Json::object(vec![
                    ("start", position(MAIN_FIXED, "import", 0)),
                    ("end", position(MAIN_FIXED, "print", 0)),
                ]),
            ),
        ]),
    );
    let hints: Vec<String> = hints
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.get("label").and_then(Json::as_str).unwrap().to_string())
        .collect();
    assert_eq!(hints, [" as Point", " as Int"], "{hints:?}");

    // Completion of a local and of a member, while the line does not parse yet.
    let typing = MAIN_FIXED.replace(
        "    print(\"{total} {origin.x}\")\n",
        "    print(\"{total}\")\n    origin.\n",
    );
    c.change(&uri, 3, &typing);
    let members = c.call(
        "textDocument/completion",
        at(&uri, &typing, "    origin.\n", 11),
    );
    let names = labels(&members);
    assert!(
        names.contains(&"x".to_string()) && names.contains(&"y".to_string()),
        "{names:?}"
    );
    let typing = MAIN_FIXED.replace(
        "    print(\"{total} {origin.x}\")\n",
        "    print(\"{total}\")\n    tot\n",
    );
    c.change(&uri, 4, &typing);
    let names = labels(&c.call("textDocument/completion", at(&uri, &typing, "    tot\n", 7)));
    assert!(names.contains(&"total".to_string()), "{names:?}");
    let names = labels(&c.call("textDocument/completion", at(&uri, &typing, "    tot\n", 4)));
    assert!(
        names.contains(&"origin".to_string())
            && names.contains(&"geometry".to_string())
            && names.contains(&"main".to_string())
            && names.contains(&"while".to_string())
            && names.contains(&"print".to_string()),
        "{names:?}"
    );

    // Formatting gives the text of `unio fmt`.
    let ugly = MAIN_FIXED.replace("x = 1, y = 2", "x = 1+0, y = 2");
    c.change(&uri, 5, &ugly);
    let edits = c.call(
        "textDocument/formatting",
        Json::object(vec![
            (
                "textDocument",
                Json::object(vec![("uri", Json::str(uri.as_str()))]),
            ),
            (
                "options",
                Json::object(vec![
                    ("tabSize", Json::int(4)),
                    ("insertSpaces", Json::Bool(true)),
                ]),
            ),
        ]),
    );
    let edits = edits.as_array().unwrap();
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!(
        edits[0].get("newText").and_then(Json::as_str),
        Some(unio::format::format_source(&ugly).unwrap().as_str())
    );
    assert!(edits[0]
        .get("newText")
        .and_then(Json::as_str)
        .unwrap()
        .contains("1 + 0"));

    // Rename: every reference in the package, in the open buffer and in the file on disk.
    c.change(&uri, 6, MAIN_FIXED);
    let mut params = at(&uri, MAIN_FIXED, "norm(origin)", 1);
    params.set("newName", Json::str("length"));
    let edit = c.call("textDocument/rename", params);
    let changes = edit.get("changes").unwrap();
    let here = changes.get(&uri).and_then(Json::as_array).unwrap();
    assert_eq!(here.len(), 1, "{edit}");
    let there = changes.get(&geometry_uri).and_then(Json::as_array).unwrap();
    assert_eq!(there.len(), 1, "{edit}");
    assert_eq!(line_of(there[0].get("range").unwrap()), 6);
    assert_eq!(
        there[0].get("newText").and_then(Json::as_str),
        Some("length")
    );
    let mut params = at(&uri, MAIN_FIXED, "origin =", 1);
    params.set("newName", Json::str("start"));
    let edit = c.call("textDocument/rename", params);
    assert_eq!(
        edit.at(&["changes"])
            .unwrap()
            .get(&uri)
            .and_then(Json::as_array)
            .unwrap()
            .len(),
        3,
        "{edit}"
    );
    let mut params = at(&uri, MAIN_FIXED, "x = 1", 0);
    params.set("newName", Json::str("left"));
    let edit = c.call("textDocument/rename", params);
    let changes = edit.get("changes").unwrap();
    assert_eq!(
        changes.get(&uri).and_then(Json::as_array).unwrap().len(),
        2,
        "{edit}"
    );
    assert_eq!(
        changes
            .get(&geometry_uri)
            .and_then(Json::as_array)
            .unwrap()
            .len(),
        3,
        "{edit}"
    );

    // Rename is refused for the standard library and for a name that is not a name.
    for (needle, name) in [("print(", "show"), ("total =", "while")] {
        let mut params = at(&uri, MAIN_FIXED, needle, 1);
        params.set("newName", Json::str(name));
        let id = c.ask("textDocument/rename", params);
        let response = c.response(id);
        assert!(response.get("error").is_some(), "{response}");
    }
    // The file on disk was never changed by the server.
    assert_eq!(fs::read_to_string(&main).unwrap(), MAIN_SAVED);

    // Closing clears the diagnostics.
    c.notify(
        "textDocument/didClose",
        Json::object(vec![(
            "textDocument",
            Json::object(vec![("uri", Json::str(uri.as_str()))]),
        )]),
    );
    let closed = c.wait(Duration::from_secs(10), "the diagnostics of closing", |m| {
        m.get("method").and_then(Json::as_str) == Some("textDocument/publishDiagnostics")
            && m.at(&["params", "uri"]).and_then(Json::as_str) == Some(uri.as_str())
            && m.at(&["params", "version"]).is_none()
    });
    assert_eq!(
        closed.at(&["params", "diagnostics"]),
        Some(&Json::Array(Vec::new()))
    );
    c.finish();
}

/// SB-13: the worker of every analysis has the rules of SB-11, shown by the probes past the
/// compiler: it reads the package, and nothing outside it, not even through a link; it
/// writes nothing, starts no process, and opens no socket. Nothing of the secret reaches the
/// editor.
#[cfg(unix)]
#[test]
fn a_hostile_project_cannot_reach_outside_through_the_server() {
    let p = Project::new("hostile");
    std::os::unix::fs::symlink(p.path("outside/secret.unio"), p.path("pkg/lib.unio")).unwrap();
    let main = p.write(
        "pkg/main.unio",
        "import lib\n\npublic function main():\n    print(lib.leak())\n",
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let op = |name: &str, path: &Path| format!("{name}={}", path.display());
    let denied = vec![
        op("read", &p.path("outside/secret.unio")),
        op("read", &p.path("pkg/lib.unio")),
        op("read", &p.path("home/.ssh/id_rsa")),
        op("list", &p.path("home")),
        "read=/etc/passwd".to_string(),
        op("write", &p.path("pkg/new.unio")),
        op("write", &p.path("pkg/main.unio")),
        op("write", &p.path("outside/escaped.txt")),
        "exec=/bin/sh".to_string(),
        "fork".to_string(),
        format!("connect={address}"),
        "udp".to_string(),
    ];
    let mut probes = denied.clone();
    probes.push(op("read", &main));
    let mut c = p.server(None, &[("UNIO_WORKER_PROBE", &probes.join(";"))]);
    c.initialize(&p.path("pkg"));
    let uri = path_to_uri(&main);
    let text = fs::read_to_string(&main).unwrap();
    c.open(&uri, &text);
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    let shown = format!("{diagnostics:?}");
    assert!(shown.contains("E0053"), "{shown}\n{}", c.log());
    // Requests run in the same kind of worker.
    let hover = c.call("textDocument/hover", at(&uri, &text, "leak", 1));
    let definition = c.call("textDocument/definition", at(&uri, &text, "leak", 1));
    let completion = c.call("textDocument/completion", at(&uri, &text, "lib.leak", 4));
    let log = c.finish();
    for operation in &denied {
        let prefix = format!("unio worker probe: {operation}: ");
        let result = log
            .lines()
            .find_map(|l| l.strip_prefix(prefix.as_str()))
            .unwrap_or_else(|| panic!("no probe {operation}:\n{log}"));
        assert!(
            result.starts_with("denied"),
            "{operation} was {result}:\n{log}"
        );
    }
    let prefix = format!("unio worker probe: {}: ", op("read", &main));
    assert!(
        log.lines().any(|l| l
            .strip_prefix(prefix.as_str())
            .is_some_and(|r| r.starts_with("allowed"))),
        "{log}"
    );
    for answer in [
        shown,
        hover.to_string(),
        definition.to_string(),
        completion.to_string(),
        log,
    ] {
        assert!(!answer.contains(SECRET), "the secret leaked: {answer}");
    }
    assert!(!p.path("outside/escaped.txt").exists());
    assert!(!p.path("pkg/new.unio").exists());
    assert!(
        listener.accept().is_err(),
        "the worker reached the listener"
    );
}

/// SB-13: an analysis that runs longer than `compile.wall_seconds` is stopped and reported as
/// a diagnostic, and the server answers meanwhile and afterwards; a request over the limit is
/// an error response, not a hang.
#[test]
fn an_analysis_over_the_time_limit_is_a_diagnostic_and_the_server_answers() {
    let p = Project::new("time");
    let main = p.write(
        "pkg/main.unio",
        "public function main():\n    print(\"hi\")\n",
    );
    let policy = p.policy("[compile]\nwall_seconds = 2\n");
    let mut c = p.server(Some(&policy), &[("UNIO_WORKER_PROBE", "spin")]);
    c.initialize(&p.path("pkg"));
    let uri = path_to_uri(&main);
    let text = fs::read_to_string(&main).unwrap();
    let started = Instant::now();
    c.open(&uri, &text);
    // While the worker spins, the server answers.
    let explanation = c.call(
        "unio/explain",
        Json::object(vec![("code", Json::str("E0001"))]),
    );
    assert!(explanation.as_str().is_some());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let hover = c.ask("textDocument/hover", at(&uri, &text, "print", 1));
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(30));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let message = diagnostics[0]
        .get("message")
        .and_then(Json::as_str)
        .unwrap();
    assert!(
        message.contains("stopped") && message.contains("compile.wall_seconds"),
        "{message}"
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    let response = c.response(hover);
    assert!(response.get("error").is_some(), "{response}");
    let log = c.finish();
    assert!(log.contains("time limit of 2 s"), "{log}");
}

/// SB-13: an analysis over `compile.memory_bytes` is stopped by the system and reported as a
/// diagnostic.
#[test]
fn an_analysis_over_the_memory_limit_is_a_diagnostic() {
    let p = Project::new("memory");
    let main = p.write(
        "pkg/main.unio",
        "public function main():\n    print(\"hi\")\n",
    );
    let policy = p.policy("[compile]\nmemory_bytes = 536870912\nwall_seconds = 60\n");
    let mut c = p.server(Some(&policy), &[("UNIO_WORKER_PROBE", "alloc=1073741824")]);
    c.initialize(&p.path("pkg"));
    let uri = path_to_uri(&main);
    c.open(&uri, &fs::read_to_string(&main).unwrap());
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let message = diagnostics[0]
        .get("message")
        .and_then(Json::as_str)
        .unwrap();
    assert!(message.contains("compile.memory_bytes"), "{message}");
    let log = c.finish();
    assert!(log.contains("memory limit of 536870912 bytes"), "{log}");
}

/// A policy whose compile memory cannot be honoured stops the server at its start, as every
/// tool (RFC 0006).
#[test]
fn a_policy_below_the_minimum_stops_the_server() {
    let p = Project::new("minimum");
    let policy = p.policy("[compile]\nmemory_bytes = 1048576\n");
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .args(["lsp", "--policy", &policy.to_string_lossy()])
        .current_dir(p.path("pkg"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(103));
    assert!(String::from_utf8_lossy(&output.stderr).contains("below the minimum"));
}

/// T-6, E0088: a mistake of the manifest is a diagnostic of the editor as of `unio check`: in a
/// source document of the package at its start, naming the line of the manifest as related
/// information, and in the manifest itself at that line; the fix clears it.
#[test]
fn a_mistake_of_the_manifest_is_a_diagnostic_of_the_package() {
    let p = Project::new("manifest");
    let manifest = p.write(
        "pkg/unio.toml",
        "[package]\nname = \"demo\"\n\n[permissions]\nfiles.read = true\n",
    );
    let main = p.write(
        "pkg/main.unio",
        "public function main():\n    print(\"hi\")\n",
    );
    let mut c = p.server(None, &[]);
    c.initialize(&p.path("pkg"));
    let uri = path_to_uri(&main);
    let manifest_uri = path_to_uri(&manifest);
    c.open(&uri, &fs::read_to_string(&main).unwrap());
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(d.get("code").and_then(Json::as_str), Some("E0088"), "{d}");
    assert_eq!(line_of(d.get("range").unwrap()), 0, "{d}");
    let message = d.get("message").and_then(Json::as_str).unwrap();
    assert!(
        message.starts_with("unio.toml:5:") && message.contains("files.read"),
        "{message}"
    );
    let related = &d
        .get("relatedInformation")
        .and_then(Json::as_array)
        .unwrap()[0];
    assert_eq!(
        related.at(&["location", "uri"]).and_then(Json::as_str),
        Some(manifest_uri.as_str()),
        "{related}"
    );
    assert_eq!(line_of(related.at(&["location", "range"]).unwrap()), 4);

    // The manifest opened as a document has the diagnostic at its line; fixing the unsaved
    // buffer clears it in both.
    let text = fs::read_to_string(&manifest).unwrap();
    c.open(&manifest_uri, &text);
    let diagnostics = c.diagnostics(&manifest_uri, Duration::from_secs(60));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(line_of(diagnostics[0].get("range").unwrap()), 4);
    c.change(
        &manifest_uri,
        2,
        &text.replace("files.read = true", "files.read = [\"$CWD/**\"]"),
    );
    let diagnostics = c.diagnostics(&manifest_uri, Duration::from_secs(60));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    c.change(&uri, 2, "public function main():\n    print(\"hello\")\n");
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    c.finish();
}

#[test]
fn fixes_are_bound_to_the_revision_of_their_diagnostics() {
    let p = Project::new("fixes");
    let text = "enum C:\n    List\n    Show\n\nfunction f() returns C:\n    return List\n\nfunction g() returns Int:\n    return \"a\"\n";
    let main = p.write("pkg/main.unio", text);
    let uri = path_to_uri(&main);
    let mut c = p.server(None, &[]);
    let init = c.initialize(&p.path("pkg"));
    assert!(
        init.at(&["capabilities", "codeActionProvider"]).is_some(),
        "{init}"
    );
    c.open(&uri, text);
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    let variant = diagnostics
        .iter()
        .find(|d| d.get("code").and_then(Json::as_str) == Some("E0072"))
        .unwrap();
    // Each diagnostic names the revision it was made for; a type mismatch the expected type.
    assert_eq!(
        variant.at(&["data", "revision"]).and_then(Json::as_i64),
        Some(1)
    );
    let mismatch = diagnostics
        .iter()
        .find(|d| d.get("code").and_then(Json::as_str) == Some("E0061"))
        .unwrap();
    assert_eq!(
        mismatch.at(&["data", "expected"]).and_then(Json::as_str),
        Some("Int"),
        "{mismatch}"
    );
    let ask = |d: &Json| {
        Json::object(vec![
            (
                "textDocument",
                Json::object(vec![("uri", Json::str(uri.clone()))]),
            ),
            ("range", d.get("range").unwrap().clone()),
            (
                "context",
                Json::object(vec![("diagnostics", Json::Array(vec![d.clone()]))]),
            ),
        ])
    };
    let actions = c.call("textDocument/codeAction", ask(variant));
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    let change = &actions[0]
        .at(&["edit", "documentChanges"])
        .unwrap()
        .as_array()
        .unwrap()[0];
    assert_eq!(
        change
            .at(&["textDocument", "version"])
            .and_then(Json::as_i64),
        Some(1),
        "{change}"
    );
    let edit = &change.get("edits").unwrap().as_array().unwrap()[0];
    assert_eq!(edit.get("newText").and_then(Json::as_str), Some("C.List"));
    assert_eq!(line_of(edit.get("range").unwrap()), 5);
    // After a change the old diagnostic offers nothing until the checks ran again.
    let changed = text.replace("return \"a\"", "return 1");
    c.change(&uri, 2, &changed);
    let stale = c.call("textDocument/codeAction", ask(variant));
    assert_eq!(stale.as_array().map(|a| a.len()), Some(0), "{stale}");
    let again = c.diagnostics(&uri, Duration::from_secs(60));
    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(
        again[0].at(&["data", "revision"]).and_then(Json::as_i64),
        Some(2)
    );
    let fresh = c.call("textDocument/codeAction", ask(&again[0]));
    assert_eq!(fresh.as_array().map(|a| a.len()), Some(1), "{fresh}");
}

/// T-13: an incomplete-code diagnostic carries the type the place expects: a syntax error where
/// an expression is missing or a bracket is left open, found by checking a name put in the place
/// of the missing code, and a missing argument, field, or return value.
#[test]
fn incomplete_code_diagnostics_carry_the_expected_type() {
    let p = Project::new("expected");
    let cases = [
        ("let_without_value", "function f() returns Int:\n    let n as Int =\n    return n\n", "E0012", "Int"),
        ("open_call", "function g(t as Text) returns Int:\n    return 1\n\nfunction f() returns Int:\n    return g(\n", "E0009", "Text"),
        ("missing_argument", "function g(a as Int, b as Bool) returns Int:\n    return a\n\nfunction f() returns Int:\n    return g(1)\n", "E0064", "Bool"),
        ("missing_field", "struct P:\n    x as Int\n    name as Text\n\nfunction f() returns P:\n    return P(x = 1)\n", "E0063", "Text"),
        ("missing_return", "function f() returns Float:\n    pass\n", "E0082", "Float"),
        ("return_without_value", "function f() returns Bool:\n    return\n", "E0061", "Bool"),
    ];
    let mut c = p.server(None, &[]);
    c.initialize(&p.path("pkg"));
    for (name, text, code, expected) in cases {
        let main = p.write(&format!("pkg/{name}.unio"), text);
        let uri = path_to_uri(&main);
        c.open(&uri, text);
        let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
        let found = diagnostics
            .iter()
            .find(|d| d.get("code").and_then(Json::as_str) == Some(code))
            .unwrap_or_else(|| panic!("{name}: no {code} in {diagnostics:?}"));
        assert_eq!(
            found.at(&["data", "expected"]).and_then(Json::as_str),
            Some(expected),
            "{name}: {found}"
        );
    }
}

/// C-8: the related locations of a diagnostic reach the editor as related information.
#[test]
fn related_locations_are_related_information() {
    let p = Project::new("related");
    let text =
        "function f() returns Int:\n    return 1\n\nfunction f() returns Int:\n    return 2\n";
    let main = p.write("pkg/main.unio", text);
    let uri = path_to_uri(&main);
    let mut c = p.server(None, &[]);
    c.initialize(&p.path("pkg"));
    c.open(&uri, text);
    let diagnostics = c.diagnostics(&uri, Duration::from_secs(60));
    let duplicate = diagnostics
        .iter()
        .find(|d| d.get("code").and_then(Json::as_str) == Some("E0051"))
        .unwrap();
    let related = duplicate
        .get("relatedInformation")
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("{duplicate}"));
    assert_eq!(related.len(), 1, "{duplicate}");
    assert_eq!(
        related[0].at(&["location", "uri"]).and_then(Json::as_str),
        Some(uri.as_str())
    );
    assert_eq!(line_of(related[0].at(&["location", "range"]).unwrap()), 0);
    assert_eq!(
        related[0].get("message").and_then(Json::as_str),
        Some("'f' is first declared here")
    );
}
