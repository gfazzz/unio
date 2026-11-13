//! `unio doc` (table 6.1, L-3, L-9b, SB-10): the documentation of a package as HTML pages, with
//! the documentation comments, the signatures, the errors, and the derived effects of its public
//! declarations, made in a worker and written by the tool; `--open` and its command.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MANIFEST: &str = "[package]\nname = \"shapes\"\nversion = \"1.2.0\"\n\n[permissions]\nfiles.read = [\"$CWD/**\"]\n";

const GEOMETRY: &str = "\
## Shapes and their areas.
##
## A second paragraph with `code` & <markup>.

import std.files

## A point on the plane.
public struct Point:
    public x as Int
    public y as Int
    secret as Int = 0

## How a shape failed to load.
public error LoadError:
    message as Text

## The area of a square with the side.
##
## Example:
##     assert area(3) == 9
public function area(side as Int) returns Int:
    return side * side

## Reads the side of a square from a file.
public function load(path as Text) returns Int fails with LoadError:
    let text = try files.read_text(Path(path)) on IoError as e:
        fail LoadError(message = e.to_text())
    return text.trim().to_int() or 0

## Applies the function to the side.
public function apply(side as Int, f as Function[(Int) -> Int]) returns Int:
    return f(side)

function helper() returns Int:
    return 1

## The origin.
public let ORIGIN as Point = Point(x = 0, y = 0)

extend Point with Display:
    ## The point as `(x, y)`.
    function to_text(self) returns Text:
        return \"({self.x}, {self.y})\"

extend Point:
    ## The point moved by the offsets.
    public function moved(self, dx as Int, dy as Int) returns Point:
        return Point(x = self.x + dx, y = self.y + dy)

    function hidden(self) returns Int:
        return self.secret

test \"the area of a square\":
    assert area(3) == 9
";

const UNITS: &str = "\
## Units of length.

## A length in millimetres.
public function millimetres(metres as Int) returns Int:
    return metres * 1000
";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn all(output: &Output) -> String {
    text(&output.stdout) + &text(&output.stderr)
}

