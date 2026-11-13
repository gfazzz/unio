//! External functions of the `alloc` layer: text, bytes, lists, maps, sets, numbers as text,
//! code points, and the JSON conversions that the compiler provides.

use super::{any, argument, bytes, float, int, self_list, self_map, self_set, text, ExternFn};
use crate::externs::float_text::{float_to_text, text_to_float};
use crate::fault::{FaultKind, Outcome};
use crate::runtime::Runtime;
use crate::value::{OrderedSet, Text, Value};

pub const BINDINGS: &[(&str, ExternFn)] = &[
    ("Int.to_text", int_to_text),
    ("Float.to_text", float_to_text_extern),
    ("Text.length", text_length),
    ("Text.byte_length", text_byte_length),
    ("Text.chars", text_chars),
    ("Text.code_points", text_code_points),
    ("Text.slice", text_slice),
    ("Text.index_of", text_index_of),
    ("Text.to_lower", text_to_lower),
    ("Text.to_upper", text_to_upper),
    ("Text.to_float", text_to_float_extern),
    ("Text.contains", text_contains),
    ("Text.starts_with", text_starts_with),
    ("Text.ends_with", text_ends_with),
    ("Text.count", text_count),
    ("Text.replace", text_replace),
    ("Text.lines", text_lines),
    ("Text.split_whitespace", text_split_whitespace),
    ("Text.trim", text_trim),
    ("Text.trim_start", text_trim_start),
    ("Text.trim_end", text_trim_end),
    ("Text.repeat", text_repeat),
    ("std.text.from_code_point", from_code_point),
    ("std.text.split", text_split),
    ("std.text.join", text_join),
    ("std.text.to_nfc", text_to_nfc),
    ("std.text.to_nfd", text_to_nfd),
    ("std.text.graphemes", text_graphemes),
    ("std.text.unicode_version", unicode_version),
    ("Bytes", bytes_new),
    ("Bytes.length", bytes_length),
    ("Bytes.to_text", bytes_to_text),
    ("List.length", list_length),
    ("List.get", list_get),
    ("List.append", list_append),
    ("List.remove_last", list_remove_last),
    ("Map.length", map_length),
    ("Map.get", map_get),
    ("Map.put", map_put),
    ("Map.remove", map_remove),
    ("Map.entries", map_entries),
    ("Set", set_new),
    ("Set.length", set_length),
    ("Set.contains", set_contains),
    ("Set.insert", set_insert),
    ("Set.remove", set_remove),
    ("Set.items", set_items),
    ("std.json.to_value", json_to_value),
    ("std.json.from_value", json_from_value),
];

fn count(n: usize) -> Value {
    Value::Int(n as i64)
}

fn int_to_text(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::text(int(runtime, arguments, 0)?.to_string()))
}

/// The shortest text that reads back as the same number, such as "0.1" or "1e+300".
fn float_to_text_extern(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::text(float_to_text(float(runtime, arguments, 0)?)))
}

// ---------------------------------------------------------------- Text

fn text_length(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(count(text(runtime, arguments, 0)?.len()))
}

fn text_byte_length(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(count(text(runtime, arguments, 0)?.as_str().len()))
}

/// The characters, each as a Text of length 1; each shares the storage of the text.
fn text_chars(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let chars = t.chars().into_iter().map(Value::Text).collect();
    let list = Value::list(chars);
    runtime.check_heap()?;
    Ok(list)
}

/// The Unicode code points of the characters.
fn text_code_points(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let list = Value::list(
        t.as_str()
            .chars()
            .map(|c| Value::Int(i64::from(u32::from(c))))
            .collect(),
    );
    runtime.check_heap()?;
    Ok(list)
}

fn position(
    runtime: &Runtime,
    value: i64,
    length: usize,
    what: &str,
) -> Result<usize, crate::fault::Unwind> {
    usize::try_from(value)
        .ok()
        .filter(|&p| p <= length)
        .ok_or_else(|| {
            runtime.fault(
                FaultKind::IndexOutOfRange,
                format_args!("{what} {value} is outside 0..{length}"),
            )
        })
}

