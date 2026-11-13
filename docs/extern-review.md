# Review of the external functions of the standard library

Russian localization: [ru/extern-review.md](ru/extern-review.md).

Requirement R-5 asks that all `external function` code of the standard library be reviewed by two people and tested with memory sanitizers. This document is the checklist of that review: it lists every external function the runtime binds, with the file that declares it, the Rust function that implements it, the authority it uses, and what the reviewers check. The review itself is done by people and is recorded here; no automated check counts as one of the two reviews. Status of the record: **pending human review**: neither review has taken place (row R-5 of [status.md](status.md)).

The sanitizer part runs in CI: the workflow [sanitizers.yml](../.github/workflows/sanitizers.yml) runs the tests of the runtime crate under AddressSanitizer and MemorySanitizer, and every `test` block of the Unio sources through a `unio` built with AddressSanitizer, so that each external is called as programs call it. It uses a nightly toolchain pinned to one date, the only part of the project that does: the sanitizers of Rust exist only behind `-Zsanitizer`, and Miri cannot run the system calls of the externals.

The list is checked against the bindings of the runtime by `compiler/unio-runtime/tests/bindings.rs` (`the_review_checklist_lists_every_external`): an external added without a row, or a row without an external, fails the tests in both languages. The numbers of the rows follow the order of the names and may change when an external is added; a review refers to the name.

## How to review

Each reviewer reads the declaration (its documentation comment is the specification, [runtime.md](runtime.md) section 7), the binding in `compiler/unio-runtime/src/externs/`, and every function of the runtime it calls, and checks:

1. **No panic.** No input, however large, negative, empty, or not UTF-8, makes the binding panic, index out of bounds, overflow an integer, or divide by zero; a wrong argument is a fault with a message, a failure of the system is the declared error.
2. **Memory.** An allocation whose size depends on the program's input is counted against the heap limit before it is made and uses fallible allocation where the size is unbounded (C-18, [runtime.md](runtime.md) 3.4); a refused allocation elsewhere ends the program with code 102.
3. **Authority.** The binding uses no authority beyond the column "Authority", and checks the sandbox before every system call that uses it ([runtime.md](runtime.md) section 9).
4. **`unsafe` code.** Every `unsafe` block reached from the binding has a `SAFETY` comment that holds: foreign declarations match the system's headers on every platform, pointers outlive the call, buffers have the stated length, `errno` and return codes are read as the system defines them. The `unsafe` blocks of `externs/` at the time of writing: `clock.rs` (`getentropy`, and its counterpart on Windows), `durability.rs` (`statfs`), `files.rs` (`MoveFileExW`, `renamex_np`, `close`, `fsync`), and `system.rs` (the home directory); the bindings also reach the `unsafe` code of `sandbox/` (`fdio.rs`, `beneath.rs`, `linux.rs`, `macos.rs`, `windows.rs`, `writer.rs`) and of `value.rs` and `reserve.rs`.
5. **The focus of its kind**, the letter in the column "Focus":

- **A.** Conversions between Int and usize (negative, huge); copy-on-write (Rc::make_mut) leaves shared values unchanged; growth counted in the heap; no panic on any index.
- **B.** Indices are code points and checked (negative, past the end); slicing only at character boundaries; sizes computed from input (repeat, replace, join) checked for overflow and counted in the heap before allocation.
- **C.** Overflow, NaN, infinities, and negative zero as specified; results match IEEE 754 or the documented rounding; no panic on any input.
- **D.** No normalization that changes the meaning of a path; separators of the platform; no panic on any text.
- **E.** Unwinding opens the reserve for release and closes it again (C-17); no user code after exhaustion; identity compares the resource, not the value.
- **F.** Arguments that are not UTF-8; the list is counted in the heap.
- **G.** Only variables the manifest grants are visible; values that are not UTF-8; no allocation sized by the environment outside the heap count.
- **H.** Sandbox scope checked before the system call; the operation opens the checked path without following a link (release condition 5); descriptor ownership: no use after close, no double close; every system error becomes IoError, never a panic; temporary buffers counted in the heap before allocation.
- **I.** Depth of nesting bounded; private fields never set (E0046); type identity by declaration; no panic on any value.
- **J.** Only listed programs start; the child gets only granted variables; arguments are passed without a shell; the timeout stops the whole tree; output is capped and counted in the heap; handles and pipes are closed on every path.
- **K.** FFI declarations and their error paths (errno, short reads, the 256-byte limit of getentropy); no overflow converting nanoseconds; sleep bounded and interruptible as documented.
- **L.** Tables match the stated Unicode version; segmentation and normalization never panic and never index past a boundary; output size bounded and counted.
- **M.** Whole text written or the error reported; a closed or full stream does not panic; no unbounded buffering.

