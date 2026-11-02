# Unio: refined concept

Version: 0.3. This document refines the original concept from the README and fixes the decisions without which implementation cannot start. There are no open questions; the rationale for the latest decisions is in section 11.

## 1. Purpose and principle

Unio is a general-purpose language that:

- is easy for people to read, including people who did not write the code;
- is convenient for AI to generate, because the vocabulary of constructs is small, there is one style, and compiler errors are precise and machine-readable;
- can be strictly checked before it runs: types, errors, resources, and side effects are visible in signatures.

Guiding principle: **clear code, explicit behavior, controlled resources**.

Consequences of the principle, against which every language decision is checked:

1. One task, one way to write it. If there are two ways, one is declared standard and the formatter rewrites to it.
2. Nothing happens implicitly: no implicit type conversions, no hidden copying of large data, no hidden exceptions.
3. A function signature is a complete contract: input, result, errors, and later effects.
4. Everything that can go wrong is either expressed in a type (`Option`, `fails with`) or is a fault with defined behavior.

## 2. Syntax

### 2.1 General rules

- Blocks are defined by indentation of 4 spaces. Tabs are forbidden.
- A block header ends with a colon.
- Keywords are whole English words, no abbreviations: `function`, `returns`, `fails with`, `struct`, `interface`, `enum`, `match`, `case`.
- Identifiers: `snake_case` for functions, variables, fields, and modules; `PascalCase` for types. The formatter and compiler warn about deviations.
- Comments start with `#`. Documentation comments start with `##` and precede a declaration.
- Source files are UTF-8 with the `.unio` extension.

### 2.2 Value declarations

```unio
let limit = 10            # immutable value, type Int inferred
var count = 0             # mutable variable
count = count + 1

let name as Text = read_line()   # explicit annotation allowed
```

Rule: `let` by default. `var` is required for anything that is reassigned. Struct fields are immutable unless marked `var`.

### 2.3 Functions

```unio
## Returns a greeting for the name.
function greet(name as Text) returns Text:
    return "Hello, {name}!"

function active_names(users as List[User]) returns List[Text]:
    return [user.name for user in users if user.active]

function log(message as Text):        # no returns: returns nothing
    print(message)
```

- Parameters are always annotated with a type. The result type is always stated when a function returns something. Only local value types are inferred.
- Arguments are passed positionally or by name: `connect(host, port = 8080)`. Default values are allowed only for trailing parameters.
- Functions are first-class values. Function type: `Function[(Int, Text) -> Bool]`. Anonymous function: `function (x as Int) returns Int: x * 2`, single-line form.

### 2.4 Text and operators

- Interpolation `"{expression}"` is the standard way to build text. Any expression whose value implements `Display` may appear inside the braces.
- The `+` operator is defined for numbers and for `Text + Text`. Mixing types (`Text + Int`) is a compile error with a hint to use interpolation. Familiar notation is preserved while implicit conversions are absent.
- Arithmetic: `+ - * / %` and `**`. `/` on `Int` gives `Int` with truncation; on `Float` gives `Float`. `Int` overflow is a fault (see 3.3), not silent wraparound.
- Comparisons `== != < <= > >=`; logic `and or not`. Logical operators are words so code reads as text.
- Membership: `item in collection`.
- Operators are bound to standard interfaces: `+` to `Add`, `-` to `Subtract`, `*` to `Multiply`, `/` to `Divide`, `==` to `Equal`, `<` and the other comparisons to `Order`. Built-in numbers and `Text` implement them. A user-defined type gets an operator only through an explicit declaration `extend Money with Add`; there are no hidden implementations and no implicit conversions. Operators cannot be redefined for types that already implement them.

### 2.5 Control flow

```unio
if count > limit:
    stop()
else if count == limit:
    warn()
else:
    continue_work()

for user in users:
    print(user.name)

for index, user in users.enumerate():
    print("{index}: {user.name}")

while not queue.is_empty():
    process(queue.pop())

match command:
    case Add(item):
        items.append(item)
    case Remove(id) if id > 0:
        items.remove(id)
    case Quit:
        return
```

`match` must be exhaustive: the compiler checks that all variants are listed or that there is a `case _`.

