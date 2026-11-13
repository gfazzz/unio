//! The coverage-guided fuzzer of the compiler (R-1), with no dependency beyond the standard
//! library of Rust and the stable toolchain.
//!
//! Coverage comes from the SanitizerCoverage pass of LLVM, which stable `rustc` runs with
//! `-Cpasses=sancov-module` and inline 8-bit counters: every edge of the instrumented crates
//! (`unio`, `unio-runtime`, Cranelift) gets a counter, and the module constructors hand the
//! counter arrays to `__sanitizer_cov_8bit_counters_init`, defined below. An input that reaches
//! a counter, or a hit count of it, not reached before joins the corpus; inputs are mutated from
//! the corpus with byte, token, line, and splice mutations whose dictionary is taken from the
//! seeds (the sources and manifests of the repository), so mutations stay close to the grammar.
//! `fuzz/build.sh` builds it with the instrumentation; built without it (as `cargo test` and
//! `cargo clippy` build examples), it runs as a blind mutator.
//!
//! Commands:
//!
//! ```text
//! fuzz run [--targets T,T...] [--jobs N] [--minutes M | --seconds S] [--min-share F]
//!          [--work DIR] [--timeout S] [--memory-mb MB]
//! fuzz exec --target T FILE...
//! fuzz minimize --target T FILE [--output FILE]
//! ```
//!
//! `run` starts N worker processes (N defaults to the number of targets and may not be fewer),
//! each fuzzing one target (in turn from the list, so every target has at least one), and
//! watches them through the supervisor (`supervisor.rs`): a panic is recorded by the worker,
//! which goes on; an abort, an overflow of the stack, or a signal ends the worker, and the
//! supervisor records the input it was running from `WORK/scratch-I/current` and starts it
//! again; an input that runs longer than the timeout is recorded as a hang; a worker that dies
//! before it wrote an input is a finding as well, kept with its output. Findings go to
//! `WORK/crashes/` (input and a `.txt` with the reason), the corpus to `WORK/corpus/TARGET/`,
//! shared by the workers and kept between runs. The supervisor counts the wall time, the
//! processor time (on Linux), and the executions of every worker across restarts
//! (`WORK/stats-I`) and writes `WORK/summary.json`. The exit code is 1 when the run recorded a
//! finding or a worker ran less than `--min-share` (0.95) of the time or no input at all, and
//! 2 on a usage error. A finding becomes a regression test when its (minimized) input is copied
//! to `tests/data/fuzz/TARGET/` (`tests/fuzz.rs`). `UNIO_FUZZ_FAULT=KIND:TARGET` makes the
//! worker of TARGET fail on purpose (`fault` below), to check that the supervisor catches it.

#[path = "supervisor.rs"]
mod supervisor;
#[path = "targets.rs"]
mod targets;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use supervisor::{fnv, now_secs};

// ---------------------------------------------------------------- coverage

const MAX_REGIONS: usize = 1 << 14;
static REGION_START: [AtomicUsize; MAX_REGIONS] = [const { AtomicUsize::new(0) }; MAX_REGIONS];
static REGION_END: [AtomicUsize; MAX_REGIONS] = [const { AtomicUsize::new(0) }; MAX_REGIONS];
static REGIONS: AtomicUsize = AtomicUsize::new(0);

/// Called by the module constructors of the instrumented code, before `main`, once for every
/// module with counters.
#[no_mangle]
pub extern "C" fn __sanitizer_cov_8bit_counters_init(start: *mut u8, end: *mut u8) {
    let i = REGIONS.fetch_add(1, Ordering::SeqCst);
    if i < MAX_REGIONS && (start as usize) < (end as usize) {
        REGION_START[i].store(start as usize, Ordering::SeqCst);
        REGION_END[i].store(end as usize, Ordering::SeqCst);
    }
}

/// The counter arrays of the instrumented modules.
fn regions() -> Vec<&'static mut [u8]> {
    let n = REGIONS.load(Ordering::SeqCst).min(MAX_REGIONS);
    (0..n)
        .filter_map(|i| {
            let (start, end) = (
                REGION_START[i].load(Ordering::SeqCst),
                REGION_END[i].load(Ordering::SeqCst),
            );
            // SAFETY: the instrumentation gives each module's counters as [start, end), which
            // live as long as the program; only this thread reads or clears them, between runs.
            (start < end)
                .then(|| unsafe { std::slice::from_raw_parts_mut(start as *mut u8, end - start) })
        })
        .collect()
}

