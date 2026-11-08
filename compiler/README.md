# The Unio compiler

The front end of the Unio compiler, written in Rust (requirements, section 10): the lexer, the
parser, the syntax tree, diagnostics and their catalog, the formatter, the outline, the static
checks of [docs/semantics.md](../docs/semantics.md), `unio symbol`, and the interpreter of
[docs/runtime.md](../docs/runtime.md) behind `unio run` and `unio test`, which run every program
in the sandbox of SB-1, and `unio build`, which compiles a program to one native executable
through Cranelift (docs/runtime.md 8.4). These are sprints S1 to S5 of the plan, the sandbox
part of S6, and the first part of its native code generation, for Linux on x86-64.

## Build and test

The front end, the interpreter and the runtime use the Rust standard library only. The code
generator of `unio build` uses Cranelift (`cranelift-codegen`, `cranelift-frontend`,
`cranelift-module`, `cranelift-object`, a stable release pinned to exact versions, with the
crates they need recorded in `Cargo.lock`), the one dependency that requirements section 10 names;
an executable contains none of it, because the linker drops what the runtime does not call. The
sandbox calls Landlock, seccomp, resource limits and Seatbelt through `extern` declarations in
`unio-runtime/src/sandbox/`, as the runtime already does for `fsync`, `getpwuid` and
`MoveFileExW`, so `libc` and `windows-sys` are not dependencies (R-6).

The library crate `unio` is also built as a static library, `target/<profile>/libunio.a`: the
runtime that `unio build` links into every executable, found next to the `unio` binary or
through `UNIO_RUNTIME_LIB`. Linking needs the system C compiler (`cc`, or `CC`).

```sh
cd compiler
cargo build --release          # target/release/unio
cargo test                     # corpus, repository sources, diagnostics, fuzzing, interpreter
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test --release -p unio --test tools -- --ignored   # Unio tools against the Python checkers
cargo test -p unio --test native                          # native executables against unio run
```

The workspace has two crates: `unio` (this front end, the interpreter, and the command) and
`unio-runtime` (values, faults, the release stack, and the externals; see its README).

`UNIO_FUZZ_ITERATIONS=100000 cargo test --release --test fuzz` runs a longer mutation run.

## Commands

| Command | What it does |
|---|---|
| `unio check [--diagnostics=json] [--lang=en\|ru] [--profile=application\|core] FILE...` | Parses the files and the modules they import, then runs the checks of docs/semantics.md, sections 1–8, and reports every error. The semantic checks run only when every module parses. With `--profile=core`, every function of the package that needs a layer above `core` is E0084 with its chain of calls (C-21). Exit code 0 without errors, 1 with errors, 2 on wrong usage or an unreadable file |
| `unio symbol [--json] MODULE.NAME` | Signature, documentation, declaration site, usages, and for a function its error set, derived effects, the lowest layer it needs and the functions it calls, plus the content hash of every function and type (C-11 to C-11b). The module is found from a prefix of the name: `std.x` in the standard library, otherwise in the package of the current directory; prelude names such as `List.map` work too |
| `unio fmt [--check] FILE...` | Writes each file in the one style of [docs/style.md](../docs/style.md). With `--check`, writes nothing, lists the files that differ, and exits with 1 if there are any. A file with syntax errors is reported and left unchanged |
| `unio outline [--json] FILE` | Lists the declarations with their signatures and lines; members of structs, enums, errors, interfaces, and extend blocks are indented (C-12) |
| `unio parse --ast FILE` | Prints the syntax tree as an S-expression without spans; the form is stable and is used to compare parses |
| `unio explain CODE` | Prints the explanation of a diagnostic code with an example of the error and the example fixed (C-9) |
| `unio run [--policy FILE] [--allow-weaker-sandbox] FILE [-- ARGS...]` | Checks the program like `unio check` and, without errors, runs `public function main()` of FILE with ARGS as `std.args.list()`, in the sandbox described below. The sandbox options come before FILE, or between FILE and `--`. Exit code: the result of `main` (0 without one), 101 after a fault with its trace on standard error, 102 on exhaustion (heap, stack, wall or CPU time of the policy), 103 when the policy or the manifest cannot be read or parsed or the sandbox cannot be applied, 1 if the program does not compile |
| `unio build [--release] FILE [-o OUT]` | Checks the program like `unio run` and, without errors, compiles it to one executable OUT (by default the name of FILE without `.unio`), for Linux on x86-64 so far; `--release` optimises the generated code. The executable runs `main` with its arguments as `std.args.list()`, in the sandbox of the manifest of FILE's package narrowed by `UNIO_POLICY`, and ends with the codes of `unio run`. A construct not compiled natively yet stops the build with its place (docs/runtime.md 8.4). Exit code 0 when built, 1 otherwise, 2 on wrong usage |
| `unio test [--json] [FILTER] FILE...` | Runs every `test` block of the files, each in a fresh state (runtime 6.2), or those whose name contains FILTER. A test passes, fails with the fault that ended it, or fails because it does not compile (T-5a): a diagnostic inside a test block fails only that test, one outside fails the whole file. Text: `ok`/`FAIL`, name and time per test, then the totals; `--json` gives an array with file, name, status, time, output and message (T-4). Each file's tests run in the sandbox of its package, with the same options and codes as `unio run`. Exit code 0 when every test passed, 1 otherwise |

