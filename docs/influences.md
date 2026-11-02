# What Unio takes from other languages, and what it declines

Version: 0.1. Status: decisions recorded in `docs/concept.md` 0.11 and `docs/requirements.md` 0.10. This document keeps the reasoning in one place so that the same proposals are not re-argued.

The test for every borrowing is the principle of the concept: clear code, explicit behavior, controlled resources. A feature is taken only if it strengthens readability, explicitness, verifiability before execution, or fitness for AI. A feature that is powerful but gives a second way to write the same thing is declined.

## 1. Zig

| Idea | Decision | Where |
|---|---|---|
| Errors as values with `try` propagation and the error set in the signature | Already in place. `try` propagates, `fails with` is the contract, a call without handling does not compile, and the rule is the same in every position. Nothing lighter is needed: removing `try` would hide control flow. | concept 3.2 |
| No hidden control flow | Already in place: no exceptions, no implicit conversions, no destructors that run behind the reader's back; `release` runs only at the end of a `using` block, which is visible. | concept 1, 4.1 |
| Explicit allocator passing | Accepted for the `system` profile only. In `system`, every function from `alloc` takes an `Allocator` as its first parameter, and the allocator is a `Resource` with a lifetime in `using`. In `application`, the GC heap is the only allocator and there is no parameter: application code must not pay in readability for a need it does not have. | concept 8.1 |
| `comptime` | Declined for 0.1 and not planned as a block. Generic parameters and `let` constants of literal expressions are the only compile-time evaluation. A general `comptime` is a second way to run code, is hard to read without executing it, and violates requirement SB-14: the compiler never runs user code. If a later need appears (sizes of hardware tables, lookup tables), it is proposed as an RFC with a pure, terminating, sandboxed subset. | requirements SB-14 |

## 2. Odin

| Idea | Decision | Where |
|---|---|---|
| Explicit `Context` parameter | Declined. Unio already carries what a context would carry through visible means: the allocator as an explicit parameter in `system`, permissions through the manifest and derived effects, logging through `std.log` with a passed logger where needed. An implicit context parameter on every call is hidden state. | concept 6, 8.1 |
| Orthogonality, one way to do a thing | Already in place and enforced: the RFC process requires the section "why existing constructs are not enough", and the formatter rewrites to the one standard form. The project metric "vocabulary size" tracks it. | rfcs/README.md, plan 7 |

## 3. Gleam

| Idea | Decision | Where |
|---|---|---|
| Diagnostics understood on first reading | Accepted and made a requirement. Every error message follows one template: what is wrong, where, and how to fix it, with the fix as a patch where one exists. A message may not name compiler internals (pass names, internal types, node kinds). Every new diagnostic code is reviewed against the template and against the R-10 corpus before it is merged. | requirements R-12 |
| One style and a formatter without options | Already in place: `unio fmt` has no settings and is idempotent. | requirements T-1 |

## 4. Mojo

| Idea | Decision | Where |
|---|---|---|
| Progressive disclosure | Accepted as a documented path, not a new mechanism. The layers already provide it: write in `application` with the GC; move a hot or memory-sensitive part to code checked with `--profile=core` or, later, to the `system` profile with an explicit allocator. The guide documents this path with one example that walks down the layers and shows what each step costs and gains. | concept 8.1, requirements D-2 |
| High-level API first, buffers and backends later | Accepted for release 0.3 "Compute": the tensor API is designed for the ordinary user first, and control of buffers, placement, and backends is an explicit lower layer of the same API, not a separate one. | concept 8, plan 8 |

## 5. Unison

| Idea | Decision | Where |
|---|---|---|
| Content-addressed code | Accepted as a tool feature, not as language semantics. `unio symbol` reports a content hash for every function and type: a hash of the normalized declaration and body together with the hashes of everything it references. The hash is the key of the incremental build and test cache and the way an AI agent names an exact version of a function. Source code keeps ordinary names; nothing in the language depends on hashes. | requirements C-11a |

## 6. Declined on purpose

| Feature | Why |
|---|---|
| Ownership and borrowing in the style of Rust | The GC and `Resource` cover memory and external resources; the only ownership analysis Unio has is the check that a resource does not escape its `using` block, and it stays that small. Lifetimes in signatures would cost the readability the language exists for. |
| Macros and a heavy metaprogramming system | A second way to write everything, code that cannot be read without expanding it, and a violation of SB-14. |
| C interop out of the box | FFI for user code is outside 0.1; `external function` exists only in the standard library and in `unsafe module` files of the `system` profile. |
| Classes and implementation inheritance | Declined from the start: structs, interfaces, and composition. |
