# Work plan: Unio 0.1 "Core" and later releases

Version: 0.1 (draft). Basis: `docs/requirements.md`. Russian localization: `docs/ru/plan.md`. Durations are given in weeks from the start of work and assume a team of 3–4 engineers. With a different team the durations scale; the order of stages stays.

Changes from the initial 0.1 draft: UniOS requires human- and AI-accessible administration and an integrated base system (section 9; `docs/os.md`, sections 1.1–1.2).

Changes from the initial 0.1 draft: post-quantum signing is required for 0.1 distribution; network and UniOS profiles follow `docs/cryptography.md` (requirements T-8a, R-6a to R-6c, OS-7).

## 1. Project principles

- **Specification before code.** A construct enters the compiler only after its syntax and semantics are written in the reference.
- **Vertical slices.** Every stage ends with a working chain "source, check, run", even for a narrow subset of the language.
- **Reference utilities as the yardstick.** The ten utilities from the specification (section 9.1) are written early and grow with the language; a stage is complete when its utilities work.
- **RFCs for language changes.** Any change to syntax or semantics after stage 1 is written up in `docs/rfcs/NNNN-title.md` with rationale, examples, and a migration plan.
- **Measurement from the first stage.** Performance and diagnostic size are tracked in CI; a regression blocks the merge.
- **Unio first** (concept, section 1.1). Everything that does not have to exist before the language runs is written in Unio: the standard library in `stdlib/`, the reference utilities, and the project tools in `tools/`. Until the interpreter of stage 3 runs them, CI uses temporary Python checkers that do the same work; they are deleted at the end of stage 3. The compiler and the runtime stay in Rust until self-hosting in 1.0.
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

Exit criterion: `todo`, `wc`, `grep-lite`, `json-query` run in the interpreter and their tests pass; the tools in `tools/` run in the interpreter, give the same results as the Python checkers, and replace them in CI; the `test` blocks of the standard library pass in the interpreter, including the end-to-end test of the `commit` protocol (a failed directory sync followed by a successful second commit); reliability tests for faults inside `using` are green.

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
