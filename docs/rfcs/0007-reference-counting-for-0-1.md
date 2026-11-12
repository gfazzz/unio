# RFC 0007: Reference counting instead of a tracing GC in 0.1

- Status: accepted (decided by the project lead on 2026-11-12; applied to the requirements, the concept, and docs/runtime.md 3; freeing without deep recursion implemented in `compiler/unio-runtime/src/value.rs`; the CI step of Verification added)
- Author: Unio project, with Claude
- Date: 2026-11-12
- Affects: requirements (C-13, L-19, S-11, S-12, the GC-pause row of 7.1) / concept (sections 4.1 and 8) / runtime documentation (docs/runtime.md 3) / tools (`bench/`)
- Russian localization: [0007-reference-counting-for-0-1.md](../ru/rfcs/0007-reference-counting-for-0-1.md)

## Summary

Release 0.1 keeps the memory management that the runtime has: reference counting with copy-on-write, no tracing, no moving, and no cycle collector, because the value model creates no cycles. C-13 is rewritten to state that model and its guarantees instead of "a precise, tracing, non-moving GC". The row "GC pause at a 100 MB heap at most 10 ms" of 7.1 measures something reference counting does not have, a pause that scans the live heap; it is replaced by the cost reference counting does have, the pause of freeing a large structure at once, bounded per megabyte freed (at most 1 ms per MB, median) and measured by a new benchmark, `bench/free/`. Freeing a structure no longer recurses on the native stack below 128 levels, so dropping a deep structure cannot overflow the stack; this part is implemented. The measured pause at 100 MB is 41 to 62 ms (median), above the 10 ms of the current row; deferred, incremental freeing that would bring it under a constant bound is proposed for 0.2, not for 0.1.

## Motivation

C-13 asks for a precise, tracing, non-moving GC. The runtime of S5 and S6 uses reference counting (docs/runtime.md 3.1, 3.2; status gap 9), and plan section 12 requires the deviation to be decided before 0.1 by measurements: either C-13 is met as written or an RFC that keeps reference counting is accepted. With the release due, a tracing collector cannot be added and checked in time (see Alternatives), so this RFC asks for the second outcome, on the measurements below.

Why reference counting fits Unio:

1. **No cycles by the value model.** Structs, enums, tuples, lists, maps, sets, bytes, and texts are values; binding, passing, returning, and storing never share mutable state, and function values capture copies (L-11a; docs/semantics.md, docs/runtime.md 2.4, 2.5, 3.1; `compiler/unio-runtime/src/value.rs`). A value therefore cannot reach itself, so counting frees every value and nothing leaks; there is no cycle for a tracing collector to find. Resources have identity but cannot be stored in a value that outlives their `using` block (concept 4.1), so they form no cycles either.
2. **Copy-on-write needs the count anyway.** L-11b changes an unshared value in place and copies a shared one; the runtime knows which by the count (`Rc::make_mut`). A tracing collector would not remove the counts; it would add a second mechanism beside them.
3. **Deterministic release.** A value is freed when its last name goes away, at a point of the program the reader can see, not when a collector runs. Memory use follows the live data, with no headroom for a collector to work in.
4. **No pause proportional to the live heap.** Nothing scans the memory that stays live. The only pause is freeing a structure the program drops, and it is proportional to that structure, at a statement the program chose.

What it costs, measured (Linux x86-64, 4 CPUs, Python 3.13, the release build of the compiler at `6928ee4` with the change of Design 3; the machine was shared with other builds, so each figure is the range of the medians of four rounds of seven runs, two with the runtime before Design 3 and two after; the two did not differ beyond that spread, except in the peak memory of deep):

| Case of `bench/free/` | Peak memory | Median pause, `unio run` | Median pause, native | Per MB |
|---|---:|---:|---:|---:|
| wide: 40,000 records with texts, a list of texts, and a map, and a map by name of 10,000 more | 100 MB native, 105 MB `unio run` | 41 to 53 ms | 46 to 62 ms | 0.39 to 0.62 ms |
| deep: a chain of 1,000,000 nested enum values | 229 MB native, 234 MB `unio run` | 92 to 108 ms | 91 to 118 ms | 0.33 to 0.51 ms |

Dropping a 100 MB structure at once takes about 50 ms, five times the 10 ms of the current row. The worst single run of these rounds was 74 ms for wide and 150 ms for deep. A tracing collector would not free this structure at once; its pause is the marking of what stays live, which reference counting never does. The two figures measure different events, and the row of 7.1 is written for the second one.

