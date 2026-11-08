# RFC 0005: Effects of function values are carried by calls

- Status: proposed (implemented in `unio check` and `unio symbol` for review)
- Author: Unio project, with Claude
- Date: 2026-11-08
- Affects: requirements / semantics / compiler
- Russian localization: [0005-effects-of-function-values.md](../ru/rfcs/0005-effects-of-function-values.md)

## Summary

The effects of a function value are attributed to the calls of the value, not to the place where the value is made. Making a value has only the effects of making it: evaluating what it captures and, for a lambda that captures, `allocates`. A call through a parameter of function type is a latent effect of its function; at every call of that function the latent effect is replaced by the effects of the value the caller passes. A value whose possible set is not known precisely, because it was read from a field, a collection, a pattern, or a parameter that the function keeps, has a conservative set: every value the program stores there, or every function value of the program of a type that may be its type. Under `--profile=core` and in a module of the `core` layer, a call through such a value is E0087 unless no value of the set needs more than core. Effects stay inferred (L-9b); there is no new syntax.

## Motivation

Before this RFC, docs/semantics.md 8.4 charged a function value where it was made: naming a function as a value counted as calling it, and the body of a lambda belonged to the function containing it. That rule is sound for one run, because a value is made before it is called, but it is false for the person reading the effects:

```unio
import std.files as files

function remember() returns Int:
    let reader = files.read_text
    let kept = [reader]
    return kept.length()
```

`remember` reads no file, yet it had `files.read`, and a package without the permission was E0085 for a reference it never calls. The same rule gave `List.map` no effects and the caller of `map` the effects of every lambda it wrote, wherever that lambda is called; a function that only returned a lambda had the effects of the lambda's body; and a library function holding a callback in a field was charged at the place the field was filled, often in another module than the call. The project lead accepted that rule only as a temporary restriction, if its false rejections were described and every path a value can travel was shown to be covered. This RFC replaces it.

The principle: storing a reference to `files.read_text` does not read a file; a later call does, and that call must carry `files.read` to the function that makes it. The same holds for callbacks and interface dispatch.

## Why existing constructs are not enough

No construct is added. The RFC changes how the compiler derives effects; the source text of programs does not change.

## Design

### Making a value

- Naming a function as a value has no effect of its body. It is recorded as a use of the name (for `unio symbol` and the content hashes) and adds the function to the set of function values of the program.
- A lambda is a holder of its own: what its body calls, allocates, or uses counts for the lambda. Making it has the effects of evaluating its captures, which are reads of local names, and `allocates` with the layer `alloc` when it captures a local name or `self` (semantics 8.4, "Allocation").

### Calling a value

A call through a function value has the effects of every value the callee expression can be. The compiler follows function values with a flow analysis over the forms that carry them:

| Form | Possible values |
|---|---|
| A named function `f`, `alias.f` | `f` |
| A lambda | that lambda |
| A local name | every value assigned to it (`let`, `var`, assignments) |
| A parameter of the current function or lambda | latent: what each call passes |
| A constant | the values of its initializer |
| `x.field` of a struct or error | every value stored in that field anywhere in the program: arguments of construction, field defaults, assignments |
| `f(...)` of a named `f` returning a function | every value its `return` statements give |
| `a if c else b` | the values of both branches |
| anything else: an element of a collection, a pattern binding, a result of a call through a value, `Option` contents | any function value of the program of a type that may be the expected type |

The last row is the conservative fallback. "A type that may be" compares the number and the types of the parameters, with generic parameters, unknown types and interfaces matching anything; results and errors are not compared.

### Latent effects of parameters

Inside a function, a call through a parameter `f` contributes the latent effect "the effects of `f`" instead of an effect of its own. A lambda calling a parameter of the function around it contributes the same latent effect to that function when the lambda is called inside it. At each call of the function, the latent effect is replaced by the effects of the values the argument can be, computed in the caller; a missing argument uses the default of the parameter. A latent effect that reaches the caller unresolved (the caller passes its own parameter on) stays latent in the caller, so it travels up to the call that names the value.

```unio
function apply(f as Function[(Path) -> Text fails with IoError], p as Path) returns Text fails with IoError:
    return try f(p)

function plain() returns Text fails with IoError:
    return try apply(constant_text, Path("a"))     # no effect

function reading() returns Text fails with IoError:
    return try apply(files.read_text, Path("a"))   # files.read
```

`apply` itself has no effect; `unio symbol apply` shows `argument effects: f`, and the JSON report has `"argument_effects":["f"]`. The library's higher-order functions work the same way: `List.map`, `List.filter`, `List.sorted_by`, `List.try_map` and the others have the latent effect of their callback, and their caller gets the callback's effects. An external function of the library has the latent effects of all its parameters of function type, because the runtime calls them.

