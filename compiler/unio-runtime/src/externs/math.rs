//! External functions of the `core` layer for `std.math` and the bit form of `Float`: the
//! elementary functions of IEEE binary64, rounding, and conversion between `Float` and `Int`.
//! None allocates. The results are those of the Rust standard library of the pinned toolchain,
//! which calls the platform's `libm` for the transcendental functions; `sqrt`, the roundings
//! and the conversions are exact and the same on every platform.

use super::{float, int, ExternFn};
use crate::fault::Outcome;
use crate::runtime::Runtime;
use crate::value::Value;

pub const BINDINGS: &[(&str, ExternFn)] = &[
    ("Float.to_int", to_int),
    ("Float.to_bits", to_bits),
    ("std.math.from_bits", from_bits),
    ("std.math.sqrt", sqrt),
    ("std.math.cbrt", cbrt),
    ("std.math.exp", exp),
    ("std.math.ln", ln),
    ("std.math.log2", log2),
    ("std.math.log10", log10),
    ("std.math.sin", sin),
    ("std.math.cos", cos),
    ("std.math.tan", tan),
    ("std.math.asin", asin),
    ("std.math.acos", acos),
    ("std.math.atan", atan),
    ("std.math.atan2", atan2),
    ("std.math.power", power),
    ("std.math.hypot", hypot),
    ("std.math.floor", floor),
    ("std.math.ceil", ceil),
    ("std.math.round", round),
    ("std.math.truncate", truncate),
];

/// The integer the number equals; none when it has a fraction, is not finite, or lies outside
/// MINIMUM_INT..MAXIMUM_INT.
pub fn exact_int(x: f64) -> Option<i64> {
    // 2^63 is exactly representable; every finite Float below it in magnitude, without a
    // fraction, fits.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if !x.is_finite() || x.fract() != 0.0 || !(-LIMIT..LIMIT).contains(&x) {
        return None;
    }
    Some(x as i64)
}

fn to_int(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let x = float(runtime, arguments, 0)?;
    Ok(Value::option(exact_int(x).map(Value::Int)))
}

/// The binary64 bits of the number, read as a two's-complement Int.
fn to_bits(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Int(float(runtime, arguments, 0)?.to_bits() as i64))
}

/// The number whose binary64 bits are the two's-complement bits of the Int.
fn from_bits(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Float(f64::from_bits(
        int(runtime, arguments, 0)? as u64
    )))
}

macro_rules! unary {
    ($($name:ident => $f:expr),* $(,)?) => {
        $(
            fn $name(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
                let f: fn(f64) -> f64 = $f;
                Ok(Value::Float(f(float(runtime, arguments, 0)?)))
            }
        )*
    };
}

unary! {
    sqrt => f64::sqrt,
    cbrt => f64::cbrt,
    exp => f64::exp,
    ln => f64::ln,
    log2 => f64::log2,
    log10 => f64::log10,
    sin => f64::sin,
    cos => f64::cos,
    tan => f64::tan,
    asin => f64::asin,
    acos => f64::acos,
    atan => f64::atan,
    floor => f64::floor,
    ceil => f64::ceil,
    round => f64::round,
    truncate => f64::trunc,
}

fn atan2(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let y = float(runtime, arguments, 0)?;
    Ok(Value::Float(y.atan2(float(runtime, arguments, 1)?)))
}

fn power(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let base = float(runtime, arguments, 0)?;
    Ok(Value::Float(base.powf(float(runtime, arguments, 1)?)))
}

fn hypot(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let x = float(runtime, arguments, 0)?;
    Ok(Value::Float(x.hypot(float(runtime, arguments, 1)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_to_int_are_exact_or_absent() {
        assert_eq!(exact_int(3.0), Some(3));
        assert_eq!(exact_int(-0.0), Some(0));
        assert_eq!(exact_int(2.5), None);
        assert_eq!(exact_int(f64::NAN), None);
        assert_eq!(exact_int(f64::INFINITY), None);
        assert_eq!(exact_int(-9_223_372_036_854_775_808.0), Some(i64::MIN));
        assert_eq!(exact_int(9_223_372_036_854_775_808.0), None);
        assert_eq!(
            exact_int(9_223_372_036_854_774_784.0),
            Some(9_223_372_036_854_774_784)
        );
    }

    #[test]
    fn bits_round_trip() {
        let mut rt = Runtime::new(vec![]);
        for x in [0.0, -0.0, 1.5, f64::INFINITY, f64::MIN_POSITIVE] {
            let bits = to_bits(&mut rt, &mut [Value::Float(x)]).unwrap();
            let back = from_bits(&mut rt, &mut [bits]).unwrap();
            assert_eq!(back, Value::Float(x));
        }
    }
}
