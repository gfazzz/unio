# Work plan: Unio 0.1 "Core" and later releases

Version: 0.1 (draft). Basis: `docs/requirements.md`. Russian localization: `docs/ru/plan.md`. Durations are given in weeks from the start of work and assume a team of 3–4 engineers. With a different team the durations scale; the order of stages stays.

Changes from the initial 0.1 draft: UniOS requires human- and AI-accessible administration and an integrated base system (section 9; `docs/os.md`, sections 1.1–1.2).

Changes from the initial 0.1 draft: post-quantum signing is required for 0.1 distribution; network and UniOS profiles follow `docs/cryptography.md` (requirements T-8a, R-6a to R-6c, OS-7).

Changes from the initial 0.1 draft: the status of the sprints in section 12 follows the requirement-level table of `docs/status.md`, lists the gaps found by the audit in the sprints they belong to, and records the order of the remaining work.

Changes from the initial 0.1 draft: section 12 states the closure conditions of the memory reserve for `release` (C-17) and of the R-10 control set before release 0.1: what must exist, the tests and CI steps that check it, and the platforms.

Changes from the initial 0.1 draft: decisions of the project lead recorded in section 12: RFC 0005 is accepted (status of S4); the order of the remaining work puts the read and execute rights of children together with a child leaving its process group next, then SB-10 to SB-13 with SB-15; closure condition 1 states the C-17/C-18 split, that the reserve is available before the fault and its trace are built, the 0.1 parameters as an engineering budget, and the measurements that close it; closure condition 2 states how the 200 programs are chosen and checked and fixes the protocol of the model trial.

## 1. Project principles

- **Specification before code.** A construct enters the compiler only after its syntax and semantics are written in the reference.
- **Vertical slices.** Every stage ends with a working chain "source, check, run", even for a narrow subset of the language.
- **Reference utilities as the yardstick.** The ten utilities from the specification (section 9.1) are written early and grow with the language; a stage is complete when its utilities work.
- **RFCs for language changes.** Any change to syntax or semantics after stage 1 is written up in `docs/rfcs/NNNN-title.md` with rationale, examples, and a migration plan.
- **Measurement from the first stage.** Performance and diagnostic size are tracked in CI; a regression blocks the merge.
- **Unio first** (concept, section 1.1). Everything that does not have to exist before the language runs is written in Unio: the standard library in `stdlib/`, the reference utilities, and the project tools in `tools/`. Since the interpreter of S5 runs them, CI uses the Unio tools; the Python checkers stay as a reference for parity and performance comparisons, next to equivalent programs in other languages in `bench/`. The compiler and the runtime stay in Rust until self-hosting in 1.0.
- **Tests first** (concept, section 1.2). Every task starts with a check that fails: corpus files for the grammar, a program of the control set for a diagnostic, `test` blocks for library and tool code, a reproducing test for a defect. A task is done when that check passes and all others still do.

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
  unios/           UniOS kernel, services, and the project site (track of section 9, after 0.5)
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
- Derived effect `allocates` and the lowest needed layer for every function; `unio check --profile=core` with call chains (C-21).

Exit criterion: the corpus of 200 programs with errors (R-10) yields the expected diagnostics; the ten utilities pass `check`.

### Stage 3. Interpreter and runtime (weeks 14–24)

Goal: core programs run, even if slowly; semantics are locked by tests before native code exists.

Work:
- Intermediate representation (IR) with explicit errors, resources, and fault points.
- IR interpreter for fast semantic validation and for `unio test` in the early stages.
- Runtime: precise GC, `Resource` and `using`, faults with trace and exit codes; the termination model of specification section 4.5 with a memory reserve for `release` and a preallocated diagnostic for code 102. The runtime is split into the `core`, `alloc`, and `std` parts of S-12 from the start.
- Atomic writes in `std.files` (S-9) and tests that kill the process and exhaust memory during writing.
- Minimal standard library: `text`, `collections`, `files`, `path`, `env`, `args`, `json`.
- `unio test` with text and JSON output, `expect_fault`.