A parameter that a function keeps (stores in a field, returns, captures in a lambda that is called elsewhere) escapes: where it is called, outside the function that received it, its values are every argument passed to that parameter at any call of the function, its default, and, if the function is itself used as a value, any value of the parameter's type. A parameter of a lambda gets any value of its type.

### Interfaces

A call of an interface method has the effects of the implementations that can reach it: only the method of the value's type when that type is known, otherwise every implementation of the method in the program (the union is closed, semantics 8.4). Latent effects of the implementations become latent effects of the interface method at the same parameter, so a callback passed through an interface method is resolved at the caller as for a function.

### Conservative cases and the core profile

A call through a value whose set is only estimated (a field, the fallback of the table, an escaped parameter, or a local, constant or result that holds one) is sound but may be wider than any run. In the application profile it simply has the union; E0085 then names the place: "the values stored in the field `app.Handler.run`", "any function value of type ...", "the values passed to `app.store` as `f`". Under `--profile=core`, and in a module of the `core` layer of the library, such a call is **E0087** when one of its possible values needs a layer above `core`: the call cannot be shown to stay in core. A function whose chain to `alloc` or `std` goes through such a call gets E0087 at the call instead of E0084. A call through a named function, a lambda, a parameter, or a constant holding known values is not an estimate and gives E0084 as any call does.

### False rejections of the conservative cases

The estimate can reject a program that never makes the call it fears:

1. **Fields are joined over the program.** A `Handler.run` filled with a pure `double` in one place and with `label` (which allocates) in another makes every call `h.run(1)` need `alloc`, even in a function that only ever gets the pure handler. Under the core profile that call is E0087.
2. **Collections and patterns fall back to the type.** `for step in steps: step(x)` with `steps as List[Function[(Int) -> Int]]` has every function value of the program with one `Int` parameter, including ones that were never put in a list.
3. **Escaped parameters join their callers.** A constructor helper `make(f) returns Box` that stores `f` makes every `box.f()` have the values passed to `make` anywhere.
4. **Values passed to a function used as a value.** When a function with a parameter of function type is itself passed around as a value, its parameter may get any value of that type.
5. **Generic code.** A value read where the type is a generic parameter `T` matches every function value of the program.

None of these is silent: the chain of E0085 or E0087 names the field, the parameter or the type it was estimated from, and the fix is to call a named function or a parameter, or to keep only core functions in that place.

### Why the paths are covered

A function value exists only because a function is named as a value or a lambda is evaluated, and both are recorded. A value moves only by the forms of the table: a binding, an assignment, an argument, a `return`, a field store, or an operation on a collection or a pattern. Each of them is either followed exactly or falls back to the type, whose set contains every value of the program of a type that may be the type at that place, so the value that a run calls is in the set the check uses. A latent effect is replaced at every call of its function; when the call is through a value, the arguments are not known and the escaped set is used. The analysis is a fixpoint over finite sets, so it ends.

## Migration

Not needed. Programs that were E0085 or E0084 only because they made a value they never called are accepted. A program can become rejected only through a conservative case above; in the repository no program changed (see Verification).

## Alternatives

- **Effect annotations on function types** (`Function[(Path) -> Text uses files.read]`). Declared effects would make every case precise, but L-9b keeps effects out of signatures and the word `uses` is reserved for a later decision; the annotation would also need grammar and corpus work. The inference above covers the cases of the library and the reference programs, and the estimate is visible where it is taken.
- **Keep charging the place of creation.** Sound for one run, but it describes the wrong function and rejects references that are never called; the project lead does not accept it as the final rule.
- **Type-only fallback for fields.** The first draft gave every field read the union of all function values of the field's type. Following the stores of each field is as sound (every write to a field is a construction, a default or an assignment) and much more precise: `std.files` keeps its system calls in fields, and the type-only union mixed the read and write steps of the library.
- **Context-sensitive analysis of closures.** A lambda returned from a function could keep the arguments of that call. It would remove false rejection 3 for closures, at the cost of a much larger analysis; it can be added later without changing the rule.

## Verification

- `compiler/unio/tests/symbol.rs`: a stored function value has no effect; `apply` carries the effects of its argument to the caller, with the E0085 chain `app.reading -> app.apply (argument f) -> std.files.read_text`; `List.map`, `List.sorted_by` and `List.try_map` carry the callback's effects to the caller and show `argument effects`; a call through a struct field has the stored values, and under the core profile is E0087 when one of them allocates; the body of a returned lambda counts where it is called, a capturing lambda still allocates where it is made; a constant holding a lambda and a parameter default count where they are called.
- The catalog has E0087 with an example and its fix (`compiler/unio/tests/catalog.rs`).
- `unio check` over the standard library, the examples, the tools and `spec/tests/semantic/valid` gives the same result as before: no program was newly rejected.
- docs/semantics.md 8.4 states the rule; compiler/README.md lists the remaining limits.
