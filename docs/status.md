# Implementation status of release 0.1

Russian localization: [ru/status.md](ru/status.md). Basis: [requirements.md](requirements.md) (specification 0.15), [plan.md](plan.md) section 12.

**This table is updated with each change.** A change that implements, checks, narrows, or breaks a requirement updates its row in the same commit, in both languages. A row says what the code in the repository does and which automated check confirms it, not what a sprint intended. Parsing a program or passing `unio check` is static validation; only `unio test`, `unio run`, and the executables of `unio build` execute Unio code. When this document and another one disagree about what is done, the code decides, and both documents are corrected.

State of the checkout this table describes: base commit `37d2d8b` and the work in progress on top of it (November 2026); facts were established by reading the code, the tests, and the CI workflow, and by probes with `unio check` and `unio run` named in the rows; no benchmark was rerun.

## Legend

Status:

- **implemented and checked**: the behavior the requirement states exists, and an automated check in CI fails if it breaks, on the platforms the requirement covers or, for a platform-independent rule, on at least one.
- **implemented, partly checked**: the behavior exists, but the check covers only part of it, only some of the required platforms, or only a review.
- **partial**: part of the behavior is missing or differs from the requirement; the row names what.
- **not started**: no implementation beyond the specification and, where noted, policy text.

Checks: `tests/X.rs` is `compiler/unio/tests/X.rs`; `runtime tests/X.rs` is `compiler/unio-runtime/tests/X.rs`; "spec corpus" is `spec/tests/valid` and `spec/tests/invalid`; "semantic corpus" is `spec/tests/semantic`; "test blocks" are the `test` blocks of `stdlib/`, `examples/`, `docs/examples/`, `docs/ru/examples/`, and `tools/`, run by `unio test` in the CI job `spec` and by `tests/interpreter.rs`. CI is [.github/workflows/ci.yml](../.github/workflows/ci.yml).

Platforms where the check runs: **Lx64** Linux x86-64 (`ubuntu-latest`), **mac** macOS arm64 (`macos-latest`), **Win** Windows x86-64 (`windows-latest`). Linux arm64, the fourth platform of R-8, is not in CI. The jobs `compiler` (`cargo test` of both crates) and `spec` (the Unio tools and test blocks) run on all three; `native`, the benchmarks, the Python parity job, and the editor tests run on Lx64 only. On Win, `unio run` and `unio test` run with `--allow-weaker-sandbox`, because there is no operating-system sandbox there.

## Summary

Counted per identifier (a grouped row counts each of its identifiers); the table 6.1 and the rows of 7.1 are counted separately at the end.

| Group | Identifiers | Implemented and checked | Implemented, partly checked | Partial | Not started |
|---|---|---|---|---|---|
| L (language) | 48 | 34 | 10 | 4 | 0 |
| C (compiler and runtime) | 23 | 11 | 5 | 7 | 0 |
| SB (sandbox) | 16 | 3 | 5 | 3 | 5 |
| S (library) | 24 | 8 | 5 | 10 | 1 |
| T (tools) | 20 | 8 | 0 | 4 | 8 |
| R (non-functional) | 16 | 2 | 3 | 3 | 8 |
| D (documentation) | 5 | 1 | 0 | 1 | 3 |
| Total | 152 | 67 | 28 | 32 | 25 |

Table 6.1 (commands): 8 of the 16 commands exist. Section 7.1 (7 metrics): 1 met and gated, 2 met in part and not gated, 1 not met, 3 not measured.

## Known gaps

These were found by an audit of the sprints S1 to S6 and are stated here precisely, so that "done" in the plan can be read against them. "Being fixed" means work in progress at the time of writing; the row changes when the fix lands with its check.

