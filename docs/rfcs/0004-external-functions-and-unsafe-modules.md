# RFC 0004: External functions and unsafe modules in 0.1

- Status: accepted (decided by the language lead; requirements S-7, S-10, T-6, T-9 and diagnostic E0086; the trust of the standard library by origin is implemented for the embedded copy, `UNIO_STDLIB`, and the source tree of a development build; the library shipped next to an installed compiler does not exist yet, see Verification)
- Author: Unio project, with Claude
- Date: 2026-11-08
- Affects: requirements / standard library / tools
- Russian localization: [0004-external-functions-and-unsafe-modules.md](../ru/rfcs/0004-external-functions-and-unsafe-modules.md)

## Summary

Two notions that the documents used as one are separated. An `unsafe module` is a marker of a module. An `external function` is the ability to bind an implementation that is not written in Unio. The marker does not grant the ability. In release 0.1 only the trusted standard library declares external functions, in its `unsafe module` files; a user package cannot declare them, the marker in a package grants no such right and does not widen the permissions of the sandbox, and foreign functions in user code (FFI) need a separate RFC. The standard library is trusted because of its origin, the toolchain or its operator, never because of the package name `std`, a module name, a directory name, or the marker. Diagnostic E0086 keeps its code and gets this meaning; E0070 stays reserved.

## Motivation

S-7 said that unsafe code is allowed "only in the standard library", while T-6 lists `unsafe module` files in every manifest and T-9 asks for confirmation when a dependency with one is installed. Read together, they suggested that a package may carry unsafe code once it marks the module and the person installing it agrees. The compiler of the work in progress does the opposite: it rejects every external function outside an `unsafe module` of the standard library (E0086), because a program could otherwise check cleanly and then fault on a call that the runtime cannot make:

```unio
unsafe module

external function poke(address as Int) returns Int
```

The documents did not say what the marker means in a package then, nor what "the standard library" is. The compiler decides the second question by the location of files, and a package can satisfy it (see Verification).

## Why the existing requirements are not enough

No keyword or construct is added. The change fixes the meaning of existing ones, which S-7, T-6, and T-9 left open, and records a decision of the lead on a normative requirement, which after stage 1 goes through an RFC.

## Design

1. **Two notions.**
   - `unsafe module` (grammar `module_header`) marks a module. It allows the module to contain `external function` declarations only where the module also belongs to the trusted standard library.
   - `external function` binds an implementation outside Unio. In 0.1 every one is a primitive of the runtime, bound by its full name (`docs/runtime.md` section 7).
2. **Rule for 0.1.**
   - Only the trusted standard library declares external functions, and only in its `unsafe module` files (S-7, S-10). Each has a binding in the runtime and passes the review of R-5.
   - An `external function` anywhere else, including an `unsafe module` of a package, is E0086 at its name. An `external function` of the standard library outside an `unsafe module` is E0086 as well, with its own message.
   - The `unsafe module` marker in a package grants no right to external functions, does not widen the permissions of the manifest (T-6) or of the sandbox (SB-1 to SB-3), and does not relax any check.
   - Foreign functions in user code need a separate RFC, which defines the binding, the review, the effects, and the sandbox of such code.
3. **The marker in a package is accepted and means only review.** In a package, `unsafe module` is a declaration that the author lists the module for review: the module is named in the `unsafe` list of the manifest (T-6), and installing a dependency that has such a module asks for explicit confirmation (T-9). In 0.1 such a module contains only safe code, because E0086 rejects external functions in it, and it is checked, built, and sandboxed exactly like any other module. Once the compiler reads the list of T-6, a marked module missing from the list, or a listed file without the marker, is reported; the code for it is assigned separately.

   Why accepted rather than rejected: T-6 and T-9 already define the list and the confirmation for packages. Rejecting the marker in a package would make both empty in 0.1, would need a new diagnostic, and would be reversed by the FFI RFC, an incompatible change in each direction. Accepting it without rights keeps the review chain of T-6 and T-9 in place for that RFC, keeps every program of 0.1 valid under it, and gives the marker no power that could be misused now. The tools must say what the marker means when they show it (T-9): "listed for review; no external functions in 0.1".
