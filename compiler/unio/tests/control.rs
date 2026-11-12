//! Check A of the R-10 control set (plan section 12, closure condition 2; T-5b, R-12): the
//! programs of `spec/tests/control/` yield their expected diagnostics, their reference fixes are
//! clean and pass their hidden checks, and every code of the catalog that a package can produce
//! has a program.
//!
//! A program is a file `NAME.unio`, or a directory `NAME/` with `unio.toml` when the error needs
//! a package. Its faulty file starts with `# expect-error: LINE CODE` and `# category: NAME`,
//! then optionally `# profile: core` (the program is checked with `--profile=core`, as a package
//! of the `core` profile is) and `# compiler-bug: TEXT` (the expected diagnostic is the first,
//! but the compiler reports more; the test fails once the bug is gone, so that the marker is
//! removed). In a directory the faulty file is the one with these headers; it may be the
//! manifest `unio.toml` (its headers are comments of TOML) when the error is a mistake of the
//! manifest (E0088), and the module that is checked and tested is then `main.unio`.
//!
//! The reference fix of a file program is `NAME.fixed.unio`; in a directory every
//! `FILE.fixed.EXT` takes the place of `FILE.EXT` (a fix may change the manifest). The hidden
//! check `NAME.check.unio` (in a directory, beside the faulty file) holds `test` blocks that are
//! appended to the fixed module and run with `unio test`, so that "fixed" means the program does
//! what it should. A check whose first line starts with `# Form check` is a form check instead:
//! the fix is clean and in the one style, and differs from the program only on the lines it
//! names in `# changed lines: FIRST-LAST`.

#[allow(dead_code)]
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{allow_weaker_on_windows, repo_root};

const CATEGORIES: [&str; 4] = ["types", "errors", "resources", "style"];

/// Codes of the catalog that no control program can carry, with the reason. None is: the tab of
/// E0001 is allowed in spec/tests/control/tab_indentation.unio by .github/scripts/check_docs.py
/// and tools/check_docs.unio.
const EXCLUDED: &[(&str, &str)] = &[];

#[derive(Debug)]
struct Program {
    name: String,
    /// The file or the directory of the program.
    path: PathBuf,
    /// The file with the headers.
    faulty: PathBuf,
    /// The module that is checked and joined by the hidden check: the faulty file, or
    /// `main.unio` beside a faulty manifest.
    module: PathBuf,
    line: usize,
    code: String,
    category: String,
    core: bool,
    bug: Option<String>,
}

fn control_dir() -> PathBuf {
    repo_root().join("spec/tests/control")
}

fn is_helper(name: &str) -> bool {
    name.ends_with(".fixed.unio") || name.ends_with(".check.unio") || name.contains(".fixed.")
}

fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// The headers of a faulty file, or None when it has none.
fn headers(path: &Path) -> Option<(usize, String, String, bool, Option<String>)> {
    let text = read(path);
    let mut lines = text.lines();
    let expect = lines.next()?.strip_prefix("# expect-error:")?;
    let mut words = expect.split_whitespace();
    let line = words.next()?.parse().ok()?;
    let code = words.next()?.to_string();
    let category = lines
        .next()?
        .strip_prefix("# category:")?
        .trim()
        .to_string();
    let mut core = false;
    let mut bug = None;
    for rest in lines {
        if let Some(profile) = rest.strip_prefix("# profile:") {
            assert_eq!(
                profile.trim(),
                "core",
                "{}: unknown profile",
                path.display()
            );
            core = true;
        } else if let Some(text) = rest.strip_prefix("# compiler-bug:") {
            bug = Some(text.trim().to_string());
        } else {
            break;
        }
    }
    Some((line, code, category, core, bug))
}

