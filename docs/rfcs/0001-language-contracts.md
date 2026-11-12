# RFC 0001: Precise language and library contracts

- Status: accepted (specification, grammar and library sources; implemented in the compiler: E0040 to E0048 are reported by `unio check` and the library contracts run in `unio test`; E0048 covers only a missing dependency, since packages do not exist (T-14); what remains partial is listed in [status.md](../status.md), rows L-28 to L-32, S-13, S-15, S-17, C-11a, C-11b)
- Author: Unio project, with Codex
- Date: 2026-11-03
- Affects: concept / requirements / grammar / standard library / tools
- Russian localization: [0001-language-contracts.md](../ru/rfcs/0001-language-contracts.md)

## Summary

Adopt the useful findings of the comparative language review. The project owner's instruction to implement those findings authorizes this stage-0 design update. The normative contracts are in [language-contracts.md](../language-contracts.md); requirements reference them explicitly. This RFC records changes to settled decisions as well as the new contextual keyword `named`. Acceptance does not mean that an interpreter, compiler, package manager or operating system has been implemented.

## Motivation

Named arguments are optional even when positional flags are unreadable. `Float` promises IEEE comparisons but the existing `Order.compare` cannot represent unordered NaN. `range` allocates a complete list, `sorted_by` repeatedly evaluates its key, automatic JSON conversion has no boundary with private constructors, and recursive source hashes have no defined cache identity. These are observable gaps in the existing design.

## Why existing constructs are not enough

A style convention cannot enforce a required argument label; structs of options are excessive for one flag. `named timeout as Int` puts the contract in the signature, including function values. An `Int` result from `Order.compare` has only three comparison outcomes and cannot preserve IEEE unordered behavior. An explicit `OrderedFloat` wrapper preserves ordinary numeric comparison and provides a lawful sorting/key type. A list cannot represent a large range in constant memory. Existing structs, interfaces and `using` suffice for cursors and streams; no `yield`, `async`, macros, inheritance or dynamic casts are introduced.

## Design

`param` gains `"named" NAME "as" type ["=" expression]`. `function_shape` uses `shape_parameter = "named" NAME "as" type | type`. The modifier is contextual: `named as Int` remains an ordinary parameter named `named`. Named-only parameters follow ordinary parameters and cannot be `var` in 0.1; defaulted parameters trail within each group. Function values preserve labels, but not default arguments. Calls must supply labels and satisfy their declared shape. Lambdas obey the same rule.

```unio
function connect(host as Text, named port as Int = 8080) returns Text:
    return "{host}:{port}"

function apply(callback as Function[(Text, named port as Int) -> Text]) returns Text:
    return callback("localhost", port = 8080)
```

Existing positional-or-named parameters remain available. A new option should use `named`; existing APIs migrate only with a diagnostic and fix. Rename tools treat labels as public API, including calls through function values. Changes to Float, ranges, package imports, symbol hashing, decoding, path normalization and other contracts are detailed in the normative document rather than duplicated here.

## Migration and diagnostics

Stage-2 diagnostics reserved by this RFC:

| Code | Contract violation | Fix or explanation |
|---|---|---|
| E0040 | Missing named-only argument label | Insert the known label at the call; do not guess an ambiguous binding |
| E0041 | Invalid parameter grouping, defaults or named function shape | Show the required signature and offending parameter |
| E0042 | Overlapping exclusive accesses | Materialize a value argument or split the calls |
| E0043 | Discarded required result | Handle it, return it, or explicitly bind `let _ = ...` |
| E0044 | Foreign or overlapping interface implementation | Name both owners/implementations; suggest a local wrapper |
| E0045 | IEEE Float used where total Order is required | Use an explicit `total_order()` key |
| E0046 | Automatic conversion would bypass private fields | Decode a public transport struct, then call its validating constructor |
| E0047 | Range used as List | Insert `.to_list()` where materialization is intended |
| E0048 | Missing or conflicting dependency identity | Show alias, origin, version constraints and lock entry |

These codes describe future compiler checks; the grammar checker does not implement semantic checks. `Path.normalize` now preserves `..`; callers needing filesystem resolution use `files.resolve`. JSON duplicate keys now fail instead of silently overwriting. `parse` has documented size/depth defaults, overridable through named arguments. Existing `encode` retains its documented lossy non-finite behavior; `encode_checked` is the required choice for strict configuration/protocol output. Public transport fields in the `todo` examples are made explicit. Numeric Float comparisons stay IEEE; generic total-order APIs require `OrderedFloat` keys.

## Alternatives

Mandatory labels for every argument would make obvious positional data cumbersome. Python's `*` delimiter adds punctuation without explaining intent. Changing IEEE operators to total order would surprise numerical users. Full borrowing, effect handlers, multiple dispatch and macro systems are not justified by these findings and remain outside this update. Multi-statement lambdas and physical-units algebra require evidence from real examples before a separate RFC.

## Verification

- Grammar: valid named declarations, lambdas/function shapes and contextual names; invalid missing types/labels. Both grammar tools read the same EBNF.
- API tools: preserve the binding name after the contextual modifier in Python and Unio versions; check a callback that is itself named-only.
- Library `test` blocks: sorting stability/singletons, collection boundaries and short circuit, fallible callbacks, exhausted cursors, extreme range bounds, duplicate JSON keys, limits, Unicode scalars, lexical paths, structured causes and deterministic property shrinking.
- Future stage-2/3 acceptance: diagnostics E0040–E0048, exact key-call counts, error/effect propagation, early resource release, decoder privacy, IEEE NaNs, source-hash cycles and package conflicts, as listed in the normative document.
- Static grammar/API checks do not execute these behavioral tests. Runtime status remains pending until stage 3.
