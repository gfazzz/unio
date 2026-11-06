# The Unio compiler

The front end of the Unio compiler, written in Rust (requirements, section 10): the lexer, the
parser, the syntax tree, diagnostics and their catalog, the formatter, the outline, the static
checks of [docs/semantics.md](../docs/semantics.md), and `unio symbol`. These are sprints S1 to S4
of the plan; the interpreter and code generation follow in later sprints.

## Build and test

Rust stable and the standard library only: there are no external crates, so the build needs no
network and is reproducible (C-4).

```sh
cd compiler
cargo build --release          # target/release/unio
cargo test                     # corpus, repository sources, diagnostics, fuzzing
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

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

The language of messages comes from `--lang`, then from the environment variable `UNIO_LANG`,
and is English otherwise (C-10). Codes and the JSON format do not depend on it.

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
  directly or captured by a lambda (E0081).

`src/effects.rs` derives effects and layers from the call graph that the checks record (section
8): an external function of `std.files`, `std.env`, `std.process` or `std.net` has the effect of
its module, every function of the `alloc` layer has `allocates`, and a function has the union of
what it calls. A library module that calls a higher layer is E0084; `--profile=core` applies the
rule to the functions of a package, with the chain of calls. When the `unio.toml` of a package has
a `[permissions]` table, a call that needs an effect the table does not grant is E0085, once per
effect and module, with the chain (`files.read` or `files.write` grant `files`; `env`, `process`
and `network` grant themselves). A manifest without the table is not checked.

`src/symbol.rs` and `src/sha.rs` compute the content hashes of C-11a and C-11b: SHA-384, with
the scheme version `unio-hash-v1` and a domain for each kind of input, over the S-expression of
the declaration (no spans, comments or layout) and the hashes of the declarations it names or
calls. Declarations that reference each other form one strongly connected component: its members
are sorted by identity, references inside it are written as identities and references outside it
as their hashes, and each member's hash binds the component hash and the member's identity.
Formatting, comments and the order of declarations do not change a hash; a change of the body,
of a name, or of anything referenced does.

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
  the permissions of the manifest.
- `tests/semantic.rs`: the semantic corpus in `spec/tests/semantic` (one focused file per code in
  `invalid/`, with `# expect-error: LINE CODE`, and programs in `valid/` that must be clean), the
  exit criterion of S3 and S4 that every `.unio` source of the repository passes `unio check`, and
  random renaming of names in real programs that must never make the checker panic (R-1). The
  semantic corpus is checked by the compiler only; `spec/check_grammar.py` checks syntax.

## Not done yet

- Known limits of S4: effects follow direct calls, so a call through an interface method or a
  function value does not carry the effects of the implementation; the core profile looks at
  calls, not at list or map literals; usages of types are listed per referencing declaration;
  a `case` whose tuple and list patterns expand to more than 256 combinations is treated as
  matching anything, so exhaustiveness may miss a gap there.
- The interpreter, `unio test`, code generation, and the other commands of requirement 6.1.
