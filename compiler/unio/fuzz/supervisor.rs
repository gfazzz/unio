//! The supervisor of a fuzzing run (R-1): it starts one worker process per slot, at least one
//! for every target, watches them, records what they leave when they die, and accounts for the
//! time and the executions of every target, so that the run can serve as evidence.
//!
//! A run fails (the result is `Ok(false)`) when any of these holds:
//!
//! - a worker recorded a finding (a panic) or left the input it died on (a crash, a hang);
//! - a worker died and left no input to record (it ended before it wrote `scratch-I/current`):
//!   the death is itself a finding, kept with the last output of the worker in
//!   `crashes/TARGET-crash-noinput-...txt`;
//! - a slot ran less than `min_share` of the planned time, or ran no input at all.
//!
//! Fewer slots than targets, or a target listed twice, is an error of the caller (`Err`).
//!
//! The protocol with a worker: it is started with its slot, target, work directory and the end
//! of the run (seconds since the epoch, `Spec`); it writes the input it runs to
//! `WORK/scratch-I/current` before running it and its counters to `WORK/scratch-I/stats`, one
//! `NAME VALUE` per line (`execs`, `corpus`, `edges`, `counters`, `findings`, `cpu_ms`). The
//! supervisor keeps the totals of every slot across restarts in `WORK/stats-I` and writes
//! `WORK/summary.json` at the end.
//!
//! Shared by the fuzzer (`main.rs`) and the tests of the supervisor (`tests/fuzz.rs`), which
//! start workers that misbehave on purpose; it uses only the standard library.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What a run is asked to do.
pub struct Config {
    /// The targets, each fuzzed by at least one slot.
    pub targets: Vec<String>,
    /// The number of worker processes; at least the number of targets.
    pub jobs: usize,
    /// The planned length of the run.
    pub seconds: u64,
    /// An input with no result within this time is a hang.
    pub timeout: Duration,
    /// The share of the planned time every slot must have run, from 0 to 1.
    pub min_share: f64,
    pub work: PathBuf,
    /// The command line of the run, for the summary.
    pub command: Vec<String>,
}

/// One worker process to start.
pub struct Spec<'a> {
    pub id: usize,
    pub target: &'a str,
    pub work: &'a Path,
    /// The end of the run, in seconds since the epoch.
    pub until: u64,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn now_exact() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

pub fn fnv(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

// ---------------------------------------------------------------- the counters of a worker

/// The counters a worker wrote, `NAME VALUE` per line; unknown names are ignored.
#[derive(Default, Clone)]
struct Counters {
    execs: u64,
    corpus: u64,
    edges: u64,
    counters: u64,
    findings: u64,
    cpu_ms: Option<u64>,
}

fn read_counters(path: &Path) -> Counters {
    let mut c = Counters::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return c;
    };
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match name {
            "execs" => c.execs = value,
            "corpus" => c.corpus = value,
            "edges" => c.edges = value,
            "counters" => c.counters = value,
            "findings" => c.findings = value,
            "cpu_ms" => c.cpu_ms = Some(value),
            _ => {}
        }
    }
    c
}

/// The totals of one slot over all the processes that ran in it.
struct Slot {
    id: usize,
    target: String,
    wall: Duration,
    execs: u64,
    /// None when a process of the slot could not measure its processor time.
    cpu_ms: Option<u64>,
    corpus: u64,
    edges: u64,
    counters: u64,
    findings: u64,
    starts: u64,
    deaths: u64,
    hangs: u64,
    /// Ended cleanly before the end of the run.
    early: u64,
}

