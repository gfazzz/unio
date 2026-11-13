#!/usr/bin/env python3
"""Reference checker of the test coverage that requirement T-5b asks CI to enforce.

Fails when a syntactic rule of spec/grammar.ebnf has no valid or no invalid file in the
corpus spec/tests, or when a code of the diagnostic catalog has no program in the control
set spec/tests/control, unless the exemption is documented.

- A valid file covers the rules of its parse tree.
- An invalid file covers the rules in progress where it fails: every rule whose attempt was
  on the way when the parser noted what it expected at the farthest token
  (check_grammar.failing_rules).
- A rule is exempt by a comment of the grammar "Corpus exemption: RULE has no valid file" or
  "... no invalid file", which gives the reason.
- A control program is a file below spec/tests/control, other than a reference fix or a hidden
  check, whose first line is "# expect-error: LINE CODE". The codes are the list CODES of
  compiler/unio/src/catalog.rs; a code without a program is exempt when it is listed, with its
  reason, in EXCLUDED of compiler/unio/tests/control.rs.

An exemption of something that is covered, or of a rule or code that does not exist, is an
error too, so that exemptions do not outlive their reason.

Usage: check_coverage.py   (from the root of the repository)
Exit code: 0 if everything is covered, 1 otherwise, 2 if an input cannot be read.
tools/check_coverage.unio is the Unio version; both print the same lines.
Only the Python standard library is used.
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import check_grammar  # noqa: E402

GRAMMAR = "spec/grammar.ebnf"
CORPUS = "spec/tests"
CONTROL = "spec/tests/control"
CATALOG = "compiler/unio/src/catalog.rs"
CONTROL_TEST = "compiler/unio/tests/control.rs"


def unio_files(folder):
    found = []
    for dirpath, _dirs, files in os.walk(folder):
        for f in files:
            if f.endswith(".unio"):
                found.append(os.path.join(dirpath, f))
    return sorted(found)


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def exemptions(grammar_text):
    """(rule, kind) pairs of the comments "Corpus exemption: RULE has no KIND file"."""
    found = []
    for piece in grammar_text.split("Corpus exemption: ")[1:]:
        words = piece.split()
        if len(words) >= 5 and words[1:3] == ["has", "no"] and words[4].startswith("file"):
            found.append((words[0], words[3]))
    return found


def quoted_block(text, start):
    """The texts in double quotes between the marker and the next "];"."""
    i = text.find(start)
    if i < 0:
        return None
    j = text.find("];", i)
    return re.findall(r'"([^"]*)"', text[i + len(start):j])


def is_code(word):
    return len(word) == 5 and word[0] == "E" and all(c in "0123456789" for c in word[1:])


def is_helper(name):
    return name.endswith(".fixed.unio") or name.endswith(".check.unio") or ".fixed." in name


def control_codes(root):
    codes = set()
    for dirpath, _dirs, files in os.walk(root):
        for f in files:
            if is_helper(f):
                continue
            first = read(os.path.join(dirpath, f)).split("\n", 1)[0]
            if first.startswith("# expect-error:"):
                words = first[len("# expect-error:"):].split()
                if len(words) == 2:
                    codes.add(words[1])
    return codes


def check_rules(grammar, grammar_text, problems):
    rules = check_grammar.syntactic_rules(grammar)
    valid, invalid = set(), set()
    for path in unio_files(os.path.join(CORPUS, "valid")):
        tree, err = check_grammar.check_source(read(path), grammar, build_tree=True)
        if err is None:
            valid.update(check_grammar.rules_in(tree))
    for path in unio_files(os.path.join(CORPUS, "invalid")):
        invalid.update(check_grammar.failing_rules(read(path), grammar))
    exempt = exemptions(grammar_text)
    for name, kind in exempt:
        if kind not in ("valid", "invalid"):
            problems.append(f"exemption of rule {name}: unknown kind {kind}")
        elif name not in rules:
            problems.append(f"exemption of rule {name}: no such syntactic rule")
        elif name in (valid if kind == "valid" else invalid):
            problems.append(f"rule {name} has {kind} corpus files; remove its exemption")
    covered = 0
    for name in rules:
        missing = [kind for kind, seen in (("valid", valid), ("invalid", invalid))
                   if name not in seen and (name, kind) not in exempt]
        for kind in missing:
            problems.append(f"rule {name} has no {kind} corpus file in {CORPUS}/{kind}")
        if not missing:
            covered += 1
    return covered, len(rules), len(exempt)


def check_codes(problems):
    catalog = quoted_block(read(CATALOG), "pub const CODES: &[&str] = &[")
    excluded_block = quoted_block(read(CONTROL_TEST), "const EXCLUDED: &[(&str, &str)] = &[")
    if catalog is None or excluded_block is None:
        raise OSError("the list CODES of the catalog or EXCLUDED of the control test is missing")
    excluded = [word for word in excluded_block if is_code(word)]
    programs = control_codes(CONTROL)
    covered = 0
    for code in catalog:
        if code in programs:
            covered += 1
            if code in excluded:
                problems.append(f"code {code} has a control program; remove it from EXCLUDED")
        elif code not in excluded:
            problems.append(f"code {code} has no control program in {CONTROL}")
    for code in sorted(programs):
        if code not in catalog:
            problems.append(f"control programs expect {code}, which is not a code of the catalog")
    for code in excluded:
        if code not in catalog:
            problems.append(f"EXCLUDED lists {code}, which is not a code of the catalog")
    return covered, len(catalog), len(excluded)


def main(argv):
    if argv and argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    try:
        grammar_text = read(GRAMMAR)
        grammar = check_grammar.load_grammar(os.path.abspath(GRAMMAR))
    except (OSError, check_grammar.GrammarError) as e:
        print(f"check_coverage: {e}")
        return 2
    problems = []
    try:
        rules = check_rules(grammar, grammar_text, problems)
        codes = check_codes(problems)
    except OSError as e:
        print(f"check_coverage: {e}")
        return 2
    for problem in problems:
        print(f"FAIL {problem}")
    print(f"grammar rules: {rules[0]}/{rules[1]} with a valid and an invalid corpus file"
          f" or an exemption ({rules[2]} exempt)")
    print(f"diagnostic codes: {codes[0]}/{codes[1]} with a control program ({codes[2]} exempt)")
    print("coverage is complete" if not problems else f"{len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