4. **Trust of the standard library is by origin.** The trusted standard library is the one the toolchain selects, in this order:
   1. in an executable of `unio build`, the copy embedded by the build, which was the trusted library of that build;
   2. the directory in `UNIO_STDLIB`, set by the operator in the environment of the tool;
   3. the library shipped with the toolchain: next to the installed compiler, or, in a development build, the `stdlib/` of the source tree the compiler was built from.

   Nothing in a package selects it or adds to it: not a directory named `stdlib` in or above the package, not a module path `std.x` or a file `std/x.unio` of the package, not a package named `std`, and not the marker. A module of a package never counts as a module of the standard library, whatever its name or path. A package that wants a different library has the operator set `UNIO_STDLIB`.
5. **Diagnostics.** E0086 keeps its code: "an `external function` outside an `unsafe module` of the trusted standard library". Its message and its explanation in `unio explain` say that the marker of a package grants no such right. E0070 stays reserved until a separate assignment; this RFC does not use it.

## Migration

Not needed for programs: the compiler already reports E0086 in packages. A package that relied on a directory named `stdlib` next to it as its standard library sets `UNIO_STDLIB` instead once the search by location is removed.

## Alternatives

- **Packages may declare external functions after confirmation (T-9).** Rejected for 0.1: the runtime has no way to bind them, so the program would fault on the first call, and confirmation by the person installing is no review of foreign code. This is the subject of the FFI RFC.
- **Reject `unsafe module` in packages.** Rejected for the reasons of Design 3.
- **Trust by the module name `std` or by the marker.** Rejected: both are written by the author of the package, so they cannot be the basis of trust.
- **Trust by a directory named `stdlib` above the program.** This is what the compiler does now. Rejected: a package can contain such a directory and replace the whole library.

## Verification

- Done: the semantic corpus file `spec/tests/semantic/invalid/external_outside_stdlib.unio`, `tests/interpreter.rs` (`an_external_function_of_a_program_is_rejected_before_it_runs`), and the catalog entry of E0086 checked by `tests/catalog.rs`.
- Found while writing this RFC, by reading `compiler/unio/src/program.rs` and a probe with the release binary: `find_stdlib` takes, after the embedded copy and `UNIO_STDLIB`, the first directory named `stdlib` that holds `core/prelude.unio` among the ancestors of the first source file, before the library of the compiler's source tree. A package that contains `stdlib/core/prelude.unio`, or any directory above the program that does, therefore becomes the standard library: in the probe, a package with its own `stdlib/std/evil.unio`, an `unsafe module` with an external function, passed `unio check` without E0086 and faulted at run time ("the runtime has no external function std.evil.poke"); a changed signature of `std.env.get` in such a copy passed `unio check` and faulted at run time on the type of an argument. By reading, not probed: such a copy also decides the effects of its external functions (from their "Permission:" lines) and therefore the manifest check E0085, and the layers. The sandbox is not affected: permissions are still enforced at run time. `sandbox_cli.rs` (`stdlib_dir`) repeats the same search to grant read access to the library, and unlike `find_stdlib` it accepts `UNIO_STDLIB` without checking that it holds `core/prelude.unio`, so the two can choose different directories. The library "next to the installed compiler" of Design 4.3 does not exist: the built-in path is the source tree fixed at compile time.
- Done in commit `374560a`: `find_stdlib` and `stdlib_dir` both call `trusted_stdlib` in `compiler/unio/src/program.rs`, which takes the embedded copy, then `UNIO_STDLIB` (only when it holds `core/prelude.unio`), then the `stdlib/` of the source tree the compiler was built from; no directory of or above a package is looked at. `tests/semantic.rs` (`a_stdlib_directory_in_a_package_does_not_replace_the_standard_library`): a package with its own `stdlib/core/prelude.unio` and an `unsafe module` declaring an external function does not get that module (E0053).
- To be done: a test that a file `std/x.unio` of a package is not a module of the standard library; a test that the directory granted by the sandbox is the one the program was loaded with; and the library shipped next to an installed compiler (Design 4.3), without which an installed `unio` finds no library on a machine without the source tree it was built from, unless `UNIO_STDLIB` is set (status gap 19, plan section 12, release condition 1).
