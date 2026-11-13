# Interactive and maintenance tools

Version 0.1-draft. Russian localization: [ru/tools.md](ru/tools.md). This document describes, as implemented, the tools of requirements T-11, T-12, T-13, T-15 and R-9 ([requirements.md](requirements.md), sections 6.5 to 7.4; the contracts are in [language-contracts.md](language-contracts.md), sections 5 and 6): the REPL, documentation tests, fixes bound to a revision, property replay, the comparison of public APIs, the migrations of `unio fmt`, and test running in the editor; and the commands `unio new`, `unio doc`, and `unio build --target` of table 6.1 with L-3 and L-9b. What each one still lacks is in [status.md](status.md), rows T-11, T-12, T-13, T-15, R-9, L-3, L-9b, and item O-6.

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

The language server (T-10) gives every diagnostic the revision of the document it was made for (`data.revision`, the version of the document), its fixes (`data.fixes`), its related locations as `relatedInformation` (C-8), and, for incomplete code, the type the place expects (`data.expected`): for a value of another type or a missing return value (E0061), a missing argument (E0064) or field (E0063), a missing `return` (E0082), and a syntax error where an expression is missing (E0012) or a bracket is left open at the end of a line (E0009). For a syntax error the server checks the document once more with a name in the place of the missing code and takes the type the checker expects for that name; nothing is run, and no name is added to the document. `textDocument/codeAction` offers the fixes as quick fixes only while the document is at that revision: after a change, the old diagnostics have none until the checks ran again. Each edit names that revision (`documentChanges` with the version of the document), so that the editor refuses to apply it to a later text, and applying it changes the document, which runs the checks again. Code actions read only what the checks published; nothing is analysed or run for them. The language adds no syntax for typed holes.

## 4. Property tests

`std.test.check_property` ([stdlib/alloc/test.unio](../stdlib/alloc/test.unio)) generates cases deterministically from a seed, shrinks a counterexample within an attempt budget, and returns its replay data (scheme `unio-property-v1`). `unio test` records every counterexample the properties of a test return. When the test fails, it prints the scheme, the seed, the case index, the sample seed, the original and the minimal value with the number of shrink attempts, and the command that replays it:

```text
FAIL p.unio: small numbers stay small
     fault: assertion failed: found == none
     property unio-property-v1: seed 3, case 3, sample seed 1216431607, original 607, minimal 500 after 216 shrink attempts
     replay: unio test --seed 3 "small numbers stay small" p.unio
```

`unio test --seed N` gives every property of the run the seed N in place of the seed of its call, so a printed seed replays its counterexample and another seed explores other cases; the same seed always gives the same cases. `unio test --json` adds to each test a list `properties` of replay records with the scheme, the seeds, the values, the shrink attempts, the replay command, and the versions of the compiler and of the library the counterexample was found with (`compiler`, `library`), to store with the record. A minimized value is minimal for that shrinker and budget only. Replay needs pure generators and predicates.

Every test has time and memory limits, with or without a policy (T-12): the wall time of the test is `limits.wall_seconds` of the policy or 60 seconds, its processor time is `limits.cpu_seconds` when the policy gives it, and its heap is `limits.heap_bytes` or 1 GiB (`TestLimits`, `DEFAULT_TEST_WALL_SECONDS` and `DEFAULT_TEST_HEAP_BYTES` in `compiler/unio/src/interp.rs`). In `unio test` the wall and processor time of the policy therefore hold for each test, not for the whole run. A watcher thread raises a flag at the limit that the interpreter reads at every call, at every turn of a loop, and while it sleeps (`compiler/unio-runtime/src/deadline.rs`); the test, or the generator, predicate or shrinker of `check_property` it is in, unwinds, fails with `fault: the test used up its wall time limit of N s (...)`, which says where the limit comes from and whether a property was running, and the next test runs. A test over its heap fails with `memory exhausted`, the default limit named; a list or a text that doubles past the default limit is granted once (up to the limit again) so that the test, not the run, ends. A test that waits outside the program (a child process, a blocked read) and so does not reach such a point within 10 seconds after its limit cannot be unwound: its children are stopped and `unio test` ends with code 102 and a message that names the test, so it never hangs. A child process of a test has no processor-time limit of its own in `unio test`. Checked by `compiler/unio/tests/property.rs` (a generator that loops, one that iterates, a test that sleeps, one that allocates without end, the default limit without a policy, and the test after them).

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

## 8. `unio new`

```sh
unio new NAME            # a program: unio.toml, main.unio, .gitignore
unio new --lib NAME      # a library: unio.toml, NAME.unio ('-' becomes '_'), .gitignore
```