Exit criterion: `todo`, `wc`, `grep-lite`, `json-query` run in the interpreter and their tests pass; the tools in `tools/` run in the interpreter, give the same results as the Python checkers, and replace them in CI, while the Python checkers stay as a reference for parity and performance; the `test` blocks of the standard library pass in the interpreter, including the end-to-end test of the `commit` protocol (a failed directory sync followed by a successful second commit); reliability tests for faults inside `using` are green.

### Stage 4. Native build (weeks 20–32)

Goal: `unio build` produces a single executable on four platforms.

Work:
- Code generation from IR through Cranelift; linking with the runtime.
- Stack maps for the precise GC; stack traces without external debug formats.
- Deterministic build (C-4), verified in CI by hash comparison.
- Cross-compilation to the four targets; CI on all of them.
- System prototype: a freestanding x86-64 binary built from a `core`-only Unio program through Cranelift, started by an existing bootloader in a virtual machine, writing to the serial port. It checks calling conventions, sections, and the binary format needed by the system profile; the result goes into the RFC for that profile.
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
- Post-quantum release verification, independently provisioned trust roots, rotation/revocation and rollback tests before publication (R-6a to R-6c). Stage 5 package verification follows T-8a; bootstrap signing tools may be external under concept section 1.1.
- Reference, guide, library documentation in both languages. The guide includes the chapter on moving from classes (requirement D-2).
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
| Cranelift lacks what system code needs (calling conventions, sections, interrupt entry) | medium | the system profile needs a second backend | Freestanding prototype in stage 4; LLVM is already considered as a second backend (requirements, section 10) |
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
| 0.2 "Server" | Task-based concurrency without shared mutable state, an explicit type for shared mutable values (object graphs with identity, concept rationale 17), `std.net`, HTTP client and server, `std.crypto` with constant-time primitives and a secure random source, TLS 1.3, CPU and RAM profiler | No shared mutable state in the core; profiler event format |
| 0.3 "Compute" | Tensors as resources, lifetime scopes with budgets, autodiff, CPU/GPU backends, buffer planning, VRAM profiling, device-completion-aware release | `Resource`, `using`, generics, effects in signatures |
| 0.4 "Quantum" | Circuits as values, simulator, hardware interface with capability description | Backend interface from 0.3 |
| 0.5 "System" | The `system` profile: memory without a GC and code without copy-on-write, control of representation, pointers, memory-mapped devices and assembly in `unsafe module`, interrupts, atomic operations and a memory model, checked constraints on effects; the prototype kernel of concept section 8.1 in a virtual machine | Layers `core`, `alloc`, `std`; derived effects; the memory model must agree with the concurrency of 0.2 |
| 1.0 | Language stabilization, compatibility guarantee, self-hosted compiler in Unio | All of the above |
| UniOS (separate track, with 0.5) | The operating system written in Unio and the proof of concept: the project site, in Unio, served on UniOS on a VM over HTTPS. See section 9 and `docs/os.md` | Native code (stage 4), concurrency and `std.net` from 0.2, the `system` profile and prototype kernel from 0.5 |

Each later release starts with its own specification modeled on `docs/requirements.md` and does not change core syntax without an RFC and an automatic migration.

## 9. UniOS track

UniOS is the operating system written in Unio. Concept section 8.1 keeps a general-purpose operating system off the language roadmap, so UniOS is a separate project with its own specification and team. Its first deliverable is the proof of concept: the project site, written in Unio, served by a web server and framework in Unio, on UniOS, on a VM of the class 8 GB RAM / 4 cores / 80 GB, reachable over HTTPS. Design, architecture, and risks are in `docs/os.md`; this section fixes the track's place in the schedule and what the language owes it.