1. **Runtime type identity by short name** (being fixed). The interpreter and the native runtime find methods by the short name of the receiver's type, so two types of one name in different modules would share an entry of the method table. The checker keeps them apart; execution does not. Rows L-6, L-6b, R-4.
2. **Indirect effects and allocation by literals** (being fixed). Effects follow direct calls only: a call through an interface method or a function value does not carry the effects of the implementation, so E0085 and `unio symbol` can miss `files`, `process`, or `env` reached that way. `--profile=core` looks at calls, not at list or map literals, so allocation by a literal is not reported. Rows L-9b, C-11, C-21, L-26, S-14, SB-7.
3. **Children of `process.run`** (being fixed). A child inherits the whole environment of the program, whatever the `env` permission says; its standard output and error are read to the end without a bound; the timeout kills only the direct child, not its process tree; the process limit counts calls of `process.run`, not processes in the tree. Rows S-6, SB-1, SB-3.
4. **Imports through symbolic links** (being fixed). A module path is resolved through links, so a link inside a package can make `import` load a file outside the package root (SB-11 forbids this). Rows L-22, SB-11, SB-15.
5. **Heap accounting** (being fixed). The runtime counts estimated capacities of its values, after the allocation has happened; `limits.heap_bytes` is therefore approximate, and an allocation that the system refuses has no controlled path to code 102 (the Rust allocator aborts the process). There is no memory reserve for `release` set aside at start. Rows L-17a, C-17, C-18, R-2, SB-3.
6. **Linux operating-system rules are wider than the manifest.** Landlock grants write access to the directory of a single written file (the atomic commit creates its temporary file there); a `process` permission lets children read and execute the system directories; `stat` is not restricted; a scope root that does not exist at start grants nothing. The runtime checks the program's own operations precisely, but children are restricted only by the operating-system rules, not by the runtime. Rows SB-2, SB-4, SB-8; [runtime.md](runtime.md) section 9.
7. **Windows has no operating-system sandbox.** AppContainer and Job Objects are not implemented; programs start only with `--allow-weaker-sandbox`, and then only the runtime layer and the limits apply. Rows SB-1, SB-4, SB-5, SB-8, R-7a.
8. **Tool isolation.** The isolated workers of SB-10 to SB-13 do not exist: `unio check`, `fmt`, `build`, and the compile step of `test` run in the tool's own process without a sandbox. The hostile-project suite SB-15 does not exist.
9. **C-13 and reference counting.** C-13 asks for a precise, tracing, non-moving GC; the runtime uses reference counting with copy-on-write and no tracing. This is a deviation from the requirement: keeping it needs an RFC after measurements, including the latency of freeing a large structure at once (the counterpart of a GC pause) and the GC-pause row of 7.1. Rows C-13, L-19.
10. **Reproducibility.** C-4 is tested only as two builds of one source in one directory (`tests/native.rs`, `two_builds_are_byte_for_byte_the_same`). The embedded program image stores absolute paths of the main file, the standard library, the sources, and the manifest, so builds from two directories or machines differ. Rows C-4, R-6.
11. **Native targets and start-up.** Native code exists only for Linux x86-64. The start of a native executable is about 14 ms (it rebuilds the declaration tables from embedded sources; [bench/README.md](../bench/README.md)), against at most 5 ms in 7.1. Rows C-5, R-8, 7.1.
12. **Signing and trust.** Post-quantum signing, trust anchors, rotation, revocation, and rollback protection exist only as policy ([cryptography.md](cryptography.md)); nothing is implemented. Rows T-8, T-8a, R-6 to R-6c.
13. **File guarantees after power loss.** The fault-injection hook fails `fsync`, `close`, `rename`, and the directory sync one by one, and the tests check the recorded state; that is not a power-loss test. dm-log-writes replay, killing processes during writes, and powering off virtual machines (S-9b) are not done. Rows S-9b, R-2, C-19.
14. **Fuzzing.** `tests/fuzz.rs` runs 2,000 deterministic mutations by default (`UNIO_FUZZ_ITERATIONS` raises it), plus random renaming in `tests/semantic.rs`; the plan asks for long coverage-guided runs, 24 hours before release (R-1).

Found while writing this table, by probes with the release binary:

15. A `Map[Float, Int]` literal passes `unio check`; only the methods of such a map are then missing (E0056). L-9 says `Float` cannot be a key. Row L-9.
16. `std.json.decode` fills a private field of a struct (probe: a struct with a private `balance` decoded from JSON, run with `unio run`); S-5 and S-17 forbid this, and E0046 is not in the catalog. Rows S-5, S-17.
17. A file outside the standard library may be an `unsafe module` with an `external function`: `unio check` accepts it and `unio run` faults at the call ("the runtime has no external function"). S-7 asks for a compile-time rejection; `tools/check_api.unio` checks only the repository. Row S-7.
18. E0045 to E0048 of RFC 0001 are not in the diagnostic catalog: an IEEE `Float` where `Order` is needed is reported as E0056 or E0062, a `Range` used as a `List` as E0061; the cases themselves are rejected. Rows L-32, S-15, S-17, T-14.

