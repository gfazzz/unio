//! Values of Unio programs (docs/runtime.md, sections 2 and 3).
//!
//! Structs, enums, tuples, lists, maps, sets, bytes, and texts are values: a value is shared
//! between names through reference counting until it is changed, and changing a shared value
//! first copies the level that changes (`Rc::make_mut`). Because values never share mutable state
//! and function values capture copies, a value can never reach itself: there are no reference
//! cycles, so reference counting frees every value and no cycle collector is needed.
//!
//! Resources are the exception: they have identity, are never copied, and live in a
//! [`ResourceCell`] that names the same object from every place that holds it.

use crate::fault::Outcome;
use crate::heap::Counted;
use crate::runtime::Runtime;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::rc::Rc;

/// Names shared by every value of `Option` and `Result`, so that building one allocates only
/// the value itself.
pub mod names {
    use std::rc::Rc;

    pub const OPTION: usize = 0;
    pub const SOME: usize = 1;
    pub const NONE: usize = 2;
    pub const VALUE: usize = 3;
    pub const RESULT: usize = 4;
    pub const OK: usize = 5;
    pub const ERROR: usize = 6;

    thread_local! {
        static NAMES: [Rc<str>; 7] = [
            Rc::from("Option"),
            Rc::from("some"),
            Rc::from("none"),
            Rc::from("value"),
            Rc::from("Result"),
            Rc::from("ok"),
            Rc::from("error"),
        ];
    }

    pub fn with<R>(f: impl FnOnce(&[Rc<str>; 7]) -> R) -> R {
        NAMES.with(f)
    }

    /// The shared name for `text` if it is one of them.
    pub fn get(text: &str) -> Option<Rc<str>> {
        let i = match text {
            "Option" => OPTION,
            "some" => SOME,
            "none" => NONE,
            "value" => VALUE,
            "Result" => RESULT,
            "ok" => OK,
            "error" => ERROR,
            _ => return None,
        };
        Some(with(|n| Rc::clone(&n[i])))
    }
}

/// A value of a Unio program.
///
/// The layout is fixed (`repr(u8)`): the variant's tag is the first byte, in the order below,
/// and an `Int` or a `Float` is the eight bytes at offset 8, a `Bool` the byte at offset 1.
/// Native code (docs/runtime.md 8.4) reads and writes the scalars directly and calls the
/// runtime only for values that own memory; [`layout`] states the numbers it relies on.
#[derive(Clone)]
#[repr(u8)]
pub enum Value {
    /// The result of a function without a result.
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(Text),
    Bytes(Rc<Counted<Vec<u8>>>),
    Path(Text),
    List(Rc<Counted<Vec<Value>>>),
    Tuple(Rc<Counted<Vec<Value>>>),
    Map(Rc<Counted<OrderedMap>>),
    Set(Rc<Counted<OrderedSet>>),
    Struct(Rc<StructValue>),
    Enum(Rc<EnumValue>),
    Function(Rc<dyn Callable>),
    Resource(Rc<ResourceCell>),
}

/// The layout of [`Value`] that native code relies on.
pub mod layout {
    /// The tags of the variants: the first byte of a value.
    pub const UNIT: u8 = 0;
    pub const INT: u8 = 1;
    pub const FLOAT: u8 = 2;
    pub const BOOL: u8 = 3;
    pub const TEXT: u8 = 4;
    pub const RESOURCE: u8 = 14;
    /// Tags up to this one own no memory: copying the bytes copies the value and nothing
    /// needs to be released (no reference count).
    pub const LAST_SCALAR: u8 = BOOL;
    /// Where the payload of `Int` and `Float` is, and that of `Bool`.
    pub const PAYLOAD: i32 = 8;
    pub const BOOL_PAYLOAD: i32 = 1;
    /// The size of a value in bytes.
    pub const SIZE: usize = std::mem::size_of::<super::Value>();

    /// Whether the layout is the one stated here; the native code generator checks it.
    pub fn holds() -> bool {
        use super::Value;
        let tag = |v: &Value| unsafe { *(v as *const Value as *const u8) };
        let int = Value::Int(0x0102_0304_0506_0708);
        let float = Value::Float(1.5);
        let boolean = Value::Bool(true);
        let at8 = |v: &Value| unsafe { *((v as *const Value as *const u8).add(8) as *const u64) };
        let at1 = |v: &Value| unsafe { *((v as *const Value as *const u8).add(1)) };
        tag(&Value::Unit) == UNIT
            && tag(&int) == INT
            && tag(&float) == FLOAT
            && tag(&boolean) == BOOL
            && tag(&Value::text("")) == TEXT
            && at8(&int) == 0x0102_0304_0506_0708
            && at8(&float) == 1.5f64.to_bits()
            && at1(&boolean) == 1
            && SIZE.is_multiple_of(8)
    }
}

/// A function value. The interpreter implements it for closures and named functions.
pub trait Callable {
    /// Calls the function. `var` parameters are changed in place in `arguments`.
    fn call(&self, runtime: &mut Runtime, arguments: &mut [Value]) -> Outcome;
    /// The name used in fault traces.
    fn name(&self) -> &str;
}