The OS inherits the language's requirements for human and AI understanding, explicit behavior, and controlled resources. Its base system follows the FreeBSD approach: kernel, libraries, essential services, and administrative tools are developed and released together, with a documented inventory and common conventions. Additional applications extend that base. Local and remote management, including SSH, share documented CLI/API contracts, structured diagnostics, and permission checks. These are baseline requirements of `docs/os.md`, sections 1.1–1.2, with acceptance scenarios; model serving is a later capability.

### 9.1 Schedule

The track starts with release 0.5 "System": the prototype kernel of that release (bootloader, entry into Unio code, serial output, page management, timer, scheduler, user process, system call) is the starting point of stage U-1. Weeks count from the start of the track; the team of 2–3 engineers is separate from the language team.

| Stage | Weeks | Result | Exit criterion |
|---|---|---|---|
| U-0 Design | 1–6, during 0.5 | UniOS specification, virtio device model, capabilities mapped from manifest permissions, system-call interface, service protocol | Specification accepted; the 0.5 prototype kernel boots under QEMU with the UniOS system-call table |
| U-1 Kernel | 6–16 | Virtio transport, interrupts, memory regions for processes, process creation and capabilities, serial console, driver processes | A driver process receives interrupts and reads a virtio device; memory and timer tests green |
| U-2 Network and site | 14–28 | virtio-net driver, TCP/IP and DNS service, `std` over UniOS system calls, HTTP/1.1 server, web framework, site over HTTP on a local VM | Site serves from a local VM; 24 h of network fuzzing survived |
| U-3 Storage, TLS, deployment | 26–44 | virtio-blk driver, file store with the S-9 guarantees, TLS 1.3 subset on `std.crypto`, ACME client for the certificate, preemptive scheduling on 4 cores, deployment on a VDS | Site reachable over HTTPS for 30 days with recorded uptime |
| U-4 Beyond the proof of concept | separate project | Shell, package installation, accelerator and QPU device models, model-serving service | Own specification, outside the language roadmap |

The proof of concept (U-0 to U-3) is the UniOS 0.1 milestone and is announced on the site it serves. U-0 fixes the base-system inventory per release and the administration acceptance scenarios. U-2 must pass the local scenarios for site services. Before public deployment, U-3 must also pass them over authenticated SSH with a standard client, including permission denial and recovery after a disconnected request, and demonstrate the site's update and recovery path. These requirements apply to both the CLI and agent workflows described in `docs/os.md`. The minimal management CLI precedes the general-purpose shell and application package manager in U-4. The draft estimates must be reassessed in U-0 against this scope.

### 9.2 What the language owes the track

Already planned: the `core`, `alloc`, and `std` layers and `unio check --profile=core` in 0.1; task-based concurrency and `std.net` as interfaces without POSIX assumptions in 0.2; the `system` profile, the memory model, and the freestanding prototype in stage 4 and release 0.5. The following consequences are recorded here so that they are not lost:

- `std` has one interface and two backends, a host operating system and UniOS; the specification test corpus and the reference utilities must run on both.
- An HTTPS server in Unio needs cryptography, which the roadmap did not contain: `std.crypto` (hashes, authenticated encryption, key exchange, signatures for certificates) with constant-time primitives in reviewed `unsafe module` files (S-7, R-5) that allocate nothing and do not touch the GC, and a cryptographically secure random source, kept separate from the fast generator of `std.random`. This is new work for release 0.2, where the HTTP server is, and goes through an RFC.
- The networking RFC applies `docs/cryptography.md`: ML-KEM hybrid key establishment, post-quantum identity authentication, versioned algorithm profiles, and explicit compatibility endpoints. UniOS U-0 defines trust enrollment and recovery; U-3 must pass OS-7 for transport and image/update signatures. A public HTTPS/ACME compatibility endpoint cannot establish strict-profile readiness. Account for key, signature, and message sizes in the resource estimates.
- The sandbox requirements (SB-1 onward) are stated so that a kernel capability model satisfies them without a host sandbox.

Any further need the track discovers goes to the language as an RFC.

### 9.3 Risks specific to the track