impl Slot {
    /// Adds the counters of a process of the slot that ended after running for `wall`.
    fn absorb(&mut self, c: &Counters, wall: Duration) {
        self.wall += wall;
        self.execs += c.execs;
        self.cpu_ms = match (self.cpu_ms, c.cpu_ms) {
            (Some(a), Some(b)) => Some(a + b),
            // A process that ran nothing and measured nothing does not make the total unknown.
            (Some(a), None) if c.execs == 0 => Some(a),
            _ => None,
        };
        self.corpus = self.corpus.max(c.corpus);
        self.edges = self.edges.max(c.edges);
        self.counters = self.counters.max(c.counters);
        self.findings += c.findings;
    }

    /// The totals with the counters of the process that is still running, if any.
    fn with_live(&self, live: Option<(&Counters, Duration)>) -> Slot {
        let mut s = Slot {
            id: self.id,
            target: self.target.clone(),
            wall: self.wall,
            execs: self.execs,
            cpu_ms: self.cpu_ms,
            corpus: self.corpus,
            edges: self.edges,
            counters: self.counters,
            findings: self.findings,
            starts: self.starts,
            deaths: self.deaths,
            hangs: self.hangs,
            early: self.early,
        };
        if let Some((c, wall)) = live {
            s.absorb(c, wall);
        }
        s
    }

    fn line(&self) -> String {
        let cpu = self.cpu_ms.map_or("unknown".to_string(), |ms| {
            format!("{:.0}", ms as f64 / 1000.0)
        });
        format!(
            "{} execs {} corpus {} edges {} of {} findings {} wall {:.0}s cpu {cpu}s starts {} deaths {} hangs {}\n",
            self.target,
            self.execs,
            self.corpus,
            self.edges,
            self.counters,
            self.findings,
            self.wall.as_secs_f64(),
            self.starts,
            self.deaths,
            self.hangs
        )
    }
}

// ---------------------------------------------------------------- the processes

struct Running {
    child: Child,
    id: usize,
    target: String,
    started: Instant,
    stderr: Arc<Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    fn tail(&mut self) -> String {
        // The reader ends when the pipe closes, which is when the process has ended (or, after
        // two seconds, when a process it started still holds the pipe, the tail read so far).
        if let Some(reader) = self.reader.take() {
            let waited = Instant::now();
            while !reader.is_finished() && waited.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(20));
            }
            if reader.is_finished() {
                let _ = reader.join();
            }
        }
        self.stderr
            .lock()
            .map(|t| String::from_utf8_lossy(&t).to_string())
            .unwrap_or_default()
    }
}

fn spawn(mut command: Command, id: usize, target: &str) -> Result<Running, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting the worker of {target}: {e}"))?;
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let mut pipe = child.stderr.take().expect("piped");
    let keep = Arc::clone(&stderr);
    let reader = std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut buffer) {
            if n == 0 {
                break;
            }
            // Echo the output of the worker and keep its tail for the record of a crash.
            eprint!("{}", String::from_utf8_lossy(&buffer[..n]));
            let mut kept = keep.lock().unwrap_or_else(|e| e.into_inner());
            kept.extend_from_slice(&buffer[..n]);
            let excess = kept.len().saturating_sub(8192);
            kept.drain(..excess);
        }
    });
    Ok(Running {
        child,
        id,
        target: target.to_string(),
        started: Instant::now(),
        stderr,
        reader: Some(reader),
    })
}

fn scratch(work: &Path, id: usize) -> PathBuf {
    work.join(format!("scratch-{id}"))
}