/// Characters from start up to, but not including, end; a fault outside 0..length() or when
/// start is after end. Constant time; the result shares storage.
fn text_slice(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let start = position(runtime, int(runtime, arguments, 1)?, t.len(), "slice start")?;
    let end = position(runtime, int(runtime, arguments, 2)?, t.len(), "slice end")?;
    match t.slice(start, end) {
        Some(slice) => Ok(Value::Text(slice)),
        None => Err(runtime.fault(
            FaultKind::IndexOutOfRange,
            format_args!("slice start {start} is after its end {end}"),
        )),
    }
}

/// Position of the first occurrence of part at or after start, in characters. An empty part is
/// found at start. A start outside 0..length() is a fault, as for slice.
fn text_index_of(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let part = text(runtime, arguments, 1)?;
    let start = position(
        runtime,
        int(runtime, arguments, 2)?,
        t.len(),
        "search start",
    )?;
    let from = t.byte_of_char(start);
    let found = t.as_str()[from..].find(part.as_str());
    Ok(Value::option(found.map(|byte| {
        let index = t
            .char_index_of_byte(from + byte)
            .expect("a match starts at a character boundary");
        count(index)
    })))
}

/// Lower case by the Unicode rules.
fn text_to_lower(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::text(
        text(runtime, arguments, 0)?.as_str().to_lowercase(),
    ))
}

/// Upper case by the Unicode rules.
fn text_to_upper(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::text(
        text(runtime, arguments, 0)?.as_str().to_uppercase(),
    ))
}

/// The number written in decimal or scientific notation; none for anything else.
fn text_to_float_extern(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(Value::option(
        text_to_float(text(runtime, arguments, 0)?.as_str()).map(Value::Float),
    ))
}

/// White space as the text methods define it: space, tab, line ends, vertical tab, form feed.
fn is_white(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{B}' | '\u{C}')
}

/// A fault with the message of the assertion that the Unio definition of the method made.
fn assertion(runtime: &Runtime, condition: &str) -> crate::fault::Unwind {
    runtime.fault(
        FaultKind::AssertionFailed,
        format_args!("assertion failed: {condition}"),
    )
}

/// A part of `t` by byte offsets, as a slice that shares the storage of `t`.
fn part(t: &Text, start: usize, end: usize) -> Value {
    let (Some(from), Some(to)) = (t.char_index_of_byte(start), t.char_index_of_byte(end)) else {
        return Value::text(&t.as_str()[start..end]);
    };
    Value::Text(t.slice(from, to).expect("a part inside the text"))
}

/// Parts of `t` at byte ranges in increasing order, as slices that share the storage of `t`.
/// The character positions are counted along the way, so that the index of positions of a
/// non-ASCII text is not needed.
fn parts(t: &Text, pieces: impl Iterator<Item = (usize, usize)>) -> Vec<Value> {
    if t.is_ascii() {
        return pieces
            .map(|(start, end)| Value::Text(t.slice(start, end).expect("a part inside the text")))
            .collect();
    }
    let s = t.as_str();
    let (mut byte, mut position) = (0, 0);
    let mut at = |target: usize| {
        position += s[byte..target].chars().count();
        byte = target;
        position
    };
    pieces
        .map(|(start, end)| {
            let from = at(start);
            let to = at(end);
            Value::Text(t.slice(from, to).expect("a part inside the text"))
        })
        .collect()
}

/// Byte ranges of the parts of `s` cut at every occurrence of `separator`.
fn cut<'a>(s: &'a str, separator: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    let mut start = 0;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        match s[start..].find(separator) {
            Some(found) => {
                let range = (start, start + found);
                start += found + separator.len();
                Some(range)
            }
            None => {
                done = true;
                Some((start, s.len()))
            }
        }
    })
}

fn text_list(runtime: &Runtime, items: Vec<Value>) -> Outcome {
    let list = Value::list(items);
    runtime.check_heap()?;
    Ok(list)
}

fn text_contains(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let part = text(runtime, arguments, 1)?;
    Ok(Value::Bool(t.as_str().contains(part.as_str())))
}

fn text_starts_with(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let prefix = text(runtime, arguments, 1)?;
    Ok(Value::Bool(t.as_str().starts_with(prefix.as_str())))
}

