# Interactive and maintenance tools

Version 0.1-draft. Russian localization: [ru/tools.md](ru/tools.md). This document describes, as implemented, the tools of requirements T-11, T-12, T-13, T-15 and R-9 ([requirements.md](requirements.md), sections 6.5 to 7.4; the contracts are in [language-contracts.md](language-contracts.md), sections 5 and 6): the REPL, documentation tests, fixes bound to a revision, property replay, the comparison of public APIs, the migrations of `unio fmt`, and test running in the editor. What each one still lacks is in [status.md](status.md), rows T-11, T-12, T-13, T-15 and R-9.

## 1. `unio repl`

```sh
unio repl [--policy FILE] [--allow-weaker-sandbox] [--lang=en|ru]
```

The session belongs to the package around the working directory (the nearest directory above it with `unio.toml`, or the directory itself). It is a module of that package named `unio_repl_session.unio` at the package root, kept in memory and never written: the declarations entered so far, then a function whose body is the current input. A file of that name in the package stops the session at the start.

**Checks.** Every input is checked as that module, with the name, type, error, resource, layer and effect checks of `unio check` and the manifest of the package; the diagnostics are those of `unio check`, with the place given as `input:LINE:COLUMN`. The check runs in a worker of the tools that executes nothing (SB-11), with the `[compile]` limits of the policy. An input with an error is rejected and changes nothing.

**Inputs.** An input that starts with `public`, `function`, `struct`, `enum`, `error`, `interface`, `extend`, `import`, `from`, `external` or `test` is a declaration and joins the session. A single line that is not a statement and assigns nothing is an expression: its value is shown with its type (`= 42 : Int`); a value of type `nothing` is not shown. Anything else is statements. A line that ends with `:` starts a block, which an empty line ends.

**Running.** An input that checks runs in a separate process, the evaluator, in the sandbox of `unio run`: the permissions of the manifest narrowed by the policy (SB-1 to SB-5), applied before the program is loaded from the sources the worker returned. The session stops the evaluator after the wall-clock limit of the policy (`[limits] wall_seconds`), or after 60 seconds without one, and goes on. A fault or an unhandled error ends the input; what it did before is not undone (no transactional rollback), and the session says so.

**Bindings.** A binding at the top level of an input (`let`, `var`) is kept by its value, written as source: integers, finite floats, truth values, texts, lists, maps, sets, tuples, `Option` and `Result` of those, and structs and enums declared in the session. The next input starts from those literals (`let x as Int = 40`), so no earlier input runs again and no effect is repeated; a change to a `var` binding is kept. A function value, a resource, bytes, a path, a non-finite float and a value of a type declared elsewhere are not kept, and the session names them. A resource never outlives the `using` block of its input. A kept value that no longer checks (a declaration it needs changed) is dropped with a note.

**Commands.** `:type EXPR` checks an expression and shows its type without running anything; `:fix [N]` applies a fix (below); `:show` lists the declarations and the kept bindings; `:reset` forgets them; `:help`; `:quit` or the end of the input ends the session.

**Fixes bound to a revision.** Every input has a revision, `r1`, `r2`, and so on. A rejected input lists the fixes of its errors as `fix N [rREV]: ...`. `:fix N` (or `:fix rREV.N`) applies fix N only while the session is still at that revision; after another input it is refused (`the fix belongs to revision r5, and the session is at r6`). The fixed input is shown and checked again as a new revision before it runs.

## 2. Documentation tests

`unio test` runs the examples of documentation comments as tests of their module. An example is a block of `##` lines indented by four spaces after `## `, below a line `## Example:` or `## Examples:` of the same comment; an empty `##` line or a line of text separates two examples:

```unio
## Adds two numbers.
##
## Example:
##     assert add(2, 3) == 5
public function add(a as Int, b as Int) returns Int:
    return a + b
```

The test of an example is named `doc: DECLARATION (line N)`, after the declaration the comment documents and the line of the example's first statement, so the name filter of `unio test` selects it (`unio test "doc: add" lib.unio`). It is checked with the module, sees what a `test` block of the module sees, and runs in the sandbox of the package; its diagnostics and traces point at the lines of the comment. Indented lines without the heading (usage text) are not examples. `unio check` does not run examples; the file is never changed. The implementation is `compiler/unio/src/doctest.rs`; the editor finds the same tests (section 7).

## 3. Diagnostics and fixes in the editor

The language server (T-10) gives every diagnostic the revision of the document it was made for (`data.revision`, the version of the document), its fixes (`data.fixes`), and, for a value of another type, the type the place expects (`data.expected`, E0061). `textDocument/codeAction` offers the fixes as quick fixes only while the document is at that revision: after a change, the old diagnostics have none until the checks ran again. Each edit names that revision (`documentChanges` with the version of the document), so that the editor refuses to apply it to a later text, and applying it changes the document, which runs the checks again. Code actions read only what the checks published; nothing is analysed or run for them. The language adds no syntax for typed holes.

## 4. Property tests

`std.test.check_property` ([stdlib/alloc/test.unio](../stdlib/alloc/test.unio)) generates cases deterministically from a seed, shrinks a counterexample within an attempt budget, and returns its replay data (scheme `unio-property-v1`). `unio test` records every counterexample the properties of a test return. When the test fails, it prints the scheme, the seed, the case index, the sample seed, the original and the minimal value with the number of shrink attempts, and the command that replays it:

```text
FAIL p.unio: small numbers stay small
     fault: assertion failed: found == none
     property unio-property-v1: seed 3, case 3, sample seed 1216431607, original 607, minimal 500 after 216 shrink attempts
     replay: unio test --seed 3 "small numbers stay small" p.unio
```

