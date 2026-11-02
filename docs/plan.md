# Work plan: Unio 0.1 "Core" and later releases

Version: 0.1 (draft). Basis: `docs/requirements.md`. Russian localization: `docs/ru/plan.md`. Durations are given in weeks from the start of work and assume a team of 3–4 engineers. With a different team the durations scale; the order of stages stays.

## 1. Project principles

- **Specification before code.** A construct enters the compiler only after its syntax and semantics are written in the reference.
- **Vertical slices.** Every stage ends with a working chain "source, check, run", even for a narrow subset of the language.
- **Reference utilities as the yardstick.** The ten utilities from the specification (section 9.1) are written early and grow with the language; a stage is complete when its utilities work.
- **RFCs for language changes.** Any change to syntax or semantics after stage 1 is written up in `docs/rfcs/NNNN-title.md` with rationale, examples, and a migration plan.
- **Measurement from the first stage.** Performance and diagnostic size are tracked in CI; a regression blocks the merge.
- **Unio first** (concept, section 1.1). Everything that does not have to exist before the language runs is written in Unio: the standard library in `stdlib/`, the reference utilities, and the project tools in `tools/`. Until the interpreter of stage 3 runs them, CI uses temporary Python checkers that do the same work; they are deleted at the end of stage 3. The compiler and the runtime stay in Rust until self-hosting in 1.0.

## 2. Repository layout

```
unio/
  README.md
  docs/            concept, requirements, plan, RFCs, reference; docs/ru for Russian
  spec/            grammar (EBNF) and specification test programs
  compiler/        Rust: lexer, parser, sema, ir, codegen, driver
  runtime/         GC, resources, faults, startup
  stdlib/          standard library in Unio plus external functions
  tools/           fmt, test, pkg, lsp, doc
  examples/        reference utilities
  bench/           performance tests
  editors/vscode/  extension
```

## 3. Stages of release 0.1

### Stage 0. Specification and RFCs (weeks 1–4)

Goal: close every question in the concept and produce a reference from which the compiler can be written.

Work:
- Core grammar in EBNF; grammar tests (a corpus of valid and invalid files).
- Semantics of types, `Option`, `fails with`, `try`, faults, `using`, modules in the reference.
- Operator interfaces, effect derivation, and `std.decimal` described in the reference (decisions from the concept, section 10).
- JSON diagnostics schema (JSON Schema), list of the first 50 error codes.
- Sandbox prototypes on Linux, macOS, and Windows: a program that tries to read a file outside its permissions is denied on each platform.
- All ten reference utilities written on paper and checked for readability by hand.

Result: `spec/` and `docs/reference/` at version 0.1-draft; accepted RFCs.

Exit criterion: three people independently parse `todo.unio` by the grammar and obtain the same tree.

### Stage 1. Front end and formatter (weeks 4–10)

Goal: any core file parses to an AST, is formatted, and round-trips unchanged.

Work:
- Indentation-aware lexer; parser with error recovery.
- AST with full spans; diagnostics in text and JSON (C-6 to C-8).
- `unio fmt` with the "AST before = AST after" test and idempotence.
- `unio outline`.
- Parser fuzzing in CI.

Exit criterion: the `spec/` corpus and the ten utilities parse; `fmt` is idempotent; the parser survives 8 hours of fuzzing.

### Stage 2. Semantics (weeks 8–18)

Goal: `unio check` verifies types, errors, and resources.

Work:
- Name resolution, modules, visibility, import cycles.
- Type checking and inference for local values; generics with constraints.
- `match` exhaustiveness.
- `fails with` analysis: propagation through `try`, handling through `on`, completeness.
- Resource checking: `using`, no escape of a resource into ordinary structs.
- Mutability checking (`let`/`var`, `var` parameters).
- `unio symbol` and `unio explain` for every code of the stage.

Exit criterion: the corpus of 200 programs with errors (R-10) yields the expected diagnostics; the ten utilities pass `check`.

### Stage 3. Interpreter and runtime (weeks 14–24)

Goal: core programs run, even if slowly; semantics are locked by tests before native code exists.