**A deep structure overflowed the stack.** Before this RFC, freeing was the recursion of Rust's `drop`: each level of nesting took nested calls. Measured with the runtime alone: on a thread with an 8 MiB stack, a chain of nested lists aborted the process with "stack overflow" (SIGABRT) between 120,000 and 160,000 levels, and a chain of enum values between 160,000 and 300,000; on a 256 KiB stack, 1,000,000 levels of any of them aborted. `unio run` and executables run the program on a thread with a 1 GiB stack, so the bound was far away but real: the chain of 1,000,000 links used about 48 MB of stack while it was freed (the peak was 277 MB natively before the change and 229 MB after it), 10,000,000 links still passed (2.7 GB of heap, a pause of 1.15 s), and by the measured 48 bytes of stack per link about 20 million links, some 6 GB of heap, would have aborted (an estimate; not run) instead of ending with a code of the runtime. Neither the heap limit nor the stack limit of the program (C-18) bounded it, since freeing is not a call of the program.

**Throughput.** Counting costs time where values are copied and dropped often. In `bench/README.md` native json takes 5.47 s against Go's 0.49 s; the parser of `std.json` copies and drops a heap value per character and an allocated `Option` per step, and "every copy of a value counts a reference". The remedy planned for it is reference-count elision and reuse in place (RFC 0003, step 3), not tracing. wc, text, and loop do not depend on counting: loop counts nothing (scalars are kept unboxed, docs/runtime.md 8.4), wc is faster than Go (Go takes 2.53x its time) and text takes about three times Go's time for reasons named there (dynamic dispatch and allocation of short texts). The change of Design 3 cost about 3% on native json (best of five: 6.20 s before, 6.39 s after) and nothing measurable on wc, text, and loop (best of nine: 0.207/0.214 s, 0.134/0.143 s, 0.019/0.019 s, within the noise of the shared machine); outputs are the same.

**Peak memory against Go** (`bench/README.md`, native, RFC 0003): wc 166.1 MB (Go 176.8 MB, 0.94x), text 21.5 MB (Go 13.4 MB, 1.61x), loop 5.1 MB (Go 1.6 MB, within 2x + 8 MB), json 728.4 MB (Go 168.6 MB, 4.32x). The json excess is the representation of the document as a list of one-character texts, each a heap value, not counting: counting frees each value as soon as it is unused, while a tracing collector lets the heap grow beyond the live data before it collects.

## Design

### 1. Requirements

C-13 becomes:

> C-13. Managed values are freed by reference counting: precisely (a value is freed when no name, value, or frame holds it any more), when its last reference goes away, without being moved, and without tracing, because values cannot form cycles (L-11a). Freeing a structure takes time proportional to the structure freed, never to the memory that stays live, and its use of the native stack does not depend on the depth of the structure. The pause of freeing, throughput, and peak memory are measured in the performance suite (section 7.1).

L-19 becomes "Managed values are managed by the runtime (C-13). A program cannot obtain the address of a value." S-11 and S-12 read "the heap of the runtime" for "the GC heap" and "the GC". In 7.1 the row "GC pause at a 100 MB heap | at most 10 ms (95th percentile)" is replaced by:

> | Pause of freeing a structure at once (reference counting, C-13) | at most 1 ms per MB of peak memory, median of `bench/free/` wide at about 100 MB, in `unio run` and natively; no pause depends on memory that stays live |

The bound is per megabyte because the pause of reference counting is proportional to what is freed; a fixed bound at 100 MB would forbid a program to drop its own data at once. 1 ms per MB is about twice the measured 0.4 to 0.6 ms per MB, the margin the start-up row also has (2.5 ms against 5 ms).

### 2. Concept

On acceptance, concept section 4.1 says "live on the heap and are freed by reference counting when their last reference goes away" for "live on the heap under the GC", sections 4 and 8 say "automatic memory management" or "the heap of the runtime" where they say "the GC", and a row is appended to the settled questions of section 10: "Memory management of managed values in 0.1: reference counting with copy-on-write, no tracing; values form no cycles (RFC 0007)". The system profile of section 8.1 ("work without a GC") is unchanged in meaning.

### 3. Freeing without deep recursion (implemented)

Every object of the runtime that holds values (a list or tuple, a map or set, a struct, an enum variant) hands its contents to `release` in `compiler/unio-runtime/src/value.rs` when it is dropped (`Measure::release` in `heap.rs`, `Drop` for `StructValue` and `EnumValue`). Up to 128 levels of nesting, `release` frees them by recursion as before. Below that level the contents go to a list that the frame at that level frees in a loop; an object dropped inside the loop only adds its contents to the list, so the stack is bounded by 128 levels whatever the depth. Values that hold no values and values still shared are dropped in place, without touching the list. Freeing stays immediate and complete: the loop ends before the outermost `drop` returns, the heap count is exact afterwards, and nothing is deferred.