Table 6.1. `unio new` makes the directory NAME (a path; the package is named after its last component, lowercase letters, digits, `-` and `_`, starting with a letter, as in [packages.md](packages.md) section 1) and writes into it a manifest with the name, the version `0.1.0`, and an empty `[permissions]` table, one module, and a `.gitignore` that leaves out `target/`, where `unio doc` writes. A program (`--bin`, the default) has `main.unio` with `public function main()`, which prints a greeting for its first argument; a library (`--lib`) has one module named after the package, which an application imports as `deps.<alias>.<module>`, with an example in its documentation comment (section 2). Each module has a documentation comment and a `test` block next to the code, so `unio check`, `unio test`, `unio fmt --check`, and `unio doc` accept the package as written, and `unio run main.unio` runs the program. An existing NAME is never changed: the command stops with exit code 1. A wrong name or option is a usage error with exit code 2, as is a library whose module would be a keyword or `std` or `deps`. Checked by `compiler/unio/tests/new_package.rs`; the implementation is `compiler/unio/src/new_package.rs`.

## 9. `unio doc`

```sh
unio doc [--open] [-o DIR] [--policy FILE] [--allow-weaker-sandbox] [PACKAGE]
```

Table 6.1, L-3, L-9b, D-3. `unio doc` writes the documentation of a package as HTML pages: the package of PACKAGE, a directory or a file in it, by default the working directory, is the nearest directory with `unio.toml`. Its modules are every `.unio` file below it, except in hidden directories, in `target/`, and in nested packages. The pages are:

- `index.html`: the name and version of the package and its modules, each with the first sentence of its documentation comment;
- one page per module, `MODULE.html` (`util.text.html` for `util/text.unio`): the documentation comment of the module (the `##` lines that open the file and do not document the declaration right below them), then each public declaration in the order of the source, with its signature and its documentation comment (L-3). A struct, an enum, or an error shows its public fields or its variants and says when it has private fields; an interface lists its methods. A function also shows its error set, the effects the compiler derives from the call graph (such as `files.read` or `allocates`; L-9b, RFC 0005), the parameters whose calls count for the caller, and the lowest library layer it needs, as `unio symbol` reports them (C-11); every declaration names its file and line, relative to the package. The methods of an `extend` block follow under its header when they are public or implement an interface.

Private declarations and tests are left out. A documentation comment is shown as paragraphs, with its lines indented by four spaces as code and text in backticks as code. The pages are the same bytes for the same sources: they hold no time and no path of the machine. A package with errors gets no pages: its diagnostics are shown, as by `unio check`, with exit code 1.

A worker reads and checks the package with the scopes of `unio check` (SB-10, SB-11; [runtime.md](runtime.md) 9.7) and returns the pages; the tool writes them into `target/doc/` of the package, or into DIR. It does not write through a link: a `target` or `target/doc` that is a link stops it with exit code 1, and a page replaces what was at its path by a new file. It removes only pages of an earlier run that `unio doc` made, of modules that are gone. It prints the path of `index.html`. `--open` then opens it with the program named in `BROWSER`, or with the opener of the system: `xdg-open` on Linux, `open` on macOS, `rundll32.exe url.dll,FileProtocolHandler` on Windows; an opener that cannot be run is reported with exit code 1, and the pages stay. The opener runs outside the sandbox, as a command of the operator. Checked by `compiler/unio/tests/doc.rs` (the pages, the documentation, the signatures, errors and effects, the determinism across runs and directories, the worker, a linked `target`, and the command of `--open`, which the test variable `UNIO_TEST_PRINT_OPEN=1` prints instead of running); the implementation is `compiler/unio/src/doc.rs` and `compiler/unio/src/doc_tool.rs`. The reference of the standard library ([stdlib-reference.md](stdlib-reference.md)) stays a Markdown file generated by `tools/reference.unio` (D-3).

## 10. `unio build --target`

```sh
unio build [--release] --target TRIPLE FILE [-o OUT]
unio build --print-toolchain --target TRIPLE
```

Table 6.1, R-8. The targets are the four platforms of R-8, named by their Rust triples: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, and `x86_64-pc-windows-msvc`; another name is a usage error that lists them. The code generator makes the object file of every target on any machine (`native::tests` checks all four); an executable also needs the runtime library built for the target and a linker with the system libraries of the target. Which pairs of machines work:

| Machine of `unio` | Linux x86-64 | Linux AArch64 | macOS AArch64 | Windows x86-64 |
|---|---|---|---|---|
| Linux x86-64 | yes | yes, cross | no | no |
| Linux AArch64 | yes, cross | yes | no | no |
| macOS AArch64 | no | no | yes | no |
| Windows x86-64 | no | no | no | yes |