The language of messages comes from `--lang`, then from the environment variable `UNIO_LANG`,
and is English otherwise (C-10). Codes and the JSON format do not depend on it.

### The sandbox

The permissions of a program are the `[permissions]` of the `unio.toml` nearest above its file
(T-6), narrowed by the policy of `--policy FILE` or `UNIO_POLICY` (SB-2, SB-3). Without a
manifest, or with one that has no `[permissions]`, a program has only its arguments and standard
streams. `unio test` uses the same rule: a test that needs files declares them in its package.

```toml
# A policy. Without [permissions] it narrows nothing; with it, a kind it does not list is denied.
allow_weaker = false        # true is the same as --allow-weaker-sandbox (SB-5)

[permissions]
files.read = ["$CWD/src/**"]

[limits]
heap_bytes = 268_435_456    # the runtime's count of live values; 102 when exceeded
wall_seconds = 60           # 102 when exceeded
cpu_seconds = 30            # 102 when exceeded; each child gets RLIMIT_CPU
processes = 4               # process.run calls; more fail with PermissionDenied
file_size_bytes = 1_048_576 # writes over it fail; RLIMIT_FSIZE on Linux and macOS
```

A permission of the policy that the manifest does not have is a warning and has no effect.
Unknown keys are errors. Paths in either file may start with `$HOME`, `$CWD` or `$TEMP`, end
with `/**` for a subtree, and are otherwise relative to the file's directory; links are resolved
before a path is compared, so a link cannot lead out of a scope.

Two layers enforce the permissions. The runtime checks every external function of `std.files`,
`std.env` and `std.process` and reports a denied operation as `IoError` or `ProcessError` of kind
`PermissionDenied` naming the permission, `none` from `env.get`, and `false` from `files.exists`
and `files.is_directory` (SB-6, S-8). The operating system restricts the thread that runs the
program, its threads and every child process (SB-1, SB-4), after the sources are loaded and
before `main` or the first test:

| System | Mechanisms | Not covered |
|---|---|---|
| Linux | `no_new_privs`; Landlock (ABI 1 to 7: files, TCP from ABI 4, signals and abstract sockets from ABI 6); seccomp-bpf: no sockets at all, no `ptrace`, `process_vm_*`, `mount`, `unshare`, `setns`, namespace flags of `clone`, `io_uring`, `bpf`, `perf_event_open`, `userfaultfd`, `keyctl`, `open_by_handle_at`, `kexec`, modules, `TIOCSTI`; `RLIMIT_CORE` 0, `RLIMIT_FSIZE` | Landlock grants a scope of one written file on its directory, because the atomic commit creates a temporary file next to the target; scope roots that do not exist at the start grant nothing; with a `process` permission, children may read and execute the system directories (`/usr`, `/lib*`, `/bin`, `/etc/ld.so.*` and the like); `stat` is not restricted by Landlock |
| macOS | a Seatbelt profile (`sandbox_init`, deny by default) and the same resource limits | Seatbelt applies to the whole process once, so `unio test` grants the union of its files' permissions plus reading their sources and the standard library; the runtime still keeps each file to its own |
| Windows | none yet: AppContainer and Job Objects are not implemented | `unio run` and `unio test` stop with 103 unless the operator allows the weaker sandbox, which keeps only the runtime layer and the limits |

