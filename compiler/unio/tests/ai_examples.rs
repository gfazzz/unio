//! D-4: the JSON examples of docs/ai.md and docs/ru/ai.md are the output of the commands they
//! show, not copies. Each ```json block of the two documents follows a comment
//! `<!-- example: ARGS -->` that names the arguments of `unio`; the command runs in
//! `tests/data/ai/`, and its output must equal the block as JSON, member by member and in order.
//! Three things may differ: a path is shown relative to that directory or to the repository; a
//! text of the block that ends in `...` stands for every text that starts with what precedes
//! it (hashes); and `time_ms` may be any number. A block without the comment fails the test, so
//! that no example is copied by hand again. The failure message shows the output to paste.

#[allow(dead_code)]
mod common;

use std::path::Path;
use std::process::Command;

use common::repo_root;
use unio::lsp::json::Json;

const MARKER: &str = "<!-- example: ";

/// The examples of a document: the arguments and the JSON text of each block, with the line of
/// its comment; a block without a comment has no arguments.
fn examples(text: &str) -> Vec<(usize, Option<String>, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "```json" {
            let args = i
                .checked_sub(1)
                .map(|j| lines[j].trim())
                .and_then(|l| l.strip_prefix(MARKER))
                .and_then(|l| l.strip_suffix("-->"))
                .map(|l| l.trim().to_string());
            let mut j = i + 1;
            let mut block = String::new();
            while j < lines.len() && lines[j].trim() != "```" {
                block.push_str(lines[j]);
                block.push('\n');
                j += 1;
            }
            found.push((i, args, block));
            i = j;
        }
        i += 1;
    }
    found
}

/// Removes the directories of the run and of the repository from the paths of the output: from
/// every text that names one, with `\\` read as `/`.
fn relative(json: Json, dirs: &[String]) -> Json {
    match json {
        Json::String(s) => {
            let slashed = s.replace('\\', "/");
            if !dirs.iter().any(|d| slashed.contains(&format!("{d}/"))) {
                return Json::String(s);
            }
            let mut out = slashed;
            for dir in dirs {
                out = out.replace(&format!("{dir}/"), "");
            }
            Json::String(out)
        }
        Json::Array(items) => Json::Array(items.into_iter().map(|i| relative(i, dirs)).collect()),
        Json::Object(members) => Json::Object(
            members
                .into_iter()
                .map(|(k, v)| (k, relative(v, dirs)))
                .collect(),
        ),
        other => other,
    }
}

/// Whether the output matches the example, with the allowances of the module comment.
fn matches(example: &Json, output: &Json, key: Option<&str>) -> bool {
    match (example, output) {
        (Json::Number(_), Json::Number(_)) if key == Some("time_ms") => true,
        (Json::String(e), Json::String(o)) => match e.strip_suffix("...") {
            Some(prefix) => o.starts_with(prefix),
            None => e == o,
        },
        (Json::Array(e), Json::Array(o)) => {
            e.len() == o.len() && e.iter().zip(o).all(|(a, b)| matches(a, b, None))
        }
        (Json::Object(e), Json::Object(o)) => {
            e.len() == o.len()
                && e.iter()
                    .zip(o)
                    .all(|((ka, a), (kb, b))| ka == kb && matches(a, b, Some(ka)))
        }
        _ => example == output,
    }
}

fn run(dir: &Path, args: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_unio"))
        .args(args.split_whitespace())
        .current_dir(dir)
        .env_remove("UNIO_LANG")
        .output()
        .expect("unio runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn the_json_examples_of_ai_md_are_the_output_of_their_commands() {
    let root = repo_root();
    let dir = root.join("compiler/unio/tests/data/ai");
    let canonical = |p: &Path| {
        let shown = p.to_string_lossy().replace('\\', "/");
        let real = std::fs::canonicalize(p)
            .map(|c| c.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| shown.clone());
        vec![shown, real.trim_start_matches("//?/").to_string(), real]
    };
    let mut dirs = canonical(&dir);
    dirs.extend(canonical(&root));
    let mut problems = Vec::new();
    let mut english = Vec::new();
    for doc in ["docs/ai.md", "docs/ru/ai.md"] {
        let text = std::fs::read_to_string(root.join(doc)).unwrap();
        let found = examples(&text);
        assert!(!found.is_empty(), "{doc} has no JSON examples");
        for (line, args, block) in &found {
            let Some(args) = args else {
                problems.push(format!(
                    "{doc}:{}: a JSON block without `{MARKER}ARGS -->` before it",
                    line + 1
                ));
                continue;
            };
            let example = match Json::parse(block) {
                Ok(json) => json,
                Err(e) => {
                    problems.push(format!("{doc}:{}: the example is not JSON: {e}", line + 1));
                    continue;
                }
            };
            let output = run(&dir, args);
            let parsed = Json::parse(&output).map(|o| relative(o, &dirs));
            if !parsed.as_ref().is_ok_and(|o| matches(&example, o, None)) {
                problems.push(format!(
                    "{doc}:{}: `unio {args}` in tests/data/ai gives\n{output}",
                    line + 1
                ));
            }
        }
        let shown: Vec<(Option<String>, String)> =
            found.into_iter().map(|(_, a, b)| (a, b)).collect();
        if doc == "docs/ai.md" {
            english = shown;
        } else if shown != english {
            problems.push("docs/ru/ai.md has other JSON examples than docs/ai.md".to_string());
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}
