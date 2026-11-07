// Benchmark (b), same algorithm as stdlib/alloc/json.unio.
use std::env;
use std::fs;

enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

struct Reader {
    chars: Vec<char>,
    position: usize,
}

impl Reader {
    fn peek(&self) -> char {
        self.chars.get(self.position).copied().unwrap_or('\0')
    }
    fn advance(&mut self) -> char {
        let c = self.peek();
        self.position += 1;
        c
    }
}

fn skip_space(r: &mut Reader) {
    while matches!(r.peek(), ' ' | '\t' | '\n' | '\r') {
        r.advance();
    }
}

fn expect(r: &mut Reader, word: &str) {
    for c in word.chars() {
        assert!(r.advance() == c, "expected {word}");
    }
}

fn read_value(r: &mut Reader) -> Value {
    skip_space(r);
    match r.peek() {
        '{' => read_object(r),
        '[' => read_array(r),
        '"' => Value::Text(read_text(r)),
        't' => {
            expect(r, "true");
            Value::Bool(true)
        }
        'f' => {
            expect(r, "false");
            Value::Bool(false)
        }
        'n' => {
            expect(r, "null");
            Value::Null
        }
        _ => read_number(r),
    }
}

fn read_object(r: &mut Reader) -> Value {
    r.advance();
    let mut fields = Vec::new();
    skip_space(r);
    if r.peek() == '}' {
        r.advance();
        return Value::Object(fields);
    }
    loop {
        skip_space(r);
        let key = read_text(r);
        skip_space(r);
        expect(r, ":");
        fields.push((key, read_value(r)));
        skip_space(r);
        if r.advance() == '}' {
            return Value::Object(fields);
        }
    }
}

fn read_array(r: &mut Reader) -> Value {
    r.advance();
    let mut items = Vec::new();
    skip_space(r);
    if r.peek() == ']' {
        r.advance();
        return Value::Array(items);
    }
    loop {
        items.push(read_value(r));
        skip_space(r);
        if r.advance() == ']' {
            return Value::Array(items);
        }
    }
}

fn read_text(r: &mut Reader) -> String {
    r.advance();
    let mut result = String::new();
    loop {
        // A run of characters that need no escape is copied at once.
        let start = r.position;
        while r.position < r.chars.len() {
            let c = r.chars[r.position];
            if c == '"' || c == '\\' || c < ' ' {
                break;
            }
            r.position += 1;
        }
        result.extend(&r.chars[start..r.position]);
        let c = r.advance();
        if c == '"' {
            return result;
        }
        if c == '\\' {
            match r.advance() {
                'u' => {
                    let hex: String = (0..4).map(|_| r.advance()).collect();
                    let code = u32::from_str_radix(&hex, 16).unwrap();
                    result.push(char::from_u32(code).unwrap());
                }
                'n' => result.push('\n'),
                'r' => result.push('\r'),
                't' => result.push('\t'),
                'b' => result.push('\u{8}'),
                'f' => result.push('\u{c}'),
                e => result.push(e),
            }
        } else {
            result.push(c);
        }
    }
}

fn read_number(r: &mut Reader) -> Value {
    let mut text = String::new();
    while matches!(r.peek(), '-' | '+' | '.' | 'e' | 'E' | '0'..='9') {
        text.push(r.advance());
    }
    if text.contains(['.', 'e', 'E']) {
        Value::Float(text.parse().unwrap())
    } else {
        Value::Int(text.parse().unwrap())
    }
}

fn escape(c: char) -> String {
    match c {
        '"' => "\\\"".to_string(),
        '\\' => "\\\\".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        c if (c as u32) < 0x20 => format!("\\u{:04x}", c as u32),
        c => c.to_string(),
    }
}

fn quote(text: &str) -> String {
    let mut result = String::from("\"");
    for c in text.chars() {
        result += &escape(c);
    }
    result + "\""
}

fn write_value(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(t) => quote(t),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(write_value).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(fields) => {
            let parts: Vec<String> = fields.iter().map(|(k, f)| quote(k) + ":" + &write_value(f)).collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn main() {
    let name = env::args().nth(1).expect("file");
    let text = fs::read_to_string(name).expect("read");
    let mut r = Reader { chars: text.chars().collect(), position: 0 };
    println!("{}", write_value(&read_value(&mut r)).len());
}
