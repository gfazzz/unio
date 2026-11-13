# Справочник стандартной библиотеки

Версия 0.1-draft. Основная версия на английском: [../stdlib-reference.md](../stdlib-reference.md). Этот файл создаётся программой [tools/reference.unio](../../tools/reference.unio) из комментариев документации [stdlib/](../../stdlib) (требование Д-3); не правьте его вручную. После изменения библиотеки создайте его заново командой `unio run tools/reference.unio -- stdlib ru > docs/ru/stdlib-reference.md`; шаг CI «Generated references are up to date» не проходит, пока файл устарел. Обзор библиотеки, её слоёв и общих правил: [stdlib.md](stdlib.md).

Комментарии документации в исходных текстах библиотеки пишутся на английском языке, поэтому здесь они приведены без перевода; переведены заголовки и пояснения. Каждое публичное объявление библиотеки показано с сигнатурой и комментарием документации в порядке исходного файла: сначала прелюдии слоёв `core`, `alloc` и `std`, затем модули по имени. Закрытые объявления опущены; методы блока `extend` показаны, если они публичны или реализуют интерфейс. Про структуру с закрытыми полями это сказано: её создают только функции её модуля.

Файлов: 30; публичных объявлений и методов: 390, из них без комментария документации: 43.

## Содержание

