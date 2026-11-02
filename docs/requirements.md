# Requirements specification: Unio release 0.1 "Core"

Specification version: 0.9 (draft). Basis: `README.md`, `docs/concept.md`. Work plan: `docs/plan.md`. Russian localization: `docs/ru/requirements.md`.

Changes from 0.8: content hashes of symbols (C-11a) and the diagnostic message standard (R-12), from `docs/influences.md`.

Changes from 0.7: interface values with dispatch at run time and default methods (L-6b, L-6c); a guide chapter on classes (D-2).

Changes from 0.6: visibility of fields and methods and functions of a type (L-23a to L-23d).

Changes from 0.5: test-first development (T-5a, T-5b).

Changes from 0.4: library layers `core`, `alloc`, `std` (S-11, S-12), build profiles and `unio check --profile=core` (L-26, C-21), the derived effect `allocates` (C-11); system programming is placed out of scope with its own release (section 2.2).

Changes from 0.3: atomic replacement, ordering, and durability of file writes are separate guarantees for named file systems (S-9, S-9a, S-9b), and `commit` records each step so that a failure after the rename is reported precisely (S-9c); the tools and package operations run in isolated workers (SB-10–SB-15).

Changes from 0.2: a mandatory sandbox enforced by the operating system and an external policy (section 4.6); a single rule for calling a function with `fails with` (L-14, L-16); a termination model with separate guarantees for faults, memory exhaustion, and abnormal termination, and atomic file writes (sections 4.5, S-9, R-2).

The words "must" and "shall" denote mandatory requirements; "should" denotes desirable ones; "may" denotes permitted ones.

## 1. General

### 1.1 Name

Unio: a programming language and tool kit. The subject of this specification is the first release, version 0.1, code name "Core".

### 1.2 Purpose of the release

Release 0.1 is intended for writing command-line utilities and automation scripts: file and text processing, running external programs, working with JSON and environment variables. The release is the foundation for the later server, compute, and quantum extensions and must not require rework for them.

### 1.3 Users

- A developer writing utilities by hand.
- An AI agent generating and fixing code from a person's task statement.
- A reviewer checking generated code before it runs.

### 1.4 Success criteria for the release

1. The utility in `docs/examples/todo.unio` compiles, passes its tests, and runs on four platforms without changes.
2. Ten reference utilities (section 9) are written in Unio, and an external reader with no knowledge of the language correctly describes their behavior from source in a controlled trial.
3. An AI agent given only the JSON diagnostics fixes at least 80% of the errors in the control set without human intervention.
4. All requirements of section 7 are confirmed by measurements and published.

## 2. Scope

### 2.1 In scope

- Language: the constructs of `docs/concept.md`, sections 2–5.
- Compiler with type and error checking; native builds for Linux x86-64, Linux arm64, macOS arm64, Windows x86-64.
- Runtime: GC, resources, faults, termination model, `main` startup.
- Library and runtime layers `core`, `alloc`, `std` and the check of a package against the `core` layer (S-11, S-12, C-21).
- Sandbox: enforcement of manifest permissions and of an external policy by operating-system mechanisms (section 4.6).
- Standard library for CLI (section 5).
- Tools: `unio build`, `run`, `check`, `fmt`, `test`, `add`, `remove`, `update`, `doc`, `symbol`, `explain`.
- Package manager with manifest, lock file, and registry.
- Language server (LSP) and a VS Code extension.
- Documentation: language reference, guide, standard library reference.

### 2.2 Out of scope

- Concurrency and asynchrony.
- Network clients and servers, other than package download.
- Compute extension: tensors, autodiff, GPU.
- Quantum module.
- FFI for user code (`external function` is used only inside the standard library).
- Self-hosted compiler written in Unio.
- Profiler (ships in 0.2; 0.1 only defines the event format).
- System programming: the `system` profile, memory without a GC, control of data representation, pointers and assembly, interrupts, atomic operations and a memory model, constraints on effects (concept, section 8.1; a later release).
- UnioS, the operating system written in Unio, and its proof of concept (`docs/os.md`; a separate track with release 0.5).

