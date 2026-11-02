#!/usr/bin/env python3
"""Checks that reference programs use only the documented standard library.

Reads the standard library from its Unio sources in stdlib/: every file must
parse with spec/grammar.ebnf, and external functions may appear only in an
unsafe module. Then every call in the given programs, and in the standard
library itself, must resolve to:

  - a function, type, or enum variant declared in the program itself;
  - a function of an imported module (module.function or from ... import);
  - a prelude function, a built-in constructor, or a prelude variant;
  - a parameter or a local name of the program (a function value);
  - a method of the standard library or one declared in the program;
  - a variant of an enum given with its name (Value.Text).

Usage:
    check_api.py [PROGRAM...]     default: stdlib/, docs/examples/, examples/, tools/
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
        elif rule in ("struct_decl", "interface_decl"):
            types.add(names_in(kids)[0])
        elif rule == "error_decl":
            types.add(names_in(kids)[1])     # kids[0] is the contextual keyword error
        elif rule == "enum_decl":
            enum = names_in(kids)[0]
            types.add(enum)
            for r2, k2 in walk(("x", kids)):
                if r2 == "variant":
                    variants.setdefault(enum, set()).add(k2[0].value)
    return functions, types, variants


def top_items(tree):
    """(rule, kids) of each top-level item: declaration, import, extend_decl, test_block."""
    for kid in tree[1]:
        if not isinstance(kid, cg.Token) and kid[0] == "top_item":
            yield kid[1][0]


def is_public(kids):
    return bool(kids) and isinstance(kids[0], cg.Token) and kids[0].kind == "public"


def signature_name(node):
    for rule, kids in walk(node):
        if rule == "function_sig":
            return names_in(kids)[0]
    return None


def module_of(rel):
    parts = rel.replace(os.sep, "/")[:-len(".unio")].split("/")
    if parts[:2] == ["std", "builtin"] or parts == ["std", "prelude"]:
        return "prelude"
    return ".".join(parts)


def load_api(grammar, problems):
    """Public API of the standard library, read from its Unio sources in stdlib/."""
    root = os.path.join(ROOT, "stdlib")
    modules = {}       # "std.files" -> public functions
    prelude = set()    # functions visible everywhere, including built-in constructors
    methods = set()    # methods of built-in and library types
    types, variants = set(), {}
    files = sorted(glob.glob(os.path.join(root, "**", "*.unio"), recursive=True))
    if not files:
        problems.append("stdlib/: no sources found")
    for path in files:
        rel = os.path.relpath(path, root)
        tree, err = cg.check_source(open(path, encoding="utf-8").read(), grammar, build_tree=True)
        if err:
            problems.append(f"stdlib/{rel}: does not parse: line {err[1]}: {err[3]}")
            continue
        unsafe = any(not isinstance(k, cg.Token) and k[0] == "module_header" for k in tree[1])
        if not unsafe and any(rule == "external_decl" for rule, _ in walk(tree)):
            problems.append(f"stdlib/{rel}: external function outside an unsafe module")
        module = module_of(rel)
        public_functions = set()
        for rule, kids in top_items(tree):
            if rule == "declaration" and is_public(kids):
                inner = kids[1]
                if inner[0] in ("function_decl", "external_decl"):
                    public_functions.add(signature_name(inner))
                else:
                    _f, t, v = declarations(("x", [inner]))
                    types |= t
                    for k, vs in v.items():
                        variants.setdefault(k, set()).update(vs)
            elif rule == "extend_decl":
                with_interface = any(isinstance(k, cg.Token) and k.kind == "with" for k in kids)
                for k in kids:
                    if isinstance(k, cg.Token) or k[0] != "member":
                        continue
                    if with_interface or is_public(k[1]):
                        methods.add(signature_name(k))
            elif rule == "declaration":
                inner = kids[0]
                if inner[0] == "interface_decl":
                    for r2, k2 in walk(inner):
                        if r2 == "function_sig":
                            methods.add(names_in(k2)[0])
        for rule, kids in top_items(tree):
            if rule == "declaration" and is_public(kids) and kids[1][0] == "interface_decl":
                for r2, k2 in walk(kids[1]):
                    if r2 == "function_sig":
                        methods.add(names_in(k2)[0])
        if module == "prelude":
            prelude |= public_functions
        else:
            modules.setdefault(module, set()).update(public_functions)
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
    locals_ = set()
    for rule, kids in walk(tree):
        if rule in ("param", "let_statement"):
            locals_ |= set(names_in(kids)[:1])
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
    # A module of the same package: a file next to the program (rule L-22).
    modules = dict(modules)
    lib_types = set(lib_types)
    lib_variants = {k: set(v) for k, v in lib_variants.items()}
    for module in set(aliases.values()) | set(imported.values()):
        if module in modules:
            continue
        local = os.path.join(os.path.dirname(path), *module.split(".")) + ".unio"
        if not os.path.exists(local):
            continue
        local_tree, local_err = cg.check_source(open(local, encoding="utf-8").read(), grammar, True)
        if local_err:
            problems.append(f"{local}: does not parse: line {local_err[1]}: {local_err[3]}")
            continue
        public = set()
        for rule, kids in top_items(local_tree):
            if rule == "declaration" and is_public(kids):
                inner = kids[1]
                if inner[0] in ("function_decl", "external_decl"):
                    public.add(signature_name(inner))
                else:
                    _f, t, v = declarations(("x", [inner]))
                    public |= t
                    lib_types |= t
                    for k, vs in v.items():
                        lib_variants.setdefault(k, set()).update(vs)
            elif rule == "extend_decl":
                for k in kids:
                    if not isinstance(k, cg.Token) and k[0] == "member":
                        methods = methods | {signature_name(k)}
        modules[module] = public
    for alias, module in aliases.items():
        if module not in modules:
            problems.append(f"{path}: module {module} is not in stdlib/ or in the package")
    for name, module in imported.items():
        if module not in modules:
            problems.append(f"{path}: module {module} is not in stdlib/ or in the package")
        elif name not in modules[module] and name not in lib_types:
            problems.append(f"{path}: {module}.{name} is not declared public")
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
                    problems.append(f"{path}:{t.line}: {module}.{member} is not in stdlib/")
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
                problems.append(f"{path}:{t.line}: method {t.value} is not in stdlib/")
            continue
        if prev is not None and prev.kind == "function":
            continue
        known = (own_functions | own_types | plain_variants | prelude | PRELUDE_VARIANTS
                 | set(imported) | locals_)
        if t.value not in known:
            problems.append(f"{path}:{t.line}: {t.value} is neither declared nor in stdlib/")


def main(argv):
    grammar = cg.load_grammar()
    problems = []
    api = load_api(grammar, problems)
    programs = argv or sorted(glob.glob(os.path.join(ROOT, "stdlib", "**", "*.unio"), recursive=True)
                              + glob.glob(os.path.join(ROOT, "docs", "examples", "*.unio"))
                              + glob.glob(os.path.join(ROOT, "docs", "ru", "examples", "*.unio"))
                              + glob.glob(os.path.join(ROOT, "examples", "*.unio"))
                              + glob.glob(os.path.join(ROOT, "tools", "*.unio")))
    for path in programs:
        check_program(path, grammar, api, problems)
    for p in problems:
        print(p)
    count = len(programs)
    print(f"{count} file(s) use only the standard library API" if not problems
          else f"{len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
