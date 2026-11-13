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

/// The depth of [`deep_closures`] for the tests: 10 million in a release build, deeper than the
/// 1 GiB stack of `unio run` and of an executable allowed while a closure freed the closure it
/// captured by recursion (about 128 bytes of the native stack a level): a release build
/// overflowed between 8 and 10 million levels, with `fatal runtime error: stack overflow` and
/// code 134 in `unio run` and SIGSEGV in an executable. A debug build is several times slower
/// and its frames are larger, so it builds 1 million.
#[allow(dead_code)]
pub const DEEP_CLOSURES: usize = if cfg!(debug_assertions) {
    1_000_000
} else {
    10_000_000
};

/// A program that builds a chain of `depth` closures, each capturing the one before, and a
/// mixed chain of `depth / 4` levels, in which a closure captures a list, a map, a struct, or
/// an Option that holds the closure before, calls each once, and drops it (RFC 0007; C-13).
/// It prints `closures 3`, `mixed 3`, and `freed`.
#[allow(dead_code)]
pub fn deep_closures(depth: usize) -> String {
    let mixed = depth / 4;
    format!(
        "struct Holder:
    held as Function[(Int) -> Int]

function closures(depth as Int) returns Function[(Int) -> Int]:
    var link = function(n as Int) returns Int: n
    for i in range(0, depth):
        let inner = link
        link = function(n as Int) returns Int: n if n < 1 else inner(n - 1) + 1
    return link

function mixed(depth as Int) returns Function[(Int) -> Int]:
    var link = function(n as Int) returns Int: n
    for i in range(0, depth):
        if i % 4 == 0:
            let items = [link]
            link = function(n as Int) returns Int: n if n < 1 else items[0](n - 1) + 1
        else if i % 4 == 1:
            let table = {{i: link}}
            link = function(n as Int) returns Int: n if n < 1 else (table.get(i) or link)(n - 1) + 1
        else if i % 4 == 2:
            let holder = Holder(held = link)
            link = function(n as Int) returns Int: n if n < 1 else holder.held(n - 1) + 1
        else:
            let maybe = some(link)
            link = function(n as Int) returns Int: n if n < 1 else (maybe or link)(n - 1) + 1
    return link

function call_once(chain as Function[(Int) -> Int]) returns Int:
    return chain(3)

public function main():
    print(\"closures {{call_once(closures({depth}))}}\")
    print(\"mixed {{call_once(mixed({mixed}))}}\")
    print(\"freed\")
"
    )
}

/// The depth of [`deep_values`] for the tests: 10 million in a release build, where `==` of two
/// chains of that depth overflowed the 1 GiB stack of `unio run` while it compared by recursion
/// (code 134); 1 million in a debug build, which is several times slower.
#[allow(dead_code)]
pub const DEEP_VALUES: usize = if cfg!(debug_assertions) {
    1_000_000
} else {
    10_000_000
};

/// A program that walks chains of `depth` enum values and a tree of lists `depth / 10` deep with
/// every operation that walks a value (L-17a, C-18): `==`, a key of a set and of a map (hash and
/// equality). It prints [`DEEP_VALUES_OUTPUT`]. Interpolation of such values calls their
/// `to_text` (E0067), so the text of a value by its structure is tested in `value.rs`.
#[allow(dead_code)]
pub fn deep_values(depth: usize) -> String {
    let trees = depth / 10;
    format!(
        "enum Chain:
    End
    Link(next as Chain)

enum Tree:
    Leaf
    Node(children as List[Tree])

function chain(depth as Int, last as Chain) returns Chain:
    var link = last
    for i in range(0, depth):
        link = Link(next = link)
    return link

function tree(depth as Int) returns Tree:
    var node = Leaf
    for i in range(0, depth):
        node = Node(children = [Leaf, node])
    return node

public function main():
    let a = chain({depth}, End)
    let b = chain({depth}, End)
    let c = Link(next = a)
    print(\"equal {{a == b}} {{a == c}} {{a != c}}\")
    var seen = Set[Chain]()
    seen.insert(a)
    print(\"set {{seen.contains(b)}} {{seen.contains(c)}}\")
    let table = {{a: 1}}
    print(\"map {{table.get(b) or 0}} {{table.get(c) or 0}}\")
    let t = tree({trees})
    var forest = Set[Tree]()
    forest.insert(t)
    print(\"tree {{t == tree({trees})}} {{t == tree({trees} - 1)}} {{forest.contains(tree({trees}))}}\")
"
    )
}

/// What [`deep_values`] prints.
#[allow(dead_code)]
pub const DEEP_VALUES_OUTPUT: &str =
    "equal true false true\nset true false\nmap 1 0\ntree true false true\n";

/// A program that converts a value 5,000 levels deep from JSON and one 20,000 levels deep
/// (S-5, L-17a, C-18): each level of `std.json.from_value` and `std.json.to_value` counts as a
/// call, so the second ends with exhaustion of the stack, code 102, at the stack limit of 10,000
/// calls. Before, the conversions recursed on the native stack without a bound and
/// overflowed it on a value deep enough. It prints [`DEEP_JSON_OUTPUT`] before it ends.
#[allow(dead_code)]
pub const DEEP_JSON: &str = "import std.json as json
from std.json import Value

enum Chain:
    End
    Link(next as Chain)

function chain(depth as Int) returns Chain:
    var link = End
    for i in range(0, depth):
        link = Link(next = link)
    return link

function document(depth as Int) returns Value:
    var link = Value.Text(\"End\")
    for i in range(0, depth):
        link = Value.Object({\"Link\": Value.Object({\"next\": link})})
    return link

function decoded(depth as Int) returns Bool:
    let back = try json.decode[Chain](document(depth)) on DecodeError as e:
        assert_fail(e.message)
    return back == chain(depth)

public function main():
    print(\"decoded {decoded(5000)}\")
    print(\"encoded {json.encode(chain(100)).length()}\")
    print(\"decoded {decoded(20000)}\")
";

/// What [`DEEP_JSON`] prints before the conversion of 20,000 levels ends it with code 102.
#[allow(dead_code)]
pub const DEEP_JSON_OUTPUT: &str = "decoded true\nencoded 1805\n";