## 3. Language requirements

### 3.1 Lexical structure

- L-1. A file is UTF-8 with the `.unio` extension; blocks by 4-space indentation; tabs and mixed indentation are errors.
- L-2. The keyword set is fixed and published in the grammar (`spec/grammar.ebnf`, section L); adding a keyword after 0.1 requires an RFC. The words `error`, `external`, `module`, `test`, and `unsafe` are contextual keywords: they have their meaning only at the start of a top-level item or a declaration and are ordinary names everywhere else.
- L-2a. `pass` is the statement that does nothing; it is the only way to write a block with no effect, because an empty block is a syntax error.
- L-3. Comments `#`, documentation comments `##`. A documentation comment attaches to the next declaration and is available to `unio doc` and `unio symbol`.

### 3.2 Types

- L-4. Built-in types: `Int`, `Float`, `Bool`, `Text`, `Bytes`, `List[T]`, `Map[K, V]`, `Set[T]`, `Option[T]`, `Result[T, E]`, `Tuple`, `Path`, `Function`, `Never`. `Never` is the result type of a function that does not return; inside an interface `Self` stands for the implementing type.
- L-5. No implicit conversions between types. Conversion is an explicit call (`to_text`, `to_int`, `to_float`).
- L-6. User-defined types: `struct`, `enum`, `interface`, `error`; interface implementation via `extend ... with ...`.
- L-6a. An enum variant is written `Enum.Variant`. The short form `Variant` is allowed when no type, function, constant, or variant of another enum with that name is visible in the module; otherwise the short form is a compile error with a fix that adds the enum name. `some`, `none`, `ok`, and `error` are always visible and cannot be declared.
- L-6b. An interface used as a type, for example `List[Shape]` or `shape as Shape`, holds a value of any type that implements it, and a method call on it is dispatched at run time to that type's implementation; `on Error as e` is such a value. An interface can be used this way only if its methods use `Self` nowhere but in the receiver and have no generic parameters, because the concrete type is unknown; otherwise the compiler names the method that prevents it. An interface value has no `==` and does not reveal its concrete type: a closed set of types is an `enum`. A resource cannot be held as an interface value. A generic function `[T with Shape]` stays resolved at compile time.
- L-6c. A method of an interface with a block has a default implementation. The block may reach the value only through the methods of the interface. A type that implements the interface may replace the default in its `extend ... with` block with the same signature; `unio symbol` shows which implementation a type uses. Defaults are the way to share code between types without inheritance of implementation.
- L-7. Generic functions and types with parameters and interface constraints.
- L-8. Type inference only for local `let`/`var`. Parameters, results, fields, and exported constants must be annotated.
- L-9. Equality of structs and enums is by content; the `Equal` implementation is derived automatically. `Float` compares per IEEE 754 and does not implement `Hash`: it cannot be a `Map` key, a `Set` element, or a `match` pattern. Resources do not implement `Equal`; `==` on them is a compile error with the hint `is_same`.
- L-9a. Operators are bound to standard interfaces (`Add`, `Subtract`, `Multiply`, `Divide`, `Equal`, `Order`, `Hash`). A user-defined type implements them only explicitly through `extend`; redefining existing implementations is forbidden. `item in collection` calls `collection.contains(item)`.
- L-9b. Function effects are not declared in signatures. The compiler derives them from the call graph and shows them in `unio symbol` and `unio doc` (C-11). The word `uses` is reserved.

### 3.3 Mutability

- L-10. `let` is immutable, `var` is mutable. Reassigning a `let` is a compile error.
- L-11. A value changes only through a place rooted in a `var` binding or a `var` parameter, and a struct field changes only if it is declared `var`: `tasks[index].done = true`. Names bound by `let`, by patterns (`case some(task)`), and by `for` are immutable. Mutating collection methods are available only on such places. A function modifies an argument only through a parameter declared `var`; the caller passes a `var` variable, and the same variable cannot be passed to two `var` parameters of one call.
- L-11a. Structs, enums, collections, and text are values: binding, passing, returning, and storing never share mutable state, so a change through one name is never seen through another. Anonymous functions capture values when they are created and cannot assign to captured variables. Resources are not values: they have identity (L-9) and are never copied.
- L-11b. Values are shared until changed. Changing a shared value copies only the changed level, nested values stay shared; changing an unshared value happens in place, and appending to an unshared list takes amortized constant time. The cost of copies is measured in the performance suite (section 7.1).

