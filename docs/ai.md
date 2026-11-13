# How Unio works with AI

Version 0.1-draft. Russian localization: [ru/ai.md](ru/ai.md). Requirement D-4: the JSON formats, the recommended loop, and prompt examples. Basis: concept section 7, requirements C-8 to C-12, T-4 and R-10.

Unio is meant to be written by people and by AI agents, and read by people. Three properties of the language serve an agent directly. There is one way to write each thing and one style that `unio fmt` enforces, so generated code has no dialects. What can go wrong is in the signatures: errors in `fails with`, effects derived from calls and checked against the manifest, resources tied to `using`, so a mistake shows up in `unio check` instead of at run time. And every tool that reports something reports it as JSON with stable codes, so an agent works from data, not from parsed prose.

## 1. Measured result

The control trial of requirement R-10 gives an agent nothing but a faulty program and its JSON diagnostics, and counts a program as fixed only when `unio check` is clean and hidden behavioral tests pass. Its run of 2026-11-12 ([bench/results/control_trial-2026-11-12.md](../bench/results/control_trial-2026-11-12.md); protocol in [plan.md](plan.md) section 12, closure condition 2):

| Category | Programs | Fixed after attempt 1 | Fixed within 3 attempts |
|---|---|---|---|
| types | 54 | 42 (77.8%) | 52 (96.3%) |
| errors | 50 | 37 (74.0%) | 46 (92.0%) |
| resources | 49 | 38 (77.6%) | 48 (98.0%) |
| style | 47 | 44 (93.6%) | 46 (97.9%) |
| total | 200 | 161 (80.5%) | 192 (96.0%) |

The criterion, at least 160 of 200 within three attempts, is met. The agents were fresh Claude agents (`claude-opus-5-5`), one per program and attempt, that could only read their prompt file and write their answer file: each received the instruction, the source and the diagnostics, and answered with the corrected source; the reference fixes and hidden tests were out of the working tree during the trial. Two lessons from it shape the loop below. First, a clean `unio check` is not a fix: six of the eight programs not fixed checked clean but failed their hidden tests, so the loop always ends with tests. Second, the trial found a gap in the compiler (`unio check` accepted `files.read = true` in a manifest), now reported as E0088. 80% is the criterion of this control set; it does not show the same success on arbitrary programs.

## 2. JSON formats

Codes, field names and the shape of the JSON do not depend on the language of messages (C-10): `--lang=ru` or `UNIO_LANG=ru` changes only `message`, `explanation`, fix descriptions and the messages of related locations. Paths are as given on the command line, lines and columns count from 1, and columns count characters.

The examples are the output of the commands named above each of them, run on the files of [compiler/unio/tests/data/ai/](../compiler/unio/tests/data/ai); `tests/ai_examples.rs` runs them and compares the output with these blocks and with the Russian version, so the examples cannot drift from the compiler. Only paths are shortened to that directory and the repository, hashes end in `...`, and `time_ms` may differ.

### 2.1 Diagnostics: `unio check --diagnostics=json FILE...`

An array of diagnostics, empty when the files are clean; the exit code is 0 when the array is empty and 1 otherwise. `unio check` reports every error it finds in one run (C-2), syntax errors first: name and type checks run only on files that parse.

<!-- example: check --diagnostics=json total.unio -->
```json
[
  {"code":"E0056","severity":"error","message":"unknown name 'summ'","explanation":"The name is not declared where it is used. Check the spelling; the fix suggests the closest visible name.","spans":[{"file":"total.unio","start":{"line":5,"column":15,"offset":130},"end":{"line":5,"column":19,"offset":134},"label":null}],"fixes":[{"file":"total.unio","range":{"start":{"line":5,"column":15,"offset":130},"end":{"line":5,"column":19,"offset":134}},"new_text":"sum","description":"replace with 'sum'"}],"related":[]},
  {"code":"E0051","severity":"error","message":"'total' is declared twice; rename one of them","explanation":"A module declares each name once, and a type has one method of each name. Rename or remove one of the declarations.","spans":[{"file":"total.unio","start":{"line":8,"column":10,"offset":168},"end":{"line":8,"column":15,"offset":173},"label":null}],"fixes":[],"related":[{"symbol":"total.total","message":"'total' is first declared here","span":{"file":"total.unio","start":{"line":2,"column":10,"offset":35},"end":{"line":2,"column":15,"offset":40},"label":null}}]}
]
```

