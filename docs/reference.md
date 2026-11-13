# Unio language reference

Version 0.1-draft. Russian localization: [ru/reference.md](ru/reference.md). Requirement D-1: the grammar in EBNF, the semantics of every construct, and the list of diagnostic codes.

This document is the entry point to the definition of the language. It goes through the grammar construct by construct and says, for each, what it means: what `unio check` requires of it before a program runs and what it does when it runs. The normative texts it summarizes stay where they are, and where this reference and one of them disagree, they win and this document is corrected:

| Part | Normative text |
|---|---|
| Syntax, including the lexical rules | [spec/grammar.ebnf](../spec/grammar.ebnf), with its checker and corpus in [spec/README.md](../spec/README.md) |
| Static semantics (what `unio check` verifies) | [semantics.md](semantics.md), sections 1–8 |
| Dynamic semantics (what a run does) | [runtime.md](runtime.md), sections 1–6 |
| Requirements of the language | [requirements.md](requirements.md) L-1 to L-33, [language-contracts.md](language-contracts.md) sections 1–2 |
| Language changes after stage 1 | [rfcs/README.md](rfcs/README.md); RFC 0001 (named parameters and contracts), RFC 0002 (brevity without implicitness) |
| The one style of a source file | [style.md](style.md) (`unio fmt`) |
| The standard library | [stdlib-reference.md](stdlib-reference.md), generated from the sources in `stdlib/`, and the overview [stdlib.md](stdlib.md) |
| Packages and manifests | [packages.md](packages.md) |

Every rule of the grammar is named below in backquotes, as `function_decl`, under the section of the grammar that defines it. Two checks keep this document complete: the CI step "Generated references are up to date" runs `unio run tools/reference.unio -- --check`, which fails when a rule of `spec/grammar.ebnf` is not named here or when the table of section 8 does not list exactly the codes of the compiler's catalog (`compiler/unio/src/catalog.rs`). How the language is used is in the guide, [guide.md](guide.md).

## 1. Lexical structure (grammar section L)

A source file is UTF-8 text with the extension `.unio` (L-1). Line ends are LF or CRLF; the repository keeps LF.

**Lines and blocks.** A logical line is a line with at least one token. A block is a run of lines indented by exactly four spaces more than the line that opens it, and that line ends with `:`. Indentation is made of spaces only: a tab is E0001, an indentation that is not a multiple of four is E0002, and an indentation that grows by more than one level or after a line that does not end with `:` is E0003. Inside `( )`, `[ ]` and `{ }` line ends and indentation are ignored, so a long call or literal continues over several lines; a bracket left open is E0009 and a closing bracket that does not match is E0010. Blank lines and lines with only a comment produce nothing.

**Comments.** `#` starts a comment to the end of the line. `##` starts a documentation comment, which attaches to the declaration that follows it (L-3); `unio symbol` shows it, and the standard library reference is generated from it. A run of `##` lines at the top of a file, followed by a blank line or by `unsafe module`, documents the module. Comments are not tokens: they never change what a program means.

**Names.** `letter` is an ASCII letter and `digit` an ASCII digit; a `NAME` is a letter or `_` followed by letters, digits and `_`, and is not a keyword. Other characters outside texts and comments are E0007. By convention functions, variables, fields and modules are `snake_case` and types are `PascalCase`.

**Keywords.** The fixed keywords of 0.1 are `and as assert break case continue else enum extend fail fails false for from function if import in interface is let match not on or pass public return returns self struct true try using var while with`; adding one needs an RFC (L-2). The contextual keywords `error external module named test unsafe` are ordinary names except where a rule uses them: at the start of a declaration, and `named` before a parameter. `uses` is reserved for a later release (E0032).

**Numbers.** An `INT` is a `decimal_int` (`1_000`), a hexadecimal number `0xFF` made of `hex_digit` characters, or a binary number `0b1010`; `_` separates digits. A `FLOAT` has a fraction, an `exponent`, or both (`2.5`, `1e-3`, `6.02e23`). A number followed directly by a letter is E0008. An `INT` has the type `Int`, a 64-bit signed integer; a `FLOAT` has the type `Float`, an IEEE 754 binary64 number. An integer literal must fit in `Int`, at most `9223372036854775807`; one that does not is E0008. The smallest `Int` is therefore written `-9223372036854775807 - 1` (the constant `MINIMUM_INT`).