### 3.4 Errors and missing values

- L-12. `Option[T]` with `some`/`none`, operators `or` and `?.`.
- L-13. `error` declares an error type. `fails with` lists a function's errors and is part of its type.
- L-14. Calling a function with `fails with` without `try` is a compile error in every position: a statement, an initializer, an argument, a `match` subject. Prefix `try` propagates; `try:` with `on` handles; `try ... on E as e:` handles one error inline. Such a call never yields a `Result` implicitly.
- L-14a. The block of an inline handler `try ... on E as e:` must not complete normally: it ends with `return`, `fail`, `break`, `continue`, or a call of a function that never returns (`assert_fail`). The value of the expression is therefore always the successful result.
- L-15. The compiler checks completeness of handling: every error is either handled or listed in the current function's `fails with`. `on Error` handles any error.
- L-16. `Result[T, E]` is an ordinary `enum` with variants `ok(value)` and `error(value)`. A `Result` value exists only where a function declares `returns Result[T, E]` or the value is constructed explicitly. `match` inspects it; `try` applied to it propagates or handles `E` exactly as for a call with `fails with E`. There is no implicit conversion in either direction.
- L-17. Faults: index out of range, division by zero, `Int` overflow, failed `assert`, `assert_fail`. Behavior is controlled termination (section 4.5): diagnostic with a stack trace to stderr, release of resources in `using`, exit code 101. A fault cannot be handled with `on`.
- L-17a. Exhaustion of memory (heap or stack) is not a fault: it causes emergency termination with exit code 102 (section 4.5), during which no user code runs, `release` included.
- L-18. Every faulting operation in the standard library has a safe form returning `Option` or an error.

### 3.5 Memory and resources

- L-19. Managed values live under the GC. A program cannot obtain the address of a value.
- L-20. The `Resource` interface with the method `release`. A resource value is created in `using` or as a field of another resource; storing a resource in an ordinary struct that outlives the block is a compile error. The name bound by `using` is a place: `var self` methods of the resource can be called and its `var` fields changed through it, but the name cannot be reassigned.
- L-21. Release in `using` happens on normal exit, on error, and on fault, in reverse order of creation. On emergency and abnormal termination (section 4.5) release is not performed; guarantees for files in those cases come only from atomic writes (S-9).

### 3.6 Modules and visibility

- L-22. A module is a file; a package is a directory with `unio.toml`. Import: `import a.b`, `import a.b as c`, `from a.b import x, y`. A name that starts with `std.` refers to the standard library; any other name refers to a module of the same package, `a.b` to the file `a/b.unio` relative to the package root.
- L-23. Everything is private by default; `public` exports. Private means visible only in the same module (file).
- L-23a. A struct field is private unless it is marked `public` (`public var balance as Int`). A struct with at least one private field can be constructed and changed through that field only in its module; other modules use its public functions.
- L-23b. The fields of an `error` and of `enum` variants are visible wherever their type is visible: they are data that callers must handle in `on` and `match`. `public` on them is redundant and the formatter removes it.
- L-23c. A method in `extend T:` is private unless it is marked `public`. A method in `extend T with I:` implements the interface `I` and is visible wherever `T` and `I` both are; `public` is not written there.
- L-23d. A function in `extend T:` without `self` is a function of the type and is called as `T.name(...)`, for example `Account.open("Anna")`. It follows the visibility rule of L-23c.
- L-24. Cyclic imports are a compile error that names the cycle.

### 3.7 Tests in the language

- L-25. A top-level `test "description":` block. `assert`, `assert_fail`, and `expect_fault` are available. Tests are excluded from `build` output.
- L-26. A build profile chooses the available library layers (S-11): `application` has `core`, `alloc`, and `std`; the `system` profile is defined by a later release. A profile only removes capabilities: a construct means the same in every profile, and a program that uses an unavailable layer is rejected with the chain of calls that leads to it.