/// A value of a struct or an `error` declaration: the type name and the fields in declaration
/// order.
#[derive(Clone, Debug)]
pub struct StructValue {
    pub type_name: Rc<str>,
    pub fields: Vec<(Rc<str>, Value)>,
}

/// A value of an enum: the type name, the variant, and its fields in declaration order.
#[derive(Clone, Debug)]
pub struct EnumValue {
    pub type_name: Rc<str>,
    pub variant: Rc<str>,
    pub fields: Fields,
}

/// The fields of a variant. Most variants, like `some` and `ok`, have at most one field, which
/// is kept inline, so that making such a value allocates once.
#[derive(Clone, Debug, Default)]
pub enum Fields {
    #[default]
    Zero,
    One([(Rc<str>, Value); 1]),
    Many(Vec<(Rc<str>, Value)>),
}

impl std::ops::Deref for Fields {
    type Target = [(Rc<str>, Value)];
    #[inline]
    fn deref(&self) -> &[(Rc<str>, Value)] {
        match self {
            Fields::Zero => &[],
            Fields::One(one) => one,
            Fields::Many(many) => many,
        }
    }
}

impl std::ops::DerefMut for Fields {
    #[inline]
    fn deref_mut(&mut self) -> &mut [(Rc<str>, Value)] {
        match self {
            Fields::Zero => &mut [],
            Fields::One(one) => one,
            Fields::Many(many) => many,
        }
    }
}

impl From<Vec<(Rc<str>, Value)>> for Fields {
    fn from(mut fields: Vec<(Rc<str>, Value)>) -> Fields {
        match fields.len() {
            0 => Fields::Zero,
            1 => Fields::One([fields.pop().expect("one field")]),
            _ => Fields::Many(fields),
        }
    }
}

impl<'a> IntoIterator for &'a Fields {
    type Item = &'a (Rc<str>, Value);
    type IntoIter = std::slice::Iter<'a, (Rc<str>, Value)>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl FromIterator<(Rc<str>, Value)> for Fields {
    fn from_iter<I: IntoIterator<Item = (Rc<str>, Value)>>(items: I) -> Fields {
        Fields::from(items.into_iter().collect::<Vec<_>>())
    }
}

/// A resource: an object with identity, released exactly once.
pub struct ResourceCell {
    pub id: u64,
    pub value: RefCell<Value>,
    pub released: Cell<bool>,
}

impl Value {
    pub fn text(text: impl Into<String>) -> Value {
        Value::Text(Text::new(text))
    }

    pub fn path(text: impl Into<String>) -> Value {
        Value::Path(Text::new(text))
    }

