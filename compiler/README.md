# The Unio compiler

The front end of the Unio compiler, written in Rust (requirements, section 10): the lexer, the
parser, the syntax tree, diagnostics and their catalog, the formatter, the outline, and name and
type checks. These are sprints S1 to S3 of the plan; errors, resources, the interpreter, and code
generation follow in later sprints.

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
| `unio check [--diagnostics=json] [--lang=en\|ru] FILE...` | Parses the files and the modules they import, then checks names and types (docs/semantics.md, sections 1–4) and reports every error. Names and types are checked only when every module parses. Exit code 0 without errors, 1 with errors, 2 on wrong usage or an unreadable file |
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
so that one mistake is reported once. Known limits: the arguments of generic interfaces are not
compared when checking an implementation (`Iterator[Int]` counts as `Iterator[Text]`); the
effects of `fails with` in function types are not compared; mutability, resources, errors and
exhaustiveness are sprint S4.

Two readings of docs/semantics.md were chosen where the document is silent: the public error
types of a module imported with `import a.b` are visible by their short names (`on IoError` after
`import std.files`, as the reference programs and concept section 3.2 write them), and a variant
declared or imported in a module hides a prelude name of the same spelling for rule L-6a.

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
- `tests/semantic.rs`: the semantic corpus in `spec/tests/semantic` (one focused file per code in
  `invalid/`, with `# expect-error: LINE CODE`, and programs in `valid/` that must be clean), the
  exit criterion of S3 that every `.unio` source of the repository passes `unio check`, and
  random renaming of names in real programs that must never make the checker panic (R-1). The
  semantic corpus is checked by the compiler only; `spec/check_grammar.py` checks syntax.

## Not done yet

- Sprint S4: `fails with` and `try` (E0073 to E0076), mutability (E0077 to E0079, E0042),
  resources (E0080, E0081), control flow (E0082, E0083), exhaustiveness (E0068), discarded
  results (E0043), layers, profiles and effects (E0084, E0085), and `unio symbol`.
- The interpreter, `unio test`, code generation, and the other commands of requirement 6.1.