## 3. Language (L)

| Requirement | Status | Implementation | Check | Platforms | Known limits |
|---|---|---|---|---|---|
| L-1, L-2, L-2a | implemented and checked | lexer and parser (`compiler/unio/src/lexer.rs`, `parser.rs`); keyword list from grammar section L | `tests/corpus.rs`, `tests/sources.rs`, spec corpus; CI step "Grammar consistency and corpus" (`tools/check_grammar.unio --corpus`); editor tests check the keywords | Lx64, mac, Win | — |
| L-3 | implemented, partly checked | documentation comments kept as trivia and shown by `unio symbol` | `tests/symbol.rs` | Lx64, mac, Win | `unio doc` does not exist (D-3) |
| L-4, L-5, L-7, L-8, L-8a | implemented and checked | `types.rs`, `check.rs`: built-in types, no implicit conversion, generics with constraints, local inference, lambda types from context (E0049) | semantic corpus (`wrong_type`, `cannot_infer`, `constraint_not_met`, `lambda_without_types`, ...) in `tests/semantic.rs`; every repository source passes `unio check` | Lx64, mac, Win | — |
| L-6, L-6b, L-6c | implemented, partly checked | user types, `extend ... with`, interface values with dispatch at run time, default methods | semantic corpus (`interface_method_missing`, `interface_not_a_type`, `interfaces_and_patterns`), test blocks | Lx64, mac, Win | gap 1: methods are found by the short type name at run time |
| L-6a | implemented and checked | short variant form with collision check (E0072) | `variant_short_collision` | Lx64, mac, Win | — |
| L-9 | partial | content equality derived; `==` on a resource is E0066; `Float` has no `Hash` | `resource_equality`, test blocks | Lx64, mac, Win | gap 15: a `Map[Float, V]` literal type is not rejected |
| L-9a | implemented and checked | operators through the standard interfaces; one implementation per type (E0044) | `text_plus_int`, `second_implementation` | Lx64, mac, Win | — |
| L-9b | implemented, partly checked | `effects.rs` derives effects from the call graph; `uses` reserved (E0032) | `tests/symbol.rs`, spec corpus | Lx64, mac, Win | gap 2: not through interface methods or function values |
| L-10, L-11, L-11a | implemented and checked | E0077, E0078, E0079, E0042; values with copy-on-write in `unio-runtime/src/value.rs` | `assign_to_let`, `change_through_let`, `field_not_var`, `var_argument_*`; test blocks | Lx64, mac, Win | — |
| L-11b | implemented, partly checked | `Rc::make_mut`: a shared value is copied at the changed level, an unshared one changed in place | test blocks; benchmarks | Lx64 | the cost of copies is not a separate benchmark of 7.1 |
| L-12, L-13, L-14, L-14a, L-15, L-16 | implemented and checked | `fails with`, `try`, `try:`, inline handlers, completeness (E0073 to E0076) | `fallible_call_without_try`, `handler_completes`, `unhandled_error`, `try_cannot_fail`, `fail_unlisted_error`, `errors_values_resources`; `tests/interpreter.rs` | Lx64, mac, Win | — |
| L-17 | implemented and checked | faults with trace and code 101 in the interpreter and native code | `tests/interpreter.rs` (`faults_release_resources_and_end_with_their_codes`), `tests/native.rs` | Lx64, mac, Win (native Lx64) | — |
| L-17a | partial | stack exhaustion ends with 102; heap limit of the policy ends with 102 | `tests/interpreter.rs` (recursion), `tests/sandbox.rs` (limits) | Lx64, mac, Win | gap 5: approximate heap count, no controlled path when the allocator fails |
| L-18 | implemented, partly checked | safe forms in `stdlib/` (`get`, `checked_divide`, `Option` results) | test blocks | Lx64, mac, Win | no check that every faulting operation has a safe form |
| L-19 | partial | no address of a value can be obtained | by construction | — | memory is reference-counted, not under a GC (gap 9, C-13) |
| L-20, L-21 | implemented and checked | `using`, release in reverse order on exit, error, and fault; resource escape (E0080, E0081) | `resource_outside_using`, `resource_escapes`, `resource_parameter_*`; `tests/interpreter.rs` | Lx64, mac, Win | — |
| L-22 | partial | modules, packages by `unio.toml`, `import`, `from ... import`, `std.` names | `module_not_found`, `imported_name_missing`, `import_collision` | Lx64, mac, Win | `deps.<alias>` names do not exist (no packages, T-14); gap 4: imports through links can leave the package |
| L-23, L-23a, L-23b, L-23c, L-23d | implemented and checked | visibility of declarations, fields, methods, functions of a type (E0057) | `private_field`, `names_and_types`, `visibility.unio` | Lx64, mac, Win | — |
| L-24 | implemented and checked | import cycles named (E0058) | `import_cycle_a`, `import_cycle_b` | Lx64, mac, Win | — |
| L-25 | implemented and checked | `test` blocks, `assert`, `assert_fail`, `expect_fault`; not compiled into `unio build` output | `tests/interpreter.rs`, CI step "Unio test blocks" | Lx64, mac, Win | — |
| L-26 | implemented, partly checked | `application` profile; `--profile=core` with the chain of calls (E0084) | `tests/symbol.rs` (`the_core_profile_reports_...`) | Lx64, mac, Win | gap 2: allocation by literals is not seen |
| L-27 | implemented and checked | left-to-right strict evaluation, fresh defaults per call, short circuit | test blocks of `examples/contracts.unio`; native parity in `tests/native.rs` | Lx64, mac, Win | — |
| L-28 | implemented and checked | named-only parameters and labels (RFC 0001; E0040, E0041, E0064) | `named_only_positional`, `parameter_grouping`, `unknown_argument_name`, spec corpus `named_parameters.unio` | Lx64, mac, Win | — |
| L-29 | implemented and checked | one name to one `var` parameter (E0042); iteration over value snapshots | `var_argument_twice`; test blocks | Lx64, mac, Win | — |
| L-30 | implemented and checked | E0043 for a dropped `Result` or `process.Output` | `discarded_result`, `result_binding_never_read`, `discarded_values` | Lx64, mac, Win | — |
| L-31 | implemented, partly checked | one implementation per type and interface (E0044) | `second_implementation` | Lx64, mac, Win | ownership across packages is untested because packages do not exist yet |
| L-32 | implemented, partly checked | `OrderedFloat`, `Float.total_order`; `Float` has no `Order` | test blocks of `stdlib/core/builtin/float.unio`, `runtime core.rs` unit tests | Lx64, mac, Win | gap 18: reported as E0056/E0062, not E0045 |
| L-33 | implemented and checked | `expr is pattern` with scoped bindings (E0050) | `is_binding_outside`, spec corpus | Lx64, mac, Win | — |