/// Records the input the worker died on; with no input, the death is recorded with the output
/// of the worker and returned as a failure.
fn record(work: &Path, worker: &mut Running, kind: &str, reason: &str, failures: &mut Vec<String>) {
    let current = scratch(work, worker.id).join("current");
    let crashes = work.join("crashes");
    let _ = std::fs::create_dir_all(&crashes);
    let tail = worker.tail();
    let target = worker.target.clone();
    match std::fs::read(&current) {
        Ok(data) => {
            let name = format!("{target}-{kind}-{}", fnv(&data));
            let saved = std::fs::write(crashes.join(&name), &data).is_ok()
                && std::fs::write(
                    crashes.join(format!("{name}.txt")),
                    format!("target {target}\n{reason}\n\nlast output:\n{tail}\n"),
                )
                .is_ok();
            eprintln!("fuzz: {reason}: input in crashes/{name}");
            let _ = std::fs::remove_file(&current);
            if !saved {
                failures.push(format!(
                    "worker {} ({target}): {reason}; crashes/{name} could not be written",
                    worker.id
                ));
            }
        }
        Err(_) => {
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            let name = format!("{target}-{kind}-noinput-w{}-{millis}.txt", worker.id);
            let _ = std::fs::write(
                crashes.join(&name),
                format!(
                    "target {target}\n{reason}, before it wrote an input to scratch-{}/current\n\nlast output:\n{tail}\n",
                    worker.id
                ),
            );
            let message = format!(
                "worker {} ({target}): {reason}, with no input to record; output in crashes/{name}",
                worker.id
            );
            eprintln!("fuzz: {message}");
            failures.push(message);
        }
    }
}