## 4. Compiler and runtime requirements

### 4.1 Compiler

- C-1. Stages: lexer, parser, name resolution, type and error checking, resource checking, code generation. Each stage has its own test set.
- C-2. The compiler recovers from errors and reports all errors in a file in one pass, not only the first.
- C-3. `unio check` performs all checks without code generation.
- C-4. Builds are deterministic: identical sources and lock file give a byte-identical artifact on one platform.
- C-5. The result of `unio build` is a single executable with no external dependencies beyond system libraries.

### 4.2 Diagnostics

- C-6. Every diagnostic has a unique code of the form `E0001` (error), `W0001` (warning), `H0001` (style hint).
- C-7. Text format: code, path, line and column, snippet with underline, message, hint.
- C-8. `--diagnostics=json` format: an array of objects, each with fields `code`, `severity`, `message`, `explanation`, `spans` (file, start, end, label), `fixes` (array of replacements: file, range, new text, description), `related` (symbols and locations). The schema is published as JSON Schema and versioned.
- C-9. `unio explain E0001` prints an extended explanation with an example of the error and its fix.
- C-10. Messages in English and Russian; the language is selected by a flag and an environment variable. Codes and the JSON schema do not depend on the language.

### 4.3 Symbol context

- C-11. `unio symbol <full name>` returns (text and JSON): signature, documentation, declaration site, usage list, and for functions the error set, derived effects (`files`, `network`, `process`, `env`, `allocates`), the lowest library layer the function needs, and the list of called functions.
- C-11a. `unio symbol --json` reports for every function and type a content hash: the hash of the normalized declaration and body together with the content hashes of everything it references, independent of formatting and comments. The incremental build and test cache is keyed by this hash, and `unio test --json` reports it for every test. Source code and the language do not use hashes.
- C-12. `unio outline <file>` returns the file's declarations with signatures.

### 4.4 Runtime

- C-13. The GC is precise, tracing, non-moving. Pauses and throughput are measured in the performance suite.
- C-14. The stack trace on a fault contains function names, files, and lines without external debug formats.
- C-15. Exit codes: 0 success; otherwise the value of `main`, which lies in 0–100; 101 fault; 102 memory exhausted (heap or stack); 103 the sandbox or the policy could not be applied (section 4.6). Codes 104–127 are reserved for the runtime. A `main` value outside 0–100 is a fault.

### 4.5 Termination model

- C-16. Normal termination: `main` returns. Buffered output is written, resources are already released by their `using` blocks.
- C-17. Controlled termination (fault, code 101): user code stops at the fault point; the runtime prints the diagnostic and the stack trace, unwinds the stack, and calls `release` for every open `using` in reverse order. `release` runs with a memory reserve set aside at startup. A fault or error inside `release` during termination is reported, and the remaining resources are still released; the exit code stays 101.
- C-18. Emergency termination (code 102): the heap is exhausted beyond the reserve, the heap limit of the policy is exceeded, or the stack overflows. The runtime writes a preallocated diagnostic to stderr and exits without running user code. Descriptors are closed by the operating system; data buffered inside the program is lost.
- C-19. Abnormal termination: a kill signal, power loss, a crash of external code, an operating-system failure. The runtime gives no guarantees; only the guarantees of the operating system and of atomic writes (S-9) remain. For files, guarantees 2 and 3 of S-9 still apply on the file systems of S-9a.
- C-20. The documentation lists for each kind of termination which guarantees hold for resources, standard output, and files. Writes through an open file handle without `open_atomic` are not called safe: after emergency or abnormal termination their content may be cut off at any point.
- C-21. `unio check --profile=core` verifies that a package needs nothing but the `core` layer: no allocation, no GC, no operating system. Each violation is reported with the chain of calls from a function of the package to the first function of a higher layer.

### 4.6 Sandbox