- **For itself.** `--target` with the triple of the machine builds what `unio build` builds without it, byte for byte.
- **Linux for the other Linux machine.** The linker is the GNU cross C compiler of the target, `aarch64-linux-gnu-gcc` or `x86_64-linux-gnu-gcc` (the packages `gcc-aarch64-linux-gnu` and `gcc-x86-64-linux-gnu` of Debian and Ubuntu), or the one named in `CC_aarch64_unknown_linux_gnu` or `CC_x86_64_unknown_linux_gnu`. The link step runs it in its sandbox, which may also read the directories where it finds the C library of the target (`-print-search-dirs`). The runtime library of the target is `UNIO_RUNTIME_LIB_AARCH64_UNKNOWN_LINUX_GNU` (or `..._X86_64_UNKNOWN_LINUX_GNU`), else `lib/unio/TRIPLE/libunio.a` of an installation, which is the file `lib/unio/libunio.a` of the release for that platform, else `target/TRIPLE/release/libunio.a` beside the compiler's `target/release/` in a checkout, built with `rustup target add TRIPLE` and `cargo build --release -p unio --lib --target TRIPLE` in `compiler/`. It must come from the same version as the compiler: an executable whose image another compiler made is refused with code 104 when it starts. A missing library, or one that holds code for another machine (read from its first object file), stops the build with a message that says where the library belongs and how to get it. The image records the cross compiler and its version (`aarch64-unknown-linux-gnu; aarch64-linux-gnu-gcc: ...`), and two builds are the same bytes (C-4).
- **Every other pair** stops before anything is compiled or written, with exit code 1 and a message that names the target and the reason: an executable for macOS is linked by Apple's linker with the macOS SDK, which come with the Xcode command line tools on a Mac and are not licensed for other systems; one for Windows is linked by `link.exe` of Visual Studio with the libraries of MSVC and the Windows SDK, which are installed on Windows only; one for Linux needs a GNU C compiler for that machine with its C library, which `unio` uses only on Linux. `--print-toolchain --target` says the same.

Checked by `compiler/unio/tests/cross.rs`: the usage error, the refusal of every pair that does not work on the machine of the test (in the job `compiler` on Linux x86-64, macOS, and Windows, and in `native-linux-arm`), the same bytes with the host's own triple, the messages for a missing runtime library of the target and for one of another machine, and, in the CI steps "Cross build for the other Linux target" of the jobs `compiler` (Linux x86-64 for AArch64) and `native-linux-arm` (Linux AArch64 for x86-64), a build for the other Linux target whose executable is for that machine, records the cross compiler, and is the same in two builds. Started by `qemu-user`, the runtime of the target accepts the image of the program; since `qemu-user` does not pass the system calls of Landlock and seccomp through, the executable then stops before `main` with code 103 and names them (SB-5), so its output under the full sandbox is compared with `unio run` only on the machine itself, as the native suite does for the host's own builds. Whether a cross-built executable gives the output of `unio run` on its own machine is not checked yet: that needs the executable of one job run in the other. The test that links for the other target runs when `UNIO_TEST_CROSS=1` is set, as those steps set it after installing the cross compiler, the runtime library of the target, and the emulator.

## 11. The incremental build and test cache

C-11a, C-11b ([language-contracts.md](language-contracts.md) section 6). `unio build` and `unio test` keep what they made in the build directory of the package, `target/unio-cache/` below the directory of `unio.toml` (SB-11), and reuse it only when its key matches. A key is SHA-384 over the framed fields of every input that decides the result (`compiler/unio/src/cache.rs`, scheme `unio-cache-v1`); a source hash alone is never one:

- **Artifact key** (`artifacts/`, the object file of `unio build`): the compiler (its version and the hash of the sources of the compiler, the code generator, the runtime, and the embedded standard library), the target and the format of its object file, the profile (`--release` or not), the identity of the linker and the SDK recorded in the image, the standard library the program is loaded from, and the texts of every file it is loaded from: its modules, those of its dependencies, the manifests with their permissions, and the lock. Linking always runs, with the runtime library of this compiler.
- **Test key** (`tests/`, the result of one test): the same program inputs, and the content hash of the test (C-11a: the canonical form of the `test` block with the hashes of what it names and calls), the runner and the scheme of property generation (`unio-property-v1`), the seed of `--seed`, the effective permissions and limits the test runs with (the manifest narrowed by the policy), and the place of the file.

Neither binds the time or the environment of the run, and the texts are taken as written: a change of formatting or a comment builds and tests again, since traces name lines. Only a test that passed and read nothing from outside the program is stored: a test that called an external function of the standard library other than those of `std.math`, `std.text`, and `std.json` (files, environment, arguments, clock, randomness, processes) used an input no key records and always runs again, as does a test that failed. `unio test --json` reports for every test its content hash (`hash`, `unio-hash-v1:sha384:...`) and whether its result was reused (`cached`); the text output says `(cached)`. `unio build` says when it reused an object file.

An entry records its scheme, kind, key, the length and the SHA-384 of its payload. One that is incomplete, damaged, or another key's does not match: it is never trusted, the result is made again and replaces it. Entries are written to a temporary file and renamed. The workers never write the cache: the compile step of `unio build` reads it (the package is in its read scope) and leaves the key of a new object for the tool, which stores it; `unio test` reads its entries before the sandbox of the tests applies and appends its results to a journal it opened then (`tests/pending/`), since the sandbox may forbid creating files afterwards; the next `unio test` checks each record and moves it into place. A file outside a package has no build directory and is not cached. The cache is never shared between packages or users and never touches the network; removing `target/` clears it. Checked by `compiler/unio/tests/cache.rs` (reuse, invalidation by a used module, a comment, the manifest, the seed and the policy, damaged, truncated and forged entries, a file outside a package), `the_build_cache_reuses_an_object_only_for_its_key` in `compiler/unio/tests/native.rs`, and the unit tests of `cache.rs`.