**Texts.** A `TEXT` is written between `"` on one line (E0005 otherwise). A `text_char` is any character but `"`, `\`, `{`, `}` and a line end. An `escape` is `\n`, `\t`, `\r`, `\0`, `\\`, `\"`, `\{`, `\}`, or `\u{...}` with one to six hexadecimal digits of a Unicode scalar value; any other escape is E0006. An `interpolation` `{expression}` puts the text form of the expression into the text: the expression is evaluated when the text is, left to right with the other parts, and its type implements `Display` (E0067). An empty or unclosed interpolation, or a `}` without `\`, is E0011. A text value is an immutable sequence of Unicode scalar values (S-18).

**Punctuation.** `( ) [ ] { } , : . ?. ... = == != < <= > >= + - * / % ** -> |`. Nesting deeper than 64 levels of expressions, patterns, types, blocks or interpolations is E0013.

## 2. Files and declarations (grammar section F)

**`file`.** A file is a module (L-22): an optional `module_header`, then `top_item`s, each an `import`, a `declaration`, an `extend_decl` or a `test_block`. Its name is its path from the package root with `/` replaced by `.` and without `.unio`; the package root is the directory of the nearest `unio.toml`. Top-level items may come in any order, and a name may be used before the line that declares it.

**`module_header`.** `unsafe module` marks a module that is listed for review (T-6, T-9, RFC 0004). In the standard library it is the only place where `external_decl` may appear (S-7). In a package the marker grants nothing: no external function (E0086), no permission, no other rule.

**`import` and `module_path`.** `import a.b` binds `b` to the module `a.b`, `import a.b as c` binds `c`, and `from a.b import x, y` binds the public names `x` and `y` of the module. A `module_path` starting with `std.` names the standard library, `deps.<alias>.<module>` a dependency locked in `unio.lock` (T-14), and any other path a module of the same package (`a.b` is the file `a/b.unio`). Importing a module also makes its public `error` types visible by their short names. Errors: a module that cannot be found or that a link leads out of the package (E0053), a name the module does not declare publicly (E0054), a name that collides (E0052), an import cycle (E0058), a `deps.` path that the manifest, the lock or the cache does not provide (E0048). Semantics 1.3, 1.8, 1.9.

**`declaration`.** A function, an external function, a struct, an enum, an error, an interface or a constant, optionally preceded by `public`. Everything is private to its module unless it is `public` (L-23); a private declaration used from another module is E0057. A module declares each name once (E0051); a declaration hides a prelude name of the same spelling, but not a built-in type (semantics 1.4).

**`function_decl` and `function_sig`.** `function name[T](item as T, count as Int) returns R fails with E1, E2:` followed by a `block`. Without `returns` the function returns nothing; with it, every path ends in `return` or `fail` (E0082). `fails with` lists the errors the function can end with (L-13); its body may let only those escape (E0074), and every caller must write `try` or handle them (E0073). A call evaluates the receiver and the arguments once, left to right in the order they are written, then the defaults of the omitted parameters in declaration order (L-27, runtime 1.1, 1.3). Functions are values: a function named without a call is a value of type `Function[(...) -> T]`.

**`external_decl`.** `external function f(...) returns T` declares a function the runtime implements, without a body. Only an `unsafe module` of the standard library may declare one (E0086); its documentation comment is its specification (runtime 7).

**`generic_params` and `generic_param`.** `[T, K with Hash and Equal]` declares type parameters of a function, struct, enum, interface or `extend`. `with I and J` constrains the parameter to types that implement the interfaces; a type argument that does not is E0062 (E0045 for a type built from `Float` where `Order` or `Hash` is required). Type arguments are inferred from the arguments and the expected type, or written at the call (`generic_call`); when they cannot be inferred it is E0060. A generic function is resolved at compile time, not dispatched (L-7).

**`params` and `param`.** A parameter is `name as Type`, optionally with a default `= expression`. `var name as Type` lets the function change the caller's variable: the caller passes a `var` name (E0079), and one variable may not reach two `var` parameters of one call (E0042, L-29). `named name as Type` is a named-only parameter: every call writes its label (E0040). Ordinary parameters come before named-only ones, and defaults trail within each group (E0041, RFC 0001). `self` or `var self` first makes a function in an `extend` block a method; `var self` methods change the receiver and need a mutable place (E0078). A default is evaluated afresh at each call that omits the argument and sees the parameters before it (semantics 3.2).