A reviewer who finds a defect opens an issue and records its number in the row instead of a name; the row is signed off when the fix is merged and reviewed again.

## The externals

118 externals.

| # | External | Declared in | Binding | Authority | Focus | Reviewer 1 | Reviewer 2 |
|---|---|---|---|---|---|---|---|
| 1 | `Bytes` | `stdlib/alloc/builtin/bytes.unio:7` | `externs/alloc.rs` `bytes_new` | none | A | pending | pending |
| 2 | `Bytes.length` | `stdlib/alloc/builtin/bytes.unio:20` | `externs/alloc.rs` `bytes_length` | none | A | pending | pending |
| 3 | `Bytes.slice` | `stdlib/alloc/builtin/bytes.unio:32` | `externs/alloc.rs` `bytes_slice` | none | A | pending | pending |
| 4 | `Bytes.to_text` | `stdlib/alloc/builtin/bytes.unio:25` | `externs/alloc.rs` `bytes_to_text` | none | B | pending | pending |
| 5 | `Float.is_finite` | `stdlib/core/builtin/float.unio:13` | `externs/core.rs` `is_finite` | none | C | pending | pending |
| 6 | `Float.is_nan` | `stdlib/core/builtin/float.unio:8` | `externs/core.rs` `is_nan` | none | C | pending | pending |
| 7 | `Float.to_bits` | `stdlib/core/builtin/float.unio:25` | `externs/math.rs` `to_bits` | none | C | pending | pending |
| 8 | `Float.to_int` | `stdlib/core/builtin/float.unio:19` | `externs/math.rs` `to_int` | none | C | pending | pending |
| 9 | `Float.to_text` | `stdlib/alloc/builtin/float.unio:6` | `externs/alloc.rs` `float_to_text_extern` | none | C | pending | pending |
| 10 | `Int.checked_add` | `stdlib/core/builtin/int.unio:18` | `externs/core.rs` `checked_add` | none | C | pending | pending |
| 11 | `Int.checked_multiply` | `stdlib/core/builtin/int.unio:28` | `externs/core.rs` `checked_multiply` | none | C | pending | pending |
| 12 | `Int.checked_subtract` | `stdlib/core/builtin/int.unio:23` | `externs/core.rs` `checked_subtract` | none | C | pending | pending |
| 13 | `Int.to_float` | `stdlib/core/builtin/int.unio:48` | `externs/core.rs` `int_to_float` | none | C | pending | pending |
| 14 | `Int.to_text` | `stdlib/alloc/builtin/int.unio:5` | `externs/alloc.rs` `int_to_text` | none | C | pending | pending |
| 15 | `List.append` | `stdlib/alloc/builtin/list.unio:21` | `externs/alloc.rs` `list_append` | none | A | pending | pending |
| 16 | `List.get` | `stdlib/alloc/builtin/list.unio:14` | `externs/alloc.rs` `list_get` | none | A | pending | pending |
| 17 | `List.length` | `stdlib/alloc/builtin/list.unio:9` | `externs/alloc.rs` `list_length` | none | A | pending | pending |
| 18 | `List.remove_last` | `stdlib/alloc/builtin/list.unio:27` | `externs/alloc.rs` `list_remove_last` | none | A | pending | pending |
| 19 | `Map.entries` | `stdlib/alloc/builtin/map.unio:33` | `externs/alloc.rs` `map_entries` | none | A | pending | pending |
| 20 | `Map.get` | `stdlib/alloc/builtin/map.unio:14` | `externs/alloc.rs` `map_get` | none | A | pending | pending |
| 21 | `Map.length` | `stdlib/alloc/builtin/map.unio:8` | `externs/alloc.rs` `map_length` | none | A | pending | pending |
| 22 | `Map.put` | `stdlib/alloc/builtin/map.unio:22` | `externs/alloc.rs` `map_put` | none | A | pending | pending |
| 23 | `Map.remove` | `stdlib/alloc/builtin/map.unio:28` | `externs/alloc.rs` `map_remove` | none | A | pending | pending |
| 24 | `Path` | `stdlib/std/builtin/path.unio:10` | `externs/system.rs` `path_new` | none | D | pending | pending |
| 25 | `Path.to_text` | `stdlib/std/builtin/path.unio:13` | `externs/system.rs` `path_to_text` | none | D | pending | pending |
| 26 | `Set` | `stdlib/alloc/builtin/set.unio:10` | `externs/alloc.rs` `set_new` | none | A | pending | pending |
| 27 | `Set.contains` | `stdlib/alloc/builtin/set.unio:21` | `externs/alloc.rs` `set_contains` | none | A | pending | pending |
| 28 | `Set.insert` | `stdlib/alloc/builtin/set.unio:29` | `externs/alloc.rs` `set_insert` | none | A | pending | pending |
| 29 | `Set.items` | `stdlib/alloc/builtin/set.unio:40` | `externs/alloc.rs` `set_items` | none | A | pending | pending |
| 30 | `Set.length` | `stdlib/alloc/builtin/set.unio:16` | `externs/alloc.rs` `set_length` | none | A | pending | pending |
| 31 | `Set.remove` | `stdlib/alloc/builtin/set.unio:35` | `externs/alloc.rs` `set_remove` | none | A | pending | pending |
| 32 | `Text.byte_length` | `stdlib/alloc/builtin/text.unio:58` | `externs/alloc.rs` `text_byte_length` | none | B | pending | pending |
| 33 | `Text.chars` | `stdlib/alloc/builtin/text.unio:63` | `externs/alloc.rs` `text_chars` | none | B | pending | pending |
| 34 | `Text.code_points` | `stdlib/alloc/builtin/text.unio:68` | `externs/alloc.rs` `text_code_points` | none | B | pending | pending |
| 35 | `Text.contains` | `stdlib/alloc/builtin/text.unio:128` | `externs/alloc.rs` `text_contains` | none | B | pending | pending |
| 36 | `Text.count` | `stdlib/alloc/builtin/text.unio:143` | `externs/alloc.rs` `text_count` | none | B | pending | pending |
| 37 | `Text.ends_with` | `stdlib/alloc/builtin/text.unio:138` | `externs/alloc.rs` `text_ends_with` | none | B | pending | pending |
| 38 | `Text.index_of` | `stdlib/alloc/builtin/text.unio:88` | `externs/alloc.rs` `text_index_of` | none | B | pending | pending |
| 39 | `Text.length` | `stdlib/alloc/builtin/text.unio:53` | `externs/alloc.rs` `text_length` | none | B | pending | pending |
| 40 | `Text.lines` | `stdlib/alloc/builtin/text.unio:154` | `externs/alloc.rs` `text_lines` | none | B | pending | pending |
| 41 | `Text.repeat` | `stdlib/alloc/builtin/text.unio:179` | `externs/alloc.rs` `text_repeat` | none | B | pending | pending |
| 42 | `Text.replace` | `stdlib/alloc/builtin/text.unio:149` | `externs/alloc.rs` `text_replace` | none | B | pending | pending |
| 43 | `Text.slice` | `stdlib/alloc/builtin/text.unio:74` | `externs/alloc.rs` `text_slice` | none | B | pending | pending |
| 44 | `Text.split_whitespace` | `stdlib/alloc/builtin/text.unio:159` | `externs/alloc.rs` `text_split_whitespace` | none | B | pending | pending |
| 45 | `Text.starts_with` | `stdlib/alloc/builtin/text.unio:133` | `externs/alloc.rs` `text_starts_with` | none | B | pending | pending |
| 46 | `Text.to_float` | `stdlib/alloc/builtin/text.unio:103` | `externs/alloc.rs` `text_to_float_extern` | none | B | pending | pending |
| 47 | `Text.to_lower` | `stdlib/alloc/builtin/text.unio:93` | `externs/alloc.rs` `text_to_lower` | none | B | pending | pending |
| 48 | `Text.to_upper` | `stdlib/alloc/builtin/text.unio:98` | `externs/alloc.rs` `text_to_upper` | none | B | pending | pending |
| 49 | `Text.trim` | `stdlib/alloc/builtin/text.unio:164` | `externs/alloc.rs` `text_trim` | none | B | pending | pending |
| 50 | `Text.trim_end` | `stdlib/alloc/builtin/text.unio:174` | `externs/alloc.rs` `text_trim_end` | none | B | pending | pending |
| 51 | `Text.trim_start` | `stdlib/alloc/builtin/text.unio:169` | `externs/alloc.rs` `text_trim_start` | none | B | pending | pending |
| 52 | `assert_fail` | `stdlib/core/prelude.unio:63` | `externs/core.rs` `assert_fail` | none (tests and identity) | E | pending | pending |
| 53 | `bytes_of` | `stdlib/alloc/builtin/bytes.unio:14` | `externs/alloc.rs` `bytes_of` | none | A | pending | pending |
| 54 | `expect_fault` | `stdlib/core/prelude.unio:68` | `externs/core.rs` `expect_fault` | none (tests and identity) | E | pending | pending |
| 55 | `is_same` | `stdlib/core/prelude.unio:75` | `externs/core.rs` `is_same` | none (tests and identity) | E | pending | pending |
| 56 | `separator` | `stdlib/std/builtin/path.unio:141` | `externs/system.rs` `separator` | none | D | pending | pending |
| 57 | `std.args.list` | `stdlib/std/args.unio:28` | `externs/system.rs` `args_list` | none (arguments) | F | pending | pending |
| 58 | `std.env.get` | `stdlib/std/env.unio:7` | `externs/system.rs` `env_get` | env | G | pending | pending |
| 59 | `std.env.home_dir` | `stdlib/std/env.unio:15` | `externs/system.rs` `env_home_dir` | env | G | pending | pending |
| 60 | `std.files.create_directory` | `stdlib/std/files.unio:457` | `externs/files.rs` `create_directory` | files.write | H | pending | pending |
| 61 | `std.files.exists` | `stdlib/std/files.unio:33` | `externs/files.rs` `exists` | files.read | H | pending | pending |
| 62 | `std.files.is_directory` | `stdlib/std/files.unio:38` | `externs/files.rs` `is_directory` | files.read | H | pending | pending |
| 63 | `std.files.list_directory` | `stdlib/std/files.unio:359` | `externs/files.rs` `list_directory` | files.read | H | pending | pending |
| 64 | `std.files.read_bytes` | `stdlib/std/files.unio:403` | `externs/files.rs` `read_bytes` | files.read | H | pending | pending |
| 65 | `std.files.read_text` | `stdlib/std/files.unio:48` | `externs/files.rs` `read_text` | files.read | H | pending | pending |
| 66 | `std.files.rename` | `stdlib/std/files.unio:445` | `externs/files.rs` `rename` | files.write | H | pending | pending |
| 67 | `std.files.resolve` | `stdlib/std/files.unio:350` | `externs/files.rs` `resolve` | files.read | H | pending | pending |
| 68 | `std.files.system_close` | `stdlib/std/files.unio:708` | `externs/files.rs` `system_close` | files.write | H | pending | pending |
| 69 | `std.files.system_open_read` | `stdlib/std/files.unio:205` | `externs/files.rs` `system_open_read` | files.read | H | pending | pending |
| 70 | `std.files.system_open_temporary` | `stdlib/std/files.unio:694` | `externs/files.rs` `system_open_temporary` | files.write | H | pending | pending |
| 71 | `std.files.system_read_chunk` | `stdlib/std/files.unio:211` | `externs/files.rs` `system_read_chunk` | files.read | H | pending | pending |
| 72 | `std.files.system_remove` | `stdlib/std/files.unio:720` | `externs/files.rs` `system_remove` | files.write | H | pending | pending |
| 73 | `std.files.system_rename` | `stdlib/std/files.unio:712` | `externs/files.rs` `system_rename` | files.write | H | pending | pending |
| 74 | `std.files.system_supports_durability` | `stdlib/std/files.unio:690` | `externs/files.rs` `system_supports_durability` | files.write | H | pending | pending |
| 75 | `std.files.system_sync_data` | `stdlib/std/files.unio:704` | `externs/files.rs` `system_sync_data` | files.write | H | pending | pending |
| 76 | `std.files.system_sync_directory` | `stdlib/std/files.unio:717` | `externs/files.rs` `system_sync_directory` | files.write | H | pending | pending |
| 77 | `std.files.system_write` | `stdlib/std/files.unio:697` | `externs/files.rs` `system_write` | files.write | H | pending | pending |
| 78 | `std.files.system_write_bytes` | `stdlib/std/files.unio:700` | `externs/files.rs` `system_write_bytes` | files.write | H | pending | pending |
| 79 | `std.json.from_value` | `stdlib/alloc/json.unio:102` | `externs/alloc.rs` `json_from_value` | none | I | pending | pending |
| 80 | `std.json.to_value` | `stdlib/alloc/json.unio:100` | `externs/alloc.rs` `json_to_value` | none | I | pending | pending |
| 81 | `std.math.acos` | `stdlib/core/math.unio:89` | `externs/math.rs` `acos` | none | C | pending | pending |
| 82 | `std.math.asin` | `stdlib/core/math.unio:84` | `externs/math.rs` `asin` | none | C | pending | pending |
| 83 | `std.math.atan` | `stdlib/core/math.unio:94` | `externs/math.rs` `atan` | none | C | pending | pending |
| 84 | `std.math.atan2` | `stdlib/core/math.unio:100` | `externs/math.rs` `atan2` | none | C | pending | pending |
| 85 | `std.math.cbrt` | `stdlib/core/math.unio:44` | `externs/math.rs` `cbrt` | none | C | pending | pending |
| 86 | `std.math.ceil` | `stdlib/core/math.unio:120` | `externs/math.rs` `ceil` | none | C | pending | pending |
| 87 | `std.math.cos` | `stdlib/core/math.unio:74` | `externs/math.rs` `cos` | none | C | pending | pending |
| 88 | `std.math.exp` | `stdlib/core/math.unio:49` | `externs/math.rs` `exp` | none | C | pending | pending |
| 89 | `std.math.floor` | `stdlib/core/math.unio:115` | `externs/math.rs` `floor` | none | C | pending | pending |
| 90 | `std.math.from_bits` | `stdlib/core/math.unio:136` | `externs/math.rs` `from_bits` | none | C | pending | pending |
| 91 | `std.math.hypot` | `stdlib/core/math.unio:110` | `externs/math.rs` `hypot` | none | C | pending | pending |
| 92 | `std.math.ln` | `stdlib/core/math.unio:54` | `externs/math.rs` `ln` | none | C | pending | pending |
| 93 | `std.math.log10` | `stdlib/core/math.unio:64` | `externs/math.rs` `log10` | none | C | pending | pending |
| 94 | `std.math.log2` | `stdlib/core/math.unio:59` | `externs/math.rs` `log2` | none | C | pending | pending |
| 95 | `std.math.power` | `stdlib/core/math.unio:105` | `externs/math.rs` `power` | none | C | pending | pending |
| 96 | `std.math.round` | `stdlib/core/math.unio:125` | `externs/math.rs` `round` | none | C | pending | pending |
| 97 | `std.math.sin` | `stdlib/core/math.unio:69` | `externs/math.rs` `sin` | none | C | pending | pending |
| 98 | `std.math.sqrt` | `stdlib/core/math.unio:39` | `externs/math.rs` `sqrt` | none | C | pending | pending |
| 99 | `std.math.tan` | `stdlib/core/math.unio:79` | `externs/math.rs` `tan` | none | C | pending | pending |
| 100 | `std.math.truncate` | `stdlib/core/math.unio:130` | `externs/math.rs` `truncate` | none | C | pending | pending |
| 101 | `std.process.run` | `stdlib/std/process.unio:48` | `externs/process.rs` `run` | process | J | pending | pending |
| 102 | `std.random.entropy` | `stdlib/std/random.unio:88` | `externs/clock.rs` `entropy` | randomness of the system | K | pending | pending |
| 103 | `std.random.secure_bytes` | `stdlib/std/random.unio:101` | `externs/clock.rs` `secure_bytes` | randomness of the system | K | pending | pending |
| 104 | `std.random.secure_int` | `stdlib/std/random.unio:120` | `externs/clock.rs` `secure_int` | randomness of the system | K | pending | pending |
| 105 | `std.text.decode_utf8_step` | `stdlib/alloc/text.unio:105` | `externs/alloc.rs` `decode_utf8_step` | none | B | pending | pending |
| 106 | `std.text.from_code_point` | `stdlib/alloc/text.unio:39` | `externs/alloc.rs` `from_code_point` | none | B | pending | pending |
| 107 | `std.text.graphemes` | `stdlib/alloc/text.unio:64` | `externs/alloc.rs` `text_graphemes` | none | L | pending | pending |
| 108 | `std.text.join` | `stdlib/alloc/text.unio:20` | `externs/alloc.rs` `text_join` | none | B | pending | pending |
| 109 | `std.text.split` | `stdlib/alloc/text.unio:26` | `externs/alloc.rs` `text_split` | none | B | pending | pending |
| 110 | `std.text.to_nfc` | `stdlib/alloc/text.unio:48` | `externs/alloc.rs` `text_to_nfc` | none | L | pending | pending |
| 111 | `std.text.to_nfd` | `stdlib/alloc/text.unio:56` | `externs/alloc.rs` `text_to_nfd` | none | L | pending | pending |
| 112 | `std.text.unicode_version` | `stdlib/alloc/text.unio:69` | `externs/alloc.rs` `unicode_version` | none | L | pending | pending |
| 113 | `std.time.monotonic_nanoseconds` | `stdlib/std/time.unio:510` | `externs/clock.rs` `monotonic_nanoseconds` | clock | K | pending | pending |
| 114 | `std.time.sleep_nanoseconds` | `stdlib/std/time.unio:514` | `externs/clock.rs` `sleep_nanoseconds` | clock | K | pending | pending |
| 115 | `std.time.wall_nanoseconds` | `stdlib/std/time.unio:512` | `externs/clock.rs` `wall_nanoseconds` | clock | K | pending | pending |
| 116 | `total_order_key` | `stdlib/core/builtin/float.unio:56` | `externs/core.rs` `total_order_key` | none | C | pending | pending |
| 117 | `write_error` | `stdlib/std/prelude.unio:18` | `externs/system.rs` `write_error` | standard streams | M | pending | pending |
| 118 | `write_output` | `stdlib/std/prelude.unio:16` | `externs/system.rs` `write_output` | standard streams | M | pending | pending |