- [Прелюдия: `core/prelude.unio`](#прелюдия-corepreludeunio)
- [Прелюдия: `core/builtin/bool.unio`](#прелюдия-corebuiltinboolunio)
- [Прелюдия: `core/builtin/float.unio`](#прелюдия-corebuiltinfloatunio)
- [Прелюдия: `core/builtin/int.unio`](#прелюдия-corebuiltinintunio)
- [Прелюдия: `core/builtin/option.unio`](#прелюдия-corebuiltinoptionunio)
- [Прелюдия: `alloc/prelude.unio`](#прелюдия-allocpreludeunio)
- [Прелюдия: `alloc/builtin/bytes.unio`](#прелюдия-allocbuiltinbytesunio)
- [Прелюдия: `alloc/builtin/float.unio`](#прелюдия-allocbuiltinfloatunio)
- [Прелюдия: `alloc/builtin/int.unio`](#прелюдия-allocbuiltinintunio)
- [Прелюдия: `alloc/builtin/list.unio`](#прелюдия-allocbuiltinlistunio)
- [Прелюдия: `alloc/builtin/map.unio`](#прелюдия-allocbuiltinmapunio)
- [Прелюдия: `alloc/builtin/set.unio`](#прелюдия-allocbuiltinsetunio)
- [Прелюдия: `alloc/builtin/text.unio`](#прелюдия-allocbuiltintextunio)
- [Прелюдия: `std/prelude.unio`](#прелюдия-stdpreludeunio)
- [Прелюдия: `std/builtin/path.unio`](#прелюдия-stdbuiltinpathunio)
- [`std.args`](#stdargs)
- [`std.collections`](#stdcollections)
- [`std.decimal`](#stddecimal)
- [`std.env`](#stdenv)
- [`std.errors`](#stderrors)
- [`std.files`](#stdfiles)
- [`std.json`](#stdjson)
- [`std.log`](#stdlog)
- [`std.math`](#stdmath)
- [`std.path`](#stdpath)
- [`std.process`](#stdprocess)
- [`std.random`](#stdrandom)
- [`std.test`](#stdtest)
- [`std.text`](#stdtext)
- [`std.time`](#stdtime)

## Прелюдия: `core/prelude.unio`

Исходный текст: [stdlib/core/prelude.unio](../../stdlib/core/prelude.unio). Слой: `core`.

The core prelude: interfaces, Option, Result and functions that need neither an allocator, nor a GC, nor an
operating system. Every module sees the preludes of the layers its profile provides (concept, section 8.1).

### `Display`

```unio
public interface Display
```

Text form of a value, used by interpolation and by print.

#### `to_text`

```unio
function to_text(self) returns Text
```

### `Error`

```unio
public interface Error
```

Common interface of all errors. Every error declaration implements it; to_text names the type and its fields.

#### `to_text`

```unio
function to_text(self) returns Text
```

#### `diagnostic_code`

```unio
function diagnostic_code(self) returns Text
```

Stable fully qualified error identifier, generated with to_text for every error declaration.
Independent of the language of messages; not a dynamic cast or a recovery mechanism.

### `Equal`

```unio
public interface Equal
```

Equality used by == and !=. Derived for structs and enums whose fields implement it.

#### `equals`

```unio
function equals(self, other as Self) returns Bool
```

### `Order`

```unio
public interface Order
```

Total order: negative if self comes first, zero if equal, positive otherwise.
Antisymmetric and transitive, consistent with Equal. IEEE Float comparisons are separate.

#### `compare`

```unio
function compare(self, other as Self) returns Int
```

### `Hash`

```unio
public interface Hash
```

Hash used by Map keys and Set elements. Float does not implement it.

#### `hash`

```unio
function hash(self) returns Int
```

### `Add`

```unio
public interface Add
```

Operator +.

#### `add`

```unio
function add(self, other as Self) returns Self
```

### `Subtract`

```unio
public interface Subtract
```

Operator -.

#### `subtract`

```unio
function subtract(self, other as Self) returns Self
```

### `Multiply`

```unio
public interface Multiply
```

Operator *.

#### `multiply`

```unio
function multiply(self, other as Self) returns Self
```

### `Divide`

```unio
public interface Divide
```

Operator /.

#### `divide`

```unio
function divide(self, other as Self) returns Self
```

### `Resource`

```unio
public interface Resource
```

A value with an explicit lifetime: created in using and released when the block ends.

#### `release`

```unio
function release(var self)
```

### `Option`

```unio
public enum Option[T]:
    some(value as T)
    none
```

A value that may be absent. The operator or gives a default; ?. passes none along.

### `Result`

```unio
public enum Result[T, E]:
    ok(value as T)
    error(value as E)
```

A result kept as a value. try propagates or handles its error like a call with fails with.

### `assert_fail`

```unio
public external function assert_fail(message as Text) returns Never
```

Ends the program, or the current test, with a fault that carries the message.
Example:

```unio
expect_fault(function (): assert_fail("unreachable"))
```

### `expect_fault`

```unio
public external function expect_fault(action as Function[()])
```

Runs the action and fails the test unless the action ends with a fault. Available only in test blocks.
Example:

```unio
expect_fault(function (): assert_fail("the action ends with a fault"))
```

### `is_same`

```unio
public external function is_same[T with Resource](a as T, b as T) returns Bool
```

Whether two names refer to the same resource; resources have no ==.
Example:

```unio
using first = ExampleResource(id = 1):
    using second = ExampleResource(id = 2):
        assert is_same(first, first) and not is_same(first, second)
```

### `min`

```unio
public function min[T with Order](a as T, b as T) returns T
```

The smaller of two values; a when they are equal.
Example:

```unio
assert min(3, 2) == 2 and min("b", "a") == "a"
```

### `max`

```unio
public function max[T with Order](a as T, b as T) returns T
```

The larger of two values; a when they are equal.
Example:

```unio
assert max(3, 2) == 3
```

### `Iterator`

```unio
public interface Iterator[T]
```

A single-pass cursor. none is permanent; next never produces another item after it.

#### `next`

```unio
function next(var self) returns Option[T]
```

### `Range`

```unio
public struct Range
```

A compact half-open integer range. Construction and iteration need no allocation.

У структуры есть закрытые поля: её создают только функции её модуля.

### `RangeIterator`

```unio
public struct RangeIterator
```

У структуры есть закрытые поля: её создают только функции её модуля.

### `range`

```unio
public function range(start as Int, end as Int) returns Range
```

start >= end gives an empty range; no subtraction of the bounds is required.
Example:

```unio
var total = 0
for n in range(1, 4):
    total = total + n
assert total == 6
var empty = range(5, 2).iterator()
assert empty.next() == none
```

### Методы `Range`

```unio
extend Range
```

#### `iterator`

```unio
public function iterator(self) returns RangeIterator
```

Each traversal has an independent cursor.
Example:

```unio
var cursor = range(0, 2).iterator()
assert cursor.next() == some(0) and cursor.next() == some(1) and cursor.next() == none
```

### `RangeIterator` реализует `Iterator[Int]`

```unio
extend RangeIterator with Iterator[Int]
```

#### `next`

```unio
function next(var self) returns Option[Int]
```

### `ExampleResource` реализует `Resource`

```unio
extend ExampleResource with Resource
```

#### `release`

```unio
function release(var self)
```

## Прелюдия: `core/builtin/bool.unio`

Исходный текст: [stdlib/core/builtin/bool.unio](../../stdlib/core/builtin/bool.unio). Слой: `core`.

Methods of Bool. The operators and, or and not are built into the language.

### `Bool` реализует `Display`

```unio
extend Bool with Display
```

#### `to_text`

```unio
function to_text(self) returns Text
```

## Прелюдия: `core/builtin/float.unio`

Исходный текст: [stdlib/core/builtin/float.unio](../../stdlib/core/builtin/float.unio). Слой: `core`.

Methods of Float, the 64-bit IEEE 754 number. Float has no Hash: it is not a Map key, a Set element or a match pattern.

### Методы `Float`

```unio
extend Float
```

#### `is_nan`

```unio
public external function is_nan(self) returns Bool
```

True for every NaN, the value that is not equal to itself.
Example:

```unio
assert (0.0 * (1e308 * 10.0)).is_nan() and not 1.0.is_nan()
```

#### `is_finite`

```unio
public external function is_finite(self) returns Bool
```

True unless the number is infinite or NaN.
Example:

```unio
assert 1.5.is_finite() and not (1e308 * 10.0).is_finite()
```

#### `to_int`

```unio
public external function to_int(self) returns Option[Int]
```

The integer equal to the number; none when it has a fraction, is infinite or NaN, or lies
outside MINIMUM_INT..MAXIMUM_INT. Round first to choose how: std.math.floor(x).to_int().
Example:

```unio
assert 3.0.to_int() == some(3) and 2.5.to_int() == none
```

#### `to_bits`

```unio
public external function to_bits(self) returns Int
```

The IEEE 754 binary64 bits of the number, read as a two's-complement Int; std.math.from_bits
is the inverse.
Example:

```unio
assert 1.0.to_bits() == 4607182418800017408 and (-0.0).to_bits() == MINIMUM_INT
```

#### `abs`

```unio
public function abs(self) returns Float
```

The absolute value: the number without its sign, so (-0.0).abs() is 0.0; NaN stays NaN.
Example:

```unio
assert (-2.5).abs() == 2.5
```

#### `total_order`

```unio
public function total_order(self) returns OrderedFloat
```

An explicit total-order key: numeric order, both zeros equal, all NaNs equal and last.
Example:

```unio
assert (-0.0).total_order() == 0.0.total_order()
```

### `OrderedFloat`

```unio
public struct OrderedFloat
```

Equality is derived from the private canonical integer key, not IEEE Float equality.

У структуры есть закрытые поля: её создают только функции её модуля.

### `OrderedFloat` реализует `Order`

```unio
extend OrderedFloat with Order
```

#### `compare`

```unio
function compare(self, other as Self) returns Int
```

### `OrderedFloat` реализует `Hash`

```unio
extend OrderedFloat with Hash
```

#### `hash`

```unio
function hash(self) returns Int
```

## Прелюдия: `core/builtin/int.unio`

Исходный текст: [stdlib/core/builtin/int.unio](../../stdlib/core/builtin/int.unio). Слой: `core`.

Methods of Int, the 64-bit signed integer. Arithmetic and comparison are built into the compiler; overflow is a fault.

### `MINIMUM_INT`

```unio
public let MINIMUM_INT as Int = -9223372036854775807 - 1
```

The smallest Int.
Example:

```unio
assert MINIMUM_INT == -9223372036854775807 - 1
```

### `MAXIMUM_INT`

```unio
public let MAXIMUM_INT as Int = 9223372036854775807
```

The largest Int.
Example:

```unio
assert MAXIMUM_INT.checked_add(1) == none
```

### Методы `Int`

```unio
extend Int
```

#### `checked_add`

```unio
public external function checked_add(self, other as Int) returns Option[Int]
```

Sum, or none on overflow.
Example:

```unio
assert 2.checked_add(3) == some(5) and MAXIMUM_INT.checked_add(1) == none
```

#### `checked_subtract`

```unio
public external function checked_subtract(self, other as Int) returns Option[Int]
```

Difference, or none on overflow.
Example:

```unio
assert 2.checked_subtract(3) == some(-1) and MINIMUM_INT.checked_subtract(1) == none
```

#### `checked_multiply`

```unio
public external function checked_multiply(self, other as Int) returns Option[Int]
```

Product, or none on overflow.
Example:

```unio
assert 6.checked_multiply(7) == some(42) and MAXIMUM_INT.checked_multiply(2) == none
```

#### `checked_divide`

```unio
public function checked_divide(self, other as Int) returns Option[Int]
```

Quotient truncated toward zero, or none when other is 0 or the result does not fit.
Example:

```unio
assert 7.checked_divide(2) == some(3) and 1.checked_divide(0) == none
```

#### `to_float`

```unio
public external function to_float(self) returns Float
```

The nearest Float (round to nearest, ties to even).
Example:

```unio
assert 3.to_float() == 3.0
```

#### `abs`

```unio
public function abs(self) returns Int
```

The absolute value; MINIMUM_INT, whose absolute value does not fit, is a fault like overflow.
Example:

```unio
assert (-5).abs() == 5 and 5.abs() == 5
```

#### `checked_abs`

```unio
public function checked_abs(self) returns Option[Int]
```

The absolute value, or none for MINIMUM_INT.
Example:

```unio
assert (-5).checked_abs() == some(5) and MINIMUM_INT.checked_abs() == none
```

#### `sign`

```unio
public function sign(self) returns Int
```

-1 for a negative number, 0 for zero, 1 for a positive number.
Example:

```unio
assert (-8).sign() == -1 and 0.sign() == 0 and 3.sign() == 1
```

## Прелюдия: `core/builtin/option.unio`

Исходный текст: [stdlib/core/builtin/option.unio](../../stdlib/core/builtin/option.unio). Слой: `core`.

Methods of Option[T]. The operator or gives a default value; ?. passes none along a chain.

### Методы `Option[T]`

```unio
extend[T] Option[T]
```

#### `is_some`

```unio
public function is_some(self) returns Bool
```

Whether a value is present.
Example:

```unio
let missing as Option[Int] = none
assert some(1).is_some() and not missing.is_some()
```

#### `is_none`

```unio
public function is_none(self) returns Bool
```

Whether the value is absent.
Example:

```unio
let missing as Option[Int] = none
assert missing.is_none()
```

## Прелюдия: `alloc/prelude.unio`

Исходный текст: [stdlib/alloc/prelude.unio](../../stdlib/alloc/prelude.unio). Слой: `alloc`.

Explicit materialization belongs to alloc; range itself belongs to core.

### Методы `Range`

```unio
extend Range
```

#### `to_list`

```unio
public function to_list(self) returns List[Int]
```

Allocates a list containing every integer of the range.
Example:

```unio
assert range(1, 4).to_list() == [1, 2, 3]
```

## Прелюдия: `alloc/builtin/bytes.unio`

Исходный текст: [stdlib/alloc/builtin/bytes.unio](../../stdlib/alloc/builtin/bytes.unio). Слой: `alloc`.

Bytes: an immutable sequence of bytes, for file contents that are not text.

### `Bytes`

```unio
public external function Bytes() returns Bytes
```

An empty sequence of bytes.
Example:

```unio
assert Bytes().length() == 0
```

### Методы `Bytes`

```unio
extend Bytes
```

#### `length`

```unio
public external function length(self) returns Int
```

The number of bytes.
Example:

```unio
assert Bytes().length() == 0
```

#### `to_text`

```unio
public external function to_text(self) returns Option[Text]
```

The bytes as text; none if they are not valid UTF-8.
Example:

```unio
assert Bytes().to_text() == some("")
```

#### `is_empty`

```unio
public function is_empty(self) returns Bool
```

Whether there are no bytes.
Example:

```unio
assert Bytes().is_empty()
```

## Прелюдия: `alloc/builtin/float.unio`

Исходный текст: [stdlib/alloc/builtin/float.unio](../../stdlib/alloc/builtin/float.unio). Слой: `alloc`.

Text form of Float; producing new text needs an allocator.

### `Float` реализует `Display`

```unio
extend Float with Display
```

#### `to_text`

```unio
external function to_text(self) returns Text
```

The shortest text that reads back as the same number, such as "0.1" or "1e+300".

## Прелюдия: `alloc/builtin/int.unio`

Исходный текст: [stdlib/alloc/builtin/int.unio](../../stdlib/alloc/builtin/int.unio). Слой: `alloc`.

Text form of Int; producing new text needs an allocator.

### `Int` реализует `Display`

```unio
extend Int with Display
```

#### `to_text`

```unio
external function to_text(self) returns Text
```

## Прелюдия: `alloc/builtin/list.unio`

Исходный текст: [stdlib/alloc/builtin/list.unio](../../stdlib/alloc/builtin/list.unio). Слой: `alloc`.

Methods of List[T]. items[index] outside 0..length() is a fault; get is its safe form.
Methods with var self change the list and are available only on a var place.

### Методы `List[T]`

```unio
extend[T] List[T]
```

#### `length`

```unio
public external function length(self) returns Int
```

The number of items.
Example:

```unio
assert [1, 2, 3].length() == 3
```

#### `get`

```unio
public external function get(self, index as Int) returns Option[T]
```

The item at the position; none outside 0..length(), where items[index] is a fault.
Example:

```unio
assert [10, 20].get(1) == some(20) and [10, 20].get(2) == none
```

#### `append`

```unio
public external function append(var self, item as T)
```

Adds the item at the end in amortized constant time.
Example:

```unio
var items = [1]
items.append(2)
assert items == [1, 2]
```

#### `remove_last`

```unio
public external function remove_last(var self) returns Option[T]
```

Removes and returns the last item; none for an empty list.
Example:

```unio
var items = [1, 2]
assert items.remove_last() == some(2) and items == [1]
```

#### `is_empty`

```unio
public function is_empty(self) returns Bool
```

Whether the list has no items.
Example:

```unio
let empty as List[Int] = []
assert empty.is_empty() and not [0].is_empty()
```

#### `first`

```unio
public function first(self) returns Option[T]
```

The first item; none for an empty list.
Example:

```unio
assert ["a", "b"].first() == some("a")
```

#### `last`

```unio
public function last(self) returns Option[T]
```

The last item; none for an empty list.
Example:

```unio
assert ["a", "b"].last() == some("b")
```

#### `slice`

```unio
public function slice(self, start as Int, end as Int) returns List[T]
```

Items from start up to, but not including, end; a fault outside 0..length().
Example:

```unio
assert [1, 2, 3, 4].slice(1, 3) == [2, 3]
```

#### `append_all`

```unio
public function append_all(var self, items as List[T])
```

Adds the items at the end, in order.
Example:

```unio
var items = [1]
items.append_all([2, 3])
assert items == [1, 2, 3]
```

#### `find`

```unio
public function find(self, predicate as Function[(T) -> Bool]) returns Option[T]
```

The first item for which the predicate is true; none if there is none.
Example:

```unio
assert [1, 4, 6].find(function (n as Int) returns Bool: n % 2 == 0) == some(4)
```

#### `find_index`

```unio
public function find_index(self, predicate as Function[(T) -> Bool]) returns Option[Int]
```

The position of the first item for which the predicate is true; none if there is none.
Example:

```unio
assert ["a", "b"].find_index(function (s as Text) returns Bool: s == "b") == some(1)
```

#### `filter`

```unio
public function filter(self, predicate as Function[(T) -> Bool]) returns List[T]
```

The items for which the predicate is true, in order.
Example:

```unio
assert [1, 2, 3, 4].filter(function (n as Int) returns Bool: n > 2) == [3, 4]
```

#### `map`

```unio
public function map[U](self, change as Function[(T) -> U]) returns List[U]
```

The results of change for every item, in order.
Example:

```unio
assert [1, 2].map(function (n as Int) returns Text: "#{n}") == ["#1", "#2"]
```

#### `try_map`

```unio
public function try_map[U, E with Error](
    self,
    change as Function[(T) -> U fails with E],
) returns List[U] fails with E
```

Like map, propagating the first callback error; later items are not visited.
Example:

```unio
let doubled = try [1, 2].try_map(positive) on ItemError as e:
    assert_fail("unexpected {e.item}")
assert doubled == [2, 4]
```

#### `try_filter`

```unio
public function try_filter[E with Error](
    self,
    predicate as Function[(T) -> Bool fails with E],
) returns List[T] fails with E
```

Like filter, propagating the first callback error.
Example:

```unio
let even = try [1, 2, 3, 4].try_filter(positive_even) on ItemError as e:
    assert_fail("unexpected {e.item}")
assert even == [2, 4]
```

#### `fold`

```unio
public function fold[U](self, initial as U, combine as Function[(U, T) -> U]) returns U
```

Left fold in input order; an empty list returns initial without calling combine.
Example:

```unio
assert [1, 2, 3].fold(0, function (sum as Int, n as Int) returns Int: sum + n) == 6
```

#### `try_fold`

```unio
public function try_fold[U, E with Error](
    self,
    initial as U,
    combine as Function[(U, T) -> U fails with E],
) returns U fails with E
```

Left fold, stopping at the first callback error.
Example:

```unio
let sum = try [1, 2].try_fold(10, add_positive) on ItemError as e:
    assert_fail("unexpected {e.item}")
assert sum == 16
```

#### `partition`

```unio
public function partition(
    self,
    predicate as Function[(T) -> Bool],
) returns Tuple[List[T], List[T]]
```

Matching items, then nonmatching items; each list keeps input order.
Example:

```unio
assert [1, 2, 3, 4].partition(function (n as Int) returns Bool: n % 2 == 0) == ([2, 4], [1, 3])
```

#### `group_by`

```unio
public function group_by[K with Hash and Equal](
    self,
    key as Function[(T) -> K],
) returns Map[K, List[T]]
```

Groups and their items keep first-occurrence order; key is called once per item.
Example:

```unio
let groups = ["apple", "bean", "avocado"].group_by(function (s as Text) returns Text: s.char_at(0))
assert groups == {"a": ["apple", "avocado"], "b": ["bean"]}
```

#### `zip`

```unio
public function zip[U](self, other as List[U]) returns Option[List[Tuple[T, U]]]
```

Equal-length lists paired in order; none for different lengths, never silent truncation.
Example:

```unio
assert [1, 2].zip(["a", "b"]) == some([(1, "a"), (2, "b")]) and [1].zip(["a", "b"]) == none
```

#### `chunks`

```unio
public function chunks(self, size as Int) returns Option[List[List[T]]]
```

Consecutive nonempty blocks, with a possibly shorter last block; none unless size > 0.
Example:

```unio
assert [1, 2, 3, 4, 5].chunks(2) == some([[1, 2], [3, 4], [5]])
```

#### `windows`

```unio
public function windows(self, size as Int) returns Option[List[List[T]]]
```

Overlapping complete windows, advancing by one; none unless size > 0.
Example:

```unio
assert [1, 2, 3].windows(2) == some([[1, 2], [2, 3]])
```

#### `enumerate`

```unio
public function enumerate(self, start as Int = 0) returns List[Tuple[Int, T]]
```

Pairs of a position, counted from start, and an item.
Example:

```unio
assert ["a", "b"].enumerate(start = 1) == [(1, "a"), (2, "b")]
```

#### `reversed`

```unio
public function reversed(self) returns List[T]
```

The items in the opposite order.
Example:

```unio
assert [1, 2, 3].reversed() == [3, 2, 1]
```

#### `sorted_with`

```unio
public function sorted_with(self, comes_before as Function[(T, T) -> Bool]) returns List[T]
```

A sorted list where comes_before(a, b) means that a goes before b; equal items keep their order.
Merge sort, O(n log n).
Example:

```unio
assert [1, 3, 2].sorted_with(function (a as Int, b as Int) returns Bool: a > b) == [3, 2, 1]
```

#### `sorted_by`

```unio
public function sorted_by[K with Order](self, key as Function[(T) -> K]) returns List[T]
```

Stable sorting; key is evaluated exactly once per item, in input order, even for a singleton.
O(n) additional key storage. Example: names.sorted_by(function (name as Text) returns Int: name.length()).
Example:

```unio
assert ["ccc", "a", "bb"].sorted_by(function (s as Text) returns Int: s.length()) == ["a", "bb", "ccc"]
```

### `ListIterator`

```unio
public struct ListIterator[T]
```

A value snapshot: later changes to the original list do not change the traversal.

У структуры есть закрытые поля: её создают только функции её модуля.

### Методы `List[T]`

```unio
extend[T] List[T]
```

#### `iterator`

```unio
public function iterator(self) returns ListIterator[T]
```

A cursor over a value snapshot of the list: later changes to the list do not change it.
Example:

```unio
var cursor = [1, 2].iterator()
assert cursor.next() == some(1) and cursor.next() == some(2) and cursor.next() == none
```

### `ListIterator[T]` реализует `Iterator[T]`

```unio
extend[T] ListIterator[T] with Iterator[T]
```

#### `next`

```unio
function next(var self) returns Option[T]
```

### Методы `List[T]`

```unio
extend[T with Equal] List[T]
```

#### `contains`

```unio
public function contains(self, item as T) returns Bool
```

Used by "item in list".
Example:

```unio
assert 2 in [1, 2] and not [1, 2].contains(3)
```

### Методы `List[T]`

```unio
extend[T with Order] List[T]
```

#### `max`

```unio
public function max(self) returns Option[T]
```

The largest item, the first of equal largest ones; none for an empty list.
Example:

```unio
assert [3, 7, 5].max() == some(7)
```

#### `sorted`

```unio
public function sorted(self) returns List[T]
```

A sorted list; equal items keep their order.
Example:

```unio
assert ["b", "c", "a"].sorted() == ["a", "b", "c"]
```

## Прелюдия: `alloc/builtin/map.unio`

Исходный текст: [stdlib/alloc/builtin/map.unio](../../stdlib/alloc/builtin/map.unio). Слой: `alloc`.

Methods of Map[K, V]. Entries keep the order in which their keys were first inserted.

### Методы `Map[K, V]`

```unio
extend[K with Hash and Equal, V] Map[K, V]
```

#### `length`

```unio
public external function length(self) returns Int
```

The number of entries.
Example:

```unio
assert {"a": 1, "b": 2}.length() == 2
```

#### `get`

```unio
public external function get(self, key as K) returns Option[V]
```

The value for the key; none if the key is absent.
Example:

```unio
let ages = {"ana": 31}
assert ages.get("ana") == some(31) and ages.get("bo") == none
```

#### `put`

```unio
public external function put(var self, key as K, value as V)
```

Sets the value for the key; an existing key keeps its position.
Example:

```unio
var ages = {"ana": 31}
ages.put("bo", 25)
ages.put("ana", 32)
assert ages.entries() == [("ana", 32), ("bo", 25)]
```

#### `remove`

```unio
public external function remove(var self, key as K) returns Option[V]
```

Removes the key and returns its value; none if the key is absent.
Example:

```unio
var ages = {"ana": 31}
assert ages.remove("ana") == some(31) and ages.remove("ana") == none
```

#### `entries`

```unio
public external function entries(self) returns List[Tuple[K, V]]
```

Pairs of a key and a value in insertion order.
Example:

```unio
assert {"b": 2, "a": 1}.entries() == [("b", 2), ("a", 1)]
```

#### `is_empty`

```unio
public function is_empty(self) returns Bool
```

Whether the map has no entries.
Example:

```unio
let empty as Map[Text, Int] = {}
assert empty.is_empty()
```

#### `contains`

```unio
public function contains(self, key as K) returns Bool
```

Used by "key in map".
Example:

```unio
assert "a" in {"a": 1} and "b" not in {"a": 1}
```

#### `keys`

```unio
public function keys(self) returns List[K]
```

The keys in insertion order.
Example:

```unio
assert {"b": 2, "a": 1}.keys() == ["b", "a"]
```

#### `values`

```unio
public function values(self) returns List[V]
```

The values in the insertion order of their keys.
Example:

```unio
assert {"b": 2, "a": 1}.values() == [2, 1]
```

## Прелюдия: `alloc/builtin/set.unio`

Исходный текст: [stdlib/alloc/builtin/set.unio](../../stdlib/alloc/builtin/set.unio). Слой: `alloc`.

Set[T]: distinct values in the order they were first inserted. Elements implement Hash and Equal,
so Float cannot be an element.

### `Set`

```unio
public external function Set[T with Hash and Equal]() returns Set[T]
```

An empty set: Set[Text]\().
Example:

<pre><code>var names = Set[Text]&#40;)
names.insert("ana")
assert names.length() == 1</code></pre>

### Методы `Set[T]`

```unio
extend[T with Hash and Equal] Set[T]
```

#### `length`

```unio
public external function length(self) returns Int
```

The number of elements.
Example:

```unio
assert [1, 2, 2].to_set().length() == 2
```

#### `contains`

```unio
public external function contains(self, item as T) returns Bool
```

Used by "item in set".
Example:

```unio
assert 2 in [1, 2].to_set() and 3 not in [1, 2].to_set()
```

#### `insert`

```unio
public external function insert(var self, item as T)
```

Adds the item; an item already present keeps its position.
Example:

```unio
var seen = [3].to_set()
seen.insert(1)
seen.insert(3)
assert seen.items() == [3, 1]
```

#### `remove`

```unio
public external function remove(var self, item as T) returns Bool
```

Removes the item and tells whether it was present.
Example:

```unio
var seen = [1, 2].to_set()
assert seen.remove(1) and not seen.remove(1)
```

#### `items`

```unio
public external function items(self) returns List[T]
```

The elements in insertion order.
Example:

```unio
assert ["b", "a", "b"].to_set().items() == ["b", "a"]
```

#### `is_empty`

```unio
public function is_empty(self) returns Bool
```

Whether the set has no elements.
Example:

<pre><code>assert Set[Int]&#40;).is_empty()</code></pre>

#### `union`

```unio
public function union(self, other as Set[T]) returns Set[T]
```

The elements of both sets: these first, then the new ones of other, each in insertion order.
Example:

```unio
assert [1, 2].to_set().union([2, 3].to_set()).items() == [1, 2, 3]
```

#### `intersection`

```unio
public function intersection(self, other as Set[T]) returns Set[T]
```

The elements of this set that are also in other, in the order of this set.
Example:

```unio
assert [1, 2, 3].to_set().intersection([3, 1].to_set()).items() == [1, 3]
```

#### `difference`

```unio
public function difference(self, other as Set[T]) returns Set[T]
```

The elements of this set that are not in other, in the order of this set.
Example:

```unio
assert [1, 2, 3].to_set().difference([2].to_set()).items() == [1, 3]
```

### Методы `List[T]`

```unio
extend[T with Hash and Equal] List[T]
```

#### `to_set`

```unio
public function to_set(self) returns Set[T]
```

The distinct items of the list, in the order of their first occurrence.
Example:

```unio
assert [3, 1, 3].to_set().items() == [3, 1]
```

## Прелюдия: `alloc/builtin/text.unio`

Исходный текст: [stdlib/alloc/builtin/text.unio](../../stdlib/alloc/builtin/text.unio). Слой: `alloc`.

Methods of Text: a sequence of Unicode scalar values stored as valid UTF-8.
Positions and lengths count scalars, not grapheme clusters; byte_length counts bytes.
Equality and hashing compare the exact scalar sequence; no implicit normalization.
Equality, order, hashing and + are built into the compiler; "part in text" calls text.contains(part).

### `Text` реализует `Display`

```unio
extend Text with Display
```

#### `to_text`

```unio
function to_text(self) returns Text
```

### Методы `Text`

```unio
extend Text
```

#### `length`

```unio
public external function length(self) returns Int
```

The number of Unicode scalars; O(1).
Example:

```unio
assert "мир".length() == 3 and "e\u{301}".length() == 2
```

#### `byte_length`

```unio
public external function byte_length(self) returns Int
```

The number of bytes of the UTF-8 encoding.
Example:

```unio
assert "мир".byte_length() == 6
```

#### `chars`

```unio
public external function chars(self) returns List[Text]
```

The characters, each as a Text of length 1.
Example:

```unio
assert "ab".chars() == ["a", "b"]
```

#### `code_points`

```unio
public external function code_points(self) returns List[Int]
```

The Unicode code points of the characters.
Example:

```unio
assert "A\u{E9}".code_points() == [65, 233]
```

#### `slice`

```unio
public external function slice(self, start as Int, end as Int) returns Text
```

Scalars from start up to, but not including, end; a fault outside 0..length().
O(1) after the shared O(n)-time, O(n)-space scalar index is constructed; slicing shares storage.
Example:

```unio
assert "привет".slice(0, 3) == "при"
```

#### `index_of`

```unio
public external function index_of(self, part as Text, start as Int = 0) returns Option[Int]
```

Position of the first occurrence of part at or after start. An empty part is found at start.
Example:

```unio
assert "banana".index_of("an") == some(1) and "banana".index_of("an", start = 2) == some(3)
assert "banana".index_of("x") == none
```

#### `to_lower`

```unio
public external function to_lower(self) returns Text
```

Lower case by the Unicode rules.
Example:

```unio
assert "ÉCOLE".to_lower() == "école"
```

#### `to_upper`

```unio
public external function to_upper(self) returns Text
```

Upper case by the Unicode rules.
Example:

```unio
assert "straße".to_upper() == "STRASSE"
```

#### `to_float`

```unio
public external function to_float(self) returns Option[Float]
```

The number written in decimal or scientific notation; none for anything else.
Example:

```unio
assert "2.5e3".to_float() == some(2500.0) and "x".to_float() == none
```

#### `char_at`

```unio
public function char_at(self, index as Int) returns Text
```

The character at the position as a Text of length 1; a fault outside 0..length().
O(1) after the scalar index is constructed, as for slice.
Example:

```unio
assert "héllo".char_at(1) == "é"
```

#### `is_empty`

```unio
public function is_empty(self) returns Bool
```

Whether the text has no characters.
Example:

```unio
assert "".is_empty() and not " ".is_empty()
```

#### `contains`

```unio
public external function contains(self, part as Text) returns Bool
```

Whether part occurs in the text; an empty part occurs in every text.
Example:

```unio
assert "hello".contains("ell") and "ell" in "hello"
```

#### `starts_with`

```unio
public external function starts_with(self, prefix as Text) returns Bool
```

Whether the text begins with the prefix; every text begins with "".
Example:

```unio
assert "unio.toml".starts_with("unio")
```

#### `ends_with`

```unio
public external function ends_with(self, suffix as Text) returns Bool
```

Whether the text ends with the suffix; every text ends with "".
Example:

```unio
assert "unio.toml".ends_with(".toml")
```

#### `count`

```unio
public external function count(self, part as Text) returns Int
```

Number of non-overlapping occurrences of part, from the start; part must not be empty.
Example:

```unio
assert "a,b,c".count(",") == 2
```

#### `replace`

```unio
public external function replace(self, old as Text, new as Text) returns Text
```

Every non-overlapping occurrence of old, from the start, replaced with new; old must not
be empty.
Example:

```unio
assert "a-b-c".replace("-", "+") == "a+b+c"
```

#### `lines`

```unio
public external function lines(self) returns List[Text]
```

Lines split on "\n" and "\r\n"; one final empty line is dropped, so "a\n" has one line.
Example:

```unio
assert "one\r\ntwo\n".lines() == ["one", "two"]
```

#### `split_whitespace`

```unio
public external function split_whitespace(self) returns List[Text]
```

Words separated by white space (space, tab, line ends, vertical tab, form feed).
Example:

```unio
assert " to  be\tor ".split_whitespace() == ["to", "be", "or"]
```

#### `trim`

```unio
public external function trim(self) returns Text
```

The text without white space at the start and at the end.
Example:

```unio
assert "  hi \n".trim() == "hi"
```

#### `trim_start`

```unio
public external function trim_start(self) returns Text
```

The text without white space at the start.
Example:

```unio
assert "  hi ".trim_start() == "hi "
```

#### `trim_end`

```unio
public external function trim_end(self) returns Text
```

The text without white space at the end.
Example:

```unio
assert "  hi ".trim_end() == "  hi"
```

#### `repeat`

```unio
public external function repeat(self, times as Int) returns Text
```

The text repeated the given number of times; a negative count is a fault.
Example:

```unio
assert "ab".repeat(3) == "ababab"
```

#### `to_int`

```unio
public function to_int(self) returns Option[Int]
```

A decimal integer with an optional sign; none for anything else, including overflow.
Example:

```unio
assert "-42".to_int() == some(-42) and "4.2".to_int() == none
assert "9223372036854775808".to_int() == none
```

## Прелюдия: `std/prelude.unio`

Исходный текст: [stdlib/std/prelude.unio](../../stdlib/std/prelude.unio). Слой: `std`.

The std prelude: functions that need the operating system.

### `print`

```unio
public function print(text as Text)
```

Writes the text and a line end to standard output.
Example:

```unio
print("hello")
```

### `print_error`

```unio
public function print_error(text as Text)
```

Writes the text and a line end to standard error.
Example:

```unio
print_error("warning: nothing to do")
```

## Прелюдия: `std/builtin/path.unio`

Исходный текст: [stdlib/std/builtin/path.unio](../../stdlib/std/builtin/path.unio). Слой: `std`.

Lexical paths. "/" is accepted on every platform; "\\" separates components only on Windows.
Anchors preserve POSIX roots, Windows drive-relative/rooted paths and UNC server/share roots.

### `Path`

```unio
public external function Path(text as Text) returns Path
```

A path from its text, as given; nothing is checked or normalized.
Example:

```unio
assert Path("a/b").to_text() == "a/b"
```

### `Path` реализует `Display`

```unio
extend Path with Display
```

#### `to_text`

```unio
external function to_text(self) returns Text
```

### Методы `Path`

```unio
extend Path
```

#### `components`

```unio
public function components(self) returns List[Text]
```

The anchor, if present, then names. Examples: ["/", "a"], ["C:", "a"], ["C:/", "a"].
UNC roots are one anchor such as "//server/share/", never ordinary directory names.
Example:

```unio
assert Path("/usr/lib").components() == ["/", "usr", "lib"]
```

#### `is_absolute`

```unio
public function is_absolute(self) returns Bool
```

Whether the path starts at a root: "/" on Linux and macOS; a drive with a root ("C:\\") or a
UNC share on Windows.
Example:

```unio
assert not Path("a/b").is_absolute()
```

#### `name`

```unio
public function name(self) returns Text
```

The last name; "" for an empty path, a drive anchor or a root.
Example:

```unio
assert Path("dir/file.txt").name() == "file.txt"
```

#### `extension`

```unio
public function extension(self) returns Text
```

Text after the last dot; a leading dot alone does not introduce an extension.
Example:

```unio
assert Path("a/report.tar.gz").extension() == "gz" and Path(".profile").extension() == ""
```

#### `parent`

```unio
public function parent(self) returns Option[Path]
```

Remove the last name; none at a root/drive anchor. UNC parents never leave the share.
Example:

```unio
assert Path("a/b.txt").parent() == some(Path("a"))
```

#### `join`

```unio
public function join(self, part as Text) returns Path
```

Append text without changing the anchor. This is not a sandbox containment check.
Example:

```unio
assert Path("a").join("b.txt") == Path("a{separator()}b.txt")
```

#### `normalize`

```unio
public function normalize(self) returns Path
```

Remove "." and redundant separators, preserving ".." and the anchor.
This does not resolve symlinks or prove filesystem equivalence; use files.resolve for resolution.
Example:

```unio
assert Path("a/./b//c").normalize() == Path("a/b/c")
```

## `std.args`

Исходный текст: [stdlib/std/args.unio](../../stdlib/std/args.unio). Слой: `std`.

Program arguments (requirements S-1, S-4): the raw list, and a parser of positional arguments,
flags and options with defaults and an automatic --help.

A program declares its parameters once; parse checks the arguments against them and help writes
the usage text from the same declarations, so the two cannot disagree:

```unio
let cli = parser("copy", "Copies a file.", [
    flag("verbose", "Print what is copied.", short = "v"),
    option("mode", "File mode of the copy.", default = some("644")),
    positional("source", "The file to read."),
    positional("target", "Where to write.", default = some("out.txt")),
])
let arguments = try parse(cli, list()) on ArgsError as e:
    ...
```

The syntax: "--name", "--name value" and "--name=value" for long names; "-x" and "-x value" for
short names, and "-abc" for several short flags; "--" ends the options, so what follows is
positional even when it starts with "-"; a lone "-" is positional. "--help" and "-h" give an
ArgsError of kind HelpRequested whose message is the help text, unless the program declares a
parameter of that name itself.

### `list`

```unio
public external function list() returns List[Text]
```

The arguments of the program without its name.
Example:

```unio
assert list().length() >= 0
```

### `Parameter`

```unio
public struct Parameter
```

One declared parameter; made by flag, option, positional or rest.

У структуры есть закрытые поля: её создают только функции её модуля.

### `Parser`

```unio
public struct Parser
```

The declared parameters of a program with its name and description.

У структуры есть закрытые поля: её создают только функции её модуля.

### `ArgsErrorKind`

```unio
public enum ArgsErrorKind:
    HelpRequested
    UnknownOption
    MissingValue
    UnexpectedValue
    MissingArgument
    UnexpectedArgument
    InvalidValue
```

What went wrong, for a program that wants to react to each case.

### `ArgsError`

```unio
public error ArgsError:
    kind as ArgsErrorKind
    message as Text
```

Arguments that do not match the declared parameters, or a request for help. message says what
is wrong in one line followed by the usage line, or is the whole help text for HelpRequested.

### `Arguments`

```unio
public struct Arguments
```

The result of parse: the value of every declared parameter.

У структуры есть закрытые поля: её создают только функции её модуля.

### `flag`

```unio
public function flag(name as Text, help as Text, named short as Text = "") returns Parameter
```

A flag: present or absent, never with a value. short is one character or "".
Example:

```unio
let cli = parser("ls", "Lists.", [flag("all", "Show hidden files.", short = "a")])
let arguments = try parse(cli, ["-a"]) on ArgsError as e:
    assert_fail(e.message)
assert arguments.flag("all")
```

### `option`

```unio
public function option(
    name as Text,
    help as Text,
    named short as Text = "",
    named default as Option[Text] = none,
) returns Parameter
```

An option with a value. Without a default it may be left out, and get gives none; repeated, the
last value counts for get and value, and all of them for values.
Example:

```unio
let cli = parser("serve", "Serves.", [option("port", "Port to listen on.", default = some("8080"))])
let arguments = try parse(cli, []) on ArgsError as e:
    assert_fail(e.message)
assert arguments.value("port") == "8080"
```

### `positional`

```unio
public function positional(
    name as Text,
    help as Text,
    named default as Option[Text] = none,
) returns Parameter
```

A positional argument, taken in the order of declaration. Without a default it is required;
with one it is optional, and every positional after it must be optional too.
Example:

```unio
let cli = parser("cat", "Prints a file.", [positional("file", "The file.")])
let arguments = try parse(cli, ["notes.txt"]) on ArgsError as e:
    assert_fail(e.message)
assert arguments.value("file") == "notes.txt"
```

### `rest`

```unio
public function rest(name as Text, help as Text) returns Parameter
```

The positional arguments that remain after the declared ones, possibly none; at most one, and
the last parameter.
Example:

```unio
let cli = parser("rm", "Removes files.", [rest("files", "The files.")])
let arguments = try parse(cli, ["a", "b"]) on ArgsError as e:
    assert_fail(e.message)
assert arguments.values("files") == ["a", "b"]
```

### `parser`

```unio
public function parser(
    program as Text,
    about as Text,
    parameters as List[Parameter],
) returns Parser
```

The declared parameters of a program. A declaration that cannot work is a fault: an empty or
repeated name or short name, a short name longer than one character, a required positional
after an optional one, or a rest that is not the last parameter.
Example:

```unio
let cli = parser("greet", "Greets someone.", [positional("name", "Who to greet.")])
assert help(cli).starts_with("Usage: greet [options] <name>")
```

### `help`

```unio
public function help(parser as Parser) returns Text
```

The usage text: the usage line, the description, and every parameter with its help and default.
--help and -h give it unless the program declares parameters of those names.
Example:

```unio
let cli = parser("wc", "Counts words.", [flag("lines", "Count lines.", short = "l")])
assert help(cli).contains("  -l, --lines  Count lines.")
```

### `parse`

```unio
public function parse(
    parser as Parser,
    arguments as List[Text],
) returns Arguments fails with ArgsError
```

Checks the arguments against the declared parameters.
Example:

```unio
let cli = parser("cp", "Copies.", [flag("force", "Overwrite.", short = "f"), positional("from", "Source."), positional("to", "Target.")])
let arguments = try parse(cli, ["-f", "a.txt", "b.txt"]) on ArgsError as e:
    assert_fail(e.message)
assert arguments.flag("force") and arguments.value("to") == "b.txt"
```

### Методы `Arguments`

```unio
extend Arguments
```

#### `flag`

```unio
public function flag(self, name as Text) returns Bool
```

Whether the flag was given. A name that is not a declared flag is a fault.
Example:

```unio
let cli = parser("ls", "Lists.", [flag("long", "Long format.")])
let arguments = try parse(cli, []) on ArgsError as e:
    assert_fail(e.message)
assert not arguments.flag("long")
```

#### `get`

```unio
public function get(self, name as Text) returns Option[Text]
```

The value of an option or positional: the last one given, else its default, else none. A
name that is not declared, or that is a flag, is a fault.
Example:

```unio
let cli = parser("run", "Runs.", [option("config", "Configuration file.")])
let arguments = try parse(cli, ["--config=app.toml"]) on ArgsError as e:
    assert_fail(e.message)
assert arguments.get("config") == some("app.toml")
```

#### `value`

```unio
public function value(self, name as Text) returns Text
```

The value of an option or positional that has one: given, or by default. A parameter without
a value is a fault, so use it for required positionals and parameters with defaults, and get
for the others.
Example:

```unio
let cli = parser("say", "Says.", [positional("word", "The word.", default = some("hi"))])
let arguments = try parse(cli, []) on ArgsError as e:
    assert_fail(e.message)
assert arguments.value("word") == "hi"
```

#### `values`

```unio
public function values(self, name as Text) returns List[Text]
```

Every value given for an option, in order, or the arguments of a rest parameter; for a
positional, its one value. Defaults are not included.
Example:

```unio
let cli = parser("tag", "Tags.", [option("tag", "A tag.", short = "t")])
let arguments = try parse(cli, ["-t", "a", "--tag", "b"]) on ArgsError as e:
    assert_fail(e.message)
assert arguments.values("tag") == ["a", "b"]
```

#### `get_int`

```unio
public function get_int(self, name as Text) returns Option[Int] fails with ArgsError
```

The value as a decimal integer; none when the parameter has no value. A value that is not an
integer fails with kind InvalidValue.
Example:

```unio
let cli = parser("head", "Prints lines.", [option("lines", "How many.", short = "n", default = some("10"))])
let arguments = try parse(cli, ["-n", "3"]) on ArgsError as e:
    assert_fail(e.message)
let count = try arguments.get_int("lines") on ArgsError as e:
    assert_fail(e.message)
assert count == some(3)
```

## `std.collections`

Исходный текст: [stdlib/alloc/collections.unio](../../stdlib/alloc/collections.unio). Слой: `alloc`.

Collections beyond the built-in List, Map and Set (requirement S-1): a double-ended queue, a
priority queue, and counting. They are values like List: assigning one copies it, and methods
with var self change only the place they are called on. The built-in collections and their
methods are in the prelude (builtin/list.unio, map.unio, set.unio).

### `Deque`

```unio
public struct Deque[T]
```

A double-ended queue: adding and removing at either end take amortized constant time.

У структуры есть закрытые поля: её создают только функции её модуля.

### `deque`

```unio
public function deque[T]() returns Deque[T]
```

An empty deque.
Example:

<pre><code>var queue = deque[Int]&#40;)
queue.push_back(1)
assert queue.length() == 1</code></pre>

### `deque_of`

```unio
public function deque_of[T](items as List[T]) returns Deque[T]
```

A deque of the items, the first item at the front.
Example:

```unio
assert deque_of([1, 2, 3]).peek_front() == some(1)
```

### Методы `Deque[T]`

```unio
extend[T] Deque[T]
```

#### `length`

```unio
public function length(self) returns Int
```

The number of items.
Example:

```unio
assert deque_of(["a", "b"]).length() == 2
```

#### `is_empty`

```unio
public function is_empty(self) returns Bool
```

Whether the deque has no items.
Example:

<pre><code>assert deque[Text]&#40;).is_empty()</code></pre>

#### `push_back`

```unio
public function push_back(var self, item as T)
```

Adds the item at the back.
Example:

```unio
var queue = deque_of([1])
queue.push_back(2)
assert queue.to_list() == [1, 2]
```

#### `push_front`

```unio
public function push_front(var self, item as T)
```

Adds the item at the front.
Example:

```unio
var queue = deque_of([1])
queue.push_front(0)
assert queue.to_list() == [0, 1]
```

#### `pop_front`

```unio
public function pop_front(var self) returns Option[T]
```

Removes and returns the item at the front; none when the deque is empty.
Example:

```unio
var queue = deque_of([1, 2])
assert queue.pop_front() == some(1) and queue.to_list() == [2]
```

#### `pop_back`

```unio
public function pop_back(var self) returns Option[T]
```

Removes and returns the item at the back; none when the deque is empty.
Example:

```unio
var queue = deque_of([1, 2])
assert queue.pop_back() == some(2) and queue.to_list() == [1]
```

#### `peek_front`

```unio
public function peek_front(self) returns Option[T]
```

The item at the front without removing it.
Example:

```unio
assert deque_of([4, 5]).peek_front() == some(4)
```

#### `peek_back`

```unio
public function peek_back(self) returns Option[T]
```

The item at the back without removing it.
Example:

```unio
assert deque_of([4, 5]).peek_back() == some(5)
```

#### `to_list`

```unio
public function to_list(self) returns List[T]
```

The items from the front to the back.
Example:

<pre><code>var queue = deque[Int]&#40;)
queue.push_front(2)
queue.push_back(3)
queue.push_front(1)
assert queue.to_list() == [1, 2, 3]</code></pre>

### `PriorityQueue`

```unio
public struct PriorityQueue[T with Order]
```

A priority queue: pop returns the smallest item by Order first; equal items come out in no
particular order. Adding and removing take O(log n) time (a binary heap).

У структуры есть закрытые поля: её создают только функции её модуля.

### `priority_queue`

```unio
public function priority_queue[T with Order]() returns PriorityQueue[T]
```

An empty priority queue.
Example:

<pre><code>var queue = priority_queue[Int]&#40;)
queue.push(3)
queue.push(1)
assert queue.pop() == some(1)</code></pre>

### `priority_queue_of`

```unio
public function priority_queue_of[T with Order](items as List[T]) returns PriorityQueue[T]
```

A priority queue of the items.
Example:

```unio
assert priority_queue_of([5, 2, 8]).peek() == some(2)
```

### Методы `PriorityQueue[T]`

```unio
extend[T with Order] PriorityQueue[T]
```

#### `length`

```unio
public function length(self) returns Int
```

The number of items.
Example:

```unio
assert priority_queue_of([1, 1]).length() == 2
```

#### `is_empty`

```unio
public function is_empty(self) returns Bool
```

Whether the queue has no items.
Example:

<pre><code>assert priority_queue[Text]&#40;).is_empty()</code></pre>

#### `peek`

```unio
public function peek(self) returns Option[T]
```

The smallest item without removing it; none when the queue is empty.
Example:

```unio
assert priority_queue_of(["b", "a"]).peek() == some("a")
```

#### `push`

```unio
public function push(var self, item as T)
```

Adds the item.
Example:

<pre><code>var queue = priority_queue[Int]&#40;)
queue.push(7)
assert queue.peek() == some(7)</code></pre>

#### `pop`

```unio
public function pop(var self) returns Option[T]
```

Removes and returns the smallest item; none when the queue is empty.
Example:

```unio
var queue = priority_queue_of([3, 1, 2])
assert [queue.pop(), queue.pop(), queue.pop(), queue.pop()] == [some(1), some(2), some(3), none]
```

#### `to_sorted_list`

```unio
public function to_sorted_list(self) returns List[T]
```

The items from the smallest to the largest; the queue is not changed.
Example:

```unio
assert priority_queue_of([3, 1, 2]).to_sorted_list() == [1, 2, 3]
```

### `counts`

```unio
public function counts[T with Hash and Equal](items as List[T]) returns Map[T, Int]
```

How many times each distinct item occurs, in the order of first occurrence.
Example:

```unio
assert counts(["a", "b", "a"]) == {"a": 2, "b": 1}
```

### `most_common`

```unio
public function most_common[T with Hash and Equal](
    items as List[T],
    n as Int,
) returns List[Tuple[T, Int]]
```

The n items that occur most often with their counts, the most frequent first; items with the
same count keep the order of first occurrence. A negative n is treated as 0.
Example:

```unio
assert most_common(["x", "y", "y", "z", "y", "x"], 2) == [("y", 3), ("x", 2)]
```

## `std.decimal`

Исходный текст: [stdlib/alloc/decimal.unio](../../stdlib/alloc/decimal.unio). Слой: `alloc`.

Exact decimal numbers (requirements S-1, S-1a). A Decimal is an integer of any size divided by
a power of ten: 12.5 is 125 / 10^1. Addition, subtraction and multiplication are always exact;
division is exact when the quotient has a finite decimal expansion, and otherwise rounds only
where the program says how (divide_rounded). Nothing overflows: the size is limited by memory,
like a List.

Decimal implements Add, Subtract, Multiply and Divide, so + - * / work, and Equal, Order, Hash
and Display. A Decimal is kept in its shortest form, without zeros at the end of the fraction,
so 1.50 and 1.5 are the same value: equal, with equal hashes, and the same key of a Map. The
text is that shortest form ("1.5"); to_fixed writes a given number of places ("1.50"). A
Decimal is made from text (parse) and Int (from_int); from Float only by from_float, which
names the places and the rounding, since a Float such as 0.1 is not a decimal number exactly.

### `Decimal`

```unio
public struct Decimal
```

A decimal number; see the module documentation.

У структуры есть закрытые поля: её создают только функции её модуля.

### `Rounding`

```unio
public enum Rounding:
    ## To the nearer one; a tie to the one whose last digit is even (banker's rounding).
    HalfEven
    ## To the nearer one; a tie away from zero (commercial rounding).
    HalfUp
    ## To the nearer one; a tie toward zero.
    HalfDown
    ## Away from zero.
    Up
    ## Toward zero (truncation).
    Down
    ## Toward positive infinity.
    Ceiling
    ## Toward negative infinity.
    Floor
```

How to round a result that lies between two numbers with the wanted places.

### `DecimalError`

```unio
public error DecimalError:
    text as Text
    message as Text
```

A text that is not a decimal number, a Float that is not finite, or an exponent out of range.

### `from_int`

```unio
public function from_int(value as Int) returns Decimal
```

The decimal number of an Int, with scale 0.
Example:

```unio
assert "{from_int(-42)}" == "-42"
```

### `parse`

```unio
public function parse(text as Text) returns Decimal fails with DecimalError
```

Reads a decimal number: an optional sign, digits with an optional point and fraction, and an
optional exponent "e" or "E" with an optional sign, up to 100000 in size: "1.50", "-.5",
"1.5e1" (15) and "2E-3" (0.002). No spaces, no "_", no NaN or infinity.
Example:

```unio
let price = try parse("19.90") on DecimalError as e:
    assert_fail(e.message)
assert "{price}" == "19.9"
```

### `from_float`

```unio
public function from_float(
    value as Float,
    places as Int,
    rounding as Rounding,
) returns Decimal fails with DecimalError
```

The Float rounded to at most the given number of places after the point with the given
rounding. The exact binary value of the Float is rounded, so from_float(0.1, 20,
Rounding.HalfEven) is 0.10000000000000000555. NaN, infinities and negative places fail with
DecimalError.
Example:

```unio
let tenth = try from_float(0.1, 2, Rounding.HalfEven) on DecimalError as e:
    assert_fail(e.message)
assert "{tenth}" == "0.1"
```

### Методы `Decimal`

```unio
extend Decimal
```

#### `scale`

```unio
public function scale(self) returns Int
```

The number of digits after the decimal point of the shortest form.
Example:

```unio
let value = try parse("1.250") on DecimalError as e:
    assert_fail(e.message)
assert value.scale() == 2
```

#### `is_zero`

```unio
public function is_zero(self) returns Bool
```

Whether the value is zero, whatever the scale.
Example:

```unio
assert from_int(0).is_zero()
```

#### `is_negative`

```unio
public function is_negative(self) returns Bool
```

Whether the value is below zero.
Example:

```unio
assert from_int(-1).is_negative() and not from_int(0).is_negative()
```

#### `negate`

```unio
public function negate(self) returns Decimal
```

The value with the opposite sign and the same scale.
Example:

```unio
assert from_int(5).negate() == from_int(-5)
```

#### `abs`

```unio
public function abs(self) returns Decimal
```

The value without its sign.
Example:

```unio
assert from_int(-5).abs() == from_int(5)
```

#### `round`

```unio
public function round(self, places as Int, rounding as Rounding) returns Decimal
```

The value rounded to at most places digits after the point with the given rounding.
Negative places is a fault.
Example:

```unio
let amount = try parse("2.675") on DecimalError as e:
    assert_fail(e.message)
assert "{amount.round(2, Rounding.HalfEven)}" == "2.68"
assert "{amount.round(2, Rounding.Down)}" == "2.67"
```

#### `divide_rounded`

```unio
public function divide_rounded(
    self,
    other as Decimal,
    places as Int,
    rounding as Rounding,
) returns Decimal
```

The quotient rounded to at most places digits after the point with the given rounding; a
zero divisor or negative places is a fault.
Example:

```unio
let third = from_int(1).divide_rounded(from_int(3), 4, Rounding.HalfEven)
assert "{third}" == "0.3333"
```

#### `checked_divide`

```unio
public function checked_divide(self, other as Decimal) returns Option[Decimal]
```

The exact quotient, or none for a zero divisor or a quotient without a finite decimal
expansion, such as 1 / 3.
Example:

```unio
let eighth = try parse("0.125") on DecimalError as e:
    assert_fail(e.message)
assert from_int(1).checked_divide(from_int(8)) == some(eighth)
assert from_int(1).checked_divide(from_int(3)) == none
```

#### `to_int`

```unio
public function to_int(self) returns Option[Int]
```

The value as an Int; none when it has a nonzero fraction or does not fit.
Example:

```unio
assert from_int(12).to_int() == some(12)
assert from_int(1).divide_rounded(from_int(2), 1, Rounding.Down).to_int() == none
```

#### `to_float`

```unio
public function to_float(self) returns Float
```

The nearest Float, rounding half to even; a value beyond the range of Float gives an
infinity. The conversion is explicit because it may lose digits.
Example:

```unio
assert (from_int(1) / from_int(8)).to_float() == 0.125
```

#### `to_fixed`

```unio
public function to_fixed(self, places as Int, rounding as Rounding) returns Text
```

The text with exactly places digits after the point, rounded with the given rounding when
the value has more: for amounts of money and tables. Negative places is a fault.
Example:

```unio
let price = try parse("19.9") on DecimalError as e:
    assert_fail(e.message)
assert price.to_fixed(2, Rounding.HalfEven) == "19.90"
assert from_int(-1).divide_rounded(from_int(3), 4, Rounding.HalfEven).to_fixed(2, Rounding.HalfEven) == "-0.33"
```

### `Decimal` реализует `Add`

```unio
extend Decimal with Add
```

#### `add`

```unio
function add(self, other as Self) returns Self
```

The exact sum, with the larger scale.

### `Decimal` реализует `Subtract`

```unio
extend Decimal with Subtract
```

#### `subtract`

```unio
function subtract(self, other as Self) returns Self
```

The exact difference, with the larger scale.

### `Decimal` реализует `Multiply`

```unio
extend Decimal with Multiply
```

#### `multiply`

```unio
function multiply(self, other as Self) returns Self
```

The exact product, with the sum of the scales.

### `Decimal` реализует `Divide`

```unio
extend Decimal with Divide
```

#### `divide`

```unio
function divide(self, other as Self) returns Self
```

The exact quotient. A zero divisor, or a quotient without a finite decimal expansion such as
1 / 3, is a fault; checked_divide is the safe form and divide_rounded rounds.

### `Decimal` реализует `Equal`

```unio
extend Decimal with Equal
```

#### `equals`

```unio
function equals(self, other as Self) returns Bool
```

Equal values; the shortest form makes this the same as equal fields.

### `Decimal` реализует `Order`

```unio
extend Decimal with Order
```

#### `compare`

```unio
function compare(self, other as Self) returns Int
```

### `Decimal` реализует `Hash`

```unio
extend Decimal with Hash
```

#### `hash`

```unio
function hash(self) returns Int
```

### `Decimal` реализует `Display`

```unio
extend Decimal with Display
```

#### `to_text`

```unio
function to_text(self) returns Text
```

Plain decimal notation of the shortest form: "-0.05", "1200", "0".

## `std.env`

Исходный текст: [stdlib/std/env.unio](../../stdlib/std/env.unio). Слой: `std`.

The environment of the process.

### `get`

```unio
public external function get(name as Text) returns Option[Text]
```

The value of an environment variable; none if it is not set or not permitted. Permission: env.
Example:

```unio
assert get("UNIO_EXAMPLE_VARIABLE_THAT_IS_NOT_SET") == none
```

### `home_dir`

```unio
public external function home_dir() returns Option[Path]
```

The home directory of the user's profile as the operating system reports it, not from $HOME;
needs no permission.
Example:

```unio
match home_dir():
    case some(home): assert home.is_absolute()
    case none: pass
```

## `std.errors`

Исходный текст: [stdlib/alloc/errors.unio](../../stdlib/alloc/errors.unio). Слой: `alloc`.

Explicit error context. The cause stays an Error value; it is never downcast.
Handle concrete error kinds before wrapping when recovery depends on them.

### `ContextError`

```unio
public error ContextError:
    operation as Text
    cause as Error
```

### `context`

```unio
public function context(operation as Text, cause as Error) returns ContextError
```

Attach an operation to an error without flattening its cause to text.
Example:

```unio
let wrapped = context("reading configuration", ExampleError(message = "missing field"))
assert wrapped.operation == "reading configuration"
assert wrapped.cause.diagnostic_code() == ExampleError(message = "").diagnostic_code()
```

## `std.files`

Исходный текст: [stdlib/std/files.unio](../../stdlib/std/files.unio). Слой: `std`.

Files and directories.

Writing replaces the target through a temporary file in the same directory (requirement S-9):
1. atomic replacement: a reader sees the old or the new complete content, however the program ends;
2. ordering: the data is synced before the rename, so this also holds after power loss;
3. durability: with durable = true the directory is synced too, and after a successful return
   the new content survives power loss.
These hold on ext4, XFS, Btrfs, APFS and NTFS (S-9a). On other file systems the write is
performed without the guarantees, and durable = true fails with kind Unsupported.

An operation blocked by the sandbox fails with kind PermissionDenied; exists and is_directory report it as false.

### `IoErrorKind`

```unio
public enum IoErrorKind:
    NotFound
    PermissionDenied
    AlreadyExists
    InvalidData
    Unsupported
    DurabilityUnconfirmed
    Other
```

### `IoError`

```unio
public error IoError:
    path as Path
    kind as IoErrorKind
    reason as Text
```

### `exists`

```unio
public external function exists(path as Path) returns Bool
```

Whether the path exists and is accessible. Permission: files.read.
Example:

```unio
assert not exists(Path("no-such-directory/file.txt"))
```

### `is_directory`

```unio
public external function is_directory(path as Path) returns Bool
```

Whether the path is an accessible directory. Permission: files.read.
Example:

```unio
assert not is_directory(Path("no-such-directory"))
```

### `read_text`

```unio
public external function read_text(path as Path) returns Text fails with IoError
```

The whole file as text; kind InvalidData if it is not UTF-8. Permission: files.read.
A file larger than the heap limit of the policy allows ends the program with code 102 before it is read.
Example:

```unio
try:
    let text = try read_text(Path("no-such-directory/notes.txt"))
    assert_fail("read {text}")
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `FileReader`

```unio
public struct FileReader
```

An owned descriptor with private state; created by open_reader inside using.

У структуры есть закрытые поля: её создают только функции её модуля.

### `open_reader`

```unio
public function open_reader(path as Path) returns FileReader fails with IoError
```

Opens a byte stream. Permission: files.read. No file content is read ahead of the first read.
Example:

```unio
try:
    using reader = try open_reader(Path("no-such-directory/data.bin")):
        let chunk = try reader.read_bytes(max_bytes = 4096)
        assert chunk != none
on IoError as e:
    assert e.path == Path("no-such-directory/data.bin")
```

### `FileReader` реализует `Resource`

```unio
extend FileReader with Resource
```

#### `release`

```unio
function release(var self)
```

### Методы `FileReader`

```unio
extend FileReader
```

#### `read_bytes`

```unio
public function read_bytes(
    var self,
    named max_bytes as Int = 65536,
) returns Option[Bytes] fails with IoError
```

some(nonempty bytes) up to max_bytes, or permanent none at EOF. max_bytes must be positive;
otherwise InvalidData. Buffered bytes are returned before a pending error on the next call.
An error makes further reads return that error. Memory is O(max_bytes), independent of file size;
a max_bytes beyond the heap limit of the policy ends the program with code 102.
Example:

```unio
try:
    using reader = try open_reader(Path("no-such-directory/data.bin")):
        var total = 0
        while true:
            match try reader.read_bytes(max_bytes = 65536):
                case some(chunk): total = total + chunk.length()
                case none: break
on IoError as e:
    assert e.kind != IoErrorKind.AlreadyExists
```

### `resolve`

```unio
public external function resolve(path as Path) returns Path fails with IoError
```

Resolve an existing path through the filesystem, including links. Permission: files.read.
This is not an authorization token; sandbox enforcement still applies at the eventual open.
Example:

```unio
try:
    let real = try resolve(Path("no-such-directory"))
    assert real.is_absolute()
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `list_directory`

```unio
public external function list_directory(path as Path) returns List[Path] fails with IoError
```

The entries of a directory, sorted by name, without "." and "..". Permission: files.read.
Example:

```unio
try:
    for entry in try list_directory(Path("no-such-directory")):
        print(entry.name())
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `list_tree`

```unio
public function list_tree(root as Path) returns List[Path] fails with IoError
```

Every file below the directory, sorted, depth first; directories themselves are not listed.
A symbolic link to a directory is listed as an entry and not followed, so that a link cannot
lead the listing into a cycle or out of the tree; the root itself may be a link.
A link is recognized because resolving it does not give its own place in the resolved tree.
Example:

```unio
try:
    let sources = [p for p in try list_tree(Path("no-such-directory")) if p.extension() == "unio"]
    assert sources.length() >= 0
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `write_text`

```unio
public function write_text(path as Path, text as Text, durable as Bool = false) fails with IoError
```

Replaces the file with the text atomically (guarantees 1 and 2); durable = true adds guarantee 3. Permission: files.write.
Example:

```unio
try:
    try write_text(Path("no-such-directory/report.txt"), "done\n")
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `read_bytes`

```unio
public external function read_bytes(path as Path) returns Bytes fails with IoError
```

The whole file as bytes. Permission: files.read. Memory is checked as in read_text.
Example:

```unio
try:
    let data = try read_bytes(Path("no-such-directory/image.png"))
    assert_fail("read {data.length()} bytes")
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `write_bytes`

```unio
public function write_bytes(
    path as Path,
    data as Bytes,
    durable as Bool = false,
) fails with IoError
```

Replaces the file with the bytes atomically, like write_text. Permission: files.write.
Example:

```unio
try:
    try write_bytes(Path("no-such-directory/empty.bin"), Bytes())
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `copy_file`

```unio
public function copy_file(
    source as Path,
    target as Path,
    durable as Bool = false,
) fails with IoError
```

Copies the content of a file; the target is replaced atomically, like write_bytes.
Permission: files.read for the source, files.write for the target.
Example:

```unio
try:
    try copy_file(Path("no-such-directory/a.txt"), Path("no-such-directory/b.txt"))
on IoError as e:
    assert e.path == Path("no-such-directory/a.txt")
```

### `rename`

```unio
public external function rename(source as Path, target as Path) fails with IoError
```

Gives a file or directory a new path on the same file system. It fails with kind AlreadyExists
instead of replacing an existing target, so that renaming never destroys data. The check and the
rename are one atomic step: renameat2 with RENAME_NOREPLACE on Linux, renamex_np with
RENAME_EXCL on macOS, MoveFileExW on Windows. Where the system or the file system has no such
step, a file is linked under the new name and then unlinked from the old one, which still never
replaces a target, and a directory fails with kind Unsupported. Permission: files.write.
Example:

```unio
try:
    try rename(Path("no-such-directory/old.txt"), Path("no-such-directory/new.txt"))
on IoError as e:
    assert e.kind != IoErrorKind.AlreadyExists
```

### `create_directory`

```unio
public external function create_directory(path as Path) fails with IoError
```

Creates one directory; fails with kind AlreadyExists if it exists and NotFound if its parent
does not. On Linux and macOS the directory that holds it is synced as well, so that a durable
write into the new directory survives power loss (S-9). If that sync fails, the directory is
still created; a later durable commit below it syncs the parent again first and fails with kind
DurabilityUnconfirmed if it still cannot. Permission: files.write.
Example:

```unio
try:
    try create_directory(Path("no-such-directory/child"))
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `create_directories`

```unio
public function create_directories(path as Path) fails with IoError
```

Creates the directory and every missing parent; an existing directory is not an error.
Permission: files.write.
Example:

```unio
try:
    try create_directories(Path("no-such-directory").join("a").join("b"))
    assert is_directory(Path("no-such-directory/a/b"))
on IoError as e:
    assert e.kind == IoErrorKind.PermissionDenied
```

### `AtomicState`

```unio
public enum AtomicState:
    ## The descriptor is open; the temporary file holds what was written so far.
    Writing
    ## The data is synced and the descriptor is released; the temporary file still exists.
    Closed
    ## The temporary file has replaced the target; the directory is not synced (yet).
    Replaced
    ## The directory is synced too: the replacement survives power loss.
    Durable
```

The progress of an atomic file. Each step of commit records its result before the next step
starts, so release and a repeated commit know exactly what the file still owns.

### `AtomicFile`

```unio
public struct AtomicFile
```

A file that is written next to its target and replaces it only on commit.
Releasing it cleans up only what it still owns: an open descriptor is closed once, a temporary
file that has not replaced the target is removed, and nothing is done after the replacement.

У структуры есть закрытые поля: её создают только функции её модуля.

### `open_atomic`

```unio
public function open_atomic(
    path as Path,
    durable as Bool = false,
) returns AtomicFile fails with IoError
```

Opens a temporary file in the directory of the target. With durable = true it fails with kind
Unsupported on a file system where durability cannot be guaranteed (S-9a). Permission: files.write.
Example:

```unio
try:
    using file = try open_atomic(Path("no-such-directory/out.txt")):
        try file.write("header\n")
        try file.write_line("row")
        try file.write_bytes(Bytes())
        try file.commit()
        assert file.is_replaced()
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### Методы `AtomicFile`

```unio
extend AtomicFile
```

#### `write`

```unio
public function write(var self, text as Text) fails with IoError
```

Appends the text to the temporary file; the target does not change before commit.
Example:

```unio
try:
    using file = try open_atomic(Path("no-such-directory/out.txt")):
        try file.write("header\n")
        try file.write_line("row")
        try file.write_bytes(Bytes())
        try file.commit()
        assert file.is_replaced()
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

#### `write_line`

```unio
public function write_line(var self, text as Text) fails with IoError
```

Appends the text and a line end "\n".
Example:

```unio
try:
    using file = try open_atomic(Path("no-such-directory/out.txt")):
        try file.write("header\n")
        try file.write_line("row")
        try file.write_bytes(Bytes())
        try file.commit()
        assert file.is_replaced()
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

#### `write_bytes`

```unio
public function write_bytes(var self, data as Bytes) fails with IoError
```

Appends the bytes.
Example:

```unio
try:
    using file = try open_atomic(Path("no-such-directory/out.txt")):
        try file.write("header\n")
        try file.write_line("row")
        try file.write_bytes(Bytes())
        try file.commit()
        assert file.is_replaced()
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

#### `is_replaced`

```unio
public function is_replaced(self) returns Bool
```

Whether the target already holds the new content.
Example:

```unio
try:
    using file = try open_atomic(Path("no-such-directory/out.txt")):
        try file.write("header\n")
        try file.write_line("row")
        try file.write_bytes(Bytes())
        try file.commit()
        assert file.is_replaced()
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

#### `commit`

```unio
public function commit(var self) fails with IoError
```

Makes the written text the content of the target: sync the data, close, rename, and with
durable sync the directory. A failed commit can be called again and continues where it
stopped. If only the directory sync fails, the error has kind DurabilityUnconfirmed: the
target is already replaced (guarantees 1 and 2 hold), but its survival after power loss is
not confirmed.
Example:

```unio
try:
    using file = try open_atomic(Path("no-such-directory/out.txt")):
        try file.write("header\n")
        try file.write_line("row")
        try file.write_bytes(Bytes())
        try file.commit()
        assert file.is_replaced()
on IoError as e:
    assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied
```

### `AtomicFile` реализует `Resource`

```unio
extend AtomicFile with Resource
```

#### `release`

```unio
function release(var self)
```

Errors are ignored: release cannot fail, and the target is unchanged in every case where it acts.

## `std.json`

Исходный текст: [stdlib/alloc/json.unio](../../stdlib/alloc/json.unio). Слой: `alloc`.

Reading and writing JSON (RFC 8259).

parse reads a number without a fraction or an exponent that fits in Int as Value.Int and any
other number as Value.Float. encode and decode work for Value, Int, Float, Bool, Text, Path,
Option, List, Map with Text keys, and public-field transport structs/enums built from them; the compiler checks
this at the call (requirements S-5, S-17); private fields are never converted automatically. A struct is an object with its field names, and a field
with a default may be missing on decoding. A variant without fields is its name as text; a
variant with fields is an object {"Variant": {fields}}.

### `Value`

```unio
public enum Value:
    Null
    Bool(value as Bool)
    Int(value as Int)
    Float(value as Float)
    Text(value as Text)
    Array(items as List[Value])
    Object(fields as Map[Text, Value])
```

A JSON document. The variants share names with built-in types, so they are always written
with the enum name: Value.Text("Anna").

### `ParseError`

```unio
public error ParseError:
    line as Int
    column as Int
    message as Text
```

### `DecodeError`

```unio
public error DecodeError:
    path as Text
    message as Text
```

path is a JSON Pointer to the mismatching value, for example "/2/title"; "" denotes the root.

### `EncodeError`

```unio
public error EncodeError:
    path as Text
    message as Text
```

### `encode`

```unio
public function encode[T](value as T, pretty as Bool = false) returns Text
```

The value as JSON text; pretty = true puts one item per line with an indent of four spaces.
A Float that is NaN or infinite is written as null, because JSON has no such numbers.
Example:

```unio
assert encode({"id": 7}) == "\{\"id\":7\}"
assert encode([1, 2]) == "[1,2]"
```

### `encode_checked`

```unio
public function encode_checked[T](
    value as T,
    named pretty as Bool = false,
) returns Text fails with EncodeError
```

Strict encoding for protocols and configuration: reject non-finite numbers instead of losing data.
Example:

```unio
let text = try encode_checked([1.5]) on EncodeError as e:
    assert_fail(e.message)
assert text == "[1.5]"
```

### `decode`

```unio
public function decode[T](value as Value) returns T fails with DecodeError
```

The value converted to the type T, strictly (S-17): a missing required field, an unknown field or
variant, a number that is not exact for the type, or a wrong kind fails with DecodeError, whose
path points at the value.
Example:

<pre><code>let numbers = try decode[List[Int]]&#40;Value.Array([Value.Int(1), Value.Int(2)])) on DecodeError as e:
    assert_fail(e.message)
assert numbers == [1, 2]</code></pre>

### `kind_name`

```unio
public function kind_name(value as Value) returns Text
```

"null", "bool", "int", "float", "text", "array", or "object".
Example:

```unio
assert kind_name(Value.Null) == "null"
```

### `parse`

```unio
public function parse(
    text as Text,
    named max_depth as Int = 128,
    named max_bytes as Int = 16777216,
) returns Value fails with ParseError
```

Bounded strict parsing: duplicate keys fail. Limits are checked before constructing the character list.
Depth counts values along a nesting path, including the root; max_bytes counts UTF-8 bytes.
Example:

```unio
let value = try parse("\{\"name\": \"Ana\"\}") on ParseError as e:
    assert_fail(e.message)
assert value == Value.Object({"name": Value.Text("Ana")})
```

## `std.log`

Исходный текст: [stdlib/std/log.unio](../../stdlib/std/log.unio). Слой: `std`.

Logging (requirement S-1): records of four levels written to standard error, one line each,
never to standard output, which belongs to the result of the program.

A line is the level, the name of the logger if it has one, the message, and the fields as
key=value pairs in the order given; a value with a space, a quote, an equals sign, a backslash or
a control character is quoted with backslash escapes, so every record stays one line that a
program can split:

```unio
WARNING sync: retrying attempt=2 path="/tmp/a b"
```

The functions of the module write every record. A Logger has a minimum level and skips records
below it; the program chooses the level, for example from its arguments. Writing to standard
error needs no permission and has no derived effect. Messages and fields are written as given:
leave secrets out of them.

### `Level`

```unio
public enum Level:
    Debug
    Info
    Warning
    Error
```

The importance of a record, from the least to the most important.

### `Level` реализует `Order`

```unio
extend Level with Order
```

#### `compare`

```unio
function compare(self, other as Self) returns Int
```

### `Level` реализует `Display`

```unio
extend Level with Display
```

#### `to_text`

```unio
function to_text(self) returns Text
```

The name written in a record: DEBUG, INFO, WARNING or ERROR.

### `Logger`

```unio
public struct Logger:
    public name as Text
    public level as Level = Level.Info
```

A named source of records with a minimum level.

### Методы `Logger`

```unio
extend Logger
```

#### `enabled`

```unio
public function enabled(self, level as Level) returns Bool
```

Whether a record of the level is written; a caller can skip building an expensive message.
Example:

```unio
assert Logger(name = "app").enabled(Level.Warning)
assert not Logger(name = "app").enabled(Level.Debug)
```

#### `write`

```unio
public function write(self, level as Level, message as Text, fields as Map[Text, Text] = {})
```

Writes a record of the level if the level is enabled.
Example:

```unio
Logger(name = "app", level = Level.Debug).write(Level.Debug, "starting", {"pid": "42"})
```

#### `debug`

```unio
public function debug(self, message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Debug if it is enabled.
Example:

```unio
Logger(name = "app").debug("not written below Info")
```

#### `info`

```unio
public function info(self, message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Info if it is enabled.
Example:

```unio
Logger(name = "app").info("started", {"port": "8080"})
```

#### `warning`

```unio
public function warning(self, message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Warning if it is enabled.
Example:

```unio
Logger(name = "app").warning("disk almost full")
```

#### `error`

```unio
public function error(self, message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Error if it is enabled.
Example:

```unio
Logger(name = "app").error("cannot open the file", {"path": "a b.txt"})
```

### `debug`

```unio
public function debug(message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Debug without a name.
Example:

```unio
debug("cache miss", {"key": "user:7"})
```

### `info`

```unio
public function info(message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Info without a name.
Example:

```unio
info("listening")
```

### `warning`

```unio
public function warning(message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Warning without a name.
Example:

```unio
warning("retrying", {"attempt": "2"})
```

### `error`

```unio
public function error(message as Text, fields as Map[Text, Text] = {})
```

Writes a record of level Error without a name.
Example:

```unio
error("request failed", {"status": "500"})
```

### `format_record`

```unio
public function format_record(
    level as Level,
    name as Text,
    message as Text,
    fields as Map[Text, Text] = {},
) returns Text
```

The line of a record without its line end, as the functions of this module write it; an empty
name is left out.
Example:

```unio
assert format_record(Level.Info, "web", "started", {"port": "80"}) == "INFO web: started port=80"
assert format_record(Level.Error, "", "failed", {"path": "a b"}) == "ERROR failed path=\"a b\""
```

## `std.math`

Исходный текст: [stdlib/core/math.unio](../../stdlib/core/math.unio). Слой: `core`.

Mathematics (requirement S-1): constants, the elementary functions of IEEE binary64, rounding,
and integer functions that report overflow as none instead of a fault. Everything here belongs
to the core layer: nothing allocates and nothing touches the operating system.

The Float functions follow IEEE 754: they never fault, and a result outside the domain is NaN
(sqrt(-1.0)) or an infinity (ln(0.0)). sqrt and the roundings are exact; the transcendental
functions come from the math library of the platform and may differ in the last bit between
platforms. A Float result becomes an Int only explicitly: math.floor(x).to_int().

### `PI`

```unio
public let PI as Float = 3.141592653589793
```

The ratio of a circle's circumference to its diameter, the Float nearest to π.
Example:

```unio
assert PI > 3.14159 and PI < 3.1416
```

### `TAU`

```unio
public let TAU as Float = 6.283185307179586
```

The Float nearest to τ = 2π.
Example:

```unio
assert TAU == 2.0 * PI
```

### `E`

```unio
public let E as Float = 2.718281828459045
```

The Float nearest to Euler's number e.
Example:

```unio
assert ln(E) == 1.0
```

### `INFINITY`

```unio
public let INFINITY as Float = 1e308 * 10.0
```

Positive infinity.
Example:

```unio
assert not INFINITY.is_finite() and INFINITY > 1e308
```

### `NAN`

```unio
public let NAN as Float = 0.0 * (1e308 * 10.0)
```

A quiet NaN. It is not equal to itself; test for it with is_nan.
Example:

```unio
assert NAN.is_nan() and NAN != NAN
```

### `sqrt`

```unio
public external function sqrt(x as Float) returns Float
```

The square root, correctly rounded; NaN for a negative number, -0.0 for -0.0.
Example:

```unio
assert sqrt(2.25) == 1.5
```

### `cbrt`

```unio
public external function cbrt(x as Float) returns Float
```

The cube root; defined for negative numbers.
Example:

```unio
assert cbrt(-27.0) == -3.0
```

### `exp`

```unio
public external function exp(x as Float) returns Float
```

e raised to the power x.
Example:

```unio
assert exp(0.0) == 1.0
```

### `ln`

```unio
public external function ln(x as Float) returns Float
```

The natural logarithm; -infinity at 0.0 and NaN for a negative number.
Example:

```unio
assert ln(1.0) == 0.0
```

### `log2`

```unio
public external function log2(x as Float) returns Float
```

The base-2 logarithm.
Example:

```unio
assert log2(1024.0) == 10.0
```

### `log10`

```unio
public external function log10(x as Float) returns Float
```

The base-10 logarithm.
Example:

```unio
assert log10(1000.0) == 3.0
```

### `sin`

```unio
public external function sin(x as Float) returns Float
```

The sine of an angle in radians.
Example:

```unio
assert sin(0.0) == 0.0
```

### `cos`

```unio
public external function cos(x as Float) returns Float
```

The cosine of an angle in radians.
Example:

```unio
assert cos(0.0) == 1.0
```

### `tan`

```unio
public external function tan(x as Float) returns Float
```

The tangent of an angle in radians.
Example:

```unio
assert tan(0.0) == 0.0
```

### `asin`

```unio
public external function asin(x as Float) returns Float
```

The arcsine in radians, in -π/2..π/2; NaN outside -1.0..1.0.
Example:

```unio
assert asin(1.0) == PI / 2.0
```

### `acos`

```unio
public external function acos(x as Float) returns Float
```

The arccosine in radians, in 0..π; NaN outside -1.0..1.0.
Example:

```unio
assert acos(1.0) == 0.0
```

### `atan`

```unio
public external function atan(x as Float) returns Float
```

The arctangent in radians, in -π/2..π/2.
Example:

```unio
assert atan(0.0) == 0.0
```

### `atan2`

```unio
public external function atan2(y as Float, x as Float) returns Float
```

The angle in radians of the point (x, y) from the positive x axis, in -π..π; it uses the signs of
both arguments to find the quadrant.
Example:

```unio
assert atan2(1.0, 0.0) == PI / 2.0
```

### `power`

```unio
public external function power(base as Float, exponent as Float) returns Float
```

base raised to the power exponent, as IEEE 754 pow: power(x, 0.0) is 1.0 even for NaN.
Example:

```unio
assert power(2.0, 10.0) == 1024.0
```

### `hypot`

```unio
public external function hypot(x as Float, y as Float) returns Float
```

The length of the hypotenuse, sqrt(x * x + y * y) without intermediate overflow.
Example:

```unio
assert hypot(3.0, 4.0) == 5.0
```

### `floor`

```unio
public external function floor(x as Float) returns Float
```

The largest whole number not greater than x.
Example:

```unio
assert floor(-2.5) == -3.0
```

### `ceil`

```unio
public external function ceil(x as Float) returns Float
```

The smallest whole number not less than x.
Example:

```unio
assert ceil(2.1) == 3.0
```

### `round`

```unio
public external function round(x as Float) returns Float
```

The nearest whole number; a half rounds away from zero.
Example:

```unio
assert round(2.5) == 3.0 and round(-2.5) == -3.0
```

### `truncate`

```unio
public external function truncate(x as Float) returns Float
```

The whole part of x: x rounded toward zero.
Example:

```unio
assert truncate(-2.7) == -2.0
```

### `from_bits`

```unio
public external function from_bits(bits as Int) returns Float
```

The Float whose IEEE 754 binary64 bits are the two's-complement bits of the Int; the inverse of
Float.to_bits.
Example:

```unio
assert from_bits(4607182418800017408) == 1.0
```

### `clamp`

```unio
public function clamp[T with Order](value as T, low as T, high as T) returns T
```

The value limited to low..high: low below it, high above it. low greater than high is a fault.
Example:

```unio
assert clamp(15, 0, 10) == 10 and clamp("b", "a", "c") == "b"
```

### `gcd`

```unio
public function gcd(a as Int, b as Int) returns Int
```

The greatest common divisor, never negative; gcd(0, 0) is 0. A result of 2^63, which only
gcd(MINIMUM_INT, 0) and gcd(MINIMUM_INT, MINIMUM_INT) give, is a fault, as for overflow.
Example:

```unio
assert gcd(12, -18) == 6
```

### `lcm`

```unio
public function lcm(a as Int, b as Int) returns Option[Int]
```

The least common multiple, never negative; 0 if either is 0, and none if it does not fit in Int.
Example:

```unio
assert lcm(4, 6) == some(12) and lcm(MAXIMUM_INT, 2) == none
```

### `checked_power`

```unio
public function checked_power(base as Int, exponent as Int) returns Option[Int]
```

base raised to a nonnegative exponent, or none on overflow or for a negative exponent.
Example:

```unio
assert checked_power(3, 4) == some(81) and checked_power(2, 63) == none
```

### `integer_sqrt`

```unio
public function integer_sqrt(n as Int) returns Int
```

The integer square root: the largest r with r * r \<= n; a negative n is a fault.
Example:

```unio
assert integer_sqrt(17) == 4 and integer_sqrt(MAXIMUM_INT) == 3037000499
```

### `floor_divide`

```unio
public function floor_divide(a as Int, b as Int) returns Int
```

The quotient rounded toward negative infinity, unlike /, which truncates toward zero. A zero
divisor and floor_divide(MINIMUM_INT, -1) are faults, as for /.
Example:

```unio
assert floor_divide(-7, 2) == -4 and -7 / 2 == -3
```

### `modulo`

```unio
public function modulo(a as Int, b as Int) returns Int
```

The remainder of floor_divide: it has the sign of the divisor, unlike %, which has the sign of
the dividend. A zero divisor is a fault.
Example:

```unio
assert modulo(-7, 3) == 2 and -7 % 3 == -1
```

## `std.path`

Исходный текст: [stdlib/std/path.unio](../../stdlib/std/path.unio). Слой: `std`.

Pure path manipulation (requirement S-1). Path itself and its methods components, is_absolute,
name, extension, parent, join and normalize are in the prelude (builtin/path.unio); this module
adds the operations on names and prefixes. Everything here is lexical, like Path.normalize: it
never reads the file system, follows no link, and proves nothing about access (S-19).

The platform contract (docs/stdlib.md, "Paths on each platform"):
- Separators: "/" separates names on every platform, "\\" also on Windows; on Linux and macOS
  "\\" is an ordinary character of a name. Path.join, and so with_name, with_extension and
  join_all, insert the separator of the platform; parent and normalize write "/".
- Anchors: a path may begin with an anchor, the first of its components. On Linux and macOS it
  is "/" (or "//"), and such a path is absolute. On Windows a drive with a root ("C:\\a",
  "C:/a") and a UNC share ("\\\\server\\share\\a") are absolute; a rooted path ("\\a", "/a",
  relative to the root of the current drive) and a drive-relative path ("C:a", relative to the
  current directory of drive C) have anchors too, "/" and "C:", but are not absolute.
- A path without an anchor is relative to the current directory: only such a path starts
  with "." (starts_with), and only it has "." as a base (relative_to).

### `stem`

```unio
public function stem(path as Path) returns Text
```

The name without its extension: "report" for "dir/report.txt", ".profile" for ".profile".
Example:

```unio
assert stem(Path("dir/report.tar.gz")) == "report.tar"
assert stem(Path(".profile")) == ".profile"
```

### `with_extension`

```unio
public function with_extension(path as Path, extension as Text) returns Path
```

The path with the extension of its name replaced, or removed when extension is empty. A path
without a name, such as a root, stays as it is.
Example:

```unio
assert with_extension(Path("dir/report.txt"), "md") == Path("dir").join("report.md")
assert "{with_extension(Path("notes"), "txt")}" == "notes.txt"
```

### `with_name`

```unio
public function with_name(path as Path, name as Text) returns Path
```

The path with its last name replaced; a path without a name, such as a root, gets the name.
Example:

```unio
assert with_name(Path("dir/old.txt"), "new.txt") == Path("dir").join("new.txt")
```

### `starts_with`

```unio
public function starts_with(path as Path, base as Path) returns Bool
```

Whether the components of base begin the components of path, compared exactly, after
Path.normalize, anchors included. A lexical test, not a containment check for access: ".." and
links are not resolved. A base of "." begins exactly the paths without an anchor: not "/a", and
on Windows neither "\\a" nor "C:a", which are relative but not to the current directory.
Example:

```unio
assert starts_with(Path("/srv/data/a.txt"), Path("/srv"))
assert not starts_with(Path("/srv2/a.txt"), Path("/srv"))
```

### `relative_to`

```unio
public function relative_to(path as Path, base as Path) returns Option[Path]
```

The path relative to base when base begins it (see starts_with); none otherwise. The result of
a path equal to base is ".".
Example:

```unio
assert relative_to(Path("/srv/data/a.txt"), Path("/srv")) == some(Path("data").join("a.txt"))
assert relative_to(Path("/srv"), Path("/home")) == none
```

### `join_all`

```unio
public function join_all(base as Path, parts as List[Text]) returns Path
```

The base with each part joined in turn, as Path.join does; "." as a base is dropped.
Example:

```unio
assert join_all(Path("a"), ["b", "c.txt"]) == Path("a").join("b").join("c.txt")
assert "{join_all(Path("."), [])}" == "."
```

### `is_relative`

```unio
public function is_relative(path as Path) returns Bool
```

Whether the path is not absolute; on Windows "C:file" and "\\file" are relative too.
Example:

```unio
assert is_relative(Path("a/b")) and is_relative(Path("C:a")) and is_relative(Path("\\a"))
# Absolute: "/a" on Linux and macOS, "C:\\a" on Windows.
assert not is_relative(Path("/a")) or not is_relative(Path("C:\\a"))
```

## `std.process`

Исходный текст: [stdlib/std/process.unio](../../stdlib/std/process.unio). Слой: `std`.

Running other programs (requirement S-6). A program is started directly, never through a shell,
so every argument reaches it exactly as given. Permission: process (the program must be listed).

### `ProcessErrorKind`

```unio
public enum ProcessErrorKind:
    NotFound
    PermissionDenied
    TimedOut
    InvalidOutput
    OutputTooLarge
    Other
```

### `ProcessError`

```unio
public error ProcessError:
    program as Text
    kind as ProcessErrorKind
    reason as Text
```

### `Output`

```unio
public struct Output:
    public status as Int
    public stdout as Text
    public stderr as Text
```

What a finished program reported.

### `run`

```unio
public external function run(
    program as Text,
    arguments as List[Text],
    timeout_seconds as Int = 60,
) returns Output fails with ProcessError
```

Runs the program with the arguments, waits at most timeout_seconds, and captures both outputs as
UTF-8 text. A program that does not finish in time is stopped and the call fails with kind
TimedOut; output that is not UTF-8 fails with kind InvalidOutput. A non-zero status is not an
error: the caller decides what it means. The child runs in the sandbox of the caller: on Linux
and macOS the system starts for it only the programs the manifest lists under process, the
interpreters of listed scripts and, on Linux, their dynamic loader, and it reads only the read
scopes and the system libraries those programs need. What it can run is wider than the list: a
listed interpreter runs any script it can read, and on Linux the loader runs any program it can
read (docs/runtime.md 9.5).

The child sees only the environment variables the manifest grants under env, and PATH (on
Windows also SystemRoot). It runs in a process tree of its own: a process group on Unix, which
its descendants cannot leave on Linux, a Job Object on Windows. When it ends, the processes it started that still run in that tree are
stopped; on a timeout the whole tree is stopped. The deadline covers reading the outputs too: if
a process keeps them open after the program ended, the call fails with kind TimedOut at the
deadline. Each of stdout and stderr holds at most 16 MiB (16_777_216 bytes); more fails with
kind OutputTooLarge and stops the tree. Captured output counts against the heap limit of the
policy, and exceeding it ends the program with code 102.
Example:

```unio
try:
    let output = try run("git", ["--version"], timeout_seconds = 10)
    if output.status == 0: assert output.stdout.starts_with("git")
on ProcessError as e:
    assert e.program == "git"
```

## `std.random`

Исходный текст: [stdlib/std/random.unio](../../stdlib/std/random.unio). Слой: `std`.

Randomness (requirement S-1), in two kinds that are kept apart.

A Generator gives pseudo-random numbers. It is a value: seeded with seeded(n), it gives the same
sequence on every platform and in every run, which makes tests and simulations reproducible;
from_entropy() seeds it from the random source of the operating system instead. The generator
is MRG32k3a (P. L'Ecuyer, "Good parameters and implementations for combined multiple recursive
random number generators", Operations Research 47(1), 1999), with a period of about 2^191,
computed in Int without overflow. It is fast and statistically good, but it is not for
cryptography: its future output follows from past output. Never use it for keys, tokens,
passwords or anything an attacker must not guess.

secure_bytes and secure_int are the only functions of the library fit for keys and secrets.
They read the random source of the operating system at every call (getentropy on Linux and
macOS, BCryptGenRandom on Windows), and when the system cannot give randomness they fail with
RandomError; they never fall back to a weaker generator.

Effects (docs/semantics.md 8.2, RFC 0009): obtaining randomness from outside the program is the
derived effect random, which from_entropy, secure_bytes and secure_int have; it needs no
permission. A Generator made by seeded has no effect: its seed and its state are explicit, and
drawing a number changes the state as the change of a var value it is (the methods take
var self), which the checker accounts as a mutation, not as an effect.

Tests: code that needs randomness takes a Generator, and a test passes seeded(n). Nothing can
replace the random source of the system with a seeded one: there is no global switch, variable
or policy for it, so from_entropy and the secure functions are never silently predictable.

### `Generator`

```unio
public struct Generator
```

The state of the generator: two recurrences of order three.

У структуры есть закрытые поля: её создают только функции её модуля.

### `seeded`

```unio
public function seeded(seed as Int) returns Generator
```

A generator whose sequence depends only on the seed: different seeds give different states.
Example:

```unio
var a = seeded(42)
var b = seeded(42)
assert a.next_int(0, 1000) == b.next_int(0, 1000)
```

### `from_entropy`

```unio
public function from_entropy() returns Generator
```

A generator seeded from 64 bits of the random source of the operating system, different in every
run. It has the effect random.
Example:

```unio
var generator = from_entropy()
assert generator.next_float() < 1.0
```

### `RandomError`

```unio
public error RandomError:
    reason as Text
```

The random source of the operating system could not give randomness.

### `secure_bytes`

```unio
public external function secure_bytes(count as Int) returns Bytes fails with RandomError
```

count bytes from the random source of the operating system, fit for keys, nonces and tokens.
A count below 0 or above 1048576 is a fault; a system that cannot give randomness fails with
RandomError, never with weaker bytes. It has the effect random.
Example:

```unio
let key = try secure_bytes(32) on RandomError as e:
    assert_fail(e.reason)
assert key.length() == 32
```

### `secure_int`

```unio
public external function secure_int(low as Int, high as Int) returns Int fails with RandomError
```

An Int in low..high, high excluded, each equally likely, from the random source of the operating
system: for secret choices such as a one-time code. low >= high is a fault; a system that cannot
give randomness fails with RandomError. It has the effect random.
Example:

```unio
let code = try secure_int(0, 1000000) on RandomError as e:
    assert_fail(e.reason)
assert code >= 0 and code < 1000000
```

### Методы `Generator`

```unio
extend Generator
```

#### `next_int`

```unio
public function next_int(var self, low as Int, high as Int) returns Int
```

An Int in low..high, high excluded, each equally likely. low >= high is a fault.
Example:

```unio
var generator = seeded(7)
let die = generator.next_int(1, 7)
assert die >= 1 and die <= 6
```

#### `next_float`

```unio
public function next_float(var self) returns Float
```

A Float in 0.0..1.0, 1.0 excluded, from 53 uniform bits.
Example:

```unio
var generator = seeded(1)
let x = generator.next_float()
assert x >= 0.0 and x < 1.0
```

#### `next_bool`

```unio
public function next_bool(var self) returns Bool
```

true or false, each with probability one half.
Example:

```unio
var generator = seeded(3)
let coin = generator.next_bool()
assert coin or not coin
```

#### `chance`

```unio
public function chance(var self, probability as Float) returns Bool
```

true with the given probability; 0.0 or less never, 1.0 or more always.
Example:

```unio
var generator = seeded(5)
assert generator.chance(1.0) and not generator.chance(0.0)
```

#### `choose`

```unio
public function choose[T](var self, items as List[T]) returns Option[T]
```

An item of the list, each equally likely; none for an empty list.
Example:

```unio
var generator = seeded(9)
assert generator.choose(["a", "b"]) in [some("a"), some("b")]
```

#### `shuffled`

```unio
public function shuffled[T](var self, items as List[T]) returns List[T]
```

The items in a random order, each order equally likely (Fisher–Yates).
Example:

```unio
var generator = seeded(11)
assert generator.shuffled([1, 2, 3]).sorted() == [1, 2, 3]
```

## `std.test`

Исходный текст: [stdlib/alloc/test.unio](../../stdlib/alloc/test.unio). Слой: `alloc`.

Deterministic property testing. Generators and predicates must be pure for replay.
This generator is for tests, never for cryptography. Scheme: unio-property-v1.

### `Counterexample`

```unio
public struct Counterexample[T]:
    public seed as Int
    public case_index as Int
    public sample_seed as Int
    public original as T
    public minimal as T
    public shrink_attempts as Int
```

### `PropertyError`

```unio
public error PropertyError:
    message as Text
```

### `check_property`

```unio
public function check_property[T](
    generate as Function[(Int) -> T],
    shrink as Function[(T) -> List[T]],
    holds as Function[(T) -> Bool],
    named seed as Int = 1,
    named cases as Int = 100,
    named max_shrinks as Int = 1000,
) returns Option[Counterexample[T]] fails with PropertyError
```

Returns the first counterexample, reduced in shrinker order within a finite attempt budget.
Reproduce the original with generate(result.sample_seed); "minimal" is local, not a proof of global minimality.
Example:

```unio
let problem = try check_property(sample, smaller, function (n as Int) returns Bool: n < 50, seed = 3) on PropertyError as e:
    assert_fail(e.message)
match problem:
    case some(found): assert found.minimal == 50 and found.original == sample(found.sample_seed)
    case none: assert_fail("expected a counterexample")
```

## `std.text`

Исходный текст: [stdlib/alloc/text.unio](../../stdlib/alloc/text.unio). Слой: `alloc`.

Operations on texts as a whole (requirements S-1, S-18): joining and splitting, characters from
code points, explicit Unicode normalization and grapheme clusters. Methods of a single text,
such as length, slice and replace, are in builtin/text.unio.

Text never normalizes by itself: equality compares the exact scalars, so "\u{E9}" and
"e\u{301}" differ until both are normalized with the same form. Normalization and grapheme
clusters use the Unicode Character Database of unicode_version(), pinned in the runtime; a newer
version may cluster new characters differently, while the normalization of an assigned
character never changes (the stability policy of Unicode).

### `join`

```unio
public external function join(parts as List[Text], separator as Text) returns Text
```

The parts with the separator between each two of them.
Example:

```unio
assert join(["a", "b", "c"], ", ") == "a, b, c"
assert join([], "-") == ""
```

### `split`

```unio
public external function split(text as Text, separator as Text) returns List[Text]
```

The text cut at every occurrence of the separator; an empty separator is a fault.
split("a.b", ".") is ["a", "b"] and split("", ".") is [""].
Example:

```unio
assert split("a..b", ".") == ["a", "", "b"]
```

### `from_code_point`

```unio
public external function from_code_point(code as Int) returns Option[Text]
```

The character with the code point; none for a surrogate or a value above 0x10FFFF.
Example:

```unio
assert from_code_point(0x41) == some("A") and from_code_point(0xD800) == none
```

### `to_nfc`

```unio
public external function to_nfc(text as Text) returns Text
```

Normalization Form C (canonical composition, Unicode Standard Annex #15): the canonical
decomposition, then each character combined with the preceding starter where Unicode composes
them. Canonically equivalent texts have the same NFC; use it before comparing or storing text
that may come in different spellings. The result may be shorter than the text.
Example:

```unio
assert to_nfc("e\u{301}") == "\u{E9}"
assert to_nfc("\u{E9}") == to_nfc("e\u{301}")
```

### `to_nfd`

```unio
public external function to_nfd(text as Text) returns Text
```

Normalization Form D (canonical decomposition): every character replaced by its canonical
decomposition, Hangul syllables by their jamo, and the combining marks in canonical order. The
result may be longer than the text.
Example:

```unio
assert to_nfd("\u{E9}") == "e\u{301}"
assert to_nfd("\u{E9}").length() == 2
```

### `graphemes`

```unio
public external function graphemes(text as Text) returns List[Text]
```

The extended grapheme clusters of the text (Unicode Standard Annex #29): what a reader sees as
one character, such as a letter with its marks, a flag or a family emoji, each as a Text. The
clusters joined give the text again. The cost is O(n) time and space, like the scalar index.
Example:

```unio
assert graphemes("e\u{301}x") == ["e\u{301}", "x"]
assert graphemes("\u{1F1FA}\u{1F1F8}") == ["\u{1F1FA}\u{1F1F8}"]
```

### `unicode_version`

```unio
public external function unicode_version() returns Text
```

The version of the Unicode Character Database that to_nfc, to_nfd and graphemes use.
Example:

```unio
assert unicode_version() == "15.0.0"
```

## `std.time`

Исходный текст: [stdlib/std/time.unio](../../stdlib/std/time.unio). Слой: `std`.

Time (requirement S-1): durations, the monotonic clock for measuring, the wall clock for
timestamps, and sleeping.

Reading the current time - now, monotonic and sleep - has the derived effect clock
(docs/semantics.md 8.2, RFC 0009): unio symbol shows it for every function that reads the time,
through calls and function values, and it needs no permission of the manifest. Operations on a
given duration, instant, timestamp or date have no effect: arithmetic, comparison, the calendar,
RFC 3339 text and ManualClock are pure. Code that must be tested at a fixed time takes a Clock
value, and the test passes a ManualClock; nothing changes global state (language contracts,
T-12). A call of now() on a Clock value whose type is not known at the call has the effect
clock, since SystemClock is among its implementations (8.4).

Duration, Instant and Timestamp count nanoseconds in an Int, so a Timestamp lies between the
years 1677 and 2262, and arithmetic that leaves that range is a fault, as for Int.

### `Duration`

```unio
public struct Duration
```

A signed length of time with nanosecond resolution.

У структуры есть закрытые поля: её создают только функции её модуля.

### `Instant`

```unio
public struct Instant
```

A reading of the monotonic clock. Only the difference of two readings of one run means anything:
the clock never goes back, and it does not follow changes of the system time.

У структуры есть закрытые поля: её создают только функции её модуля.

### `Timestamp`

```unio
public struct Timestamp
```

A point in time on the wall clock (UTC), in nanoseconds since 1970-01-01T00:00:00Z, without leap
seconds, as the operating system counts it. The text form is RFC 3339 in UTC.

У структуры есть закрытые поля: её создают только функции её модуля.

### `DateTime`

```unio
public struct DateTime:
    public year as Int
    public month as Int
    public day as Int
    public hour as Int = 0
    public minute as Int = 0
    public second as Int = 0
    public nanosecond as Int = 0
```

The fields of a Timestamp in the UTC calendar (the proleptic Gregorian calendar).

### `TimeError`

```unio
public error TimeError:
    text as Text
    message as Text
```

A text that is not a timestamp of RFC 3339, or fields that are not a date and time.

### `nanoseconds`

```unio
public function nanoseconds(n as Int) returns Duration
```

A duration of n nanoseconds.
Example:

```unio
assert nanoseconds(1500).total_nanoseconds() == 1500
```

### `microseconds`

```unio
public function microseconds(n as Int) returns Duration
```

A duration of n microseconds; one that does not fit in Int nanoseconds is a fault.
Example:

```unio
assert microseconds(2).total_nanoseconds() == 2000
```

### `milliseconds`

```unio
public function milliseconds(n as Int) returns Duration
```

A duration of n milliseconds.
Example:

```unio
assert milliseconds(1500) == seconds(1) + milliseconds(500)
```

### `seconds`

```unio
public function seconds(n as Int) returns Duration
```

A duration of n seconds.
Example:

```unio
assert seconds(90) == minutes(1) + seconds(30)
```

### `minutes`

```unio
public function minutes(n as Int) returns Duration
```

A duration of n minutes.
Example:

```unio
assert minutes(60) == hours(1)
```

### `hours`

```unio
public function hours(n as Int) returns Duration
```

A duration of n hours.
Example:

```unio
assert hours(24).total_seconds() == 86400
```

### Методы `Duration`

```unio
extend Duration
```

#### `total_nanoseconds`

```unio
public function total_nanoseconds(self) returns Int
```

The length in nanoseconds.
Example:

```unio
assert milliseconds(3).total_nanoseconds() == 3000000
```

#### `total_milliseconds`

```unio
public function total_milliseconds(self) returns Int
```

The length in whole milliseconds, truncated toward zero.
Example:

```unio
assert microseconds(2500).total_milliseconds() == 2
```

#### `total_seconds`

```unio
public function total_seconds(self) returns Int
```

The length in whole seconds, truncated toward zero.
Example:

```unio
assert milliseconds(-1500).total_seconds() == -1
```

#### `as_seconds`

```unio
public function as_seconds(self) returns Float
```

The length in seconds as a Float, for display and rates.
Example:

```unio
assert milliseconds(250).as_seconds() == 0.25
```

#### `is_negative`

```unio
public function is_negative(self) returns Bool
```

Whether the duration is below zero.
Example:

```unio
assert (seconds(1) - seconds(2)).is_negative()
```

### `Duration` реализует `Add`

```unio
extend Duration with Add
```

#### `add`

```unio
function add(self, other as Self) returns Self
```

The sum; a fault on overflow.

### `Duration` реализует `Subtract`

```unio
extend Duration with Subtract
```

#### `subtract`

```unio
function subtract(self, other as Self) returns Self
```

The difference; a fault on overflow.

### `Duration` реализует `Order`

```unio
extend Duration with Order
```

#### `compare`

```unio
function compare(self, other as Self) returns Int
```

### `Duration` реализует `Hash`

```unio
extend Duration with Hash
```

#### `hash`

```unio
function hash(self) returns Int
```

### `Duration` реализует `Display`

```unio
extend Duration with Display
```

#### `to_text`

```unio
function to_text(self) returns Text
```

Seconds with the fraction that is needed and an "s": "1.5s", "0s", "-0.000001s".

### Методы `Instant`

```unio
extend Instant
```

#### `since`

```unio
public function since(self, earlier as Instant) returns Duration
```

The time from earlier to this reading; negative when earlier is later.
Example:

```unio
assert monotonic().since(monotonic()) <= seconds(0)
```

#### `plus`

```unio
public function plus(self, duration as Duration) returns Instant
```

The reading the duration after this one.
Example:

```unio
let start = monotonic()
assert start.plus(seconds(1)) > start
```

### `Instant` реализует `Order`

```unio
extend Instant with Order
```

#### `compare`

```unio
function compare(self, other as Self) returns Int
```

### Методы `Timestamp`

```unio
extend Timestamp
```

#### `unix_nanoseconds`

```unio
public function unix_nanoseconds(self) returns Int
```

Nanoseconds since 1970-01-01T00:00:00Z; negative before it.
Example:

```unio
assert from_unix_seconds(2).unix_nanoseconds() == 2000000000
```

#### `unix_seconds`

```unio
public function unix_seconds(self) returns Int
```

Whole seconds since 1970-01-01T00:00:00Z, rounded toward negative infinity.
Example:

```unio
assert from_unix_nanoseconds(-1).unix_seconds() == -1
```

#### `since`

```unio
public function since(self, earlier as Timestamp) returns Duration
```

The time from earlier to this timestamp; negative when earlier is later.
Example:

```unio
assert from_unix_seconds(60).since(from_unix_seconds(0)) == minutes(1)
```

#### `plus`

```unio
public function plus(self, duration as Duration) returns Timestamp
```

The timestamp the duration after this one; a negative duration goes back.
Example:

```unio
assert from_unix_seconds(0).plus(hours(1)).unix_seconds() == 3600
```

#### `to_utc`

```unio
public function to_utc(self) returns DateTime
```

The calendar fields in UTC.
Example:

```unio
assert from_unix_seconds(86400).to_utc() == DateTime(year = 1970, month = 1, day = 2)
```

### `Timestamp` реализует `Order`

```unio
extend Timestamp with Order
```

#### `compare`

```unio
function compare(self, other as Self) returns Int
```

### `Timestamp` реализует `Hash`

```unio
extend Timestamp with Hash
```

#### `hash`

```unio
function hash(self) returns Int
```

### `Timestamp` реализует `Display`

```unio
extend Timestamp with Display
```

#### `to_text`

```unio
function to_text(self) returns Text
```

RFC 3339 in UTC with the fraction that is needed: "2026-11-12T08:30:00Z",
"1969-12-31T23:59:59.5Z". parse_timestamp reads it back.

### `from_unix_seconds`

```unio
public function from_unix_seconds(n as Int) returns Timestamp
```

The timestamp n seconds after 1970-01-01T00:00:00Z.
Example:

```unio
assert "{from_unix_seconds(0)}" == "1970-01-01T00:00:00Z"
```

### `from_unix_nanoseconds`

```unio
public function from_unix_nanoseconds(n as Int) returns Timestamp
```

The timestamp n nanoseconds after 1970-01-01T00:00:00Z.
Example:

```unio
assert "{from_unix_nanoseconds(1500000000)}" == "1970-01-01T00:00:01.5Z"
```

### `from_utc`

```unio
public function from_utc(fields as DateTime) returns Timestamp fails with TimeError
```

The timestamp of calendar fields in UTC; fields that are not a valid date and time, or a time
outside the years 1677 to 2262, fail with TimeError.
Example:

```unio
let leap_day = try from_utc(DateTime(year = 2000, month = 2, day = 29)) on TimeError as e:
    assert_fail(e.message)
assert "{leap_day}" == "2000-02-29T00:00:00Z"
```

### `parse_timestamp`

```unio
public function parse_timestamp(text as Text) returns Timestamp fails with TimeError
```

Reads an RFC 3339 timestamp: date, "T", time with an optional fraction of up to nine digits, and
"Z" or an offset such as "+02:00". The result is the same instant in UTC.
Example:

```unio
let parsed = try parse_timestamp("2026-11-12T10:30:00+02:00") on TimeError as e:
    assert_fail(e.message)
assert "{parsed}" == "2026-11-12T08:30:00Z"
```

### `Clock`

```unio
public interface Clock
```

A source of the current time. Functions that need the time take a Clock, so that a test can pass
a ManualClock instead of SystemClock.

#### `now`

```unio
function now(self) returns Timestamp
```

The current time of the wall clock.

#### `monotonic`

```unio
function monotonic(self) returns Instant
```

The current reading of the monotonic clock.

### `SystemClock`

```unio
public struct SystemClock
```

The clocks of the operating system: now() and monotonic() of this module.
Example:

```unio
assert SystemClock().now() > from_unix_seconds(0)
```

### `SystemClock` реализует `Clock`

```unio
extend SystemClock with Clock
```

#### `now`

```unio
function now(self) returns Timestamp
```

#### `monotonic`

```unio
function monotonic(self) returns Instant
```

### `ManualClock`

```unio
public struct ManualClock
```

A clock for tests: it stands still until advance moves both of its times. Made by manual_clock.
Example:

```unio
var clock = manual_clock(from_unix_seconds(0))
clock.advance(seconds(5))
assert clock.now().unix_seconds() == 5
```

У структуры есть закрытые поля: её создают только функции её модуля.

### `manual_clock`

```unio
public function manual_clock(at as Timestamp) returns ManualClock
```

A ManualClock at the wall time, with its monotonic reading at 0.
Example:

```unio
assert manual_clock(from_unix_seconds(7)).now() == from_unix_seconds(7)
```

### Методы `ManualClock`

```unio
extend ManualClock
```

#### `advance`

```unio
public function advance(var self, duration as Duration)
```

Moves the wall time and the monotonic reading forward by the duration; a negative duration is
a fault, since a monotonic clock never goes back.
Example:

```unio
var clock = manual_clock(from_unix_seconds(0))
let start = clock.monotonic()
clock.advance(minutes(1))
assert clock.monotonic().since(start) == minutes(1)
```

#### `set`

```unio
public function set(var self, at as Timestamp)
```

Sets the wall time, forward or back, as when the system time is changed; the monotonic reading
stays.
Example:

```unio
var clock = manual_clock(from_unix_seconds(0))
clock.set(from_unix_seconds(-5))
assert clock.now().unix_seconds() == -5
```

### `ManualClock` реализует `Clock`

```unio
extend ManualClock with Clock
```

#### `now`

```unio
function now(self) returns Timestamp
```

#### `monotonic`

```unio
function monotonic(self) returns Instant
```

### `now`

```unio
public function now() returns Timestamp
```

The current time of the wall clock of the system. It can jump when the system time is set; use
monotonic() to measure.
Example:

```unio
assert now() > from_unix_seconds(1577836800)
```

### `monotonic`

```unio
public function monotonic() returns Instant
```

The current reading of the monotonic clock.
Example:

```unio
let start = monotonic()
assert not monotonic().since(start).is_negative()
```

### `sleep`

```unio
public function sleep(duration as Duration)
```

Suspends the program for at least the duration; zero or a negative duration returns at once.
Example:

```unio
let start = monotonic()
sleep(milliseconds(2))
assert monotonic().since(start) >= milliseconds(2)
```