## 4. Compiler and runtime (C)

| Requirement | Status | Implementation | Check | Platforms | Known limits |
|---|---|---|---|---|---|
| C-1 | implemented and checked | lexer, parser, names, types and errors, resources, lowering, Cranelift code generation | one test set per stage: `tests/corpus.rs`, `tests/semantic.rs`, `tests/interpreter.rs`, `tests/native.rs` | Lx64, mac, Win | — |
| C-2 | implemented and checked | recovery after syntax errors | `tests/diagnostics.rs` (`several_errors_are_reported_in_one_pass`) | Lx64, mac, Win | semantic checks run only when every module parses |
| C-3 | implemented and checked | `unio check` | CI step "Unio sources pass name and type checks" | Lx64, mac, Win | — |
| C-4 | implemented, partly checked | deterministic encoding of the image and the object file | `tests/native.rs` (`two_builds_are_byte_for_byte_the_same`), CI step "Native executables ..." | Lx64 | gap 10: two builds in one directory only; the image stores absolute paths |
| C-5 | implemented, partly checked | `unio build` links the object with `libunio.a` into one executable | `tests/native.rs` | Lx64 | Linux x86-64 only; needs the system C compiler at build time |
| C-6, C-7, C-9 | implemented and checked | codes, text form with snippet, `unio explain` from `src/catalog.rs` | `tests/catalog.rs`, `tests/diagnostics.rs` | Lx64, mac, Win | — |
| C-8 | partial | `--diagnostics=json` with code, severity, message, explanation, spans, fixes | `tests/diagnostics.rs` (`json_format_has_the_required_fields`) | Lx64, mac, Win | `related` is always empty; no published, versioned JSON Schema |
| C-10 | implemented and checked | English and Russian; `--lang`, `UNIO_LANG` | `tests/diagnostics.rs` | Lx64, mac, Win | — |
| C-11 | implemented, partly checked | `unio symbol` in text and JSON with errors, effects, layer, calls | `tests/symbol.rs` | Lx64, mac, Win | gap 2: indirect effects missing |
| C-11a | partial | SHA-384 content hashes, scheme `unio-hash-v1`, components of mutual recursion | `tests/symbol.rs` (hash tests) | Lx64, mac, Win | no incremental build or test cache; `unio test --json` does not report the hash |
| C-11b | partial | recursive symbol hashes | `tests/symbol.rs` | Lx64, mac, Win | artifact and test-result keys of language-contracts section 6 do not exist |
| C-12 | implemented and checked | `unio outline` | `tests/outline.rs` | Lx64, mac, Win | — |
| C-13 | partial | reference counting with copy-on-write; no cycles by the value model | runtime unit tests | — | gap 9: not a tracing GC; needs an RFC after measurements; GC-pause row of 7.1 not measured |
| C-14 | implemented and checked | trace of function, file, line in interpreter and native code | `tests/interpreter.rs`, `tests/native.rs` | Lx64, mac, Win | — |
| C-15 | implemented and checked | codes 0–100, 101, 102, 103; `main` outside 0–100 is a fault | `tests/interpreter.rs`, `tests/sandbox.rs`, `tests/native.rs` | Lx64, mac, Win | — |
| C-16 | implemented and checked | normal end flushes output | `tests/interpreter.rs`, `tests/native.rs` | Lx64, mac, Win | — |
| C-17 | partial | unwinding releases every open `using` in reverse order, keeps going after a failing release | `tests/interpreter.rs` | Lx64, mac, Win | gap 5: no memory reserve set aside for `release` |
| C-18 | partial | stack overflow, heap limit, wall and CPU time end with 102 | `tests/interpreter.rs`, `tests/sandbox.rs` | Lx64, mac, Win | gap 5: count after allocation; allocator failure aborts without the preallocated diagnostic |
| C-19 | implemented, partly checked | no guarantees beyond S-9 by definition | fault injection in `runtime tests/files.rs` | Lx64, mac, Win | gap 13: no power-loss or kill test |
| C-20 | partial | guarantees described in [runtime.md](runtime.md) sections 3–5 and requirements 4.5 | — | — | no single statement per termination kind for resources, standard output, and files |
| C-21 | implemented, partly checked | `--profile=core`, E0084 with the chain | `tests/symbol.rs` | Lx64, mac, Win | gap 2: literals that allocate are not reported |

