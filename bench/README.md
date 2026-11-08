# Benchmarks

The performance suite of requirement 7.1 ([docs/requirements.md](../docs/requirements.md)): Unio against CPython 3.13 and, for comparison, Ruby, JavaScript (Node), Go, Java, and Rust.

```sh
cargo build --release --manifest-path compiler/Cargo.toml -p unio
python3 bench/run.py                      # every benchmark, best of 3 runs
python3 bench/run.py --only wc,text,loop --check-floor
python3 bench/run.py --native             # also the executables of unio build --release
python3 bench/run.py --native --check-go  # the limits against Go of RFC 0003
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
| tools | [../tools/](../tools/) | the project checkers in Unio against their Python references |

## Rules of the comparison

- Every language runs the same algorithm, written plainly in the style of that language, with its standard library only. For JSON, the "same algorithm" row ports `std.json` to each language; a second row uses each language's own JSON library and is shown for information.
- Times are wall clock, best of N runs, including startup. A ratio N× is the other language's time divided by Unio's: above 1 means Unio is faster.
- The floor of 7.1 is 0.5x of Python on wc, json, text, and loop; the target is 3x. `--check-floor` exits with 1 when a benchmark is below the floor.
- RFC 0003 adds limits for native executables against Go: at most 2x Go's time and at most 2x Go's peak memory plus 8 MB, with parity as the target; wc and loop are gated for 0.1, every benchmark for 0.2. `--check-go` exits with 1 when a gated benchmark is above a limit.
- Peak memory is the peak resident set size of each process, the smallest of the runs. It is measured by [peak_rss.c](peak_rss.c), a small launcher built with the system C compiler, because Linux counts the memory of the process that starts a program into the program's peak, and Python would add about 13 MB to every number. Without a C compiler, memory is not measured.

## Status

Measured on Linux x86-64 with 4 CPUs, best of 3 runs, wall time including start-up (`python3 bench/run.py --native --only wc,json,text,loop`). The ratio is the other language's time divided by the native executable's: above 1, Unio is faster.

| Benchmark | Interpreter | Native | Python | Go | Rust | Native vs Python | Native vs Go | Native vs Rust |
|---|---|---|---|---|---|---|---|---|
| wc | 0.295 s | 0.285 s | 0.351 s | 0.545 s | 0.106 s | 1.23x | 1.91x | 0.37x |
| json | 12.688 s | 6.901 s | 4.551 s | 0.720 s | 0.326 s | 0.66x | 0.10x | 0.05x |
| text | 0.189 s | 0.177 s | 0.110 s | 0.068 s | 0.066 s | 0.62x | 0.38x | 0.37x |
| loop | 0.865 s | 0.036 s | 1.062 s | 0.023 s | 0.016 s | 29.14x | 0.63x | 0.44x |

The native executables of sprint S6 (docs/runtime.md 8.4) meet the floor of 7.1 on all four benchmarks; the interpreter meets it on wc, text, and loop, and json is 0.36x. Where the time goes:

- loop: integers live in registers and every operator is one instruction with its overflow branch, so the computation itself runs as fast as Go's; about 14 ms of the 36 ms is the start, which rebuilds the declaration tables from the embedded sources. Serialised tables close the gap.
- json: the parser of `std.json` works on a list of one-character texts and a `Reader` struct passed by `var`; every character is a heap value, every method call is dispatched by the runtime by the type of the receiver, every `Option` is allocated, and every copy of a value counts a reference. What closes it, in order: the checker's types in the lowered form for static dispatch and monomorphised generics, then reference-count elision and reuse in place (Perceus), then a representation of characters without a heap value.
- text: the work is in the externals `split`, `join`, and `replace` and in building a text per line; the start is 14 ms of the 177 ms. Static dispatch of the method calls and fewer allocations of short texts would close it.

Peak memory in MB, smallest of 3 runs (`--native --check-go`):

| Benchmark | Native | Python | Go | Rust | Native vs Go |
|---|---|---|---|---|---|
| wc | 168.4 | 231.4 | 183.5 | 22.0 | 0.92x |
| json | 731.5 | 233.3 | 172.8 | 178.8 | 4.23x |
| text | 23.6 | 28.5 | 13.6 | 6.1 | 1.74x |
| loop | 6.6 | 11.2 | 1.9 | 2.2 | 3.43x, within 2x + 8 MB |

json holds the document as a list of one-character texts, each a heap value; loop carries the front end of the compiler in the executable to rebuild its tables at the start. Both are steps of RFC 0003.

The tools are 0.23x to 0.72x of their Python references in the interpreter. CI enforces the floor on the native executables for all four benchmarks and on the interpreter for wc, text, and loop, and reports the interpreter's json without failing.