A missing mechanism (no Landlock or seccomp in the kernel, Landlock below ABI 3, Windows) stops
the start with 103; with `--allow-weaker-sandbox` or `allow_weaker = true` the program runs and
the missing restrictions are printed (SB-5). The program itself cannot lift any of this.

A child of `process.run` gets only the variables granted under `env` and `PATH` (on Windows also
`SystemRoot`); it runs in a process group of its own on Unix and in a Job Object with
kill-on-close on Windows, and what is left of that tree is stopped when the child ends, times
out, or overflows its output. The deadline covers reading the outputs; each output holds at most
16 MiB (`OutputTooLarge`) and counts against the heap limit (docs/runtime.md 9.5). Reads whose
size the program decides (`read_bytes(max_bytes)`, whole files, `Text.repeat`) check the heap
limit before they allocate, so a refused allocation ends with 102 instead of an abort
(docs/runtime.md 3.4, 3.5).

## What is implemented

- The lexer of grammar section L: INDENT, NEWLINE, and DEDENT, implicit continuation inside
  brackets, comments and documentation comments kept as trivia with spans, keywords and
  contextual keywords, the reserved word `uses`, numbers, and text with recursive interpolation.
  It reports E0001–E0003, E0005–E0011, and E0032 on the same lines as `spec/check_grammar.py`,
  and continues after an error.
- The parser of grammar sections F, S, E, P, and T, including `named` parameters and function
  shapes (RFC 0001), the pattern test `is`, lambda parameters without types, and one-line
  branches (RFC 0002). It follows the ordered choices of the EBNF, implements the generic-call
  rule, and after an error skips to the end of the line and over the block that follows, so that
  one file reports several errors (C-2).
- The syntactic constraints that the EBNF states in comments: chained comparisons (E0020), a
  positional argument after a named one (E0023), an inline handler without a block or not at the
  end of its statement (E0024), an invalid assignment target (E0030), and a rest element that is
  not last (E0034).
- Diagnostics in the text form of C-7 and the JSON form of C-8, in English and Russian (C-10),
  with a fix where one is obvious (a tab replaced by spaces, a missing ':').
- The diagnostic catalog of R-12 in `src/catalog.rs`: every message, explanation, example of the
  error, and fixed example; the review checklist is [DIAGNOSTICS.md](DIAGNOSTICS.md).
- The formatter of `src/format.rs`: layout, blank lines, sorted imports, spaces, line breaking at
  the outermost group that ends last, one-line branches (T-1a), comments placed by their source
  lines, literals and parentheses as written. Every `.unio` source of the repository is in this
  style, and CI checks it with `unio fmt --check`.
- The outline of `src/outline.rs` in text and JSON.

Diagnostic codes added by the front end:

| Code | Meaning |
|---|---|
| E0012 | Unexpected token: the message names what the grammar expects at that place |
| E0013 | Code nested too deeply (expressions, patterns, types, blocks, or texts inside interpolations) |

Lexical diagnostics come first, in the order they are found, followed by syntax diagnostics in
source order. A syntax error on a line next to a lexical error is not reported, because it is
nearly always a consequence of it.

## Name and type checks (sprint S3)

`src/program.rs` loads a program: the given files, the modules they import, and the standard
library. The standard library is found through `UNIO_STDLIB`, an ancestor directory that holds
`stdlib/core/prelude.unio`, or the repository of the compiler. A package is the directory with
`unio.toml`, otherwise the directory of the file; `import a.b` loads `a/b.unio` from the package
root and `import std.x` loads `stdlib/<layer>/x.unio`. The `prelude.unio` and `builtin/` files of
the three layers form the prelude of the `application` profile. It then collects declarations,
resolves imports and cycles, the types of signatures, fields and variants, and checks every
`extend ... with` against its interface.

