//! External functions of the `core` layer: faults, identity, checked arithmetic, and the
//! classification and total order of `Float`.

use super::{any, float, int, text, ExternFn};
use crate::fault::{FaultKind, Outcome, Unwind};
use crate::runtime::Runtime;
use crate::value::Value;
use std::rc::Rc;

pub const BINDINGS: &[(&str, ExternFn)] = &[
    ("assert_fail", assert_fail),
    ("expect_fault", expect_fault),
    ("is_same", is_same),
    ("Int.checked_add", checked_add),
    ("Int.checked_subtract", checked_subtract),
    ("Int.checked_multiply", checked_multiply),
    ("Int.to_float", int_to_float),
    ("Float.is_nan", is_nan),
    ("Float.is_finite", is_finite),
    ("total_order_key", total_order_key),
];

/// Ends the program, or the current test, with a fault that carries the message.
fn assert_fail(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let message = text(runtime, arguments, 0)?;
    Err(runtime.fault(FaultKind::AssertFail, message.as_str()))
}

/// Runs the action and succeeds only if it ends with a fault; the resources the action opened
/// were released by its own unwinding before this returns (docs/runtime.md, 4.3).
fn expect_fault(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let action = match any(runtime, arguments, 0)? {
        Value::Function(f) => f,
        other => {
            return Err(runtime.fault(
                FaultKind::InvalidArgument,
                format_args!(
                    "internal: expect_fault needs a function, found a {}",
                    other.kind()
                ),
            ))
        }
    };
    // The fault opens the reserve for `release` (C-17); once it is received here, the test goes
    // on with the budget it had, unless the reserve was already open for an outer fault.
    let hooks = crate::layers::hooks();
    let was_open = (hooks.reserve_is_open)();
    match action.call(runtime, &mut []) {
        Err(Unwind::Fault(fault)) => {
            drop(fault);
            if !was_open {
                (hooks.close_reserve)();
            }
            Ok(Value::Unit)
        }
        Err(Unwind::Exhausted(what)) => Err(Unwind::Exhausted(what)),
        Ok(_) | Err(Unwind::Error(_)) => Err(runtime.fault(
            FaultKind::ExpectedFault,
            "expect_fault: the action ended without a fault",
        )),
    }
}

/// Whether two names refer to the same resource; resources have no ==.
fn is_same(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    match (any(runtime, arguments, 0)?, any(runtime, arguments, 1)?) {
        (Value::Resource(a), Value::Resource(b)) => Ok(Value::Bool(Rc::ptr_eq(&a, &b))),
        (a, _) => Err(runtime.fault(
            FaultKind::InvalidArgument,
            format_args!("internal: is_same compares resources, found a {}", a.kind()),
        )),
    }
}

fn checked(result: Option<i64>) -> Outcome {
    Ok(Value::option(result.map(Value::Int)))
}

/// Sum, or none on overflow.
fn checked_add(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    checked(int(runtime, arguments, 0)?.checked_add(int(runtime, arguments, 1)?))
}

/// Difference, or none on overflow.
fn checked_subtract(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    checked(int(runtime, arguments, 0)?.checked_sub(int(runtime, arguments, 1)?))
}

/// Product, or none on overflow.
fn checked_multiply(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    checked(int(runtime, arguments, 0)?.checked_mul(int(runtime, arguments, 1)?))
}

/// The nearest Float (round to nearest, ties to even).
fn int_to_float(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Float(int(runtime, arguments, 0)? as f64))
}

fn is_nan(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Bool(float(runtime, arguments, 0)?.is_nan()))
}

/// True unless the number is infinite or NaN.
fn is_finite(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Bool(float(runtime, arguments, 0)?.is_finite()))
}

/// For non-NaN raw binary64 bits, the magnitude is the bits with the sign bit cleared; the key
/// is -magnitude for negative numbers and magnitude otherwise, so both zeros give 0. All NaN
/// encodings give MAXIMUM_INT.
pub fn total_order_key_of(value: f64) -> i64 {
    if value.is_nan() {
        return i64::MAX;
    }
    let bits = value.to_bits();
    let magnitude = (bits & !(1u64 << 63)) as i64;
    if bits >> 63 == 1 {
        -magnitude
    } else {
        magnitude
    }
}

