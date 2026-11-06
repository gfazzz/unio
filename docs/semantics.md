# Static semantics of Unio 0.1

Version 0.1-draft. Russian localization: [ru/semantics.md](ru/semantics.md). Basis: concept sections 2–6, requirements L-1 to L-33, [language-contracts.md](language-contracts.md), RFC 0001 and RFC 0002. This document is the reference that `unio check` implements in sprints S3 and S4 of the plan; where it and the requirements disagree, the requirements win and this document is corrected.

`unio check` runs every check on a whole package and reports every error it finds, not only the first (C-2). Each rule below names its diagnostic. Codes E0051 to E0085 are allocated here; the column "Sprint" says when the compiler starts to report them.

## 1. Packages, modules and names

1. A package is the directory with `unio.toml`; without a manifest, the directory of the file being checked is the package root. A module is a file; its name is its path from the package root with `/` replaced by `.` and without `.unio`. Names that start with `std.` are modules of the standard library (L-22).
2. A module declares names in one namespace: functions, constants, structs, enums, errors, and interfaces. Two declarations with one name in a module are E0051. Methods live in the namespace of their type; two methods with one name for one type are E0051.
3. `import a.b` binds `b`, and `import a.b as c` binds `c`, to the module; `from a.b import x, y` binds `x` and `y`. A bound name that collides with a declaration or another import of the module is E0052. A module that cannot be found is E0053; an imported name the module does not declare, or does not make public, is E0054.
4. The prelude of every available layer (S-11) is visible in every module. A declaration of the module hides a prelude name of the same spelling; `some`, `none`, `ok` and `error` cannot be declared (L-6a).
5. Local names are introduced by parameters, `let`, `var`, `for`, `using`, `on E as e`, patterns of `case`, lambda parameters, and `is` (L-33). A local name is visible from its introduction to the end of the block that introduces it; `is` bindings follow L-33. A local name may not repeat a local name that is visible at that point (E0055): a reader never has to work out which of two equal names is meant.
6. A name that resolves to nothing is E0056; the fix suggests the closest visible name.
7. Visibility follows L-23 to L-23d: a private declaration, field, or method used from another module is E0057.
8. An import cycle is E0058 and names the modules of the cycle (L-24).

## 2. Types

1. The built-in types are those of L-4. A type name with the wrong number of type arguments is E0059. Type names resolve like other names (E0056).
2. The type of a local `let` or `var` is the type of its initializer (L-8). An initializer that does not determine a type, such as `[]`, `{}` or `none` alone, needs an annotation (E0060).
3. Types are compared by name and arguments; there are no implicit conversions (L-5). An expression whose type is not the expected one is E0061; the message names both types, and for `Text + Int` the fix suggests interpolation. Integer literals have type `Int`, other number literals `Float`.
4. `Never` is the type of `fail`, `return`, `break`, `continue`, and calls of functions that return `Never`; it is accepted wherever any type is expected.
5. A value of type `T` is accepted where an interface type `I` is expected when `T` implements `I` and `I` can be used as a type (L-6b); otherwise E0061, and for an interface that cannot be used as a type E0071 names the method that prevents it.
6. `some(v)` and `none` have type `Option[T]`, `ok(v)` and `error(e)` have type `Result[T, E]`; the missing type argument comes from the expected type, and without one it is E0060.

## 3. Declarations, calls and generics

1. A struct value is built with named arguments for every field without a default; a missing or unknown field is E0063. Fields that are private to another module make the struct impossible to build there (E0057).
2. A call matches positional arguments to the leading parameters and named arguments by name. Too many or too few arguments, an unknown name, or a name given twice is E0064. Named-only parameters and function shapes follow RFC 0001 (E0040, E0041). A default value is evaluated at each call that omits the argument.
3. Generic arguments of a function are inferred from its arguments and from the expected result; when they cannot be, the call must write them (E0060). A type argument that does not meet a constraint `[T with I]` is E0062.
4. A method call `x.m(...)` finds `m` in the `extend` blocks of the type of `x` that are visible, then in the interfaces the type implements, including their default methods (L-6c). A function of a type is called as `T.f(...)` (L-23d).
5. A lambda takes the types of its parameters and result from the expected function type; where there is none, they must be written (E0049, L-8a).
6. `extend T with I` must define every method of `I` that has no default, with the signature of `I` (with `Self` replaced by `T`); a missing method or a different signature is E0069. A second implementation of `I` for `T`, or one in a module that owns neither `T` nor `I`, is E0044.
7. An enum variant whose short form collides with a visible name must be written with its enum name (L-6a): E0072.

## 4. Operators and expressions

1. `+`, `-`, `*`, `/`, `%`, `**` need the operand type to implement `Add`, `Subtract`, `Multiply`, `Divide` (L-9a); `%` and `**` are defined for `Int` and `Float`. Both operands have one type; otherwise E0065.
2. `==` and `!=` need `Equal`, `<` `<=` `>` `>=` need `Order`; `Float` follows L-32 (E0045). `==` on a resource is E0066 with the fix `is_same(a, b)`.
3. `and`, `or`, `not` take `Bool`, except `or` with an `Option[T]` left operand, which gives `T` with a `T` right operand or `Option[T]` with an `Option[T]` one. Conditions of `if`, `while`, `assert` and guards are `Bool` (E0061).
4. `x in c` calls `c.contains(x)`; `c` must have such a method (E0061).
5. `a?.f` needs `a` of type `Option[T]` and gives `Option[U]` for a field `f` of type `U`.
6. `items[i]` is defined for `List[T]` with an `Int` index and for `Map[K, V]` with a `K` key; it is a fault when the element is missing, and `get` is its safe form (L-18).
7. Every interpolated expression must implement `Display` (E0067).
8. `x is p` is `Bool`; its bindings follow L-33 (E0050).

