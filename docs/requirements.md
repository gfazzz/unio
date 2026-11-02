# Requirements specification: Unio release 0.1 "Core"

Specification version: 0.2 (draft). Basis: `README.md`, `docs/concept.md`. Work plan: `docs/plan.md`. Russian localization: `docs/ru/requirements.md`.

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
- Runtime: GC, resources, faults, `main` startup.
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

## 3. Language requirements

### 3.1 Lexical structure

- L-1. A file is UTF-8 with the `.unio` extension; blocks by 4-space indentation; tabs and mixed indentation are errors.
- L-2. The keyword set is fixed and published in the reference; adding a keyword after 0.1 requires an RFC.
- L-3. Comments `#`, documentation comments `##`. A documentation comment attaches to the next declaration and is available to `unio doc` and `unio symbol`.

### 3.2 Types

- L-4. Built-in types: `Int`, `Float`, `Bool`, `Text`, `Bytes`, `List[T]`, `Map[K, V]`, `Set[T]`, `Option[T]`, `Result[T, E]`, `Tuple`, `Path`, `Function`.
- L-5. No implicit conversions between types. Conversion is an explicit call (`to_text`, `to_int`, `to_float`).
- L-6. User-defined types: `struct`, `enum`, `interface`, `error`; interface implementation via `extend ... with ...`.
- L-7. Generic functions and types with parameters and interface constraints.
- L-8. Type inference only for local `let`/`var`. Parameters, results, fields, and exported constants must be annotated.
- L-9. Equality of structs and enums is by content; the `Equal` implementation is derived automatically. `Float` compares per IEEE 754 and does not implement `Hash`: it cannot be a `Map` key, a `Set` element, or a `match` pattern. Resources do not implement `Equal`; `==` on them is a compile error with the hint `is_same`.
- L-9a. Operators are bound to standard interfaces (`Add`, `Subtract`, `Multiply`, `Divide`, `Equal`, `Order`, `Hash`). A user-defined type implements them only explicitly through `extend`; redefining existing implementations is forbidden.
- L-9b. Function effects are not declared in signatures. The compiler derives them from the call graph and shows them in `unio symbol` and `unio doc` (C-11). The word `uses` is reserved.

### 3.3 Mutability

- L-10. `let` is immutable, `var` is mutable. Reassigning a `let` is a compile error.
- L-11. Mutating collection methods are available only on `var`. A function modifies an argument only through a parameter declared `var`; the caller must pass a `var` value.

### 3.4 Errors and missing values

- L-12. `Option[T]` with `some`/`none`, operators `or` and `?.`.
- L-13. `error` declares an error type. `fails with` lists a function's errors and is part of its type.
- L-14. Calling a function with `fails with` without `try` is a compile error. Prefix `try` propagates; `try:` with `on` handles; `try ... on E as e:` handles one error inline.
- L-15. The compiler checks completeness of handling: every error is either handled or listed in the current function's `fails with`. `on Error` handles any error.
- L-16. `Result[T, E]` supports `try` and `match`.
- L-17. Faults: index out of range, division by zero, `Int` overflow, failed `assert`, out of memory. Behavior: diagnostic to stderr, exit code 101, release of resources in `using`. A fault cannot be handled with `on`.
- L-18. Every faulting operation in the standard library has a safe form returning `Option` or an error.

### 3.5 Memory and resources

- L-19. Managed values live under the GC. A program cannot obtain the address of a value.
- L-20. The `Resource` interface with the method `release`. A resource value is created in `using` or as a field of another resource; storing a resource in an ordinary struct that outlives the block is a compile error.
- L-21. Release in `using` happens on normal exit, on error, and on fault, in reverse order.

### 3.6 Modules and visibility

- L-22. A module is a file; a package is a directory with `unio.toml`. Import: `import a.b`, `import a.b as c`, `from a.b import x, y`.
- L-23. Everything is private by default; `public` exports.
- L-24. Cyclic imports are a compile error that names the cycle.

