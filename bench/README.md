# Benchmarks

The performance suite of requirement 7.1 ([docs/requirements.md](../docs/requirements.md)): Unio against CPython 3.13 and, for comparison, Ruby, JavaScript (Node), Go, Java, and Rust.

```sh
cargo build --release --manifest-path compiler/Cargo.toml -p unio
python3 bench/run.py                      # every benchmark, best of 3 runs
python3 bench/run.py --only wc,text,loop --check-floor
```

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

## Status

Measured on Linux x86-64 with 4 CPUs for the interpreter of sprint S5 with the lowered form of sprint S6 (`compiler/unio/src/lower.rs`). wc is 1.33x Python, loop 1.09x, text 0.56x; json is 0.39x and below the floor. The tools are 0.23x to 0.72x of their Python references. The remaining cost is in calls, method calls, and `Option` values of a tree-walking interpreter; native code of sprint S6 is the way past it. CI enforces the floor on wc, text, and loop and reports json without failing until it is above the floor.