`unio test --seed N` gives every property of the run the seed N in place of the seed of its call, so a printed seed replays its counterexample and another seed explores other cases; the same seed always gives the same cases. `unio test --json` adds to each test a list `properties` of replay records with the scheme, the seeds, the values, the shrink attempts, the replay command, and the versions of the compiler and of the library the counterexample was found with (`compiler`, `library`), to store with the record. A minimized value is minimal for that shrinker and budget only. Replay needs pure generators and predicates. A run is bounded by the `[limits]` of the policy, which the sandbox enforces even when a generator never returns.

## 5. `unio api-diff`

```sh
unio api-diff [--json] DIR                      # the public API of a package
unio api-diff [--json] [--callers DIR]... OLD NEW
```

The public API of a package (scheme `unio-api-v1`) records, for every public declaration of its modules, by name relative to the package (`strings.shout`, `strings.Box`): the kind; the type parameters with their constraints; the parameters with their labels, `named` and `var` markers and default values; the result; the error set; the effects a call needs; the public fields and whether there is a private one; the variants; the members of an interface; and the content hash (C-11a, C-11b). It also records the permissions of `[permissions]` of the manifest. The package must check without errors. The work runs in a worker of the tools that reads only the given packages, their locked dependencies, and the standard library (SB-11).

Comparing OLD with NEW gives each change a level:

| Level | Changes |
|---|---|
| major (incompatible) | a public declaration removed; a parameter added without a default, removed, retyped, made `var` or not, made `named`, or renamed (every parameter can be given by its label); a default removed or changed; the result changed; the error set widened; a new effect; a type parameter added or removed, or a constraint added; an enum variant added (a `match` without `case _` no longer covers it) or removed; a public field removed, retyped, no longer `var`, or added without a default to a type callers can construct; a private field added to such a type; an interface member added or changed; a permission added to the manifest (installing needs a new confirmation, T-9) |
| minor (addition) | a public declaration added; a parameter added with a default or given a default; a parameter no longer `named`; the error set narrowed; an effect or a constraint dropped; a public field added with a default; a permission dropped |
| patch | only the content hash changed: the implementation, never by itself an incompatibility |

The semantic-version rule: from `X.Y.Z` with `X ≥ 1`, major changes need `X+1.0.0`, minor ones `X.Y+1.0`, patch ones a later version; from `0.Y.Z`, major changes need `0.Y+1.0` and the others `0.Y.Z+1`. The exit code is 1 when the version of NEW is too small for its changes (the report says which version is needed), 2 for a usage mistake or a package that does not check, and 0 otherwise.

`--callers DIR` names a package that depends on the compared one (by `deps.<alias>`, locked to either version): each change lists the places there that use the changed declaration, and the declarations that reference a changed type. A change has a migration when it is unambiguous: a function renamed (one removed and exactly one added in the same module with the same signature), a label renamed in place, or a parameter made `named`; with `--callers`, the migration is given as edits at those places (`main.unio:5:19: whisper -> murmur`, `main.unio:6:28: insert 'size = '`). The tool reports edits and does not write them; a parameter made `named` is also migrated by `unio fmt --migrate` (section 6) once the dependency is updated. The implementation is `compiler/unio/src/api.rs`.

## 6. `unio fmt --migrate`

```sh
unio fmt --migrate [--from=VERSION] [--check] FILE...
unio fmt --migrate          # lists the migrations
```

R-9: within 0.x an incompatible change of the language ships with an automatic migration or a diagnostic with a fix. A migration is a registered diagnostic whose fix is unambiguous: `unio fmt --migrate` checks the files as one program, applies exactly the fixes the checker gives for the registered codes, reports each change as `FILE:LINE:COLUMN: MIGRATION: TEXT`, and formats the result. A fix that is not unambiguous is not offered, so it is never applied. The rewrite and the diagnostic cannot disagree, and a second run finds nothing to do. With `--check`, the files that would change are listed and none is written (exit code 1). `--from=VERSION` applies only the migrations of the language versions after VERSION.

The registry (`compiler/unio/src/migrate.rs`), keyed by the version of [concept.md](concept.md) that made the change:

| Migration | Version | Code | Change | Decided in |
|---|---|---|---|---|
| `variant-in-full` | 0.4 | E0072 | a variant whose short name collides with a visible name is written `Enum.Variant`, when one enum has it and its name is visible | concept 0.4, section 2.6; L-6a |
| `named-only-label` | 0.13 | E0040 | a positional argument for the one `named` parameter the call leaves without its name gets its label | RFC 0001; L-28 |

The other accepted RFCs keep existing programs valid (RFC 0002 to RFC 0008 say so in their Migration sections) and need no entry. A new incompatible change adds an entry with the fix of its diagnostic and a test in `compiler/unio/tests/migrate.rs` that migrates the example of its catalog entry into the fixed example.

## 7. Test running in the editor

The VS Code and Cursor extension ([editors/vscode](../editors/vscode/README.md)) shows the tests of `.unio` files in the Testing view: the `test` blocks and the documentation examples of each file, found in its text by the rules of `unio test`, refreshed as the file changes. Running a file, or one test, runs `unio test --json` with the executable and the sandbox arguments (`--policy`) of the extension's settings, in the sandbox of the package, and shows each result by name: passed, failed with its message, output, and the replay of a failing property, or errored when the file does not compile. The quick fixes of section 3 appear as code actions. The module `testing.js` does this without the VS Code API, so its tests run under Node.js (`npm test`), against a built `unio` when there is one.
