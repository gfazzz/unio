# Unio: refined concept

Version: 0.6. Changes from 0.5: library layers and build profiles, and the plan for system programming (section 8.1).

Changes from 0.4: file writes keep their guarantees after power loss on named file systems, and the compiler and package tools run isolated (sections 3.3, 6).

Changes from 0.3: the language is built in the language (section 1.1); the sandbox is mandatory (section 6), the rule for calling a function with `fails with` is the same in every position (section 3.2), and the guarantees on termination are split by kind (sections 3.3, 4.1), values are never shared between names (section 2.7), and variants with a colliding name are written in full (section 2.6). This document refines the original concept from the README and fixes the decisions without which implementation cannot start. There are no open questions; the rationale for the latest decisions is in section 11.

## 1. Purpose and principle

Unio is a general-purpose language that:

- is easy for people to read, including people who did not write the code;
- is convenient for AI to generate, because the vocabulary of constructs is small, there is one style, and compiler errors are precise and machine-readable;
- can be strictly checked before it runs: types, errors, resources, and side effects are visible in signatures.

Guiding principle: **clear code, explicit behavior, controlled resources**.

Consequences of the principle, against which every language decision is checked:

1. One task, one way to write it. If there are two ways, one is declared standard and the formatter rewrites to it.
2. Nothing happens implicitly: no implicit type conversions, no state shared between names behind the reader's back, no hidden exceptions.
3. A function signature is a complete contract: input, result, errors, and later effects.
4. Everything that can go wrong is either expressed in a type (`Option`, `fails with`) or is a fault with defined behavior.

### 1.1 The language is built in the language

Unio is developed in Unio wherever that is possible: the standard library, the project tools, the reference utilities, and, by 1.0, the compiler. Every gap found while doing this is closed in the language or in the library, and the improved language is then used to write the next part. The language thus improves recursively, and every decision is tested on real code before it is fixed.

Where Unio cannot be used yet, a temporary implementation in another language does the work and is removed as soon as Unio can replace it: the compiler and the runtime are in Rust until self-hosting, the checkers in `spec/` are in Python until the interpreter of stage 3 runs their Unio versions in `tools/`. A part written in another language needs a stated reason and a stated end.

This has already paid off: writing the standard library and the checkers in Unio exposed the explicit commit of atomic files, contextual keywords, `pass`, package modules, and missing library functions, all settled in version 0.4.

## 2. Syntax

### 2.1 General rules

- Blocks are defined by indentation of 4 spaces. Tabs are forbidden.
- A block header ends with a colon.
- Keywords are whole English words, no abbreviations: `function`, `returns`, `fails with`, `struct`, `interface`, `enum`, `match`, `case`. The words `error`, `external`, `module`, `test`, and `unsafe` are keywords only where a declaration starts; elsewhere they are ordinary names, so a field can be called `error`.
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
- Membership: `item in collection`, which calls `collection.contains(item)`.
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

`match` must be exhaustive: the compiler checks that all variants are listed or that there is a `case _`. A branch that deliberately does nothing says so with `pass`; an empty block is an error.

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

Built-in core types: `Int` (64-bit signed), `Float` (64-bit), `Bool`, `Text`, `Bytes`, `List[T]`, `Map[K, V]`, `Set[T]`, `Option[T]`, `Tuple`, `Path`, `Function`, and `Never`, the result type of a function that does not return (`assert_fail`).

Enum variants are written `Command.Add(...)`. The short form `Add(...)` is allowed when nothing else visible in the module has that name. A variant named like a type must therefore be written in full: `Value.Text("Anna")`, and `List` as a variant of `Command` cannot be written alone.

Equality:

- `struct` and `enum` compare by content when all fields implement `Equal`; the implementation is derived automatically and may be replaced by an explicit one.
- `Float` implements `Equal` and `Order` per IEEE 754: `nan == nan` is `false`, and comparisons with `nan` are `false`. `Float` does not implement `Hash`, so it cannot be a `Map` key, a `Set` element, or a `match` pattern. For sorting and keys there is `float.total_order()`; for checks, `float.is_nan()`.
- Resources (section 4) do not implement `Equal`: `==` on them is a compile error. Identity is checked with the function `is_same(a, b)`. This keeps a reader from confusing comparison of file contents with comparison of handles.
- `Decimal` (exact decimal arithmetic for money) is not a built-in type but a struct in `std.decimal` that implements `Add`, `Subtract`, `Multiply`, `Divide`, `Equal`, `Order`, and `Hash`. It is created explicitly: `Decimal("19.99")`; the core has no literal for it. This keeps the core vocabulary small while preserving the familiar notation `price * quantity`.

### 2.7 Mutability and values

- Structs, enums, collections, and text are values. Binding, passing, returning, and storing never share mutable state: a change made through one name is never seen through another.
- A value changes only through a place that starts with a `var` name or a `var` parameter, and a field changes only if it is declared `var`. Names bound by `let`, by patterns, and by `for` are immutable:

```unio
var tasks = load()
match tasks.find_index(function (task as Task) returns Bool: task.id == id):
    case some(index):
        tasks[index].done = true        # changes the list
    case none:
        fail NotFound(id = id)
```

  Writing `task.done = true` for a task bound by `case some(task)` is a compile error with a hint to change the element through the collection.
- Mutating methods (`append`, `remove`, `sort`) are available only on `var` places and are marked as such in documentation.
- A function modifies an argument only through a parameter declared `var name as List[Int]`; the caller passes a `var` variable, and one variable cannot go to two `var` parameters of the same call.
- Anonymous functions capture values when they are created and cannot assign to captured variables.
- Resources are not values: they have identity and are never copied (section 4).

Values are shared until changed; changing a shared value copies only the level that changes, and changing an unshared value happens in place. No copy can change the behavior of a program, and the cost of copies is measured. A reader who sees `var` in a signature knows that the function modifies the state passed to it, and a reader who sees `let` knows that the value cannot change at all.

State that several functions change together is modeled as a struct with `var self` methods: `reader.advance()` changes the reader, and every place that changes it is visible at the call. Passing the same state through `var` parameters works too but is longer.

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

   The rule is the same in every position: a statement, an initializer, an argument, the subject of `match`. A call to a function with `fails with` never turns into a `Result` by itself.

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
7. For functions where an error is needed as a value there is the type `Result[T, E]` with variants `ok(value)` and `error(value)`; `try` works with it too. A `Result` exists only where a function declares it as its result or the value is built explicitly. In the standard library `Result` is used only where errors must be stored or passed on. The conversion from `fails with` to `Result` is written explicitly:

```unio
function parse_safely(arguments as List[Text]) returns Result[Command, UsageError]:
    try:
        return ok(parse_command(arguments))
    on UsageError as e:
        return error(e)
```

8. The block of an inline handler `try ... on E as e:` does not complete normally: it ends with `return`, `fail`, `break`, `continue`, or a call that never returns (`assert_fail`). The value of the expression is therefore always the successful result.

### 3.3 Faults

A fault is a contract violation that cannot be handled inside the program: index out of range in `items[i]`, division by zero, `Int` overflow, a failed `assert`.

- Behavior is defined: the program prints a diagnostic with a stack trace to stderr, the runtime unwinds the stack and calls `release` for open `using` blocks in reverse order, and the program exits with code 101.
- Exhaustion of memory (heap or stack) is not a fault. The runtime cannot promise to run user code when there is no memory, so it writes a preallocated diagnostic and exits with code 102 without calling `release`. The same holds for anything outside the runtime's control: a kill signal, power loss, a crash in external code.
- What survives every kind of termination is stated separately. A file written by `files.write_text`, or through `files.open_atomic` and committed, is replaced atomically: a reader sees either the old or the new complete content, because the write goes to a temporary file that replaces the target by rename. The data reaches storage before the rename, so this holds after power loss too; `durable = true` adds that the new content survives power loss once the call has returned. The guarantees are promised for named file systems (ext4, XFS, Btrfs, APFS, NTFS); elsewhere a request for `durable` fails instead of pretending.
- A fault cannot be caught with `on`. It can be observed only in tests (`expect_fault`) and at the process boundary.
- Every faulting operation has a safe form returning `Option` or an error: `items.get(i)`, `a.checked_divide(b)`, `a.checked_add(b)`.