Work:
- Intermediate representation (IR) with explicit errors, resources, and fault points.
- IR interpreter for fast semantic validation and for `unio test` in the early stages.
- Runtime: precise GC, `Resource` and `using`, faults with trace and exit codes; the termination model of specification section 4.5 with a memory reserve for `release` and a preallocated diagnostic for code 102.
- Atomic writes in `std.files` (S-9) and tests that kill the process and exhaust memory during writing.
- Minimal standard library: `text`, `collections`, `files`, `path`, `env`, `args`, `json`.
- `unio test` with text and JSON output, `expect_fault`.

Exit criterion: `todo`, `wc`, `grep-lite`, `json-query` run in the interpreter and their tests pass; the tools in `tools/` run in the interpreter, give the same results as the Python checkers, and replace them in CI; the `test` blocks of the standard library pass in the interpreter, including the end-to-end test of the `commit` protocol (a failed directory sync followed by a successful second commit); reliability tests for faults inside `using` are green.

### Stage 4. Native build (weeks 20–32)

Goal: `unio build` produces a single executable on four platforms.

Work:
- Code generation from IR through Cranelift; linking with the runtime.
- Stack maps for the precise GC; stack traces without external debug formats.
- Deterministic build (C-4), verified in CI by hash comparison.
- Cross-compilation to the four targets; CI on all of them.
- Benchmarks of specification section 7.1 in CI with thresholds.
- Sandbox (specification section 4.6) on four platforms: Landlock and seccomp-bpf, Seatbelt, AppContainer with a Job Object; the external policy and its limits; the SB-8 suite of forbidden actions in CI. Isolated workers for `check`, `build`, `fmt`, and `doc` (SB-10, SB-11) and the hostile-project suite SB-15. Power-loss tests of atomic writes (S-9b).

Exit criterion: all ten utilities build natively on four platforms and run in the sandbox with the permissions from their manifests; the SB-8 suite is green on all platforms; the 7.1 metrics are met, or deviations are recorded with a remediation plan.

### Stage 5. Standard library and packages (weeks 28–38)

Goal: the full module set of the specification and reproducible dependencies.

Work:
- Modules `process`, `time`, `random`, `log`, `math`, `decimal`, `test`; documentation and examples for every public function.
- Review and sanitizers for all `external function` code.
- `unio.toml`, `unio.lock`, scoped permissions (T-6), `unsafe module`.
- Registry: signed publishing, verification, no deletion; `add`, `remove`, `update`. Download and unpacking in separate workers (SB-12).
- Documentation generation with `unio doc`.

Exit criterion: all ten utilities use only the standard library and build offline from the lock file; the registry accepts and serves a test package.

### Stage 6. Editor and AI workflow (weeks 34–42)

Goal: the language is comfortable in an editor and in an AI agent's loop.

Work:
- Language server: diagnostics, go to definition, hints, completion, rename, formatting. Analysis runs in an isolated worker (SB-13).
- VS Code extension.
- The document "How Unio works with AI", agent scenarios, the R-10 trial with published methodology and results.
- Diagnostic message refinement from trial results.

Exit criterion: the R-10 trial reaches at least 80%; the LSP passes the protocol test suite.

### Stage 7. Stabilization and release 0.1 (weeks 40–48)

Work:
- 24-hour compiler fuzzing; defect fixes.
- Readability trial (1.4, item 2) with external participants.
- Reproducible release build, signatures, SBOM, installers for four platforms.
- Reference, guide, library documentation in both languages.
- Acceptance per specification section 9.2.

Result: Unio 0.1, public repository, package registry, documentation site.

## 4. Stage summary

| Stage | Weeks | Main result | Exit criterion |
|---|---|---|---|
| 0 | 1–4 | Reference, grammar, RFCs, diagnostics schema | Independent parses of `todo.unio` agree |
| 1 | 4–10 | Parser, `fmt`, `outline`, JSON diagnostics | Corpus parses, `fmt` idempotent |
| 2 | 8–18 | `check`: types, errors, resources, `symbol`, `explain` | 200 error programs yield expected diagnostics |
| 3 | 14–24 | IR, interpreter, GC, resources, `test`, minimal library | Four utilities run, fault tests green |
| 4 | 20–32 | Native build on 4 platforms, benchmarks | Ten utilities build, 7.1 metrics |
| 5 | 28–38 | Full library, packages, registry, `doc` | Offline build, registry works |
| 6 | 34–42 | LSP, VS Code, AI trial | R-10 ≥ 80% |
| 7 | 40–48 | Release 0.1 | Acceptance per 9.2 |