fn programs() -> Vec<Program> {
    let mut found = Vec::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(control_dir())
        .expect("spec/tests/control exists")
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = file_name(&path);
        let faulty = if path.is_dir() {
            assert!(
                path.join("unio.toml").is_file(),
                "{name}: a program directory has unio.toml"
            );
            let with_headers: Vec<PathBuf> = files_below(&path)
                .into_iter()
                .filter(|f| {
                    let n = file_name(f);
                    (n.ends_with(".unio") || n == "unio.toml")
                        && !is_helper(&n)
                        && headers(f).is_some()
                })
                .collect();
            assert_eq!(
                with_headers.len(),
                1,
                "{name}: exactly one file of the package has the headers"
            );
            with_headers[0].clone()
        } else if name.ends_with(".unio") && !is_helper(&name) {
            path.clone()
        } else {
            continue;
        };
        let (line, code, category, core, bug) = headers(&faulty).unwrap_or_else(|| {
            panic!(
                "{}: no '# expect-error: LINE CODE' and '# category:' headers",
                faulty.display()
            )
        });
        let module = if file_name(&faulty) == "unio.toml" {
            faulty.with_file_name("main.unio")
        } else {
            faulty.clone()
        };
        found.push(Program {
            name: name.trim_end_matches(".unio").to_string(),
            path,
            faulty,
            module,
            line,
            code,
            category,
            core,
            bug,
        });
    }
    found
}

/// The hidden check of a program.
fn check_file(program: &Program) -> PathBuf {
    if program.path.is_dir() {
        let stem = file_name(&program.module)
            .trim_end_matches(".unio")
            .to_string();
        program.module.with_file_name(format!("{stem}.check.unio"))
    } else {
        control_dir().join(format!("{}.check.unio", program.name))
    }
}

fn unio(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unio"));
    command.current_dir(dir);
    allow_weaker_on_windows(&mut command);
    command
}

/// Runs `f` on every program on a few threads and collects the failures.
fn each<F>(programs: &[Program], f: F) -> Vec<String>
where
    F: Fn(&Program) -> Result<(), String> + Sync,
{
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    let chunk = programs.len().div_ceil(threads).max(1);
    let mut failures = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = programs
            .chunks(chunk)
            .map(|part| {
                let f = &f;
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|p| f(p).err().map(|e| format!("{}: {e}", p.name)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            failures.extend(handle.join().unwrap());
        }
    });
    failures
}

/// A JSON value: enough of JSON to read the output of `--diagnostics=json` and `test --json`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    fn parse(text: &str) -> Result<Json, String> {
        let chars: Vec<char> = text.chars().collect();
        let mut at = 0;
        let value = parse_value(&chars, &mut at)?;
        skip_space(&chars, &mut at);
        if at != chars.len() {
            return Err(format!("text after the JSON value at {at}"));
        }
        Ok(value)
    }

    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(fields) => fields.get(key).unwrap_or(&Json::Null),
            _ => &Json::Null,
        }
    }

    fn items(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }

    fn text(&self) -> &str {
        match self {
            Json::Text(text) => text,
            _ => "",
        }
    }

    fn number(&self) -> f64 {
        match self {
            Json::Number(n) => *n,
            _ => f64::NAN,
        }
    }
}

fn skip_space(chars: &[char], at: &mut usize) {
    while *at < chars.len() && chars[*at].is_whitespace() {
        *at += 1;
    }
}

fn expect_word(chars: &[char], at: &mut usize, word: &str) -> Result<(), String> {
    for c in word.chars() {
        if chars.get(*at) != Some(&c) {
            return Err(format!("expected '{word}' at {at}"));
        }
        *at += 1;
    }
    Ok(())
}

