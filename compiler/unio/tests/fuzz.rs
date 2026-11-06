//! R-1: the front end never panics. Deterministic mutations of the corpus and the sources.

mod common;

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
