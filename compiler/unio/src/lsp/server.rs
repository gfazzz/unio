//! `unio lsp`: the language server over standard input and output (T-10, SB-13;
//! docs/runtime.md 9.7, item 7). Part of the `unio` command, not of the library, since it
//! starts the workers of the tools (`worker.rs`).
//!
//! The server only speaks the protocol (JSON-RPC 2.0 with `Content-Length` framing) and holds
//! the open documents. It reads no source: every analysis runs in a worker with the scopes and
//! the `[compile]` limits of every tool, which reads the package of the document and the
//! standard library and receives the request and the unsaved buffers of that package on its
//! standard input, after its configuration ([`worker::Spec::input`]). A worker stopped at a
//! limit, or one that fails, gives a diagnostic or an error response; the server answers other
//! messages meanwhile, since each analysis waits for its worker on a thread of its own, and the
//! diagnostics of a document run one at a time, the last change again when they end.

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{mpsc, Arc, Mutex};

use unio::lsp::json::Json;
use unio::lsp::{uri_to_path, Kind, Request};
use unio::{sandbox_cli, Lang};

use crate::worker;

/// The largest message the server accepts.
const MAX_MESSAGE: usize = 256 << 20;

/// `unio lsp [--policy FILE] [--allow-weaker-sandbox] [--lang=en|ru] [--stdio]`, or, in its
/// worker, one analysis.
pub fn main(arguments: &[String]) -> ExitCode {
    if worker::inside() {
        return analyse();
    }
    let (sandbox, rest) = match sandbox_cli::take(arguments) {
        Ok(taken) => taken,
        Err(problem) => return crate::usage(&problem),
    };
    let mut lang = std::env::var("UNIO_LANG")
        .ok()
        .and_then(|v| Lang::parse(&v));
    for argument in &rest {
        if let Some(value) = argument.strip_prefix("--lang=") {
            match Lang::parse(value) {
                Some(l) => lang = Some(l),
                None => return crate::usage(&format!("unknown language '{value}'; use en or ru")),
            }
        } else if argument != "--stdio" {
            return crate::usage(&format!("unio lsp: unknown argument '{argument}'"));
        }
    }
    // The policy and its limits are read once here, so that a policy that cannot be honoured
    // stops the server at its start, as it stops every tool (RFC 0006).
    if let Err(code) = worker::Spec::for_files(&[PathBuf::from("unio.toml")], &sandbox) {
        return ExitCode::from(code);
    }
    Server::new(sandbox, lang).serve()
}

/// The worker's side: decodes the request from its input, answers it on the stack of the
/// front end, and writes the result as JSON on standard output.
fn analyse() -> ExitCode {
    let Some(request) = Request::decode(worker::input()) else {
        eprintln!("unio lsp: the worker received a malformed request");
        return ExitCode::from(2);
    };
    let result = crate::on_stack(crate::FRONT_END_STACK, move || {
        unio::lsp::analysis::answer(&request).to_string()
    });
    let mut out = std::io::stdout().lock();
    if out
        .write_all(result.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------- framing

/// Reads one message: headers up to an empty line, then `Content-Length` bytes. None at the
/// end of the input.
fn read_message(input: &mut impl BufRead) -> Option<Result<Json, String>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        match input.by_ref().take(64 * 1024).read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let length = length?;
    if length > MAX_MESSAGE {
        return None;
    }
    let mut body = vec![0u8; length];
    input.read_exact(&mut body).ok()?;
    Some(
        String::from_utf8(body)
            .map_err(|_| "the message is not UTF-8".to_string())
            .and_then(|text| Json::parse(&text)),
    )
}

/// The writer of messages, shared by the threads that answer.
#[derive(Clone)]
struct Output(Arc<Mutex<std::io::Stdout>>);

impl Output {
    fn send(&self, message: &Json) {
        let body = message.to_string();
        let mut out = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = out.flush();
    }

    fn respond(&self, id: &Json, result: Json) {
        self.send(&Json::object(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", id.clone()),
            ("result", result),
        ]));
    }

    fn fail(&self, id: &Json, code: i64, message: &str) {
        self.send(&Json::object(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", id.clone()),
            (
                "error",
                Json::object(vec![
                    ("code", Json::int(code)),
                    ("message", Json::str(message)),
                ]),
            ),
        ]));
    }

    fn notify(&self, method: &str, params: Json) {
        self.send(&Json::object(vec![
            ("jsonrpc", Json::str("2.0")),
            ("method", Json::str(method)),
            ("params", params),
        ]));
    }
}

