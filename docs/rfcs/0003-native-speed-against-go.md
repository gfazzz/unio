# RFC 0003: Native speed measured against Go

- Status: proposed
- Author: Unio project, with Claude
- Date: 2026-11-07
- Affects: requirements / tools
- Russian localization: [0003-native-speed-against-go.md](../ru/rfcs/0003-native-speed-against-go.md)

## Summary

Requirement 7.1 measures Unio only against CPython: at least 0.5x, target 3x. Native executables of `unio build --release` are already 26x faster than Python on integer code, so the Python floor no longer says whether native code is good. This RFC adds a second measure for native executables: on the benchmark suite in `bench/`, each benchmark takes at most twice the time of the same algorithm in Go, with parity with Go as the target. The gate applies in two steps, so that 0.1 does not wait for optimizations planned after it.

## Motivation

Measured on Linux x86-64 with 4 CPUs, best of 3 runs, wall time including start-up (`python3 bench/run.py --native`, commit of sprint S6 part three). The last column is Unio native's time divided by Go's: 1 is parity, 2 is the proposed limit.

| Benchmark | Unio native | Python | Go | Rust | Native ÷ Go |
|---|---|---|---|---|---|
| wc | 0.273 s | 0.429 s | 0.523 s | 0.103 s | 0.52 |
| json | 6.858 s | 4.307 s | 0.596 s | 0.323 s | 11.5 |
| text | 0.243 s | 0.123 s | 0.065 s | 0.057 s | 3.7 |
| loop | 0.036 s | 0.969 s | 0.015 s | 0.014 s | 2.4 |

All four pass the Python floor, which hides the real picture: JSON is eleven times slower than Go and text almost four times. Users of a compiled language compare it with other compiled languages; Go has a garbage collector, bounds checks, and a simple compiler, as Unio does, and is therefore a fair yardstick. Rust and Mojo are not: they remove most of the checks that Unio keeps on purpose (overflow of `Int` is always a fault, L-17).

## Why the existing requirements are not enough

7.1 has no measure for native code against compiled languages, and its Python floor is already met by a wide margin on two of four benchmarks. Without a second measure, a regression that makes native JSON twice as slow would still pass CI.

## Design

1. A new row in the table of requirements section 7.1:

   | Metric | Requirement |
   |---|---|
   | Native executables (`unio build --release`) on the benchmarks of `bench/` | each at most 2x the time of the same algorithm in Go; target parity |

   Times are wall clock including start-up, best of N runs, on Linux x86-64, by the rules of `bench/README.md`. The Go version used is recorded with the results.
2. The gate applies in two steps:
   - Release 0.1: wc and loop are within 2x of Go; json and text are measured and published.
   - Release 0.2: every benchmark of the suite is within 2x of Go.
3. `bench/run.py --check-go` exits with 1 when a gated benchmark is above 2x of Go and prints the ratio of every benchmark. CI runs it on Ubuntu; until a benchmark is gated, its ratio is reported without failing the run.
4. The order of work that closes the gaps, from the measurements:
   1. Start-up: the executable rebuilds the declaration tables from its embedded sources, about 14 ms. Serialized tables remove it; this also meets the 5 ms start-up row of 7.1, which native code misses today. Expected effect: loop within 2x of Go.
   2. Static dispatch and monomorphized generics from the types of the checker, instead of dispatch by the type of the receiver at run time. Expected effect: most of the gap in text and part of json.
   3. Reference-count elision and reuse in place (Perceus): dup and drop at last use, borrowed parameters, and reuse of a released cell of the same size. Expected effect: json, which spends most of its time on counts and allocations of `Option` values and one-character texts.
   4. A representation of characters without a heap value.
   5. A second back end through LLVM for `--release`, if steps 1 to 4 leave a benchmark above the limit (requirements section 10 already names LLVM).

## Migration

Not needed: no language construct or program changes.

## Alternatives

- Parity with Go as a hard limit: not reachable for 0.1 without steps 2 and 3; a limit that is never met stops meaning anything.
- Rust as the yardstick: Rust removes overflow and many bounds checks that Unio keeps by design, so the comparison would measure the safety of Unio, not the quality of its compiler.
- A fixed time per benchmark: depends on the machine; a ratio to Go on the same machine does not.
- No gate, only published numbers: regressions would go unnoticed, which is the problem this RFC solves.

## Verification

- `bench/run.py --check-go` on the suite, in CI on Ubuntu, with the ratios in the job log.
- The table of `bench/README.md` updated with each step of Design 4, so that the effect of each step is recorded.
- Release notes of 0.1 and 0.2 publish the results, as section 7.1 already requires.