## 5. Errors

1. A call of a function with `fails with` outside `try` is E0073, in every position (L-14).
2. Every error a `try` propagates must be listed in the `fails with` of the current function, or handled by an `on` of an enclosing `try:` (L-15); otherwise E0074 names the error and the fix adds it to `fails with`.
3. `fail x` needs `x` of an `error` type that the function lists, or that a surrounding `try:` handles (E0074).
4. The block of an inline handler must not complete normally (L-14a): E0075.
5. `try` before an expression that cannot fail is E0076; the fix removes it.

## 6. Values, mutability and control flow

1. Assigning to a name bound by `let`, a parameter, a `for` variable, or a pattern is E0077 (L-10).
2. Changing a value through a place that does not start at a `var` name, a `var` parameter or a `using` name, changing a field not declared `var`, or calling a `var self` method on such a place is E0078 (L-11).
3. A `var` parameter takes a `var` name of the caller (E0079); one name may not reach two `var` parameters of one call (E0042).
4. A function with a result whose end can be reached without `return` is E0082. `break` and `continue` outside a loop are E0083.
5. `match` must cover every variant of its subject or have `case _` (E0068). A `match` on `Float` is E0061 (L-9).
6. The result of a call that is not `Never` or nothing must be used, assigned, or returned (E0043, RFC 0001).

## 7. Resources

1. A value of a type that implements `Resource` is created only as the initializer of `using` or of a field of another resource; anywhere else it is E0080 (L-20).
2. A resource may not leave its `using` block: storing it in a struct, a collection, or a name of an enclosing block, returning it, or capturing it in a lambda is E0081.
3. A resource is not a value: it cannot be held as an interface value (L-6b) or compared with `==` (E0066).

## 8. Layers, profiles and effects

1. A module of a library layer may use only its layer and the layers before it (S-11); in a package, `unio check --profile=core` applies the same rule to every function the package defines. A violation is E0084 and lists the chain of calls from the function to the first one of a higher layer (C-21).
2. Effects are derived, not declared (L-9b). An external function has the effect of its standard-library module: `std.files` gives `files`, `std.env` gives `env`, `std.process` gives `process`, `std.net` gives `network`; any function of the `alloc` layer gives `allocates`. A function has the union of the effects of the functions it calls. `unio symbol` shows them (C-11).
3. A package that uses a function whose effect its manifest does not permit is E0085, with the chain of calls (concept section 6, T-6).

## 9. Diagnostics of this document

| Code | Rule | Sprint |
|---|---|---|
| E0051 | Two declarations, or two methods of one type, with one name | S3 |
| E0052 | An imported name collides with a declaration or another import | S3 |
| E0053 | Module not found | S3 |
| E0054 | Imported name not declared or not public in the module | S3 |
| E0055 | A local name repeats a visible local name | S3 |
| E0056 | Unknown name or type | S3 |
| E0057 | Private declaration, field, or method used from another module | S3 |
| E0058 | Import cycle | S3 |
| E0059 | Wrong number of type arguments | S3 |
| E0060 | Type cannot be inferred; an annotation or explicit generic arguments are needed | S3 |
| E0061 | Expression of a type other than the expected one | S3 |
| E0062 | Type argument does not meet its constraint | S3 |
| E0063 | Struct construction with a missing or unknown field | S3 |
| E0064 | Call with wrong arguments: count, unknown name, or a name given twice | S3 |
| E0065 | Operator not defined for the operand types | S3 |
| E0066 | `==` on a resource | S3 |
| E0067 | Interpolated expression does not implement `Display` | S3 |
| E0068 | `match` does not cover every variant | S4 |
| E0069 | Interface implementation misses a method or changes its signature | S3 |
| E0070 | Reserved | — |
| E0071 | Interface cannot be used as a type | S3 |
| E0072 | Short form of a variant that collides with a visible name | S3 |
| E0073 | Call of a function with `fails with` outside `try` | S4 |
| E0074 | Error neither handled nor listed in `fails with` | S4 |
| E0075 | Inline handler block completes normally | S4 |
| E0076 | `try` before an expression that cannot fail | S4 |
| E0077 | Assignment to an immutable name | S4 |
| E0078 | Change through a place that is not mutable | S4 |
| E0079 | Non-`var` argument for a `var` parameter | S4 |
| E0080 | Resource created outside `using` or a resource field | S4 |
| E0081 | Resource leaves its `using` block | S4 |
| E0082 | End of a function with a result reached without `return` | S4 |
| E0083 | `break` or `continue` outside a loop | S4 |
| E0084 | Layer or profile violation | S4 |
| E0085 | Effect not permitted by the manifest | S4 |

Codes E0040 to E0050 belong to RFC 0001 and RFC 0002 and are reported in the sprint of the rule they refine. Every code has an explanation with an example and a fix in the catalog of the compiler (C-9, R-12).
