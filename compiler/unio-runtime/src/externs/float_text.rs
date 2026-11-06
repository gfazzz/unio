//! The text form of numbers: the shortest text that reads back as the same `Float`, and the
//! grammar `Text.to_float` accepts.

/// The shortest text that reads back as the same number. Numbers with a decimal exponent
/// from -6 to 20 are written in decimal notation and always show a fractional part ("2.0",
/// "0.1"); others use scientific notation with a signed exponent ("1e+300", "1.5e-8"). The
/// values that are not finite are written "nan", "inf", and "-inf"; they do not read back,
/// because `to_float` accepts only decimal and scientific notation.
pub fn float_to_text(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // `{:e}` gives the shortest digits that round-trip, as d.ddde±k.
    let scientific = format!("{x:e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("exponent form");
    let exponent: i32 = exponent.parse().expect("an exponent");
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if (-6..21).contains(&exponent) {
        let body = if exponent >= 0 {
            let integer = (exponent + 1) as usize;
            if integer >= digits.len() {
                format!("{digits}{}.0", "0".repeat(integer - digits.len()))
            } else {
                format!("{}.{}", &digits[..integer], &digits[integer..])
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize))
        };
        format!("{sign}{body}")
    } else {
        let mantissa = if digits.len() == 1 {
            digits.clone()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        let exponent_sign = if exponent >= 0 { "+" } else { "-" };
        format!("{sign}{mantissa}e{exponent_sign}{}", exponent.abs())
    }
}

/// Whether the text is a number in decimal or scientific notation: an optional sign, digits
/// with an optional fraction (or a fraction alone), and an optional exponent.
pub fn is_number_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let integer_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - integer_start;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let fraction_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - fraction_start;
    }
    if digits == 0 {
        return false;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exponent_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exponent_start {
            return false;
        }
    }
    i == bytes.len()
}

/// The number written in decimal or scientific notation; none for anything else.
pub fn text_to_float(text: &str) -> Option<f64> {
    if is_number_text(text) {
        text.parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_forms() {
        let cases = [
            (0.1, "0.1"),
            (2.0, "2.0"),
            (-2.5, "-2.5"),
            (1e300, "1e+300"),
            (1.5e-8, "1.5e-8"),
            (1e-7, "1e-7"),
            (1e21, "1e+21"),
            (123456789012345680000.0, "123456789012345680000.0"),
            (0.000001, "0.000001"),
            (-0.0, "-0.0"),
            (f64::NAN, "nan"),
            (f64::NEG_INFINITY, "-inf"),
        ];
        for (value, expected) in cases {
            assert_eq!(float_to_text(value), expected, "{value:e}");
        }
    }

    #[test]
    fn every_finite_text_reads_back_as_the_same_number() {
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut values = vec![
            0.1,
            0.2 + 0.1,
            1.0 / 3.0,
            f64::MAX,
            f64::MIN_POSITIVE,
            5e-324,
            -1e-310,
        ];
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state);
            if value.is_finite() {
                values.push(value);
            }
        }
        for value in values {
            let text = float_to_text(value);
            let back = text_to_float(&text).unwrap_or_else(|| panic!("{text} does not read back"));
            assert_eq!(back.to_bits(), value.to_bits(), "{text}");
        }
    }

    #[test]
    fn only_decimal_and_scientific_notation_is_a_number() {
        for good in ["1", "-1.5", "+2.", ".5", "1e10", "1.5E-3", "007"] {
            assert!(text_to_float(good).is_some(), "{good}");
        }
        for bad in [
            "", "-", ".", "e5", "1e", "1e+", "nan", "inf", "infinity", " 1", "1 ", "1_000", "0x10",
        ] {
            assert!(text_to_float(bad).is_none(), "{bad}");
        }
    }
}
