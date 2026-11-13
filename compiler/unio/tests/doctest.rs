//! Documentation tests (T-13): the examples of documentation comments run under `unio test`
//! as tests of their module, with the lines of the comment in their messages.

use std::fs;
use std::process::Command;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

#[test]
fn examples_of_documentation_comments_run_as_tests() {
    let dir = std::env::temp_dir().join(format!("unio-doctest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let source = "## Adds two numbers.
##
## Example:
##     assert add(2, 3) == 5
##
##     assert add(1, 1) == 3
public function add(a as Int, b as Int) returns Int:
    return a + b

## Example:
##     assert missing(1) == 1
public function other() returns Int:
    return 1

## Usage text is not an example:
##     add A B
public function third() returns Int:
    return 3

test \"plain\":
    assert add(1, 2) == 3
";
    fs::write(dir.join("lib.unio"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(&dir)
        .args(["test", "lib.unio"])
        .output()
        .unwrap();
    let out = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{out}");
    assert!(out.contains("ok   lib.unio: doc: add (line 4)"), "{out}");
    assert!(out.contains("FAIL lib.unio: doc: add (line 6)"), "{out}");
    // The trace and the diagnostic point at the line of the comment.
    assert!(out.contains("(lib.unio:6)"), "{out}");
    assert!(out.contains("11:E0056"), "{out}");
    assert!(out.contains("2 passed, 2 failed"), "{out}");
    // The file itself is unchanged and checks without its examples.
    assert_eq!(fs::read_to_string(dir.join("lib.unio")).unwrap(), source);
    let json = Command::new(env!("CARGO_BIN_EXE_unio"))
        .current_dir(&dir)
        .args(["test", "--json", "doc: add", "lib.unio"])
        .output()
        .unwrap();
    let json = text(&json.stdout);
    assert!(json.contains("\"name\": \"doc: add (line 4)\""), "{json}");
    assert!(!json.contains("plain"), "{json}");
    let _ = fs::remove_dir_all(&dir);
}