**`struct_decl`, `field_block` and `field`.** A struct is a record with named fields, built as `Task(id = 1, title = "x")` with a named argument for every field without a default (E0063). A field is `name as Type`, optionally `var` (it may change through a mutable place) and `public` (visible outside the module; L-23a), with an optional default evaluated at each construction that omits it. A struct with a private field can be built and changed through that field only in its module. Structs are values: equality is by content when every field has `Equal` (L-9), and binding, passing and storing copy the value as far as anyone can observe (L-11a; runtime 2.4 shares it until it is changed).

**`error_decl`.** `error Name:` with fields declares an error type, which implements `Error` and `Display` with a generated `to_text` and a stable `diagnostic_code` (semantics 2.7, S-20). Its fields are visible wherever the type is (L-23b). An error without fields is written `error Name` on one line. An error value is raised by `fail` and received by `on`.

**`enum_decl`, `variant` and `variant_field`.** An enum is a closed sum type: a value is exactly one `variant`, which may carry named fields (`Add(title as Text)`). A variant is built as `Command.Add(title = "x")` or by its short name `Add(...)` where that name collides with nothing visible (L-6a, otherwise E0072); a variant with fields may also be built with positional arguments in field order. Variants are tested by `match`, `is`, and patterns. Equality is by content.

**`interface_decl` and `interface_member`.** An interface is a set of method signatures (`function_sig`). A member with a `block` has a default implementation, which reaches the value only through the methods of the interface (L-6c). Inside an interface `Self` is the implementing type. An interface is a constraint (`[T with Shape]`, resolved at compile time) and a type: a value of type `Shape` holds a value of any type that implements it, and a call on it is dispatched at run time to that type's method (L-6b). An interface whose methods use `Self` outside the receiver or have type parameters cannot be a type (E0071). An interface value has no `==`. A resource cannot be an interface value.

**`extend_decl` and `member`.** `extend T:` adds methods (with `self`) and functions of the type (without `self`, called as `T.f(...)`, L-23d) to a type of the same module; such a member is private unless `public` (L-23c). `extend T with I:` implements the interface `I` for `T`: it defines every method of `I` without a default, with the signature of `I` (E0069), and may replace defaults. Its methods are visible wherever `T` and `I` both are, and `public` is not written there. A type implements an interface once, in the package of the type or of the interface (E0044, L-31): any module of that package may hold it. Operators come from such implementations: `extend Money with Add` gives `+` (L-9a). `extend[T] List[T]:` extends a generic type with type parameters of its own.

**`constant_decl`.** A top-level `let NAME as Type = expression` declares a module constant. The type may be left out only for a private constant whose initializer determines it (L-8). Constants are evaluated once, in dependency order, before `main` or before the first test that needs them; a fault in an initializer ends the program (runtime 6.3). There is no global mutable state: a top-level `var` does not exist.

**`test_block`.** `test "description":` with a block is a test. `unio test` runs every test of the given modules, each in a fresh state and in isolation; `unio run` and `unio build` leave tests out (L-25, T-4). A test passes when its block ends normally and fails on a fault (an `assert` that does not hold, `assert_fail`) or when it does not compile, which is reported for that test alone (T-5a). `expect_fault(action)` succeeds only when the action ends with a fault (T-5).

## 3. Statements (grammar section S)

**`block` and `statement`.** A block is one or more statements, executed in order. An empty block is a syntax error; `pass` is the statement that does nothing (L-2a). A `statement` is a `simple_statement` on its own line or a compound statement with a block.

**`simple_statement`.** A `let_statement`, an `assignment`, a `return_statement`, a `fail_statement`, an `assert_statement`, `break`, `continue`, `pass`, or an `expression` evaluated for its effect. An expression statement may drop the value of a call, except a `Result` or a `std.process.Output`, which must be used or dropped with `let _ = ...` (E0043, L-30).

