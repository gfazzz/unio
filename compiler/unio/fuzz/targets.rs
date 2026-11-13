//! The fuzz targets of R-1: each feeds one input to one part of the compiler, and the part must
//! end with a result or diagnostics, never with a panic, an abort, or an overflow of the stack.
//!
//! Shared by the coverage-guided fuzzer (`fuzz/main.rs`, the example `fuzz`) and the regression
//! tests of `tests/fuzz.rs`, which run every reproducer the fuzzer found
//! (`tests/data/fuzz/<target>/`) through the target that found it.

use std::path::Path;

/// The targets, in the order the fuzzer gives them to its processes.
pub const TARGETS: &[&str] = &["front", "check", "format", "build", "manifest", "archive"];

/// What the seeds of a target are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seeds {
    /// `.unio` sources.
    Sources,
    /// Manifests, locks, policies, and the other text files of TOML, the index, and releases.
    Manifests,
    /// Package archives (tar).
    Archives,
}

pub fn seeds(target: &str) -> Seeds {
    match target {
        "manifest" => Seeds::Manifests,
        "archive" => Seeds::Archives,
        _ => Seeds::Sources,
    }
}

/// The stack of the thread that runs a target: the front-end stack of the tool
/// (`FRONT_END_STACK` of `src/main.rs`), so that a nesting the tool survives is survived here.
pub const STACK: usize = 64 * 1024 * 1024;

/// Runs one input through the target; `scratch` is an empty directory of this process for the
/// targets that read a file. Unknown targets panic: they are mistakes of the caller.
pub fn run(target: &str, data: &[u8], scratch: &Path) {
    match target {
        "front" => front(&String::from_utf8_lossy(data)),
        "format" => format(&String::from_utf8_lossy(data)),
        "check" => check(data, scratch),
        "build" => build(data, scratch),
        "manifest" => manifest(data),
        "archive" => archive(data),
        other => panic!("no fuzz target {other}"),
    }
}

/// The lexer, the parser, the diagnostics in both languages and both formats, the outline, and
/// the S-expression of `unio parse --ast`.
fn front(text: &str) {
    let analysis = unio::check_source("fuzz.unio", text);
    let _ = analysis.render_text(unio::Lang::Ru);
    let _ = analysis.render_json(unio::Lang::En);
    let _ = analysis.ast_sexpr();
    let _ = unio::outline::outline_text(&analysis.file);
    let _ = unio::outline::outline_json(&analysis.file);
}

/// `unio fmt`: the formatted text is formatted again.
fn format(text: &str) {
    if let Ok(formatted) = unio::format::format_source(text) {
        let _ = unio::format::format_source(&formatted);
    }
}

fn source_file(data: &[u8], scratch: &Path) -> std::path::PathBuf {
    let path = scratch.join("main.unio");
    std::fs::write(&path, data).expect("the scratch directory is writable");
    path
}

/// `unio check`: names, types, effects, and the `core` profile, for every fourth input.
fn check(data: &[u8], scratch: &Path) {
    let path = source_file(data, scratch);
    let profile = if data.len() % 4 == 3 {
        unio::effects::Profile::Core
    } else {
        unio::effects::Profile::Application
    };
    for analysis in unio::check_files_with(&[path], profile) {
        let _ = analysis.render_text(unio::Lang::En);
        let _ = analysis.render_json(unio::Lang::Ru);
    }
}

/// `unio build` up to the object file: checking, lowering every body, the program image, and the
/// code of Cranelift. A program with errors stops at the check.
fn build(data: &[u8], scratch: &Path) {
    let path = source_file(data, scratch);
    let target = unio::native::Target::host().unwrap_or(unio::native::Target::LinuxX86_64);
    let _ = unio::native::compile_object_for(&path, target, false, "fuzz");
}

/// The readers of the text files of a package and of the tool: the manifest and its E0088
/// check, the lock, the configuration, the index of a registry, keys and releases, the TOML of
/// the sandbox with its permissions and policies, and, line by line, versions, requirements,
/// URLs, and permission entries.
fn manifest(data: &[u8]) {
    use unio::package::{config, index, lock, manifest, semver, trust};
    use unio_runtime::sandbox;
    let text = String::from_utf8_lossy(data);
    let _ = unio::program::manifest_diagnostics(&text);
    let _ = manifest::Manifest::parse(&text);
    let _ = manifest::check(&text);
    let _ = lock::Lock::parse(&text);
    let _ = config::Config::parse(&text);
    let _ = index::Index::parse(&text, "owner/name");
    let _ = trust::parse_keys(data);
    let _ = unio::release::parse(data);
    let _ = sandbox::toml::parse(&text);
    let _ = sandbox::declared_in_manifest(&text);
    let places = sandbox::Places {
        home: Some("/home/fuzz".into()),
        cwd: "/work".into(),
        temp: "/tmp".into(),
        base: "/work/package".into(),
    };
    let _ = sandbox::parse_manifest(&text, &places);
    let _ = sandbox::parse_policy(&text, &places);
    for line in text.lines() {
        let line = line.trim();
        let _ = semver::Version::parse(line);
        let _ = semver::Req::parse(line);
        let _ = unio::package::http::Url::parse(line);
        let _ = manifest::scope_problem(line);
        let _ = manifest::valid_alias(line);
        let _ = manifest::valid_name(line);
        let _ = manifest::valid_registry_package(line);
        let _ = trust::valid_date(line);
        let _ = lock::digest_hex(line);
    }
}

/// The reader of package archives and the content hash of what it read.
fn archive(data: &[u8]) {
    use unio::package::{archive, tree};
    if let Ok(entries) = archive::read(data, &archive::Limits::default()) {
        let _ = tree::content_hash(&entries);
    }
}
