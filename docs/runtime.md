# Dynamic semantics and the runtime of Unio 0.1

Version 0.1-draft. Russian localization: [ru/runtime.md](ru/runtime.md). Basis: concept sections 2–4, requirements L-10 to L-21, C-13 to C-20, S-9 to S-12, SB-1, T-4, T-5a; [semantics.md](semantics.md) for what is checked before a program runs. This document is the reference for the interpreter of sprint S5 and for the native runtime of sprint S6.

A program that reaches the runtime has passed `unio check`: names resolve, types agree, errors are handled, resources stay in their blocks. The runtime therefore checks only what cannot be known before execution: faults, exhaustion, and the outside world.

## 1. Evaluation

1. Evaluation is strict and goes from left to right: the operands of an operator, the arguments of a call, the fields of a struct construction, the items of a list or map literal, and the parts of a text with interpolations are evaluated in the order they are written.
2. `and` and `or` evaluate their right operand only when it decides the result; `a or b` on an `Option` evaluates `b` only for `none`. `x if c else y` evaluates `c` and then one branch.
3. A default value of a parameter is evaluated at each call that omits the argument, after the arguments that precede it, and can use the parameters before it.
4. A `for` loop evaluates its collection once, before the first iteration; changing the source variable inside the loop does not change the iteration, because the loop holds the value it started with.
5. A comprehension `[e for x in c if g]` evaluates `c` once, then `g` and `e` for each item in order.

## 2. Values

1. `Int` is a 64-bit two's-complement integer; overflow of `+`, `-`, `*`, `**`, division by zero, and `%` by zero are faults (L-17). `/` truncates toward zero, and `%` has the sign of the left operand.
2. `Float` is IEEE 754 binary64 with round-to-nearest-even; no operation on `Float` faults.
3. `Text` is an immutable sequence of Unicode scalar values stored as UTF-8 with an index of character positions, so that `length`, `slice`, and `char_at` take constant time.
4. Structs, enums, tuples, lists, maps, sets, `Bytes`, and texts are values (L-11a). A value is shared between names until it is changed; changing a shared value first copies the level that changes, and changing an unshared value happens in place (L-11b). Map and set iterate in the order keys were first inserted, and hashing does not depend on the run, so the output of a program is the same on every run.
5. Function values capture the values of the names they use when they are created (L-11a).
6. A resource is not a value: it has identity, is never copied, and its `release` runs exactly once (section 5).

## 3. Memory

1. Because values never share mutable state and closures capture copies, a value can never reach itself: there are no reference cycles. The interpreter of S5 therefore manages memory by reference counting, which frees each value when its last name goes away and also tells the copy-on-write rule when a value is unshared.
2. Requirement C-13 asks for a precise, tracing, non-moving collector. Reference counting is precise and non-moving, has no collection pauses at all, and needs no tracing because cycles cannot occur. Whether the native runtime of S6 keeps reference counting or adds tracing is decided by the measurements of section 7.1 of the requirements; replacing C-13 needs an RFC.
3. The runtime counts the memory it holds. When it exceeds the heap limit of the policy (`limits.heap_bytes`, SB-3a) or the memory the system grants, the program ends with code 102 (C-18). Recursion deeper than the stack limit also ends with code 102.

## 4. Errors and faults

1. A function with `fails with` ends either with its result or with one of its errors. `try` passes an error on unchanged; `on E as e` receives it; `fail e` ends the function with `e`. An error is a value and never unwinds past a function that does not list it, which the checker guarantees.
2. A fault (L-17) stops the program at its point. The runtime records a trace of function names, files, and lines (C-14), unwinds the stack, calls `release` of every open `using` block in reverse order (C-17), writes the diagnostic and the trace to standard error, and ends with code 101. A fault inside `release` during this unwinding is reported and the remaining resources are still released.
3. A fault cannot be received by `on`. `expect_fault(action)` in a `test` block runs `action` and succeeds only if it ends with a fault; the resources opened inside `action` are released before `expect_fault` returns.
4. `assert c` and `assert_fail(m)` are faults with the text of the condition or the message.

## 5. Resources

1. `using r = e:` evaluates `e`, binds `r`, and runs the block. When the block ends, normally, through `return`, `break`, `continue`, an error, or a fault, `release` of `r` runs; several `using` blocks release in reverse order of opening.
2. `release` cannot fail: its signature has no `fails with`, and an error that happens inside it is the resource's own business (for example, `AtomicFile` ignores errors of removing a temporary file).
3. On emergency termination (code 102) and abnormal termination, `release` does not run (C-18, C-19).

## 6. Programs and tests

1. A program starts at `public function main()` of the module given to `unio run`. Without a result, the exit code is 0; with an `Int` result, it is that value, which must be 0–100, otherwise it is a fault (C-15).
2. `unio test` runs every `test` block of the given modules, each in a fresh state; the order of the blocks does not matter, because there is no global mutable state. The result of each test is passed, failed with the fault that ended it, or failed with the diagnostics that keep it from compiling (T-5a), in text or JSON with name, status, time, and output (T-4).
3. Top-level constants are evaluated once, in dependency order, before `main` or before the first test that needs them; a constant whose evaluation faults ends the program with that fault.

## 7. External functions

1. The standard library declares 70 external functions, all in `unsafe module` files (S-7, S-10). Each is bound by its full name, such as `std.files.system_rename` or the method `Text.slice`, to a function of the runtime. The documentation comment of the declaration is its specification.
2. A test of the runtime checks that every external declaration has a binding and every binding has a declaration, so that the library and the runtime cannot drift apart.
3. The groups and the layer each needs:

| Group | Externals | Layer |
|---|---|---|
| Faults, identity, checked arithmetic, `Float` classification and total order | `assert_fail`, `expect_fault`, `is_same`, `Int.checked_*`, `Int.to_float`, `Float.is_nan`, `Float.is_finite`, `total_order_key` | core |
| Text, bytes, lists, maps, sets, numbers as text, code points | 29 methods and constructors in `alloc/builtin/*`, and `std.text.from_code_point` | alloc |
| JSON conversions generated for each type | `std.json.to_value`, `std.json.from_value` (implemented by the compiler, not by hand) | alloc |
| Standard output, arguments, environment, paths | `write_output`, `write_error`, `std.args.list`, `std.env.*`, `Path`, `Path.to_text`, `separator` | std |
| Files and the atomic commit protocol | 19 functions of `std.files` | std |
| Processes | `std.process.run` | std |

4. `unio run` and `unio test` start every program inside the sandbox of requirements section 4.6: the permissions of the nearest `unio.toml`, narrowed by the policy, are applied before `main` or before the first test. Section 9 says how this works and where its limits are.
5. Behavior fixed while implementing the externals, where the declarations were silent:
   - `Float.to_text` writes decimal notation for exponents from −6 to 20 and scientific notation otherwise (`1e+300`), always with a fraction for whole numbers (`2.0`), and `nan`, `inf`, `-inf` for non-finite values; `Text.to_float` does not read the last three back.
   - `Text.index_of` with a `start` outside `0..length()` is a fault, like `slice`. `Int ** n` with a negative `n` is a fault.
   - `std.files.rename` never replaces a target: for files it links and then unlinks, which is atomic; for directories it checks and then renames, which leaves a short race window that the documentation of the function states.
   - `std.env.home_dir` asks the operating system for the profile directory of the user; on Windows that directory is what the system reports through the user profile, not a variable the program set.
   - `std.process.run` gives a child an empty standard input; a child ended by a signal has status 128 plus the signal number; a timeout of zero or less is a `ProcessError` of kind `Other`.

## 8. The interpreter of sprint S5

1. The interpreter of S5 walked the checked syntax tree with the tables of the checker. Sprint S6 lowers that tree first (`compiler/unio/src/lower.rs`): names are resolved to slots of local values and indexes of constants, field positions and enum variants are found once at each place, literal lists and tuples are built once, and calls share one value stack. Several prelude functions (`is_empty`, `first`, `last`, `enumerate`, `contains`, `keys`, `values`, `append_all`, `range`, `Text.to_int`) run as intrinsics of the runtime with the results of their Unio definitions. The explicit error edges and release points that code generation needs are added to this form with the native back end. Measurements are in [bench/README.md](../bench/README.md); the JSON part of requirement 7.1 is not met by the interpreter yet.
2. The interpreter is the reference for the meaning of programs: the native code of S6 must give the same output, exit code, and diagnostics for every test of the repository.
3. Exit criterion of S5: every `test` block of `stdlib/`, `examples/`, `docs/examples/`, and `tools/` runs and passes; the tools in `tools/` run in the interpreter and give the same results as the Python checkers, which stay in the repository as the reference for parity and for comparisons of performance.

## 9. The sandbox in practice

This section is the documentation of the limits of the sandbox that SB-9 requires.

1. **Where permissions come from.** `unio run` and `unio test` read `[permissions]` from the nearest `unio.toml` above the program (T-6) and narrow it by the policy (SB-3a). Without a manifest, a program can read its arguments and standard input and write to standard output and standard error, nothing else (SB-2). Tests get the same permissions as the program; a test that needs files declares them, for example `files.write = ["$TEMP/**"]`.
2. **Paths.** Relative entries are relative to the directory of the manifest or the policy. Links and `..` are resolved before a path is compared. Writing one file also permits the temporary files of its atomic commit next to it and syncing its directory. A program name without a path matches only a call by that name, never a path that ends in it.
3. **Blocked operations** are the errors the functions declare, of kind `PermissionDenied`, naming the missing permission; `exists` and `is_directory` report `false`, and `env.get` reports `none` (SB-6).
4. **Platforms.**
   - Linux: the runtime checks every operation, and the operating system enforces the same scopes with `no_new_privs`, Landlock (files, TCP, signals), and seccomp (sockets, `ptrace`, mounts, namespaces, `io_uring`, `bpf`, and other dangerous calls); core dumps are disabled. Without Landlock ABI 3 or seccomp the start fails with code 103 unless the operator allows a weaker sandbox.
   - macOS: a deny-by-default Seatbelt profile built from the scopes; it is checked in CI on macOS.
   - Windows: AppContainer and Job Objects are not implemented yet. A program does not start (code 103) unless the operator passes `--allow-weaker-sandbox`; then only the checks of the runtime and the time and heap limits apply. SB-8 is not met on Windows until this is done.
5. **Limits.** Wall and CPU time are enforced by a watchdog that ends the program with code 102; the CPU limit also applies to children. The process limit counts calls of `process.run`. The file-size limit is checked by the runtime and, on Linux and macOS, by the operating system.
6. **What the sandbox does not do** (SB-9).
   - It does not protect against vulnerabilities of the kernel, against side channels, or against misuse of what was granted: a program allowed to write `$HOME/**` can destroy those files.
   - On Linux the operating-system rules are coarser than the checks of the runtime. Permission to write one file opens the directory of that file to writes at the operating-system level, because the atomic commit needs it; a scope whose directory does not exist at start grants nothing at that level; and any `process` permission lets children read and execute the system directories. On macOS the same holds for the system directories, and children can also list the names in `/`, which the loader reads at start.
   - On Linux the rules apply to the threads that run the program and to its children; the idle main thread of `unio run` is not restricted.
   - For code written by AI, use a minimal manifest, a policy with limits, and a separate user, container, or virtual machine for untrusted code.