## 4.6 Sandbox (SB)

| Requirement | Status | Implementation | Check | Platforms | Known limits |
|---|---|---|---|---|---|
| SB-1 | partial | the sandbox is applied before `main` in `unio run`, `unio test`, and built executables; children inherit the operating-system rules | `tests/sandbox.rs` (`child_processes_inherit_the_sandbox`), `tests/native.rs` (`the_executable_applies_the_manifest_of_its_package`) | Lx64, mac | gap 7: none on Windows; gap 3: children inherit the whole environment |
| SB-2 | implemented, partly checked | intersection of manifest and policy; nothing without permissions | `tests/sandbox.rs` | Lx64, mac (runtime layer also Win) | gap 6: Linux rules wider than the manifest |
| SB-3 | partial | `--policy`, `UNIO_POLICY`, limits of heap, wall, CPU, processes, file size; warnings for extra permissions | `tests/sandbox.rs` (`a_policy_narrows_...`, `the_limits_of_a_policy_...`) | Lx64, mac, Win | gap 5: heap limit approximate; gap 3: processes counts calls, not the tree |
| SB-3a | implemented and checked | the policy format, unknown keys and `network = true` are code 103 | `tests/sandbox.rs`, runtime TOML unit tests | Lx64, mac, Win | — |
| SB-4 | partial | Linux: `no_new_privs`, Landlock, seccomp-bpf, resource limits; macOS: Seatbelt and limits | `tests/sandbox.rs`, runtime unit tests | Lx64, mac | gap 7: Windows not implemented; gap 6: Linux file rules coarser than the scopes |
| SB-5 | implemented and checked | fails closed with 103; `--allow-weaker-sandbox` and `allow_weaker` print what is missing | `tests/sandbox.rs` (`a_missing_mechanism_fails_closed`) | Lx64, mac, Win | — |
| SB-6 | implemented and checked | denied operations are `PermissionDenied` errors naming the permission | `tests/sandbox.rs` | Lx64, mac, Win | — |
| SB-7 | implemented, partly checked | E0085 against the manifest; effects in `unio symbol` | `tests/symbol.rs` (`the_manifest_permits_effects`) | Lx64, mac, Win | gap 2: effects through interfaces and function values escape; a manifest without `[permissions]` is not checked |
| SB-8 | implemented, partly checked | forbidden actions through `std.files`, `std.env`, `std.process` and through children | `tests/sandbox.rs` | Lx64, mac | not on Windows or Linux arm64; children are checked only by the operating-system rules (gap 6) |
| SB-9 | implemented, partly checked | limits of the sandbox documented in [runtime.md](runtime.md) section 9 | review; `check_docs` checks only links | — | the section is kept in step by hand with gap 6 |
| SB-10, SB-11, SB-12, SB-13 | not started | — | — | — | gap 8: tools run unisolated; gap 4: imports through links can leave the package |
| SB-14 | implemented, partly checked | no build scripts, macros, or compile-time evaluation of user code exist | by construction; no dedicated test | — | — |
| SB-15 | not started | — | — | — | gap 8 |

