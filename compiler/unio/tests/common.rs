//! Helpers shared by the integration tests.

use std::fs;
use std::path::{Path, PathBuf};

/// The root of the repository: compiler/unio/../..
#[allow(dead_code)]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Every `.unio` file below the directory, sorted.
#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The variable that makes a missing prerequisite of a test a failure: CI sets it to `1` on
/// every job (`.github/workflows/ci.yml`), so that a test whose body did not run cannot count
/// as release evidence (R-7a, SB-8, SB-15).
#[allow(dead_code)]
pub const REQUIRE_PREREQUISITES: &str = "UNIO_REQUIRE_PREREQUISITES";

/// A prerequisite of a test is missing here: a program such as python3 or a C compiler, or a
/// machine for which no native executables are made. With UNIO_REQUIRE_PREREQUISITES=1 the
/// test fails with `what`; otherwise, on a developer's machine, it notes the skip and the
/// caller returns. `cargo test` hides that note, so only the variable makes a skip visible.
#[allow(dead_code)]
#[track_caller]
pub fn missing_prerequisite(what: &str) {
    if std::env::var_os(REQUIRE_PREREQUISITES).is_some_and(|v| v == "1") {
        panic!(
            "a prerequisite of this test is missing: {what}; with \
             {REQUIRE_PREREQUISITES}=1 a skipped test is a failure"
        );
    }
    eprintln!("skipped: {what} (set {REQUIRE_PREREQUISITES}=1 to make this a failure)");
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

/// The room the test hook leaves on a "full" heap for a program that must reach its fault: the
/// heap counts every block (RFC 0010), so the calls, the open file, and the first resource
/// before the fault need some room, while the releases after it need far more (2,000 texts,
/// about 230 KB), which only the reserve can serve.
#[allow(dead_code)]
pub const NEARLY_FULL: usize = 64 * 1024;

/// A fresh package for a program of the reserve: `RESERVE_PRELUDE` and `body` in `main.unio`,
/// a manifest that lets it write `open/`, `open/target.txt` holding "old content", and
/// `policy.toml` with a heap limit of 8 MiB.
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
    fs::write(
        dir.join("policy.toml"),
        "[limits]\nheap_bytes = 8_388_608\n",
    )
    .unwrap();
    fs::canonicalize(dir).unwrap()
}

/// Writes the policy of a reserve package with a heap limit of `heap` bytes, or without a heap
/// limit for `None`.
#[allow(dead_code)]
pub fn set_heap_limit(dir: &Path, heap: Option<usize>) {
    let limits = match heap {
        Some(bytes) => format!("[limits]\nheap_bytes = {bytes}\n"),
        None => String::new(),
    };
    fs::write(dir.join("policy.toml"), limits).unwrap();
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

/// A program that makes 20,000 small structs, enums, and closures (RFC 0010). Its lists hold
/// 32-byte values, about 2 MiB in all; the wrappers of the values (the reference-counted blocks
/// of each struct, enum and closure, their fields and captures) take several times that, and
/// the estimated count of before RFC 0010 did not see them.
#[allow(dead_code)]
pub const SMALL_VALUES: &str = "struct Point:
    x as Int
    y as Int
    z as Int

enum Shape:
    Circle(radius as Int)
    Square(side as Int, depth as Int)

public function main():
    var points as List[Point] = []
    var shapes as List[Shape] = []
    var adders as List[(Int) -> Int] = []
    for i in range(0, 20000):
        points.append(Point(x = i, y = i, z = i))
        shapes.append(Square(side = i, depth = i))
        let k = i
        adders.append(function(n as Int) returns Int: n + k)
    print(\"{points.length()} {shapes.length()} {adders.length()}\")
";