/// Error codes of JSON-RPC and LSP.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const REQUEST_FAILED: i64 = -32803;

// ---------------------------------------------------------------- the server

struct Document {
    path: Option<PathBuf>,
    text: String,
    version: i64,
}

#[derive(Default)]
struct Diagnosing {
    running: bool,
    /// The document changed while its diagnostics ran.
    again: bool,
}

enum Event {
    Message(Result<Json, String>),
    End,
    Diagnosed {
        uri: String,
        version: i64,
        items: Json,
    },
}

struct Server {
    sandbox: sandbox_cli::Options,
    lang: Option<Lang>,
    out: Output,
    documents: HashMap<String, Document>,
    diagnosing: HashMap<String, Diagnosing>,
    shut_down: bool,
    events: mpsc::Sender<Event>,
    receiver: Option<mpsc::Receiver<Event>>,
}

impl Server {
    fn new(sandbox: sandbox_cli::Options, lang: Option<Lang>) -> Server {
        let (events, receiver) = mpsc::channel();
        Server {
            sandbox,
            lang,
            out: Output(Arc::new(Mutex::new(std::io::stdout()))),
            documents: HashMap::new(),
            diagnosing: HashMap::new(),
            shut_down: false,
            events,
            receiver: Some(receiver),
        }
    }

    fn lang(&self) -> Lang {
        self.lang.unwrap_or(Lang::En)
    }

    fn serve(mut self) -> ExitCode {
        let messages = self.events.clone();
        std::thread::spawn(move || {
            let mut input = std::io::BufReader::new(std::io::stdin());
            loop {
                match read_message(&mut input) {
                    Some(message) => {
                        if messages.send(Event::Message(message)).is_err() {
                            return;
                        }
                    }
                    None => {
                        let _ = messages.send(Event::End);
                        return;
                    }
                }
            }
        });
        let receiver = self.receiver.take().expect("the events");
        for event in receiver {
            match event {
                Event::Message(Ok(message)) => {
                    if let Some(code) = self.handle(message) {
                        return code;
                    }
                }
                Event::Message(Err(problem)) => self.out.fail(&Json::Null, PARSE_ERROR, &problem),
                Event::End => break,
                Event::Diagnosed {
                    uri,
                    version,
                    items,
                } => self.diagnosed(uri, version, items),
            }
        }
        // The input ended without `exit`.
        ExitCode::from(1)
    }