Stages overlap: the front end and semantics proceed in parallel with specification of later parts; the library starts on the interpreter before the native build.

## 5. Roles

| Role | Responsibility |
|---|---|
| Language lead | Specification, RFCs, semantics, final syntax decisions |
| Compiler engineer | Front end, checks, IR, code generation |
| Runtime engineer | GC, resources, faults, platforms, benchmarks |
| Tools and library engineer | `fmt`, `test`, packages, registry, LSP, standard library |
| Technical writer (from stage 5) | Reference, guide, AI document |

## 6. Risks and mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Resource checking (escape from `using`) proves too strict or too weak | medium | semantics rework after stage 2 | Prototype the analysis on paper in stage 0; resource test corpus before implementation |
| Precise GC with Cranelift takes longer than planned | high | stage 4 slips | Start with conservative stack scanning as a fallback; move to stack maps if time allows |
| Diagnostics prove awkward for AI; the 80% goal is missed | medium | extra refinement cycle | Run the R-10 trial from stage 2 on a partial corpus instead of waiting for stage 6 |
| Text and JSON performance below CPython | medium | 7.1 violated | Benchmarks in CI from stage 3; optimize `Text` (UTF-8 with cached length) before stage 4 |
| Language vocabulary grows under feature pressure | high | loss of readability, the main property | RFC process with a mandatory section "why existing constructs cannot express this" |
| Windows support lags | medium | R-8 violated | Windows CI from stage 1; paths through `Path`, no assumptions about separators |
| Sandbox mechanisms differ between platforms or are missing on older systems | high | SB-1 holds unevenly; stage 4 slips | Prototype all three mechanisms in stage 0–1; one SB-8 suite for all platforms; fail closed with code 103 instead of a silently weaker sandbox |
| Termination guarantees cannot be tested reliably | medium | R-2 unverified | Guarantees limited to atomic writes; tests that kill the process at random points of a write |
| File systems differ in what survives power loss | medium | S-9 holds unevenly | Guarantees only for named file systems with per-platform sync calls; `durable` fails elsewhere; replay of recorded block writes on Linux, power-off tests in virtual machines |
| Isolating the compiler slows the edit-check loop | medium | 7.1 `check` time violated | Long-lived workers reused between runs of the language server; the cost of starting a worker is measured in the performance suite |

## 7. Project metrics

Tracked in CI and published with every release:

- `check` and `build` times for the reference utilities;
- the metrics of specification section 7.1;
- share of R-10 corpus programs fixed by an agent;
- number of diagnostic codes with explanation and `fixes`; target 100% for errors;
- language vocabulary size: number of keywords and constructs (target: no growth after 0.1 without an RFC).

## 8. After release 0.1

| Release | Contents | Dependencies on 0.1 |
|---|---|---|
| 0.2 "Server" | Task-based concurrency without shared mutable state, `std.net`, HTTP client and server, CPU and RAM profiler | No shared mutable state in the core; profiler event format |
| 0.3 "Compute" | Tensors as resources, lifetime scopes with budgets, autodiff, CPU/GPU backends, buffer planning, VRAM profiling, device-completion-aware release | `Resource`, `using`, generics, effects in signatures |
| 0.4 "Quantum" | Circuits as values, simulator, hardware interface with capability description | Backend interface from 0.3 |
| 1.0 | Language stabilization, compatibility guarantee, self-hosted compiler in Unio | All of the above |

Each later release starts with its own specification modeled on `docs/requirements.md` and does not change core syntax without an RFC and an automatic migration.

## 9. Immediate actions

1. Concept 0.4 closes the review findings: mandatory sandbox, one rule for calls with `fails with`, termination model. Set up the RFC process in `docs/rfcs/` for future changes. Concept 0.5 and requirements 0.4 close the second review: guarantees of file writes after power loss and isolated tools.
2. Fix the EBNF grammar (`spec/grammar.ebnf`) and a small set of reference programs (`spec/tests/`) checked against it automatically.
3. Create the repository layout of section 2 and CI with document formatting checks and the grammar check.
4. Write the remaining reference utilities in Unio following `docs/examples/todo.unio`.