fn findings(work: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(work.join("crashes"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| !n.ends_with(".txt"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn describe(status: ExitStatus) -> String {
    format!("worker ended: {status}")
}

// ---------------------------------------------------------------- the run

/// Runs the workers that `worker` makes until the end of the run; true when the run found
/// nothing and every slot ran for its share of the time.
pub fn run(config: &Config, worker: &dyn Fn(&Spec) -> Command) -> Result<bool, String> {
    let mut seen = HashSet::new();
    for target in &config.targets {
        if !seen.insert(target) {
            return Err(format!("the target {target} is listed twice"));
        }
    }
    if config.targets.is_empty() {
        return Err("no targets".to_string());
    }
    if config.jobs < config.targets.len() {
        return Err(format!(
            "--jobs {} is fewer than the {} targets; every target needs a worker",
            config.jobs,
            config.targets.len()
        ));
    }
    if !(0.0..=1.0).contains(&config.min_share) {
        return Err("--min-share must be between 0 and 1".to_string());
    }
    std::fs::create_dir_all(&config.work).map_err(|e| e.to_string())?;
    let work = std::fs::canonicalize(&config.work).map_err(|e| e.to_string())?;
    let work = work.as_path();
    let before: HashSet<String> = findings(work).into_iter().collect();
    let started = now_exact();
    let until = now_secs() + config.seconds;
    let planned = (until as f64 - started).max(0.0);
    let required = planned * config.min_share;
    eprintln!(
        "fuzz: {} workers on {} for {:.0} seconds, work directory {}",
        config.jobs,
        config.targets.join(", "),
        planned,
        work.display()
    );
    let mut slots: Vec<Slot> = (0..config.jobs)
        .map(|id| Slot {
            id,
            target: config.targets[id % config.targets.len()].clone(),
            wall: Duration::ZERO,
            execs: 0,
            cpu_ms: Some(0),
            corpus: 0,
            edges: 0,
            counters: 0,
            findings: 0,
            starts: 0,
            deaths: 0,
            hangs: 0,
            early: 0,
        })
        .collect();
    let start = |slot: &mut Slot| -> Result<Running, String> {
        let dir = scratch(work, slot.id);
        let _ = std::fs::remove_file(dir.join("current"));
        let _ = std::fs::remove_file(dir.join("stats"));
        slot.starts += 1;
        spawn(
            worker(&Spec {
                id: slot.id,
                target: &slot.target,
                work,
                until,
            }),
            slot.id,
            &slot.target,
        )
    };
    let mut running: Vec<Running> = Vec::new();
    for slot in &mut slots {
        running.push(start(slot)?);
    }
    let mut failures: Vec<String> = Vec::new();
    let mut last_report = Instant::now();
    let mut restarts = 0usize;
    while !running.is_empty() {
        std::thread::sleep(Duration::from_millis(500));
        let mut i = 0;
        while i < running.len() {
            let id = running[i].id;
            let current = scratch(work, id).join("current");
            let ended = match running[i].child.try_wait() {
                Ok(Some(status)) => Some((status, false)),
                Ok(None) => {
                    let stuck = std::fs::metadata(&current)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > config.timeout);
                    // A worker still running two minutes after the end is stuck as well.
                    if stuck || now_secs() > until + 120 {
                        let _ = running[i].child.kill();
                        let status = running[i].child.wait().map_err(|e| e.to_string())?;
                        Some((status, true))
                    } else {
                        None
                    }
                }
                Err(e) => return Err(e.to_string()),
            };
            let Some((status, hang)) = ended else {
                i += 1;
                continue;
            };
            let mut ended = running.remove(i);
            let wall = ended.started.elapsed();
            let counters = read_counters(&scratch(work, id).join("stats"));
            let slot = &mut slots[id];
            slot.absorb(&counters, wall);
            let restart = if hang {
                slot.hangs += 1;
                record(
                    work,
                    &mut ended,
                    "hang",
                    &format!("no result within {} seconds", config.timeout.as_secs()),
                    &mut failures,
                );
                true
            } else if !status.success() {
                slot.deaths += 1;
                record(work, &mut ended, "crash", &describe(status), &mut failures);
                true
            } else {
                if now_secs() < until {
                    slot.early += 1;
                }
                false
            };
            if restart && now_secs() + 5 < until {
                if restarts < 1000 {
                    restarts += 1;
                    running.push(start(slot)?);
                } else {
                    failures.push(format!(
                        "worker {id} ({}): not started again, the run reached 1000 restarts",
                        slot.target
                    ));
                }
            }
        }
        if last_report.elapsed() > Duration::from_secs(60) {
            last_report = Instant::now();
            report(work, &slots, &running);
        }
    }
    report(work, &slots, &running);
    let ended = now_exact();
    for slot in &slots {
        if slot.execs == 0 {
            failures.push(format!(
                "worker {} ({}): ran no input",
                slot.id, slot.target
            ));
        }
        if slot.wall.as_secs_f64() < required {
            failures.push(format!(
                "worker {} ({}): ran {:.1} of the {:.1} seconds required ({:.0}% of {:.1}){}",
                slot.id,
                slot.target,
                slot.wall.as_secs_f64(),
                required,
                config.min_share * 100.0,
                planned,
                if slot.early > 0 {
                    "; it ended before the end of the run"
                } else {
                    ""
                }
            ));
        }
    }
    let new: Vec<String> = findings(work)
        .into_iter()
        .filter(|n| !before.contains(n))
        .collect();
    let ok = new.is_empty() && failures.is_empty();
    let summary = summary(
        config,
        &slots,
        &new,
        &failures,
        (started, ended, planned, required),
        ok,
    );
    std::fs::write(work.join("summary.json"), summary).map_err(|e| format!("summary.json: {e}"))?;
    if !new.is_empty() {
        eprintln!("fuzz: {} findings:", new.len());
        for name in &new {
            eprintln!("  crashes/{name}");
        }
    }
    if !failures.is_empty() {
        eprintln!("fuzz: {} failures of the run:", failures.len());
        for failure in &failures {
            eprintln!("  {failure}");
        }
    }
    if ok {
        eprintln!("fuzz: no findings; every target ran for its time");
    }
    Ok(ok)
}

/// Prints the totals of every slot and keeps them in `WORK/stats-I`.
fn report(work: &Path, slots: &[Slot], running: &[Running]) {
    for slot in slots {
        let live = running.iter().find(|r| r.id == slot.id).map(|r| {
            (
                read_counters(&scratch(work, slot.id).join("stats")),
                r.started.elapsed(),
            )
        });
        let totals = slot.with_live(live.as_ref().map(|(c, w)| (c, *w)));
        let line = totals.line();
        eprint!("fuzz[{}]: {line}", slot.id);
        let _ = std::fs::write(work.join(format!("stats-{}", slot.id)), line);
    }
}

// ---------------------------------------------------------------- the summary

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| json_string(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// A time in seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
fn iso(secs: f64) -> String {
    let secs = secs as i64;
    let (days, rest) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // The civil date of a day number (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

fn summary(
    config: &Config,
    slots: &[Slot],
    new: &[String],
    failures: &[String],
    (started, ended, planned, required): (f64, f64, f64, f64),
    ok: bool,
) -> String {
    let mut targets = BTreeMap::new();
    for target in &config.targets {
        let mine: Vec<&Slot> = slots.iter().filter(|s| &s.target == target).collect();
        let sum = |f: &dyn Fn(&Slot) -> u64| mine.iter().map(|s| f(s)).sum::<u64>();
        let max = |f: &dyn Fn(&Slot) -> u64| mine.iter().map(|s| f(s)).max().unwrap_or(0);
        let wall = mine
            .iter()
            .map(|s| s.wall.as_secs_f64())
            .fold(0.0, f64::max);
        let worker_seconds: f64 = mine.iter().map(|s| s.wall.as_secs_f64()).sum();
        let cpu = mine
            .iter()
            .map(|s| s.cpu_ms)
            .try_fold(0u64, |a, b| b.map(|b| a + b))
            .map_or("null".to_string(), |ms| {
                format!("{:.3}", ms as f64 / 1000.0)
            });
        let findings: Vec<String> = new
            .iter()
            .filter(|n| n.starts_with(&format!("{target}-")))
            .cloned()
            .collect();
        let problems: Vec<String> = failures
            .iter()
            .filter(|f| f.contains(&format!("({target})")))
            .cloned()
            .collect();
        let workers: Vec<String> = mine.iter().map(|s| s.id.to_string()).collect();
        let target_ok = findings.is_empty() && problems.is_empty() && sum(&|s| s.findings) == 0;
        targets.insert(
            target.clone(),
            format!(
                concat!(
                    "{{\"ok\": {}, \"workers\": [{}], \"wall_seconds\": {:.3}, ",
                    "\"worker_seconds\": {:.3}, \"required_seconds\": {:.3}, ",
                    "\"cpu_seconds\": {}, \"execs\": {}, \"corpus\": {}, \"edges\": {}, ",
                    "\"counters\": {}, \"starts\": {}, \"deaths\": {}, \"hangs\": {}, ",
                    "\"findings\": {}, \"problems\": {}}}"
                ),
                target_ok,
                workers.join(", "),
                wall,
                worker_seconds,
                required,
                cpu,
                sum(&|s| s.execs),
                max(&|s| s.corpus),
                max(&|s| s.edges),
                max(&|s| s.counters),
                sum(&|s| s.starts),
                sum(&|s| s.deaths),
                sum(&|s| s.hangs),
                json_list(&findings),
                json_list(&problems),
            ),
        );
    }
    let targets: Vec<String> = targets
        .iter()
        .map(|(name, body)| format!("    {}: {body}", json_string(name)))
        .collect();
    format!(
        concat!(
            "{{\n  \"format\": 1,\n  \"ok\": {},\n  \"command\": {},\n",
            "  \"started\": {},\n  \"ended\": {},\n  \"planned_seconds\": {:.3},\n",
            "  \"elapsed_seconds\": {:.3},\n  \"min_share\": {},\n  \"jobs\": {},\n",
            "  \"timeout_seconds\": {},\n  \"targets\": {{\n{}\n  }},\n",
            "  \"findings\": {},\n  \"failures\": {}\n}}\n"
        ),
        ok,
        json_list(&config.command),
        json_string(&iso(started)),
        json_string(&iso(ended)),
        planned,
        ended - started,
        config.min_share,
        config.jobs,
        config.timeout.as_secs(),
        targets.join(",\n"),
        json_list(new),
        json_list(failures),
    )
}