**`let_statement`.** `let name = expression` binds an immutable name; `var name = expression` binds a variable that may be assigned and changed (L-10). `as Type` annotates the name; without it the type is the type of the initializer, and an initializer that has none of its own (`[]`, `{}`, `none`) needs the annotation (E0060). A local name may not repeat a visible local name (E0055); it is visible from the next statement to the end of its block. `let _ = expression` evaluates the expression and drops its value explicitly.

**`assignment` and `place`.** `place = expression` stores a value. A place is a name, a field `a.b` or an element `a[i]` (E0030 otherwise) and starts at a `var` name, a `var` parameter or a `using` name (E0077, E0078); a field changes only if it is declared `var`. The destination, its indexes included, is resolved first, then the right side is evaluated, and the value is stored only when that succeeds (L-27). Changing an element of a collection changes the collection: `tasks[index].done = true` (L-11). Assigning to a missing element of a list is a fault; assigning to a key of a map inserts or replaces the value, and a part of a value inside a map changes through its key, `books[id].borrower = member`, which is a fault when the key is missing.

**`return_statement`.** `return expression` ends the function with its result; a bare `return` ends a function without a result. Open `using` blocks are released first.

**`fail_statement`.** `fail expression` ends the function with an error: the expression has an `error` type that the function lists in `fails with` or that an enclosing `try:` handles (E0074). Its type is `Never`.

**`assert_statement`.** `assert condition` does nothing when the `Bool` condition is true and is a fault with the text of the condition when it is false (runtime 4.4). It is meant for tests and for invariants that a correct program never breaks; an expected failure is an error, not a fault.

**`break`, `continue`, `pass`.** `break` leaves the innermost `for` or `while`, `continue` starts its next iteration, and both outside a loop are E0083. Leaving a loop releases the `using` blocks opened inside it.

**`if_statement`.** The conditions are `Bool` (E0061) and are evaluated in order until one is true; its branch runs, or the `else` branch when none is. A name bound by `is` in a condition is visible in that branch (L-33).

**`for_statement`.** `for item in collection:` evaluates the collection once and runs the block for each of its items in order: a `List` from first to last, a `Range` from start to end, a `Set` in insertion order; the loop takes one cursor from `iterator()` and advances it once per iteration (S-15), so changing the source variable inside the loop does not change the iteration. With several names, `for index, item in items.enumerate():` takes each item apart as a tuple. A map is visited as its `(key, value)` pairs, `for name, age in ages:`, in the order its keys were first inserted; `keys()` and `values()` give one side. The loop names are immutable (E0077).

**`while_statement`.** `while condition:` evaluates the `Bool` condition before each iteration and runs the block while it is true. A name bound by `is` in the condition is visible in the block.

**`match_statement` and `case`.** `match subject:` evaluates the subject once and tries the cases in order; the first case whose `pattern` matches and whose `if` guard, evaluated with the names of the pattern, is true runs its branch, and no other case runs. The cases cover every value of the subject, or the last is `case _`; guarded cases do not count toward that (E0068, which names the missing cases). A subject of type `Float` is E0061, since `Float` has no patterns (L-9). Names bound by the pattern are immutable and visible in the guard and the branch.

**`try_statement` and `handler`.** `try:` runs its block, where a call of a function with `fails with` needs no `try` prefix. When an error leaves the block, the first `on Type as name:` handler whose type is the error's type, or `Error` for any error, runs with the error bound to the name, and execution continues after the statement. An error that no handler takes goes on to the caller, and must be listed in `fails with` (E0074). A fault is never received by `on` (L-17).

**`handled_statement`, `handled_head` and `inline_handler`.** `let x = try f() on E as e:` followed by a block handles one error of the `try` expression that ends the statement; the head is a `let`, an assignment, a `return` or an expression. When `f` succeeds, the statement completes with its value; when it fails with `E`, the block runs, and it must not complete normally: it ends with `return`, `fail`, `break`, `continue` or a call of a function that never returns (E0075, L-14a). One handler per statement, at its end (E0024).

**`branch_block`.** A branch of `if`, `else if`, `else`, `case` or `on` that holds one simple statement may be written on its header line: `case none: return 0` (RFC 0002). `unio fmt` keeps it there when the line fits 100 characters (T-1a). Functions, loops, `using`, `test`, `try:` and handled statements always have a block.