`src/check.rs` checks the bodies of functions, interface defaults, constants, and tests:
scopes of local names, types with local inference and expected types, calls with positional,
named and default arguments and RFC 0001 labels, generic functions with inferred arguments and
constraints, methods through `extend` blocks, interfaces and defaults, functions of a type,
struct and variant construction, operators through interfaces, `or` on `Option`, `?.`, indexing,
`in`, interpolation, lambdas typed from the expected function type, patterns, and `is`.

Codes reported: E0040, E0041, E0044, E0049, E0050, E0051 to E0067, E0069, E0071, E0072. Where a
type cannot be determined, the checker uses an unknown type that is compatible with every type,
so that one mistake is reported once. The arguments of generic interfaces are compared in
implementation checks, constraints and assignability (`Iterator[Int]` is not `Iterator[Text]`).

Two readings of docs/semantics.md were chosen where the document is silent: the public error
types of a module imported with `import a.b` are visible by their short names (`on IoError` after
`import std.files`, as the reference programs and concept section 3.2 write them), and a variant
declared or imported in a module hides a prelude name of the same spelling for rule L-6a.

## Errors, values, resources, layers and effects (sprint S4)

`src/check.rs` also follows errors, places and control flow (docs/semantics.md, sections 5–7):

- Errors: a call that can fail needs `try` or an enclosing `try:` (E0073); every error that
  leaves a function is handled or listed in `fails with`, with a fix that adds it (E0074); an
  inline handler must not complete (E0075); `try` before something that cannot fail is E0076.
  The `fails with` of function types is compared: a callback that fails with more than the
  expected type allows is E0061.
- Values: assignment to an immutable name (E0077); a change through a place that does not start
  at a `var` name, a `var` parameter or a `using` name, of a field not declared `var`, or a
  `var self` method on such a place (E0078); `var` arguments (E0079, E0042).
- Control flow: a reachable end of a function with a result (E0082), `break` and `continue`
  outside a loop (E0083), `match` exhaustiveness over enums, `Option`, `Result`, `Bool`, tuples
  and list patterns, with the missing cases named in the message (E0068).
- Results: a dropped `Result` or `std.process.Output`, or a local holding one that is never
  read, is E0043 (L-30); `let _ = ...` drops it on purpose. Other results may be dropped.
- Resources: created only as a `using` initializer, the return of a function that returns a
  resource, or a field of another resource (E0080); a `using` name must not leave its block,
  directly or captured by a lambda (E0081). A parameter of a resource type may be used, compared,
  and passed on to a parameter of a resource type or of a generic type bounded by `Resource`
  (`is_same`), but storing, returning or capturing it is E0081 too (semantics 7.2).
- Coverage of `match` is searched with a budget of 10,000 pattern rows per `match`, and the
  alternatives (`|`) of one tuple or list pattern expand to at most 256 rows; a column in which
  no case names a constructor is skipped in one step. A `match` that exceeds either limit and has
  no `case _` is not taken as covered: E0068 says that its patterns combine into more cases than
  the checker enumerates (semantics 6.5). Before S5, such a pattern counted as matching anything.

`src/effects.rs` derives effects and layers from the call graph that the checks record (section
8): an external function of `std.env`, `std.process` or `std.net` has the effect of its module,
an external of `std.files` has the scopes `files.read` and `files.write` that the "Permission:"
sentence of its documentation names ("Permission: none" for an operation on a descriptor that is
already open; a missing sentence counts as both scopes, and a test requires the sentence on every
external of `std.files`), every function of the `alloc` layer has `allocates`, and a function has
the union of what it calls. The derivation is conservative (semantics 8.4): a call through an
interface method, written or implicit (operators, `==`, comparisons, interpolation, `for`, the end
of `using`, `in`), has the union of every implementation of the method in the program, or the
method of the value's type when it is known; a lambda's body belongs to the function that makes
it and a function named as a value counts as called there; a constant counts for the functions
that use it, a parameter default for its function, and field defaults for the functions that
build the type; list, map and comprehension literals, interpolation, `+` on texts and lambdas that
capture values give `allocates` and the layer `alloc`. A library module that calls a higher
layer, or a `core` module that allocates, is E0084; `--profile=core` applies the rule to the
functions of a package, with the chain of calls down to the call or the allocating expression. When
the `unio.toml` of a package has a `[permissions]` table, a call that needs an effect the table
does not grant is E0085, once per effect and module, with the chain; `files.read` and
`files.write` are checked separately against the keys of the same names (T-6), and `env`,
`process` and `network` grant themselves. A manifest without the table is not checked.

