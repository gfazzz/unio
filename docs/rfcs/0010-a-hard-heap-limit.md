# RFC 0010: A hard heap limit on the bytes a program requests

- Status: accepted (decided by the project lead on 2026-11-13; implemented in `compiler/unio-runtime/src/heap.rs`, `reserve.rs` and `sandbox/`, see Verification)
- Author: Unio project, with Claude
- Date: 2026-11-13
- Affects: requirements (C-18, SB-3) / runtime documentation (docs/runtime.md 3.3 to 3.6)
- Russian localization: [0010-a-hard-heap-limit.md](../ru/rfcs/0010-a-hard-heap-limit.md)

## Summary

`limits.heap_bytes` becomes a hard limit on a quantity that is defined exactly: the bytes the threads of the program request from the allocator while it runs and still hold, each block counted as its size rounded up to its alignment, the reserve for `release` included. The global allocator of the runtime counts every allocation, reallocation, and free when it happens and refuses an allocation that would exceed the limit; the refusal is exhaustion with code 102 (C-18). What the allocator adds on its own is not counted; it is stated as an exception with a measured bound and covered by a limit of the operating system that the sandbox sets behind the heap limit.

## Motivation

An external review of `a61b961` found that the heap limit checked an estimate. The runtime counted the capacities of the storage of texts, bytes, lists, tuples, maps, and sets, an estimated index of texts, the reserve, and part of the temporary buffers, on the current thread only. It did not count the reference-counted blocks of structs, enums, closures, and resources, their fields, captures, and names, the headers of `Rc`, the structures of the interpreter, or the chunks that the readers of `process.run` allocate on their own threads; and a value nested in another could be counted twice. The global allocator handled a refusal of the system but did not compare an allocation with the limit, and on Linux the sandbox of a program set no limit of the address space. A program that makes many small values ran far past its limit:

```unio
struct Point:
    x as Int
    y as Int
    z as Int

public function main():
    var points as List[Point] = []
    for i in range(0, 20000):
        points.append(Point(x = i, y = i, z = i))
```

With 20,000 points, shapes, and closures (`common::SMALL_VALUES` in `compiler/unio/tests/common.rs`) and `heap_bytes = 4_194_304`, the build of `a61b961` ran to the end; the process held about 12 MB more than with 1,000 of each. The project lead decided on 2026-11-13 that the limit must be hard.

## Why existing constructs are not enough

No language construct is involved. The normative text of C-18 and SB-3 named a heap limit without saying what it measures, so the estimate met the letter of it; the definition has to be stated for the limit to be checkable.

## Design

### The measured quantity

The heap of a program is every block that its threads request from the Rust global allocator while it runs and have not freed: values and their wrappers, vectors, maps, names, the structures the interpreter makes during the run, temporary buffers of the runtime, and the reserve for `release`. A block counts as `Layout::pad_to_align().size()`, its size rounded up to its alignment; a reallocation counts the new size in place of the old one. Each block is counted once, where it is allocated, so a value shared by several places or nested in another is not counted again.

### The count and the moment of the check

The runtime of a program owns a meter, made before anything of the program runs (before the reserve is set aside) and dropped after everything else of the runtime. The thread that runs the program counts into it with plain loads and stores; a thread the runtime starts for the program (the readers of `process.run`) joins the same meter and counts atomically, so a block allocated on one thread and freed on the other leaves the count exact. A thread that outlives its program stops counting: the meter carries a generation. Values hold `Rc`, so no value of a program can be freed on another thread.

The allocator checks every allocation and the growth of every reallocation before it asks the system. Over the limit it refuses: inside a fallible reservation of the runtime (`reserve::fallible`, `try_reserve`) the caller receives the refusal and unwinds with exhaustion; anywhere else the program ends at once with code 102 and the prepared diagnostic, as on a refusal of the system, and no user code runs. The blocks of the open reserve for `release` are not counted again, since the reserve is counted as a whole; C-17 is unchanged: on the path of a fault the reserve bounds the path whatever room the heap has.

The harness of `unio test` defers a refusal so that one test cannot end the whole run: the allocation is granted, the test is marked exhausted, and its next call or check of the heap ends it with exhaustion. A deferred refusal grants at most 1 MiB above the limit (`heap::OVERDRAFT`); beyond it the process ends with code 102.