## 5. Standard library (S)

| Requirement | Status | Implementation | Check | Platforms | Known limits |
|---|---|---|---|---|---|
| S-1 | partial | `std.text`, `std.json`, `std.errors`, `std.test`, `std.files`, `std.env`, `std.args`, `std.process`; `Path` as a prelude type | test blocks; `runtime tests/bindings.rs` | Lx64, mac, Win | `std.collections`, `std.math`, `std.decimal`, `std.path`, `std.time`, `std.random`, `std.log` do not exist (S7) |
| S-1a | not started | — | — | — | — |
| S-2 | implemented, partly checked | concrete error types in every `fails with` of `stdlib/` | review only; no automated check | — | — |
| S-3 | partial | every module and public function has a documentation comment | — | — | public functions have no examples |
| S-4 | partial | `std.args.list()` | test blocks | Lx64, mac, Win | no flags, defaults, or automatic help |
| S-5 | partial | `parse`, `encode`, `encode_checked`, `decode`, conversions generated from the static type | test blocks of `stdlib/alloc/json.unio`, `tests/interpreter.rs` (`json_query`) | Lx64, mac, Win | gap 16: decoding fills private fields |
| S-6 | implemented, partly checked | `process.run` without a shell, captured output, status, timeout | `runtime tests/process.rs`, `tests/sandbox.rs` | Lx64, mac, Win | gap 3: environment, output size, and process tree not bounded |
| S-7 | partial | external functions only in `unsafe module` files of `stdlib/` | `tools/check_api.unio` (repository only) | Lx64, mac, Win | gap 17: the compiler accepts `unsafe module` outside the library; the reliability review of 7.3 (R-5) not done |
| S-8 | implemented and checked | blocked operations through the declared errors; `exists`, `env.get` report absence | `tests/sandbox.rs` | Lx64, mac, Win (runtime layer) | — |
| S-9, S-9c | implemented and checked | atomic replacement with recorded steps `Writing`, `Closed`, `Replaced`, `Durable`; repeatable `commit` | `runtime tests/files.rs`, test blocks of `stdlib/std/files.unio` | Lx64, mac, Win | guarantee 2 and 3 after power loss not tested (gap 13) |
| S-9a | implemented, partly checked | `fsync`, `F_FULLFSYNC`, `FlushFileBuffers` and `MoveFileExW`; file-system detection; `Unsupported` for `durable` elsewhere | runtime unit tests (`durability.rs`) | Lx64, mac, Win | tested only on the file systems of the CI runners |
| S-9b | partial | fault-injection hook `fail_point`; each `commit` step failed in turn; the same protocol in test blocks | `runtime tests/files.rs`, test blocks | Lx64, mac, Win | gap 13: no dm-log-writes, no kill or power-off tests |
| S-10 | implemented and checked | library in Unio; 70 externals bound in `unio-runtime` | `runtime tests/bindings.rs`; CI step "Unio sources use only the standard library API" | Lx64, mac, Win | — |
| S-11 | implemented and checked | layers by directory | `tools/check_api.unio`, E0084 (`tests/symbol.rs`) | Lx64, mac, Win | — |
| S-12 | partial | externals grouped by layer (`externs/core.rs`, `alloc.rs`, `system.rs`, `files.rs`, `process.rs`) | — | — | no check that the `core` runtime makes no system calls; no hook interface between the runtime layers |
| S-13 | implemented and checked | `sorted_by` with one key per element, `fold`, `partition`, `group_by`, `zip`, `chunks`, `windows` | test blocks of `stdlib/alloc/builtin/list.unio` | Lx64, mac, Win | — |
| S-14 | partial | `try_map`, `try_filter`, `try_fold` with concrete callback errors | test blocks | Lx64, mac, Win | gap 2: effects of callbacks are not derived |
| S-15 | implemented and checked | `Range` in `core`, independent cursors, `to_list` in `alloc` | test blocks | Lx64, mac, Win | gap 18: misuse reported as E0061, not E0047 |
| S-16 | implemented, partly checked | `open_reader`, bounded `read_bytes`, retained errors, release once | test blocks of `stdlib/std/files.unio`, `runtime tests/files.rs` | Lx64, mac, Win | no test with a file larger than the memory budget |
| S-17 | partial | strict decoding: unknown fields and variants, missing fields, `Float` for `Int`, JSON Pointer paths; duplicate keys and limits in the parser | test blocks of `stdlib/alloc/json.unio` | Lx64, mac, Win | gap 16: private fields are filled; E0046 absent |
| S-18 | partial | text as Unicode scalars with an index; no implicit normalization | test blocks of `stdlib/alloc/builtin/text.unio` | Lx64, mac, Win | no explicit normalization or grapheme API |
| S-19 | implemented, partly checked | lexical `Path.normalize`; `files.resolve` through the file system | test blocks of `stdlib/std/builtin/path.unio` | Lx64, mac, Win | UNC and drive-relative cases only as far as the test blocks go |
| S-20 | implemented and checked | `diagnostic_code` generated for every error; `std.errors.ContextError` | test blocks of `stdlib/alloc/errors.unio` | Lx64, mac, Win | — |