| Risk | Likelihood | Mitigation |
|---|---|---|
| UniOS pulls engineers from the language before 0.5 is stable | high | Separate team; U-1 starts only when the 0.5 prototype kernel boots |
| TLS written from scratch has protocol defects | medium | TLS 1.3 with explicitly selected profiles under `docs/cryptography.md`, RFC test vectors, interoperability with three existing clients, external review before public exposure |
| `std` over UniOS diverges from `std` over Linux | medium | One interface, two backends, one test corpus |
| "AI in the kernel" remains a slogan | medium | U-4 ships a measurable model-serving service with memory and latency visible in the profiler |
| Expectation that UniOS runs on quantum hardware | medium | A QPU is a coprocessor device model; stated plainly in `docs/os.md` |
| Timing side channels in cryptography written in Unio | medium | Constant-time primitives in reviewed `unsafe module` files without allocation or GC; statistical timing tests in CI; external review before public exposure |

## 10. Immediate actions

1. Concept 0.4 closes the review findings: mandatory sandbox, one rule for calls with `fails with`, termination model. Set up the RFC process in `docs/rfcs/` for future changes. Concept 0.5 and requirements 0.4 close the second review: guarantees of file writes after power loss and isolated tools.
2. Fix the EBNF grammar (`spec/grammar.ebnf`) and a small set of reference programs (`spec/tests/`) checked against it automatically.
3. Create the repository layout of section 2 and CI with document formatting checks and the grammar check.
4. The ten reference utilities of requirements section 9.1 are written in Unio with their tests (`examples/README.md`); they grow with the language.
5. Keep the `std` interface free of host assumptions so the UniOS backend of section 9.2 stays possible; record it in the standard library review checklist.

## 11. Acceptance of comparative-review findings

Changes from the initial 0.1 draft: [RFC 0001](rfcs/0001-language-contracts.md) is accepted; the [contracts](language-contracts.md) are mandatory at these gates. This is work within the existing stages; estimates must be reassessed against its scope.

| Stage | Additional deliverable and acceptance |
|---|---|
| 0–1 | Named-parameter/function-shape grammar, positive/negative corpus, RFC and synchronized localization |
| 2 | E0040–E0048: labels, exclusivity, required results, implementation ownership, Float, JSON privacy, Range migration and packages. Callback effects; SCC hashes and distinct target/toolchain keys |
| 3 | Execute all new test blocks; evaluation/laziness/defaults, key-call counts, NaN, Unicode, JSON boundaries, streaming EOF/errors/release, property shrinking. REPL with ordinary semantics and sandbox |
| 4 | Same semantic scenarios in native code on four platforms; bounded streaming memory; core-only Range; actual descriptor and Windows/UNC tests |
| 5 | Strict decoding and explicit Unicode APIs; property-test JSON replay; locked install versus update, dependency conflicts, API compatibility; documented domain wrappers |
| 6 | Expected types for incomplete code, revision-bound fixes and rechecking after application; executable documentation examples |
| 0.2 / UniOS | Owned task scopes, cancellation/cleanup, bounded queues, restart budgets/process-isolated supervision; parameterized database queries; local and SSH scenarios |
| 0.3 / 0.5 | Typed compute kernels and buffers; ABI/layout/operation costs and cross-target tests |

Accepting the document and passing static scripts does not complete stage 2 or 3. The delivery table in the contracts identifies mechanisms that are still design-only. Multi-statement lambdas, general yield, dimensional algebra and dynamic downcasts are outside this change.

## 12. Sprint to release 0.1

The stages of section 3 stay the acceptance criteria; this section orders the work into sprints so that release 0.1 is reached as early as possible. A sprint is a vertical slice that ends with green CI, updated documentation in both languages, and its exit criterion met, not a fixed number of weeks. Sprints follow the critical path; work that does not block the next sprint runs in parallel streams. Every task starts with a failing check (section 1). Scope is cut only through an RFC, never silently.

