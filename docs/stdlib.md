# Standard library: 0.1-draft

Version 0.1-draft. Russian localization: [ru/stdlib.md](ru/stdlib.md). Basis: requirements section 5, concept sections 2–4.

The standard library is written in Unio in [stdlib/](../stdlib), split into three layers. The sources are the normative API: signatures, documentation comments, and implementations. Everything the language can express is implemented in Unio; only operating-system and runtime primitives are `external function` declarations, allowed only in files marked `unsafe module` (S-7, S-10). The external functions are implemented by the runtime in [compiler/unio-runtime](../compiler/unio-runtime) (115 functions, [runtime.md](runtime.md) section 7), and a test there fails when a declaration and a binding disagree. This document is an overview written by hand; `unio doc`, which is to generate the reference from the sources (D-3), does not exist yet. What is implemented and checked, requirement by requirement, is in [status.md](status.md).

The library contains what the reference programs and the project tools use. `tools/check_api.unio` checks in CI that `docs/examples/`, `examples/`, `tools/`, and the library itself call nothing that is not declared in it (its Python reference `spec/check_api.py` gives the same result), and `unio test` runs the `test` blocks of every module in CI on Linux, macOS, and Windows. All fifteen modules of requirement S-1 exist.

Every module, public function and public constant has a documentation comment with a description and an example (S-3). An example is a block of lines indented by four spaces under a line `## Example:`; it is written as it runs inside its module, and `unio test` runs it as a test of that module (T-13), so the examples are checked like the `test` blocks. `compiler/unio-runtime/tests/documentation.rs` fails when a public function has no description or no example. The library has no manifest, so its tests and examples run without permissions: the examples of `std.files` and `std.process` show the failure paths, which are the same with and without permissions.

## Layers

| Layer | Needs | Directory | Used by |
|---|---|---|---|
| `core` | nothing: no allocator, no GC, no operating system | [stdlib/core/](../stdlib/core) | every profile, including the system profile (concept, section 8.1) |
| `alloc` | an allocator; in 0.1 the GC heap | [stdlib/alloc/](../stdlib/alloc) | applications, and system code that provides an allocator |
| `std` | the operating system | [stdlib/std/](../stdlib/std) | applications |

A file may use only its own layer and the layers before it; `spec/check_api.py` and `tools/check_api.unio` check this for imports, prelude functions, and methods. Module names do not contain the layer: `stdlib/alloc/json.unio` is `std.json`, `stdlib/std/files.unio` is `std.files`. The `prelude.unio` and `builtin/` files of all available layers together form the prelude. A type can be extended in several layers: `Int` gets its checked arithmetic in `core` and its text form, which creates new text, in `alloc`.

## Files