## 6. Tools (T)

Table 6.1: `unio build` (without `--target`), `run`, `check`, `fmt`, `test`, `symbol`, `outline`, `explain` exist; `new`, `add`, `remove`, `update`, `install`, `repl`, `api-diff`, and `doc` do not.

| Requirement | Status | Implementation | Check | Platforms | Known limits |
|---|---|---|---|---|---|
| T-1, T-1a, T-2 | implemented and checked | `src/format.rs`: one style, idempotent, comments kept, same tree | `tests/format.rs`; CI step "Unio sources are in the one style" | Lx64, mac, Win | — |
| T-3 | implemented and checked | — | CI step "Formatter speed (T-3, release build)" | Lx64, mac, Win | — |
| T-4, T-5, T-5a | implemented and checked | `unio test` with a fresh state per test, text and JSON, `expect_fault`, non-compiling tests fail alone | `tests/interpreter.rs` | Lx64, mac, Win | — |
| T-5b | partial | the practice of tests first | review | — | CI does not reject a grammar change without corpus files or a diagnostic without a control-set program |
| T-6 | partial | `[permissions]` with scopes and placeholders is read and enforced | `tests/sandbox.rs` | Lx64, mac, Win | dependencies and the list of `unsafe module` are not read |
| T-6a | implemented and checked | relative paths, links and `..` resolved before comparison | `tests/sandbox.rs` (`a_link_inside_a_scope_does_not_lead_out_of_it`) | Lx64, mac | — |
| T-7, T-8, T-8a, T-9 | not started | — | — | — | gap 12: signing is policy only |
| T-10 | not started | — | — | — | — |
| T-11 | partial | VS Code and Cursor extension with highlighting, packaged in CI | editor job `npm test` | Lx64 | no test running from the editor |
| T-12 | partial | `std.test.check_property` with deterministic generation, shrinking, replay data | test blocks of `stdlib/alloc/test.unio` | Lx64, mac, Win | no runner integration, no JSON replay |
| T-13, T-14, T-15 | not started | — | — | — | — |

