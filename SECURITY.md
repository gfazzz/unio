# Security policy

Russian localization: [docs/ru/security.md](docs/ru/security.md). The architecture of defenses is in [docs/contributing.md](docs/contributing.md).

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's "Report a vulnerability" form for this repository (Security tab), not through public issues or pull requests. Include the affected files or commands, steps to reproduce, and the impact you believe it has.

You will get an acknowledgement within 7 days and a decision on severity and a fix plan within 30 days. Please do not publish details before a fix is released or 90 days have passed, whichever comes first.

## Scope

In scope now: the compiler and the runtime in `compiler/` (`unio check`, `fmt`, `run`, `test`, `build`, and the executables it makes), the sandbox as implemented (requirements SB-1 to SB-9 on Linux and macOS; its documented limits are in [docs/runtime.md](docs/runtime.md) section 9 and [docs/status.md](docs/status.md)), the isolated workers of the tools and the hostile-project suite as implemented (SB-10, SB-11, SB-14, SB-15; on Linux and macOS), the standard library and its `unsafe module` files, the project tools in `tools/`, the checkers in `spec/` and `.github/scripts/`, the CI workflows, the editor extension in `editors/`, and anything in the repository that could run code on a contributor's or maintainer's machine or on a CI runner.

A documented limit is not a vulnerability by itself, while a way past it is. For example, a child of a program may start only the programs its manifest lists, the interpreters of listed scripts and, on Linux, the dynamic loader, but a listed interpreter runs any script it can read and on Linux the loader runs any program it can read ([docs/runtime.md](docs/runtime.md) 9.5).

Not implemented yet, and in scope once they exist: the Windows sandbox (AppContainer; on Windows programs and tools run only under the weaker sandbox the operator allows), the workers of the package operations and of the language server (SB-12, SB-13), the package manager and registry, signing and the rest of the [cryptography policy](docs/cryptography.md), the language server, and release artifacts. These are requirements only; nothing of them exists in the code.

## What happens after a report

A confirmed malicious change is reverted before it is analyzed. A compromised dependency, key, or release is revoked, and affected versions are named in the release notes. Every incident ends with a change to the defenses in `docs/contributing.md` or to a check.