fn parse_value(chars: &[char], at: &mut usize) -> Result<Json, String> {
    skip_space(chars, at);
    match chars.get(*at) {
        Some('{') => {
            *at += 1;
            let mut fields = BTreeMap::new();
            skip_space(chars, at);
            if chars.get(*at) == Some(&'}') {
                *at += 1;
                return Ok(Json::Object(fields));
            }
            loop {
                skip_space(chars, at);
                let key = match parse_value(chars, at)? {
                    Json::Text(key) => key,
                    _ => return Err(format!("expected a key at {at}")),
                };
                skip_space(chars, at);
                expect_word(chars, at, ":")?;
                let value = parse_value(chars, at)?;
                fields.insert(key, value);
                skip_space(chars, at);
                match chars.get(*at) {
                    Some(',') => *at += 1,
                    Some('}') => {
                        *at += 1;
                        return Ok(Json::Object(fields));
                    }
                    _ => return Err(format!("expected ',' or '}}' at {at}")),
                }
            }
        }
        Some('[') => {
            *at += 1;
            let mut items = Vec::new();
            skip_space(chars, at);
            if chars.get(*at) == Some(&']') {
                *at += 1;
                return Ok(Json::Array(items));
            }
            loop {
                items.push(parse_value(chars, at)?);
                skip_space(chars, at);
                match chars.get(*at) {
                    Some(',') => *at += 1,
                    Some(']') => {
                        *at += 1;
                        return Ok(Json::Array(items));
                    }
                    _ => return Err(format!("expected ',' or ']' at {at}")),
                }
            }
        }
        Some('"') => {
            *at += 1;
            let mut text = String::new();
            loop {
                let c = *chars.get(*at).ok_or("unterminated text")?;
                *at += 1;
                match c {
                    '"' => return Ok(Json::Text(text)),
                    '\\' => {
                        let e = *chars.get(*at).ok_or("unterminated escape")?;
                        *at += 1;
                        match e {
                            'n' => text.push('\n'),
                            't' => text.push('\t'),
                            'r' => text.push('\r'),
                            'b' => text.push('\u{8}'),
                            'f' => text.push('\u{c}'),
                            'u' => {
                                let hex: String =
                                    chars.get(*at..*at + 4).ok_or("short \\u")?.iter().collect();
                                *at += 4;
                                let code =
                                    u32::from_str_radix(&hex, 16).map_err(|e| e.to_string())?;
                                text.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                            other => text.push(other),
                        }
                    }
                    other => text.push(other),
                }
            }
        }
        Some('t') => expect_word(chars, at, "true").map(|_| Json::Bool(true)),
        Some('f') => expect_word(chars, at, "false").map(|_| Json::Bool(false)),
        Some('n') => expect_word(chars, at, "null").map(|_| Json::Null),
        Some(_) => {
            let start = *at;
            while *at < chars.len() && "+-0123456789.eE".contains(chars[*at]) {
                *at += 1;
            }
            let number: String = chars[start..*at].iter().collect();
            number
                .parse()
                .map(Json::Number)
                .map_err(|_| format!("expected a value at {start}"))
        }
        None => Err("unexpected end of JSON".to_string()),
    }
}

/// An error diagnostic: its code, the file name of its first span, and its line and column.
#[derive(Debug)]
struct Found {
    code: String,
    file: String,
    line: usize,
    column: usize,
    message: String,
}

impl std::fmt::Display for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {}:{}:{} ({})",
            self.code, self.file, self.line, self.column, self.message
        )
    }
}