/// The hit count of a counter as a class bit, as AFL does: 1, 2, 3, 4-7, 8-15, 16-31, 32-127,
/// 128 and more.
fn class(count: u8) -> u8 {
    match count {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        4..=7 => 8,
        8..=15 => 16,
        16..=31 => 32,
        32..=127 => 64,
        _ => 128,
    }
}

struct Coverage {
    counters: Vec<&'static mut [u8]>,
    /// The classes seen per counter, over all counter arrays in order.
    seen: Vec<u8>,
    /// Counters reached at least once.
    edges: usize,
}

impl Coverage {
    fn new() -> Coverage {
        let counters = regions();
        let total = counters.iter().map(|r| r.len()).sum();
        Coverage {
            counters,
            seen: vec![0; total],
            edges: 0,
        }
    }

    fn total(&self) -> usize {
        self.seen.len()
    }

    fn clear(&mut self) {
        for region in &mut self.counters {
            region.fill(0);
        }
    }

    /// Merges the counters of the last run; true when they reached something new.
    fn merge(&mut self) -> bool {
        let mut new = false;
        let mut base = 0;
        for region in &self.counters {
            for (i, chunk) in region.chunks(8).enumerate() {
                if chunk.iter().all(|c| *c == 0) {
                    continue;
                }
                for (j, count) in chunk.iter().enumerate() {
                    let bit = class(*count);
                    let seen = &mut self.seen[base + i * 8 + j];
                    if bit != 0 && *seen & bit == 0 {
                        if *seen == 0 {
                            self.edges += 1;
                        }
                        *seen |= bit;
                        new = true;
                    }
                }
            }
            base += region.len();
        }
        new
    }
}

// ---------------------------------------------------------------- running one input

/// The location and message of the last panic, kept by the hook so that the worker can name
/// the finding.
static PANIC: Mutex<Option<String>> = Mutex::new(None);

fn quiet_panics() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let message = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            String::new()
        };
        let mut last = PANIC.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_none() {
            *last = Some(format!("panicked at {location}: {message}"));
        }
    }));
}

/// Runs the input on a thread with the stack of the tool; Err holds the panic.
fn run_one(target: &str, data: &[u8], scratch: &Path) -> Result<(), String> {
    *PANIC.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let (target, data, scratch) = (target.to_string(), data.to_vec(), scratch.to_path_buf());
    let joined = std::thread::Builder::new()
        .name("fuzz-target".to_string())
        .stack_size(targets::STACK)
        .spawn(move || targets::run(&target, &data, &scratch))
        .expect("the thread of the target starts")
        .join();
    match joined {
        Ok(()) => Ok(()),
        Err(_) => Err(PANIC
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_else(|| "panicked".to_string())),
    }
}

// ---------------------------------------------------------------- inputs and mutation

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

const MAX_LEN: usize = 64 * 1024;

/// Pieces every source dictionary has, beyond those taken from the seeds: the delimiters and
/// the forms most likely to break nesting, strings, and numbers.
const PIECES: &[&str] = &[
    " ",
    "\n",
    "\t",
    "\r\n",
    "    ",
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
    "##",
    ".",
    ",",
    "=",
    "-",
    "+",
    "*",
    "/",
    "%",
    "<",
    ">",
    "?",
    "?.",
    "...",
    "_",
    "\"{",
    "}\"",
    "{x}",
    "\\{",
    "\\u{",
    "0",
    "-1",
    "1.5",
    "1e309",
    "0x",
    "0b",
    "1_000",
    "9223372036854775807",
    "-9223372036854775808",
    "18446744073709551616",
    "\u{0}",
    "\u{feff}",
    "é",
    "😀",
    "\u{202e}",
];

