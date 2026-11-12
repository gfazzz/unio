# Benchmarks

The performance suite of requirement 7.1 ([docs/requirements.md](../docs/requirements.md)): Unio against CPython 3.13 and, for comparison, Ruby, JavaScript (Node), Go, Java, and Rust.

```sh
cargo build --release --manifest-path compiler/Cargo.toml -p unio
python3 bench/run.py                      # every benchmark, best of 3 runs
python3 bench/run.py --only wc,text,loop --check-floor
python3 bench/run.py --native             # also the executables of unio build --release
python3 bench/run.py --native --check-go  # the limits against Go of RFC 0003
python3 bench/run.py --native --only empty --check-startup  # start-up of 7.1, at most 5 ms
python3 bench/run.py --native --only free --check-free      # pause of freeing at once, RFC 0007
```

With `--native`, each Unio program is also built with `unio build --release` into `bench/build/` and measured as "Unio native"; the ratios and `--check-floor` then refer to the native executable, and the interpreter stays in its own column.

The runner uses only the Python standard library. It generates the inputs into `bench/data/` from a fixed seed (a 20 MB text and a 10 MB JSON document), builds the compiled languages into `bench/build/`, skips a language whose tools are missing, checks that every language prints the same output, and writes the table and `bench/results.json`. None of these files are committed.

## Benchmarks

| Name | Directory | Work |
|---|---|---|
| wc | [wc/](wc/) | lines, words, and UTF-8 bytes of the text |
| json | [json/](json/) | parse the JSON document and encode it again |
| text | [text/](text/) | split lines, join, replace, interpolate a record per line |
| loop | [loop/](loop/) | naive recursion and an arithmetic loop |
| empty | [empty/](empty/) | nothing: the start of an empty program |
| free | [free/](free/) | build about 100 MB of records (wide) or a chain of a million nested values (deep) and drop it at once |
| tools | [../tools/](../tools/) | the project checkers in Unio against their Python references |

## Rules of the comparison

- Every language runs the same algorithm, written plainly in the style of that language, with its standard library only. For JSON, the "same algorithm" row ports `std.json` to each language; a second row uses each language's own JSON library and is shown for information.
- Times are wall clock, best of N runs, including startup. A ratio N× is the other language's time divided by Unio's: above 1 means Unio is faster.
- The floor of 7.1 is 0.5x of Python on wc, json, text, and loop; the target is 3x. `--check-floor` exits with 1 when a benchmark is below the floor.
- RFC 0003 adds limits for native executables against Go: at most 2x Go's time and at most 2x Go's peak memory plus 8 MB, with parity as the target; wc and loop are gated for 0.1, every benchmark for 0.2. `--check-go` exits with 1 when a gated benchmark is above a limit.
- The start of an empty program (7.1: at most 5 ms) is the median of 101 runs, because one start takes a few milliseconds and a single run is noise; the best run and the spread are recorded too. It is timed by the launcher below, from its fork to the end of the program, so that the start of the launcher by Python is not counted. `--check-startup` exits with 1 when the native executable's median is above 5 ms; CI runs it on Linux x86-64.
- The pause of freeing a structure at once (plan section 12, closure condition 3; RFC 0007) is the counterpart of a GC pause under reference counting. The program of free/ prints a line, drops the whole structure by one assignment, and prints another; the runner times the drop between the two lines as they arrive on a pipe, since the language has no clock. Each case runs 7 times in `unio run` and, with `--native`, as an executable; the median and the worst are reported with the peak memory. `--check-free` exits with 1 when the median pause of wide is above 1 ms per MB of peak memory, the row of 7.1 that RFC 0007 (accepted) put in place of "GC pause at a 100 MB heap at most 10 ms".
- Peak memory is the peak resident set size of each process, the smallest of the runs. It is measured by [peak_rss.c](peak_rss.c), a small launcher built with the system C compiler, because Linux counts the memory of the process that starts a program into the program's peak, and Python would add about 13 MB to every number. Without a C compiler, memory is not measured.

## Status

Measured on Linux x86-64 with 4 CPUs (Go 1.24, Rust 1.97, Python 3.13), best of 9 runs, wall time including start-up (`python3 bench/run.py --native --runs 9 --only empty,wc,json,text,loop --check-floor --check-go --check-startup`, the first of three runs; loop was 1.37x, 1.35x and 1.39x Go in the three), with the executables that read a prepared program image at start and the code generation of docs/runtime.md 8.4. The row empty is the median of 101 runs (`--only empty --native`). The ratio is the other language's time divided by the native executable's: above 1, Unio is faster.

| Benchmark | Interpreter | Native | Python | Go | Rust | Native vs Python | Native vs Go | Native vs Rust |
|---|---|---|---|---|---|---|---|---|
| wc | 0.248 s | 0.216 s | 0.278 s | 0.547 s | 0.087 s | 1.29x | 2.53x | 0.40x |
| json | 9.580 s | 5.470 s | 3.567 s | 0.493 s | 0.275 s | 0.65x | 0.09x | 0.05x |
| text | 0.174 s | 0.147 s | 0.095 s | 0.051 s | 0.046 s | 0.64x | 0.35x | 0.31x |
| loop | 0.703 s | 0.018 s | 0.770 s | 0.013 s | 0.012 s | 43.38x | 0.73x | 0.68x |
| empty | 21.6 ms | 2.5 ms | 10.4 ms | 0.9 ms | 1.1 ms | 4.22x | 0.36x | 0.45x |