    /// Handles one message; Some(code) when the server ends.
    fn handle(&mut self, message: Json) -> Option<ExitCode> {
        let method = message.get("method").and_then(Json::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        if method == "exit" {
            return Some(ExitCode::from(if self.shut_down { 0 } else { 1 }));
        }
        let Some(id) = id else {
            self.notification(method, &params);
            return None;
        };
        if method.is_empty() {
            // A response to a request of the server: it sends none.
            return None;
        }
        if self.shut_down {
            self.out
                .fail(&id, INVALID_REQUEST, "the server has been shut down");
            return None;
        }
        match method {
            "initialize" => {
                if self.lang.is_none() {
                    let locale = params.get("locale").and_then(Json::as_str).unwrap_or("");
                    if locale.starts_with("ru") {
                        self.lang = Some(Lang::Ru);
                    }
                }
                self.out.respond(&id, capabilities());
            }
            "shutdown" => {
                self.shut_down = true;
                self.out.respond(&id, Json::Null);
            }
            "unio/explain" => {
                // Reads no source, as `unio explain`: answered here.
                let code = params.get("code").and_then(Json::as_str).unwrap_or("");
                match unio::catalog::explain(code, self.lang()) {
                    Some(text) => self.out.respond(&id, Json::str(text)),
                    None => self.out.fail(&id, INVALID_PARAMS, "unknown code"),
                }
            }
            "textDocument/hover" => self.request(id, &params, Kind::Hover),
            "textDocument/definition" => self.request(id, &params, Kind::Definition),
            "textDocument/completion" => self.request(id, &params, Kind::Completion),
            "textDocument/formatting" => self.request(id, &params, Kind::Formatting),
            "textDocument/rename" => self.request(id, &params, Kind::Rename),
            "textDocument/inlayHint" => self.request(id, &params, Kind::InlayHints),
            _ => self
                .out
                .fail(&id, METHOD_NOT_FOUND, &format!("unknown method {method}")),
        }
        None
    }

    fn notification(&mut self, method: &str, params: &Json) {
        let uri = params
            .at(&["textDocument", "uri"])
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        match method {
            "textDocument/didOpen" => {
                let text = params
                    .at(&["textDocument", "text"])
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string();
                let version = params
                    .at(&["textDocument", "version"])
                    .and_then(Json::as_i64)
                    .unwrap_or(0);
                self.documents.insert(
                    uri.clone(),
                    Document {
                        path: uri_to_path(&uri),
                        text,
                        version,
                    },
                );
                self.diagnose(uri);
            }
            "textDocument/didChange" => {
                let version = params
                    .at(&["textDocument", "version"])
                    .and_then(Json::as_i64);
                // Full synchronisation: the last change holds the whole text.
                let text = params
                    .get("contentChanges")
                    .and_then(Json::as_array)
                    .and_then(|c| c.last())
                    .and_then(|c| c.get("text"))
                    .and_then(Json::as_str)
                    .map(str::to_string);
                if let (Some(document), Some(text)) = (self.documents.get_mut(&uri), text) {
                    document.text = text;
                    document.version = version.unwrap_or(document.version + 1);
                    self.diagnose(uri);
                }
            }
            "textDocument/didSave" if self.documents.contains_key(&uri) => self.diagnose(uri),
            "textDocument/didClose" if self.documents.remove(&uri).is_some() => {
                self.out.notify(
                    "textDocument/publishDiagnostics",
                    Json::object(vec![
                        ("uri", Json::str(uri)),
                        ("diagnostics", Json::Array(Vec::new())),
                    ]),
                );
            }
            _ => {}
        }
    }

    /// The request of an analysis of the document, with the unsaved buffers of its package.
    fn analysis(
        &self,
        uri: &str,
        kind: Kind,
        line: u32,
        character: u32,
        new_name: String,
    ) -> Option<(Request, PathBuf)> {
        let document = self.documents.get(uri);
        let path = match document {
            Some(d) => d.path.clone()?,
            None => uri_to_path(uri)?,
        };
        let package = package_of(&path);
        let mut buffers: Vec<(PathBuf, String)> = self
            .documents
            .values()
            .filter_map(|d| {
                let p = d.path.as_ref()?;
                package_of(p)
                    .eq(&package)
                    .then(|| (p.clone(), d.text.clone()))
            })
            .collect();
        buffers.sort();
        Some((
            Request {
                kind,
                path: path.clone(),
                line,
                character,
                new_name,
                lang: self.lang(),
                buffers,
            },
            path,
        ))
    }

    fn request(&mut self, id: Json, params: &Json, kind: Kind) {
        let uri = params
            .at(&["textDocument", "uri"])
            .and_then(Json::as_str)
            .unwrap_or("");
        let number = |key: &str| {
            params
                .at(&["position", key])
                .and_then(Json::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0)
        };
        let new_name = params
            .get("newName")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let Some((request, path)) =
            self.analysis(uri, kind, number("line"), number("character"), new_name)
        else {
            // Not a file of a package: nothing to analyse.
            self.out.respond(&id, Json::Null);
            return;
        };
        let out = self.out.clone();
        let sandbox = self.sandbox.clone();
        let lang = self.lang();
        std::thread::spawn(move || match run_worker(&request, &path, &sandbox, lang) {
            Ok(result) => match result.get("error").and_then(Json::as_str) {
                Some(problem) => out.fail(&id, REQUEST_FAILED, problem),
                None if !within_reach(kind, &result, &path) => out.fail(
                    &id,
                    REQUEST_FAILED,
                    "the analysis worker named a file outside the package of the document",
                ),
                None => out.respond(&id, result),
            },
            Err(problem) => out.fail(&id, REQUEST_FAILED, &problem),
        });
    }

    /// Runs the diagnostics of the document, or once more after the run in progress.
    fn diagnose(&mut self, uri: String) {
        let state = self.diagnosing.entry(uri.clone()).or_default();
        if state.running {
            state.again = true;
            return;
        }
        let Some(document) = self.documents.get(&uri) else {
            return;
        };
        let version = document.version;
        let Some((request, path)) = self.analysis(&uri, Kind::Diagnostics, 0, 0, String::new())
        else {
            return;
        };
        let state = self.diagnosing.entry(uri.clone()).or_default();
        state.running = true;
        let events = self.events.clone();
        let sandbox = self.sandbox.clone();
        let lang = self.lang();
        std::thread::spawn(move || {
            let items = match run_worker(&request, &path, &sandbox, lang) {
                Ok(result) => match result.get("diagnostics") {
                    Some(items @ Json::Array(_)) => items.clone(),
                    _ => Json::Array(vec![worker_diagnostic(
                        "the analysis worker returned no diagnostics",
                    )]),
                },
                Err(problem) => Json::Array(vec![worker_diagnostic(&problem)]),
            };
            let _ = events.send(Event::Diagnosed {
                uri,
                version,
                items,
            });
        });
    }

    fn diagnosed(&mut self, uri: String, version: i64, items: Json) {
        let again = match self.diagnosing.get_mut(&uri) {
            Some(state) => {
                state.running = false;
                std::mem::take(&mut state.again)
            }
            None => false,
        };
        let current = self.documents.get(&uri).map(|d| d.version);
        if current == Some(version) {
            self.out.notify(
                "textDocument/publishDiagnostics",
                Json::object(vec![
                    ("uri", Json::str(uri.clone())),
                    ("version", Json::int(version)),
                    ("diagnostics", items),
                ]),
            );
        }
        if again || current.is_some_and(|v| v != version) {
            self.diagnose(uri);
        }
    }
}

/// The package of a file as the workers find it: the nearest directory with `unio.toml`.
fn package_of(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    dir.ancestors()
        .find(|a| a.join("unio.toml").is_file())
        .map(Path::to_path_buf)
        .unwrap_or(dir)
}

/// Whether the path lies in the directory, by the paths as written or as resolved.
fn lies_in(path: &Path, dir: &Path) -> bool {
    if path.starts_with(dir) {
        return true;
    }
    match (std::fs::canonicalize(path), std::fs::canonicalize(dir)) {
        (Ok(p), Ok(d)) => p.starts_with(d),
        _ => false,
    }
}

/// Whether the files a result names are those the analysis may name: the edits of a rename
/// only `.unio` files of the document's package, a definition also the standard library. The
/// worker runs on untrusted input, so the server does not let even a worker gone wrong lead the
/// editor to change a file outside the package (SB-13).
fn within_reach(kind: Kind, result: &Json, document: &Path) -> bool {
    let package = package_of(document);
    let named = |uri: &str, library: bool| {
        uri_to_path(uri).is_some_and(|p| {
            let source = p.extension().is_some_and(|e| e == "unio");
            let stdlib =
                library && unio::program::trusted_stdlib().is_some_and(|dir| lies_in(&p, &dir));
            source && (lies_in(&p, &package) || stdlib)
        })
    };
    match kind {
        Kind::Rename => match result.get("changes") {
            Some(Json::Object(changes)) => changes.iter().all(|(uri, _)| named(uri, false)),
            Some(_) => false,
            // No edit at all; any other form of edit is not one the worker makes.
            None => *result == Json::Null,
        },
        Kind::Definition => match result.get("uri").and_then(Json::as_str) {
            Some(uri) => named(uri, true),
            None => true,
        },
        _ => true,
    }
}

fn capabilities() -> Json {
    Json::object(vec![
        (
            "capabilities",
            Json::object(vec![
                (
                    "textDocumentSync",
                    Json::object(vec![
                        ("openClose", Json::Bool(true)),
                        ("change", Json::int(1)),
                        (
                            "save",
                            Json::object(vec![("includeText", Json::Bool(false))]),
                        ),
                    ]),
                ),
                ("hoverProvider", Json::Bool(true)),
                ("definitionProvider", Json::Bool(true)),
                (
                    "completionProvider",
                    Json::object(vec![(
                        "triggerCharacters",
                        Json::Array(vec![Json::str(".")]),
                    )]),
                ),
                ("documentFormattingProvider", Json::Bool(true)),
                ("renameProvider", Json::Bool(true)),
                ("inlayHintProvider", Json::Bool(true)),
            ]),
        ),
        (
            "serverInfo",
            Json::object(vec![
                ("name", Json::str("unio")),
                ("version", Json::str(env!("CARGO_PKG_VERSION"))),
            ]),
        ),
    ])
}

/// The diagnostic of an analysis that did not finish, at the start of the document.
fn worker_diagnostic(problem: &str) -> Json {
    Json::object(vec![
        ("range", unio::lsp::range("", 0, 0)),
        ("severity", Json::int(1)),
        ("source", Json::str("unio")),
        ("message", Json::str(problem)),
    ])
}

/// Runs one analysis in a worker of the tools and gives its result, checked as JSON.
fn run_worker(
    request: &Request,
    path: &Path,
    sandbox: &sandbox_cli::Options,
    lang: Lang,
) -> Result<Json, String> {
    let ru = lang == Lang::Ru;
    let mut spec = worker::Spec::for_files(&[path.to_path_buf()], sandbox).map_err(|code| {
        if ru {
            format!("анализ не начат: политику или песочницу нельзя применить (код {code}, SB-11)")
        } else {
            format!("the analysis did not start: the policy or the sandbox cannot be applied (code {code}, SB-11)")
        }
    })?;
    spec.input = request.encode();
    let temp = worker::TempDir::new()?;
    match worker::start("tool", &["lsp".to_string()], &spec, &temp, true, &[]) {
        Ok(finished) if finished.code == 0 => String::from_utf8(finished.data)
            .map_err(|_| "the analysis worker returned malformed data".to_string())
            .and_then(|text| Json::parse(&text))
            .map_err(|problem| format!("the analysis worker returned malformed data: {problem}")),
        Ok(finished) => Err(if ru {
            format!(
                "рабочий процесс анализа завершился с кодом {}",
                finished.code
            )
        } else {
            format!("the analysis worker ended with code {}", finished.code)
        }),
        Err(worker::EXIT_LIMIT) => Err(if ru {
            "анализ остановлен: рабочий процесс превысил предел таблицы [compile] политики \
             (compile.memory_bytes, compile.wall_seconds, compile.cpu_seconds; SB-11, RFC 0006)"
                .to_string()
        } else {
            "the analysis was stopped: its worker went over a limit of the [compile] table of \
             the policy (compile.memory_bytes, compile.wall_seconds, compile.cpu_seconds; SB-11, \
             RFC 0006)"
                .to_string()
        }),
        Err(code) => Err(if ru {
            format!("рабочий процесс анализа не выполнен (код {code}; SB-5, SB-11)")
        } else {
            format!("the analysis worker did not run (code {code}; SB-5, SB-11)")
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_may_change_only_sources_of_the_package() {
        let dir = std::env::temp_dir().join(format!("unio-lsp-reach-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::write(dir.join("pkg/unio.toml"), "").unwrap();
        let document = dir.join("pkg/main.unio");
        let edit = |path: &Path| {
            Json::object(vec![(
                "changes",
                Json::Object(vec![(
                    unio::lsp::path_to_uri(path),
                    Json::Array(Vec::new()),
                )]),
            )])
        };
        assert!(within_reach(
            Kind::Rename,
            &edit(&dir.join("pkg/a.unio")),
            &document
        ));
        assert!(!within_reach(
            Kind::Rename,
            &edit(&dir.join("other.unio")),
            &document
        ));
        assert!(!within_reach(
            Kind::Rename,
            &edit(&dir.join("pkg/unio.toml")),
            &document
        ));
        assert!(within_reach(Kind::Rename, &Json::Null, &document));
        let other = Json::object(vec![("documentChanges", Json::Array(Vec::new()))]);
        assert!(!within_reach(Kind::Rename, &other, &document));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn messages_are_framed_by_their_length() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"x"}"#;
        let text = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{body}Content-Length: 2\r\n\r\n{{}}",
            body.len()
        );
        let mut input = std::io::Cursor::new(text.into_bytes());
        let first = read_message(&mut input).unwrap().unwrap();
        assert_eq!(first.get("method").and_then(Json::as_str), Some("x"));
        assert_eq!(
            read_message(&mut input).unwrap().unwrap(),
            Json::Object(Vec::new())
        );
        assert!(read_message(&mut input).is_none());
    }
}
