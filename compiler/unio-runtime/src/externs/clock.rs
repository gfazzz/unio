//! External functions of the `std` layer for `std.time` and `std.random`: the monotonic and
//! the wall clock, sleeping, and the random source of the operating system. None needs a
//! permission of the manifest; the compiler derives the effects `clock` and `random` from them
//! (docs/semantics.md 8.2, RFC 0009), so that `unio symbol` shows which functions read the
//! current time or obtain randomness from outside the program.
//!
//! The random source of the system is the only one: no variable, policy or global switch
//! replaces it with a seeded generator. A test that needs reproducible numbers passes a
//! `Generator` made with `random.seeded`. The one hook, [`set_source_unavailable`], is for the
//! tests of this crate and can only make the source fail.

use super::{int, ExternFn};
use crate::fault::{FaultKind, Outcome, Unwind};
use crate::runtime::Runtime;
use crate::value::Value;
use std::cell::Cell;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const BINDINGS: &[(&str, ExternFn)] = &[
    ("std.time.monotonic_nanoseconds", monotonic_nanoseconds),
    ("std.time.wall_nanoseconds", wall_nanoseconds),
    ("std.time.sleep_nanoseconds", sleep_nanoseconds),
    ("std.random.entropy", entropy),
    ("std.random.secure_bytes", secure_bytes),
    ("std.random.secure_int", secure_int),
];

thread_local! {
    static UNAVAILABLE: Cell<bool> = const { Cell::new(false) };
}

/// Tests only: makes the random source of the system fail on this thread until it is set back,
/// as a system without entropy would. It cannot provide other bytes: a failing source is the
/// only thing it can simulate, so no test can make secure randomness weak.
pub fn set_source_unavailable(unavailable: bool) {
    UNAVAILABLE.with(|flag| flag.set(unavailable));
}

/// The largest request of `secure_bytes`: 1 MiB, beyond which a key or a nonce is not meant.
const MAXIMUM_SECURE_BYTES: i64 = 1 << 20;

/// Nanoseconds of the monotonic clock since a point fixed when the process first asks, so the
/// value starts near 0 and never decreases within one run.
fn monotonic_nanoseconds(_: &mut Runtime, _: &mut [Value]) -> Outcome {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    let elapsed = start.elapsed().as_nanos();
    Ok(Value::Int(i64::try_from(elapsed).unwrap_or(i64::MAX)))
}

/// Nanoseconds since 1970-01-01T00:00:00Z by the wall clock of the system, negative before it.
/// The wall clock can jump when the system time is set.
fn wall_nanoseconds(_: &mut Runtime, _: &mut [Value]) -> Outcome {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_nanos()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |n| -n),
    };
    Ok(Value::Int(nanos))
}

/// Suspends the program for at least the given number of nanoseconds; zero or less returns at
/// once.
fn sleep_nanoseconds(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let nanos = int(runtime, arguments, 0)?;
    if nanos > 0 {
        // In steps, so that the time limit of a test ends the sleep too (T-12).
        let end = Instant::now() + Duration::from_nanos(nanos as u64);
        loop {
            if crate::deadline::time_up() {
                return Err(Unwind::Exhausted(crate::fault::Exhaustion::Time));
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(10)));
        }
    }
    Ok(Value::Unit)
}

/// 64 bits from the random source of the operating system, as an Int. A system that cannot
/// give them ends the program with a fault: there is no weaker fallback.
fn entropy(runtime: &mut Runtime, _: &mut [Value]) -> Outcome {
    let mut bytes = [0u8; 8];
    if let Err(error) = system_random(&mut bytes) {
        return Err(runtime.fault(
            FaultKind::InvalidArgument,
            format_args!("the random source of the operating system failed: {error}"),
        ));
    }
    Ok(Value::Int(i64::from_le_bytes(bytes)))
}

/// A `std.random.RandomError` with the reason.
fn random_error(reason: String) -> Unwind {
    Unwind::Error(Box::new(Value::record(
        "RandomError",
        vec![("reason", Value::text(reason))],
    )))
}

/// count bytes from the random source of the system, for keys and secrets; a source that fails
/// is a `RandomError`, never a weaker generator. A negative count or one above 1 MiB is a fault.
fn secure_bytes(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let count = int(runtime, arguments, 0)?;
    if !(0..=MAXIMUM_SECURE_BYTES).contains(&count) {
        return Err(runtime.fault(
            FaultKind::InvalidArgument,
            format_args!("secure_bytes: the count {count} is not 0 to {MAXIMUM_SECURE_BYTES}"),
        ));
    }
    let mut data = super::buffer(runtime, count as usize)?;
    system_random(&mut data)
        .map_err(|e| random_error(format!("the random source of the system failed: {e}")))?;
    Ok(Value::bytes(data))
}

/// An Int in low..high, high excluded, each equally likely, from the random source of the
/// system (rejection sampling on 64 bits); low >= high is a fault, a failing source a
/// `RandomError`.
fn secure_int(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let low = int(runtime, arguments, 0)?;
    let high = int(runtime, arguments, 1)?;
    if low >= high {
        return Err(runtime.fault(
            FaultKind::InvalidArgument,
            "secure_int: low must be less than high",
        ));
    }
    let width = high.abs_diff(low);
    // The largest multiple of width that fits in 2^64, so that every value is equally likely.
    let zone = u64::MAX - (u64::MAX - width + 1) % width;
    loop {
        let mut bytes = [0u8; 8];
        system_random(&mut bytes)
            .map_err(|e| random_error(format!("the random source of the system failed: {e}")))?;
        let value = u64::from_le_bytes(bytes);
        if value <= zone {
            return Ok(Value::Int(low.wrapping_add((value % width) as i64)));
        }
    }
}

