# What Unio takes from other languages, and what it declines

Version: 0.4. Changes from 0.3: Zero (Vercel Labs), compared after a reader's question. Changes from 0.2: Ruby and Python brevity, RFC 0002. Changes from 0.1: RFC 0001 and precise cache contracts. Status: decisions recorded in `docs/concept.md` 0.14 and `docs/requirements.md` 0.13. This document keeps the reasoning in one place so that the same proposals are not re-argued.

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
| Content-addressed code | Accepted as a tool feature, not as language semantics. `unio symbol` reports a content hash for every function and type: a hash of the normalized declaration and body together with the hashes of everything it references. The hash is an input to the composite build and test cache keys (C-11b) and the way an AI agent names an exact version of a function. Source code keeps ordinary names; nothing in the language depends on hashes. | requirements C-11a |

## 6. Declined on purpose

| Feature | Why |
|---|---|
| Ownership and borrowing in the style of Rust | The GC and `Resource` cover memory and external resources; the only ownership analysis Unio has is the check that a resource does not escape its `using` block, and it stays that small. Lifetimes in signatures would cost the readability the language exists for. |
| Macros and a heavy metaprogramming system | A second way to write everything, code that cannot be read without expanding it, and a violation of SB-14. |
| C interop out of the box | FFI for user code is outside 0.1; `external function` exists only in the standard library and in `unsafe module` files of the `system` profile. |
| Classes and implementation inheritance | Declined from the start: structs, interfaces, and composition. |

## 7. Accepted findings from the extended review

Python: required labels and sorting keys; Ruby: a small complete collection API; JS/TS: task ownership and external-data validation; Swift/Rust: exclusive access and implementation coherence; Go/Unison: exact semantics, dependencies and caches; C#/Kotlin/Java/OTP: iteration, cancellation and supervision. Accepted in [RFC 0001](rfcs/0001-language-contracts.md), the [contracts](language-contracts.md), requirements L-27–L-32, C-11b, S-13–S-20, T-12–T-15 and plan §11. Concept 0.13, requirements 0.12. Earlier declined features stay declined; later features are explicitly release-bound.

## 8. Ruby and Python: brevity

| Idea | Decision | Where |
|---|---|---|
| Ruby blocks and short lambdas | Accepted in Unio form: a lambda keeps `function`, and its types come from the expected function type. A second lambda syntax (`{ \|x\| ... }`, `=>`) is declined. | RFC 0002, L-8a |
| One-line bodies (`if x: return y` in Python) | Accepted for branches only, and the formatter decides the form, so each branch has one written form. | RFC 0002, T-1a |
| Ruby 3 `case ... in` and Rust `if let` | Accepted as `expr is pattern` with names bound in the guarded branch; `match` stays the way to handle several variants. | RFC 0002, L-33 |
| Implicit return, optional parentheses, `unless`, `until`, statement modifiers | Declined: several written forms for one meaning, and control flow that is easy to miss in review. | concept 1, rationale 20 |
| `name: Text` instead of `name as Text` | Not proposed: the gain is smaller than the cost of changing every source and document. | RFC 0002, alternatives |

## 9. Swift and Python: reference counting

| Idea | Decision | Where |
|---|---|---|
| Automatic reference counting instead of a tracing collector | Accepted for the interpreter of S5. Values never share mutable state and closures capture copies, so no value can reach itself and reference counting frees everything without a cycle collector; it also tells copy-on-write when a value is unshared, and it has no collection pauses. Whether the native runtime keeps it instead of the tracing collector of C-13 is decided by the measurements of S6, and a change of C-13 needs an RFC. | runtime.md 3, C-13 |

## 10. Zero

Zero (Zerolang, Vercel Labs, Apache-2.0) is an experimental language that is also designed for AI agents. Compared from its [README at tag v0.3.4](https://github.com/vercel-labs/zerolang/blob/v0.3.4/README.md) (commit `5b3a90a`) after a reader asked; nothing here was taken from its compiler or documentation beyond that page. Two architectural ideas are declined and two are recorded as candidates for the tools; nothing extends the language, and neither candidate is a condition of release 0.1.

| Idea | Decision | Where |
|---|---|---|
| The semantic graph is the program: `zero.graph` is the compiler input, `.0` text files are projections for humans, and agents edit through `zero query` and checked `zero patch` operations | Declined. In Unio the text is the source of truth, because the program is written for a reader who is not its author and is reviewed with ordinary tools (diffs, pull requests). The corresponding tasks of navigation and analysis are solved in Unio's text model: `unio outline` for the shape of a file, `unio symbol` for a signature, effects, calls, usages, and content hash, and JSON diagnostics with fixes as patches. This does not reproduce the whole contract of graph editing: Unio edits are text ranges, not operations on semantic nodes. | concept 1, 7; C-11, C-11a |
| An edit made against a stale graph hash is refused before the store is written | Candidate for the tools. The content hashes of C-11a are the basis for such a check, not the check itself: a patch command would carry the expected hash of each declaration it changes, compare it when it applies the edit, and on a mismatch refuse the whole edit without partial changes. Unio already binds fixes to a revision in two places, and the candidate would extend the same rule to agents' patches: in the REPL `:fix N` is refused once the session has moved past the revision of the fix (T-13), and the language server names the document version in every edit, so the editor refuses it on a later text ([tools.md](tools.md)). | C-11a, T-13 |
| Capabilities passed as a value (`main(world: World)`, `world.out.write(...)`) | Declined in this form, by the model Unio chose rather than by equivalence. A capability value can be passed down and narrowed inside the program, so a callee receives only what its caller hands it; Unio has no such narrowing in 0.1. Unio declares permissions once per package in the manifest, lets only the operator narrow them further from outside (SB-3a), derives the effects of each function from its calls so a reader sees what a function reaches, rejects standard library use beyond the manifest, and enforces the manifest with the sandbox of the operating system. The cost avoided is a capability parameter on every function that reaches the system, the same cost that declined the `Context` of Odin. | concept 6; SB-3a; section 2 |
| Agent instructions shipped with the compiler and matched to its version (`zero skills`) | Candidate for the tools. Unio keeps the same material in [ai.md](ai.md): the JSON formats, the recommended loop, and prompts. Shipping it with the toolchain, matched to its version, is a proposal, not a decision. | docs/ai.md |