Domain errors and program defects are thus distinguished syntactically, and the reader sees exactly what can happen.

## 4. Memory and resources

### 4.1 Two models

- **Managed values.** Structs, collections, text, and closures live on the heap under the GC. The programmer does not think about freeing them.
- **Resources.** Files, sockets, processes, device handles, and in the compute extension tensors and GPU buffers. They have an explicit lifetime.

The boundary between the models: a type implements the interface `Resource` (method `release`), therefore it is a resource. Such a value:

- is created in a `using` block or as a field of another resource;
- is released on leaving the block, in reverse order of creation, including on error and on fault (but not on memory exhaustion or abnormal termination, see 3.3);
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

The first release contains the GC and resources with `using`. The GC belongs to the `alloc` and `std` layers; `core` does not depend on it, so that code without a GC remains possible (section 8.1). Memory budgets, lifetimes for tensors, and accounting for GPU computation completion belong to the compute extension and are designed on top of the same `Resource` interface: a tensor is a resource; a lifetime scope is an owning resource with a budget.

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
- Least privilege: the manifest declares what the package needs (`files` with read and write paths, `env` with variable names, `process` with program names, `network`). The compiler forbids using standard-library modules outside the declared permissions. This answers the reader's question "what can this program do" before it runs.
- Mandatory sandbox: the same permissions are enforced at run time by the operating system (Landlock and seccomp on Linux, Seatbelt on macOS, AppContainer on Windows). The operator can narrow them further with an external policy (`--policy` or `UNIO_POLICY`) and set limits on memory, time, and processes; the program cannot widen them. If the required mechanism is missing, the program does not start unless the operator explicitly allows a weaker sandbox. The compile-time check tells the reader what the program intends to do; the sandbox limits what it can do even if the check, the compiler, or the standard library is wrong. This matters most for running AI-generated code.
- The tools are isolated as well: `check`, `build`, the language server, and package operations run in worker processes that can read only the project and its locked dependencies, write only the build directory, and have no network (except the download worker, limited to the configured registries). The compiler never runs code of the project, so opening or building an untrusted project is no more dangerous than reading it.
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

### 8.1 System programming: library layers and build profiles

A web server and the services and applications of an operating system are ordinary programs: they need networking and concurrency (release 0.2), not a different language. A kernel is different: an ordinary program receives memory, files, threads, and isolation from the operating system, while a kernel has to create them itself. Unio supports both kinds of work as one language.

**Library layers.** The standard library and the runtime are split into three layers, and a module may use only its own layer and the layers before it:

- `core`: basic types and operations that need neither an allocator, nor a GC, nor an operating system;
- `alloc`: what needs an allocator (text that is built at run time, lists, maps, JSON); in 0.1 the allocator is the GC heap;
- `std`: what needs the operating system (files, environment, processes, standard output, the sandbox).

Module names do not contain the layer (`std.json` lives in `alloc`), so applications are not affected; the layer is a property of where a module lives and is checked.

**Build profiles.** A profile chooses which layers are available. The `application` profile has all three, the GC, and the sandbox of the operating system. The `system` profile always has `core`, has `alloc` only with an allocator that the program provides, and has neither a GC nor `std`. The syntax, the types, and error handling are the same in every profile; a profile only removes capabilities and never changes what a construct means. If a function needs a layer that the profile lacks, the compiler reports the chain of calls that leads to it. Release 0.1 implements the `application` profile and the check `unio check --profile=core`, which verifies that a package depends on nothing but `core`.

**What the system extension adds.** These capabilities are designed through RFCs for a later release and are available only in the `system` profile and in `unsafe module` files where noted:

1. Predictable memory: explicitly owned buffers, data placed in given regions, work without a GC, and code where copy-on-write is forbidden: the compiler reports every place where a copy could occur, because an unexpected copy of a large array is not acceptable there.
2. Control of representation: size and alignment of structs, hardware registers, page tables, calling conventions, and the placement of sections in the executable.
3. Controlled low-level access: pointers, memory-mapped devices, special processor instructions, and small amounts of assembly, only in `unsafe module` files.
4. Interrupts and synchronization: interrupt handlers, atomic operations, and a defined memory model that agrees with the concurrency of release 0.2. `volatile` access and synchronization between threads are different operations.
5. Checked constraints: a function can require that it does not allocate, block, or use the GC, and the compiler checks the whole chain of called functions, not only the body. This builds on the derived effects of section 6 (`allocates` is derived from 0.1 on) and is designed together with `uses`.

**Guarantees.** The sandbox of an application relies on the operating system. A kernel on real hardware manages privileges and isolation itself, so its low-level part belongs to the trusted computing base. The first experiments run in virtual machines.

**The first operating system in Unio** is a small kernel with services in separate processes, so that most components stay in safe Unio and the hardware complexity is concentrated in a small part. The first checkable prototype is the chain: an existing bootloader, entry into Unio code, output to the serial port, management of memory pages, a timer, a simple scheduler, a user process, and a system call. A small start layer in Rust or assembly is acceptable under the rule of section 1.1, with a stated reason and end. Whether Cranelift supports the needed calling conventions and binary format is checked by an early prototype. An operating system with drivers and Linux-level compatibility is a project of many years in which the language is a small part, and it is not on the roadmap of the language.

A GC does not make an operating system impossible: it can serve services and some subsystems. Early boot, interrupt handlers, and critical sections need the independent memory model of the `system` profile; a fully managed kernel would make its runtime a separate complex project.

## 9. Complete example

See `docs/examples/todo.unio`: a command-line utility with a task list, a storage file, argument parsing, and error handling. The example shows every construct of the core.

## 10. Settled questions

| Question | Decision |
|---|---|
| `+` for text | Only `Text + Text` is allowed; interpolation is the standard |
| How `Option`, `fails with`, and `Result` relate | `Option` is absence of a value; `fails with` is a function error; `Result` is an error as a stored value; `try` works with the latter two |
| What happens on `items[i]` out of range | Fault with exit code 101, uncatchable; safe form `get` |
| Calling a function with `fails with` without `try` | Compile error in any position, including the `match` subject; no implicit `Result` |
| Out of memory | Not a fault: exit code 102, no user code runs; files are protected only by atomic writes |
| Enforcement of permissions | Mandatory OS sandbox, narrowed by an external policy; fails closed |
| What the language and its tools are written in | In Unio wherever possible (section 1.1); another language only temporarily, with a reason and an end |
| Kernels and code without an operating system | One language: library layers `core`, `alloc`, `std` and build profiles that only remove capabilities; system features in a later release (section 8.1) |
| Boundary between GC and resources | A type implementing `Resource` is a resource and lives in `using` |
| Compiler error format | JSON with code, span, explanation, and patch; fixed in the requirements before the first release |
| Concurrency in the core | None. The core has no shared mutable state between threads, so a concurrency model can be added later without breakage |
| `Decimal` in the core | No. `std.decimal` as a struct with the standard operator interfaces; explicit construction `Decimal("19.99")` |
| Operators for user-defined types | Only through explicit `extend T with Add` and the other standard interfaces; redefining existing implementations is forbidden |
| Effects in signatures | Not in 0.1: package-level permissions; function effects are derived by the compiler and shown in `unio symbol`. `uses` is reserved for 0.3 |
| `==` for `Float` | IEEE 754; `Float` without `Hash`, not a key and not a `match` pattern; `total_order()` and `is_nan()` |
| `==` for resources | Compile error; identity via `is_same(a, b)` |
| Changing a value found in a collection | Values, not shared references: change through the collection, `tasks[index].done = true`; names from patterns and loops are immutable |
| Variant with the name of a type | Short form only when the name is unambiguous; otherwise `Enum.Variant` |
| Words such as `error` and `test` as names | Contextual keywords: special only where a declaration starts |
| A branch that does nothing | `pass`; an empty block stays an error |
| Compiler implementation language | Rust with code generation through Cranelift in 0.1; self-hosted implementation in Unio by 1.0, once the language is stable |

