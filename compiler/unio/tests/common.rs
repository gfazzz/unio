//! Helpers shared by the integration tests.

use std::fs;
use std::path::{Path, PathBuf};

/// The root of the repository: compiler/unio/../..
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Every `.unio` file below the directory, sorted.
pub fn unio_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "unio") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Windows has no sandbox of the operating system for programs or for the workers of the tools
/// yet, so `unio` refuses there with code 103 unless the operator allows a weaker sandbox
/// (SB-5, SB-10). The functional tests act as that operator through the external policy; the
/// refusal itself is tested separately without it.
#[allow(dead_code)]
pub fn allow_weaker_on_windows(command: &mut std::process::Command) {
    if cfg!(windows) {
        command.env("UNIO_POLICY", weaker_policy());
    }
}

/// A policy file that only allows the weaker sandbox.
#[allow(dead_code)]
pub fn weaker_policy() -> PathBuf {
    // Written once per test process and put in place by a rename: tests run in parallel, and
    // rewriting the file for each of them let a `unio` started by another test read it while
    // it was empty, a policy without `allow_weaker` (code 103 on Windows).
    static POLICY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    POLICY
        .get_or_init(|| {
            let dir = std::env::temp_dir();
            let policy = dir.join(format!("unio-weaker-{}.toml", std::process::id()));
            let partial = dir.join(format!("unio-weaker-{}.toml.partial", std::process::id()));
            fs::write(&partial, "allow_weaker = true\n").unwrap();
            fs::rename(&partial, &policy).unwrap();
            policy
        })
        .clone()
}

/// The resources of the programs of the reserve for `release` (C-17): `Log` builds 2,000 texts
/// in its `release`, more than a full heap allows, so that it can only run from the reserve;
/// `Faulty` faults in its `release` after doing the same; `Hungry` needs more than the reserve.
#[allow(dead_code)]
pub const RESERVE_PRELUDE: &str = "import std.files

struct Log:
    name as Text

extend Log with Resource:
    function release(var self):
        let words = [\"{self.name} {i}\" for i in range(0, 2000)]
        print(\"released {self.name}: {words.length()} words, last {words[1999]}\")

struct Faulty:
    name as Text

extend Faulty with Resource:
    function release(var self):
        let words = [\"{self.name} {i}\" for i in range(0, 2000)]
        print(\"releasing {self.name}: {words.length()} words\")
        print(words[words.length() + 5])

struct Hungry:
    name as Text

extend Hungry with Resource:
    function release(var self):
        print(\"releasing {self.name}\")
        let numbers = [i for i in range(0, 300000)]
        print(\"released {self.name}: {numbers.length()}\")

function divide(a as Int, b as Int) returns Int:
    return a / b

";

/// Releases that exhaust the reserve with memory they free again before any check of the
/// heap: one large block (a text of 2.4 MB built by concatenation, which no check of the heap
/// follows) and many small ones (40,000 texts in a list that is dropped at once). Both are
/// exhaustion whatever the heap, with a full heap, with room on it, and without a heap limit:
/// code 102, the preallocated diagnostic, the release of `first` skipped, the temporary file
/// kept and the target unchanged (C-17, C-18, plan section 12 condition 1). `KIND` may also be
/// `Hungry` of [`RESERVE_PRELUDE`], which keeps its list until the check.
#[allow(dead_code)]
pub const SPIKES: &str = "struct Spike:
    name as Text

extend Spike with Resource:
    function release(var self):
        print(\"releasing {self.name}\")
        let part = \"s\".repeat(400000)
        let n = (part + part + part + part + part + part).length()
        print(\"released {self.name}: {n}\")

struct Crowd:
    name as Text

extend Crowd with Resource:
    function release(var self):
        print(\"releasing {self.name}\")
        let n = [\"{self.name} {i}\" for i in range(0, 40000)].length()
        print(\"released {self.name}: {n}\")

public function main() returns Int:
    try:
        using first = Log(name = \"first\"):
            using file = try files.open_atomic(Path(\"open/target.txt\")):
                try file.write(\"new content\")
                using spike = KIND(name = \"spike\"):
                    return divide(1, 0)
    on IoError as e:
        print(\"error {e.reason}\")
    return 0
";

/// A fresh package for a program of the reserve: `RESERVE_PRELUDE` and `body` in `main.unio`,
/// a manifest that lets it write `open/`, `open/target.txt` holding "old content", and
/// `policy.toml` with a heap limit of 8 MiB (on Windows also allowing the weaker sandbox).
#[allow(dead_code)]
pub fn reserve_package(prefix: &str, name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-reserve-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("open")).unwrap();
    fs::write(dir.join("open/target.txt"), "old content").unwrap();
    fs::write(
        dir.join("unio.toml"),
        "[package]\nname = \"reserve\"\n\n[permissions]\nfiles.write = [\"$CWD/open/**\"]\n",
    )
    .unwrap();
    fs::write(dir.join("main.unio"), format!("{RESERVE_PRELUDE}{body}")).unwrap();
    let weaker = if cfg!(windows) {
        "allow_weaker = true\n"
    } else {
        ""
    };
    fs::write(
        dir.join("policy.toml"),
        format!("{weaker}[limits]\nheap_bytes = 8_388_608\n"),
    )
    .unwrap();
    fs::canonicalize(dir).unwrap()
}

/// Writes the policy of a reserve package with a heap limit of `heap` bytes, or without a heap
/// limit for `None` (on Windows also allowing the weaker sandbox).
#[allow(dead_code)]
pub fn set_heap_limit(dir: &Path, heap: Option<usize>) {
    let weaker = if cfg!(windows) {
        "allow_weaker = true\n"
    } else {
        ""
    };
    let limits = match heap {
        Some(bytes) => format!("[limits]\nheap_bytes = {bytes}\n"),
        None => String::new(),
    };
    fs::write(dir.join("policy.toml"), format!("{weaker}{limits}")).unwrap();
}

/// Puts a reserve package back as it was made: no temporary file, the old target.
#[allow(dead_code)]
pub fn reset_target(dir: &Path) {
    for name in temporaries(dir) {
        fs::remove_file(dir.join("open").join(name)).unwrap();
    }
    fs::write(dir.join("open/target.txt"), "old content").unwrap();
}

/// The resources of [`SPIKES`] without its `main`, for programs of their own.
#[allow(dead_code)]
pub fn spike_resources() -> &'static str {
    SPIKES
        .split("public function main")
        .next()
        .expect("the resources of SPIKES")
}

/// The names in `open/` of a reserve package other than the target: a temporary file of
/// `open_atomic` left behind.
#[allow(dead_code)]
pub fn temporaries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join("open"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "target.txt")
        .collect();
    names.sort();
    names
}

#[allow(dead_code)]
#[track_caller]
pub fn assert_target_unchanged(dir: &Path) {
    assert_eq!(read(&dir.join("open/target.txt")), "old content");
}