/// Fills the buffer from `getentropy` (Linux, macOS), which takes at most 256 bytes.
#[cfg(unix)]
pub fn system_random(buffer: &mut [u8]) -> std::io::Result<()> {
    if UNAVAILABLE.with(Cell::get) {
        return Err(std::io::Error::other(
            "the source is unavailable (test hook)",
        ));
    }
    extern "C" {
        fn getentropy(buffer: *mut std::ffi::c_void, length: usize) -> i32;
    }
    for chunk in buffer.chunks_mut(256) {
        // SAFETY: the pointer and length describe a writable chunk of at most 256 bytes.
        if unsafe { getentropy(chunk.as_mut_ptr().cast(), chunk.len()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Fills the buffer from `BCryptGenRandom` with the preferred random generator of the system,
/// which works inside an AppContainer.
#[cfg(windows)]
pub fn system_random(buffer: &mut [u8]) -> std::io::Result<()> {
    if UNAVAILABLE.with(Cell::get) {
        return Err(std::io::Error::other(
            "the source is unavailable (test hook)",
        ));
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    for chunk in buffer.chunks_mut(u32::MAX as usize) {
        // SAFETY: no algorithm handle is needed with the flag; the pointer and length describe
        // a writable chunk.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(std::io::Error::other(format!(
                "BCryptGenRandom returned {status:#x}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_monotonic_clock_never_goes_back() {
        let mut rt = Runtime::new(vec![]);
        let first = monotonic_nanoseconds(&mut rt, &mut []).unwrap();
        sleep_nanoseconds(&mut rt, &mut [Value::Int(1_000_000)]).unwrap();
        let second = monotonic_nanoseconds(&mut rt, &mut []).unwrap();
        match (first, second) {
            (Value::Int(a), Value::Int(b)) => assert!(b - a >= 1_000_000, "{a} then {b}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_wall_clock_is_after_2020() {
        let mut rt = Runtime::new(vec![]);
        match wall_nanoseconds(&mut rt, &mut []).unwrap() {
            Value::Int(n) => assert!(n > 1_577_836_800 * 1_000_000_000),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn secure_randomness_has_the_requested_size_and_range() {
        let mut rt = Runtime::new(vec![]);
        match secure_bytes(&mut rt, &mut [Value::Int(32)]).unwrap() {
            Value::Bytes(b) => assert_eq!(b.len(), 32),
            other => panic!("{other:?}"),
        }
        for (low, high) in [
            (0, 2),
            (-5, 5),
            (i64::MIN, i64::MAX),
            (i64::MAX - 1, i64::MAX),
        ] {
            for _ in 0..50 {
                match secure_int(&mut rt, &mut [Value::Int(low), Value::Int(high)]).unwrap() {
                    Value::Int(n) => assert!(low <= n && n < high, "{n} in {low}..{high}"),
                    other => panic!("{other:?}"),
                }
            }
        }
        assert!(secure_bytes(&mut rt, &mut [Value::Int(-1)]).is_err());
        assert!(secure_int(&mut rt, &mut [Value::Int(3), Value::Int(3)]).is_err());
    }

    #[test]
    fn an_unavailable_source_fails_and_never_falls_back() {
        let mut rt = Runtime::new(vec![]);
        set_source_unavailable(true);
        let bytes = secure_bytes(&mut rt, &mut [Value::Int(16)]);
        let number = secure_int(&mut rt, &mut [Value::Int(0), Value::Int(10)]);
        let seed = entropy(&mut rt, &mut []);
        set_source_unavailable(false);
        for outcome in [bytes, number] {
            match outcome {
                Err(Unwind::Error(e)) => {
                    assert_eq!(e.field("reason").map(|_| ()), Some(()));
                    assert!(format!("{e:?}").contains("RandomError"), "{e:?}");
                }
                other => panic!("expected a RandomError, got {other:?}"),
            }
        }
        assert!(matches!(seed, Err(Unwind::Fault(_))), "{seed:?}");
        assert!(secure_bytes(&mut rt, &mut [Value::Int(16)]).is_ok());
    }

    #[test]
    fn no_variable_replaces_the_source() {
        // Variables a test harness might set to make randomness reproducible change nothing.
        let mut rt = Runtime::new(vec![]);
        for name in ["UNIO_SEED", "UNIO_RANDOM_SEED", "SOURCE_DATE_EPOCH"] {
            std::env::set_var(name, "1");
        }
        let draws: Vec<Value> = (0..4)
            .map(|_| {
                secure_int(&mut rt, &mut [Value::Int(i64::MIN), Value::Int(i64::MAX)]).unwrap()
            })
            .collect();
        for name in ["UNIO_SEED", "UNIO_RANDOM_SEED", "SOURCE_DATE_EPOCH"] {
            std::env::remove_var(name);
        }
        assert!(draws.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn entropy_differs_between_calls() {
        let mut rt = Runtime::new(vec![]);
        let values: Vec<Value> = (0..4).map(|_| entropy(&mut rt, &mut []).unwrap()).collect();
        assert!(values.windows(2).any(|pair| pair[0] != pair[1]));
    }
}
