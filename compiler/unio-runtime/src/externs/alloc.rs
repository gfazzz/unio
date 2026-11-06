//! External functions of the `alloc` layer: text, bytes, lists, maps, sets, numbers as text,
//! code points, and the JSON conversions that the compiler provides.

use super::{any, bytes, float, int, self_list, self_map, self_set, text, ExternFn};
use crate::externs::float_text::{float_to_text, text_to_float};
use crate::fault::{FaultKind, Outcome};
use crate::runtime::Runtime;
use crate::value::{OrderedSet, Value};

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
    ("std.text.from_code_point", from_code_point),
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
    let chars = (0..t.len())
        .map(|i| Value::Text(t.char_at(i).expect("an index inside the text")))
        .collect();
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
                format!("{what} {value} is outside 0..{length}"),
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
            format!("slice start {start} is after its end {end}"),
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

/// The character with the code point; none for a surrogate or a value above 0x10FFFF.
fn from_code_point(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let code = int(runtime, arguments, 0)?;
    let character = u32::try_from(code).ok().and_then(char::from_u32);
    Ok(Value::option(character.map(|c| Value::text(c.to_string()))))
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
            format!("internal: expected a List, found a {}", other.kind()),
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
    items.recount();
    runtime.check_heap()?;
    Ok(Value::Unit)
}

/// Removes and returns the last item; none for an empty list.
fn list_remove_last(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let items = self_list(runtime, arguments)?;
    let last = items.pop();
    items.recount();
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
    map.recount();
    runtime.check_heap()?;
    Ok(Value::Unit)
}

/// Removes the key and returns its value; none if the key is absent.
fn map_remove(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let key = any(runtime, arguments, 1)?;
    let map = self_map(runtime, arguments)?;
    let removed = map.remove(&key);
    map.recount();
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
    set.recount();
    runtime.check_heap()?;
    Ok(Value::Unit)
}

/// Removes the item and tells whether it was present.
fn set_remove(runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome {
    let item = any(runtime, arguments, 1)?;
    let set = self_set(runtime, arguments)?;
    let removed = set.remove(&item);
    set.recount();
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