fn text_ends_with(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let suffix = text(runtime, arguments, 1)?;
    Ok(Value::Bool(t.as_str().ends_with(suffix.as_str())))
}

/// Number of non-overlapping occurrences of part; part must not be empty.
fn text_count(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let part = text(runtime, arguments, 1)?;
    if part.is_empty() {
        return Err(assertion(runtime, "not part.is_empty()"));
    }
    let n = match part.as_str().as_bytes() {
        [byte] => t.as_str().bytes().filter(|b| b == byte).count(),
        _ => t.as_str().matches(part.as_str()).count(),
    };
    Ok(count(n))
}

/// Every occurrence of old replaced with new; old must not be empty.
fn text_replace(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let old = text(runtime, arguments, 1)?;
    let new = text(runtime, arguments, 2)?;
    if old.is_empty() {
        return Err(assertion(runtime, "not old.is_empty()"));
    }
    if !t.as_str().contains(old.as_str()) {
        return Ok(Value::Text(t));
    }
    let result = Value::text(t.as_str().replace(old.as_str(), new.as_str()));
    runtime.check_heap()?;
    Ok(result)
}

/// Lines split on "\n" and "\r\n"; one final empty line is dropped, so "a\n" has one line.
fn text_lines(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let s = t.as_str();
    let mut lines = parts(
        &t,
        cut(s, "\n").map(|(start, end)| {
            if s[start..end].ends_with('\r') {
                (start, end - 1)
            } else {
                (start, end)
            }
        }),
    );
    if matches!(lines.last(), Some(Value::Text(last)) if last.is_empty()) {
        lines.pop();
    }
    text_list(runtime, lines)
}

/// Words separated by white space (space, tab, line ends, vertical tab, form feed).
fn text_split_whitespace(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let s = t.as_str();
    let mut ranges = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (is_white(c), start) {
            (true, Some(b)) => {
                ranges.push((b, i));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(b) = start {
        ranges.push((b, s.len()));
    }
    let words = parts(&t, ranges.into_iter());
    text_list(runtime, words)
}

fn trimmed(t: &Text, start: bool, end: bool) -> Value {
    let s = t.as_str();
    let mut from = 0;
    let mut to = s.len();
    if start {
        from = s.len() - s.trim_start_matches(is_white).len();
    }
    if end {
        to = from + s[from..].trim_end_matches(is_white).len();
    }
    part(t, from, to)
}

/// The text without white space at the start and at the end.
fn text_trim(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(trimmed(&text(runtime, arguments, 0)?, true, true))
}

fn text_trim_start(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(trimmed(&text(runtime, arguments, 0)?, true, false))
}

fn text_trim_end(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    Ok(trimmed(&text(runtime, arguments, 0)?, false, true))
}

/// The text repeated the given number of times; a negative count is a fault.
fn text_repeat(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let times = int(runtime, arguments, 1)?;
    let Ok(times) = usize::try_from(times) else {
        return Err(assertion(runtime, "times >= 0"));
    };
    let Some(size) = t.as_str().len().checked_mul(times) else {
        return Err(crate::fault::Unwind::Exhausted(
            crate::fault::Exhaustion::Heap,
        ));
    };
    // The size is checked against the heap limit and reserved before the text is built.
    let mut data = Vec::new();
    super::reserve(runtime, &mut data, size)?;
    for _ in 0..times {
        data.extend_from_slice(t.as_str().as_bytes());
    }
    let repeated = String::from_utf8(data).expect("repeated UTF-8 is UTF-8");
    let result = Value::text(repeated);
    runtime.check_heap()?;
    Ok(result)
}

/// The text cut at every occurrence of the separator; an empty separator is a fault.
fn text_split(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let separator = text(runtime, arguments, 1)?;
    if separator.is_empty() {
        return Err(assertion(runtime, "not separator.is_empty()"));
    }
    let items = parts(&t, cut(t.as_str(), separator.as_str()));
    text_list(runtime, items)
}

/// The parts with the separator between each two of them.
fn text_join(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let separator = text(runtime, arguments, 1)?;
    let joined = match argument(runtime, arguments, 0)? {
        Value::List(items) => {
            let mut out = String::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(separator.as_str());
                }
                match item {
                    Value::Text(t) => out.push_str(t.as_str()),
                    other => return Err(super::wrong(runtime, 0, "List[Text]", other)),
                }
            }
            out
        }
        other => {
            let other = other.clone();
            return Err(super::wrong(runtime, 0, "List[Text]", &other));
        }
    };
    let result = Value::text(joined);
    runtime.check_heap()?;
    Ok(result)
}