    pub fn bytes(data: Vec<u8>) -> Value {
        Value::Bytes(Rc::new(Counted::new(data)))
    }

    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Rc::new(Counted::new(items)))
    }

    pub fn tuple(items: Vec<Value>) -> Value {
        Value::Tuple(Rc::new(Counted::new(items)))
    }

    pub fn map(map: OrderedMap) -> Value {
        Value::Map(Rc::new(Counted::new(map)))
    }

    pub fn set(set: OrderedSet) -> Value {
        Value::Set(Rc::new(Counted::new(set)))
    }

    /// A struct or error value with named fields in declaration order.
    pub fn record(type_name: &str, fields: Vec<(&str, Value)>) -> Value {
        Value::Struct(Rc::new(StructValue {
            type_name: Rc::from(type_name),
            fields: fields.into_iter().map(|(n, v)| (Rc::from(n), v)).collect(),
        }))
    }

    /// An enum value.
    pub fn variant(type_name: &str, variant: &str, fields: Vec<(&str, Value)>) -> Value {
        Value::Enum(Rc::new(EnumValue {
            type_name: Rc::from(type_name),
            variant: Rc::from(variant),
            fields: fields.into_iter().map(|(n, v)| (Rc::from(n), v)).collect(),
        }))
    }

    pub fn some(value: Value) -> Value {
        Value::interned_variant(names::OPTION, names::SOME, Some(value))
    }

    /// `none`, shared by every use: it has no fields, so one value serves all.
    pub fn none() -> Value {
        thread_local! {
            static NONE: Value = Value::interned_variant(names::OPTION, names::NONE, None);
        }
        NONE.with(Value::clone)
    }

    /// A variant of `Option` or `Result`, whose names are shared instead of allocated.
    fn interned_variant(type_name: usize, variant: usize, value: Option<Value>) -> Value {
        names::with(|n| {
            Value::Enum(Rc::new(EnumValue {
                type_name: Rc::clone(&n[type_name]),
                variant: Rc::clone(&n[variant]),
                fields: match value {
                    Some(v) => Fields::One([(Rc::clone(&n[names::VALUE]), v)]),
                    None => Fields::Zero,
                },
            }))
        })
    }

    pub fn option(value: Option<Value>) -> Value {
        match value {
            Some(v) => Value::some(v),
            None => Value::none(),
        }
    }

    pub fn ok(value: Value) -> Value {
        Value::interned_variant(names::RESULT, names::OK, Some(value))
    }

    pub fn error(value: Value) -> Value {
        Value::interned_variant(names::RESULT, names::ERROR, Some(value))
    }

    /// The name of the kind of value, for internal error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Unit => "nothing",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Bool(_) => "Bool",
            Value::Text(_) => "Text",
            Value::Bytes(_) => "Bytes",
            Value::Path(_) => "Path",
            Value::List(_) => "List",
            Value::Tuple(_) => "Tuple",
            Value::Map(_) => "Map",
            Value::Set(_) => "Set",
            Value::Struct(_) => "struct",
            Value::Enum(_) => "enum",
            Value::Function(_) => "Function",
            Value::Resource(_) => "resource",
        }
    }

    /// The field of a struct or enum value with this name.
    pub fn field(&self, name: &str) -> Option<&Value> {
        let fields = match self {
            Value::Struct(s) => &s.fields[..],
            Value::Enum(e) => &e.fields[..],
            _ => return None,
        };
        fields.iter().find(|(n, _)| &**n == name).map(|(_, v)| v)
    }

    /// The variant name of an enum value.
    pub fn variant_name(&self) -> Option<&str> {
        match self {
            Value::Enum(e) => Some(&e.variant),
            _ => None,
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => write!(f, "()"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{}", crate::externs::float_text::float_to_text(*x)),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Text(t) => write!(f, "{:?}", t.as_str()),
            Value::Bytes(b) => write!(f, "Bytes({})", b.len()),
            Value::Path(p) => write!(f, "Path({:?})", p.as_str()),
            Value::List(items) => f.debug_list().entries(items.iter()).finish(),
            Value::Tuple(items) => {
                let mut t = f.debug_tuple("");
                for item in items.iter() {
                    t.field(item);
                }
                t.finish()
            }
            Value::Map(m) => f.debug_map().entries(m.entries()).finish(),
            Value::Set(s) => f.debug_set().entries(s.items()).finish(),
            Value::Struct(s) => {
                let mut d = f.debug_struct(&s.type_name);
                for (n, v) in &s.fields {
                    d.field(n, v);
                }
                d.finish()
            }
            Value::Enum(e) => {
                write!(f, "{}.{}", e.type_name, e.variant)?;
                if !e.fields.is_empty() {
                    let mut t = f.debug_tuple("");
                    for (_, v) in &e.fields {
                        t.field(v);
                    }
                    t.finish()?;
                }
                Ok(())
            }
            Value::Function(c) => write!(f, "function {}", c.name()),
            Value::Resource(r) => write!(f, "resource #{}", r.id),
        }
    }
}