The native executables of sprint S6 (docs/runtime.md 8.4) meet the floor of 7.1 on all four benchmarks and start an empty program within the 5 ms of 7.1; the interpreter meets the floor on wc, text, and loop, and json is 0.37x. The start of an executable was 6.2 to 7.3 ms on this machine while it rebuilt the declaration tables from embedded sources (about 14 ms on the machine of the earlier measurements); reading the prepared image instead takes 2.6 ms, of which about 0.9 ms is the start of any process, as Go's shows. Where the time goes:

- loop: 17.7 ms against Go's 12.9 ms, 1.37x Go (before the changes of the code generation below: 28 to 30 ms, 2.2x to 2.3x). Of the 17.7 ms, the start is 2.5 ms (Go 0.9 ms), fib(30) about 6 ms (Go about 3.4 ms) and the loop of ten million steps about 9 ms (Go about 7.4 ms). Measured on the executables before the changes (no profiler on the machine: separate programs for fib and for the loop, and their machine code), fib(30) took about 11 ms and the loop about 11.5 ms. The cost of a call was the push and pop of the frame through memory (the depth loaded, stored, and stored back), a store of the line for every statement, three overflow branches, five saved registers, and a status check; the loop spent a store of the line per step and kept `total` in memory, because the registers that survive calls held the stack's addresses. What changed (docs/runtime.md 8.4): the depth travels in a register and is stored only before a call of the runtime, the frame is pushed with one store, the line is stored only before a call that can read it (a variable of the code, so the trace still names the statement that ran last), `n - 1` and `i * i` carry no overflow check where the bounds of a tested parameter or of the loop prove them in range, `% 7` on a value known not to be negative needs no sign correction, the loop tests its bound once per step, and a call of `fib` decides its leading `if n < 2: return n` at the call when the stack has room, so the leaves of the recursion are not called. Every check of the language stays: an overflow still faults at its line, and traces and the stack limit are those of `unio run` (`tests/native.rs`). Functions start at 64-byte boundaries: without it the time of fib moved between 10 and 22 ms from build to build with the code before it.
- json: the parser of `std.json` works on a list of one-character texts and a `Reader` struct passed by `var`; every character is a heap value, every method call is dispatched by the runtime by the type of the receiver, every `Option` is allocated, and every copy of a value counts a reference. What closes it, in order: the checker's types in the lowered form for static dispatch and monomorphised generics, then reference-count elision and reuse in place (Perceus), then a representation of characters without a heap value.
- text: the work is in the externals `split`, `join`, and `replace` and in building a text per line; the start is 2.6 ms of the 149 ms. Static dispatch of the method calls and fewer allocations of short texts would close it.

Peak memory in MB, smallest of the runs (`--native --check-go`; empty from `--only empty`):

| Benchmark | Native | Python | Go | Rust | Native vs Go |
|---|---|---|---|---|---|
| wc | 166.1 | 231.5 | 176.8 | 22.0 | 0.94x |
| json | 728.4 | 233.4 | 168.6 | 178.7 | 4.32x |
| text | 21.5 | 28.5 | 13.4 | 6.0 | 1.61x |
| loop | 5.1 | 11.1 | 1.6 | 2.2 | 3.17x, within 2x + 8 MB |
| empty | 5.1 | 11.1 | 1.3 | 2.1 | 3.92x, within 2x + 8 MB |

The executable no longer carries the parser, the checker, or the lowering, nor the sources and syntax trees of the program at run time: an empty program peaks at 5.1 MB instead of 7.6 MB and loop at 5.3 MB instead of 7.7 MB (measured on this machine with the executables of both kinds), and the executable of an empty program is 5.96 MB instead of 6.33 MB (1.68 MB instead of 2.02 MB stripped; code and data 1.69 MB instead of 2.03 MB). Of the rest, 1 MiB is the reserve for `release` (C-17), written page by page at start. json holds the document as a list of one-character texts, each a heap value; that is a step of RFC 0003.

The tools are 0.23x to 0.72x of their Python references in the interpreter. CI enforces the floor on the native executables for all four benchmarks and on the interpreter for wc, text, and loop, and reports the interpreter's json without failing; it enforces the start-up of 7.1 on the native executable of empty (the median of 101 runs, since shared runners are noisy), and enforces the limits against Go of RFC 0003 for wc and loop (best of 9 runs), reporting json and text, which are gated from 0.2.

Freeing a structure at once (`python3 bench/run.py --only free --native`, 7 runs per case; RFC 0007): the range of the medians of four rounds, two with the runtime before freeing stopped recursing below 128 levels and two after, on the same machine while other builds ran on it. The two runtimes did not differ beyond this spread, except that deep peaked at 277 MB natively before, about 48 MB of it the stack of the recursive drop.

| Case | Peak memory | `unio run` | Native | Per MB of peak |
|---|---:|---:|---:|---:|
| wide: 40,000 records and a map of 10,000 more | 100 MB (105 MB `unio run`) | 41 to 53 ms | 46 to 62 ms | 0.39 to 0.62 ms |
| deep: a chain of 1,000,000 nested enum values | 229 MB (234 MB `unio run`) | 92 to 108 ms | 91 to 118 ms | 0.33 to 0.51 ms |

The pause at 100 MB is about five times the 10 ms of the former GC-pause row of 7.1; RFC 0007 bounds it per megabyte freed (1 ms per MB) for 0.1 and leaves incremental freeing as the mitigation for later. Before the change, freeing recursed once per level of nesting: on an 8 MiB stack a chain of nested lists aborted the process between 120,000 and 160,000 levels; on the 1 GiB stack of `unio run` and executables, 10,000,000 links still passed (a pause of 1.15 s). CI runs it on Linux x86-64 with `--check-free` (step "Pause of freeing a structure at once (RFC 0007)" of the job `benchmarks`).