fn total_order_key(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::Int(total_order_key_of(float(
        runtime, arguments, 0,
    )?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::externs::call;
    use crate::fault::Fault;
    use crate::value::Callable;

    struct Action(bool);

    impl Callable for Action {
        fn call(&self, runtime: &mut Runtime, _: &mut [Value]) -> Outcome {
            if self.0 {
                Err(runtime.fault(FaultKind::IndexOutOfRange, "boom"))
            } else {
                Ok(Value::Unit)
            }
        }
        fn name(&self) -> &str {
            "action"
        }
    }

    fn fault_of(outcome: Outcome) -> Fault {
        match outcome {
            Err(Unwind::Fault(f)) => *f,
            other => panic!("expected a fault, got {other:?}"),
        }
    }

    #[test]
    fn assert_fail_and_expect_fault() {
        let mut rt = Runtime::new(vec![]);
        let fault = fault_of(call(&mut rt, "assert_fail", &mut [Value::text("no")]));
        assert_eq!(
            (fault.kind, fault.message.as_str()),
            (FaultKind::AssertFail, "no")
        );
        let faulting = Value::Function(Rc::new(Action(true)));
        assert_eq!(
            call(&mut rt, "expect_fault", &mut [faulting]).unwrap(),
            Value::Unit
        );
        let quiet = Value::Function(Rc::new(Action(false)));
        assert_eq!(
            fault_of(call(&mut rt, "expect_fault", &mut [quiet])).kind,
            FaultKind::ExpectedFault
        );
    }

    #[test]
    fn identity_of_resources() {
        let mut rt = Runtime::new(vec![]);
        let a = rt.new_resource(Value::Unit);
        let b = rt.new_resource(Value::Unit);
        let same = call(&mut rt, "is_same", &mut [a.clone(), a.clone()]).unwrap();
        let different = call(&mut rt, "is_same", &mut [a, b]).unwrap();
        assert_eq!((same, different), (Value::Bool(true), Value::Bool(false)));
    }

    #[test]
    fn checked_arithmetic() {
        let mut rt = Runtime::new(vec![]);
        let max = Value::Int(i64::MAX);
        assert_eq!(
            call(
                &mut rt,
                "Int.checked_add",
                &mut [max.clone(), Value::Int(1)]
            )
            .unwrap(),
            Value::none()
        );
        assert_eq!(
            call(
                &mut rt,
                "Int.checked_subtract",
                &mut [Value::Int(5), Value::Int(7)]
            )
            .unwrap(),
            Value::some(Value::Int(-2))
        );
        assert_eq!(
            call(&mut rt, "Int.checked_multiply", &mut [max, Value::Int(2)]).unwrap(),
            Value::none()
        );
        assert_eq!(
            call(&mut rt, "Int.to_float", &mut [Value::Int(3)]).unwrap(),
            Value::Float(3.0)
        );
    }

    #[test]
    fn float_classification_and_total_order() {
        let mut rt = Runtime::new(vec![]);
        assert_eq!(
            call(&mut rt, "Float.is_nan", &mut [Value::Float(f64::NAN)]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            call(
                &mut rt,
                "Float.is_finite",
                &mut [Value::Float(f64::INFINITY)]
            )
            .unwrap(),
            Value::Bool(false)
        );
        assert_eq!(total_order_key_of(0.0), total_order_key_of(-0.0));
        let ordered = [
            f64::NEG_INFINITY,
            -1.5,
            -1e-300,
            0.0,
            1e-300,
            2.0,
            f64::INFINITY,
        ];
        for pair in ordered.windows(2) {
            assert!(
                total_order_key_of(pair[0]) < total_order_key_of(pair[1]),
                "{pair:?}"
            );
        }
        assert_eq!(total_order_key_of(f64::NAN), i64::MAX);
        assert_eq!(total_order_key_of(-f64::NAN), i64::MAX);
        assert!(total_order_key_of(f64::INFINITY) < i64::MAX);
    }
}
