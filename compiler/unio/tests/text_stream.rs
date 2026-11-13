//! S-16, S-18: the text-stream adapter `FileReader.read_text` over real files
//! (docs/stdlib.md, docs/language-contracts.md sections 3-4). A character split between reads
//! is decoded once whatever `max_bytes` cuts it, invalid UTF-8 and a truncated end fail with
//! `IoError` of kind `InvalidData` after the text before them, and a file larger than the heap
//! limit of the policy is read in bounded memory. The decoder itself is tested by the `test`
//! blocks of `stdlib/alloc/text.unio` and `stdlib/std/files.unio`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

const MANIFEST: &str =
    "[package]\nname = \"streams\"\nversion = \"0.1.0\"\n\n[permissions]\nfiles.read = [\"$CWD/data/**\"]\n";

const READER: &str = "import std.args
import std.files

## The text of the file read with read_text in steps of max_bytes, and the number of reads.
function read_all(path as Path, max_bytes as Int) returns Text fails with IoError:
    var result = \"\"
    using reader = try files.open_reader(path):
        while true:
            match try reader.read_text(max_bytes = max_bytes):
                case some(part):
                    assert not part.is_empty()
                    assert part.byte_length() <= max_bytes + 3
                    result = result + part
                case none: break
    return result

public function main() returns Int:
    let names = args.list()
    for name in names:
        let path = Path(\"data/{name}\")
        let whole = try files.read_text(path) on IoError as e:
            print(\"{name}: not text\")
            if check_errors(path, name) != 0: return 1
            continue
        for step in range(1, whole.byte_length() + 2):
            let read = try read_all(path, step) on IoError as e:
                print(\"{name} {step}: {e.reason}\")
                return 1
            if read != whole:
                print(\"{name} {step}: different text\")
                return 1
        print(\"{name}: same text with every max_bytes\")
    return 0

## A file that is not UTF-8: every max_bytes gives the same error after the same text.
function check_errors(path as Path, name as Text) returns Int:
    var reported = \"\"
    for step in range(1, 12):
        var before = 0
        try:
            using reader = try files.open_reader(path):
                while true:
                    match try reader.read_text(max_bytes = step):
                        case some(part): before = before + part.length()
                        case none: break
            print(\"{name} {step}: no error\")
            return 1
        on IoError as e:
            assert e.kind == files.IoErrorKind.InvalidData
            let line = \"{before} characters, then {e.reason}\"
            if reported != \"\" and reported != line:
                print(\"{name} {step}: {line}, not {reported}\")
                return 1
            reported = line
    print(\"{name}: {reported}\")
    return 0
";

fn package(name: &str, program: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-streams-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("unio.toml"), MANIFEST).unwrap();
    fs::write(dir.join("main.unio"), program).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn run(dir: &Path, options: &[&str], args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .arg("run")
        .args(options)
        .arg("main.unio")
        .arg("--")
        .args(args)
        .output()
        .unwrap()
}

/// Every character of 1 to 4 bytes is split at every place by some `max_bytes`, and the text
/// read is the text of the file; invalid UTF-8 and a truncated end fail the same way whatever
/// the size of the reads.
#[test]
fn read_text_decodes_characters_split_between_reads_and_reports_invalid_utf8() {
    let dir = package("split", READER);
    fs::write(
        dir.join("data/mixed.txt"),
        "a\u{E9}b\u{20AC}c\u{1F600}d\n\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} Zo\u{EB}\n",
    )
    .unwrap();
    fs::write(dir.join("data/empty.txt"), "").unwrap();
    fs::write(dir.join("data/invalid.txt"), b"ab\xE2\x82\xAC\xC0\xAFcd").unwrap();
    fs::write(dir.join("data/truncated.txt"), b"ok \xF0\x9F\x98").unwrap();
    let output = run(
        &dir,
        &[],
        &["mixed.txt", "empty.txt", "invalid.txt", "truncated.txt"],
    );
    let out = text(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{out}{}",
        text(&output.stderr)
    );
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "mixed.txt: same text with every max_bytes",
            "empty.txt: same text with every max_bytes",
            "invalid.txt: not text",
            "invalid.txt: 3 characters, then invalid UTF-8 at byte 5",
            "truncated.txt: not text",
            "truncated.txt: 3 characters, then the file ends inside a UTF-8 character that starts at byte 3",
        ]
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A file of 24 MB, larger than the heap limit of 8 MB, is read as text in steps of 64 KiB:
/// the memory of `read_text` does not depend on the size of the file (S-16).
#[test]
fn read_text_reads_a_file_larger_than_the_heap_limit() {
    let program = "import std.files

public function main() returns Int:
    var characters = 0
    var lines = 0
    try:
        using reader = try files.open_reader(Path(\"data/large.txt\")):
            while true:
                match try reader.read_text(max_bytes = 65536):
                    case some(part):
                        characters = characters + part.length()
                        lines = lines + part.count(\"\\n\")
                    case none: break
    on files.IoError as e:
        print(\"error {e.reason}\")
        return 1
    print(\"{characters} characters, {lines} lines\")
    return 0
";
    let dir = package("large", program);
    // 10 bytes and 5 characters a line, so most reads of 64 KiB end inside a character.
    let line = "\u{E9}\u{20AC}\u{1F600}a\n";
    let count = 24 * 1024 * 1024 / line.len();
    fs::write(dir.join("data/large.txt"), line.repeat(count)).unwrap();
    let policy = dir.join("policy.toml");
    fs::write(&policy, "[limits]\nheap_bytes = 8_000_000\n").unwrap();
    let output = run(&dir, &["--policy", &policy.to_string_lossy()], &[]);
    let out = text(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{out}{}",
        text(&output.stderr)
    );
    assert_eq!(
        out.trim(),
        format!("{} characters, {count} lines", count * 5)
    );
    let _ = fs::remove_dir_all(&dir);
}
