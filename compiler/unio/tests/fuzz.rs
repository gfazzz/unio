//! R-1: the front end never panics. Deterministic mutations of the corpus and the sources, and
//! the reproducers of the coverage-guided fuzzer (`fuzz/main.rs`) run through the targets that
//! found them.

mod common;
#[path = "../fuzz/targets.rs"]
mod targets;

use common::{read, repo_root, unio_files};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const PIECES: &[&str] = &[
    " ",
    "\n",
    "\t",
    ":",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "\"",
    "\\",
    "#",
    ".",
    ",",
    "=",
    "-",
    "+",
    "*",
    "**",
    "<",
    "_",
    "a",
    "Z",
    "0",
    "9",
    "try ",
    "on ",
    "function ",
    "    ",
    "...",
    "?.",
    "{x}",
    "\"{",
    "}\"",
    "if ",
    "else",
    "case ",
    "match ",
    "named ",
    "é",
    "\u{0}",
];

#[test]
fn mutated_sources_never_panic() {
    let root = repo_root();
    let mut sources: Vec<String> = Vec::new();
    for dir in ["spec/tests", "examples", "stdlib"] {
        sources.extend(unio_files(&root.join(dir)).iter().map(|p| read(p)));
    }
    let iterations: usize = std::env::var("UNIO_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..iterations {
        let mut chars: Vec<char> = sources[rng.below(sources.len())].chars().collect();
        for _ in 0..1 + rng.below(6) {
            let at = rng.below(chars.len() + 1);
            match rng.below(3) {
                0 if !chars.is_empty() => {
                    chars.remove(at.min(chars.len() - 1));
                }
                1 => {
                    let piece = PIECES[rng.below(PIECES.len())];
                    for (k, c) in piece.chars().enumerate() {
                        chars.insert(at + k, c);
                    }
                }
                _ => {
                    let end = (at + rng.below(20)).min(chars.len());
                    chars.drain(at.min(end)..end);
                }
            }
        }
        let text: String = chars.into_iter().collect();
        let analysis = unio::check_source("fuzz.unio", &text);
        let _ = analysis.render_text(unio::Lang::Ru);
        let _ = analysis.render_json(unio::Lang::En);
    }
}

#[test]
fn deep_nesting_is_a_diagnostic_not_a_crash() {
    let text = format!("let x = {}1{}\n", "(".repeat(5000), ")".repeat(5000));
    let analysis = unio::check_source("deep.unio", &text);
    assert!(!analysis.diagnostics.is_empty());
    let text = format!("let x = {}1\n", "-".repeat(5000));
    assert!(!unio::check_source("deep.unio", &text)
        .diagnostics
        .is_empty());
    let mut nested = String::from("x");
    for _ in 0..300 {
        nested = format!("\"{{{nested}}}\"");
    }
    let text = format!("let x = {nested}\n");
    let _ = unio::check_source("deep.unio", &text);
}

/// Runs one input through a fuzz target on the stack of the tool; Err holds the panic.
fn run_target(target: &str, data: &[u8], scratch: &std::path::Path) -> Result<(), String> {
    let (target, data, scratch) = (target.to_string(), data.to_vec(), scratch.to_path_buf());
    std::thread::Builder::new()
        .stack_size(targets::STACK)
        .spawn(move || targets::run(&target, &data, &scratch))
        .unwrap()
        .join()
        .map_err(|panic| {
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default()
        })
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fuzz-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every input the coverage-guided fuzzer found to crash a target, once fixed, is kept in
/// `tests/data/fuzz/TARGET/` and must stay handled (R-1; the fuzzer, `fuzz/main.rs`, and the
/// scheduled workflow `.github/workflows/fuzz.yml`).
#[test]
fn every_fuzz_reproducer_is_handled() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/fuzz");
    let dir = scratch("reproducers");
    let mut failures = Vec::new();
    let mut count = 0;
    for target in targets::TARGETS {
        let Ok(entries) = std::fs::read_dir(root.join(target)) else {
            continue;
        };
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for file in files {
            count += 1;
            let data = std::fs::read(&file).unwrap();
            if let Err(panic) = run_target(target, &data, &dir) {
                failures.push(format!("{}: {panic}", file.display()));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        failures.is_empty(),
        "{count} reproducers, failing: {failures:#?}"
    );
}

/// Every target of the fuzzer runs on an empty input, a valid program or manifest, and bytes
/// that are not UTF-8, so that the targets stay in step with the compiler they call.
#[test]
fn every_fuzz_target_runs() {
    let dir = scratch("targets");
    let program = b"public function main() returns Int:\n    print(\"{1 + 2}\")\n    return 0\n";
    let manifest = b"[package]\nname = \"demo\"\nversion = \"1.0.0\"\n\n[permissions]\nfiles.read = [\"$CWD/**\"]\n";
    for target in targets::TARGETS {
        let valid: &[u8] = match targets::seeds(target) {
            targets::Seeds::Sources => program,
            targets::Seeds::Manifests => manifest,
            targets::Seeds::Archives => b"",
        };
        for data in [&b""[..], valid, &[0xff, 0xfe, b'\n', 0x80][..]] {
            run_target(target, data, &dir).unwrap_or_else(|panic| panic!("{target}: {panic}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
