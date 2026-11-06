# Unio specification files

Version 0.1-draft. Russian localization: [docs/ru/spec.md](../docs/ru/spec.md).

| File | Contents |
|---|---|
| [grammar.ebnf](grammar.ebnf) | Core grammar: lexical structure, declarations, statements, expressions, patterns, types |
| [check_grammar.py](check_grammar.py) | Reference checker that interprets `grammar.ebnf` directly and parses programs with it |
| [tests/valid/](tests/valid) | Programs that must parse, one file per area of the grammar |
| [tests/invalid/](tests/invalid) | Programs that must fail on the line, and with the code, stated in their first line |

The reference programs are [docs/examples/todo.unio](../docs/examples/todo.unio) and the utilities in [examples/](../examples). They, the standard library in [stdlib/](../stdlib), and the tools in [tools/](../tools) are checked with the same grammar; [check_api.py](check_api.py) checks that they call only what the standard library declares.

The Python scripts are temporary. Their Unio versions are in [tools/](../tools); once the stage 3 interpreter runs them and they give the same results, CI switches to them and the Python scripts are deleted.

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

Name resolution, types, error sets, exhaustiveness, mutability, and resources belong to stage 2 of the plan.

## Syntax-level diagnostic codes

These codes are fixed by the grammar. The full list of the first 50 codes, with explanations and fixes, is a separate stage-0 deliverable.

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

## RFC 0001

The EBNF supports contextual `named` and labels in function shapes; `named_parameters.unio` and negative fixtures are in the corpus. Both API checkers extract the actual parameter binding. Group/default ordering, required labels at calls and E0040–E0048 belong to the future compiler, not these scripts. Normative rules: [contracts](../docs/language-contracts.md), [RFC](../docs/rfcs/0001-language-contracts.md).