/// The character with the code point; none for a surrogate or a value above 0x10FFFF.
fn from_code_point(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let code = int(runtime, arguments, 0)?;
    let character = u32::try_from(code).ok().and_then(char::from_u32);
    Ok(Value::option(character.map(|c| Value::text(c.to_string()))))
}

/// Normalization Form C of the text (docs/language-contracts.md, S-18).
fn text_to_nfc(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let result = Value::text(super::unicode::nfc(t.as_str()));
    runtime.check_heap()?;
    Ok(result)
}

/// Normalization Form D of the text.
fn text_to_nfd(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let result = Value::text(super::unicode::nfd(t.as_str()));
    runtime.check_heap()?;
    Ok(result)
}

/// The extended grapheme clusters of the text (UAX #29), in order.
fn text_graphemes(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let t = text(runtime, arguments, 0)?;
    let items = super::unicode::graphemes(t.as_str())
        .into_iter()
        .map(Value::text)
        .collect();
    text_list(runtime, items)
}

/// The version of the Unicode Character Database of normalization and grapheme clusters.
fn unicode_version(_: &mut Runtime, _: &mut [Value]) -> Outcome {
    Ok(Value::text(super::unicode_tables::UNICODE_VERSION))
}

// ---------------------------------------------------------------- Bytes

/// An empty sequence of bytes.
fn bytes_new(_: &mut Runtime, _: &mut [Value]) -> Outcome {
    Ok(Value::bytes(Vec::new()))
}

fn bytes_length(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    match any(runtime, arguments, 0)? {
        Value::Bytes(b) => Ok(count(b.len())),
        _ => Ok(count(bytes(runtime, arguments, 0)?.len())),
    }
}

/// The bytes as text; none if they are not valid UTF-8.
fn bytes_to_text(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let data = bytes(runtime, arguments, 0)?;
    Ok(Value::option(String::from_utf8(data).ok().map(Value::text)))
}

// ---------------------------------------------------------------- List

fn list_items(runtime: &Runtime, arguments: &[Value]) -> Result<Vec<Value>, crate::fault::Unwind> {
    match any(runtime, arguments, 0)? {
        Value::List(items) => Ok(items.to_vec()),
        other => Err(runtime.fault(
            FaultKind::InvalidArgument,
            format_args!("internal: expected a List, found a {}", other.kind()),
        )),
    }
}

fn list_length(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    match &arguments[..] {
        [Value::List(items), ..] => Ok(count(items.len())),
        _ => Ok(count(list_items(runtime, arguments)?.len())),
    }
}

/// The item at the index; none outside 0..length().
fn list_get(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let index = int(runtime, arguments, 1)?;
    match &arguments[..] {
        [Value::List(items), ..] => Ok(Value::option(
            usize::try_from(index)
                .ok()
                .and_then(|i| items.get(i).cloned()),
        )),
        _ => Err(runtime.fault(
            FaultKind::InvalidArgument,
            "internal: List.get needs a List",
        )),
    }
}

/// Adds the item at the end in amortized constant time; copies the list first if it is shared.
fn list_append(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let item = any(runtime, arguments, 1)?;
    let items = self_list(runtime, arguments)?;
    items.push(item);
    runtime.check_heap()?;
    Ok(Value::Unit)
}

/// Removes and returns the last item; none for an empty list.
fn list_remove_last(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let items = self_list(runtime, arguments)?;
    let last = items.pop();
    Ok(Value::option(last))
}

// ---------------------------------------------------------------- Map

fn map_length(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    match any(runtime, arguments, 0)? {
        Value::Map(m) => Ok(count(m.len())),
        _ => Err(runtime.fault(
            FaultKind::InvalidArgument,
            "internal: Map.length needs a Map",
        )),
    }
}