- SB-1. Every program started through `unio run`, `unio test`, or as a built executable runs in a sandbox. The runtime applies the restrictions by operating-system mechanisms before `main`; the program cannot lift them later, and child processes inherit them.
- SB-2. Effective permissions are the intersection of the manifest permissions (T-6) and the external policy (SB-3). Without declared permissions a program can only read its arguments and standard input and write to standard output and standard error.
- SB-3. The external policy is a file given by the operator with `--policy <file>` or the environment variable `UNIO_POLICY`. It can only narrow the manifest permissions and sets limits: heap size, wall time, CPU time, number of processes, size of written files. Extra permissions in a policy have no effect and produce a warning. A policy that cannot be read or parsed stops the start with code 103.
- SB-4. Mechanisms: Linux — Landlock for the file system, seccomp-bpf for system calls, `no_new_privs`, resource limits; macOS — a Seatbelt profile and resource limits; Windows — AppContainer and a Job Object. The choice is confirmed by prototypes in stage 4 of the plan; the requirement is the outcome, verified by SB-8.
- SB-5. The sandbox fails closed: if the operating system lacks a required mechanism, the program does not start (code 103). Only the operator can allow a weaker sandbox, with `--allow-weaker-sandbox` or `allow_weaker = true` in the policy; the runtime then prints the restrictions that are missing. The program itself cannot do this.
- SB-6. An operation blocked by the sandbox ends with the error that the standard library function already declares (for example, `IoError` of kind `PermissionDenied` naming the denied permission), not with a fault. Safe code reaches the operating system only through the standard library.
- SB-7. The compile-time check stays: using a standard-library module outside the declared permissions is a compile error, and derived effects are shown in `unio symbol`. The sandbox is the second line of defence, not a replacement.
- SB-8. A test suite of programs that attempt forbidden actions through every standard-library module and through child processes runs on all four platforms; every attempt must be denied.
- SB-9. The documentation states the limits of the sandbox: it does not protect against kernel vulnerabilities and side channels, nor against misuse of granted permissions. For AI-generated code it recommends a minimal manifest, an external policy with limits, and a separate user, container, or virtual machine for untrusted code.
- SB-10. The tools handle untrusted input too. `unio check`, `build`, `fmt`, `doc`, the compile step of `test`, the language server, and the package operations (`add`, `remove`, `update`, installing from `unio.lock`) run their work in isolated worker processes, with the mechanisms of SB-4 and failing closed as in SB-5. The tool itself only starts workers and shows their results.
- SB-11. A compiler worker may read only the package, its locked dependencies in the package cache, and the standard library, and may write only to the build directory (`target/`) and its own temporary directory. It has no network, cannot start processes, and sees no environment variables except those that select the language of messages. Imports cannot leave the package root. Limits on memory and time come from the policy or from defaults (2 GiB and 10 minutes per build); a worker that exceeds them is stopped and the tool reports a diagnostic instead of hanging.
- SB-12. Package operations are split between two workers. The download worker may connect only to the registries in the configuration and write only to the download area. The unpacking worker has no network and writes only the cache entry of the package being installed; it checks signatures and hashes before unpacking and rejects archive entries with absolute paths, `..`, links, or device files.
- SB-13. The language server analyses every opened project in such a worker, so opening an untrusted project in an editor is no more dangerous than reading its files.
- SB-14. The compiler never executes code of the package or of its dependencies (R-7): Unio 0.1 has no build scripts, macros, or compile-time evaluation of user code. A later feature of this kind runs inside the sandbox of SB-1 and needs an RFC.
- SB-15. The SB-8 suite includes hostile projects: archives with path traversal and links, deeply nested and oversized sources, imports that try to reach files outside the package, and inputs that make the checker work for a long time. Every case must be denied or stopped by a limit on every platform.

## 5. Standard library requirements

