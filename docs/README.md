# Unio documentation

| Document | Contents |
|---|---|
| [concept.md](concept.md) | Refined language concept: syntax, types, errors, memory, modules, security, working with AI; settled decisions and their rationale |
| [requirements.md](requirements.md) | Requirements specification for release 0.1 "Core": scope, language, compiler, library, tooling, acceptance |
| [plan.md](plan.md) | Work plan: stages, exit criteria, roles, risks, metrics, later releases |
| [status.md](status.md) | Implementation status: every requirement of 0.1 with its implementation, check, platforms, and known limits; updated with each change |
| [cryptography.md](cryptography.md) | Post-quantum policy: algorithms, trust, compatibility, and acceptance |
| [examples/todo.unio](examples/todo.unio) | Complete example utility written in the language core |
| [../spec/README.md](../spec/README.md) | EBNF grammar, its reference checker, and the specification test corpus |
| [stdlib.md](stdlib.md) | Standard library: overview of its Unio sources in `stdlib/` |
| [style.md](style.md) | The one style of Unio sources that `unio fmt` writes: layout, blank lines, spaces, line breaking, branches, comments |
| [semantics.md](semantics.md) | Static semantics of 0.1: names, types, calls, operators, errors, mutability, resources, layers and effects, with diagnostics E0051–E0088 |
| [packages.md](packages.md) | Packages: the manifest, `unio.lock`, `unio add`, `remove`, `update`, `install`, the cache, signed registries as static files with `unio publish` and keys, the archive format |
| [runtime.md](runtime.md) | Dynamic semantics and the runtime: evaluation order, values and memory, errors and faults, resources, programs and tests, the 83 external functions, the interpreter of S5 |
| [../examples](../examples) | Reference utilities checked against the grammar |
| [../tools](../tools) | Project tools written in Unio |
| [rfcs/README.md](rfcs/README.md) | RFC process for language changes |
| [influences.md](influences.md) | What Unio takes from Zig, Odin, Gleam, Mojo, and Unison, and what it declines |
| [contributing.md](contributing.md) | Accepting contributions: threat model, trust levels, layers of defense against bad and malicious code |
| [../SECURITY.md](../SECURITY.md) | How to report a vulnerability and what happens next |
| [os.md](os.md) | UniOS: a site about Unio, in Unio, on an operating system written in Unio; architecture, stages, risks |
| [customers.md](customers.md) | Reference customer tanstaafl: its scenarios by release and the gaps they find |
| [ru/](ru/README.md) | Russian localization |

- [Comparative-review contracts](language-contracts.md): semantics, library, tooling and acceptance.
- [RFC 0001](rfcs/0001-language-contracts.md): accepted language changes.