| Sprint | Contents | Exit criterion | Stage |
|---|---|---|---|
| S1 Front end | Rust lexer and parser with spans and error recovery, diagnostics in text and JSON in English and Russian, `unio check` for syntax, `unio parse --ast` | Every corpus file and every `.unio` source of the repository gives the same result as `spec/check_grammar.py`; the parser survives randomized mutation | 1 |
| S2 Formatter and context | `unio fmt` (comments kept, AST before equals AST after, idempotent), `unio outline`, `unio explain`, the catalog of syntax codes; CI runs the Rust parser next to the Python checker | Stage 1 exit criterion; all sources are formatted | 1 |
| S3 Names and types | Following `docs/semantics.md` sections 1–4: modules and packages, visibility (L-23 to L-23d), types and local inference, generics, interface values and defaults (L-6b, L-6c), named parameters (RFC 0001) | The standard library, the reference utilities, and the tools pass name and type checks | 2 |
| S4 Errors, resources, effects | Following `docs/semantics.md` sections 5–8: `fails with` and `try` analysis, `match` exhaustiveness, `using` and resource escape, mutability and values, layers and `--profile=core`, derived effects with `allocates`, `unio symbol`; the R-10 control set grows from here | Stage 2 exit criterion | 2 |
| S5 Interpreter and runtime | Following `docs/runtime.md`: IR, interpreter, GC, `Resource` and `using`, the termination model, `unio test` with T-5a, the external primitives of `core`, `alloc`, and `std` | Every `test` block of `stdlib/`, `examples/`, `docs/examples/`, and `tools/` runs and passes; CI switches to the Unio tools; the Python checkers remain as the reference for parity and benchmarks | 3 |
| S6 Native build and sandbox | Cranelift code generation, one executable, deterministic builds, Linux x86-64 first and then the other three targets, the sandbox with the SB-8 and SB-15 suites, power-loss tests of atomic writes, benchmarks of 7.1 | Stage 4 exit criterion | 4 |
| S7 Packages and library | `unio.toml`, `unio.lock`, the registry with post-quantum signatures, `add`, `remove`, `update`, `unio doc`, the remaining modules of S-1 | Stage 5 exit criterion | 5 |
| S8 Editor, trials, release | Language server and the VS Code extension on top of the existing highlighting, the R-10 and readability trials, 24 hours of fuzzing, the guide with the chapter on classes, reproducible signed release | Acceptance of requirements section 9.2 | 6, 7 |

Status of the sprints. A sprint is done when its exit criterion holds without known exceptions; the requirement-level facts, with the test or CI step that checks each one and the platforms it runs on, are in [status.md](status.md), which is updated with each change. The gaps below are numbered as there.

| Sprint | Status | Open in this sprint |
|---|---|---|
| S1, S2 | Done | Long coverage-guided fuzzing belongs to S8 (R-1); now 2,000 deterministic mutations |
| S3 | Exit criterion met, with exceptions | Type identity at run time by short name (gap 1); a `Map[Float, V]` type is not rejected (gap 15) |
| S4 | Exit criterion of stage 2 not met | Effects through interfaces, function values and allocating expressions are derived (gap 2; RFC 0005, accepted: calls carry the effects of function values, E0087 for unprovable calls under `core`; acceptance fixes the semantics, complete only for the closed set of functions and implementations of the current program, a rule kept for S7; the correctness of the implementation is confirmed separately by tests on passing, returning, storing and calling function values, including recursive chains, which are being added); E0045–E0048 are in the catalog (gap 18); the R-10 control set of 200 programs does not exist (68 invalid semantic corpus files); closure condition 2 below |
| S5 | Exit criterion met, with exceptions | Runtime boundaries: children of `process.run` inherit the whole environment and their output, tree, and timeout are not bounded (gap 3), imports through links can leave the package (gap 4); memory: approximate heap accounting after allocation, no controlled path on allocator failure, no reserve for `release` (gap 5, closure condition 1 below); reference counting instead of the tracing GC of C-13, which needs an RFC after measurements (gap 9); `std.json` fills private fields (gap 16) |
| S6 | In progress | Sandbox precision: Linux rules wider than the manifest, children checked only by the operating system (gap 6); tool isolation SB-10 to SB-13 with the SB-15 suite (gap 8); reproducibility across directories, the image stores absolute paths (gap 10); start-up about 14 ms against 5 ms (gap 11); the other three native targets; the Windows sandbox (gap 7); power-loss tests of atomic writes, fault injection is not one (gap 13) |
| S7, S8 | Not started | Packages, registry, signing (gap 12), remaining modules of S-1, editor, trials, release |