| File | Contents | Written in Unio | External primitives |
|---|---|---|---|
| [core/prelude.unio](../stdlib/core/prelude.unio) | `Display`, `Error`, `Equal`, `Order`, `Hash`, operator interfaces, `Resource`, `Option`, `Result`, `min`, `max`, `Range`, `RangeIterator`, `Iterator`, `range`, `assert_fail`, `expect_fault`, `is_same` | interfaces, enums, `min`, `max`, range cursors | faults, identity of resources |
| [core/builtin/int.unio](../stdlib/core/builtin/int.unio), [float.unio](../stdlib/core/builtin/float.unio), [bool.unio](../stdlib/core/builtin/bool.unio), [option.unio](../stdlib/core/builtin/option.unio) | Numbers, `Bool`, `Option` | `checked_divide`, `abs`, `checked_abs`, `sign`, `Float.abs`, `Bool.to_text`, `is_some`, `is_none`, `OrderedFloat`, `Float.total_order` | checked arithmetic, `is_nan`, `is_finite`, `Float.to_int`, `Float.to_bits`, canonical Float key |
| [core/math.unio](../stdlib/core/math.unio) | `std.math`: `PI`, `TAU`, `E`, `INFINITY`, `NAN`, elementary functions and rounding of `Float`, `clamp`, `gcd`, `lcm`, `checked_power`, `integer_sqrt`, `floor_divide`, `modulo` | the integer functions and `clamp` | the `Float` functions (IEEE 754, `libm` of the platform), `from_bits` |
| [alloc/prelude.unio](../stdlib/alloc/prelude.unio) | `Range.to_list` | `to_list` | — |
| [alloc/builtin/text.unio](../stdlib/alloc/builtin/text.unio) | Methods of `Text` | searching, splitting, trimming, `char_at`, `to_int`, `replace`, `lines` | length, characters, slices, `index_of`, case, `to_float` |
| [alloc/builtin/list.unio](../stdlib/alloc/builtin/list.unio) | Methods of `List[T]` | `find`, `map`, `filter`, `enumerate`, `contains`, `max`, `sorted`, `sorted_by`, `fold`, `partition`, `group_by`, `zip`, `chunks`, `windows`, `try_map/filter/fold`, `iterator`, `sorted_with` (merge sort) | length, `get`, `append`, `remove_last` |
| [alloc/builtin/map.unio](../stdlib/alloc/builtin/map.unio), [set.unio](../stdlib/alloc/builtin/set.unio) | `Map[K, V]`, `Set[T]`, `List.to_set` | `contains`, `keys`, `values`, `union`, `intersection`, `difference` | storage of entries |
| [alloc/builtin/int.unio](../stdlib/alloc/builtin/int.unio), [float.unio](../stdlib/alloc/builtin/float.unio) | Text form of numbers | — | `to_text` |
| [alloc/builtin/bytes.unio](../stdlib/alloc/builtin/bytes.unio) | `Bytes` | `is_empty` | construction, `length`, `to_text` |
| [alloc/text.unio](../stdlib/alloc/text.unio) | `std.text`: `join`, `split`, `from_code_point`, `to_nfc`, `to_nfd`, `graphemes`, `unicode_version` | — | all, the last four on tables of the Unicode Character Database 15.0.0 in the runtime |
| [alloc/collections.unio](../stdlib/alloc/collections.unio) | `std.collections`: `Deque`, `PriorityQueue`, `counts`, `most_common` | all | — |
| [alloc/decimal.unio](../stdlib/alloc/decimal.unio) | `std.decimal`: `Decimal`, `Rounding`, `parse`, `from_int`, `from_float` (S-1a) | all: integers of any size in base 10^9 | — |
| [alloc/json.unio](../stdlib/alloc/json.unio) | `std.json`: `Value`, `parse`, `encode`, `encode_checked`, `decode` | the parser and the writer | conversions between a type and `Value`, generated by the compiler |
| [alloc/errors.unio](../stdlib/alloc/errors.unio) | `std.errors`: `ContextError`, `context` | structured cause | — |
| [alloc/test.unio](../stdlib/alloc/test.unio) | `std.test`: `check_property`, `Counterexample` | generation, shrinking, replay data | — |
| [std/prelude.unio](../stdlib/std/prelude.unio) | `print`, `print_error` | both | standard output and error |
| [std/builtin/path.unio](../stdlib/std/builtin/path.unio) | `Path` | components, `name`, `extension`, `parent`, `join`, `normalize` | construction, text form, separator of the platform |
| [std/args.unio](../stdlib/std/args.unio) | `std.args`: program arguments, and a parser of positional arguments, flags and options with defaults and automatic help (S-4) | the parser and `help` | the arguments |
| [std/path.unio](../stdlib/std/path.unio) | `std.path`: `stem`, `with_extension`, `with_name`, `starts_with`, `relative_to`, `join_all`, `is_relative` | all, lexical | — |
| [std/time.unio](../stdlib/std/time.unio) | `std.time`: `Duration`, `Instant`, `Timestamp`, `DateTime`, `now`, `monotonic`, `sleep`, RFC 3339 text, `Clock`, `SystemClock`, `ManualClock` (`manual_clock`) | durations, the calendar, RFC 3339 | the monotonic and the wall clock, sleeping (effect `clock`) |
| [std/random.unio](../stdlib/std/random.unio) | `std.random`: `Generator` (MRG32k3a, not for cryptography), `seeded`, `from_entropy`; `secure_bytes`, `secure_int` and `RandomError`, the only API fit for keys and secrets | the generator, bounded integers, floats, `shuffled`, `choose` | entropy of the system (effect `random`); a missing source is `RandomError`, never a weaker generator |
| [std/log.unio](../stdlib/std/log.unio) | `std.log`: `Level`, `Logger`, `debug`, `info`, `warning`, `error`, `format_record` | all, to standard error | — |
| [std/env.unio](../stdlib/std/env.unio) | `std.env`: variables, home directory | — | both |
| [std/files.unio](../stdlib/std/files.unio) | `std.files`: bounded byte streams, resolution, directories, atomic writing, copying, renaming | `open_reader`, `FileReader.read_bytes/release`, `write_text`, `write_bytes`, `copy_file`, `create_directories`, `list_tree`, the commit protocol of `AtomicFile` | the individual system calls, `rename` without replacing |
| [std/process.unio](../stdlib/std/process.unio) | `std.process`: running programs without a shell, with a timeout | — | `run` |

## Rules common to all modules

