//! The semantic corpus (spec/tests/semantic) and the exit criterion of sprint S3: every Unio
//! source of the repository passes name and type checks. The corpus is checked by the compiler
//! only; spec/check_grammar.py checks syntax.

#[allow(dead_code)]
mod common;

use common::{repo_root as repository, unio_files};

fn expectation(source: &str) -> (usize, String) {
    let line = source.lines().next().unwrap_or_default();
    let rest = line
        .strip_prefix("# expect-error:")
        .unwrap_or_else(|| panic!("no '# expect-error: LINE CODE' line: {line}"));
    let mut words = rest.split_whitespace();
    let number = words.next().unwrap().parse().unwrap();
    let code = words.next().expect("a code").to_string();
    (number, code)
}

#[test]
fn valid_semantic_corpus_has_no_diagnostics() {
    let dir = repository().join("spec/tests/semantic/valid");
    for file in unio_files(&dir) {
        let results = unio::check_files(std::slice::from_ref(&file));
        for analysis in &results {
            assert!(
                analysis.diagnostics.is_empty(),
                "{}:\n{}",
                analysis.path,
                analysis.render_text(unio::Lang::En)
            );
        }
    }
}

#[test]
fn invalid_semantic_corpus_reports_the_expected_first_error() {
    let dir = repository().join("spec/tests/semantic/invalid");
    let mut failures = Vec::new();
    for file in unio_files(&dir) {
        let source = std::fs::read_to_string(&file).unwrap();
        let (line, code) = expectation(&source);
        let results = unio::check_files(std::slice::from_ref(&file));
        let analysis = results
            .iter()
            .find(|a| std::path::Path::new(&a.path) == file)
            .expect("the file is checked");
        let mut found: Vec<_> = analysis.diagnostics.iter().collect();
        found.sort_by_key(|d| (d.line(), d.span.start.column));
        match found.first() {
            Some(d) if d.line() == line && d.code == code => {}
            Some(_) => failures.push(format!(
                "{}: expected {code} on line {line}, got:\n{}",
                file.display(),
                analysis.render_text(unio::Lang::En)
            )),
            None => failures.push(format!(
                "{}: expected {code} on line {line}, got nothing",
                file.display()
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_repository_source_passes_name_and_type_checks() {
    let root = repository();
    let mut files = Vec::new();
    for dir in ["docs/examples", "docs/ru/examples", "examples", "tools"] {
        files.extend(unio_files(&root.join(dir)));
    }
    files.extend(unio_files(&root.join("stdlib")));
    let results = unio::check_files(&files);
    let report: Vec<String> = results
        .iter()
        .filter(|a| !a.diagnostics.is_empty())
        .map(|a| a.render_text(unio::Lang::En))
        .collect();
    assert!(report.is_empty(), "{}", report.join("\n"));
}

/// R-1 for the name and type checks: renaming names at random keeps the syntax valid and must
/// never make the checker panic. Deterministic, so a failure can be reproduced.
#[test]
fn renamed_names_never_make_the_checker_panic() {
    let root = repository();
    let mut sources = unio_files(&root.join("examples"));
    sources.extend(unio_files(&root.join("docs/examples")));
    let iterations: usize = std::env::var("UNIO_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let dir = std::env::temp_dir().join(format!("unio-semantic-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut state: u64 = 0x5eed;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    for i in 0..iterations {
        let source = std::fs::read_to_string(&sources[i % sources.len()]).unwrap();
        let words: Vec<(usize, usize)> = word_spans(&source);
        if words.len() < 2 {
            continue;
        }
        let mut mutated = source.clone();
        for _ in 0..3 {
            let (start, end) = words[next() % words.len()];
            let (s2, e2) = words[next() % words.len()];
            if end > mutated.len() || e2 > source.len() {
                continue;
            }
            let replacement = source[s2..e2].to_string();
            mutated.replace_range(start..end.min(mutated.len()), &replacement);
            break;
        }
        let file = dir.join("mutated.unio");
        std::fs::write(&file, &mutated).unwrap();
        let _ = unio::check_files(std::slice::from_ref(&file));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Byte ranges of lower-case identifiers outside comments and texts.
fn word_spans(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    let mut in_text = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_text = !in_text;
        } else if c == b'#' && !in_text {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if !in_text && c.is_ascii_lowercase() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            spans.push((start, i));
            continue;
        }
        i += 1;
    }
    spans
}

/// An import resolves inside the package or the standard library; a link that leads out of the
/// package is not followed (docs/semantics.md section 1, E0053).
#[cfg(unix)]
#[test]
fn an_import_through_a_link_out_of_the_package_is_rejected() {
    let dir = std::env::temp_dir().join(format!("unio-import-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("package/lib")).unwrap();
    std::fs::create_dir_all(dir.join("outside")).unwrap();
    std::fs::write(dir.join("package/unio.toml"), "[package]\nname = \"p\"\n").unwrap();
    std::fs::write(
        dir.join("outside/secret.unio"),
        "public function value() returns Int:\n    return 1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("package/lib/inner.unio"),
        "public function value() returns Int:\n    return 2\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        dir.join("outside/secret.unio"),
        dir.join("package/secret.unio"),
    )
    .unwrap();
    std::os::unix::fs::symlink(dir.join("outside"), dir.join("package/linked")).unwrap();
    std::os::unix::fs::symlink(
        dir.join("package/lib/inner.unio"),
        dir.join("package/alias.unio"),
    )
    .unwrap();
    let main = dir.join("package/main.unio");
    std::fs::write(
        &main,
        "import secret\nimport linked.secret as other\nimport alias\n\npublic function main() returns Int:\n    return alias.value()\n",
    )
    .unwrap();
    let results = unio::check_files(std::slice::from_ref(&main));
    let analysis = results
        .iter()
        .find(|a| std::path::Path::new(&a.path) == main)
        .expect("the file is checked");
    let rendered = analysis.render_text(unio::Lang::En);
    let lines: Vec<usize> = analysis
        .diagnostics
        .iter()
        .filter(|d| d.code == "E0053")
        .map(|d| d.line())
        .collect();
    assert_eq!(lines, [1, 2], "{rendered}");
    assert!(rendered.contains("outside the package"), "{rendered}");
    assert_eq!(analysis.diagnostics.len(), 2, "{rendered}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// T-14: `deps.<alias>` names a manifest dependency, never a directory of the package; with no
/// package manager in 0.1 an import of one is E0048, which says whether the alias is declared.
#[test]
fn a_dependency_import_is_missing_identity() {
    let dir = std::env::temp_dir().join(format!("unio-semantic-deps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("deps/money")).unwrap();
    std::fs::write(
        dir.join("unio.toml"),
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\n\n[dependencies]\nmoney = { registry = \"main\", package = \"acme/money\", version = \"^1.2\" }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("deps/money/ledger.unio"),
        "public let TOTAL as Int = 1\n",
    )
    .unwrap();
    let main = dir.join("main.unio");
    std::fs::write(
        &main,
        "import deps.money.ledger\nimport deps.tax.rates\n\nfunction main():\n    print(\"shop\")\n",
    )
    .unwrap();
    let results = unio::check_files(std::slice::from_ref(&main));
    let _ = std::fs::remove_dir_all(&dir);
    let analysis = &results[0];
    let found: Vec<(&str, usize, String)> = analysis
        .diagnostics
        .iter()
        .map(|d| (d.code, d.line(), d.message.en.clone()))
        .collect();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found.iter().all(|(code, _, _)| *code == "E0048"),
        "{found:?}"
    );
    assert!(found[0].2.contains("declared in unio.toml"), "{found:?}");
    assert!(found[1].2.contains("not a dependency"), "{found:?}");
}

/// The trusted standard library is chosen by its origin (RFC 0004): a `stdlib` directory in or
/// above a package never replaces it, so a package cannot bring its own library modules.
#[test]
fn a_stdlib_directory_in_a_package_does_not_replace_the_standard_library() {
    let dir = std::env::temp_dir().join(format!("unio-semantic-own-stdlib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let real = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
    std::fs::create_dir_all(dir.join("stdlib/core")).unwrap();
    std::fs::create_dir_all(dir.join("stdlib/std")).unwrap();
    std::fs::copy(
        real.join("core/prelude.unio"),
        dir.join("stdlib/core/prelude.unio"),
    )
    .unwrap();
    std::fs::write(
        dir.join("stdlib/std/evil.unio"),
        "unsafe module\n\n## Not a part of the library.\npublic external function poke() returns Int\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("unio.toml"),
        "[package]\nname = \"own\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let main = dir.join("main.unio");
    std::fs::write(
        &main,
        "import std.evil\n\nfunction main():\n    print(\"{evil.poke()}\")\n",
    )
    .unwrap();
    let results = unio::check_files(std::slice::from_ref(&main));
    let _ = std::fs::remove_dir_all(&dir);
    let codes: Vec<&str> = results[0].diagnostics.iter().map(|d| d.code).collect();
    assert!(
        codes.contains(&"E0053"),
        "std.evil must not be found: {codes:?}"
    );
}

/// L-31: an implementation belongs to the package of the type or of the interface, so any
/// module of that package may hold it (docs/semantics.md 3.6); a second implementation in
/// another module of the package is still E0044, and so is one of a type and an interface
/// that the package does not own, here `Add` for `Bool`, both of the standard library.
#[test]
fn an_implementation_may_be_in_any_module_of_the_package_of_the_type_or_interface() {
    let dir = std::env::temp_dir().join(format!("unio-semantic-l31-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("shapes")).unwrap();
    std::fs::create_dir_all(dir.join("views")).unwrap();
    std::fs::write(
        dir.join("unio.toml"),
        "[package]\nname = \"geometry\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("shapes/point.unio"),
        "public struct Point:\n    public x as Int\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("views/named.unio"),
        "public interface Named:\n    function name(self) returns Text\n",
    )
    .unwrap();
    // The type and the interface are declared in two other modules of the package.
    std::fs::write(
        dir.join("views/point_view.unio"),
        "from shapes.point import Point\nfrom views.named import Named\n\nextend Point with Named:\n    function name(self) returns Text:\n        return \"point {self.x}\"\n\nextend Point with Display:\n    function to_text(self) returns Text:\n        return \"({self.x})\"\n",
    )
    .unwrap();
    let main = dir.join("main.unio");
    std::fs::write(
        &main,
        "from shapes.point import Point\nfrom views.named import Named\nimport views.point_view\n\nfunction describe(n as Named) returns Text:\n    return n.name()\n\npublic function main() returns Int:\n    let p = Point(x = 1)\n    print(describe(p))\n    print(\"{p}\")\n    return 0\n",
    )
    .unwrap();
    let results = unio::check_files(std::slice::from_ref(&main));
    let found: Vec<(String, &str, usize)> = results
        .iter()
        .flat_map(|a| {
            a.diagnostics
                .iter()
                .map(|d| (a.path.clone(), d.code, d.line()))
        })
        .collect();
    assert!(found.is_empty(), "{found:?}");

    // A second implementation in a third module of the same package is still refused.
    std::fs::write(
        dir.join("again.unio"),
        "from shapes.point import Point\nfrom views.named import Named\nimport views.point_view\n\nextend Point with Named:\n    function name(self) returns Text:\n        return \"again\"\n",
    )
    .unwrap();
    // An implementation of a library interface for a library type is outside the package.
    std::fs::write(
        dir.join("foreign.unio"),
        "from views.named import Named\n\nextend Int with Named:\n    function name(self) returns Text:\n        return \"int\"\n\nextend Bool with Add:\n    function add(self, other as Bool) returns Bool:\n        return self or other\n",
    )
    .unwrap();
    let files = [
        dir.join("again.unio"),
        dir.join("views/point_view.unio"),
        dir.join("foreign.unio"),
    ];
    let results = unio::check_files(&files);
    let _ = std::fs::remove_dir_all(&dir);
    let codes = |name: &str| -> Vec<(&str, usize)> {
        results
            .iter()
            .filter(|a| a.path.ends_with(name))
            .flat_map(|a| a.diagnostics.iter().map(|d| (d.code, d.line())))
            .collect()
    };
    // One of the two implementations of `Named` for `Point` is the second, whichever is loaded
    // first.
    let mut second = codes("again.unio");
    second.extend(codes("point_view.unio"));
    assert_eq!(second.len(), 1, "{second:?}");
    assert_eq!(second[0].0, "E0044", "{second:?}");
    // `Named` is the package's own, so `Int` may implement it; `Add` for `Bool` is not.
    let foreign = codes("foreign.unio");
    assert_eq!(foreign, [("E0044", 7)], "{foreign:?}");
    let rendered = results
        .iter()
        .find(|a| a.path.ends_with("foreign.unio"))
        .unwrap()
        .render_text(unio::Lang::En);
    assert!(rendered.contains("only in the package"), "{rendered}");
}