The threshold exists because freeing every level through the list was measured about 50% slower on the wide case (with only 3% more instructions under `callgrind`, so the cost is presumably the order in which the allocator receives the blocks, a parent before its contents). With the threshold, ordinary structures are freed in the same order as before.

What is still recursive: a chain of function values each capturing the previous one is freed by the recursion of the closures of the interpreter and of native code, one level per closure; a key of a map is hashed by recursion when it is put, so a deep key overflows when it is built, before it can be freed.

### 4. Mitigation of the pause, for 0.2

The pause of about 50 ms at 100 MB can be bounded by a constant by deferred, incremental freeing: when a structure is dropped, its contents go on the list of Design 3, and the list is freed in slices of bounded work (for example 1,000 objects) at allocations and at the checks of the heap, so that no single step frees more than a slice. The list exists now; what is missing:

- The heap count would include garbage not yet freed, so the check before code 102 (C-18) must first drain the list, and the reserve of C-17 must not be spent on it.
- Peak memory rises while garbage waits, against the memory limit of RFC 0003.
- The order of freeing changes, which the measurement above shows to cost time.
- Resources are released by `using`, not by freeing (concept 4.1, docs/runtime.md 5), and cannot be stored in a value that outlives their block, so deferring cannot delay a `release`; this must stay checked.

Freeing on a background thread is not possible with the runtime as it is: values use `Rc`, not atomic counts, and the heap count is per thread (`heap.rs`), so a value cannot be handed to another thread (`Value` is not `Send`). Atomic counts would cost every copy of a value on every program, the opposite of the elision of RFC 0003.

Neither is small enough to be safe for 0.1. This RFC therefore states the measured pause and bounds it per megabyte; a later RFC may tighten the row to a constant once incremental freeing exists.

## Migration

Not needed: programs do not change. The measurement in CI changes (Verification).

## Alternatives

- **Meet C-13 as written in 0.1.** A precise tracing collector needs the roots of the interpreter and stack maps of the native code (Cranelift can emit them), and a non-moving mark and sweep. To meet 10 ms at a 100 MB live heap it must also be incremental or concurrent, with write barriers in the native code, because marking 100 MB at once takes longer than 10 ms. Copy-on-write would still need counts or another way to know that a value is unshared. It is a project of its own and cannot be checked before the release.
- **A tracing GC in 0.2, keeping C-13 for 0.2.** Possible, but nothing in the measurements calls for it: there are no cycles to collect, and the pause that remains is the freeing of what the program drops, which tracing would turn into a pause over what it keeps.
- **Perceus (RFC 0003, step 3).** Elision of counts at last use, borrowed parameters, and reuse of a released cell of the same size. It removes most of the cost of counting in json and keeps reference counting; it complements this RFC and does not change the pause of freeing a structure at once.
- **Keep the 10 ms row and report it as not met.** That would leave closure condition 3 open, since the row would measure a collector that the runtime does not have.
- **Incremental freeing in 0.1** (Design 4). Rejected for 0.1 for the reasons given there.

## Verification

- `bench/free/free.unio` and `python3 bench/run.py --only free --native [--check-free]`: builds the two cases of the table, drops each by one assignment, and times the drop between the two lines the program prints (the language has no clock), median and worst of seven runs, with the peak memory of the process; `--check-free` exits with 1 when the median wide pause is above 1 ms per MB of peak memory. The figures above are its output; the rules are in `bench/README.md`.
- Unit tests of the runtime in `compiler/unio-runtime/src/value.rs`: `deep_structures_are_freed_without_recursion` frees 1,000,000 levels of six shapes (lists, tuples, enum variants, `some`, structs, and maps) and a chain of sets with keys that hold values, on a thread with a 256 KiB stack, and checks that the heap count returns to where it was; before Design 3 it aborts with "stack overflow". `freeing_a_structure_keeps_its_shared_parts` checks that a shared value survives the structure that dropped it.
- A CI step in the job `benchmarks` on Linux x86-64, beside the start-up step: `python3 bench/run.py --only free --native --check-free --json bench/results-free.json`, with the results kept like the others. It was added on acceptance (step "Pause of freeing a structure at once (RFC 0007)").
- Closure condition 3 is met when this RFC is accepted, the requirements read as in Design 1, the CI step passes, and status row C-13 says "implemented and checked".
