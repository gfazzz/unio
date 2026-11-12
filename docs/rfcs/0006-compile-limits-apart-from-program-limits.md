# RFC 0006: Compile limits apart from program limits

- Status: accepted (decided by the project lead on 2026-11-12; implemented in `compiler/unio/src/worker.rs` and the policy parser of `compiler/unio-runtime/src/sandbox/mod.rs`, see Verification)
- Author: Unio project, with Claude
- Date: 2026-11-12
- Affects: requirements (SB-3, SB-3a, SB-11) / runtime documentation (docs/runtime.md 9.7) / tools
- Russian localization: [0006-compile-limits-apart-from-program-limits.md](../ru/rfcs/0006-compile-limits-apart-from-program-limits.md)

## Summary

The external policy gets a `[compile]` table with the limits of the workers of the tools: `compile.memory_bytes`, `compile.wall_seconds`, and `compile.cpu_seconds`. They hold alike for every worker of `unio check`, `fmt`, `outline`, `symbol`, `parse --ast`, `build` (the compile step and the link step), and the compile steps of `run` and `test`. Without them the documented defaults apply: 2 GiB, 600 seconds, and no CPU limit of their own. The limits of the program, `[limits]`, no longer reach a worker. A limit of the operator is never raised: a limit that cannot be honoured, a `compile.memory_bytes` below the measured minimum of 192 MiB, is refused with code 103 and a diagnostic before anything is compiled.

## Motivation

Before this RFC one set of keys served two different processes. `limits.heap_bytes`, `limits.wall_seconds`, and `limits.cpu_seconds` were the limits of the program (SB-3) and also the limits of every worker of the tools (SB-11). The two cannot share a memory limit: the heap of a program may be as small as 2 MiB (C-17), while a worker that loads the compiler and the standard library needs far more address space. Commit `c51d269` made the compile steps of `run` and `test` keep the limits of the policy, and to let a 2 MiB program still compile it raised their memory to a hidden floor of 256 MiB (`COMPILE_MEMORY_FLOOR`):

```toml
[limits]
heap_bytes = 2_097_152   # the program: 2 MiB
```

With this policy the operator asked for 2 MiB and the compile step ran with 256 MiB; `docs/status.md` recorded it as a deviation from strict narrowing (rows SB-3, SB-3a, SB-11). An explicit limit of the operator was silently raised, which a sandbox must never do: the operator cannot tell from the policy what the tool will be allowed. The project lead decided on 2026-11-12 that the floor must go.

## Why existing constructs are not enough

No language construct is involved. The existing keys of `[limits]` cannot carry both meanings: any rule that derives the worker's memory from the program's heap either raises the operator's limit (the floor) or makes a small heap impossible to compile.

## Design

### Keys

```toml
[limits]                    # the program (SB-3)
heap_bytes = 2_097_152

[compile]                   # the workers of the tools (SB-11)
memory_bytes = 268_435_456
wall_seconds = 120
cpu_seconds = 60
```

- `compile.memory_bytes`: the memory limit of each worker, as `limits.heap_bytes` has been for workers so far: the address space on Linux (`RLIMIT_AS`, also the file-size limit of the worker), the footprint the tool watches on macOS, the process memory of the Job Object on Windows. Default 2 GiB.
- `compile.wall_seconds`: the wall time of each worker, kept by the tool. Default 600 seconds.
- `compile.cpu_seconds`: the processor time of each worker, `RLIMIT_CPU` on Linux and macOS, the user time of the Job Object on Windows. No default: the wall time bounds the worker. On Linux and macOS the tool reports the end at this limit as 102; on Windows that end is not yet told apart from another exit of the worker.

The keys are whole positive numbers. An unknown key in `[compile]` is an error with code 103, as every unknown key of the policy is (SB-3a), so a mistyped limit is never silently absent. `unio.toml` sets neither `[limits]` nor `[compile]`; a program, including an executable of `unio build`, ignores `[compile]`.

### Which workers

The same limits apply, each worker separately, to every worker the tools start: `unio check`, `fmt`, `outline`, `symbol`, `parse --ast`, the compile steps of `unio run` and `unio test`, and both workers of `unio build`, the compile step and the link step. The link step runs the system linker under the same limits, so the operator bounds all of `build`; each of its two workers has the whole `compile.wall_seconds`. `unio explain` reads no source and starts no worker. The limits of `[limits]` apply only to the program that `run` and `test` then execute, in the tool's own process (SB-1).

### A limit that cannot be honoured

The tool never raises a limit of the operator. A worker needs some memory to start at all; below it the limit cannot be honoured, and the tool refuses before it starts a worker or compiles anything:

```
unio: error: compile.memory_bytes = 1048576 of the policy is below the minimum of 201326592 bytes (192 MiB) that a worker of the tools needs to start and compile; the tool does not raise a limit of the operator, so it compiles nothing (RFC 0006, docs/runtime.md 9.7)
```

