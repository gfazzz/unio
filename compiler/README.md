# The Unio compiler

The front end of the Unio compiler, written in Rust (requirements, section 10): the lexer, the
parser, the syntax tree, diagnostics and their catalog, the formatter, and the outline. These are
sprints S1 and S2 of the plan; type checking, the interpreter, and code generation follow in
later sprints.

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
| `unio check [--diagnostics=json] [--lang=en\|ru] FILE...` | Lexes and parses the files and reports every syntax error. Exit code 0 without errors, 1 with errors, 2 on wrong usage or an unreadable file |
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
  confirms; messages follow the template and name no compiler internals.

## Not done yet

- Name resolution, types, error sets, exhaustiveness, mutability, and resources (stage 2).
- Sprint S3: modules and packages, visibility (L-23 to L-23d), types and local inference,
  generics, interface values and defaults, named parameters; then the semantic codes E0040 to
  E0050 in the catalog.
- The interpreter, `unio test`, code generation, and the other commands of requirement 6.1.
