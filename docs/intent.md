# The intent layer: people describe, AI writes Unio

Version: 0.2 (draft). Changes from 0.1: answers to open questions 1 and 5 proposed by readers in the discussion of the article. Russian localization: [ru/intent.md](ru/intent.md).

Status: an idea for a track after release 0.1. This note records no decision: it changes no requirement, no section of the plan, and nothing in the language, and it adds nothing to the conditions of release 0.1. A decision drawn from it goes to the plan of a later release or, if it touches the language, to an RFC ([rfcs/README.md](rfcs/README.md)).

## 1. Where it came from

The idea came from the discussion of the first public article about Unio (Habr, November 2026). A reader argued that the real problem of generated code is its volume, which no person can read, and that a language of higher-order abstractions would be worth more than another general-purpose language. Unio does not make code shorter. The answer recorded here is a division of work: Unio is the lower layer; above it a person describes the intent, an AI agent translates the intent into Unio, and the checks of Unio connect the two.

The concept already points this way: natural language is used to state the task, and executable code has unambiguous semantics ([README](../README.md)). The intent layer makes stating the task a separate level that can be checked.

## 2. The loop

1. A person writes the intent: the purpose in plain words, the properties that must hold ("the sum of debits never exceeds the balance", "every order is processed once"), the interface (inputs, results, expected errors), and the permissions ("reads only this directory, no network").
2. An AI agent writes Unio: the code, `unio.toml`, the signatures, and `test` blocks.
3. The compiler and the tests check the translation.
4. When a check fails, the agent fixes the code from the JSON diagnostics, following the loop of [ai.md](ai.md); the person does not read the code.
5. The person reviews changes at the top: what changed in the intent, and what changed in the permissions and effects of the code.

## 3. What Unio already gives the link

| Part of the intent | Becomes in Unio | Checked by |
|---|---|---|
| Permissions | `[permissions]` of `unio.toml` | the compiler, which rejects standard library use beyond them (SB-7, T-6), and the sandbox of the operating system at run time (SB-1, SB-2) |
| Properties | property tests in `test` blocks | `unio test`, deterministic, with shrinking and replay (T-12) |
| Interface and expected errors | signatures with `fails with` | the compiler: an error that is neither handled nor declared does not compile ([concept.md](concept.md), 3.2) |
| What the code reaches | effects derived from the call graph | `unio symbol` (L-9b, C-11) |
| A change of the public interface | | `unio api-diff` |

When an abstraction leaks, the person goes down into code where the signatures, the effects, and the manifest show what depends on what, instead of into arbitrary generated code.

## 4. Principles

- The intent layer is a separate language, not a part of Unio. Macros and heavy metaprogramming are declined in Unio ([influences.md](influences.md), section 6), and the top layer must not bring them back through the side door.
- The translation by the AI is not trusted. Every part of the intent that can be checked becomes a check. The R-10 trial showed why: six of the eight programs that were not fixed passed `unio check` and still failed their hidden tests ([ai.md](ai.md)).
- The lower layer stays ordinary, readable Unio, reviewed with ordinary tools.

## 5. Open questions

1. **How formal is the top layer?** Free text cannot be checked, and the agent fills the gaps by guessing. A full formal specification, as in Lean or in Ada with SPARK (both raised in the same discussion), checks everything but needs specialists, which brings back a hard language for the person. The answer is likely in between: the purpose in plain words, and the properties, the interface, and the permissions in a structure that can be turned into checks mechanically. Readers proposed three refinements:
   - **Where the boundary lies.** Between a prompt for a model and machine code, the line is what can be checked mechanically: everything that can become a check is written in structure, and only the purpose stays in words.
   - **An intermediate specification.** The person does not have to write the formal part. The model translates requirements written in free text into a formal specification; the person reads and reviews only that specification, which is shorter than the code and closer to what was meant; the Unio code is then checked against the specification mechanically. The weak point moves but does not vanish: the translation from free text into the specification is also made by a model and is checked only by the person who reads it.
   - **The forms engineers already use.** The top layer need not be text. Engineers explain systems to each other with state diagrams and entity-relationship diagrams, and much of them turns into checks almost mechanically: the states become the variants of an `enum`, the allowed transitions become property tests ("an order that is paid never returns to draft"), and entities and their relations become structs.
2. **What happens to a property that cannot become a test?** It is either rewritten until it can, in the spirit of concept section 1.2, or marked as unchecked so the reader sees it.
3. **How do the layers stay in step?** When the lower layer is changed by hand or by an agent, the intent can drift from it. Zero keeps an explicit boundary between its graph and its text projections ([influences.md](influences.md), section 10); the intent layer needs an equivalent.
4. **Who checks the purpose?** The part written in plain words has no mechanical check; only the person can judge whether the code does what was meant.
5. **Does the top layer grow from what Unio already has?** Documentation comments and `test` blocks written before the code are already a small intent layer ([concept.md](concept.md), 1.2: the intention in a documentation comment, then a test, then the code). The top layer may grow from them rather than start as a new language. A reader put it more broadly: what is missing is not another language but methods and tools for structuring specifications, which today are usually encoded in the implementation. The direction of the field supports the layering: each step from assembly to C, Rust, and Python moved away from the machine, and programming in natural language would push the intermediate layer deeper still; the layer still has to be checkable for the moment an abstraction leaks.

## 6. A possible first step

Nothing is scheduled. Before any design, the intent of a few reference programs in [examples/](../examples) could be written by hand next to their Unio code, to see which parts become checks and which stay words.
