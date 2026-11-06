# Diagnostic review checklist

Requirement R-12: every message lives in one catalog, follows one template, and does not name compiler internals. The catalog is [unio/src/catalog.rs](unio/src/catalog.rs); [unio/tests/catalog.rs](unio/tests/catalog.rs) checks it mechanically. A new diagnostic code is merged only when every item below holds.

## The template

1. **What is wrong.** The message states the problem in a few words, starts in lower case, has no final period, and stays within 200 characters (R-11). Example: `expected ':', found the end of the line`.
2. **Where.** The span points at the smallest piece of source that shows the problem; the text format underlines it (C-7).
3. **How to fix.** A fix with a replacement is attached when the fix is unambiguous (a tab replaced by spaces, a missing `:`). Otherwise the explanation says what to write instead.

## The catalog entry

- The message is a function of `catalog.rs`, in English and in Russian; no message text is written elsewhere.
- The code is listed in `CODES`, and `entry` gives an explanation in both languages, an example that produces the code, and the same example fixed. The tests run both examples: the first must report the code, the second must be clean.
- `unio explain CODE` prints the entry (C-9).
- The code is added to `spec/README.md` and `docs/ru/spec.md` if it is a syntax code, or to the RFC or requirement that introduces it.

## No internals

A message and an explanation never name what only the compiler knows: token classes (`INDENT`, `NEWLINE`), node kinds of the syntax tree, names of grammar rules, names of passes, or Rust types. `INTERNAL_NAMES` in `catalog.rs` is the maintained list; a test fails when a message contains one of its words. When the compiler gains a new internal name that could leak (a new pass, a new tree node), add it to the list in the same change.

## Before merging

- `cargo test` passes, including `tests/catalog.rs`.
- The R-10 control set, once it exists (stage 2), is run, and the new diagnostic is fixable by an agent from its JSON form alone.