Order of the remaining work, agreed after the audit. Each step starts with checks that fail and ends when [status.md](status.md) shows its rows as implemented and checked:

1. Semantics: type identity at run time, indirect effects (interfaces, function values, literals), and the `core` check. The semantics of function values is fixed by RFC 0005; what remains is the tests that confirm its implementation (status of S4 above).
2. Next: the read and execute rights of children, narrowed to what the manifest grants, together with keeping a child from leaving its process group, so that the promise to end the whole tree at the timeout becomes complete (gaps 3 and 6).
3. Then tool isolation (SB-10 to SB-13) with the SB-15 tests (gap 8).
4. Memory: accounting before allocation, the allocator-failure path, and the reserve for `release` of closure condition 1 below (gap 5).
5. Strict performance checks in CI, then start-up time and a program image that does not depend on the build directory.
6. The other native targets, the Windows sandbox, and trials of the file guarantees (power loss, killed processes).
7. S7, with stable package identity, safe installation, and signature verification.

Closure conditions before release 0.1. The memory reserve for `release` and the R-10 control set are not optional tails: the first is a guarantee the runtime declares (C-17, C-18), the second is the exit criterion of S4 (stage 2) and success criterion 3 of requirements section 1.4. Release 0.1 is not published until both hold as stated here and the rows C-17 and R-10 of [status.md](status.md) say "implemented and checked". The tests named below are to be created; each is written first and fails until the work lands.