/// Structural equality as `==` defines it: `Float` follows IEEE 754, resources and functions
/// have no `==` (the checker rejects it), so they compare by identity here.
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Unit, Value::Unit) => true,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Text(x), Value::Text(y)) | (Value::Path(x), Value::Path(y)) => x == y,
        (Value::Bytes(x), Value::Bytes(y)) => x[..] == y[..],
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b))
        }
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.entries()
                    .all(|(k, v)| y.get(k).is_some_and(|w| values_equal(v, w)))
        }
        (Value::Set(x), Value::Set(y)) => {
            x.len() == y.len() && x.items().all(|item| y.contains(item))
        }
        (Value::Struct(x), Value::Struct(y)) => {
            x.type_name == y.type_name && fields_equal(&x.fields, &y.fields)
        }
        (Value::Enum(x), Value::Enum(y)) => {
            x.type_name == y.type_name
                && x.variant == y.variant
                && fields_equal(&x.fields, &y.fields)
        }
        (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
        (Value::Resource(x), Value::Resource(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn fields_equal(a: &[(Rc<str>, Value)], b: &[(Rc<str>, Value)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|((_, x), (_, y))| values_equal(x, y))
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        values_equal(self, other)
    }
}

// ---------------------------------------------------------------- text

/// Immutable text: UTF-8 with an index of character positions, so that length, slicing, and
/// character access take constant time. A slice shares the storage of the text it comes from.
///
/// A text of ASCII characters needs no index: a character position is a byte offset. Any other
/// text builds its index once, the first time a position inside it is needed: the byte offset
/// of every 16th character, from which a position is found in at most 15 steps.
#[derive(Clone)]
pub struct Text {
    data: Rc<Counted<TextData>>,
    start: usize,
    end: usize,
}

/// The storage of a text: the string, the number of characters, and the index of positions,
/// which only a text with non-ASCII characters builds.
#[derive(Clone)]
pub struct TextData {
    string: String,
    chars: usize,
    index: std::cell::OnceCell<Vec<usize>>,
}

impl TextData {
    fn new(string: String) -> TextData {
        let chars = if string.is_ascii() {
            string.len()
        } else {
            string.chars().count()
        };
        TextData {
            string,
            chars,
            index: std::cell::OnceCell::new(),
        }
    }

    #[inline]
    fn is_ascii(&self) -> bool {
        self.chars == self.string.len()
    }

    /// The byte offset of a character position, from 0 to the number of characters.
    #[inline]
    fn byte(&self, char_index: usize) -> usize {
        if self.is_ascii() || char_index == 0 {
            return char_index;
        }
        if char_index == self.chars {
            return self.string.len();
        }
        let bytes = self.string.as_bytes();
        let mut at = self.index()[char_index / STRIDE];
        for _ in 0..char_index % STRIDE {
            at += utf8_width(bytes[at]);
        }
        at
    }

    /// The character position of a byte offset, if it is the start of a character or the end.
    fn char_of_byte(&self, byte: usize) -> Option<usize> {
        if self.is_ascii() {
            return (byte <= self.string.len()).then_some(byte);
        }
        if byte == self.string.len() {
            return Some(self.chars);
        }
        if !self.string.is_char_boundary(byte) {
            return None;
        }
        let index = self.index();
        let block = index.partition_point(|&b| b <= byte) - 1;
        let bytes = self.string.as_bytes();
        let mut at = index[block];
        let mut position = block * STRIDE;
        while at < byte {
            at += utf8_width(bytes[at]);
            position += 1;
        }
        Some(position)
    }

    fn index(&self) -> &Vec<usize> {
        self.index.get_or_init(|| {
            self.string
                .char_indices()
                .step_by(STRIDE)
                .map(|(i, _)| i)
                .collect()
        })
    }
}

/// The distance between two entries of the index of a text, in characters.
const STRIDE: usize = 16;

/// The length in bytes of the UTF-8 character that starts with `lead`.
fn utf8_width(lead: u8) -> usize {
    match lead {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

impl crate::heap::Measure for TextData {
    /// The index is counted as if it were built, so that the count does not depend on
    /// which positions a program happened to use.
    fn measure(&self) -> usize {
        let index = if self.is_ascii() {
            0
        } else {
            (self.chars / STRIDE + 1) * std::mem::size_of::<usize>()
        };
        self.string.capacity() + index
    }
}

impl Text {
    /// A text; its length is counted once, in time proportional to its size.
    pub fn new(text: impl Into<String>) -> Text {
        let data = TextData::new(text.into());
        let end = data.chars;
        Text {
            data: Rc::new(Counted::new(data)),
            start: 0,
            end,
        }
    }

    /// The text as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        let data = &self.data;
        let start = data.byte(self.start);
        if self.end == self.start + 1 {
            // One character, as `chars` and `char_at` make: its end follows from its start.
            let end = start + utf8_width(data.string.as_bytes()[start]);
            return &data.string[start..end];
        }
        &data.string[start..data.byte(self.end)]
    }

    /// The number of characters.
    #[inline]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Whether every character is ASCII, so that positions are byte offsets.
    pub fn is_ascii(&self) -> bool {
        self.data.is_ascii()
    }

    /// Characters from `start` up to, but not including, `end`; none outside `0..=len()` or
    /// when `start > end`. Constant time; the result shares storage.
    pub fn slice(&self, start: usize, end: usize) -> Option<Text> {
        if start > end || end > self.len() {
            return None;
        }
        Some(Text {
            data: Rc::clone(&self.data),
            start: self.start + start,
            end: self.start + end,
        })
    }

    /// The characters as texts of length 1. For a text with non-ASCII characters, each
    /// distinct character gets storage of its own, shared by its occurrences, so that reading
    /// one needs no search for its position in the whole text.
    pub fn chars(&self) -> Vec<Text> {
        if self.is_ascii() {
            return (0..self.len())
                .map(|i| Text {
                    data: Rc::clone(&self.data),
                    start: self.start + i,
                    end: self.start + i + 1,
                })
                .collect();
        }
        let mut seen: HashMap<char, Text, BuildHasherDefault<Fnv>> = HashMap::default();
        self.as_str()
            .chars()
            .map(|c| {
                seen.entry(c)
                    .or_insert_with(|| Text::new(c.to_string()))
                    .clone()
            })
            .collect()
    }

    /// The character at the position as a text of length 1.
    pub fn char_at(&self, index: usize) -> Option<Text> {
        self.slice(index, index + 1)
    }

    /// The character index of a byte offset inside this text, if it is a character boundary.
    pub fn char_index_of_byte(&self, byte: usize) -> Option<usize> {
        let data = &self.data;
        if byte == 0 {
            return Some(0);
        }
        let base = data.byte(self.start);
        let position = data.char_of_byte(base + byte)?;
        (position <= self.end).then(|| position - self.start)
    }

    /// The byte offset of a character index inside this text.
    pub fn byte_of_char(&self, index: usize) -> usize {
        self.data.byte(self.start + index) - self.data.byte(self.start)
    }

    /// This text followed by `tail`. When this value is the only holder of its whole
    /// storage, the storage grows in place, so that a loop of `t = t + part` takes linear
    /// time; otherwise the result is a new text. The value is the same either way.
    pub fn append(mut self, tail: &str) -> Text {
        let whole = self.start == 0 && self.end == self.data.chars;
        if whole {
            if let Some(data) = Rc::get_mut(&mut self.data) {
                data.string.push_str(tail);
                data.chars += if tail.is_ascii() {
                    tail.len()
                } else {
                    tail.chars().count()
                };
                data.index = std::cell::OnceCell::new();
                data.recount();
                self.end = data.chars;
                return self;
            }
        }
        let mut string = String::with_capacity(self.as_str().len() + tail.len());
        string.push_str(self.as_str());
        string.push_str(tail);
        Text::new(string)
    }
}

impl PartialEq for Text {
    #[inline]
    fn eq(&self, other: &Text) -> bool {
        if self.len() != other.len() {
            return false;
        }
        if Rc::ptr_eq(&self.data, &other.data) && self.start == other.start {
            return true;
        }
        self.as_str() == other.as_str()
    }
}

impl Eq for Text {}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl crate::heap::Measure for Vec<Value> {
    fn measure(&self) -> usize {
        self.capacity() * std::mem::size_of::<Value>()
    }

    fn release(&mut self) {
        if self.iter().any(owns_values) {
            release(Pending::Values(std::mem::take(self).into_iter()));
        }
    }
}

impl crate::heap::Measure for Vec<u8> {
    fn measure(&self) -> usize {
        self.capacity()
    }
}

// ---------------------------------------------------------------- hashing

/// FNV-1a: a hash that does not depend on the run, so that maps and sets behave the same on
/// every run (docs/runtime.md, section 2.4).
#[derive(Default)]
pub struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}

type FnvBuild = BuildHasherDefault<Fnv>;

/// A value used as a key: structural equality where `Float` compares by bits, so that equality
/// is reflexive. The checker already forbids `Float` keys (L-9).
#[derive(Clone, Debug)]
pub struct Key(pub Value);

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        key_equal(&self.0, &other.0)
    }
}