### 2.6 Data types

```unio
struct User:
    id as Int
    name as Text
    var active as Bool = true

enum Command:
    Add(item as Text)
    Remove(id as Int)
    Quit

interface Display:
    function to_text(self) returns Text

extend User with Display:
    function to_text(self) returns Text:
        return "{self.name} (#{self.id})"
```

- `struct` is a record with named fields, constructed as `User(id = 1, name = "Anna")`. Struct values compare by content.
- `enum` is a sum type with variants; variants may have fields. Used for commands, states, and parse results.
- `interface` is a set of methods. Implementation is declared explicitly with `extend ... with ...`. There is no implementation inheritance; reuse goes through composition and generic functions.
- Generics: `function first[T](items as List[T]) returns Option[T]`. Constraints: `[T with Display]`.

Built-in core types: `Int` (64-bit signed), `Float` (64-bit), `Bool`, `Text`, `Bytes`, `List[T]`, `Map[K, V]`, `Set[T]`, `Option[T]`, `Tuple`, `Path`, `Function`.

Equality:

- `struct` and `enum` compare by content when all fields implement `Equal`; the implementation is derived automatically and may be replaced by an explicit one.
- `Float` implements `Equal` and `Order` per IEEE 754: `nan == nan` is `false`, and comparisons with `nan` are `false`. `Float` does not implement `Hash`, so it cannot be a `Map` key, a `Set` element, or a `match` pattern. For sorting and keys there is `float.total_order()`; for checks, `float.is_nan()`.
- Resources (section 4) do not implement `Equal`: `==` on them is a compile error. Identity is checked with the function `is_same(a, b)`. This keeps a reader from confusing comparison of file contents with comparison of handles.
- `Decimal` (exact decimal arithmetic for money) is not a built-in type but a struct in `std.decimal` that implements `Add`, `Subtract`, `Multiply`, `Divide`, `Equal`, `Order`, and `Hash`. It is created explicitly: `Decimal("19.99")`; the core has no literal for it. This keeps the core vocabulary small while preserving the familiar notation `price * quantity`.

### 2.7 Mutability and values

- All built-in collections have an immutable interface by default. Mutating methods (`append`, `remove`, `sort`) are available only on values declared with `var` and are marked as such in documentation.
- Passing a struct to a function passes a reference to an immutable value. A function cannot modify an argument unless the parameter is declared `var name as List[Int]`; the caller must then pass a `var` value.
- Copying is explicit: `items.copy()`.

This is enough for a reader who sees `var` in a signature to understand that the function modifies the state passed to it.

## 3. Missing values and errors

### 3.1 Option

```unio
function find_user(id as Int) returns Option[User]:
    for user in users:
        if user.id == id:
            return some(user)
    return none

let user = find_user(7) or default_user
let name = find_user(7)?.name            # Option[Text]

match find_user(7):
    case some(user):
        print(user.name)
    case none:
        print("not found")
```

`Option[T]` is an ordinary `enum` with variants `some(value)` and `none`. There is no `null`. The `or` operator on `Option` supplies a default; `?.` propagates `none` through a chain.

### 3.2 Errors in the signature

```unio
error IoError:
    path as Path
    reason as Text

error ParseError:
    line as Int
    message as Text

function read_config(path as Path) returns Config fails with IoError, ParseError:
    let text = try read_text(path)                 # IoError propagates
    let parsed = try parse_toml(text)              # ParseError propagates
    return Config.from(parsed)
```

Rules:

1. `error` declares an error type; it is a struct with fields. Errors are values, not exceptions.
2. A function that can end in an error lists the errors after `fails with`. The error set is part of the function's type.
3. `fail IoError(path = path, reason = "access denied")` ends the function with an error.
4. Calling a function with `fails with` without handling is a compile error. There are two ways to handle:
   - prefix `try` before the call: the error propagates to the caller, which must list it in its own `fails with`;
   - a `try:` block with `on` handlers.

```unio
try:
    let config = read_config(path)
    run(config)
on IoError as e:
    print("Could not read {e.path}: {e.reason}")
on ParseError as e:
    print("Line {e.line}: {e.message}")
```