/// The errors of `unio check --diagnostics=json` on the files, the faulty or main file first,
/// then in order of line and column.
fn errors(dir: &Path, files: &[PathBuf], core: bool, first: &Path) -> Result<Vec<Found>, String> {
    let mut command = unio(dir);
    command.arg("check").arg("--diagnostics=json");
    if core {
        command.arg("--profile=core");
    }
    command.args(files);
    let output = command.output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = Json::parse(stdout.trim()).map_err(|e| {
        format!(
            "no JSON diagnostics ({e}): {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    let first = file_name(first);
    let mut found: Vec<Found> = json
        .items()
        .iter()
        .filter(|d| d.get("severity").text() == "error")
        .map(|d| {
            let span = &d.get("spans").items()[0];
            Found {
                code: d.get("code").text().to_string(),
                file: file_name(Path::new(span.get("file").text())),
                line: span.get("start").get("line").number() as usize,
                column: span.get("start").get("column").number() as usize,
                message: d.get("message").text().to_string(),
            }
        })
        .collect();
    found.sort_by_key(|f| (f.file != first, f.file.clone(), f.line, f.column));
    if found.is_empty() != output.status.success() {
        return Err(format!(
            "exit code {:?} does not agree with the diagnostics",
            output.status.code()
        ));
    }
    Ok(found)
}

#[test]
fn the_control_set_has_200_programs_in_four_categories() {
    let programs = programs();
    // The 200 programs of the model trial of check B, and the programs of codes added after it
    // (E0088, found by that trial).
    assert!(
        programs.len() >= 200,
        "the control set has at least 200 programs"
    );
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &programs {
        assert!(
            CATEGORIES.contains(&p.category.as_str()),
            "{}: unknown category '{}'",
            p.name,
            p.category
        );
        *counts
            .entry(CATEGORIES.iter().find(|c| **c == p.category).unwrap())
            .or_default() += 1;
        assert!(
            p.code.len() == 5 && p.code.starts_with('E'),
            "{}: '{}' is not a code",
            p.name,
            p.code
        );
        assert!(p.line >= 1, "{}: the line starts at 1", p.name);
        let fixed: Vec<PathBuf> = if p.path.is_dir() {
            files_below(&p.path)
                .into_iter()
                .filter(|f| file_name(f).contains(".fixed."))
                .collect()
        } else {
            let f = control_dir().join(format!("{}.fixed.unio", p.name));
            if f.is_file() {
                vec![f]
            } else {
                vec![]
            }
        };
        assert!(!fixed.is_empty(), "{}: no reference fix", p.name);
        assert!(check_file(p).is_file(), "{}: no hidden check", p.name);
    }
    for category in CATEGORIES {
        let count = counts.get(category).copied().unwrap_or(0);
        assert!(
            count >= 40,
            "the category {category} has {count} programs, fewer than 40: {counts:?}"
        );
    }
}

#[test]
fn every_control_program_yields_its_expected_diagnostic() {
    let programs = programs();
    let failures = each(&programs, |p| {
        let dir = p.module.parent().unwrap();
        let found = errors(
            dir,
            &[PathBuf::from(file_name(&p.module))],
            p.core,
            &p.faulty,
        )?;
        let list = || {
            found
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        };
        let Some(first) = found.first() else {
            return Err(format!(
                "expected {} on line {}, got no error",
                p.code, p.line
            ));
        };
        if first.code != p.code || first.line != p.line || first.file != file_name(&p.faulty) {
            return Err(format!(
                "expected {} on line {} of {}, got: {}",
                p.code,
                p.line,
                file_name(&p.faulty),
                list()
            ));
        }
        let others = &found[1..];
        match &p.bug {
            Some(bug) if others.is_empty() => Err(format!(
                "the marked compiler bug ({bug}) no longer shows; remove '# compiler-bug:'"
            )),
            Some(_) => Ok(()),
            // An import cycle is reported once in each module of the cycle.
            None if others
                .iter()
                .all(|f| f.code == "E0058" && p.code == "E0058" && f.file != first.file) =>
            {
                Ok(())
            }
            None => Err(format!("other errors besides the expected one: {}", list())),
        }
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A fresh copy of the fixed program in a directory of its own: the module that the hidden
/// check joins, and every source of the fixed program.
fn fixed_copy(p: &Program) -> (PathBuf, PathBuf, Vec<PathBuf>) {
    let root = std::env::temp_dir().join(format!("unio-control-{}-{}", std::process::id(), p.name));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    if !p.path.is_dir() {
        let module = root.join(format!("{}.unio", p.name));
        fs::copy(
            control_dir().join(format!("{}.fixed.unio", p.name)),
            &module,
        )
        .unwrap();
        return (root, module.clone(), vec![module]);
    }
    let mut sources = Vec::new();
    let mut module = None;
    for file in files_below(&p.path) {
        let relative = file.strip_prefix(&p.path).unwrap();
        let name = file_name(&file);
        if name.ends_with(".check.unio") {
            continue;
        }
        let target_name = name.replacen(".fixed.", ".", 1);
        if !name.contains(".fixed.") && file.with_file_name(fixed_name(&name)).is_file() {
            continue; // replaced by its fix
        }
        let target = root.join(relative).with_file_name(&target_name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(&file, &target).unwrap();
        if target_name.ends_with(".unio") {
            sources.push(target.clone());
        }
        if file.with_file_name(&target_name) == p.module {
            module = Some(target);
        }
    }
    sources.sort();
    (root, module.expect("the faulty module is copied"), sources)
}

/// `main.unio` -> `main.fixed.unio`, `unio.toml` -> `unio.fixed.toml`.
fn fixed_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}.fixed.{ext}"),
        None => format!("{name}.fixed"),
    }
}

/// The lines in which the program and its fix differ, numbered as in the program: the lines
/// between the common start and the common end, after the headers.
fn changed_lines(program: &str, fixed: &str) -> (usize, usize) {
    let all: Vec<&str> = program.split('\n').collect();
    let skip = all
        .iter()
        .take_while(|l| {
            [
                "# expect-error:",
                "# category:",
                "# profile:",
                "# compiler-bug:",
            ]
            .iter()
            .any(|h| l.starts_with(h))
        })
        .count();
    let a = &all[skip..];
    let b: Vec<&str> = fixed.split('\n').collect();
    let mut prefix = 0;
    while prefix < a.len().min(b.len()) && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len().min(b.len()) - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let first = prefix + 1 + skip;
    let last = (a.len() - suffix + skip).max(first);
    (first, last)
}

#[test]
fn every_reference_fix_is_clean_and_passes_its_tests() {
    let programs = programs();
    let failures = each(&programs, |p| {
        let (root, module, sources) = fixed_copy(p);
        let result = (|| {
            let found = errors(&root, &sources, p.core, &module)?;
            if !found.is_empty() {
                let list: Vec<String> = found.iter().map(|f| f.to_string()).collect();
                return Err(format!("the fix is not clean: {}", list.join("; ")));
            }
            let output = unio(&root)
                .arg("fmt")
                .arg("--check")
                .args(&sources)
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "the fix is not in the one style: {}",
                    String::from_utf8_lossy(&output.stdout)
                ));
            }
            let check = read(&check_file(p));
            if check.starts_with("# Form check") {
                let range = check
                    .lines()
                    .find_map(|l| l.strip_prefix("# changed lines:"))
                    .ok_or("a form check names its changed lines")?;
                let (from, to) = range.trim().split_once('-').ok_or("FIRST-LAST")?;
                let (from, to): (usize, usize) = (
                    from.parse().map_err(|_| "a line number")?,
                    to.parse().map_err(|_| "a line number")?,
                );
                let fixed = read(&module);
                let (first, last) = changed_lines(&read(&p.faulty), &fixed);
                if first < from || last > to {
                    return Err(format!(
                        "the fix changes lines {first}-{last}, the form check allows {from}-{to}"
                    ));
                }
                return Ok(());
            }
            let mut text = read(&module);
            text.push('\n');
            text.push_str(&check);
            fs::write(&module, text).unwrap();
            let output = unio(module.parent().unwrap())
                .arg("test")
                .arg("--json")
                .arg(file_name(&module))
                .output()
                .map_err(|e| e.to_string())?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let json = Json::parse(stdout.trim()).map_err(|e| {
                format!(
                    "no JSON from unio test ({e}): {stdout}{}",
                    String::from_utf8_lossy(&output.stderr)
                )
            })?;
            let results = json.items();
            if results.is_empty() {
                return Err("the hidden check ran no test".to_string());
            }
            let failed: Vec<String> = results
                .iter()
                .filter(|r| r.get("status").text() != "passed")
                .map(|r| format!("{}: {}", r.get("name").text(), r.get("message").text()))
                .collect();
            if !failed.is_empty() || !output.status.success() {
                return Err(format!("hidden tests fail: {}", failed.join("; ")));
            }
            Ok(())
        })();
        let _ = fs::remove_dir_all(&root);
        result
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_code_has_a_control_program() {
    let programs = programs();
    let mut missing = Vec::new();
    for code in unio::catalog::CODES {
        let excluded = EXCLUDED.iter().any(|(c, _)| c == code);
        let covered = programs.iter().any(|p| p.code == *code);
        assert!(
            !(excluded && covered),
            "{code} has a control program; remove it from EXCLUDED"
        );
        if !excluded && !covered {
            missing.push(*code);
        }
    }
    assert!(
        missing.is_empty(),
        "codes without a control program: {missing:?}"
    );
    for p in &programs {
        assert!(
            unio::catalog::CODES.contains(&p.code.as_str()),
            "{}: {} is not a code of the catalog",
            p.name,
            p.code
        );
    }
}