impl Eq for Key {}

fn key_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| key_equal(a, b))
        }
        (Value::Struct(x), Value::Struct(y)) => {
            x.type_name == y.type_name
                && x.fields.len() == y.fields.len()
                && x.fields
                    .iter()
                    .zip(&y.fields)
                    .all(|((_, a), (_, b))| key_equal(a, b))
        }
        (Value::Enum(x), Value::Enum(y)) => {
            x.type_name == y.type_name
                && x.variant == y.variant
                && x.fields.len() == y.fields.len()
                && x.fields
                    .iter()
                    .zip(&y.fields)
                    .all(|((_, a), (_, b))| key_equal(a, b))
        }
        _ => values_equal(a, b),
    }
}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_value(&self.0, state);
    }
}

fn hash_value<H: Hasher>(value: &Value, state: &mut H) {
    match value {
        Value::Unit => state.write_u8(0),
        Value::Int(i) => {
            state.write_u8(1);
            state.write_i64(*i);
        }
        Value::Float(x) => {
            state.write_u8(2);
            state.write_u64(x.to_bits());
        }
        Value::Bool(b) => {
            state.write_u8(3);
            state.write_u8(u8::from(*b));
        }
        Value::Text(t) | Value::Path(t) => {
            state.write_u8(4);
            state.write(t.as_str().as_bytes());
            state.write_u8(0xff);
        }
        Value::Bytes(b) => {
            state.write_u8(5);
            state.write(&b[..]);
            state.write_usize(b.len());
        }
        Value::List(items) | Value::Tuple(items) => {
            state.write_u8(6);
            state.write_usize(items.len());
            for item in items.iter() {
                hash_value(item, state);
            }
        }
        Value::Map(m) => {
            state.write_u8(7);
            state.write_usize(m.len());
        }
        Value::Set(s) => {
            state.write_u8(8);
            state.write_usize(s.len());
        }
        Value::Struct(s) => {
            state.write_u8(9);
            state.write(s.type_name.as_bytes());
            for (_, v) in &s.fields {
                hash_value(v, state);
            }
        }
        Value::Enum(e) => {
            state.write_u8(10);
            state.write(e.type_name.as_bytes());
            state.write(e.variant.as_bytes());
            for (_, v) in &e.fields {
                hash_value(v, state);
            }
        }
        Value::Function(_) => state.write_u8(11),
        Value::Resource(r) => {
            state.write_u8(12);
            state.write_u64(r.id);
        }
    }
}

// ---------------------------------------------------------------- map and set

/// A map that iterates in the order keys were first inserted. Removed entries leave a hole
/// that is compacted when holes outnumber entries.
#[derive(Clone, Default)]
pub struct OrderedMap {
    entries: Vec<Option<(Value, Value)>>,
    index: HashMap<Key, usize, FnvBuild>,
}