5. The compiler checks that every error from the callee's `fails with` is either handled in `on` or listed in the current function's `fails with`. The handler `on Error as e` catches any error; `Error` is the common interface of all errors with the method `to_text`.
6. For scripts and prototypes the signature `fails with Error` is allowed: the concrete set is not fixed, but the fact that an error is possible stays visible.
7. For functions where an error is needed as a value there is the type `Result[T, E]` with variants `ok(value)` and `error(value)`; `try` works with it too. In the standard library `Result` is used only where errors must be stored or passed on.

### 3.3 Faults

A fault is a contract violation that cannot be handled inside the program: index out of range in `items[i]`, division by zero, `Int` overflow, out of memory, a failed `assert`.

- Behavior is defined: the program prints a diagnostic with a stack trace to stderr and exits with code 101. Open resources in `using` blocks are closed.
- A fault cannot be caught with `on`. It can be observed only in tests (`expect_fault`) and at the process boundary.
- Every faulting operation has a safe form returning `Option` or an error: `items.get(i)`, `a.checked_divide(b)`, `a.checked_add(b)`.

Domain errors and program defects are thus distinguished syntactically, and the reader sees exactly what can happen.

## 4. Memory and resources

### 4.1 Two models

- **Managed values.** Structs, collections, text, and closures live on the heap under the GC. The programmer does not think about freeing them.
- **Resources.** Files, sockets, processes, device handles, and in the compute extension tensors and GPU buffers. They have an explicit lifetime.

The boundary between the models: a type implements the interface `Resource` (method `release`), therefore it is a resource. Such a value:

- is created in a `using` block or as a field of another resource;
- is released on leaving the block, in reverse order of creation, including on error and on fault;
- cannot be stored in an ordinary struct or collection that outlives the block; that is a compile error. If a resource must be stored, the owner itself is declared a `Resource` and releases it in `release`.

```unio
function count_lines(path as Path) returns Int fails with IoError:
    using file = try open_file(path):
        var count = 0
        for line in file.lines():
            count = count + 1
        return count
```

### 4.2 What the first release includes

The first release contains the GC and resources with `using`. Memory budgets, lifetimes for tensors, and accounting for GPU computation completion belong to the compute extension and are designed on top of the same `Resource` interface: a tensor is a resource; a lifetime scope is an owning resource with a budget.

### 4.3 Garbage collector

- Precise, tracing, non-moving in the first version. Ease of debugging matters more than peak throughput.
- Pauses are bounded for CLI workloads; measured and published.
- `Resource` does not depend on the GC: release is deterministic and does not wait for a collection.

## 5. Modules and packages

```unio
import std.files
import std.json as json
from std.text import split, join

let data = json.parse(text)
```

- A module is a file. A package is a directory with the manifest `unio.toml`. The module name matches the file path.
- Visibility: everything is private by default; export with `public` before a declaration.
- Dependencies are pinned in `unio.lock` with content hashes. Offline builds are reproducible.
- One standard way to import and one order (the formatter sorts).

## 6. Security

- All first-release code is written in the safe subset: no pointers, no manual memory management, bounds checks, initialized values.
- Unsafe operations appear only in external functions (`external function`, FFI) and only in modules marked `unsafe module`. Such modules are listed in the manifest and shown when a dependency is installed.
- Least privilege: the manifest declares what the package needs (`files`, `network`, `process`, `env`). The compiler forbids using standard-library modules outside the declared permissions. This answers the reader's question "what can this program do" before it runs.
- Per-function effects in the signature (`uses files`) are not declared in the first release. The compiler derives effects from the call graph and shows them in `unio symbol` and in documentation: the reader and the AI see that a function touches files or processes without growing the language vocabulary. The word `uses` is reserved for the compute extension (`uses gpu`), where an effect influences resource scheduling, not only permissions.
- Dependencies: registry signatures, hash verification, warnings about `unsafe module`, no network actions during build.
- Release: reproducible build, signed artifacts, SBOM.
- Separate reliability tests: behavior under out-of-memory, under a fault inside `using`, under an error in external code.

## 7. Working with AI

What an AI agent gets from the tooling:

1. **Diagnostics in JSON**: error code, span in the file, message, explanation, suggested fixes as patches, related symbols. The same information a person sees, without parsing text.
2. **Symbol context**: `unio symbol app.users.find_user` returns the signature, documentation, usages, result type, and error set.
3. **Check without build**: `unio check` fast enough for the loop "generate, check, fix".
4. **One style**: `unio fmt` makes generated code indistinguishable from hand-written code, so review looks at meaning, not layout.
5. **Tests next to code**: `test "description":` blocks in the same file, run with `unio test --json`.

What is required from the AI: describe intent in documentation comments and tests; the code stays unambiguous.

## 8. AI computation and the quantum extension

Not part of the first release. Constraints on the core so that the extensions remain possible:

- the `Resource` type and `using` are sufficient for tensors and lifetime scopes;
- generics and interfaces are sufficient for operations on tensors of different types and devices;
- effects (`uses gpu`) can be added to signatures later without changing existing syntax;
- the profiler is designed with a common event format to which RAM/VRAM will be added later.

Quantum module: circuits as values, a simulator as a backend, hardware through the same backend interface with a capability description. Speedup is not promised; verifiability of a circuit before execution is guaranteed.

## 9. Complete example

See `docs/examples/todo.unio`: a command-line utility with a task list, a storage file, argument parsing, and error handling. The example shows every construct of the core.

## 10. Settled questions

| Question | Decision |
|---|---|
| `+` for text | Only `Text + Text` is allowed; interpolation is the standard |
| How `Option`, `fails with`, and `Result` relate | `Option` is absence of a value; `fails with` is a function error; `Result` is an error as a stored value; `try` works with the latter two |
| What happens on `items[i]` out of range | Fault with exit code 101, uncatchable; safe form `get` |
| Boundary between GC and resources | A type implementing `Resource` is a resource and lives in `using` |
| Compiler error format | JSON with code, span, explanation, and patch; fixed in the requirements before the first release |
| Concurrency in the core | None. The core has no shared mutable state between threads, so a concurrency model can be added later without breakage |
| `Decimal` in the core | No. `std.decimal` as a struct with the standard operator interfaces; explicit construction `Decimal("19.99")` |
| Operators for user-defined types | Only through explicit `extend T with Add` and the other standard interfaces; redefining existing implementations is forbidden |
| Effects in signatures | Not in 0.1: package-level permissions; function effects are derived by the compiler and shown in `unio symbol`. `uses` is reserved for 0.3 |
| `==` for `Float` | IEEE 754; `Float` without `Hash`, not a key and not a `match` pattern; `total_order()` and `is_nan()` |
| `==` for resources | Compile error; identity via `is_same(a, b)` |
| Compiler implementation language | Rust with code generation through Cranelift in 0.1; self-hosted implementation in Unio by 1.0, once the language is stable |

## 11. Rationale for the latest decisions

The decisions on questions left open in version 0.1 of the concept follow from the principle "clear code, explicit behavior, controlled resources".

1. **`Decimal` outside the core.** A small vocabulary of constructs matters more than convenience for one domain. Familiar arithmetic notation is preserved because operators are bound to interfaces, not to built-in types. Explicit `Decimal("19.99")` rules out hidden rounding when mixing with `Float`.
2. **Effects without syntax.** Package permissions already answer "what can the program do". Function effects are useful to the reader and the AI, but they can be computed rather than required in writing; signatures stay short and no information is lost. Once an effect influences execution (GPU), it becomes part of the signature through `uses`.
3. **`Float` per IEEE 754.** Deviating from the standard would make numeric code surprising to everyone who knows other languages. The dangerous spots are closed by forbidding `Hash` and `match` patterns, not by changing the operator's semantics.
4. **Resources without `==`.** Comparing handles has no obvious meaning for a reader: contents or identity? A compile error with the hint `is_same` makes the choice explicit.
5. **Rust and Cranelift.** Unio's safe subset rests on a safe compiler; Rust provides that without manual memory checking. Cranelift gives fast, deterministic builds, which matters more than peak optimization for CLI utilities and for the "generate, check" loop. LLVM may become a second backend for `--release` if the 0.2 benchmarks require it. Self-hosting moves to 1.0: writing a compiler in a language that is still changing means writing it twice.
