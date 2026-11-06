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

## Documentation

- [Refined concept](docs/concept.md)
- [Requirements specification for release 0.1](docs/requirements.md)
- [Work plan](docs/plan.md)
- [Cryptography policy](docs/cryptography.md): post-quantum defaults, trust, compatibility, and acceptance
- [Example program](docs/examples/todo.unio)
- [Grammar and specification tests](spec/README.md)
- [Standard library](docs/stdlib.md), written in Unio in [stdlib/](stdlib) in the layers `core`, `alloc`, and `std`
- [Reference utilities](examples)
- [Project tools in Unio](tools)
- [RFC process](docs/rfcs/README.md)
- [Influences](docs/influences.md): what Unio takes from other languages and what it declines
- [Accepting contributions](docs/contributing.md): defenses against bad and malicious code; [security policy](SECURITY.md)
- [UnioS](docs/os.md): the operating system in Unio and the self-hosted site proof of concept
- [Reference customer](docs/customers.md): tanstaafl, a product written with AI, as scenarios and the gaps they find
- [Русская версия документации](docs/ru/README.md)
