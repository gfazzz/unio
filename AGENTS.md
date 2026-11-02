# Working on Unio

## Current state and sources

Unio is currently a language specification with a grammar, reference programs,
standard library sources, and project tools. The executable checks are temporary
Python scripts. The compiler, runtime, and `unio` CLI described in the roadmap
are not implemented in this checkout; do not report Unio programs or their
`test` blocks as executed when only parsing or API checks have run.

Read the relevant sources before changing language behavior:

- [docs/concept.md](docs/concept.md): design decisions and their rationale.
- [docs/requirements.md](docs/requirements.md): release 0.1 requirements.
- [docs/plan.md](docs/plan.md): scope, stages, and acceptance criteria.
- [spec/grammar.ebnf](spec/grammar.ebnf): the source of truth for syntax,
  including lexical rules in section L.
- [spec/README.md](spec/README.md): checker usage, corpus conventions, and
  the boundary between syntax checks and future compiler checks.
- [docs/rfcs/README.md](docs/rfcs/README.md): the language change process.

The directories listed in the roadmap include future work. Inspect the actual
checkout before choosing build commands or adding infrastructure.

## Development and validation

Run commands from the repository root. The checkers use only the Python standard
library and support Python 3.9 or newer; CI uses Python 3.13. Prefer Python 3.13
when available. No package installation, credentials, database, or service startup
is needed for these checks.

Run the four checks in [.github/workflows/ci.yml](.github/workflows/ci.yml):

```sh
python3.13 .github/scripts/check_docs.py
python3.13 spec/check_grammar.py --corpus
python3.13 spec/check_grammar.py docs/examples/*.unio docs/ru/examples/*.unio examples/*.unio $(find stdlib -name '*.unio') tools/*.unio
python3.13 spec/check_api.py
```

These are shell commands for a Unix-like environment. Use `python3` or `python`
instead if that is the supported interpreter on your machine. For a focused
syntax investigation, use `python3.13 spec/check_grammar.py --tree FILE.unio`.

The corpus command also checks grammar consistency. Invalid corpus files pass
when the expected error is found. The API checker checks declared API usage and
library layer boundaries; it is not a full type checker or runtime test runner.
Report which checks actually ran and distinguish static validation from execution.

## Making changes

- Specify syntax and semantics before implementing a construct. Follow the RFC
  process: before the end of stage 1, update the concept, requirements, and
  specification directly with the prescribed "Changes from" note; after stage 1,
  syntax, semantics, keyword, and normative requirement changes require an RFC.
  Adding keywords or revisiting the settled decisions in concept sections 10–11
  requires an RFC regardless of stage.
- For behavior changes, start with a reproducing check. Every new grammar rule
  needs at least one valid and one invalid corpus file. Keep each fixture focused;
  invalid files begin with `# expect-error: LINE` or `# expect-error: LINE CODE`.
- Grammar alternatives are ordered choices. Preserve their intended precedence,
  and keep the lexical implementation consistent with section L of the EBNF.
- Keep the temporary Python checkers and their Unio counterparts in `tools/`
  consistent when changing checker behavior. Retain the Python checkers until
  the stage 3 interpreter can run equivalent Unio checks.
- Write library, utility, and project tool code in Unio according to the plan.
  Put behavioral `test` blocks alongside that code even while execution awaits
  the interpreter; do not describe those tests as passing based on parsing alone.
- Respect the standard library layers: `core` may use only `core`, `alloc` may use
  `core` and `alloc`, and `std` may use all three. Place external function
  declarations in `unsafe module` files. Reference programs use the API declared
  in `stdlib/`; keep [docs/stdlib.md](docs/stdlib.md) consistent with API changes.
- Keep English documentation and its Russian localization in `docs/ru/` in
  sync, including corresponding examples and RFCs. Root `README.md` maps to
  `docs/ru/README.md`; `spec/README.md` maps to `docs/ru/spec.md`.
- Use UTF-8, a final newline, no trailing whitespace, and four spaces for Unio
  indentation. Tabs are permitted only in the intentional invalid tab fixture.
  Preserve working relative Markdown links.
- Keep changes focused and preserve existing user work. In a managed cloud task,
  use the existing checkout: each task is already isolated, so create a Git
  worktree only when the user explicitly requests one.