**`using_statement`.** `using name = expression:` creates a resource (a value of a type that implements `Resource`), binds it and runs the block. When the block ends, normally, by `return`, `break`, `continue`, an error or a fault, `release` of the resource runs exactly once; nested `using` blocks release in reverse order (L-21, runtime 5). A resource is created only here or as a field of another resource (E0080), cannot leave its block by being stored, returned or captured (E0081), and is compared with `is_same` rather than `==` (E0066). The name is a place: its `var self` methods may be called, but it cannot be reassigned.

## 4. Expressions (grammar section E)

Precedence, from lowest to highest, as the grammar fixes it:

| Level | Operators | Associativity |
|---|---|---|
| 1 | `a if c else b` | right |
| 2 | `or` | left |
| 3 | `and` | left |
| 4 | `not` | prefix |
| 5 | `==` `!=` `<` `<=` `>` `>=` `in` `not in` `is` | none (E0020) |
| 6 | `+` `-` | left |
| 7 | `*` `/` `%` | left |
| 8 | `-` (negation), `try` | prefix |
| 9 | `**` | right |
| 10 | call, generic call, index, `.`, `?.` | postfix |

Evaluation is strict and goes left to right (runtime 1.1, L-27). There are no implicit conversions (L-5): the operands of an operator have one type (E0065), and an expression of another type than the expected one is E0061.

**`expression`.** A `lambda` or a `conditional`.

**`lambda` and `lambda_param`.** `function (x as Int) returns Int: x * 2` is an anonymous function whose body is one expression. Its parameter and result types may be left out where a function type is expected, as in an argument: `tasks.map(function (task): task.id)` (L-8a); elsewhere they are required (E0049). A lambda may declare `fails with`. It captures the values of the names it uses when it is made, and cannot assign to them (L-11a). Its effects count where it is called (semantics 8.4).

**`conditional`.** `a if c else b` evaluates the `Bool` `c`, then only the chosen branch; both branches have one type.

**`or_expr`, `and_expr` and `not_expr`.** `and`, `or` and `not` take `Bool`; `and` and `or` evaluate the right operand only when the left does not decide the result (runtime 1.2). `option or fallback` with an `Option[T]` on the left gives the value of `some`, or evaluates the fallback for `none`: a `T` fallback gives `T`, an `Option[T]` fallback gives `Option[T]` (semantics 4.3).

**`comparison` and `comparison_op`.** `==` and `!=` need `Equal`; `<`, `<=`, `>`, `>=` need `Order`, except that `Float` compares by IEEE 754, where every comparison with NaN is false (L-32). Comparisons do not chain (E0020). `a in c` calls `c.contains(a)` and `not in` negates it. `x is pattern` is a `Bool` that tests a pattern; its names are bound in the branch the test guards, in the right operand of a following `and`, and nowhere else (E0050, L-33). `==` on a resource is E0066.

**`additive` and `multiplicative`.** `+`, `-`, `*`, `/`, `%` on `Int` and `Float`, and `+` on two texts, which joins them; a user type gets them by implementing `Add`, `Subtract`, `Multiply`, `Divide` (L-9a). On `Int`, overflow, division by zero and `%` by zero are faults; `/` truncates toward zero and `%` has the sign of the left operand (runtime 2.1). `Float` follows IEEE 754 and never faults. Lists have no `+`: `append_all` joins them.

**`unary`.** `-x` negates a number. `try x` evaluates an expression that can fail, a call of a function with `fails with` or a `Result` value (L-16), and passes its error on to the caller unchanged; it is required before such a call outside a `try:` block (E0073), and before an expression that cannot fail it is E0076.

**`power`.** `a ** b` raises `Int` or `Float` to a power; it binds tighter than negation, so `-2 ** 2` is `-4`, and to the right, so `2 ** 3 ** 2` is `2 ** 9`. On `Int`, overflow and a negative exponent are faults.

**`postfix`.** A `primary` followed by any number of calls, generic calls, indexes, field accesses and safe accesses, applied from left to right.

**`call` and `argument`.** `f(a, b, name = c)` calls a function, a function value, a method (`x.m(...)`), a function of a type (`T.f(...)`), a struct or variant constructor. Positional arguments come first (E0023) and bind the leading parameters; named arguments bind by name, in any order, without changing the order of evaluation. A wrong count, an unknown name or a name given twice is E0064. A method is found in the `extend` blocks of the receiver's type, then in the interfaces it implements, their defaults included (semantics 3.4).