| Field | Meaning |
|---|---|
| `code` | the stable code, `E` and four digits; the [language reference](reference.md) section 8 lists them all |
| `severity` | `error` in 0.1 |
| `message` | what is wrong, at most 200 characters (R-11) |
| `explanation` | how to fix it; `unio explain CODE` adds an example and its fix |
| `spans` | where: `file`, `start` and `end` with `line`, `column` and byte `offset`, and an optional `label` |
| `fixes` | replacements, present when the fix is unambiguous: replace `range` of `file` by `new_text`; `description` says what it does |
| `related` | other locations that matter, each with `message`, `span` (as in `spans`, `label` null) and `symbol`, the full name when the location is a declaration, else null: the earlier declaration of a duplicate, the declaration of a type in a mismatch or of the function a call does not fit, the `fails with` list of a function called without `try`, the handlers and the signature for an unhandled error, the binding of a name that cannot change, the manifest line for E0085, E0088 and E0089. Locations in the standard library are left out, except for a layer violation in it (E0084); `unio symbol` shows them. The text form shows each as `= note: FILE:LINE:COLUMN: MESSAGE` |

The format is versioned with the compiler, and its JSON Schema is published as [compiler/diagnostics.schema.json](../compiler/diagnostics.schema.json) (`urn:unio:diagnostics:1`); `tests/diagnostics.rs` checks the output against it (C-8).

### 2.2 Tests: `unio test --json FILE...`

An array with one object per `test` block; the exit code is 0 when every test passed and 1 otherwise. A test that does not compile fails alone with its diagnostics in `message`, and the other tests still run (T-5a).

<!-- example: test --json double.unio -->
```json
[{"file": "double.unio", "name": "double of two", "status": "failed", "time_ms": 0, "output": "", "message": "fault: assertion failed: double(2) == 5\n    at test \"double of two\" (double.unio:5)\n", "properties": [], "hash": "unio-hash-v1:sha384:...", "cached": false},
 {"file": "double.unio", "name": "uses a missing name", "status": "failed", "time_ms": 0, "output": "", "message": "the test does not compile:\n8:E0056: unknown name 'duble'\n", "properties": [], "hash": "unio-hash-v1:sha384:...", "cached": false}]
```

`status` is `passed` or `failed`; `output` is what the test printed; `message` is the fault with its trace, or the diagnostics that keep the test from compiling; `properties` holds the replay records of the property tests ([tools.md](tools.md)); `hash` is the key of the test result and `cached` says whether the result was reused. A filter after the options runs only the tests whose names contain it: `unio test --json "parse_command" todo.unio`.

### 2.3 Outline: `unio outline --json FILE`

What a file declares, without bodies: an array of declarations, each with `kind` (`function`, `struct`, `enum`, `error`, `interface`, `constant`, `extend` or `test`), `name`, `signature` on one line, `line`, and `members` in the same shape: the fields (`field`) of a struct or error, the variants (`variant`) of an enum, the methods (`method`) of an interface or an `extend` block. It is the cheap way to see a file before reading it.

<!-- example: outline --json double.unio -->
```json
[
  {"kind": "function", "name": "double", "signature": "function double(n as Int) returns Int", "line": 1, "members": []},
  {"kind": "test", "name": "double of two", "signature": "test \"double of two\"", "line": 4, "members": []},
  {"kind": "test", "name": "uses a missing name", "signature": "test \"uses a missing name\"", "line": 7, "members": []}
]
```

### 2.4 Symbols: `unio symbol --json MODULE.NAME`

