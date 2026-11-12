# Unio

**Unio is a concept for a general-purpose programming language that is easy for people to read, convenient for AI to generate, and strictly checkable before it runs.** It combines the simplicity of Python, the expressiveness of Ruby, the reliability of static typing, and the capabilities of native compilation.

Guiding principle: **clear code, explicit behavior, controlled resources**.

The syntax stays close to natural language: blocks are defined by indentation, the purpose of each construct is spelled out in words, and mathematical operations keep their familiar notation.

```unio
function greet(name as Text) returns Text:
    return "Hello, {name}!"

function active_names(users as List[User]) returns List[Text]:
    return [user.name for user in users if user.active]
```

Unio has a small vocabulary of constructs and a single standard style. This helps people verify generated programs. Natural language is used to state the task; executable code has unambiguous semantics.

The concept rests on the following decisions:

- **Simplicity with strict checks.** Types of local values are inferred automatically. Function interfaces explicitly describe input, result, and expected errors. Missing values are handled through `Option`; errors through typed results and the readable constructs `fails with`, `try`, and `fail`.
- **Object-oriented and functional approaches.** Structures, methods, interfaces, and composition organize the domain. Functions, immutable values, and collection transformations describe computation. Mutable state is marked explicitly.
- **Two memory-management models.** Ordinary objects are served by a GC. Large tensors and external resources have managed lifetimes, budgets, and verifiable ownership. Freeing GPU memory accounts for the completion of device computation.
- **AI-assisted development.** The compiler provides precise machine-readable errors and context for individual symbols. Unified tools for formatting, testing, and analysis help produce small, verifiable changes. People and AI work in the same language. Development is test first: a test states the intent before the code exists, and a test that does not compile yet is simply a failing test.
- **Development of AI itself.** The compute extension provides tensors, automatic differentiation, CPU/GPU backends, buffer planning, and RAM/VRAM profiling. Memory savings come from suitable algorithms, data formats, and cache management, and are evaluated together with speed and model quality.
- **Security on several levels.** A safe subset of the language rules out common memory errors. Execution isolation, least privilege, dependency verification, and a protected release process limit the consequences of compromise. Out-of-memory, abnormal termination, and native components are tested separately.
- **Extensible computation.** A quantum module adds circuits, simulation, and hardware access through shared tooling. Support for a specific device is defined by its capabilities.
- **From applications to kernels.** The library and the runtime are split into the layers `core` (no allocator, GC, or operating system), `alloc`, and `std`. Build profiles choose the layers without changing what any construct means, so a later system profile without a GC can serve kernels and embedded devices in the same language.
- **Built in itself.** Unio is developed in Unio wherever possible: the standard library and the project tools are already written in it, and the compiler follows by 1.0. Every gap found this way is closed in the language, so the language improves recursively.

Unio is to ship as a single kit: compiler, standard library, dependency management, formatter, testing, editor support, and profiler.

**The first release is a small, reliable core for automation and CLI tools.** Server capabilities, AI computation, and the quantum extension follow. Performance, readability, and security are confirmed by measurement and verification; universal C-level speed, the absence of any vulnerability, and automatic quantum speedup are not promised.

**Current state.** A compiler in Rust exists in [compiler/](compiler/README.md): `unio check`, `fmt`, `outline`, `symbol`, `explain`, `run`, and `test` with an interpreter and an operating-system sandbox on Linux and macOS, and `unio build`, which makes native executables for Linux x86-64 and AArch64, macOS AArch64, and Windows x86-64 (Windows runs programs and tools only under the weaker sandbox the operator allows, since AppContainer is not implemented). The 0.1 runtime manages memory by reference counting, not by the GC described above, until the decision on C-13. Packages install from a directory, an archive, or a registry served as static files, with a lock file and isolated download and unpack workers; `unio publish` signs packages with ML-DSA-65, and a registry package is installed only after its signature is verified against the operator's trust anchor, with rotation, revocation, and rollback protection ([docs/packages.md](docs/packages.md)); there is no TLS in 0.1, and no component claims post-quantum compliance before an independent review ([docs/cryptography.md](docs/cryptography.md) section 7). The language server and the R-10 control set are requirements only: none of them is implemented. Release 0.1 is not published; [the status table](docs/status.md) lists every requirement with what exists and what checks it.

## Documentation

- [Refined concept](docs/concept.md)
- [Requirements specification for release 0.1](docs/requirements.md)
- [Work plan](docs/plan.md)
- [Implementation status](docs/status.md): every requirement with its implementation, check, platforms, and known limits
- [Cryptography policy](docs/cryptography.md): post-quantum defaults, trust, compatibility, and acceptance
- [Example program](docs/examples/todo.unio)
- [Grammar and specification tests](spec/README.md)
- [The Unio style](docs/style.md): the one form that `unio fmt` writes
- [Static semantics](docs/semantics.md): names, types, errors, values, resources, and layers that `unio check` verifies
- [Packages](docs/packages.md): the manifest, `unio.lock`, the package commands, the cache, and signed registries with publishing
- [Dynamic semantics and runtime](docs/runtime.md): evaluation, values, memory, faults, resources, and the interpreter
- [Standard library](docs/stdlib.md), written in Unio in [stdlib/](stdlib) in the layers `core`, `alloc`, and `std`
- [Reference utilities](examples)
- [Project tools in Unio](tools)
- [Editor support](editors/README.md): VS Code, Cursor, JetBrains, Vim and Neovim
- [RFC process](docs/rfcs/README.md)
- [Influences](docs/influences.md): what Unio takes from other languages and what it declines
- [Accepting contributions](docs/contributing.md): defenses against bad and malicious code; [security policy](SECURITY.md)
- [UniOS](docs/os.md): the operating system in Unio and the self-hosted site proof of concept
- [Reference customer](docs/customers.md): tanstaafl, a product written with AI, as scenarios and the gaps they find
- [Русская версия документации](docs/ru/README.md)

- [Comparative-review contracts](docs/language-contracts.md): semantics, library, tooling and acceptance.
- [RFC 0001](docs/rfcs/0001-language-contracts.md): accepted language changes.

## Credits

Unio grew out of the idea [“A universal programming language for AI and humans”](https://eurecable.com/ideas/992) by fazzz, published on Eurecable on November 4, 2026: writing programs with AI runs into the limits of languages that evolved for people alone, so a language designed from scratch for AI should still be one that people can read and check.

## Licence

Unio is licensed under either of the [MIT licence](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE), at your option, as the crates of `compiler/` declare (`license = "MIT OR Apache-2.0"`). A contribution is licensed in the same way unless its author states otherwise. The third-party code that the `unio` binary and its runtime library contain is listed in [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES), which `.github/scripts/third_party_notices.py` generates from `compiler/Cargo.lock`; CI fails when it is stale.