**`generic_call`.** `json.decode[List[Task]]` before the arguments of a call writes the type arguments that the call cannot infer. `[` after an expression starts type arguments when the first token is a name with an upper-case first letter and the matching `]` is followed by `(`; otherwise it is an index.

**`index`.** `items[i]` reads the element of a `List` at an `Int` position or the value of a `Map` under a key; a missing element or key is a fault, and `get` is the safe form that returns `Option` (L-18). On a `var` place it can be assigned.

**`field_access`.** `a.b` reads a field of a struct, an error or a variant value, or names a method for a call. A private field of another module is E0057.

**`safe_access`.** `a?.b` on an `Option[T]` gives `some(a.b)` when `a` is `some`, and `none` without evaluating the rest of the chain when it is `none` (L-12, L-27).

**`primary`.** A number, a text, `true`, `false`, `self` (the receiver inside a method), a name, an expression in parentheses, a tuple, a list or a map.

**`tuple`.** `(a, b)` is a value of type `Tuple[A, B]`; a one-element tuple is `(a,)`. A tuple is taken apart by a tuple pattern or by `for a, b in ...`.

**`list` and `comprehension`.** `[a, b, c]` is a `List[T]` whose items have one type; `[]` needs an expected type. `[e for x in c if g]` evaluates `c` once, then for each item the condition `g` and, where it holds, `e`, giving the list of the results in order (runtime 1.5); several names take tuples apart as in `for`.

**`map` and `map_entry`.** `{key: value, ...}` is a `Map[K, V]` whose keys implement `Hash` and `Equal` (`Float` cannot be a key, E0045). Entries are evaluated in order and a repeated key keeps the last value; iteration follows the order in which keys were first inserted. `{}` needs an expected type. There is no set literal: `Set[T]()` and `List.to_set()` make sets.

## 5. Patterns (grammar section P)

Patterns appear after `case` and `is`. A pattern either matches a value, binding names to parts of it, or does not; matching does not evaluate anything but the value it inspects.

**`pattern`.** One or more alternatives separated by `|`, tried from left to right; the first that matches binds its names.

**`bind_pattern`.** `p as name` matches what `p` matches and also binds the whole value to `name`.

**`primary_pattern`.** `_` matches anything and binds nothing; the other forms follow.

**`literal_pattern`.** An integer, possibly negative, a text, `true` or `false` matches a value equal to it. There are no `Float` patterns (L-9).

**`pattern_name`.** A name that denotes a variant without fields (`Show`, `Command.Show`, `none`) matches that variant; any other single name binds the value to a new immutable local name.

**`constructor_pattern` and `pattern_arg`.** `Add(title)`, `some(value)`, `error(e)` or `Shape.Circle(radius = r)` matches the variant and then matches its fields against the arguments: positional arguments in the order of the fields, named ones by field name.

**`list_pattern` and `list_element`.** `[a, b]` matches a list of exactly that length, element by element. `name...` matches the rest of the list, possibly empty, as a new list; there is at most one, and it is last (E0034): `case ["add", rest...] if not rest.is_empty():`.

**`tuple_pattern`.** `(a, b)` matches a tuple of that many elements, element by element.

## 6. Types (grammar section T)

**`type`, `type_name` and `type_argument`.** A type is a name, possibly qualified by a module (`files.IoError`), with type arguments in brackets: `List[Task]`, `Map[Text, Int]`, `Result[Int, ParseError]`. A wrong number of type arguments is E0059; an unknown type is E0056. The built-in types are `Int`, `Float`, `Bool`, `Text`, `Bytes`, `List[T]`, `Map[K, V]`, `Set[T]`, `Option[T]`, `Result[T, E]`, `Tuple[...]`, `Path`, `Function[...]` and `Never` (L-4); `Self` stands for the implementing type inside an interface. Types are compared by name and arguments (semantics 2.3): two declarations with one name in two modules, or in two packages, are two types.

**`function_shape` and `shape_parameter`.** The argument of `Function` is a shape: `Function[(Int, Text) -> Bool]`, `Function[(T) -> U fails with E]`, `Function[(named limit as Int) -> Int]`. A function value fits a shape when its parameter types, result, errors and named labels agree; a shape has no defaults (L-28). Without `->` the function returns nothing.