One declaration with what it means for a caller (C-11): `name`, `kind`, `signature`, `documentation` (its `##` comment), `declared` (file, line and column), `usages` in the program, and for a function `errors` (its `fails with`), `effects` derived from its calls (`files.read`, `files.write`, `env`, `process`, `network`, `allocates`), `argument_effects` (parameters whose effects are the caller's), `layer` (the lowest library layer it needs), `calls`, `hash`, a content hash of the declaration and what it depends on, `default_of` and `implementations`. A method declared in an interface shows only its own signature. For a type, `implementations` lists the interfaces it implements in `extend ... with` blocks (L-6c), each with `interface`, `declared` (the `extend` block), and `methods`: for every method of the interface, the `function` the type uses, where it is `declared`, and `default`, true when the type uses the default of the interface instead of replacing it. A method that a type gets from a default is found through the type: `unio symbol m.Square.name` reports the default under that name, `declared` at the interface, with `default_of` naming it (`m.Shape.name`); `default_of` is empty for every other declaration, and `implementations` is empty for everything but a type. Names of the standard library start with `std.`; names of the program are relative to its package, run from its directory.

<!-- example: symbol --json std.files.write_text -->
```json
[{"name":"std.files.write_text","kind":"function","signature":"function write_text(path as Path, text as Text, durable as Bool = false) fails with IoError","documentation":"Replaces the file with the text atomically (guarantees 1 and 2); durable = true adds guarantee 3. Permission: files.write.\nExample:\n    try:\n        try write_text(Path(\"no-such-directory/report.txt\"), \"done\\n\")\n    on IoError as e:\n        assert e.kind == IoErrorKind.NotFound or e.kind == IoErrorKind.PermissionDenied","declared":"stdlib/std/files.unio:391:17","default_of":"","usages":[],"errors":["IoError"],"effects":["files.write","allocates"],"argument_effects":[],"layer":"std","calls":["std.files.AtomicFile.commit","std.files.AtomicFile.write","std.files.open_atomic"],"implementations":[],"hash":"unio-hash-v1:sha384:..."}]
```

A type `Square` that implements an interface `Shape`, replacing `area` and using the default `name`; the text form shows the same as `implements: m.Shape` with one line per method:

<!-- example: symbol --json m.Square -->
```json
[{"name":"m.Square","kind":"type","signature":"public struct Square:","documentation":"A square with a side of a whole number of units.","declared":"m.unio:9:15","default_of":"","usages":["m.unio:13:14","m.unio:17:17"],"errors":[],"effects":[],"argument_effects":[],"layer":"core","calls":[],"implementations":[{"interface":"m.Shape","declared":"m.unio:12:1","methods":[{"name":"area","function":"m.Square.area","declared":"m.unio:13:14","default":false},{"name":"name","function":"m.Shape.name","declared":"m.unio:5:14","default":true}]}],"hash":"unio-hash-v1:sha384:..."}]
```

### 2.5 Explanations: `unio explain CODE`

Text, not JSON: the explanation of the code, an example that produces it, and the example fixed. The `explanation` field of a diagnostic is its first paragraph, so an agent needs `unio explain` only when the explanation and the fix are not enough.

## 3. The recommended loop

1. **Check.** Run `unio check --diagnostics=json` on every file the change touches, or on the program's entry module, which checks what it imports. Fix syntax errors first: until a file parses, names and types are not checked.
2. **Read each diagnostic.** Use `code`, the span and `message`. When `fixes` is not empty, apply the replacement exactly: it is offered only when it is unambiguous. Otherwise read `explanation`, and `unio explain CODE` when that is not enough. Look up an unknown API with `unio symbol --json std.MODULE.NAME` or in the [standard library reference](stdlib-reference.md), not from memory of other languages.
3. **Change only what the error needs.** Do not delete the code that is wrong, comment it out, or weaken a check. The usual traps are known: E0073 and E0074 ask for `try` or a handler, not for removing `fails with`; E0085 means the program needs a permission its manifest does not grant, which is a decision for the person who owns the manifest, not a line to add silently; E0077 and E0078 ask for `var` where a value must change, or for changing an element through its collection.
4. **Check again** until the array is empty. One mistake gives one diagnostic: a value whose type is unknown because of an earlier error produces no further messages ([semantics.md](semantics.md) 2.8), so each diagnostic is a mistake to fix.
5. **Format.** `unio fmt` the changed files; the formatter does not change meaning, and CI rejects a file that is not in the one style.
6. **Test.** `unio test --json` on the changed modules. Read `message` of each failed test: a fault names its condition and trace, a test that does not compile lists its diagnostics. A clean check is not the end: in the R-10 trial most of the programs that were not fixed checked clean and failed their tests.
7. **Run, when the change is a program.** `unio run FILE -- ARGS` runs it in the sandbox of its manifest; exit code 101 is a fault, 102 exhaustion of memory or time, 103 a policy that cannot be met.

An agent can stop at three iterations of steps 1 to 4 without progress and report the remaining diagnostics, as the trial did.

## 4. Prompt examples

Prompts work best when they give the agent the tools of the loop, the files it may change, and the criterion of done. The instruction of the R-10 trial, which had no tools, is a model for a single fix:

> You are given a program in the Unio language and the diagnostics that `unio check --diagnostics=json` reports for it. Fix the program so that `unio check` reports no error and the program does what its code and comments say it should do. Change only what the error needs; do not delete the code that is wrong or work around the check. Answer only with the full corrected source.

An agent with tools, adding a feature:

> Add a command `todo remove N` to `docs/examples/todo.unio` that deletes the task with number N and fails with `UsageError` when there is none. Write its `test` blocks first. Loop: `unio check --diagnostics=json docs/examples/todo.unio`, fix every diagnostic (apply `fixes` when present, use `unio explain CODE` otherwise), then `unio fmt` and `unio test --json docs/examples/todo.unio`. Done means an empty diagnostic array and every test passed. Do not change `unio.toml`.

Learning an API before writing code:

> Before writing the function, run `unio symbol --json std.files.write_text` and `unio symbol --json std.json.decode` and use their signatures, errors and effects. Every function that calls them must list their errors in `fails with` or handle them with `try ... on`.

A review:

> Run `unio check --diagnostics=json` and `unio test --json` on the changed files and report each diagnostic and failed test with its code or message. Then list every function whose `unio symbol` effects grew with this change, and say whether the manifest permits them.

Prompts to avoid ask the agent to "make the errors go away": an agent can always delete the code that is wrong, and the trial counted such answers as failures for that reason.

## 5. Limits in 0.1

- Warnings (`W`) and style hints (`H`) are defined by C-6 but none exists: every diagnostic is an error.
- `unio test --json` has no replay data for property tests yet (T-12), and the REPL of T-13 does not exist.
- The language server `unio lsp` gives the same diagnostics to an editor as LSP messages ([editors/README.md](../editors/README.md)).