const TOML_PIECES: &[&str] = &[
    "[",
    "]",
    "[[",
    "]]",
    "=",
    "\"",
    "'",
    "\"\"\"",
    "'''",
    "\\",
    "\\u",
    "#",
    "\n",
    ".",
    ",",
    "{",
    "}",
    "true",
    "false",
    "0",
    "-1",
    "1e400",
    "0x7f",
    "$HOME",
    "$CWD",
    "$TEMP",
    "..",
    "/",
    "*",
    "**",
    "\u{0}",
    "é",
    "[package]",
    "[permissions]",
    "[dependencies]",
    "[limits]",
    "[compile]",
    "name",
    "version",
    "read",
    "write",
    "run",
    "net",
    "env",
    "path",
    "registry",
    "sha384",
];

/// The dictionary: the fixed pieces and the most frequent words and punctuation runs of the
/// seeds.
fn dictionary(seeds: &[Vec<u8>], fixed: &[&str]) -> Vec<Vec<u8>> {
    let mut counts: HashMap<&[u8], usize> = HashMap::new();
    for seed in seeds {
        let mut i = 0;
        while i < seed.len() {
            let word = seed[i].is_ascii_alphanumeric() || seed[i] == b'_';
            let punct = seed[i].is_ascii_punctuation();
            let start = i;
            while i < seed.len()
                && i - start < 24
                && ((word && (seed[i].is_ascii_alphanumeric() || seed[i] == b'_'))
                    || (punct && seed[i].is_ascii_punctuation()))
            {
                i += 1;
            }
            if i == start {
                i += 1;
                continue;
            }
            *counts.entry(&seed[start..i]).or_default() += 1;
        }
    }
    let mut frequent: Vec<(&[u8], usize)> = counts.into_iter().collect();
    frequent.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut words: Vec<Vec<u8>> = fixed.iter().map(|p| p.as_bytes().to_vec()).collect();
    words.extend(frequent.into_iter().take(600).map(|(w, _)| w.to_vec()));
    words
}

fn line_starts(data: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        data.iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1),
    );
    starts
}

fn random_line(rng: &mut Rng, data: &[u8]) -> (usize, usize) {
    let starts = line_starts(data);
    let i = rng.below(starts.len());
    let start = starts[i].min(data.len());
    let end = starts.get(i + 1).copied().unwrap_or(data.len());
    (start, end)
}

struct Mutator {
    rng: Rng,
    words: Vec<Vec<u8>>,
    archive: bool,
}

impl Mutator {
    fn mutate(&mut self, base: &[u8], other: &[u8]) -> Vec<u8> {
        let mut data = base.to_vec();
        let rounds = 1 << self.rng.below(4);
        for _ in 0..rounds {
            self.once(&mut data, other);
            if data.len() > MAX_LEN {
                data.truncate(MAX_LEN);
            }
        }
        if self.archive {
            fix_tar_checksums(&mut data);
        }
        data
    }