**`type_list`.** The types after `fails with`, in a signature or a shape, separated by commas. The order does not matter.

**`Option`, `Result` and `Never`.** `Option[T]` is `some(value)` or `none` (L-12); `Result[T, E]` is an ordinary enum with `ok(value)` and `error(value)`, which exists only where it is declared or built explicitly (L-16). `Never` is the type of `fail`, `return`, `break`, `continue` and of calls that do not return; it fits wherever a type is expected.

## 7. Programs, packages, and the boundary of the language

- **Programs.** A program starts at `public function main()` of the module given to `unio run` or `unio build`; `main` takes no parameters (arguments come from `std.args.list()`) and has no `fails with`, and any other signature is E0090. Its exit code is 0, or the `Int` that `main` returns, which must be 0 to 100. A fault ends it with 101 after releasing the open resources, exhaustion of memory or time with 102, and a policy that cannot be met with 103 (C-15, runtime 6).
- **Errors and faults.** An error is a value in the signature, handled by `try` and `on`; a fault is a bug or an exhausted check that ends the program in a controlled way and cannot be handled (concept section 3).
- **Packages and permissions.** A package is a directory with `unio.toml` (T-6, [packages.md](packages.md)). The effects of each function (`files.read`, `files.write`, `env`, `process`, `network`, `allocates`) are derived from the calls it makes (L-9b) and must be permitted by the manifest (E0085); `allocates`, `clock` and `random` need no permission, and a package without a manifest or without a `[permissions]` table permits none of the others. The manifest itself is checked as a whole (E0088), and its `unsafe_modules` must name exactly the modules marked `unsafe module` (E0089). Every run happens in the sandbox those permissions describe (SB-1, runtime 9).
- **Layers and profiles.** The standard library has the layers `core`, `alloc` and `std`; a module of a layer, and a package checked with `--profile=core`, uses nothing above what it may (E0084, E0087; semantics 8).

## 8. Diagnostic codes

Every diagnostic has a code, a message in English or Russian, a span, an explanation, and, where the fix is unambiguous, a fix with a replacement (C-6 to C-10). `unio explain CODE` prints the explanation with an example of the error and the example fixed; `unio check --diagnostics=json` prints the same diagnostics as JSON ([ai.md](ai.md) section 2). The table lists every code of the catalog; the CI check of this document fails when the catalog gains or loses a code that the table does not. E0004 is not used, E0070 is reserved, and the gaps between the other numbers were never allocated.

