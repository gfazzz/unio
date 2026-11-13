# Unio specification files

Version 0.1-draft. Russian localization: [docs/ru/spec.md](../docs/ru/spec.md).

| File | Contents |
|---|---|
| [grammar.ebnf](grammar.ebnf) | Core grammar: lexical structure, declarations, statements, expressions, patterns, types |
| [check_grammar.py](check_grammar.py) | Reference checker that interprets `grammar.ebnf` directly and parses programs with it |
| [check_coverage.py](check_coverage.py) | Reference checker of T-5b: every grammar rule has corpus files, every diagnostic code a control program |
| [tests/valid/](tests/valid) | Programs that must parse, one file per area of the grammar |
| [tests/invalid/](tests/invalid) | Programs that must fail on the line, and with the code, stated in their first line |

The reference programs are [docs/examples/todo.unio](../docs/examples/todo.unio) and the utilities in [examples/](../examples). They, the standard library in [stdlib/](../stdlib), and the tools in [tools/](../tools) are checked with the same grammar; [check_api.py](check_api.py) checks that they call only what the standard library declares.

CI runs the Unio versions of these scripts, in [tools/](../tools), with the interpreter of sprint S5. The Python scripts stay in the repository as the reference: the CI job "Unio tools agree with the Python checkers" (`compiler/unio/tests/tools.rs`) checks that both give the same output and exit code, and they serve in performance comparisons ([bench/README.md](../bench/README.md)). A change to checker behavior changes both.

## Using the checker

The checker needs only Python 3.9 or later.

```sh
python3 spec/check_grammar.py --self-check                # grammar consistency
python3 spec/check_grammar.py --corpus                    # spec/tests
python3 spec/check_grammar.py docs/examples/todo.unio examples/*.unio
python3 spec/check_grammar.py --tree examples/wc.unio      # parse tree
```

`--self-check` verifies that every rule used is defined and reachable from `file`, and that the terminals of the rules match the keyword and punctuation lists in section L of the grammar.

The grammar is read as ordered choice: alternatives are tried in the order written, and the first one that matches is taken. The EBNF is written so that this reading gives the intended parse; a change that breaks it shows up in the corpus.

## What the grammar fixes and what it leaves to the checker of the compiler

The grammar decides whether a file is syntactically correct. Rules that the EBNF cannot express are stated in comments next to the rules they restrict and are checked by the compiler, not by `check_grammar.py`:

- generic arguments versus an index after an expression (rule `generic_call`; the checker implements this rule);
- at most one inline handler per statement, at the end of it (E0024);
- an assignment target ends in a name, a field, or an index (E0030);
- positional arguments come before named ones (E0023);
- at most one rest element in a list pattern, in the last position (E0034).

Name resolution, types, error sets, exhaustiveness, mutability, and resources are checked by `unio check` ([docs/semantics.md](../docs/semantics.md)); their corpus is [tests/semantic/](tests/semantic), which only the compiler checks.

## Syntax-level diagnostic codes

These codes are fixed by the grammar. Every code of the compiler, with its explanation, an example of the error, and the example fixed, is in the diagnostic catalog (`compiler/unio/src/catalog.rs`, shown by `unio explain CODE`); the semantic codes are listed in [docs/semantics.md](../docs/semantics.md).

| Code | Meaning |
|---|---|
| E0001 | Tab in indentation |
| E0002 | Indentation is not a multiple of 4 spaces |
| E0003 | Indentation grows by more than one level, or after a line that does not end with `:` |
| E0005 | Unterminated text literal |
| E0006 | Invalid escape sequence in text |
| E0007 | Invalid character |
| E0008 | Invalid number literal |
| E0009 | Unclosed bracket |
| E0010 | Closing bracket without a matching opening one |
| E0011 | Empty or unterminated interpolation, or `}` in text without `\` |
| E0012 | Unexpected token; the message names what the grammar expects there (compiler front end) |
| E0013 | Nesting too deep (more than 64 levels of expressions, patterns, types, blocks, or interpolations) |
| E0020 | Chained comparison such as `a < b < c` |
| E0023 | Positional argument after a named one |
| E0024 | More than one inline handler in a statement, or not at its end |
| E0030 | Invalid assignment target |
| E0032 | Word reserved for a later release (`uses`) |
| E0034 | Rest element `name...` that is not the last, or more than one |

E0004 is not used: because indentation grows one level at a time, a dedent always returns to an open level once E0002 holds.

## Corpus conventions

- A file in `tests/valid/` must parse completely.
- A file in `tests/invalid/` starts with `# expect-error: LINE` or `# expect-error: LINE CODE`. The checker must report the first error on that line, with that code when one is given. The checker reports codes for lexical errors only; every other syntax error is reported as `syntax`.
- One file tests one thing. A new grammar rule comes with at least one valid and one invalid file.
- CI enforces the last rule (T-5b) with `tools/check_coverage.unio`, whose reference is [check_coverage.py](check_coverage.py). A valid file covers the rules of its parse tree; an invalid file covers the rules in progress where it fails, that is, every rule whose attempt was on the way when the parser noted what it expected at the farthest token. A rule that cannot have such a file is exempt by a comment of the grammar beside it, `(* Corpus exemption: RULE has no invalid file. REASON *)`; today only `handled_head` is. The same check fails when a code of the catalog (`CODES` in `compiler/unio/src/catalog.rs`) has no program in [tests/control/](tests/control) and is not listed with its reason in `EXCLUDED` of `compiler/unio/tests/control.rs`, and when an exemption names something that is covered or does not exist.

## RFC 0001

The EBNF supports contextual `named` and labels in function shapes; `named_parameters.unio` and negative fixtures are in the corpus. Both API checkers extract the actual parameter binding. Group/default ordering, required labels at calls and the diagnostics E0040–E0044 are checked by the compiler, not by these scripts; E0045–E0048 are not in the compiler's catalog yet ([docs/status.md](../docs/status.md)). Normative rules: [contracts](../docs/language-contracts.md), [RFC](../docs/rfcs/0001-language-contracts.md).
