#!/usr/bin/env python3
"""Checks that reference programs use only the documented standard library.

Reads docs/stdlib.md: every ```unio block must parse with spec/grammar.ebnf
(a block of bare signatures is checked line by line with the rule
function_sig). Then every call in the given programs must resolve to:

  - a function, type, or enum variant declared in the program itself;
  - a function of an imported module (module.function or from ... import);
  - a prelude function, a built-in constructor, or a prelude variant;
  - a method listed in docs/stdlib.md or declared in the program;
  - a variant of an enum given with its name (Value.Text).

Usage:
    check_api.py [PROGRAM...]     default: docs/examples/*.unio examples/*.unio
"""

import glob
import os
import re
import sys

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import check_grammar as cg  # noqa: E402

PRELUDE_VARIANTS = {"some", "none", "ok", "error"}
BUILTIN_TYPES = {"Int", "Float", "Bool", "Text", "Bytes", "List", "Map", "Set", "Option",
                 "Result", "Tuple", "Path", "Function", "Never", "Self"}


def unqualified_variant_clashes(tree, own_types, own_variants):
    """Rule L-6a: a variant whose name is also a visible type is written Enum.Variant."""
    clash = set()
    for enum, vs in own_variants.items():
        clash |= {v for v in vs if v in BUILTIN_TYPES or v in own_types}
    found = []
    for rule, kids in walk(tree):
        if rule in ("primary", "primary_pattern", "pattern_name") and len(kids) == 1 \
                and isinstance(kids[0], cg.Token) and kids[0].value in clash:
            found.append(kids[0])
    return found


def walk(node):
    """Yields (rule, kids) for every rule node of a parse tree."""
    if isinstance(node, cg.Token):
        return
    name, kids = node
    yield name, kids
    for k in kids:
        yield from walk(k)


def names_in(kids):
    return [k.value for k in kids if isinstance(k, cg.Token) and k.kind == "NAME"]


def declarations(tree):
    """Functions, types and variants declared in a parsed file."""
    functions, types, variants = set(), set(), {}
    for rule, kids in walk(tree):
        if rule == "function_sig":
            functions.add(names_in(kids)[0])
        elif rule in ("struct_decl", "error_decl", "interface_decl"):
            types.add(names_in(kids)[0])
        elif rule == "enum_decl":
            enum = names_in(kids)[0]
            types.add(enum)
            for r2, k2 in walk(("x", kids)):
                if r2 == "variant":
                    variants.setdefault(enum, set()).add(names_in(k2)[0])
    return functions, types, variants


def parse_block(code, grammar, where, problems):
    lines = [l for l in code.split("\n") if l.strip() and not l.strip().startswith("#")]
    if lines and all(l.startswith("function ") for l in lines):
        sigs = set()
        for line in lines:
            toks = cg.Lexer(line + "\n", *grammar[2:]).tokens()
            toks = [t for t in toks if t.kind not in ("NEWLINE", "EOF")]
            p = cg.Parser(grammar[0], toks)
            if p.parse("function_sig") is None:
                problems.append(f"{where}: signature does not parse: {line}")
                continue
            sigs.add(toks[1].value)
        return sigs, set(), {}
    tree, err = cg.check_source(code, grammar, build_tree=True)
    if err:
        problems.append(f"{where}: block does not parse: line {err[1]}: {err[3]}")
        return set(), set(), {}
    return declarations(tree)


def load_api(grammar, problems):
    path = os.path.join(ROOT, "docs", "stdlib.md")
    text = open(path, encoding="utf-8").read()
    modules = {}       # "std.files" -> functions
    prelude = set()    # prelude functions and built-in constructors
    methods = set()    # methods of built-in and library types
    types, variants = set(), {}
    section = None
    for part in re.split(r"(?m)^(#{2,3} .*)$", text):
        heading = re.match(r"#{2,3} (.*)", part)
        if heading:
            title = heading.group(1).strip()
            if title.startswith("std."):
                section = ("module", title)
            elif title == "Prelude":
                section = ("prelude", None)
            elif section and section[0] in ("builtin", "type") or title == "Built-in types":
                section = ("type", title) if title != "Built-in types" else ("builtin", None)
            else:
                section = ("other", title)
            continue
        for code in re.findall(r"```unio\n(.*?)```", part, flags=re.S):
            where = f"docs/stdlib.md ({section[1] if section else 'top'})"
            functions, t, v = parse_block(code, grammar, where, problems)
            types |= t
            for k, vs in v.items():
                variants.setdefault(k, set()).update(vs)
            # Methods of interfaces declared in a block count as methods.
            for line in code.split("\n"):
                m = re.match(r"\s+function ([a-z_][a-z0-9_]*)\(self", line)
                if m:
                    methods.add(m.group(1))
            if not section or section[0] in ("other",):
                continue
            if section[0] == "module":
                sigs = [l for l in code.split("\n") if l.startswith("function ")]
                bare = {re.match(r"function (\w+)", l).group(1) for l in sigs}
                if all(re.search(r"\(var self|\(self", l) for l in sigs) and sigs:
                    methods |= bare
                else:
                    modules.setdefault(section[1], set()).update(functions)
            elif section[0] == "prelude":
                prelude |= functions
            elif section[0] == "type":
                for f in functions:
                    (prelude if f[:1].isupper() else methods).add(f)
    return modules, prelude, methods, types, variants