- S-1. Modules: `std.text`, `std.collections`, `std.math`, `std.decimal`, `std.files`, `std.path`, `std.env`, `std.args`, `std.process`, `std.json`, `std.time`, `std.random`, `std.log`, `std.test`.
- S-1a. `std.decimal.Decimal` provides exact decimal arithmetic, implements all operator interfaces and `Hash`; constructed from text and `Int`; conversion from `Float` is explicit only, with a stated rounding mode.
- S-2. Every function that can fail declares concrete error types; `fails with Error` is forbidden in the standard library.
- S-3. Every module is documented; every public function has an example.
- S-4. `std.args`: positional arguments, flags, defaults, automatic help.
- S-5. `std.json`: parse to a value, encode and decode structs from their declaration, without annotations.
- S-6. `std.process`: run a program with arguments, capture output, exit code, timeout.
- S-7. Unsafe code (`external function`) is allowed only in the standard library and passes a separate reliability review (section 7.3).
- S-8. Every standard-library operation blocked by the sandbox reports it through its declared error type (SB-6). A function that reports absence instead of an error (`files.exists`, `env.get`) reports a blocked object as absent.
- S-9. Writing in `std.files` replaces the target through a temporary file in the same directory. Three guarantees are stated separately:
  1. Atomic replacement: a reader sees either the old or the new complete content, never a partly written file, however the program ends (normally, by fault, by memory exhaustion, or by a kill signal).
  2. Ordering: the data of the temporary file always reaches storage before the rename, so after power loss or an operating-system crash the target still holds either the old or the new complete content; which of the two is not specified.
  3. Durability: with `durable = true` the directory is synced after the rename as well, and once `write_text` or `commit` has returned successfully, the new content survives power loss.

  Guarantees 1–3 hold on the file systems of S-9a. `write_text` and `write_bytes` perform the whole sequence in one call. `open_atomic(path)` returns a resource whose content replaces the target only when `commit()` is called. Releasing it without a commit, whether on error, on fault, or because the block ended without one, removes the temporary file and leaves the target unchanged. The commit is explicit because `release` cannot know how its block ended. After emergency or abnormal termination a temporary file named `.<name>.unio-tmp-<id>` may remain; it never replaces the target.
- S-9a. Supported file systems and mechanisms: Linux ext4, XFS, and Btrfs (`fsync` of the file before `rename`, `fsync` of the directory for durability); macOS APFS (`fcntl(F_FULLFSYNC)`, because `fsync` there does not reach storage); Windows NTFS (`FlushFileBuffers`, then `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` and `MOVEFILE_WRITE_THROUGH`). On other file systems (FAT, exFAT, network file systems) the write is performed, but guarantees 1–3 are not promised; a write with `durable = true` there fails with kind `Unsupported`, because the requested guarantee cannot be given. Storage that acknowledges writes it has not stored is beyond what software can guarantee, and the documentation says so.
- S-9c. `commit` is a sequence of steps, and each step records its result in the state of the file before the next one starts: `Writing` (descriptor open), `Closed` (data synced, descriptor released), `Replaced` (the target holds the new content), `Durable` (the directory is synced). `close` releases the descriptor even when it reports an error, so the state becomes `Closed` before `close` is called. A failed `commit` can be called again and continues from the recorded state. If the directory sync fails after the rename, `commit` fails with kind `DurabilityUnconfirmed`: the target is already replaced and guarantees 1 and 2 hold, but survival after power loss is not confirmed; `is_replaced()` reports this. `release` acts only on what the file still owns: in `Writing` it closes the descriptor once and removes the temporary file, in `Closed` it only removes the temporary file, after the replacement it does nothing.
- S-9b. Verification: on Linux, block writes are recorded during atomic writes (dm-log-writes) and every prefix of them is replayed and checked; on all four platforms, processes are killed at random points and virtual machines are powered off during writes. The runtime has a fault-injection hook in test builds that fails a chosen system call; the suite fails each step of `commit` in turn (data sync, close, rename, directory sync) on every platform and checks the state, the error kind, the content of the target, the presence of the temporary file, and, through the descriptor table, that no descriptor is closed twice. The same protocol is checked without the operating system by the `test` blocks of `stdlib/std/files.unio`, which replace the system steps with failing ones.
- S-10. The standard library is written in Unio in `stdlib/`. Everything the language can express is implemented in it; only operating-system and runtime primitives are `external function` declarations in `unsafe module` files. These sources are the normative API; the reference programs use only it, verified by `spec/check_api.py`.
- S-11. The standard library is divided into layers: `core` needs neither an allocator, nor a GC, nor an operating system; `alloc` needs an allocator, in 0.1 the GC heap; `std` needs the operating system. A module may use only its own layer and the layers before it. The layer is given by the directory (`stdlib/core/`, `stdlib/alloc/`, `stdlib/std/`) and is not part of the module name. Checked by `spec/check_api.py` in stage 0 and by the compiler from stage 2.
- S-12. The runtime is divided the same way: the `core` runtime (program entry, fault reporting through a hook, checked arithmetic) makes no calls to the operating system; the allocator and the GC form the `alloc` runtime; files, processes, standard output, and the sandbox form the `std` runtime. The `core` runtime reaches the others only through hooks that the `application` profile provides.