    fn once(&mut self, data: &mut Vec<u8>, other: &[u8]) {
        let rng = &mut self.rng;
        let at = rng.below(data.len() + 1);
        match rng.below(14) {
            0 if !data.is_empty() => {
                let i = rng.below(data.len());
                data[i] ^= 1 << rng.below(8);
            }
            1 if !data.is_empty() => {
                let i = rng.below(data.len());
                data[i] = match rng.below(3) {
                    0 => rng.below(256) as u8,
                    1 => b" \n\t:()[]{}\"\\#.,=_0aZ"[rng.below(20)],
                    _ => 0x20 + rng.below(95) as u8,
                };
            }
            2 if !data.is_empty() => {
                let start = rng.below(data.len());
                let end = (start + 1 + rng.below(32)).min(data.len());
                data.drain(start..end);
            }
            3 if !data.is_empty() => {
                let start = rng.below(data.len());
                let end = (start + 1 + rng.below(64)).min(data.len());
                let piece = data[start..end].to_vec();
                let at = rng.below(data.len() + 1);
                data.splice(at..at, piece);
            }
            4 | 5 => {
                let word = self.words[rng.below(self.words.len())].clone();
                data.splice(at..at, word);
            }
            6 if !data.is_empty() => {
                // Replace the word or the run of punctuation at a position.
                let i = rng.below(data.len());
                let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
                let same = |b: u8| word(b) == word(data[i]) && !b.is_ascii_whitespace();
                let mut start = i;
                while start > 0 && same(data[start - 1]) {
                    start -= 1;
                }
                let mut end = i;
                while end < data.len() && same(data[end]) {
                    end += 1;
                }
                let replacement = self.words[rng.below(self.words.len())].clone();
                data.splice(start..end, replacement);
            }
            7 => {
                let (start, end) = random_line(rng, data);
                data.drain(start..end);
            }
            8 => {
                let (start, end) = random_line(rng, data);
                let line = data[start..end].to_vec();
                let (to, _) = random_line(rng, data);
                data.splice(to..to, line);
            }
            9 => {
                // Indentation: four spaces more or fewer at the start of a line.
                let (start, end) = random_line(rng, data);
                if rng.chance(2) {
                    data.splice(start..start, *b"    ");
                } else if data[start..end].starts_with(b"    ") {
                    data.drain(start..start + 4);
                }
            }
            10 if !other.is_empty() => {
                // Lines of another input at a line of this one.
                let (start, _) = random_line(rng, other);
                let (_, end) = random_line(rng, other);
                let piece = other[start.min(end)..end.max(start)].to_vec();
                let (to, _) = random_line(rng, data);
                data.splice(to..to, piece);
            }
            11 if !other.is_empty() => {
                let cut = rng.below(other.len());
                data.truncate(at);
                data.extend_from_slice(&other[cut..]);
            }
            12 if !data.is_empty() => {
                // Nesting: a range wrapped in many delimiters.
                let start = rng.below(data.len());
                let end = (start + rng.below(16)).min(data.len());
                let (open, close) = [(b'(', b')'), (b'[', b']'), (b'{', b'}')][rng.below(3)];
                let deep = rng.chance(8);
                let depth = 1 + rng.below(if deep { 4000 } else { 40 });
                data.splice(end..end, std::iter::repeat_n(close, depth));
                data.splice(start..start, std::iter::repeat_n(open, depth));
            }
            _ if !data.is_empty() => {
                // A byte repeated many times.
                let b = data[rng.below(data.len())];
                let long = rng.chance(8);
                let n = 1 + rng.below(if long { 8000 } else { 64 });
                data.splice(at..at, std::iter::repeat_n(b, n));
            }
            _ => data.extend_from_slice(&self.words[rng.below(self.words.len())]),
        }
    }
}

/// Sets the checksum of every block that looks like a ustar header, so that mutations reach the
/// checks behind the checksum.
fn fix_tar_checksums(data: &mut [u8]) {
    let mut at = 0;
    while at + 512 <= data.len() {
        if &data[at + 257..at + 262] == b"ustar" {
            let block = &mut data[at..at + 512];
            block[148..156].copy_from_slice(b"        ");
            let sum: u64 = block.iter().map(|b| u64::from(*b)).sum();
            let text = format!("{sum:06o}\0 ");
            block[148..156].copy_from_slice(text.as_bytes());
        }
        at += 512;
    }
}

// ---------------------------------------------------------------- seeds

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn walk(dir: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !matches!(name.as_ref(), "target" | "node_modules" | ".git") {
                walk(&path, keep, out);
            }
        } else if keep(&path) {
            out.push(path);
        }
    }
}

fn seed_inputs(kind: targets::Seeds) -> Vec<Vec<u8>> {
    let root = repo_root();
    let mut paths = Vec::new();
    match kind {
        targets::Seeds::Sources => walk(
            &root,
            &|p| p.extension().is_some_and(|e| e == "unio"),
            &mut paths,
        ),
        targets::Seeds::Manifests => walk(
            &root,
            &|p| {
                p.extension().is_some_and(|e| e == "toml" || e == "lock")
                    || p.file_name().is_some_and(|n| n == "RELEASE" || n == "KEYS")
            },
            &mut paths,
        ),
        targets::Seeds::Archives => {}
    }
    paths.sort();
    let mut seeds: Vec<Vec<u8>> = paths
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter(|d| d.len() <= MAX_LEN)
        .collect();
    if kind == targets::Seeds::Archives {
        seeds.extend(archive_seeds());
    }
    if seeds.is_empty() {
        seeds.push(Vec::new());
    }
    seeds
}