`src/symbol.rs` and `src/sha.rs` compute the content hashes of C-11a and C-11b: SHA-384, with
the scheme version `unio-hash-v1` and a domain for each kind of input, over the S-expression of
the declaration (no spans, comments or layout) and the hashes of the declarations it names or
calls. Declarations that reference each other form one strongly connected component: its members
are sorted by identity, references inside it are written as identities and references outside it
as their hashes, and each member's hash binds the component hash and the member's identity.
Formatting, comments and the order of declarations do not change a hash; a change of the body,
of a name, or of anything referenced does.

## The interpreter (sprint S5)

`src/interp.rs` executes a checked program; `unio-runtime` supplies the values, faults, the
release stack and the 70 externals. The interpreter walks the checked syntax tree with the
program tables of the checker (resolved declarations, methods by type, signatures); runtime 8.1
describes a lowered representation with slots and explicit error edges, which is left to the
native runtime of S6, because the tree walk already gives the evaluation order of runtime 1 and
is the reference for it.

- Evaluation is strict and left to right (runtime 1.1), including assignment: the target place
  and its indices are resolved before the right-hand side. Defaults of parameters are evaluated
  at each call that omits them, in the module of the function and after the preceding
  arguments; externals get their defaults the same way.
- Values come from `unio_runtime::value`: reference counting with copy-on-write, so changing an
  unshared value happens in place. `var` arguments and `var self` receivers are taken out of
  their place for the call and written back afterwards, also when the call fails. A resource is a
  shared cell: its methods see one value, `release` runs once.
- Calls: functions and methods through the tables of the checker, interface values and default
  methods of interfaces, functions of a type (`T.f`), and lambdas as closures that capture the
  values of the names they use; named functions and closures implement `unio_runtime::Callable`,
  so that externals such as `expect_fault` can call them.
- Types: a value of a declared type carries the identity of its type, its name for a type of the
  standard library (whose names are unique there and which the externals use) and `module.Name`
  for a type of a package, so two types of one name in different modules keep their own methods,
  implementations, handlers and variants; the method table, the per-call-site method caches and
  the lowered handlers and patterns use the identity, in the interpreter and in native code.
  Interpolation and the generated `to_text` show the short name.
- Errors travel as `Unwind::Error` to the nearest `try ... on` or `try:` handler that names their
  type or `Error`; faults (`Unwind::Fault`) carry the trace of function, file and line of every
  frame (C-14), run the `release` of every open `using` block on the way out (C-17), and end
  `unio run` with code 101; exhaustion of the stack, or of the heap limit when a policy sets one,
  ends it with 102 (C-18). An
  exit code of `main` outside 0–100 is a fault (C-15).
- Top-level constants are evaluated on first use, once, which is dependency order (runtime 6.3).
- Externals are called by their binding-table names (`Text.slice`, `std.files.system_rename`,
  bare names for the prelude); `std.json.to_value` and `std.json.from_value` are generated from
  the static type: structs, enums, options, lists, maps and sets, with the strict decoding of
  S-17 (unknown fields and variants, missing required fields, `Float` for `Int` are
  `DecodeError` with a JSON Pointer path; defaults are fresh).
- `unio run` and `unio test` run the program on a thread with a 1 GiB stack, so that the frame
  limit of the runtime, not the native stack, decides exhaustion.

Decisions made where docs/runtime.md is silent:

- `try e` unwraps a `Result` value only when `e` is not a call of a function with `fails with`;
  `a or b` keeps an `Option` when `b` is `none` or `some(...)`, and gives the inner value
  otherwise, following semantics 4.3.
- Iterating a `Map` yields its keys in insertion order; iterating a `Text` yields its characters
  as one-character texts; `Range` iteration needs no allocation.