## 6. Tooling requirements

### 6.1 Commands

| Command | Purpose | Required options |
|---|---|---|
| `unio new <name>` | Create a package with manifest and example | `--lib`, `--bin` |
| `unio build` | Build an executable | `--release`, `--target` |
| `unio run` | Build and run | arguments after `--`, `--policy` |
| `unio check` | Check without building | `--diagnostics=json`, `--profile` |
| `unio fmt` | Format | `--check` for CI |
| `unio test` | Run tests | `--json`, name filter, `--policy` |
| `unio add/remove/update` | Manage dependencies | version, `--unsafe-ok` |
| `unio doc` | Build package documentation | `--open` |
| `unio symbol`, `unio outline`, `unio explain` | Context for people and AI | `--json` |

### 6.2 Formatter

- T-1. One style, no settings. Formatting is idempotent.
- T-2. The formatter preserves comments and does not change semantics; verified by an "AST before = AST after" test.
- T-3. Formatting a 1,000-line file takes at most 50 ms.

### 6.3 Testing

- T-4. Tests from `test` blocks run in isolation; results in text and JSON with name, status, time, and output.
- T-5. Tests can check faults with `expect_fault`.
- T-5a. A `test` block that does not compile is reported by `unio test` as a failing test with its diagnostics; the other tests of the file and the package still run. An error outside `test` blocks still stops the run, because the tested code itself is wrong.
- T-5b. Every change to the compiler, the runtime, the standard library, and the tools comes with a check that fails before the change and passes after it (concept, section 1.2); CI rejects a change to the grammar without new corpus files and a change to diagnostics without a program in the control set.

### 6.4 Packages

- T-6. Manifest `unio.toml`: name, semantic version, dependencies, permissions, list of `unsafe module`. Permissions are scoped: `files.read` and `files.write` are lists of paths with the placeholders `$HOME`, `$CWD`, `$TEMP` and the suffix `/**` for a subtree; `env` is a list of variable names; `process` is a list of programs; `network` is `false` in 0.1. Example:

```toml
[permissions]
files.read = ["$HOME/.todo.json"]
files.write = ["$HOME/.todo.json"]
env = ["TODO_FILE", "HOME"]
```
- T-7. `unio.lock` contains versions and content hashes of all dependencies. A build with a lock file does not touch the network.
- T-8. Registry: signed publishing, signature verification on install, no deletion of published versions.
- T-9. Installing a dependency with `unsafe module` or with permissions wider than the package's own requires explicit confirmation.

### 6.5 Editor

- T-10. Language server over LSP: diagnostics, go to definition, type hints, completion, formatting, rename.
- T-11. VS Code extension with highlighting and test running.

## 7. Non-functional requirements

### 7.1 Performance

Measured with the reference utilities (section 9) on Linux x86-64; results are published with each release.

| Metric | Requirement |
|---|---|
| `unio check` on a 10,000-line package | at most 1 s |
| Full `--release` build of a 10,000-line package | at most 30 s |
| Startup time of an empty program | at most 5 ms |
| Text and JSON processing | no more than 0.5x slower than CPython 3.13; target 3x faster |
| GC pause at a 100 MB heap | at most 10 ms (95th percentile) |

Universal C-level speed is not promised and is not measured as a requirement.

### 7.2 Reliability