### Exceptions

Not counted, and stated in docs/runtime.md 3.3: what the allocator adds to a block (headers, size classes, freed pages it keeps), measured at +54% for one-byte `Bytes` and between 0% and 29% for other values; memory not requested from the Rust allocator (thread stacks, code, mappings of the system); memory that is not the program's (the captured output of a test and its report); blocks made before the program starts and freed during it, which lower the count.

### The backstop of the operating system

With a heap limit, the sandbox of the program also limits the process: on Linux the soft `RLIMIT_AS` becomes the address space mapped when the sandbox applies plus twice the heap limit plus 512 MiB; on Windows a Job Object limits the committed memory of the process to what it has committed then plus the same amount. On macOS no system limit applies to `mmap`, and the count is the only bound. A refusal of the system beyond the backstop ends the program with code 102 like any refusal. Children of `process.run` and the writer process get their address space back before they run.

### Normative text

C-18 states that the heap limit of the policy is hard, on the bytes the program requests from the allocator, the reserve included, and that the bytes the allocator adds are not counted and are bounded by the backstop of the system where it has one. SB-3 names the heap size as that quantity.

## Migration

Not needed for sources. A policy keeps its keys; a program that ran close to its limit under the estimate may now end with code 102, since its wrappers count: its `heap_bytes` has to grow. The test hook `UNIO_TEST_HEAP_FREE` still fills the heap exactly, but a program now needs some room to reach a fault; the tests of the reserve leave 64 KiB.

## Alternatives

- **Keep the estimate and extend it** to the wrappers: still an estimate of what the allocator does, and every new structure of the runtime would have to remember to count itself.
- **A header in each block** that names its meter, which would make the count exact also for a block made before the program and freed during it: 16 bytes more for every value, a large share of a small value, for a case the runtime does not produce.
- **Only a limit of the operating system**: the address space is not the heap (it includes stacks, code, and reserved pages), it cannot be set on macOS, and it cannot tell code 102 from a crash of a library.
- **Refuse in `unio test` as in a program**: one test that exhausts its heap would end the whole run without a report.

## Verification

- `compiler/unio-runtime/src/heap.rs`: `every_block_is_counted_as_requested` (allocation, growth, shrinking, alignment), `an_allocation_over_the_limit_is_refused` (a fallible reservation receives the refusal; a refused block is not counted), `a_deferred_refusal_is_granted_once_and_marks_exhaustion`, `a_joined_thread_counts_into_the_program`, `outside_counts_nothing`, `the_test_hook_fills_the_heap_exactly`, `the_overhead_of_the_allocator_is_measured` (Linux, glibc).
- `compiler/unio-runtime/src/value.rs`: `the_heap_counts_each_block_once` (a shared and a nested value are counted once; a struct counts exactly its blocks), `deep_structures_are_freed_without_recursion` (the count returns to where it was after freeing chains of a million levels).
- `compiler/unio-runtime/src/sandbox/mod.rs`: `the_heap_limit_is_backed_by_the_address_space` (Linux: the soft `RLIMIT_AS`, and a block outside the count beyond it ends with code 102 and the prepared diagnostic).
- `compiler/unio/tests/interpreter.rs`: `small_structs_enums_and_closures_count_against_the_heap_limit` (`unio run` ends with 102 under 8 MiB and runs without the limit; in `unio test` the test fails with exhaustion and the next test passes); `compiler/unio/tests/native.rs`: `the_executable_counts_small_values_against_the_heap_limit` (the same in an executable, with the same output as `unio run`). The tests of the reserve (C-17) pass with 64 KiB left by the hook.
- The cost of counting, `bench/run.py --native --only wc,loop,json --check-go --runs 5` on Linux x86-64 (4 CPUs), before and after, best of 5: `wc` native 0.273 s and 0.281 s (0.51x and 0.49x of Go), `loop` native 0.023 s and 0.024 s (1.30x of Go both times), `json` native 7.80 s and 8.35 s; Python on the same machine took 5.8% longer on `json` in the second run, so the cost is within the noise of a few percent. Both gated benchmarks stay within the limit of RFC 0003 (2.0x of Go), and peak memory is unchanged (166.7 MB for `wc`, 5.3 MB for `loop`).
