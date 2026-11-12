# RFC 0002: Brevity without implicitness

- Status: accepted (specification, grammar, corpus, editor highlighting, and the compiler: lambda parameter types from context with E0049, one-line branches in the parser and the formatter, `expr is pattern` with E0050; [status.md](../status.md), rows L-8a, L-33, T-1a)
- Author: Unio project, with Claude
- Date: 2026-11-06
- Affects: concept / requirements / grammar / tools
- Russian localization: [0002-brevity-without-implicitness.md](../ru/rfcs/0002-brevity-without-implicitness.md)

## Summary

Three changes remove the largest sources of noise in Unio code without hiding behavior: types of lambda parameters are taken from the expected function type, a branch can hold one simple statement on its header line, and `expr is pattern` tests a pattern in a condition and binds its names in the guarded branch. The new keyword is `is`.

## Motivation

A review of about 4,000 lines of Unio (the reference programs, the standard library, and the tools) found three patterns that make code long without making it clearer:

```unio
tasks.map(function (task as Task) returns Int: task.id)

match env.get("TODO_FILE"):
    case some(value):
        return Path(value)
    case none:
        return home.join(".todo.json")

match fields.get(key):
    case some(child):
        return child
    case none:
        pass
fail QueryError(message = "no key \"{key}\"")
```

The types of the lambda are already fixed by `map`; the branches of the `match` are one statement each; the last `match` exists only to test one variant.

## Why existing constructs are not enough

The lambda cannot be shortened without inference: the only alternative is a named function, which moves the logic away from its use. One-line branches cannot be expressed: every block needs its own line. Testing one variant needs a full `match`, often with an empty `case none: pass`. No combination of existing constructs is shorter, and each of the three appears in almost every file of the repository.

## Design

1. **Lambda parameter types (L-8a).** `lambda_param = "named" NAME [ "as" type ] | NAME [ "as" type ]`. Types and the result may be left out when the lambda is an argument, an initializer with a declared type, or a returned value whose function type is known; then they are taken from that type. Everywhere else they are required (E0049). Parameters of named functions are always annotated, as before.

   ```unio
   tasks.map(function (task): task.id)
   ```

2. **One-line branches (T-1a).** `branch_block = simple_statement NEWLINE | block`, used by `if`, `else if`, `else`, `case`, and `on`. Functions, loops, `using`, `test`, `try:`, and handled statements keep a block. The formatter writes a branch of one simple statement on its header line when the line fits 100 characters and on the next line otherwise; a branch therefore still has exactly one written form.

   ```unio
   match env.get("TODO_FILE"):
       case some(value): return Path(value)
       case none: return home.join(".todo.json")
   ```

3. **Pattern test (L-33).** `comparison = additive [ comparison_op additive | "is" pattern ]`. `x is some(v)` is a `Bool`. Its names are bound in the branch the test guards: the body of `if` or `else if`, the body of `while`, and the right operand of `and` that follows the test. They are not bound under `or`, under `not`, or after the branch; using them there is E0050. A pattern test does not replace `match`: a value with several variants to handle is still matched completely.

   ```unio
   if fields.get(key) is some(child): return child
   fail QueryError(message = "no key \"{key}\"")
   ```

## Migration and diagnostics

Existing code stays valid. The formatter moves one-statement branches onto their header line. Two diagnostics are reserved for stage 2:

| Code | Violation | Fix or explanation |
|---|---|---|
| E0049 | A lambda parameter without a type where no function type is expected | Add the type, or pass the lambda where its type is known |
| E0050 | A name bound by `is` used where it is not bound | Use it inside the guarded branch, or use `match` |

## Alternatives

- `name: Text` instead of `name as Text`: shorter, but every source and document changes, and the gain is smaller than these three changes. Not proposed.
- Ruby blocks `{ |x| ... }` and `=>` lambdas: a second lambda syntax. Declined; the existing `function` form with inferred types is close enough.
- Implicit return, optional parentheses, `unless`, statement modifiers (`return if x`): several written forms for one meaning, against principles 1 and 2 of the concept. Declined.

## Verification

Corpus files `valid/brevity.unio`, `invalid/inline_body_on_loop.unio`, `invalid/inline_body_on_function.unio`, `invalid/is_without_pattern.unio`; the highlighting tests of `editors/vscode` cover `is`, one-line branches, and lambdas without types, and fail for a keyword of the grammar that is not highlighted.