- The generated `to_text` of an error is `Name(field = value, ...)`, with each field as its own
  `to_text` writes it (texts without quotes).
- A `match` without a matching case cannot happen after E0068, so it is an internal fault.

Speed, measured on the reference tools in a release build on one core: the whole repository's
113 test blocks run in 0.9 s; `tools/check_grammar.unio --corpus` takes 3.7 s, the parse check of
every source 30 s, and `tools/check_api.unio` 29 s, against 0.1 s, 0.2 s and 1.4 s for the Python
checkers. A loop of a million iterations and 250,000 recursive calls takes 0.7 s. The binding
table is built once per process, frames hold shared names, and method lookup does not allocate;
the remaining cost is the tree walk and allocation of environments, which S6 replaces.

## Native code (sprint S6)

`src/native/` compiles the lowered form of `src/lower.rs` (docs/runtime.md 8.4):

- `units.rs` lists the bodies of a program (functions, constants, defaults, lambdas) in one
  order and the lowered statements, nodes, patterns and literals each passes to the runtime, in
  one canonical walk that the build and the executable share.
- `codegen.rs` translates every body with Cranelift: locals of type `Int`, `Float` and `Bool`
  unboxed in registers with inline overflow and zero checks, direct calls with scalars in
  registers, inline push and pop of the call frame and the current line for traces, temporaries
  on the stack released on every path out, and unwinding by status to `try`, `on` and `using`.
  It refuses a unit rather than generate doubtful code: a construct it cannot compile yet, or an
  internal inconsistency it detects, stops the build with the place.
- `support.rs` is the runtime side: the context of native code, the functions it calls for values
  that own memory (methods, fields, construction, patterns, iteration, places), the hooks
  through which the interpreter's dispatch and the constants and defaults enter native code, and
  `unio_native_main`, the start of an executable.
- `image.rs` encodes the embedded program; `mod.rs` loads, checks, compiles, and links.

The executable rebuilds the declaration tables from its embedded sources (`src/vfs.rs` serves
them to `Program::load`), lowers the program the same way, fills the reference tables of its
bodies, applies the sandbox, and runs `main`; the interpreter then only dispatches and never
evaluates lowered code. Timings against Python, Go and Rust are in
[bench/README.md](../bench/README.md).

## Tests

The tests were written before the code (concept, section 1.2):

- `tests/corpus.rs`: every file in `spec/tests/valid` parses without diagnostics; every file in
  `spec/tests/invalid` reports its first error on the line, and with the code, in its
  `# expect-error:` comment.
- `tests/sources.rs`: every `.unio` file in `docs/examples`, `docs/ru/examples`, `examples`,
  `stdlib`, and `tools` parses; the AST dump is stable.
- `tests/diagnostics.rs`: several errors in one run, the text and JSON formats, both languages,
  and the syntactic constraint codes.
- `tests/fuzz.rs`: deterministic mutations of the corpus and the sources never make the front end
  panic, and extreme nesting gives E0013 instead of a stack overflow (R-1).
- `tests/format.rs`: a golden test for each rule of docs/style.md; for every valid corpus file
  and every repository source, the formatted text parses, has the same tree with imports sorted
  (T-2), formats to itself (T-1), and keeps every comment in order; a generated 1,000-line source
  formats within 50 ms in a release build (`cargo test --release --test format thousand`, T-3;
  a debug build is allowed 1 s).
- `tests/outline.rs`: the text and JSON forms of the outline.
- `tests/catalog.rs`: every code has an explanation and a pair of examples that the front end
  confirms, the semantic codes through the whole check; messages follow the template and name no
  compiler internals.
- `tests/symbol.rs`: `unio symbol` in text and JSON, content hashes that ignore layout, comments
  and order and follow changes through references and mutual recursion, the core profile and
  the permissions of the manifest, allocating expressions under the core profile and in a
  `core` module of the library, and effects that reach a manifest through an interface method
  and through a function value kept in a constant or a parameter default (semantics 8.4).