### 3.7 Tests in the language

- L-25. A top-level `test "description":` block. `assert`, `assert_fail`, and `expect_fault` are available. Tests are excluded from `build` output.

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

- C-11. `unio symbol <full name>` returns (text and JSON): signature, documentation, declaration site, usage list, and for functions the error set, derived effects (`files`, `network`, `process`, `env`), and the list of called functions.
- C-12. `unio outline <file>` returns the file's declarations with signatures.

### 4.4 Runtime

- C-13. The GC is precise, tracing, non-moving. Pauses and throughput are measured in the performance suite.
- C-14. The stack trace on a fault contains function names, files, and lines without external debug formats.
- C-15. Exit codes: 0 success, the value of `main` otherwise, 101 fault, 102 out of memory.

## 5. Standard library requirements

- S-1. Modules: `std.text`, `std.collections`, `std.math`, `std.decimal`, `std.files`, `std.path`, `std.env`, `std.args`, `std.process`, `std.json`, `std.time`, `std.random`, `std.log`, `std.test`.
- S-1a. `std.decimal.Decimal` provides exact decimal arithmetic, implements all operator interfaces and `Hash`; constructed from text and `Int`; conversion from `Float` is explicit only, with a stated rounding mode.
- S-2. Every function that can fail declares concrete error types; `fails with Error` is forbidden in the standard library.
- S-3. Every module is documented; every public function has an example.
- S-4. `std.args`: positional arguments, flags, defaults, automatic help.
- S-5. `std.json`: parse to a value, encode and decode structs from their declaration, without annotations.
- S-6. `std.process`: run a program with arguments, capture output, exit code, timeout.
- S-7. Unsafe code (`external function`) is allowed only in the standard library and passes a separate reliability review (section 7.3).

## 6. Tooling requirements

### 6.1 Commands

| Command | Purpose | Required options |
|---|---|---|
| `unio new <name>` | Create a package with manifest and example | `--lib`, `--bin` |
| `unio build` | Build an executable | `--release`, `--target` |
| `unio run` | Build and run | arguments after `--` |
| `unio check` | Check without building | `--diagnostics=json` |
| `unio fmt` | Format | `--check` for CI |
| `unio test` | Run tests | `--json`, name filter |
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

### 6.4 Packages

- T-6. Manifest `unio.toml`: name, semantic version, dependencies, permissions (`files`, `network`, `process`, `env`), list of `unsafe module`.
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
- R-2. The runtime, on out-of-memory, exits with code 102 and a diagnostic without corrupting files opened through `using`.
- R-3. A fault inside `using` releases all resources; verified by tests on every platform.

### 7.3 Security

- R-4. User code cannot bypass bounds, type, or resource checks.
- R-5. All `external function` code in the standard library is reviewed by two people and tested with memory sanitizers.
- R-6. The release build is reproducible, artifacts are signed, an SBOM is published.
- R-7. The compiler and package manager do not execute dependency code during a build.

### 7.4 Compatibility and platforms

- R-8. Platforms: Linux x86-64, Linux arm64, macOS arm64, Windows x86-64. All tests pass on all platforms in CI.
- R-9. Within 0.x, incompatible language changes are possible; each ships with an automatic migration in `unio fmt --migrate` or a diagnostic with a fix.

### 7.5 Fitness for AI

- R-10. A control set of 200 programs with injected errors (types, errors, resources, style). Criterion: an agent receiving only the JSON diagnostics fixes at least 80% within at most three iterations.
- R-11. The average diagnostic message is at most 200 characters; the explanation is separate.

## 8. Documentation requirements

- D-1. Language reference: grammar in EBNF, semantics of every construct, list of diagnostic codes.
- D-2. Guide: from installation to publishing a package, using `todo` as the example.
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
- Language decisions are made through RFCs in `docs/rfcs/`; the specification is updated when an RFC is accepted.
- The questions left open in the first version of the concept are settled in `docs/concept.md`, sections 10–11. Revisiting them is possible only through an RFC.