1. **Memory reserve for `release` (C-17).**
   - What exists. Before `main`, the runtime sets aside a reserve of 1 MiB (1,048,576 bytes): it is allocated, written so that the system has delivered its pages, and counted in the heap. The reserve counts within the overall limit `limits.heap_bytes`, not on top of it: user code may use the limit minus the reserve, so at the minimum limit of 2 MiB normal execution has at most 1 MiB. A policy whose `limits.heap_bytes` is below 2 MiB is rejected before `main` with code 103 and a diagnostic that names the limit, the minimum, and the reserve. The reserve opens only on a controlled fault (code 101): it serves building the diagnostic and the trace and the unwinding with the `release` calls of the open `using` blocks. It becomes available before any allocation needed to build the fault and its trace; otherwise a full heap would end the program with 102 before it entered the release path. The reserved memory really serves that path: those allocations are taken from the pages set aside, not merely checked against a higher logical limit. The size and the rules are written in `docs/runtime.md` section 3 in the same change.
   - Parameters. The 1 MiB reserve and the 2 MiB minimum heap limit are the initial parameters of 0.1. 1 MiB is an engineering budget, not a proof that it suffices. The closure condition includes measurements of the peak use of the reserve during deep unwinding (many nested `using` blocks on a deep stack), repeated faults (a fault or error inside `release` during unwinding, in several releases in turn), and building diagnostics with long traces; the measured peaks and the margin are recorded in `docs/runtime.md` section 3, and if a peak does not fit, the parameters change with that record.
   - Paths that must run `release` from the reserve: a fault in user code while the rest of the heap is full; a fault or error inside one `release` during that unwinding, after which the remaining resources are still released in reverse order; each of them in `unio run`, in `unio test` (including `expect_fault`), and in executables of `unio build`.
   - Paths that must not run `release` (C-18): on exhaustion of memory or stack (code 102) no further user code runs, `release` included. These are user code exhausting the heap up to the reserve, the allocator refusing memory, the stack overflowing, and a `release` that exhausts the reserve during unwinding, after which the program goes to 102 and the remaining releases are skipped. Each ends with code 102 and the preallocated diagnostic, and a file of `open_atomic` keeps its temporary file and leaves the target unchanged (S-9).
   - How it is checked. A hook in `unio-runtime/src/heap.rs` (`fill_to`, through `UNIO_TEST_HEAP_FREE`; present in every build because the native tests run release executables, and it can only shrink the budget, [runtime.md](runtime.md) 3.6) fills the heap exactly instead of by estimate. Unit tests of the runtime: `the_release_reserve_is_set_aside_before_main`, `the_reserve_opens_only_for_unwinding`, `the_reserve_opens_before_the_fault_is_built`, and `the_unwinding_path_allocates_from_the_reserved_pages`; `the_reserve_peak_is_measured` (in `compiler/unio/src/interp.rs`, because it needs the interpreter to run real `release` code) runs the three measured cases of the parameters above, reports each peak, and fails when one exceeds the reserve. In `compiler/unio/tests/sandbox.rs`: `a_heap_limit_below_the_minimum_ends_with_103`. In `compiler/unio/tests/interpreter.rs`: `release_runs_from_the_reserve_after_a_fault_on_a_full_heap` (two resources, one an `open_atomic` file; the heap filled to the reserve by the hook; then a fault on the full heap, so that the fault and its trace are built from the reserve: code 101, the whole trace on standard error, no message of code 102, both releases ran in reverse order, the temporary file is gone and the target unchanged), `a_fault_in_release_on_a_full_heap_still_releases_the_rest`, `exhausting_the_reserve_in_release_ends_with_102`, and `exhaustion_does_not_run_release` (a program that really fills the heap under `limits.heap_bytes` = 8 MiB inside `using` of an `open_atomic` file: code 102, the temporary file remains, the target unchanged). In `compiler/unio/tests/native.rs`: `the_executable_releases_from_the_reserve_after_a_fault_on_a_full_heap` and `the_executable_does_not_release_on_exhaustion`.
   - CI and platforms. The runtime and interpreter tests run in the step "Tests (corpus, all .unio sources, diagnostics, mutation)" of the `compiler` job on Linux x86-64, macOS arm64, and Windows x86-64 (on Windows with `--allow-weaker-sandbox`, whose heap limit is enforced by the runtime); the native tests run in the step "Native executables give the output of unio run, builds are deterministic" on Linux x86-64 and on each further native target in the job that builds it; Linux arm64 joins with R-8. Closed when these pass on every platform where 0.1 ships, the measured peaks are recorded, and gap 5 no longer names the reserve.