/// Small valid archives of packages, made by the writer of the compiler.
fn archive_seeds() -> Vec<Vec<u8>> {
    use unio::package::archive::{write, Entries, Item};
    let mut out = Vec::new();
    let manifest = b"[package]\nname = \"demo\"\nversion = \"1.0.0\"\n".to_vec();
    let mut entries = Entries::new();
    entries.insert("unio.toml".into(), Item::File(manifest.clone()));
    out.push(write(&entries));
    entries.insert("src".into(), Item::Directory);
    entries.insert(
        "src/lib.unio".into(),
        Item::File(b"public function one() returns Int:\n    return 1\n".to_vec()),
    );
    out.push(write(&entries));
    let mut nested = Entries::new();
    nested.insert("demo".into(), Item::Directory);
    nested.insert("demo/unio.toml".into(), Item::File(manifest));
    nested.insert("demo/a".into(), Item::Directory);
    nested.insert("demo/a/b".into(), Item::Directory);
    nested.insert("demo/a/b/c.unio".into(), Item::File(Vec::new()));
    out.push(write(&nested));
    out
}

// ---------------------------------------------------------------- arguments

struct Args {
    command: String,
    options: HashMap<String, String>,
    files: Vec<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut all = std::env::args().skip(1);
    let command = all
        .next()
        .ok_or("usage: fuzz run|exec|minimize|worker ...")?;
    let mut options = HashMap::new();
    let mut files = Vec::new();
    while let Some(arg) = all.next() {
        if let Some(name) = arg.strip_prefix("--") {
            let value = all.next().ok_or(format!("--{name} needs a value"))?;
            options.insert(name.to_string(), value);
        } else {
            files.push(PathBuf::from(arg));
        }
    }
    Ok(Args {
        command,
        options,
        files,
    })
}

impl Args {
    fn get(&self, name: &str, default: &str) -> String {
        self.options
            .get(name)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }
    fn number(&self, name: &str, default: u64) -> Result<u64, String> {
        match self.options.get(name) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("--{name}: not a number: {v}")),
        }
    }
    fn target(&self) -> Result<String, String> {
        let target = self.get("target", "");
        if targets::TARGETS.contains(&target.as_str()) {
            Ok(target)
        } else {
            Err(format!(
                "--target must be one of {}",
                targets::TARGETS.join(", ")
            ))
        }
    }
}