def tokens_with_interpolations(tokens):
    for t in tokens:
        yield t
        if t.kind == "TEXT":
            for sub in t.parts or []:
                yield from tokens_with_interpolations(sub)
                yield cg.Token("NEWLINE", "", t.line, t.col)


def check_program(path, grammar, api, problems):
    modules, prelude, methods, lib_types, lib_variants = api
    text = open(path, encoding="utf-8").read()
    tree, err = cg.check_source(text, grammar, build_tree=True)
    if err:
        problems.append(f"{path}: does not parse: line {err[1]}: {err[3]}")
        return
    own_functions, own_types, own_variants = declarations(tree)
    for tok in unqualified_variant_clashes(tree, own_types, own_variants):
        enum = next(e for e, vs in own_variants.items() if tok.value in vs)
        problems.append(f"{path}:{tok.line}: variant {tok.value} has the name of a type; "
                        f"write {enum}.{tok.value} (L-6a)")
    aliases, imported = {}, {}
    for rule, kids in walk(tree):
        if rule != "import":
            continue
        words = [k.value for k in kids if isinstance(k, cg.Token)]
        if words[0] == "import":
            module = ".".join(names_in(kids[1][1]) if not isinstance(kids[1], cg.Token) else [kids[1].value])
            alias = names_in(kids)[-1] if "as" in words else module.split(".")[-1]
            aliases[alias] = module
        else:
            mod_node = kids[1]
            module = ".".join(names_in(mod_node[1])) if not isinstance(mod_node, cg.Token) else mod_node.value
            for name in names_in(kids[3:]):
                imported[name] = module
    for alias, module in aliases.items():
        if module not in modules:
            problems.append(f"{path}: module {module} is not in docs/stdlib.md")
    for name, module in imported.items():
        if module not in modules:
            problems.append(f"{path}: module {module} is not in docs/stdlib.md")
        elif name not in modules[module] and name not in lib_types:
            problems.append(f"{path}: {module}.{name} is not in docs/stdlib.md")
    all_variants = {}
    for src in (own_variants, lib_variants):
        for enum, vs in src.items():
            all_variants.setdefault(enum, set()).update(vs)
    plain_variants = set().union(*own_variants.values()) if own_variants else set()
    lexed = cg.Lexer(text, *grammar[2:]).tokens()
    toks = list(tokens_with_interpolations(lexed))
    for i, t in enumerate(toks):
        if t.kind != "NAME":
            continue
        prev = toks[i - 1] if i else None
        nxt = toks[i + 1] if i + 1 < len(toks) else None
        if nxt is not None and nxt.kind == "." and i + 2 < len(toks) and toks[i + 2].kind == "NAME":
            member = toks[i + 2].value
            if prev is not None and prev.kind in (".", "?."):
                continue
            if t.value in aliases:
                module = aliases[t.value]
                if module in modules and member not in modules[module]:
                    problems.append(f"{path}:{t.line}: {module}.{member} is not in docs/stdlib.md")
                continue
            if t.value in all_variants:
                if member not in all_variants[t.value]:
                    problems.append(f"{path}:{t.line}: {t.value} has no variant {member}")
                continue
        is_call = nxt is not None and nxt.kind == "("
        if not is_call:
            continue
        if prev is not None and prev.kind in (".", "?."):
            before = toks[i - 2] if i >= 2 else None
            if before is not None and before.kind == "NAME" and (before.value in aliases
                                                                 or before.value in all_variants):
                continue
            if t.value not in methods and t.value not in own_functions:
                problems.append(f"{path}:{t.line}: method {t.value} is not in docs/stdlib.md")
            continue
        if prev is not None and prev.kind == "function":
            continue
        known = (own_functions | own_types | plain_variants | prelude | PRELUDE_VARIANTS
                 | set(imported))
        if t.value not in known:
            problems.append(f"{path}:{t.line}: {t.value} is neither declared nor in docs/stdlib.md")


def main(argv):
    grammar = cg.load_grammar()
    problems = []
    api = load_api(grammar, problems)
    programs = argv or sorted(glob.glob(os.path.join(ROOT, "docs", "examples", "*.unio"))
                              + glob.glob(os.path.join(ROOT, "docs", "ru", "examples", "*.unio"))
                              + glob.glob(os.path.join(ROOT, "examples", "*.unio")))
    for path in programs:
        check_program(path, grammar, api, problems)
    for p in problems:
        print(p)
    count = len(programs)
    print(f"{count} program(s) use only the documented API" if not problems
          else f"{len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