impl OrderedMap {
    pub fn new() -> OrderedMap {
        OrderedMap::default()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn get(&self, key: &Value) -> Option<&Value> {
        let position = *self.index.get(&Key(key.clone()))?;
        self.entries[position].as_ref().map(|(_, v)| v)
    }

    /// The value under the key, to change in place, as `items[key].field = x` does.
    pub fn get_mut(&mut self, key: &Value) -> Option<&mut Value> {
        let position = *self.index.get(&Key(key.clone()))?;
        self.entries[position].as_mut().map(|(_, v)| v)
    }

    /// Sets the value; an existing key keeps its position.
    pub fn put(&mut self, key: Value, value: Value) {
        match self.index.get(&Key(key.clone())) {
            Some(&position) => self.entries[position] = Some((key, value)),
            None => {
                self.index.insert(Key(key.clone()), self.entries.len());
                self.entries.push(Some((key, value)));
            }
        }
    }

    pub fn remove(&mut self, key: &Value) -> Option<Value> {
        let position = self.index.remove(&Key(key.clone()))?;
        let removed = self.entries[position].take().map(|(_, v)| v);
        if self.entries.len() > 8 && self.entries.len() > 2 * self.index.len() {
            self.compact();
        }
        removed
    }

    fn compact(&mut self) {
        let entries: Vec<Option<(Value, Value)>> =
            self.entries.drain(..).filter(Option::is_some).collect();
        self.entries = entries;
        self.index.clear();
        for (position, entry) in self.entries.iter().enumerate() {
            if let Some((k, _)) = entry {
                self.index.insert(Key(k.clone()), position);
            }
        }
    }

    /// Pairs in insertion order.
    pub fn entries(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.entries.iter().flatten().map(|(k, v)| (k, v))
    }
}

impl crate::heap::Measure for OrderedMap {
    fn measure(&self) -> usize {
        self.entries.capacity() * 2 * std::mem::size_of::<Value>()
            + self.index.capacity() * (std::mem::size_of::<Value>() + 16)
    }

    fn release(&mut self) {
        // A key is held twice, by its entry and by the index, so both are handed over: the one
        // dropped last frees the key, and it must not do so by recursion either.
        if self.index.keys().any(|k| owns_values(&k.0)) {
            release(Pending::Keys(std::mem::take(&mut self.index).into_keys()));
        }
        if self
            .entries
            .iter()
            .flatten()
            .any(|(k, v)| owns_values(k) || owns_values(v))
        {
            release(Pending::Entries(
                std::mem::take(&mut self.entries).into_iter(),
                None,
            ));
        }
    }
}

/// A set that iterates in insertion order.
#[derive(Clone, Default)]
pub struct OrderedSet {
    map: OrderedMap,
}

impl OrderedSet {
    pub fn new() -> OrderedSet {
        OrderedSet::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn contains(&self, item: &Value) -> bool {
        self.map.get(item).is_some()
    }

    /// Adds the item; an item already present keeps its position.
    pub fn insert(&mut self, item: Value) {
        if !self.contains(&item) {
            self.map.put(item, Value::Unit);
        }
    }

    /// Removes the item and tells whether it was present.
    pub fn remove(&mut self, item: &Value) -> bool {
        self.map.remove(item).is_some()
    }

    pub fn items(&self) -> impl Iterator<Item = &Value> {
        self.map.entries().map(|(k, _)| k)
    }
}

impl crate::heap::Measure for OrderedSet {
    fn measure(&self) -> usize {
        self.map.measure()
    }