fn map_get(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let key = any(runtime, arguments, 1)?;
    match any(runtime, arguments, 0)? {
        Value::Map(m) => Ok(Value::option(m.get(&key).cloned())),
        _ => Err(runtime.fault(FaultKind::InvalidArgument, "internal: Map.get needs a Map")),
    }
}

/// Sets the value for the key; an existing key keeps its position.
fn map_put(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let key = any(runtime, arguments, 1)?;
    let value = any(runtime, arguments, 2)?;
    let map = self_map(runtime, arguments)?;
    map.put(key, value);
    runtime.check_heap()?;
    Ok(Value::Unit)
}

/// Removes the key and returns its value; none if the key is absent.
fn map_remove(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let key = any(runtime, arguments, 1)?;
    let map = self_map(runtime, arguments)?;
    let removed = map.remove(&key);
    Ok(Value::option(removed))
}

/// Pairs of a key and a value in insertion order.
fn map_entries(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    match any(runtime, arguments, 0)? {
        Value::Map(m) => {
            let pairs = m
                .entries()
                .map(|(k, v)| Value::tuple(vec![k.clone(), v.clone()]))
                .collect();
            Ok(Value::list(pairs))
        }
        _ => Err(runtime.fault(
            FaultKind::InvalidArgument,
            "internal: Map.entries needs a Map",
        )),
    }
}

// ---------------------------------------------------------------- Set

/// An empty set.
fn set_new(_: &mut Runtime, _: &mut [Value]) -> Outcome {
    Ok(Value::set(OrderedSet::new()))
}

fn set_length(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    match any(runtime, arguments, 0)? {
        Value::Set(s) => Ok(count(s.len())),
        _ => Err(runtime.fault(
            FaultKind::InvalidArgument,
            "internal: Set.length needs a Set",
        )),
    }
}

/// Used by "item in set".
fn set_contains(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let item = any(runtime, arguments, 1)?;
    match any(runtime, arguments, 0)? {
        Value::Set(s) => Ok(Value::Bool(s.contains(&item))),
        _ => Err(runtime.fault(
            FaultKind::InvalidArgument,
            "internal: Set.contains needs a Set",
        )),
    }
}

/// Adds the item; an item already present keeps its position.
fn set_insert(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let item = any(runtime, arguments, 1)?;
    let set = self_set(runtime, arguments)?;
    set.insert(item);
    runtime.check_heap()?;
    Ok(Value::Unit)
}

/// Removes the item and tells whether it was present.
fn set_remove(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let item = any(runtime, arguments, 1)?;
    let set = self_set(runtime, arguments)?;
    let removed = set.remove(&item);
    Ok(Value::Bool(removed))
}

/// The elements in insertion order.
fn set_items(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    match any(runtime, arguments, 0)? {
        Value::Set(s) => Ok(Value::list(s.items().cloned().collect())),
        _ => Err(runtime.fault(
            FaultKind::InvalidArgument,
            "internal: Set.items needs a Set",
        )),
    }
}

// ---------------------------------------------------------------- JSON

/// The conversions between a type and `Value` are generated by the compiler for every type
/// allowed by S-5; the runtime has no generic implementation. A program that reaches this
/// binding was run without the generated code, which is a fault of the toolchain.
fn json_to_value(runtime: &mut Runtime, _: &mut [Value]) -> Outcome {
    Err(runtime.fault(
        FaultKind::NotProvided,
        "std.json.to_value is provided by the compiler for each type, not by the runtime",
    ))
}

fn json_from_value(runtime: &mut Runtime, _: &mut [Value]) -> Outcome {
    Err(runtime.fault(
        FaultKind::NotProvided,
        "std.json.from_value is provided by the compiler for each type, not by the runtime",
    ))
}

#[cfg(test)]
mod tests {
    use crate::externs::call;
    use crate::fault::{FaultKind, Unwind};
    use crate::runtime::Runtime;
    use crate::value::{OrderedMap, Value};

    fn rt() -> Runtime {
        Runtime::new(vec![])
    }