## 11. Rationale for the latest decisions

The decisions on questions left open in version 0.1 of the concept follow from the principle "clear code, explicit behavior, controlled resources". Items 6–14 were added in version 0.4 after review and after writing the standard library in Unio.

1. **`Decimal` outside the core.** A small vocabulary of constructs matters more than convenience for one domain. Familiar arithmetic notation is preserved because operators are bound to interfaces, not to built-in types. Explicit `Decimal("19.99")` rules out hidden rounding when mixing with `Float`.
2. **Effects without syntax.** Package permissions already answer "what can the program do". Function effects are useful to the reader and the AI, but they can be computed rather than required in writing; signatures stay short and no information is lost. Once an effect influences execution (GPU), it becomes part of the signature through `uses`.
3. **`Float` per IEEE 754.** Deviating from the standard would make numeric code surprising to everyone who knows other languages. The dangerous spots are closed by forbidding `Hash` and `match` patterns, not by changing the operator's semantics.
4. **Resources without `==`.** Comparing handles has no obvious meaning for a reader: contents or identity? A compile error with the hint `is_same` makes the choice explicit.
5. **Rust and Cranelift.** Unio's safe subset rests on a safe compiler; Rust provides that without manual memory checking. Cranelift gives fast, deterministic builds, which matters more than peak optimization for CLI utilities and for the "generate, check" loop. LLVM may become a second backend for `--release` if the 0.2 benchmarks require it. Self-hosting moves to 1.0: writing a compiler in a language that is still changing means writing it twice.
6. **Sandbox as a requirement, not an option.** Permissions in the manifest are checked by the compiler, but a compiler check protects only against what the compiler understands. Code written by AI is run often and reviewed briefly, so the consequences of a mistake must be limited by the operating system. Failing closed keeps a missing kernel feature from silently turning the sandbox off.
7. **Honest guarantees on termination.** Running `release` needs memory and a working runtime, which are exactly what is missing after memory exhaustion or a kill signal. Promising more would be untestable; atomic replacement of files gives a guarantee that does not depend on how the program ends.
8. **Values instead of shared references.** If a name from `case some(task)` could change an element of the list, the reader would have to track every alias to know what a line changes. With values, a change is visible only through the place where it is written, which is the property a reviewer of generated code needs most. Sharing until change keeps the cost of this low.
9. **Qualified variants when names collide.** `return List` reads as a type, not as a command. The short form is kept where it is unambiguous because it makes `match` readable; the compiler adds the enum name where it is not.
10. **Contextual keywords.** A reader never sees `error` or `test` in a position where both meanings fit: as keywords they start a top-level declaration, where no expression can stand. Making them contextual keeps natural names for fields and variables, which matters more than a uniform rule for the lexer.
11. **`pass` instead of empty blocks.** An empty block looks like unfinished code; `pass` states that doing nothing is intended. The word is familiar from Python and adds no new idea.
12. **Shared state through `var self` methods.** Values make every change visible where it is written. The cost is that changed state must be named at every call; a struct with `var self` methods keeps that explicit without repeating parameters.
13. **The language built in the language.** A design checked only on examples misses what a large program needs; a library and tools written in Unio are such programs, and they are needed anyway. Writing them first in Unio turns every awkward spot into a concrete finding, while the cost of a temporary second implementation is limited to the time until the interpreter runs.
14. **Layers and profiles now, system features later.** Splitting the library and the runtime is cheap while they are small and expensive once applications depend on them; checking the layers from the start keeps `core` free of hidden dependencies on the allocator, the GC, and the operating system. The system features themselves need careful design and do not help the first release, so they wait for their own release, while these two boundaries keep the way open, also for embedded devices and high-performance computing.