- `tests/semantic.rs`: the semantic corpus in `spec/tests/semantic` (one focused file per code in
  `invalid/`, with `# expect-error: LINE CODE`, and programs in `valid/` that must be clean), the
  exit criterion of S3 and S4 that every `.unio` source of the repository passes `unio check`, and
  random renaming of names in real programs that must never make the checker panic (R-1). The
  semantic corpus is checked by the compiler only; `spec/check_grammar.py` checks syntax.
- `tests/interpreter.rs`: every `test` block of `stdlib`, `examples`, `docs/examples`,
  `docs/ru/examples` and `tools` runs and passes under `unio test` (the exit criterion of S5);
  failing and non-compiling tests and the filter; the reference utilities `wc`, `grep_lite`,
  `json_query`, `csv_sum`, `log_tail`, `rename --dry-run` and `todo` (with `TODO_FILE` in a
  temporary directory) end to end with their output and exit codes; a fault that releases two
  `using` blocks in reverse order and ends with 101 and its trace, exit codes of `main` inside and
  outside 0–100, and recursion that ends with 102; two modules with a struct, an error and an
  enum of the same names (`tests/programs/same_names`), whose methods, interface
  implementations, handlers and variants stay apart under `unio run` and `unio test`.
- `tests/sandbox.rs` (SB-8): programs that try forbidden actions through `std.files`, `std.env`
  and `std.process` (reading, listing, resolving, writing, copying, renaming, creating outside
  the scopes, `..` and links out of a scope, hidden variables, unlisted programs), with no
  permissions at all, and through child processes that read, write, connect and start
  grandchildren (Linux and macOS); a policy that narrows and warns, through `--policy` and
  `UNIO_POLICY`; the limits of heap, wall and CPU time (102), processes and file size; code 103
  for an unreadable or invalid policy or manifest; tests that run in their package's sandbox; a
  child that sees only the granted environment; a read whose `max_bytes` exceeds the heap limit,
  which ends with 102; and `list_tree` over a cycle of links (Unix). `unio-runtime/tests` cover
  the process tree (a grandchild that holds the output, a timeout that stops the grandchild),
  output over 16 MiB, captured output over the heap limit, renaming that never replaces a file
  or a directory, and reads and `Text.repeat` over the heap limit.
  `unio-runtime` has unit tests for the TOML subset, scopes, narrowing, the runtime checks,
  Landlock and seccomp on a thread, and Seatbelt in a child process.
- `tests/native.rs` (Linux x86-64; skipped with a note elsewhere): every reference program that
  the interpreter tests run, with the same inputs, is built with `unio build --release` and
  gives the same standard output, standard error and exit code as `unio run`: the utilities,
  the task list, faults of every kind with their traces, exit codes, exhaustion of the stack, the
  benchmarks, the four tools over the repository, and the types of one name in two modules of
  `tests/programs/same_names`; two builds give identical bytes; a program
  with errors is not built; the executable applies the manifest of its package.
- `tests/tools.rs` (ignored by default, run in CI in a release build): `tools/check_docs.unio`,
  `tools/check_grammar.unio` (corpus, self-check, sources, invalid sources, parse trees) and
  `tools/check_api.unio` under `unio run` give the same output and exit code as the Python
  checkers.

## Not done yet

- Known limits of the effects: a call through an interface counts every implementation in the
  program, not only those whose values can reach it, and a call through a function value is
  charged where the value is made, so `unio symbol` shows the value's effects for the function
  that makes it, not for one like `List.map` that only calls it (semantics 8.4); the checks of
  parameter and field defaults record their effects but report no type errors yet; usages of
  types are listed per referencing declaration.
- Known limits of `process.run` (docs/runtime.md 9.5): a process that leaves the process group
  (`setsid`) or, on Windows, starts before the child joins its job is not stopped; on Unix an
  interrupt from the terminal does not reach a child in its own group. Heap exhaustion that the
  system reports late (overcommit and its out-of-memory killer) ends the process without 102.
- The sandbox of the operating system on Windows (AppContainer, the limits of a Job Object), the isolated workers
  of the tools (SB-10 to SB-13), the hostile projects of SB-15, native code for macOS, Windows and AArch64 and the rest of S6
  (docs/runtime.md 8.6), and the other commands of requirement 6.1.
