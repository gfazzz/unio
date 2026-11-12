#!/usr/bin/env python3
"""Formatting and consistency checks for documentation and specification files.

- UTF-8, a final newline, no trailing white space, no tabs (except the corpus
  and control files that test tabs);
- relative links in Markdown files point to existing files;
- every English document has a Russian localization.
"""

import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CHECKED = (".md", ".unio", ".ebnf", ".py", ".yml", ".json", ".toml")
SKIP_DIRS = {".git", "target", "node_modules", "__pycache__"}
TABS_ALLOWED = {
    os.path.join("spec", "tests", "invalid", "tab_indent.unio"),
    os.path.join("spec", "tests", "control", "tab_indentation.unio"),
}

# English document -> Russian localization.
SPECIAL = {
    "README.md": "docs/ru/README.md",
    "spec/README.md": "docs/ru/spec.md",
    "SECURITY.md": "docs/ru/security.md",
}


def files():
    for dirpath, dirs, names in os.walk(ROOT):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for n in names:
            if n.endswith(CHECKED):
                yield os.path.relpath(os.path.join(dirpath, n), ROOT)


def localization(rel):
    rel = rel.replace(os.sep, "/")
    if rel in SPECIAL:
        return SPECIAL[rel]
    if rel.startswith("docs/") and not rel.startswith("docs/ru/") and rel.endswith((".md", ".unio")):
        return "docs/ru/" + rel[len("docs/"):]
    return None


def main():
    problems = []
    for rel in sorted(files()):
        path = os.path.join(ROOT, rel)
        try:
            text = open(path, encoding="utf-8").read()
        except UnicodeDecodeError:
            problems.append(f"{rel}: not UTF-8")
            continue
        if text and not text.endswith("\n"):
            problems.append(f"{rel}: no newline at end of file")
        for number, line in enumerate(text.split("\n"), 1):
            if line != line.rstrip():
                problems.append(f"{rel}:{number}: trailing white space")
            if "\t" in line and rel not in TABS_ALLOWED:
                problems.append(f"{rel}:{number}: tab character")
        if rel.endswith(".md"):
            for target in re.findall(r"\]\(([^)#\s]+)(?:#[^)]*)?\)", text):
                if re.match(r"[a-z]+:", target):
                    continue
                full = os.path.normpath(os.path.join(os.path.dirname(path), target))
                if not os.path.exists(full):
                    problems.append(f"{rel}: broken link {target}")
        ru = localization(rel)
        if ru and not os.path.exists(os.path.join(ROOT, ru)):
            problems.append(f"{rel}: missing Russian localization {ru}")
    for p in problems:
        print(p)
    print("documentation checks passed" if not problems else f"{len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