- A function that can fail lists concrete error types (S-2); none uses `fails with Error`.
- An operation blocked by the sandbox ends with the declared error, with `kind = PermissionDenied` (SB-6). A function that reports absence instead of an error (`files.exists`, `env.get`) reports a blocked object as absent.
- Every faulting operation has a safe form (L-18): `items[i]` and `items.get(i)`, `a / b` and `a.checked_divide(b)`.
- Methods with `var self` change the value and are available only on a `var` place (L-11).
- The comment of a function that needs a manifest permission (T-6) names it: `Permission: files.read`.
- `a in b` calls `b.contains(a)`.
- Writing a file is atomic (S-9). `AtomicFile` replaces its target only on an explicit `commit()`, because `release` cannot know how its block ended. `commit` records each step (`Writing`, `Closed`, `Replaced`, `Durable`), so a failed commit can be repeated and `release` cleans up only what the file still owns; a failed directory sync after the rename is reported as `DurabilityUnconfirmed` (S-9c):

```unio
using file = try files.open_atomic(path):
    for line in report:
        try file.write_line(line)
    try file.commit()
```

## RFC 0001 contracts and examples

Changes from the initial 0.1-draft: the [contracts](language-contracts.md) refine behavior and layers. New implementations and `test` blocks are written in Unio, and `unio test` executes them. `FileReader`, `files.resolve`, the Float representation key (`total_order_key`), the JSON conversions generated by the compiler, and `Error.diagnostic_code` are implemented by the interpreter, the native code, and the runtime. A JSON conversion of a type with a private field is E0046, and decoding such a struct at run time fails with `DecodeError` and sets no field (S-17). Known gaps ([status.md](status.md)): `std.test` has the property-testing core, but runner integration and JSON replay (T-12) are not implemented.

| Task | Example and contract |
|---|---|
| Range | `range(0, 100)` is allocation-free core; `.iterator()` creates a cursor, `.next()` returns Option; `.to_list()` explicitly needs alloc |
| List snapshot | `items.iterator()` preserves the value when created; changing the original list does not change traversal |
| Total Float ordering | `values.sorted_by(function (x as Float) returns OrderedFloat: x.total_order())`; NaNs equal and last, both zeros equal |
| Fold | `items.fold(0, function (sum as Int, x as Int) returns Int: sum + x)`; empty lists return the initial value |
| Partition/grouping | `items.partition(predicate)`, `items.group_by(key)` preserve order; one key evaluation per item |
| Pairs/chunks/windows | `a.zip(b)` requires equal lengths; `items.chunks(2)` includes the remainder; `items.windows(2)` only complete windows; invalid sizes give none |
| Callback errors | `try items.try_map(parse_item)`, `try items.try_filter(validate_item)`, `try items.try_fold(initial, combine)` stop on the first error |
| JSON | `try json.parse(text, max_depth = 64, max_bytes = 1048576)`; `try json.encode_checked(value, pretty = true)`; duplicate keys rejected |
| Error context | `errors.context("reading configuration", cause)` preserves a typed cause; `cause.diagnostic_code()` is independent of message localization |
| Properties | `try test.check_property(generate, shrink, holds, seed = 7, cases = 100, max_shrinks = 500)` returns none or a replayable counterexample |
| Paths | `Path("link/../file").normalize()` preserves `..`; `try files.resolve(path)` accesses the OS; `components()` preserves drive/UNC identity in one anchor; `path.relative_to(file, base)` is lexical |
| Unicode | `text.to_nfc(a) == text.to_nfc(b)` compares canonically equivalent spellings; `text.graphemes(s)` keeps a family emoji or a flag whole; nothing normalizes implicitly |
| Decimal | `decimal.parse("0.1")` plus `decimal.parse("0.2")` is exactly 0.3; `a / b` is exact or a fault, `a.divide_rounded(b, 2, Rounding.HalfEven)` names the rounding; `decimal.from_float(x, 2, Rounding.HalfUp)` is the only way from Float |
| Time | `time.monotonic().since(start)` measures; `time.now()` is RFC 3339 text in UTC; a function takes `clock as time.Clock` and a test passes `time.manual_clock(...)` |
| Randomness | `var generator = random.seeded(42)` repeats on every platform and has no effect; `random.from_entropy()` differs per run; neither is for cryptography: keys come from `try random.secure_bytes(32)`, which fails with `RandomError` rather than fall back; no switch makes the system source seeded in tests, a test passes its `Generator` |
| Arguments | `args.parse(args.parser("copy", "...", [args.flag(...), args.option(..., default = some("644")), args.positional(...)]), args.list())`; `--help` gives `ArgsError` of kind `HelpRequested` with the generated text |

```unio
using reader = try files.open_reader(path):
    while true:
        match try reader.read_bytes(max_bytes = 65536):
            case some(chunk):
                consume(chunk)
            case none:
                break
```

Here `path` and `consume` are supplied by the application; the example demonstrates bounded reading, not a library function called `consume`. EOF and break do not close the resource before the end of using. Complete behavioral examples: [examples/contracts.unio](../examples/contracts.unio); other tests live beside their implementations. New APIs have explicit effects and concrete errors; generic `E with Error` is instantiated with the caller's concrete error type.