## 7. Non-functional requirements (R)

### 7.1 Performance

| Metric | Status | Measurement | Check |
|---|---|---|---|
| `unio check` of 10,000 lines at most 1 s | not measured | — | — |
| `--release` build of 10,000 lines at most 30 s | not measured | — | — |
| Start of an empty program at most 5 ms | not met | about 14 ms for a native executable (gap 11) | — |
| Text and JSON at least 0.5x of CPython | met natively and gated | native wc 1.23x, json 0.66x, text 0.62x, loop 29x; interpreter json 0.36x | CI benchmarks: native floor enforced; interpreter json reported only |
| Native time at most 2x Go (RFC 0003) | met in part, not gated | wc and loop within; json and text outside, gated from 0.2 | reported with `continue-on-error` |
| Native peak memory at most 2x Go plus 8 MB | met in part, not gated | wc, text, loop within; json 4.2x | reported |
| GC pause at a 100 MB heap at most 10 ms | not measured | no GC (C-13); freeing large structures not measured | — |

Figures are from [bench/README.md](../bench/README.md), Linux x86-64, and were not rerun for this table.

### 7.2 to 7.5

| Requirement | Status | Implementation | Check | Platforms | Known limits |
|---|---|---|---|---|---|
| R-1 | partial | the front end and checker survive mutations | `tests/fuzz.rs` (2,000 mutations), `tests/semantic.rs` (renaming) | Lx64, mac, Win | gap 14: no long coverage-guided fuzzing |
| R-2 | partial | 102 with a diagnostic for stack and policy limits; atomic writes | `tests/interpreter.rs`, `tests/sandbox.rs`, `runtime tests/files.rs` | Lx64, mac, Win | gap 5, gap 13: no allocator-failure path, no kill-during-write tests |
| R-3 | implemented and checked | release on fault inside `using` | `tests/interpreter.rs` | Lx64, mac, Win | Linux arm64 not in CI |
| R-4 | implemented, partly checked | bounds, type, and resource checks | semantic corpus, test blocks | Lx64, mac, Win | gap 1: run-time type identity by short name |
| R-5 | not started | — | — | — | no two-person review record, no sanitizer runs |
| R-6, R-6a, R-6b, R-6c | not started | policy in [cryptography.md](cryptography.md) | — | — | gap 10, gap 12 |
| R-7 | implemented, partly checked | the compiler runs no package code (no build scripts or macros) | by construction | — | no package manager exists yet |
| R-7a | not started | — | — | — | SB-8 fails on Windows and SB-15 does not exist |
| R-8 | partial | CI on Linux x86-64, macOS arm64, Windows x86-64 | all CI jobs | Lx64, mac, Win | no Linux arm64; native code and SB-8 not on every platform |
| R-9 | not started | — | — | — | no `unio fmt --migrate` |
| R-10 | not started | — | — | — | no control set of 200 programs; the semantic corpus has 57 invalid files with expected codes |
| R-11 | implemented and checked | every catalog message within 200 characters | `tests/catalog.rs` | Lx64, mac, Win | — |
| R-12 | implemented, partly checked | one catalog, template, list of internal names; review checklist `compiler/DIAGNOSTICS.md` | `tests/catalog.rs` | Lx64, mac, Win | the run on the R-10 corpus cannot happen until it exists |

## 8. Documentation (D)

| Requirement | Status | Implementation | Check | Platforms | Known limits |
|---|---|---|---|---|---|
| D-1 | partial | grammar, [semantics.md](semantics.md), [runtime.md](runtime.md), diagnostic catalog with `unio explain` | `tests/catalog.rs`, `check_docs` | — | no single language reference |
| D-2 | not started | — | — | — | no guide, no chapter on classes |
| D-3 | not started | — | — | — | `unio doc` does not exist; [stdlib.md](stdlib.md) is written by hand |
| D-4 | not started | — | — | — | — |
| D-5 | implemented and checked | Russian localization in `docs/ru/` | CI step "Documentation formatting and links" (`tools/check_docs.unio`), Python parity | Lx64, mac, Win | — |
