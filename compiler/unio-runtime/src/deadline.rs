//! The time limits of one test of `unio test` (T-12, docs/tools.md): a test, and a generator or
//! predicate of `std.test.check_property` that it calls, ends as a failed test when its time is
//! used up, even when it never returns, and the next test still runs.
//!
//! A [`Deadline`] watches the test from a thread of its own. When the wall time or the processor
//! time of the test is used up, it raises a flag of the thread of the test that the interpreter
//! reads at every call and at every turn of a loop ([`time_up`]): the test unwinds with [`Exhaustion::Time`], as it does
//! when its heap is exhausted, and its harness reports which limit ended it. Sleeping
//! (`std.time.sleep_nanoseconds`) wakes up for the flag too. A test that does not reach such a
//! point within [`GRACE`] after its limit, because it waits in the system (a child process, a
//! blocked read), cannot be unwound: the watcher then stops the children of the program and
//! ends the process with code 102 and a message that names the test, so that `unio test` never
//! hangs.
//!
//! [`Exhaustion::Time`]: crate::fault::Exhaustion::Time

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

thread_local! {
    /// Raised while the test that runs on this thread has used up a time limit.
    static TIME_UP: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
}

/// How long a test may go on after its limit before the process ends (code 102).
pub const GRACE: Duration = Duration::from_secs(10);

/// Whether the test that runs has used up its time; the interpreter unwinds when it has.
#[inline]
pub fn time_up() -> bool {
    TIME_UP
        .try_with(|flag| flag.load(Ordering::Relaxed))
        .unwrap_or(false)
}

/// The limits of one test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub wall: Duration,
    pub cpu: Option<Duration>,
}

/// Which limit a test used up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expired {
    Wall,
    Cpu,
}

const NONE: u8 = 0;
const WALL: u8 = 1;
const CPU: u8 = 2;

/// The watcher of one test; dropping it ends the watch and lowers the flag.
pub struct Deadline {
    flag: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    expired: Arc<AtomicU8>,
    thread: Option<JoinHandle<()>>,
}

impl Deadline {
    /// Starts watching the test that runs on this thread, named `label` for the message of
    /// the last resort.
    pub fn start(limits: Limits, label: String) -> Deadline {
        let flag = TIME_UP.with(Arc::clone);
        flag.store(false, Ordering::SeqCst);
        let watch_flag = Arc::clone(&flag);
        let stop = Arc::new(AtomicBool::new(false));
        let expired = Arc::new(AtomicU8::new(NONE));
        let (watch_stop, watch_expired) = (Arc::clone(&stop), Arc::clone(&expired));
        let started = Instant::now();
        let cpu_start = crate::sandbox::process_cpu_time();
        let thread = std::thread::Builder::new()
            .name("unio-test-limits".into())
            .spawn(move || {
                let mut ended_at: Option<Instant> = None;
                loop {
                    if watch_stop.load(Ordering::SeqCst) {
                        return;
                    }
                    match ended_at {
                        None => {
                            let used = if started.elapsed() >= limits.wall {
                                WALL
                            } else if limits.cpu.is_some_and(|c| {
                                crate::sandbox::process_cpu_time().saturating_sub(cpu_start) >= c
                            }) {
                                CPU
                            } else {
                                NONE
                            };
                            if used != NONE {
                                watch_expired.store(used, Ordering::SeqCst);
                                watch_flag.store(true, Ordering::SeqCst);
                                ended_at = Some(Instant::now());
                            }
                        }
                        Some(at) if at.elapsed() >= GRACE => {
                            crate::externs::process::stop_children();
                            eprintln!(
                                "unio test: the test {label} used up its time limit and did not \
                                 end within {} s after it, waiting outside the program (a child \
                                 process or a read); unio test ends with code 102 (T-12)",
                                GRACE.as_secs()
                            );
                            std::process::exit(crate::fault::EXIT_EXHAUSTED);
                        }
                        Some(_) => {}
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(_) => {
                // Without its watcher a test could hang `unio test`: fail closed.
                eprintln!("unio test: the watcher of the time limits of a test cannot start");
                std::process::exit(crate::fault::EXIT_SANDBOX);
            }
        };
        Deadline {
            flag,
            stop,
            expired,
            thread,
        }
    }

    /// The limit the test used up, if it did.
    pub fn expired(&self) -> Option<Expired> {
        match self.expired.load(Ordering::SeqCst) {
            WALL => Some(Expired::Wall),
            CPU => Some(Expired::Cpu),
            _ => None,
        }
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.flag.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_rises_at_the_limit_and_falls_with_the_watch() {
        let deadline = Deadline::start(
            Limits {
                wall: Duration::from_millis(50),
                cpu: None,
            },
            "\"probe\"".into(),
        );
        let started = Instant::now();
        while !time_up() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the flag never rose"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(deadline.expired(), Some(Expired::Wall));
        drop(deadline);
        assert!(!time_up());
    }
}