| Code | Meaning | Defined in |
|---|---|---|
| `E0001` | Tab in indentation | grammar L, [spec/README.md](../spec/README.md) |
| `E0002` | Indentation is not a multiple of four spaces | grammar L |
| `E0003` | Indentation grows by more than one level, or after a line that does not end with `:` | grammar L |
| `E0005` | Unterminated text literal | grammar L |
| `E0006` | Invalid escape sequence in a text | grammar L |
| `E0007` | Invalid character | grammar L |
| `E0008` | Invalid number literal | grammar L |
| `E0009` | Unclosed bracket | grammar L |
| `E0010` | Closing bracket without a matching opening one | grammar L |
| `E0011` | Empty or unterminated interpolation, or `}` in a text without `\` | grammar L |
| `E0012` | Unexpected token; the message names what the grammar expects there | grammar F, S, E, P, T |
| `E0013` | Nesting deeper than 64 levels | grammar L |
| `E0020` | Chained comparison such as `a < b < c` | grammar E |
| `E0023` | Positional argument after a named one | grammar E |
| `E0024` | More than one inline handler in a statement, or one not at its end | grammar S |
| `E0030` | Invalid assignment target | grammar S |
| `E0032` | Word reserved for a later release (`uses`) | grammar L |
| `E0034` | Rest element `name...` that is not last, or more than one | grammar P |
| `E0040` | A named-only argument given without its label | RFC 0001, semantics 3.2 |
| `E0041` | Parameters in the wrong groups or order, or a named function shape that does not fit | RFC 0001 |
| `E0042` | One variable reaches two `var` parameters of one call | RFC 0001, semantics 6.3 |
| `E0043` | A `Result` or `std.process.Output` dropped or never read | RFC 0001, semantics 6.6 |
| `E0044` | An interface implemented twice, or outside the package of the type or the interface | RFC 0001, semantics 3.6 |
| `E0045` | `Float`, or a type built from it, where `Order` or `Hash` is required | RFC 0001, semantics 2.9, 3.3 |
| `E0046` | Automatic JSON conversion of a type with a private field | RFC 0001, semantics 3.8 |
| `E0047` | A `Range` used as a `List` | RFC 0001, semantics 4.9 |
| `E0048` | A `deps.` import that the manifest, the lock or the cache does not provide | RFC 0001, semantics 1.9 |
| `E0049` | A lambda parameter without a type where no function type is expected | RFC 0002, semantics 3.5 |
| `E0050` | A name bound by `is` used where it is not bound | RFC 0002, semantics 4.8 |
| `E0051` | Two declarations, or two methods of one type, with one name | semantics 1.2 |
| `E0052` | An imported name collides with a declaration or another import | semantics 1.3 |
| `E0053` | Module not found | semantics 1.3 |
| `E0054` | Imported name not declared or not public in the module | semantics 1.3 |
| `E0055` | A local name repeats a visible local name | semantics 1.5 |
| `E0056` | Unknown name or type | semantics 1.6 |
| `E0057` | Private declaration, field, or method used from another module | semantics 1.7 |
| `E0058` | Import cycle | semantics 1.8 |
| `E0059` | Wrong number of type arguments | semantics 2.1 |
| `E0060` | Type cannot be inferred; an annotation or explicit type arguments are needed | semantics 2.2, 2.6, 3.3 |
| `E0061` | Expression of a type other than the expected one | semantics 2.3, 2.5 |
| `E0062` | Type argument does not meet its constraint | semantics 3.3 |
| `E0063` | Struct construction with a missing or unknown field | semantics 3.1 |
| `E0064` | Call with wrong arguments: count, unknown name, or a name given twice | semantics 3.2 |
| `E0065` | Operator not defined for the operand types | semantics 4.1 |
| `E0066` | `==` on a resource | semantics 4.2, 7.3 |
| `E0067` | Interpolated expression does not implement `Display` | semantics 4.7 |
| `E0068` | `match` does not cover every variant | semantics 6.5 |
| `E0069` | Interface implementation misses a method or changes its signature | semantics 3.6 |
| `E0071` | Interface cannot be used as a type | semantics 2.5 |
| `E0072` | Short form of a variant that collides with a visible name | semantics 3.7 |
| `E0073` | Call of a function with `fails with` outside `try` | semantics 5.1 |
| `E0074` | Error neither handled nor listed in `fails with` | semantics 5.2, 5.3 |
| `E0075` | Inline handler block completes normally | semantics 5.4 |
| `E0076` | `try` before an expression that cannot fail | semantics 5.5 |
| `E0077` | Assignment to an immutable name | semantics 6.1 |
| `E0078` | Change through a place that is not mutable | semantics 6.2 |
| `E0079` | Non-`var` argument for a `var` parameter | semantics 6.3 |
| `E0080` | Resource created outside `using` or a resource field | semantics 7.1 |
| `E0081` | Resource leaves its `using` block | semantics 7.2 |
| `E0082` | End of a function with a result reached without `return` | semantics 6.4 |
| `E0083` | `break` or `continue` outside a loop | semantics 6.4 |
| `E0084` | Layer or profile violation | semantics 8.1, 8.4 |
| `E0085` | Effect not permitted by the manifest | semantics 8.3 |
| `E0086` | `external function` outside an `unsafe module` of the standard library | semantics 8.5 |
| `E0087` | Call through an estimated function value that may need more than `core` | semantics 8.4, RFC 0005 |
| `E0088` | A mistake in the manifest `unio.toml` | semantics 8.6, RFC 0008 |
| `E0089` | `unsafe_modules` does not name exactly the modules marked `unsafe module` | semantics 8.6, RFC 0004 |
| `E0090` | `main` of a given module has parameters, type parameters, a result other than `Int`, or `fails with` | semantics 3.9, runtime 6.1 |

Faults and exit codes are not diagnostics: they are the run-time outcomes of runtime.md sections 4 and 6.