- R-1. The compiler does not crash on any input; verified by fuzzing for at least 24 hours before release.
- R-2. On memory exhaustion the runtime exits with code 102 and a preallocated diagnostic. Files written with the atomic operations of S-9 keep either the old or the new complete content; verified by tests that exhaust memory and kill the process during writing on every platform. No other guarantee about files is given. Power loss is covered by S-9 and S-9b.
- R-3. A fault inside `using` releases all resources; verified by tests on every platform.

### 7.3 Security

- R-4. User code cannot bypass bounds, type, or resource checks.
- R-5. All `external function` code in the standard library is reviewed by two people and tested with memory sanitizers.
- R-6. The release build is reproducible, artifacts are signed, an SBOM is published.
- R-7. The compiler and package manager do not execute dependency code during a build.
- R-7a. The sandbox (section 4.6) passes the SB-8 and SB-15 suites on every platform; a release with a failing test of either suite is not published.

### 7.4 Compatibility and platforms

- R-8. Platforms: Linux x86-64, Linux arm64, macOS arm64, Windows x86-64. All tests pass on all platforms in CI.
- R-9. Within 0.x, incompatible language changes are possible; each ships with an automatic migration in `unio fmt --migrate` or a diagnostic with a fix.

### 7.5 Fitness for AI

- R-10. A control set of 200 programs with injected errors (types, errors, resources, style). Criterion: an agent receiving only the JSON diagnostics fixes at least 80% within at most three iterations.
- R-11. The average diagnostic message is at most 200 characters; the explanation is separate.
- R-12. Every error message follows one template: what is wrong, where, and how to fix it, with the fix as a patch in `fixes` where one exists. A message does not name compiler internals (pass names, internal types, node kinds). A new diagnostic code is merged only after review against the template and a run on the R-10 corpus; the review checklist is part of the repository.

## 8. Documentation requirements

- D-1. Language reference: grammar in EBNF, semantics of every construct, list of diagnostic codes.
- D-2. Guide: from installation to publishing a package, using `todo` as the example. The guide has a chapter on moving from classes to Unio: structs and `extend` instead of classes, interfaces with defaults and interface values instead of base classes, composition instead of inheritance, identifiers and maps instead of shared object graphs.
- D-3. Standard library reference generated from documentation comments.
- D-4. The document "How Unio works with AI": JSON formats, recommended loop, prompt examples.
- D-5. Documentation in English (primary) and Russian (localization in `docs/ru`).

## 9. Acceptance

### 9.1 Reference utilities

Ten programs covering the core and the standard library; stored in the repository and serving as tests, benchmarks, and material for the reading trial:

1. `todo`: task list (the example from the concept).
2. `wc`: count lines, words, bytes.
3. `grep-lite`: pattern search in files.
4. `json-query`: selection from JSON by path.
5. `rename`: bulk file renaming.
6. `env-check`: check environment and program versions.
7. `csv-sum`: aggregate CSV columns.
8. `backup`: copy a directory with filters.
9. `log-tail`: read and filter logs.
10. `release-notes`: generate release notes from git history via `std.process`.

### 9.2 Acceptance procedure

1. All compiler, runtime, library, and tooling tests pass on four platforms.
2. The reference utilities build, their tests pass, performance results meet 7.1.
3. The trials of 1.4 (readability, AI error fixing) are conducted with a published methodology.
4. Fuzzing per R-1 is complete and found defects are fixed.
5. Documentation per section 8 is published.

## 10. Constraints and assumptions

- The implementation language of the 0.1 compiler is Rust; code generation through Cranelift. Moving to LLVM or a self-hosted implementation is considered after 0.1.
- Everything that does not have to exist before the language runs is written in Unio: the standard library (S-10), the reference utilities, and the project tools in `tools/`. The Python checkers in `spec/` and `.github/scripts/` are temporary support for CI and are removed once the stage 3 interpreter runs their Unio versions.
- Language decisions are made through RFCs in `docs/rfcs/`; the specification is updated when an RFC is accepted.
- The questions left open in the first version of the concept are settled in `docs/concept.md`, sections 10–11. Revisiting them is possible only through an RFC.