fn main() -> ExitCode {
    let result = args().and_then(|args| match args.command.as_str() {
        "run" => supervise(&args),
        "worker" => worker(&args).map(|()| true),
        "exec" => exec(&args),
        "minimize" => minimize(&args).map(|()| true),
        other => Err(format!("unknown command {other}")),
    });
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("fuzz: {message}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------- exec and minimize

/// Runs each file once in this process; false when one panicked. An abort ends the process.
fn exec(args: &Args) -> Result<bool, String> {
    let target = args.target()?;
    let scratch = std::env::temp_dir().join(format!("unio-fuzz-exec-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    quiet_panics();
    let mut clean = true;
    for file in &args.files {
        let data = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
        if let Err(panic) = run_one(&target, &data, &scratch) {
            eprintln!("fuzz: {}: {panic}", file.display());
            clean = false;
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(clean)
}

/// What a failing run of `exec` shows: the place of the panic, the overflow of the stack, or
/// the exit status.
fn signature(target: &str, data: &[u8], scratch: &Path) -> Option<String> {
    let file = scratch.join("candidate");
    std::fs::write(&file, data).ok()?;
    let output = Command::new(std::env::current_exe().ok()?)
        .args(["exec", "--target", target])
        .arg(&file)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    if output.status.success() {
        return None;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(at) = stderr.find("panicked at ") {
        let place = stderr[at + 12..].split(':').take(2).collect::<Vec<_>>();
        return Some(format!("panic {}", place.join(":")));
    }
    if stderr.contains("overflowed its stack") {
        return Some("stack overflow".to_string());
    }
    Some(format!("exit {:?}", output.status.code()))
}

/// Reduces a failing input, first by lines and then by bytes, keeping the failure the same.
fn minimize(args: &Args) -> Result<(), String> {
    let target = args.target()?;
    let input = args.files.first().ok_or("minimize needs a file")?;
    let output = args
        .options
        .get("output")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let mut name = input.as_os_str().to_owned();
            name.push(".min");
            PathBuf::from(name)
        });
    let scratch = std::env::temp_dir().join(format!("unio-fuzz-min-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let mut data = std::fs::read(input).map_err(|e| e.to_string())?;
    let wanted = signature(&target, &data, &scratch).ok_or("the input does not fail")?;
    eprintln!(
        "fuzz: reducing {} bytes that fail with {wanted}",
        data.len()
    );
    // Lines, then bytes: remove chunks of halving size while the failure stays.
    for by_lines in [true, false] {
        let units = |d: &[u8]| -> Vec<(usize, usize)> {
            if by_lines {
                let starts = line_starts(d);
                starts
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (*s, starts.get(i + 1).copied().unwrap_or(d.len())))
                    .filter(|(s, e)| s < e)
                    .collect()
            } else {
                (0..d.len()).map(|i| (i, i + 1)).collect()
            }
        };
        let mut chunk = units(&data).len().max(1);
        while chunk >= 1 {
            let mut i = 0;
            loop {
                let list = units(&data);
                if i >= list.len() {
                    break;
                }
                let end = (i + chunk).min(list.len());
                let (from, to) = (list[i].0, list[end - 1].1);
                let mut candidate = data[..from].to_vec();
                candidate.extend_from_slice(&data[to..]);
                if signature(&target, &candidate, &scratch).as_deref() == Some(&wanted) {
                    data = candidate;
                } else {
                    i += chunk;
                }
            }
            if chunk == 1 {
                break;
            }
            chunk /= 2;
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::write(&output, &data).map_err(|e| e.to_string())?;
    eprintln!(
        "fuzz: {} bytes, written to {}",
        data.len(),
        output.display()
    );
    Ok(())
}

// ---------------------------------------------------------------- the worker

struct Corpus {
    dir: PathBuf,
    names: HashSet<String>,
    inputs: Vec<Vec<u8>>,
}

impl Corpus {
    fn add(&mut self, data: Vec<u8>) {
        let name = fnv(&data);
        if self.names.insert(name.clone()) {
            let _ = std::fs::write(self.dir.join(&name), &data);
            self.inputs.push(data);
        }
    }

    /// Files the other workers added since the last look.
    fn unseen(&mut self) -> Vec<Vec<u8>> {
        let mut found = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !self.names.contains(&name) {
                    if let Ok(data) = std::fs::read(entry.path()) {
                        self.names.insert(name);
                        found.push(data);
                    }
                }
            }
        }
        found
    }
}

/// What a worker keeps while it runs inputs.
struct Session {
    target: String,
    id: String,
    scratch: PathBuf,
    /// The input being run, written before it runs, so that the supervisor can record it when
    /// the worker dies.
    current: PathBuf,
    crashes: PathBuf,
    execs: u64,
    findings: usize,
    /// Panics recorded per place, at most three inputs each.
    places: HashMap<String, usize>,
    /// A fault asked for by `UNIO_FUZZ_FAULT`, for the tests of the supervisor.
    fault: Option<String>,
}

impl Session {
    /// Runs the input; true when it reached new coverage, false also when it panicked (the
    /// panic is recorded).
    fn execute(&mut self, data: &[u8], coverage: &mut Coverage) -> bool {
        let _ = std::fs::write(&self.current, data);
        if self.fault.as_deref() == Some("abort-in-input") {
            eprintln!("fuzz[{}]: the fault abort-in-input", self.id);
            std::process::abort();
        }
        coverage.clear();
        let result = run_one(&self.target, data, &self.scratch);
        self.execs += 1;
        let Err(panic) = result else {
            return coverage.merge();
        };
        let place = panic.split(": ").next().unwrap_or("panicked").to_string();
        let seen = self.places.entry(place).or_default();
        *seen += 1;
        if *seen <= 3 {
            let target = &self.target;
            let name = format!("{target}-panic-{}", fnv(data));
            let _ = std::fs::write(self.crashes.join(&name), data);
            let _ = std::fs::write(
                self.crashes.join(format!("{name}.txt")),
                format!("target {target}\n{panic}\n"),
            );
            self.findings += 1;
            eprintln!("fuzz[{}]: {target}: {panic} (crashes/{name})", self.id);
        }
        false
    }

    /// The counters for the supervisor (`supervisor.rs`), `NAME VALUE` per line.
    fn stats(&self, corpus: usize, coverage: &Coverage) -> String {
        let mut text = format!(
            "target {}\nexecs {}\ncorpus {corpus}\nedges {}\ncounters {}\nfindings {}\n",
            self.target,
            self.execs,
            coverage.edges,
            coverage.total(),
            self.findings
        );
        if let Some(ms) = cpu_ms() {
            text.push_str(&format!("cpu_ms {ms}\n"));
        }
        text
    }
}

/// Writes the counters whole, so that the supervisor never reads half of them.
fn write_stats(path: &Path, text: &str) {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".new");
    if std::fs::write(&partial, text).is_ok() {
        let _ = std::fs::rename(&partial, path);
    }
}

/// The processor time of this process, threads that ended included, in milliseconds: user and
/// system time from `/proc/self/stat`, in the clock ticks of 1/100 second that Linux reports
/// there on every architecture. None elsewhere.
fn cpu_ms() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Fields 14 and 15 of stat(5); the rest begins with field 3.
    let user: u64 = fields.get(11)?.parse().ok()?;
    let system: u64 = fields.get(12)?.parse().ok()?;
    Some((user + system) * 10)
}

/// The fault `UNIO_FUZZ_FAULT=KIND:TARGET` asks the worker of TARGET for: `abort-before-input`
/// (the worker aborts before it writes any input), `abort-in-input` (it aborts on its first
/// input, after writing it), or `idle` (it ends at once with success, running nothing). The
/// supervisor must fail the run in each case; the tests of the harness use it.
fn fault(target: &str) -> Option<String> {
    let value = std::env::var("UNIO_FUZZ_FAULT").ok()?;
    let (kind, of) = value.split_once(':')?;
    (of == target).then(|| kind.to_string())
}

fn worker(args: &Args) -> Result<(), String> {
    let target = args.target()?;
    let work = PathBuf::from(args.get("work", "fuzz-work"));
    let id = args.get("id", "0");
    let until = args.number("until", now_secs() + 60)?;
    let fault = fault(&target);
    match fault.as_deref() {
        Some("abort-before-input") => {
            eprintln!("fuzz[{id}]: the fault abort-before-input");
            std::process::abort();
        }
        Some("idle") => {
            eprintln!("fuzz[{id}]: the fault idle");
            return Ok(());
        }
        _ => {}
    }
    let scratch = work.join(format!("scratch-{id}"));
    let crashes = work.join("crashes");
    let corpus_dir = work.join("corpus").join(&target);
    for dir in [&scratch, &crashes, &corpus_dir] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let current = scratch.join("current");
    let stats = scratch.join("stats");
    quiet_panics();
    let kind = targets::seeds(&target);
    let seeds = seed_inputs(kind);
    let fixed: &[&str] = if kind == targets::Seeds::Sources {
        PIECES
    } else {
        TOML_PIECES
    };
    let mut mutator = Mutator {
        rng: Rng::new(now_secs() ^ (u64::from(std::process::id()) << 32)),
        words: dictionary(&seeds, fixed),
        archive: kind == targets::Seeds::Archives,
    };
    let mut coverage = Coverage::new();
    let mut corpus = Corpus {
        dir: corpus_dir,
        names: HashSet::new(),
        inputs: Vec::new(),
    };
    let mut session = Session {
        target: target.clone(),
        id: id.clone(),
        scratch,
        current,
        crashes,
        execs: 0,
        findings: 0,
        places: HashMap::new(),
        fault,
    };
    // The corpus of earlier runs and of the other workers, then the seeds.
    let mut start: Vec<Vec<u8>> = corpus.unseen();
    start.extend(seeds);
    let mut last_stats = Instant::now();
    for data in start {
        if last_stats.elapsed() > Duration::from_secs(5) {
            last_stats = Instant::now();
            write_stats(&stats, &session.stats(corpus.inputs.len(), &coverage));
        }
        // Without instrumentation every seed is kept: there is no coverage to choose by.
        if session.execute(&data, &mut coverage)
            || coverage.total() == 0
            || corpus.inputs.is_empty()
        {
            corpus.add(data);
        } else {
            corpus.names.insert(fnv(&data));
        }
    }
    eprintln!(
        "fuzz[{id}]: {target}: {} inputs in the corpus, {} of {} counters reached",
        corpus.inputs.len(),
        coverage.edges,
        coverage.total()
    );
    if coverage.total() == 0 {
        eprintln!("fuzz[{id}]: no coverage instrumentation; build with fuzz/build.sh");
    }
    let mut last_sync = Instant::now();
    while now_secs() < until {
        let n = corpus.inputs.len();
        let (a, b) = (mutator.rng.below(n), mutator.rng.below(n));
        // Of two random entries the shorter, so that small inputs are mutated more often.
        let base = if corpus.inputs[a].len() <= corpus.inputs[b].len() {
            a
        } else {
            b
        };
        let other = mutator.rng.below(n);
        let data = mutator.mutate(&corpus.inputs[base], &corpus.inputs[other]);
        if session.execute(&data, &mut coverage) {
            corpus.add(data);
        }
        if last_sync.elapsed() > Duration::from_secs(30) {
            last_sync = Instant::now();
            for data in corpus.unseen() {
                if session.execute(&data, &mut coverage) {
                    corpus.inputs.push(data);
                }
            }
        }
        if last_stats.elapsed() > Duration::from_secs(5) {
            last_stats = Instant::now();
            write_stats(&stats, &session.stats(corpus.inputs.len(), &coverage));
        }
    }
    write_stats(&stats, &session.stats(corpus.inputs.len(), &coverage));
    Ok(())
}

// ---------------------------------------------------------------- the supervisor

fn supervise(args: &Args) -> Result<bool, String> {
    let list = args.get("targets", &targets::TARGETS.join(","));
    let chosen: Vec<String> = list.split(',').map(str::to_string).collect();
    for target in &chosen {
        if !targets::TARGETS.contains(&target.as_str()) {
            return Err(format!("no target {target}"));
        }
    }
    let jobs = args.number("jobs", chosen.len() as u64)? as usize;
    let minutes = args.number("minutes", 10)?;
    let seconds = args.number("seconds", minutes * 60)?;
    let min_share = match args.options.get("min-share") {
        None => 0.95,
        Some(v) => v
            .parse::<f64>()
            .map_err(|_| format!("--min-share: not a number: {v}"))?,
    };
    let memory_mb = args.number("memory-mb", 4096)?;
    let config = supervisor::Config {
        targets: chosen,
        jobs,
        seconds,
        timeout: Duration::from_secs(args.number("timeout", 60)?),
        min_share,
        work: PathBuf::from(args.get("work", "fuzz-work")),
        command: std::env::args().collect(),
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    supervisor::run(&config, &|spec| worker_command(&exe, spec, memory_mb))
}

/// The command of a worker process: this program with `worker`, under a limit of the address
/// space where the shell can set one, so that an input that makes the compiler allocate
/// without bound ends as a finding, not as the end of the runner.
fn worker_command(exe: &Path, spec: &supervisor::Spec, memory_mb: u64) -> Command {
    let worker_args = [
        "worker".to_string(),
        "--target".to_string(),
        spec.target.to_string(),
        "--work".to_string(),
        spec.work.display().to_string(),
        "--id".to_string(),
        spec.id.to_string(),
        "--until".to_string(),
        spec.until.to_string(),
    ];
    if cfg!(unix) && memory_mb > 0 {
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(format!(
                "ulimit -v {} && exec \"$0\" \"$@\"",
                memory_mb * 1024
            ))
            .arg(exe)
            .args(&worker_args);
        c
    } else {
        let mut c = Command::new(exe);
        c.args(&worker_args);
        c
    }
}
