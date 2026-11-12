//! A small JSON value with a parser and a writer, enough for the messages of the language
//! server (JSON-RPC 2.0 of LSP). The standard library has none, and the project adds no
//! dependency for it (compiler/README.md).

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    /// Members in the order written.
    Object(Vec<(String, Json)>),
}

/// The deepest nesting a message may have; deeper input is refused, not recursed into.
const MAX_DEPTH: usize = 128;

impl Json {
    pub fn object(members: Vec<(&str, Json)>) -> Json {
        Json::Object(
            members
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    pub fn str(text: impl Into<String>) -> Json {
        Json::String(text.into())
    }

    pub fn int(n: impl Into<i64>) -> Json {
        Json::Number(n.into() as f64)
    }

    /// The member of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value at a path of member names.
    pub fn at(&self, path: &[&str]) -> Option<&Json> {
        path.iter().try_fold(self, |v, key| v.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Number(n) if *n >= 0.0 && n.fract() == 0.0 && *n <= 9.007_199_254_740_992e15 => {
                Some(*n as u64)
            }
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Number(n) if n.fract() == 0.0 && n.abs() <= 9.007_199_254_740_992e15 => {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Adds or replaces a member of an object.
    pub fn set(&mut self, key: &str, value: Json) {
        if let Json::Object(members) = self {
            match members.iter_mut().find(|(k, _)| k == key) {
                Some((_, v)) => *v = value,
                None => members.push((key.to_string(), value)),
            }
        }
    }

    pub fn parse(text: &str) -> Result<Json, String> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            at: 0,
        };
        parser.space();
        let value = parser.value(0)?;
        parser.space();
        if parser.at != parser.bytes.len() {
            return Err(format!("unexpected text at byte {}", parser.at));
        }
        Ok(value)
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => {
                if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else if n.is_finite() {
                    let _ = write!(out, "{n}");
                } else {
                    out.push_str("null");
                }
            }
            Json::String(s) => quote(out, s),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(members) => {
                out.push('{');
                for (i, (k, v)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    quote(out, k);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl std::fmt::Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = String::new();
        self.write(&mut out);
        f.write_str(&out)
    }
}

fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.bytes.get(self.at) {
            self.at += 1;
        }
    }

    fn fail<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.at))
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            self.fail("an unknown word")
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return self.fail("nesting too deep");
        }
        match self.bytes.get(self.at) {
            None => self.fail("unexpected end"),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::String),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    self.space();
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return self.fail("expected ',' or ']'"),
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Object(members));
                }
                loop {
                    self.space();
                    if self.bytes.get(self.at) != Some(&b'"') {
                        return self.fail("expected a member name");
                    }
                    let key = self.string()?;
                    self.space();
                    if self.bytes.get(self.at) != Some(&b':') {
                        return self.fail("expected ':'");
                    }
                    self.at += 1;
                    self.space();
                    let value = self.value(depth + 1)?;
                    members.push((key, value));
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Json::Object(members));
                        }
                        _ => return self.fail("expected ',' or '}'"),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.at;
                while let Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') =
                    self.bytes.get(self.at)
                {
                    self.at += 1;
                }
                let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or("");
                match text.parse::<f64>() {
                    Ok(n) => Ok(Json::Number(n)),
                    Err(_) => self.fail("a malformed number"),
                }
            }
            Some(_) => self.fail("unexpected character"),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .and_then(|d| std::str::from_utf8(d).ok())
            .and_then(|d| u32::from_str_radix(d, 16).ok());
        match digits {
            Some(n) => {
                self.at += 4;
                Ok(n)
            }
            None => self.fail("a malformed escape"),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.bytes.get(self.at) {
                None => return self.fail("an unterminated string"),
                Some(b'"') => {
                    self.at += 1;
                    return String::from_utf8(out).or_else(|_| self.fail("invalid UTF-8"));
                }
                Some(b'\\') => {
                    self.at += 1;
                    let c = match self.bytes.get(self.at) {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.at += 1;
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first)
                                && self.bytes[self.at..].starts_with(b"\\u")
                            {
                                self.at += 2;
                                let second = self.hex4()?;
                                0x10000 + ((first - 0xD800) << 10) + (second.wrapping_sub(0xDC00))
                            } else {
                                first
                            };
                            let c = char::from_u32(code).unwrap_or('\u{FFFD}');
                            let mut buffer = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                            continue;
                        }
                        _ => return self.fail("a malformed escape"),
                    };
                    self.at += 1;
                    let mut buffer = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
                Some(&b) => {
                    out.push(b);
                    self.at += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn values_round_trip() {
        let text = r#"{"a":[1,-2.5,true,false,null],"b":"x\"y\\z\né😀","c":{}}"#;
        let value = Json::parse(text).unwrap();
        assert_eq!(value.at(&["a"]).unwrap().as_array().unwrap().len(), 5);
        assert_eq!(value.get("b").unwrap().as_str().unwrap(), "x\"y\\z\né😀");
        let again = Json::parse(&value.to_string()).unwrap();
        assert_eq!(value, again);
        assert_eq!(Json::int(42).to_string(), "42");
    }

    #[test]
    fn malformed_input_is_an_error() {
        for bad in ["", "{", "[1,", "\"abc", "{\"a\" 1}", "nul", "[1] x"] {
            assert!(Json::parse(bad).is_err(), "{bad}");
        }
        let deep = "[".repeat(1000) + &"]".repeat(1000);
        assert!(Json::parse(&deep).is_err());
    }
}
