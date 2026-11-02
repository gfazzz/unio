# Standard library API: 0.1-draft subset

Version 0.1-draft. Russian localization: `docs/ru/stdlib.md`. Basis: requirements section 5, concept sections 2–4.

This document gives the part of the standard library that the reference programs use: `docs/examples/todo.unio` and `examples/`. The reference programs call only what is listed here; `spec/check_api.py` checks this. The remaining modules of requirement S-1 (`std.collections`, `std.math`, `std.decimal`, `std.path`, `std.process`, `std.time`, `std.random`, `std.log`, `std.test`) are specified in stage 5 of the plan in the same form.

Signatures are written in Unio without bodies. Each code block is checked against `spec/grammar.ebnf`. A comment `# permission: ...` names the manifest permission (requirement T-6) that the function needs; a function without such a comment needs none.

## Rules common to all modules

- A function that can fail lists concrete error types (S-2); none uses `fails with Error`.
- An operation blocked by the sandbox ends with the declared error, with `kind = PermissionDenied` (SB-6). A function that reports absence instead of an error (`files.exists`, `env.get`) reports a blocked object as absent.
- Every faulting operation has a safe form (L-18): `items[i]` and `items.get(i)`, `a / b` and `a.checked_divide(b)`.
- Methods marked `var self` change the value and are available only on a `var` place (L-11).

## Prelude

Available in every module without an import.

```unio
interface Display:
    function to_text(self) returns Text

interface Error:
    function to_text(self) returns Text

interface Equal:
    function equals(self, other as Self) returns Bool

interface Order:
    function compare(self, other as Self) returns Int

interface Hash:
    function hash(self) returns Int

interface Add:
    function add(self, other as Self) returns Self

interface Subtract:
    function subtract(self, other as Self) returns Self

interface Multiply:
    function multiply(self, other as Self) returns Self

interface Divide:
    function divide(self, other as Self) returns Self

interface Resource:
    function release(var self)
```

`Self` inside an interface stands for the type that implements it. `Option[T]` has the variants `some(value)` and `none`; `Result[T, E]` has `ok(value)` and `error(value)`. These four names are always visible and cannot be declared by user code.

```unio
function print(text as Text)
function print_error(text as Text)
function assert_fail(message as Text) returns Never
function expect_fault(action as Function[()])
function is_same[T with Resource](a as T, b as T) returns Bool
```

`print` writes the text and a line end to standard output, `print_error` to standard error. `assert_fail` ends a test, or a program, with a fault; its result type `Never` means it does not return, so it may end an inline handler (L-14a). `expect_fault` is available only inside `test` blocks.

## Built-in types

### Text

Text is a sequence of Unicode characters stored as UTF-8. Positions and lengths count characters; `byte_length` counts bytes.

```unio
function length(self) returns Int
function byte_length(self) returns Int
function is_empty(self) returns Bool
function contains(self, part as Text) returns Bool
function starts_with(self, prefix as Text) returns Bool
function count(self, part as Text) returns Int
function to_lower(self) returns Text
function lines(self) returns List[Text]
function split_whitespace(self) returns List[Text]
function to_int(self) returns Option[Int]
```

`count` counts non-overlapping occurrences. `lines` splits on `\n` and `\r\n` and drops one final empty line. `to_int` accepts an optional sign and decimal digits and returns `none` for anything else, including overflow.

### List[T]

```unio
function length(self) returns Int
function is_empty(self) returns Bool
function get(self, index as Int) returns Option[T]
function find(self, predicate as Function[(T) -> Bool]) returns Option[T]
function find_index(self, predicate as Function[(T) -> Bool]) returns Option[Int]
function map[U](self, change as Function[(T) -> U]) returns List[U]
function enumerate(self, start as Int = 0) returns List[Tuple[Int, T]]
function max(self) returns Option[T]
function append(var self, item as T)
```

`items[index]` with an index outside `0..length()` is a fault; `get` is its safe form. `max` requires `T with Order`. `==` compares lists element by element.

### Map[K, V]

`K` implements `Equal` and `Hash`.

```unio
function length(self) returns Int
function get(self, key as K) returns Option[V]
```

### Path

```unio
function Path(text as Text) returns Path
function join(self, part as Text) returns Path
function to_text(self) returns Text
```

`Path` implements `Display`, `Equal`, and `Hash`. `join` adds one component with the separator of the platform.

## std.args

```unio
function list() returns List[Text]
```

`list` returns the program arguments without the program name. Flags, defaults, and automatic help (S-4) are added in stage 5.

## std.env

```unio
# permission: env (the name must be listed)
function get(name as Text) returns Option[Text]
function home_dir() returns Option[Path]
```

`get` returns `none` for a variable that is not set or not permitted. `home_dir` asks the operating system rather than the environment and needs no permission; it returns `none` when the system has no home directory for the user.

## std.files

```unio
enum IoErrorKind:
    NotFound
    PermissionDenied
    AlreadyExists
    InvalidData
    Other

error IoError:
    path as Path
    kind as IoErrorKind
    reason as Text
```

```unio
# permission: files.read
function exists(path as Path) returns Bool
# permission: files.read
function read_text(path as Path) returns Text fails with IoError
# permission: files.write
function write_text(path as Path, text as Text, durable as Bool = false) fails with IoError
# permission: files.write
function open_atomic(path as Path, durable as Bool = false) returns AtomicFile fails with IoError
```

`exists` returns `false` for a path that does not exist or cannot be accessed. `read_text` fails with kind `InvalidData` if the file is not valid UTF-8. `write_text` replaces the file atomically (S-9): a reader sees either the old or the new complete content; `durable = true` also syncs the file and its directory to storage.

`AtomicFile` is a resource. Its content replaces the target only when its `using` block completes normally; on error or fault the target stays unchanged.

```unio
function write(var self, text as Text) fails with IoError
function write_line(var self, text as Text) fails with IoError
function release(var self)
```

## std.json

```unio
enum Value:
    Null
    Bool(value as Bool)
    Int(value as Int)
    Float(value as Float)
    Text(value as Text)
    Array(items as List[Value])
    Object(fields as Map[Text, Value])

error ParseError:
    line as Int
    column as Int
    message as Text

error DecodeError:
    path as Text
    message as Text
```

The variants of `Value` share names with built-in types, so they are always written with the enum name: `Value.Text("Anna")` (L-6a).

```unio
function parse(text as Text) returns Value fails with ParseError
function encode[T](value as T, pretty as Bool = false) returns Text
function decode[T](value as Value) returns T fails with DecodeError
function kind_name(value as Value) returns Text
```

`parse` reads a number without a fraction or exponent that fits in `Int` as `Value.Int`, any other number as `Value.Float`. `encode` and `decode` work for `Value`, `Int`, `Float`, `Bool`, `Text`, `Path`, `Option`, `List`, `Map` with `Text` keys, and structs and enums built from them; the compiler checks this for `T` at the call (S-5). A struct is encoded as an object with its field names; a field with a default may be missing on decoding. An enum variant without fields is encoded as its name, one with fields as an object `{"Variant": {fields}}`. `DecodeError.path` is the dotted path to the value that did not match, for example `"2.title"`. `kind_name` returns `"null"`, `"bool"`, `"int"`, `"float"`, `"text"`, `"array"`, or `"object"`.

## std.text

```unio
function join(parts as List[Text], separator as Text) returns Text
function split(text as Text, separator as Text) returns List[Text]
```

`split` with an empty separator is a fault; `split("", ".")` returns `[""]`.
