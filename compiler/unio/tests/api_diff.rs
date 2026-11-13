//! `unio api-diff` (T-15, docs/language-contracts.md section 6): the public API of a package,
//! the comparison of two versions with the semantic-version verdict, the callers of another
//! package that a change affects, and the migrations that are unambiguous.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let root = std::env::temp_dir().join(format!("unio-api-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["old", "new", "app", "cache", "home"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("packages.toml"), "").unwrap();
        Project { root }
    }

    fn write(&self, path: &str, text: &str) {
        fs::write(self.root.join(path), text).unwrap();
    }

    fn unio(&self, dir: &str, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
        command
            .current_dir(self.root.join(dir))
            .env_remove("UNIO_POLICY")
            .env_remove("UNIO_WORKER_PROBE")
            .env("UNIO_CACHE", self.root.join("cache"))
            .env("UNIO_PACKAGES_CONFIG", self.root.join("packages.toml"))
            .env("HOME", self.root.join("home"))
            .stdin(Stdio::null())
            .args(args);
        command.output().unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn all(output: &Output) -> String {
    format!("{}{}", out(output), String::from_utf8_lossy(&output.stderr))
}

const OLD: &str = "public enum Mode:
    Loud
    Quiet

public struct Box:
    public width as Int

public function shout(text as Text, width as Int = 0) returns Text:
    return text.to_upper() + \"!\"

public function pad(text as Text, size as Int) returns Text:
    return text

public function whisper(text as Text) returns Text:
    return text.to_lower()

public function same(a as Int) returns Int:
    return a
";

const NEW: &str = "public enum Mode:
    Loud
    Quiet
    Silent

public struct Box:
    public width as Int
    public height as Int = 1

public function shout(text as Text, count as Int = 0) returns Text:
    return text.to_upper() + \"!\"

public function pad(text as Text, named size as Int) returns Text:
    return text

public function murmur(text as Text) returns Text:
    return text.to_lower()

public function same(a as Int) returns Int:
    return a + 0

public function added() returns Int:
    return 1
";

fn packages(p: &Project, new_version: &str) {
    p.write(
        "old/unio.toml",
        "[package]\nname = \"util\"\nversion = \"1.0.0\"\n",
    );
    p.write("old/strings.unio", OLD);
    p.write(
        "new/unio.toml",
        &format!(
            "[package]\nname = \"util\"\nversion = \"{new_version}\"\n\n[permissions]\nenv = [\"HOME\"]\n"
        ),
    );
    p.write("new/strings.unio", NEW);
}

#[test]
fn the_api_of_a_package_lists_its_public_declarations() {
    let p = Project::new("show");
    packages(&p, "2.0.0");
    let output = p.unio(".", &["api-diff", "old"]);
    assert!(output.status.success(), "{}", all(&output));
    let text = out(&output);
    assert!(text.contains("util 1.0.0 (unio-api-v1)"), "{text}");
    assert!(
        text.contains("function strings.shout(text as Text, width as Int = 0) returns Text"),
        "{text}"
    );
    assert!(text.contains("enum strings.Mode(Loud, Quiet)"), "{text}");
    let json = p.unio(".", &["api-diff", "--json", "new"]);
    let json = out(&json);
    assert!(json.contains("\"permissions\":[\"env = HOME\"]"), "{json}");
    assert!(json.contains("\"named\":true"), "{json}");
    assert!(json.contains("\"hash\":\"unio-hash-v1:sha384:"), "{json}");
}

#[test]
fn incompatible_changes_need_a_major_version_and_name_their_migrations() {
    let p = Project::new("diff");
    packages(&p, "1.1.0");
    p.write(
        "app/unio.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    );
    p.write(
        "app/main.unio",
        "import deps.util.strings\n\npublic function main():\n    print(strings.shout(\"a\", width = 2))\n    print(strings.whisper(\"B\"))\n    print(strings.pad(\"c\", 3))\n",
    );
    let added = p.unio("app", &["add", "util", "--path", "../old"]);
    assert!(added.status.success(), "{}", all(&added));
    let output = p.unio(".", &["api-diff", "old", "new", "--callers", "app"]);
    let text = out(&output);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    // An added variant breaks exhaustive matches; a renamed label and a wider permission too.
    assert!(
        text.contains("major strings.Mode: variant 'Silent' was added"),
        "{text}"
    );
    assert!(
        text.contains("major strings.shout: parameter 'width' is named 'count' now"),
        "{text}"
    );
    assert!(text.contains("main.unio:4:30: width -> count"), "{text}");
    assert!(
        text.contains("the function was renamed to 'strings.murmur'"),
        "{text}"
    );
    assert!(text.contains("main.unio:5:19: whisper -> murmur"), "{text}");
    assert!(
        text.contains("parameter 'size' is given only by name now"),
        "{text}"
    );
    assert!(text.contains("main.unio:6:28: insert 'size = '"), "{text}");
    assert!(
        text.contains("requires the permission env = HOME now"),
        "{text}"
    );
    // A field with a default and a new function are additions; a changed body is a patch.
    assert!(
        text.contains("minor strings.Box: public field 'height' was added"),
        "{text}"
    );
    assert!(
        text.contains("minor strings.added: the public function was added"),
        "{text}"
    );
    assert!(
        text.contains("patch strings.same: the implementation changed"),
        "{text}"
    );
    assert!(text.contains("they need at least 2.0.0"), "{text}");
    let json = out(&p.unio(".", &["api-diff", "--json", "old", "new"]));
    assert!(json.contains("\"level\":\"major\""), "{json}");
    assert!(json.contains("\"version_ok\":false"), "{json}");
    assert!(json.contains("\"required_version\":\"2.0.0\""), "{json}");
}

#[test]
fn a_major_version_is_enough_and_identical_apis_have_no_changes() {
    let p = Project::new("ok");
    packages(&p, "2.0.0");
    let output = p.unio(".", &["api-diff", "old", "new"]);
    assert!(output.status.success(), "{}", all(&output));
    assert!(out(&output).contains("version 2.0.0 is enough"));
    let same = p.unio(".", &["api-diff", "old", "old"]);
    assert!(same.status.success(), "{}", all(&same));
    assert!(out(&same).contains("no changes to the public API"));
    let wrong = p.unio(".", &["api-diff"]);
    assert_eq!(wrong.status.code(), Some(2));
    p.write(
        "old/bad.unio",
        "public function f() returns Int:\n    return missing\n",
    );
    let bad = p.unio(".", &["api-diff", "old", "new"]);
    assert_eq!(bad.status.code(), Some(2), "{}", all(&bad));
    assert!(all(&bad).contains("E0056"), "{}", all(&bad));
}