    fn release(&mut self) {
        self.map.release();
    }
}

// ---------------------------------------------------------------- freeing without recursion

// Dropping a value drops what it holds, and Rust does that by recursion: a chain of a million
// nested values took a million nested calls of `drop`, and a chain long enough ran out of the
// native stack and aborted the process instead of ending with a code of the runtime. Here every
// object that holds values hands them to `release` when it is dropped. Up to `DEEP` levels it
// frees them by recursion, as before, which keeps the order and the speed of freeing ordinary
// structures: freeing every level through a list measured about 50% slower on the wide case of
// bench/free/ with only 3% more instructions, the cost of freeing a parent before its contents.
// Below that level the contents go to a list that a loop of the frame at that level frees one
// by one: objects dropped inside the loop only add their contents to the list, so the depth of
// the stack is bounded by `DEEP` whatever the depth of the structure (RFC 0007). Values that
// hold no values, and values still shared, are dropped in place. Freeing stays immediate and
// complete: the loop ends before the outermost `drop` returns, nothing is deferred, and the
// heap count is exact.

/// The levels of nesting freed by recursion before the loop takes over.
const DEEP: u32 = 128;

/// The contents of a dropped object, still to be freed.
enum Pending {
    One(Option<Value>),
    Values(std::vec::IntoIter<Value>),
    Fields(std::vec::IntoIter<(Rc<str>, Value)>),
    Entries(std::vec::IntoIter<Option<(Value, Value)>>, Option<Value>),
    Keys(std::collections::hash_map::IntoKeys<Key, usize>),
}

impl Pending {
    /// The next value, and whether more may follow.
    fn next(&mut self) -> Option<(Value, bool)> {
        match self {
            Pending::One(value) => value.take().map(|v| (v, false)),
            Pending::Values(items) => items.next().map(|v| (v, items.len() > 0)),
            Pending::Fields(items) => items.next().map(|(_, v)| (v, items.len() > 0)),
            Pending::Keys(keys) => keys.next().map(|k| (k.0, keys.len() > 0)),
            Pending::Entries(items, spare) => {
                if let Some(v) = spare.take() {
                    return Some((v, items.len() > 0));
                }
                let (k, v) = items.by_ref().flatten().next()?;
                *spare = Some(v);
                Some((k, true))
            }
        }
    }
}

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    static RELEASING: Cell<bool> = const { Cell::new(false) };
}

/// Whether dropping the value would drop other values: it is the last holder of an object
/// that contains values. Dropping any other value never reaches [`release`].
fn owns_values(value: &Value) -> bool {
    match value {
        Value::List(r) | Value::Tuple(r) => Rc::strong_count(r) == 1 && !r.is_empty(),
        Value::Map(r) => Rc::strong_count(r) == 1 && !r.is_empty(),
        Value::Set(r) => Rc::strong_count(r) == 1 && !r.is_empty(),
        Value::Struct(r) => Rc::strong_count(r) == 1 && !r.fields.is_empty(),
        Value::Enum(r) => Rc::strong_count(r) == 1 && !r.fields.is_empty(),
        Value::Function(r) => Rc::strong_count(r) == 1,
        Value::Resource(r) => Rc::strong_count(r) == 1,
        _ => false,
    }
}

/// Frees the contents of a dropped object, by recursion up to `DEEP` levels and by a loop
/// below; see the comment above.
fn release(work: Pending) {
    let Ok(depth) = DEPTH.try_with(Cell::get) else {
        // The thread is ending and its lists are gone: free by recursion as Rust does.
        drop(work);
        return;
    };
    if depth < DEEP {
        DEPTH.with(|d| d.set(depth + 1));
        drop(work);
        DEPTH.with(|d| d.set(depth));
        return;
    }
    let outer = RELEASING.with(|r| !r.replace(true));
    PENDING.with(|p| p.borrow_mut().push(work));
    if !outer {
        return;
    }
    loop {
        // Take the next value that holds others, dropping in place those that do not; the
        // borrow ends before the value is dropped, since its drop adds to the list.
        let next = PENDING.with(|p| {
            let mut pending = p.borrow_mut();
            loop {
                let top = pending.last_mut()?;
                match top.next() {
                    Some((value, more)) => {
                        if !more {
                            pending.pop();
                        }
                        if owns_values(&value) {
                            return Some(value);
                        }
                    }
                    None => {
                        pending.pop();
                    }
                }
            }
        });
        match next {
            Some(value) => drop(value),
            None => break,
        }
    }
    RELEASING.with(|r| r.set(false));
}

impl Drop for StructValue {
    fn drop(&mut self) {
        if self.fields.iter().any(|(_, v)| owns_values(v)) {
            release(Pending::Fields(
                std::mem::take(&mut self.fields).into_iter(),
            ));
        }
    }
}

impl Drop for EnumValue {
    fn drop(&mut self) {
        match std::mem::take(&mut self.fields) {
            Fields::Zero => {}
            Fields::One([(_, value)]) => {
                if owns_values(&value) {
                    release(Pending::One(Some(value)));
                }
            }
            Fields::Many(fields) => {
                if fields.iter().any(|(_, v)| owns_values(v)) {
                    release(Pending::Fields(fields.into_iter()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a chain of `depth` levels of each kind of value that holds values and frees it on
    /// a thread with a small stack: freeing by recursion overflowed a 8 MiB stack at about
    /// 100,000 levels of lists and aborted the process (RFC 0007). The stack is 2 MiB, far
    /// below the 48 MB a recursive free of 1,000,000 levels needs, and enough for the 128
    /// levels freed by recursion in a debug build on Windows, whose frames are the largest
    /// (256 KiB overflowed there in CI).
    #[test]
    fn deep_structures_are_freed_without_recursion() {
        let freed = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let before = crate::heap::live_bytes();
                let depth = 1_000_000;
                let wrappers: [fn(usize, Value) -> Value; 6] = [
                    |_, v| Value::list(vec![Value::text("item"), v]),
                    |_, v| Value::tuple(vec![v, Value::Int(1)]),
                    |i, v| {
                        Value::variant(
                            "Chain",
                            "Link",
                            vec![("value", Value::Int(i as i64)), ("next", v)],
                        )
                    },
                    |_, v| Value::variant("Option", "some", vec![("value", v)]),
                    |_, v| Value::record("Node", vec![("next", v), ("name", Value::text("n"))]),
                    |i, v| {
                        let mut map = OrderedMap::new();
                        map.put(Value::Int(i as i64), v);
                        Value::map(map)
                    },
                ];
                for wrap in wrappers {
                    let mut chain = Value::Unit;
                    for i in 0..depth {
                        chain = wrap(i, chain);
                    }
                    drop(chain);
                    assert_eq!(crate::heap::live_bytes(), before);
                }
                // Keys are hashed when they are put, so a key cannot be built a million levels
                // deep; a set of keys that hold values checks that keys are freed by the loop too.
                let mut chain = Value::Unit;
                for i in 0..depth {
                    let mut set = OrderedSet::new();
                    set.insert(Value::list(vec![Value::Int(i as i64)]));
                    set.insert(Value::tuple(vec![Value::text("key"), Value::Int(i as i64)]));
                    chain = Value::list(vec![Value::set(set), chain]);
                }
                drop(chain);
                assert_eq!(crate::heap::live_bytes(), before);
            })
            .unwrap()
            .join();
        assert!(freed.is_ok());
    }

    /// A value that is still shared is not freed with the structure that dropped it.
    #[test]
    fn freeing_a_structure_keeps_its_shared_parts() {
        let shared = Value::list(vec![Value::text("kept"), Value::list(vec![Value::Int(7)])]);
        let mut outer = Value::Unit;
        for _ in 0..1000 {
            outer = Value::list(vec![shared.clone(), outer]);
        }
        drop(outer);
        match &shared {
            Value::List(items) => {
                assert_eq!(Rc::strong_count(items), 1);
                assert_eq!(items.len(), 2);
                assert!(
                    matches!(&items[1], Value::List(inner) if matches!(inner[0], Value::Int(7)))
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn text_indexes_characters_not_bytes() {
        let text = Text::new("ёж и 😀!");
        assert_eq!(text.len(), 7);
        assert_eq!(text.as_str().len(), 13);
        assert_eq!(text.char_at(5).unwrap().as_str(), "😀");
        let slice = text.slice(1, 6).unwrap();
        assert_eq!(slice.as_str(), "ж и 😀");
        assert_eq!(slice.slice(2, 3).unwrap().as_str(), "и");
        assert!(text.slice(3, 8).is_none());
        assert!(text.slice(4, 3).is_none());
        assert_eq!(slice.byte_of_char(2), "ж ".len());
        assert_eq!(slice.char_index_of_byte("ж ".len()), Some(2));
    }

    #[test]
    fn slices_share_storage() {
        let text = Text::new("abcdef");
        let slice = text.slice(2, 4).unwrap();
        assert!(Rc::ptr_eq(&text.data, &slice.data));
    }

    #[test]
    fn maps_keep_insertion_order_and_positions() {
        let mut map = OrderedMap::new();
        for word in ["b", "a", "c"] {
            map.put(Value::text(word), Value::Int(1));
        }
        map.put(Value::text("a"), Value::Int(2));
        let keys: Vec<String> = map
            .entries()
            .map(|(k, _)| match k {
                Value::Text(t) => t.as_str().to_string(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(keys, ["b", "a", "c"]);
        assert_eq!(map.get(&Value::text("a")), Some(&Value::Int(2)));
        assert_eq!(map.remove(&Value::text("b")), Some(Value::Int(1)));
        assert_eq!(map.len(), 2);
        assert!(map.get(&Value::text("b")).is_none());
    }

    #[test]
    fn many_removals_compact_without_losing_order() {
        let mut map = OrderedMap::new();
        for i in 0..100 {
            map.put(Value::Int(i), Value::Int(i * 10));
        }
        for i in 0..90 {
            map.remove(&Value::Int(i));
        }
        let keys: Vec<Value> = map.entries().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, (90..100).map(Value::Int).collect::<Vec<_>>());
        assert_eq!(map.get(&Value::Int(95)), Some(&Value::Int(950)));
    }

    #[test]
    fn hashing_does_not_depend_on_the_run() {
        let mut a = Fnv::default();
        Key(Value::text("unio")).hash(&mut a);
        let mut b = Fnv::default();
        Key(Value::text("unio")).hash(&mut b);
        assert_eq!(a.finish(), b.finish());
        // FNV-1a of the tag 4, the bytes of "unio", and the terminator 0xff.
        assert_eq!(a.finish(), 0xd0b2_323b_fec6_ad65);
        let mut c = Fnv::default();
        Key(Value::text("Unio")).hash(&mut c);
        assert_ne!(a.finish(), c.finish());
    }

    #[test]
    fn copy_on_write_copies_only_shared_values() {
        let original = Value::list(vec![Value::Int(1)]);
        let mut copy = original.clone();
        if let Value::List(items) = &mut copy {
            let items = Rc::make_mut(items);
            items.push(Value::Int(2));
            items.recount();
        }
        match (&original, &copy) {
            (Value::List(a), Value::List(b)) => {
                assert_eq!(a.len(), 1);
                assert_eq!(b.len(), 2);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn float_equality_follows_ieee_but_keys_are_reflexive() {
        assert_ne!(Value::Float(f64::NAN), Value::Float(f64::NAN));
        assert_eq!(Key(Value::Float(f64::NAN)), Key(Value::Float(f64::NAN)));
        assert_eq!(Value::Float(0.0), Value::Float(-0.0));
    }
}