The exit code is 103, the code of a policy that cannot be applied. The minimum is `MINIMUM_COMPILE_MEMORY` in `compiler/unio/src/worker.rs`: 192 MiB (201,326,592 bytes). The time limits have no minimum: a worker that cannot finish within a positive limit is stopped with 102, which is the limit honoured.

### The minimum, from measurements

Measured on Linux x86-64 (kernel 6.18, 4 CPUs, GNU ld 2.42, GCC 13.3) on 2026-11-12: for each command, the smallest `compile.memory_bytes` with which the worker finished, found by bisection to 1 MiB with a build of the tool in which the minimum was disabled. Repeated runs vary by a few MiB, since the layout of the address space changes. The address space includes the 64 MiB stack of the thread of the front end.

| Command | Empty program, release | Empty program, debug | `tools/grammar.unio` (38 KB, the largest source), release | the same, debug |
|---|---|---|---|---|
| `check`, `run`, `test`, `symbol` | 109 MiB | 121 MiB | 153 MiB (`check`), 154 MiB (`test`) | 164 MiB |
| `fmt` | 78 MiB | 90 MiB | 123 MiB | 134 MiB |
| `outline`, `parse --ast` | 78 MiB | 90 MiB | 115 MiB | 127 MiB |
| `build`, both steps | 127 MiB | 533 MiB | about 165 MiB | 534 MiB |
| `build --release`, both steps | 126 MiB | 533 MiB | 175 MiB | 534 MiB |
| `build`, the compile step alone | 127 MiB or less | 139 MiB | about 165 MiB | 173 MiB |

The minimum is set with a margin over what an empty program needs: 192 MiB is about half again over the 127 MiB of `build` with the release tool and over the 139 MiB of the compile step of a debug tool. 256 MiB stays the recommended value: it holds the largest source of the repository with a margin. The link step runs the system linker, whose need depends on the toolchain and on `libunio.a`: with the 80 MB library of a release build it fits within the release numbers above, with the 290 MB library of a debug build, which carries debug information, it needs about 530 MiB. The minimum is therefore measured for the compile steps; a limit above it can still be too small for a linker, which then ends the build with 102, or with code 1 and the linker's message when that message does not show the exhaustion.

macOS limits the footprint, not the address space, so the reserved stack does not count there; Windows limits the memory of the Job Object. Their numbers are pending: the CI on those systems runs `a_compile_memory_below_the_minimum_is_refused_before_compiling`, which compiles and runs the empty program at exactly the minimum.

## Migration

A policy that limited the tools through `[limits]` keeps working and still limits the program, but no longer the tools: the operator moves those limits to `[compile]`. No file of the repository or of CI does so. The hidden floor of 256 MiB is removed; a policy that relied on it, a small `[limits] heap_bytes` for a program, now compiles under the default of 2 GiB.

## Alternatives

- Keep the floor and document it. Rejected by the project lead: an explicit limit of the operator must never be raised silently.
- Derive the worker's memory from the program's heap with a factor. Still raises a small limit, and couples two processes that need different amounts.
- Top-level keys such as `limits.compile_memory_bytes`. The `[limits]` table would again mix the program and the tools, and the rule "`[limits]` never reaches a worker" could not be stated.
- Let `unio.toml` set the compile limits. A hostile project could then raise the limits that protect the machine from it (SB-10); only the operator sets limits.
- Raise to the minimum instead of refusing. That is the floor again.

## Verification

Tests written to fail first, then passing on Linux x86-64 in debug and in release:

- `compiler/unio-runtime/src/sandbox/mod.rs`: `policies_read_compile_limits_apart_from_the_program_limits` (the keys, `[limits]` leaves `[compile]` unset, unknown keys, zero, negative and text values).
- `compiler/unio/tests/sandbox.rs`: `unknown_or_invalid_keys_of_the_compile_table_end_with_103` (`run` and `check`).
- `compiler/unio/tests/workers.rs`, with the probes of `UNIO_WORKER_PROBE`: `a_worker_over_the_compile_time_limit_is_stopped_alike_in_every_command` (`spin`; `check`, `build`, `run`, `test`; the policy by `--policy` and by `UNIO_POLICY`), `a_worker_over_the_compile_memory_limit_is_stopped_alike_in_every_command` (`alloc`), `a_worker_over_the_compile_cpu_limit_is_stopped_alike_in_every_command` (Linux and macOS), `the_program_limits_do_not_change_the_compile_limits` (a program heap of 2 MiB, wall and CPU time of 1 s: the compile step allocates 512 MiB and outlasts a second, and the program runs), `a_compile_memory_below_the_minimum_is_refused_before_compiling` (103 and the diagnostic for `check`, `build`, `run`, `test`, `fmt`, `outline`, `symbol`, `parse`, with the worker never started; at the minimum the empty program compiles and runs, `build` included with a release tool), `the_manifest_does_not_set_the_compile_limits`, and the existing `pathological_sources_end_with_a_diagnostic` and `the_link_step_over_the_time_limit_ends_with_its_process_tree`, now with `[compile]`.

macOS and Windows: pending the CI runs of the same tests.