/// A fresh directory with the package `shapes`: `geometry.unio` and `util/units.unio`.
fn package(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("unio-doc-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("pkg/util")).unwrap();
    fs::write(dir.join("pkg/unio.toml"), MANIFEST).unwrap();
    fs::write(dir.join("pkg/geometry.unio"), GEOMETRY).unwrap();
    fs::write(dir.join("pkg/util/units.unio"), UNITS).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn unio(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command
        .current_dir(dir)
        .env_remove("UNIO_POLICY")
        .env_remove("UNIO_WORKER_PROBE")
        .env_remove("BROWSER")
        .env_remove("UNIO_TEST_PRINT_OPEN")
        .args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().unwrap()
}

/// Every file below the directory with its bytes, by relative path.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path.strip_prefix(dir).unwrap().to_string_lossy();
                out.push((relative.replace('\\', "/"), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// A path as the tool prints it: without the prefix `\\?\` of canonical paths on Windows.
fn plain(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\")
        .map_or(text.clone(), str::to_string)
}

fn page(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn doc_writes_the_pages_of_a_package_into_target_doc() {
    let root = package("default");
    let output = unio(&root.join("pkg"), &["doc"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    let out = root.join("pkg/target/doc");
    let files: Vec<String> = tree(&out).into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        files,
        ["geometry.html", "index.html", "util.units.html"],
        "{}",
        all(&output)
    );
    assert!(
        text(&output.stdout).contains(&plain(&out.join("index.html"))),
        "{}",
        all(&output)
    );
    let index = page(&out, "index.html");
    assert!(index.starts_with("<!DOCTYPE html>"), "{index}");
    assert!(index.contains("shapes 1.2.0"), "{index}");
    assert!(index.contains("href=\"geometry.html\""), "{index}");
    assert!(index.contains("href=\"util.units.html\""), "{index}");
    // The first sentence of each module's documentation stands in the list.
    assert!(index.contains("Shapes and their areas."), "{index}");
    assert!(index.contains("Units of length."), "{index}");
    let _ = fs::remove_dir_all(&root);
}

/// L-3: the documentation comments of the module and of each public declaration; L-9b: the
/// effects derived from the call graph, with the errors and the effects through arguments.
#[test]
fn doc_shows_documentation_signatures_errors_and_effects() {
    let root = package("content");
    let out = root.join("out");
    let output = unio(
        &root.join("pkg"),
        &["doc", "-o", &out.to_string_lossy()],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    let geometry = page(&out, "geometry.html");
    for expected in [
        // The documentation of the module, escaped, with inline code.
        "<p>Shapes and their areas.</p>",
        "<p>A second paragraph with <code>code</code> &amp; &lt;markup&gt;.</p>",
        // Each public declaration with its signature and documentation comment.
        "id=\"Point\"",
        "public struct Point:\n    public x as Int\n    public y as Int",
        "Private fields: the type is built by the functions of its module.",
        "id=\"LoadError\"",
        "<p>How a shape failed to load.</p>",
        "id=\"area\"",
        "function area(side as Int) returns Int",
        "<p>The area of a square with the side.</p>",
        "<pre><code>assert area(3) == 9</code></pre>",
        "id=\"load\"",
        "function load(path as Text) returns Int fails with LoadError",
        "<p>Reads the side of a square from a file.</p>",
        "id=\"apply\"",
        "id=\"ORIGIN\"",
        "<p>The origin.</p>",
        // The methods of the extend blocks: the implementation of an interface and the public
        // methods.
        "extend Point with Display",
        "id=\"Point.to_text\"",
        "<p>The point as <code>(x, y)</code>.</p>",
        "id=\"Point.moved\"",
        "function moved(self, dx as Int, dy as Int) returns Point",
    ] {
        assert!(
            geometry.contains(expected),
            "missing {expected:?} in:\n{geometry}"
        );
    }
    // The facts of each function: errors, effects, effects through arguments, layer.
    let load = section(&geometry, "load");
    assert!(
        load.contains("<dt>Errors</dt><dd><code>LoadError</code></dd>"),
        "{load}"
    );
    assert!(
        load.contains("<dt>Effects</dt><dd><code>files.read</code>"),
        "{load}"
    );
    let area = section(&geometry, "area");
    assert!(area.contains("<dt>Errors</dt><dd>none</dd>"), "{area}");
    assert!(area.contains("<dt>Effects</dt><dd>none</dd>"), "{area}");
    let apply = section(&geometry, "apply");
    assert!(
        apply.contains("<dt>Effects through arguments</dt><dd><code>f</code>"),
        "{apply}"
    );
    // Private declarations, private fields and members, and tests are left out.
    for absent in [
        "helper",
        "hidden",
        "secret",
        "the area of a square",
        "import std.files",
    ] {
        assert!(!geometry.contains(absent), "{absent:?} in:\n{geometry}");
    }
    let units = page(&out, "util.units.html");
    assert!(units.contains("<p>Units of length.</p>"), "{units}");
    assert!(
        units.contains("function millimetres(metres as Int) returns Int"),
        "{units}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// The part of a page from the declaration with the id to the next one.
fn section<'a>(page: &'a str, id: &str) -> &'a str {
    let marker = format!("id=\"{id}\"");
    let start = page.find(&marker).unwrap_or_else(|| panic!("no {marker}"));
    let rest = &page[start..];
    let end = rest[marker.len()..]
        .find("<section")
        .map_or(rest.len(), |i| i + marker.len());
    &rest[..end]
}

/// The pages are the same bytes in two runs and for the package copied to another directory:
/// they hold no time and no path of the machine.
#[test]
fn doc_is_deterministic_and_holds_no_path_of_the_machine() {
    let first = package("same-1");
    let second = package("same-2-in-a-longer-directory");
    let a = first.join("out");
    let b = first.join("out-again");
    let c = second.join("out");
    for (dir, out) in [(&first, &a), (&first, &b), (&second, &c)] {
        let output = unio(
            &dir.join("pkg"),
            &["doc", "-o", &out.to_string_lossy()],
            &[],
        );
        assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    }
    assert_eq!(tree(&a), tree(&b));
    assert_eq!(tree(&a), tree(&c));
    for (name, bytes) in tree(&a) {
        let page = String::from_utf8(bytes).unwrap();
        assert!(!page.contains(&*first.to_string_lossy()), "{name}");
        assert!(!page.contains("unio-doc-"), "{name}");
    }
    let _ = fs::remove_dir_all(&first);
    let _ = fs::remove_dir_all(&second);
}

/// A package with errors gets no documentation: the diagnostics, exit code 1, no pages.
#[test]
fn doc_of_a_package_with_errors_reports_them_and_writes_nothing() {
    let root = package("errors");
    fs::write(
        root.join("pkg/broken.unio"),
        "public function f() returns Int:\n    return \"text\"\n",
    )
    .unwrap();
    let output = unio(&root.join("pkg"), &["doc"], &[]);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("broken.unio:2"),
        "{}",
        all(&output)
    );
    assert!(!root.join("pkg/target").exists());
    let _ = fs::remove_dir_all(&root);
}

/// The package is named by a directory or a file in it; a path with no package is an error.
#[test]
fn doc_takes_the_package_of_a_directory_or_a_file() {
    let root = package("paths");
    for (i, target) in ["pkg", "pkg/util/units.unio"].into_iter().enumerate() {
        let out = root.join(format!("out-{i}"));
        let output = unio(&root, &["doc", target, "-o", &out.to_string_lossy()], &[]);
        assert_eq!(output.status.code(), Some(0), "{target}: {}", all(&output));
        assert!(out.join("geometry.html").is_file(), "{target}");
    }
    let output = unio(&root, &["doc", "missing"], &[]);
    assert_eq!(output.status.code(), Some(2), "{}", all(&output));
    let output = unio(&root, &["doc", "--unknown"], &[]);
    assert_eq!(output.status.code(), Some(2), "{}", all(&output));
    let _ = fs::remove_dir_all(&root);
}

/// The pages are made in a worker (SB-10): past the compiler, it cannot read outside the
/// package; the tool writes the pages.
#[test]
fn doc_reads_the_package_in_a_worker() {
    let root = package("worker");
    fs::write(root.join("secret.txt"), "SECRET-OF-THE-OPERATOR").unwrap();
    let probe = format!("read={}", root.join("secret.txt").display());
    let output = unio(
        &root.join("pkg"),
        &["doc"],
        &[("UNIO_WORKER_PROBE", &probe)],
    );
    let err = text(&output.stderr);
    let result = err
        .lines()
        .find_map(|l| l.strip_prefix(&format!("unio worker probe: {probe}: ")))
        .unwrap_or_else(|| panic!("no probe result:\n{}", all(&output)));
    assert!(result.starts_with("denied"), "{result}");
    assert!(!all(&output).contains("SECRET-OF-THE-OPERATOR"));
    let _ = fs::remove_dir_all(&root);
}

/// A `target` that is a link is not followed: the tool writes only a real directory of the
/// package or the directory the operator names.
#[cfg(unix)]
#[test]
fn doc_does_not_write_through_a_linked_target() {
    let root = package("linked-target");
    fs::create_dir_all(root.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), root.join("pkg/target")).unwrap();
    let output = unio(&root.join("pkg"), &["doc"], &[]);
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(text(&output.stderr).contains("link"), "{}", all(&output));
    assert!(fs::read_dir(root.join("elsewhere"))
        .unwrap()
        .next()
        .is_none());
    let _ = fs::remove_dir_all(&root);
}

/// `--open` runs the opener of the system with the index page (BROWSER when it is set); the
/// test variable prints the command instead of running it.
#[test]
fn doc_open_names_the_command_that_opens_the_index() {
    let root = package("open");
    let index = root
        .join("pkg")
        .join("target")
        .join("doc")
        .join("index.html");
    let shown = plain(&index);
    let output = unio(
        &root.join("pkg"),
        &["doc", "--open"],
        &[("UNIO_TEST_PRINT_OPEN", "1")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    let expected = if cfg!(target_os = "macos") {
        format!("open {shown}")
    } else if cfg!(windows) {
        format!("rundll32.exe url.dll,FileProtocolHandler {shown}")
    } else {
        format!("xdg-open {shown}")
    };
    assert!(
        text(&output.stdout)
            .lines()
            .any(|l| l == format!("unio doc: would run {expected}")),
        "expected {expected}:\n{}",
        all(&output)
    );
    let output = unio(
        &root.join("pkg"),
        &["doc", "--open"],
        &[("UNIO_TEST_PRINT_OPEN", "1"), ("BROWSER", "my-browser")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", all(&output));
    assert!(
        text(&output.stdout)
            .lines()
            .any(|l| l == format!("unio doc: would run my-browser {shown}")),
        "{}",
        all(&output)
    );
    // An opener that cannot be run is reported; the pages stay.
    let output = unio(
        &root.join("pkg"),
        &["doc", "--open"],
        &[("BROWSER", "/no/such/browser")],
    );
    assert_eq!(output.status.code(), Some(1), "{}", all(&output));
    assert!(
        text(&output.stderr).contains("/no/such/browser"),
        "{}",
        all(&output)
    );
    assert!(index.is_file());
    let _ = fs::remove_dir_all(&root);
}