2. **The R-10 control set.**
   - What exists. `spec/tests/control/` holds 200 programs with injected errors, each a file or, where the error needs a package, a directory with `unio.toml`. Each starts with `# expect-error: LINE CODE`, as in the corpus, and `# category:` with one of four categories: `types` (names, types, generics, interfaces, visibility; requirements L-4 to L-9a, L-23, L-28), `errors` (`fails with`, `try`, handlers, results that must be used; L-12 to L-16, L-30), `resources` (`using`, escape, mutability and `var`; L-10 to L-11a, L-20, L-21, L-29), and `style` (syntax and layout codes of `spec/README.md`, contextual keywords; L-1, L-2, T-1). Each category has at least 40 programs; the remaining 40 are chosen by difficulty and by gaps in coverage (the codes, constructs and requirements the first 160 reach least), not as near-duplicates of programs already in the set. Every code of the catalog that a program of a package can produce appears in at least one. Each program has its expected diagnostic (the header), a reference fix `NAME.fixed.unio`, and an independent check that the fix preserves behaviour: hidden behavioral tests in `NAME.check.unio`, `test` blocks that the fix passes, so that "fixed" means the program does what it should, not that the faulty code was deleted. For a syntax or style case with no behaviour beyond its form, the check may be specialised to the form only (the fix parses, is in the one style, and changes only the faulty construct) instead of inventing behaviour; `NAME.check.unio` then states that it is a form check.
   - Check A, the S4 exit criterion, on every change: `compiler/unio/tests/control.rs` with `the_control_set_has_200_programs_in_four_categories`, `every_control_program_yields_its_expected_diagnostic` (the expected code at the expected line in `--diagnostics=json`, and no other error), `every_reference_fix_is_clean_and_passes_its_tests` (`unio check` is clean and `unio test` runs the hidden tests and they pass, or the form check holds), and `every_code_has_a_control_program` (T-5b, R-12). It runs as the CI step "Control set yields the expected diagnostics (R-10)" in the `compiler` job on Linux x86-64, macOS arm64, and Windows x86-64, so the diagnostics an agent receives are the same on each.
   - Check B, the agent criterion, before release: `bench/control_trial.py` gives an agent each program and its JSON diagnostics of `unio check --diagnostics=json`, applies its answer, and repeats. A program counts as fixed when `unio check` is clean and its hidden tests pass under `unio test` (or its form check holds). The criterion is at least 160 of 200 (80%) within at most three attempts.
   - The protocol of check B is fixed and recorded in the report before the trial starts. Three attempts in total per program, the first fix included. The model version, the instructions, the available tools, and the budget are fixed for the whole trial. The agent has access only to the source of the program and its JSON diagnostics: no reference fixes, no hidden tests, no history of solutions. The checking infrastructure (the compiler, the control set, the hidden tests, the trial script) is not changed during the trial, and the task is not bypassed: an answer that deletes the faulty code, disables a check, or changes anything but the program does not count. Results are published after the first attempt and after the third, per category and in total, with the model and version and the compiler commit.
   - 80% is a criterion of this control set; it does not prove the same success on arbitrary programs. Later, a separate held-out set, not used for tuning diagnostics, is useful to measure that. It runs on Linux x86-64, since check A already shows the diagnostics are the same on every platform, in a CI workflow started by hand and before release, because it needs an external model service; its report is kept in `bench/results/` and published with the release (section 1.4, criterion 4). A new diagnostic code runs check B on its programs before merging (R-12).
   - Closed when check A is green on the three CI platforms, a check B report of the release candidate, run under the recorded protocol, meets 80%, and the status rows R-10 and R-12 say "implemented and checked".

Streams that run beside the critical path: the standard library and the reference programs grow with each sprint; documentation and its Russian localization follow every change; editor support started early, the syntax highlighting of `editors/` exists already and is tested against `spec/grammar.ebnf`.

## 13. Path to the self-hosted compiler

Concept section 1.1 asks for Unio to be built in Unio wherever possible, with the compiler self-hosted by 1.0. The compiler moves to Unio step by step and without hurry: a step starts only when it is ready and efficient, never by date.

A step starts when all of the following hold:

1. The Rust component it replaces is stable: its rules are settled in the documents and have not changed for a release.
2. Running the Unio version is fast enough for daily use: it meets the requirement that the Rust version meets (for example T-3 for the formatter, 7.1 for `unio check`) on the interpreter or on native code.
3. The language and the library have what the step needs without workarounds; a gap found on the way goes to the language as an RFC first.

A step ends when the Unio version gives the same results as the Rust version on every source of the repository, checked by a parity test in CI, and is then the one CI uses. The Rust version stays as the reference, as the Python checkers do (section 1).

| Step | Moves to Unio | Expected readiness |
|---|---|---|
| 1 | `unio fmt` and `unio outline` | After the performance work on the interpreter |
| 2 | Lexer, parser, and diagnostics catalog | After native code (S6) |
| 3 | Name and type checking | Release 0.3–0.4 |
| 4 | Code generation and the runtime | With the `system` profile of release 0.5 |
| 5 | The compiler builds itself; the Rust compiler remains as stage 0 for bootstrapping and for checking that two independent builds agree | 1.0 |