    fn texts(value: Value) -> Vec<String> {
        match value {
            Value::List(items) => items
                .iter()
                .map(|v| match v {
                    Value::Text(t) => t.as_str().to_string(),
                    other => panic!("{other:?}"),
                })
                .collect(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_methods_count_characters() {
        let mut rt = rt();
        let t = Value::text("Привет, мир 😀");
        assert_eq!(
            call(&mut rt, "Text.length", &mut [t.clone()]).unwrap(),
            Value::Int(13)
        );
        assert_eq!(
            call(&mut rt, "Text.byte_length", &mut [t.clone()]).unwrap(),
            Value::Int(25)
        );
        assert_eq!(
            call(
                &mut rt,
                "Text.slice",
                &mut [t.clone(), Value::Int(8), Value::Int(11)]
            )
            .unwrap(),
            Value::text("мир")
        );
        assert_eq!(
            call(
                &mut rt,
                "Text.index_of",
                &mut [t.clone(), Value::text("мир"), Value::Int(0)]
            )
            .unwrap(),
            Value::some(Value::Int(8))
        );
        assert_eq!(
            call(
                &mut rt,
                "Text.index_of",
                &mut [t.clone(), Value::text(""), Value::Int(13)]
            )
            .unwrap(),
            Value::some(Value::Int(13))
        );
        assert_eq!(
            call(
                &mut rt,
                "Text.index_of",
                &mut [t.clone(), Value::text("и"), Value::Int(6)]
            )
            .unwrap(),
            Value::some(Value::Int(9))
        );
        assert_eq!(
            texts(call(&mut rt, "Text.chars", &mut [Value::text("ёж")]).unwrap()),
            ["ё", "ж"]
        );
        assert_eq!(
            call(&mut rt, "Text.code_points", &mut [Value::text("A😀")]).unwrap(),
            Value::list(vec![Value::Int(65), Value::Int(0x1F600)])
        );
        assert_eq!(
            call(&mut rt, "Text.to_upper", &mut [Value::text("straße")]).unwrap(),
            Value::text("STRASSE")
        );
        assert_eq!(
            call(&mut rt, "Text.to_lower", &mut [Value::text("ЁЖ")]).unwrap(),
            Value::text("ёж")
        );
    }

    #[test]
    fn slicing_outside_the_text_is_a_fault() {
        let mut rt = rt();
        for (start, end) in [(-1, 2), (0, 14), (5, 4)] {
            match call(
                &mut rt,
                "Text.slice",
                &mut [
                    Value::text("Привет, мир 😀"),
                    Value::Int(start),
                    Value::Int(end),
                ],
            ) {
                Err(Unwind::Fault(f)) => assert_eq!(f.kind, FaultKind::IndexOutOfRange),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn numbers_and_code_points() {
        let mut rt = rt();
        assert_eq!(
            call(&mut rt, "Int.to_text", &mut [Value::Int(-42)]).unwrap(),
            Value::text("-42")
        );
        assert_eq!(
            call(&mut rt, "Float.to_text", &mut [Value::Float(0.1)]).unwrap(),
            Value::text("0.1")
        );
        assert_eq!(
            call(&mut rt, "Text.to_float", &mut [Value::text("1e+300")]).unwrap(),
            Value::some(Value::Float(1e300))
        );
        assert_eq!(
            call(&mut rt, "Text.to_float", &mut [Value::text("nan")]).unwrap(),
            Value::none()
        );
        assert_eq!(
            call(
                &mut rt,
                "std.text.from_code_point",
                &mut [Value::Int(0x451)]
            )
            .unwrap(),
            Value::some(Value::text("ё"))
        );
        assert_eq!(
            call(
                &mut rt,
                "std.text.from_code_point",
                &mut [Value::Int(0xD800)]
            )
            .unwrap(),
            Value::none()
        );
        assert_eq!(
            call(
                &mut rt,
                "std.text.from_code_point",
                &mut [Value::Int(0x110000)]
            )
            .unwrap(),
            Value::none()
        );
        assert_eq!(
            call(&mut rt, "std.text.from_code_point", &mut [Value::Int(-1)]).unwrap(),
            Value::none()
        );
    }

    #[test]
    fn bytes() {
        let mut rt = rt();
        let empty = call(&mut rt, "Bytes", &mut []).unwrap();
        assert_eq!(
            call(&mut rt, "Bytes.length", &mut [empty]).unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            call(
                &mut rt,
                "Bytes.to_text",
                &mut [Value::bytes("ёж".as_bytes().to_vec())]
            )
            .unwrap(),
            Value::some(Value::text("ёж"))
        );
        assert_eq!(
            call(&mut rt, "Bytes.to_text", &mut [Value::bytes(vec![0xff])]).unwrap(),
            Value::none()
        );
    }

    #[test]
    fn lists_change_in_place_and_copy_when_shared() {
        let mut rt = rt();
        let shared = Value::list(vec![Value::Int(1)]);
        let mut arguments = [shared.clone(), Value::Int(2)];
        call(&mut rt, "List.append", &mut arguments).unwrap();
        assert_eq!(
            arguments[0],
            Value::list(vec![Value::Int(1), Value::Int(2)])
        );
        assert_eq!(shared, Value::list(vec![Value::Int(1)]));
        assert_eq!(
            call(
                &mut rt,
                "List.get",
                &mut [arguments[0].clone(), Value::Int(1)]
            )
            .unwrap(),
            Value::some(Value::Int(2))
        );
        assert_eq!(
            call(
                &mut rt,
                "List.get",
                &mut [arguments[0].clone(), Value::Int(-1)]
            )
            .unwrap(),
            Value::none()
        );
        let mut list = [arguments[0].clone()];
        assert_eq!(
            call(&mut rt, "List.remove_last", &mut list).unwrap(),
            Value::some(Value::Int(2))
        );
        assert_eq!(
            call(&mut rt, "List.length", &mut list).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn maps_and_sets_keep_insertion_order() {
        let mut rt = rt();
        let mut map = [Value::map(OrderedMap::new())];
        for (k, v) in [("b", 1), ("a", 2), ("b", 3)] {
            let mut arguments = [map[0].clone(), Value::text(k), Value::Int(v)];
            call(&mut rt, "Map.put", &mut arguments).unwrap();
            map[0] = arguments[0].clone();
        }
        assert_eq!(
            call(&mut rt, "Map.entries", &mut map.clone()).unwrap(),
            Value::list(vec![
                Value::tuple(vec![Value::text("b"), Value::Int(3)]),
                Value::tuple(vec![Value::text("a"), Value::Int(2)]),
            ])
        );
        let mut arguments = [map[0].clone(), Value::text("b")];
        assert_eq!(
            call(&mut rt, "Map.remove", &mut arguments).unwrap(),
            Value::some(Value::Int(3))
        );
        assert_eq!(
            call(&mut rt, "Map.length", &mut [arguments[0].clone()]).unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            call(
                &mut rt,
                "Map.get",
                &mut [arguments[0].clone(), Value::text("a")]
            )
            .unwrap(),
            Value::some(Value::Int(2))
        );

        let mut set = [call(&mut rt, "Set", &mut []).unwrap(), Value::Unit];
        for item in ["x", "y", "x"] {
            set[1] = Value::text(item);
            call(&mut rt, "Set.insert", &mut set).unwrap();
        }
        assert_eq!(
            call(&mut rt, "Set.items", &mut [set[0].clone()]).unwrap(),
            Value::list(vec![Value::text("x"), Value::text("y")])
        );
        assert_eq!(
            call(
                &mut rt,
                "Set.contains",
                &mut [set[0].clone(), Value::text("y")]
            )
            .unwrap(),
            Value::Bool(true)
        );
        set[1] = Value::text("x");
        assert_eq!(
            call(&mut rt, "Set.remove", &mut set).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            call(&mut rt, "Set.length", &mut [set[0].clone()]).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn json_conversions_are_provided_by_the_compiler() {
        let mut rt = rt();
        match call(&mut rt, "std.json.to_value", &mut [Value::Int(1)]) {
            Err(Unwind::Fault(f)) => assert_eq!(f.kind, FaultKind::NotProvided),
            other => panic!("{other:?}"),
        }
    }
}
