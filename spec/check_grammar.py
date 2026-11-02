#!/usr/bin/env python3
"""Reference checker for the Unio 0.1 core grammar.

The checker reads spec/grammar.ebnf and interprets its syntactic rules
directly (ordered choice with backtracking and memoization), so the EBNF file
stays the single source of the grammar. The lexer below implements section L
of the grammar; the keyword and punctuation lists are taken from the comments
of that section.

Usage:
    check_grammar.py FILE...          check files, report the first error
    check_grammar.py --tree FILE      print the parse tree
    check_grammar.py --corpus DIR     check spec/tests: valid/ must parse,
                                      invalid/ must fail where "# expect-error:"
                                      says
    check_grammar.py --self-check     check the grammar file for consistency

Only the Python standard library is used.
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
GRAMMAR = os.path.join(HERE, "grammar.ebnf")

# Contextual keywords, read from section L of the grammar: lexed as NAME.
CONTEXTUAL = set()
# Token classes supplied by the lexer instead of EBNF rules.
TOKEN_CLASSES = {"NAME", "INT", "FLOAT", "TEXT", "NEWLINE", "INDENT", "DEDENT", "EOF"}
# Lexical rules described in the EBNF for people; the lexer implements them.
LEXICAL_RULES = {
    "letter", "digit", "decimal_int", "exponent", "hex_digit",
    "text_char", "escape", "interpolation",
} | TOKEN_CLASSES


# ---------------------------------------------------------------- EBNF reader

class GrammarError(Exception):
    pass


def strip_comments(text):
    return re.sub(r"\(\*.*?\*\)", lambda m: " " * len(m.group(0)), text, flags=re.S)


def read_word_list(text, start, end):
    """Words between two markers inside grammar comments."""
    i = text.index(start)
    j = text.index(end, i)
    return text[i + len(start):j].split()


def load_grammar(path=GRAMMAR):
    raw = open(path, encoding="utf-8").read()
    keywords = set(read_word_list(raw, "requires an RFC):", "Contextual keywords"))
    CONTEXTUAL.clear()
    CONTEXTUAL.update(read_word_list(raw, "where a rule uses them):", "A contextual keyword"))
    reserved = set(read_word_list(raw, "(using them is E0032):", "Punctuation"))
    punct = read_word_list(raw, "Punctuation and operators:", "*)")
    text = strip_comments(raw)
    rules = {}
    order = []
    for m in re.finditer(r"([A-Za-z_][A-Za-z0-9_]*)\s*=(.*?);(?=\s*(?:[A-Za-z_][A-Za-z0-9_]*\s*=|$))",
                         text, flags=re.S):
        name, body = m.group(1), m.group(2)
        if name in rules:
            raise GrammarError(f"rule {name} defined twice")
        order.append(name)
        if name in LEXICAL_RULES:
            continue
        rules[name] = parse_rule_body(name, body)
    return rules, order, keywords, reserved, punct


def tokenize_ebnf(body):
    pos = 0
    out = []
    while pos < len(body):
        c = body[pos]
        if c.isspace():
            pos += 1
        elif c in "\"'":
            end = body.index(c, pos + 1)
            out.append(("T", body[pos + 1:end]))
            pos = end + 1
        elif c.isalpha() or c == "_":
            m = re.match(r"[A-Za-z_][A-Za-z0-9_]*", body[pos:])
            out.append(("N", m.group(0)))
            pos += len(m.group(0))
        elif c in "|[]{}()":
            out.append((c, c))
            pos += 1
        elif body.startswith("...", pos):
            out.append(("...", "..."))
            pos += 3
        else:
            raise GrammarError(f"unexpected character {c!r} in EBNF")
    return out


def parse_rule_body(name, body):
    toks = tokenize_ebnf(body)
    pos = 0

    def alt():
        nonlocal pos
        options = [seq()]
        while pos < len(toks) and toks[pos][0] == "|":
            pos += 1
            options.append(seq())
        return options[0] if len(options) == 1 else ("alt", options)

    def seq():
        nonlocal pos
        items = []
        while pos < len(toks) and toks[pos][0] not in "|]})":
            kind, val = toks[pos]
            pos += 1
            if kind == "T":
                items.append(("t", val))
            elif kind == "N":
                items.append(("r", val))
            elif kind in "[{(":
                inner = alt()
                close = {"[": "]", "{": "}", "(": ")"}[kind]
                if pos >= len(toks) or toks[pos][0] != close:
                    raise GrammarError(f"rule {name}: missing {close}")
                pos += 1
                items.append(({"[": "opt", "{": "rep", "(": "grp"}[kind], inner))
            else:
                raise GrammarError(f"rule {name}: unexpected {val}")
        return items[0] if len(items) == 1 else ("seq", items)

    node = alt()
    if pos != len(toks):
        raise GrammarError(f"rule {name}: trailing tokens")
    return node


def self_check(rules, order, keywords, reserved, punct):
    problems = []
    used_rules, used_terms = set(), set()

    def walk(node):
        kind = node[0]
        if kind == "t":
            used_terms.add(node[1])
        elif kind == "r":
            used_rules.add(node[1])
        elif kind in ("alt", "seq"):
            for n in node[1]:
                walk(n)
        else:
            walk(node[1])

    for n in rules.values():
        walk(n)
    for r in sorted(used_rules):
        if r not in rules and r not in TOKEN_CLASSES:
            problems.append(f"rule {r} is used but not defined")
    reachable, stack = set(), ["file"]
    while stack:
        r = stack.pop()
        if r in reachable or r not in rules:
            continue
        reachable.add(r)
        sub = set()

        def collect(node):
            if node[0] == "r":
                sub.add(node[1])
            elif node[0] in ("alt", "seq"):
                for n in node[1]:
                    collect(n)
            elif node[0] != "t":
                collect(node[1])

        collect(rules[r])
        stack.extend(sub)
    for r in order:
        if r in rules and r not in reachable:
            problems.append(f"rule {r} is not reachable from file")
    words = {t for t in used_terms if re.fullmatch(r"[A-Za-z]+", t)}
    for w in sorted(words - keywords - CONTEXTUAL):
        problems.append(f"terminal {w!r} is not in the keyword list")
    for w in sorted((keywords | CONTEXTUAL) - words):
        problems.append(f"keyword {w!r} is not used by any rule")
    if keywords & CONTEXTUAL:
        problems.append("a word is both a keyword and a contextual keyword")
    symbols = {t for t in used_terms if not re.fullmatch(r"[A-Za-z_]+", t)}
    for s in sorted(symbols - set(punct)):
        problems.append(f"terminal {s!r} is not in the punctuation list")
    for s in sorted(set(punct) - symbols):
        problems.append(f"punctuation {s!r} is not used by any rule")
    if keywords & reserved:
        problems.append("a word is both a keyword and reserved")
    return problems


# ---------------------------------------------------------------- lexer

class Token:
    __slots__ = ("kind", "value", "line", "col", "parts")

    def __init__(self, kind, value, line, col, parts=None):
        self.kind, self.value, self.line, self.col, self.parts = kind, value, line, col, parts

    def __repr__(self):
        return f"{self.kind}:{self.value!r}@{self.line}:{self.col}"


class LexError(Exception):
    def __init__(self, code, message, line, col):
        super().__init__(message)
        self.code, self.message, self.line, self.col = code, message, line, col


OPEN = {"(": ")", "[": "]", "{": "}"}
CLOSE = {")", "]", "}"}
ESCAPES = set("ntr0\\\"{}")


class Lexer:
    def __init__(self, text, keywords, reserved, punct):
        self.text = text
        self.keywords, self.reserved = keywords, reserved
        self.punct = sorted(punct, key=len, reverse=True)
        self.pos, self.line, self.col = 0, 1, 1

    def error(self, code, message, line=None, col=None):
        raise LexError(code, message, line or self.line, col or self.col)

    def peek(self, k=0):
        i = self.pos + k
        return self.text[i] if i < len(self.text) else ""

    def advance(self, n=1):
        for _ in range(n):
            if self.text[self.pos] == "\n":
                self.line += 1
                self.col = 1
            else:
                self.col += 1
            self.pos += 1

    def tokens(self):
        """Tokens of a whole file, with NEWLINE, INDENT and DEDENT."""
        out = []
        levels = [0]
        while self.pos < len(self.text):
            # Start of a physical line: measure indentation.
            start_line = self.line
            width = 0
            while self.peek() in (" ", "\t"):
                if self.peek() == "\t":
                    self.error("E0001", "tab in indentation")
                width += 1
                self.advance()
            if self.peek() in ("\n", "\r", "#", ""):
                self.skip_rest_of_line()
                continue
            if width % 4:
                self.error("E0002", "indentation is not a multiple of 4 spaces", start_line, 1)
            if width > levels[-1]:
                if width != levels[-1] + 4:
                    self.error("E0003", "indentation grows by more than one level", start_line, 1)
                prev = out[-2] if len(out) >= 2 else None
                if prev is None or prev.kind != ":":
                    self.error("E0003", "unexpected indentation: the previous line does not end with ':'",
                               start_line, 1)
                levels.append(width)
                out.append(Token("INDENT", "", self.line, 1))
            else:
                while width < levels[-1]:
                    levels.pop()
                    out.append(Token("DEDENT", "", self.line, 1))
            self.logical_line(out)
        while len(levels) > 1:
            levels.pop()
            out.append(Token("DEDENT", "", self.line, 1))
        out.append(Token("EOF", "", self.line, self.col))
        return out

    def skip_rest_of_line(self):
        while self.peek() not in ("\n", ""):
            self.advance()
        if self.peek() == "\n":
            self.advance()

    def logical_line(self, out):
        depth = []
        while True:
            c = self.peek()
            if c == "":
                if depth:
                    self.error("E0009", f"unclosed '{depth[-1][0]}'", depth[-1][1], depth[-1][2])
                break
            if c in (" ", "\t"):
                self.advance()
                continue
            if c == "\r" and self.peek(1) == "\n":
                self.advance()
                continue
            if c == "#":
                while self.peek() not in ("\n", ""):
                    self.advance()
                continue
            if c == "\n":
                self.advance()
                if depth:
                    continue
                break
            tok = self.token(depth)
            out.append(tok)
        out.append(Token("NEWLINE", "", self.line, self.col))

    def token(self, depth):
        line, col = self.line, self.col
        c = self.peek()
        if c.isalpha() or c == "_":
            m = re.match(r"[A-Za-z_][A-Za-z0-9_]*", self.text[self.pos:])
            word = m.group(0)
            self.advance(len(word))
            if word in self.reserved:
                self.error("E0032", f"'{word}' is reserved for a later release", line, col)
            return Token(word if word in self.keywords else "NAME", word, line, col)
        if c.isdigit():
            return self.number(line, col)
        if c == '"':
            return self.text_literal(line, col)
        for p in self.punct:
            if self.text.startswith(p, self.pos):
                self.advance(len(p))
                if p in OPEN:
                    depth.append((p, line, col))
                elif p in CLOSE:
                    if not depth:
                        self.error("E0010", f"unmatched '{p}'", line, col)
                    if OPEN[depth[-1][0]] != p:
                        self.error("E0010", f"'{p}' does not match '{depth[-1][0]}'", line, col)
                    depth.pop()
                return Token(p, p, line, col)
        if ord(c) > 127 or not c.isprintable():
            self.error("E0007", f"invalid character {c!r}", line, col)
        self.error("E0007", f"unexpected character {c!r}", line, col)

    def number(self, line, col):
        rest = self.text[self.pos:]
        m = (re.match(r"0x[0-9A-Fa-f][0-9A-Fa-f_]*", rest)
             or re.match(r"0b[01][01_]*", rest))
        kind = "INT"
        if not m:
            m = re.match(r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?", rest)
            kind = "FLOAT" if m else kind
        if not m:
            m = re.match(r"[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*", rest)
            kind = "FLOAT" if m else kind
        if not m:
            m = re.match(r"[0-9][0-9_]*", rest)
            kind = "INT"
        word = m.group(0)
        after = rest[len(word):len(word) + 1]
        if after and (after.isalnum() or after == "_"):
            self.error("E0008", f"invalid number literal starting with {word!r}", line, col)
        self.advance(len(word))
        return Token(kind, word, line, col)

    def text_literal(self, line, col):
        self.advance()  # opening quote
        parts = []
        while True:
            c = self.peek()
            if c in ("", "\n"):
                self.error("E0005", "unterminated text literal", line, col)
            if c == '"':
                self.advance()
                return Token("TEXT", "", line, col, parts)
            if c == "\\":
                n = self.peek(1)
                if n == "u" and self.peek(2) == "{":
                    m = re.match(r"\\u\{[0-9A-Fa-f]{1,6}\}", self.text[self.pos:])
                    if not m:
                        self.error("E0006", "invalid \\u{...} escape")
                    self.advance(len(m.group(0)))
                    continue
                if n not in ESCAPES:
                    self.error("E0006", f"invalid escape sequence '\\{n}'")
                self.advance(2)
                continue
            if c == "}":
                self.error("E0011", "'}' in text must be written as '\\}'")
            if c == "{":
                iline, icol = self.line, self.col
                self.advance()
                sub = []
                depth = [("{", iline, icol)]
                while True:
                    while self.peek() == " ":
                        self.advance()
                    ch = self.peek()
                    if ch in ("", "\n"):
                        self.error("E0011", "unterminated interpolation", iline, icol)
                    if ch == "}" and len(depth) == 1:
                        self.advance()
                        break
                    sub.append(self.token(depth))
                if not sub:
                    self.error("E0011", "empty interpolation", iline, icol)
                parts.append(sub)
                continue
            self.advance()


# ---------------------------------------------------------------- parser

class Fail(Exception):
    pass


class Parser:
    """Ordered choice over the EBNF rules, memoized per (rule, position)."""

    def __init__(self, rules, tokens, build_tree=False):
        self.rules = rules
        self.toks = tokens
        self.memo = {}
        self.far = 0
        self.expected = set()
        self.build_tree = build_tree
        self.sub_errors = []

    def note(self, pos, what):
        if pos > self.far:
            self.far, self.expected = pos, {what}
        elif pos == self.far:
            self.expected.add(what)

    def parse(self, start="file"):
        result = self.rule(start, 0)
        if result is None:
            return None
        end, tree = result
        if end != len(self.toks):
            self.note(end, "end of input")
            return None
        return tree if self.build_tree else True

    def rule(self, name, pos):
        key = (name, pos)
        if key in self.memo:
            return self.memo[key]
        self.memo[key] = None  # guards against accidental left recursion
        r = self.match(self.rules[name], pos)
        if r is not None and name == "generic_call" and not self.generic_allowed(pos):
            r = None
        if r is not None:
            end, kids = r
            r = (end, (name, kids) if self.build_tree else None)
        self.memo[key] = r
        return r

    def generic_allowed(self, pos):
        # Prose rule: "[" starts generic arguments only if the first token
        # inside is a NAME that begins with an upper-case letter.
        nxt = self.toks[pos + 1] if pos + 1 < len(self.toks) else None
        return nxt is not None and nxt.kind == "NAME" and nxt.value[:1].isupper()

    def match(self, node, pos):
        kind = node[0]
        if kind == "t":
            tok = self.toks[pos] if pos < len(self.toks) else None
            want = node[1]
            ok = tok is not None and (tok.kind == want or (tok.kind == "NAME" and tok.value == want
                                                           and (want == "_" or want in CONTEXTUAL)))
            if not ok:
                self.note(pos, repr(want))
                return None
            return pos + 1, [tok] if self.build_tree else None
        if kind == "r":
            name = node[1]
            if name in TOKEN_CLASSES:
                tok = self.toks[pos] if pos < len(self.toks) else None
                if tok is None or tok.kind != name:
                    self.note(pos, name)
                    return None
                if name == "TEXT" and not self.check_text(tok):
                    return None
                return pos + 1, [tok] if self.build_tree else None
            r = self.rule(name, pos)
            if r is None:
                return None
            return r[0], [r[1]] if self.build_tree else None
        if kind == "seq":
            kids = []
            for item in node[1]:
                r = self.match(item, pos)
                if r is None:
                    return None
                pos = r[0]
                if self.build_tree:
                    kids.extend(r[1])
            return pos, kids
        if kind == "alt":
            for option in node[1]:
                r = self.match(option, pos)
                if r is not None:
                    return r
            return None
        if kind in ("opt", "grp"):
            r = self.match(node[1], pos)
            if r is None:
                return (pos, []) if kind == "opt" else None
            return r
        if kind == "rep":
            kids = []
            while True:
                r = self.match(node[1], pos)
                if r is None or r[0] == pos:
                    return pos, kids
                pos = r[0]
                if self.build_tree:
                    kids.extend(r[1])
        raise AssertionError(kind)

    def check_text(self, tok):
        for sub in tok.parts or []:
            p = Parser(self.rules, sub, self.build_tree)
            tree = p.parse("expression")
            if tree is None:
                bad = sub[min(p.far, len(sub) - 1)]
                self.sub_errors.append((bad.line, bad.col, sorted(p.expected)))
                return False
        return True


# ---------------------------------------------------------------- driver

def check_source(text, grammar, build_tree=False):
    """Returns (tree, error). error is (code, line, col, message) or None."""
    rules, _order, keywords, reserved, punct = grammar
    if text.startswith("\ufeff"):
        text = text[1:]
    try:
        tokens = Lexer(text, keywords, reserved, punct).tokens()
    except LexError as e:
        return None, (e.code, e.line, e.col, e.message)
    p = Parser(rules, tokens, build_tree)
    tree = p.parse("file")
    if tree is not None:
        return tree, None
    if p.sub_errors and p.toks[p.far].kind == "TEXT":
        line, col, exp = p.sub_errors[-1]
        return None, ("syntax", line, col, "in interpolation: expected " + ", ".join(exp))
    tok = p.toks[min(p.far, len(p.toks) - 1)]
    found = tok.kind if tok.kind in TOKEN_CLASSES else repr(tok.value)
    exp = ", ".join(sorted(p.expected))
    return None, ("syntax", tok.line, tok.col, f"expected {exp}; found {found}")


def format_tree(node, indent=0):
    lines = []
    if isinstance(node, Token):
        if node.kind == "TEXT":
            lines.append("  " * indent + "TEXT")
            for sub in node.parts or []:
                lines.append("  " * (indent + 1) + "interpolation")
        elif node.kind in ("NEWLINE", "INDENT", "DEDENT", "EOF"):
            pass
        else:
            lines.append("  " * indent + (node.value if node.kind == node.value else
                                          f"{node.kind} {node.value}"))
        return lines
    name, kids = node
    # Collapse chains of rules that only wrap a single child.
    while len(kids) == 1 and not isinstance(kids[0], Token):
        name, kids = kids[0]
    if len(kids) == 1:
        return format_tree(kids[0], indent)
    lines.append("  " * indent + name)
    for k in kids:
        lines.extend(format_tree(k, indent + 1))
    return lines


def read_expectation(text):
    m = re.search(r"^# expect-error:\s*(\d+)(?:\s+(E\d{4}))?\s*$", text, flags=re.M)
    if not m:
        return None
    return int(m.group(1)), m.group(2)


def run_corpus(root, grammar):
    failures = 0
    count = 0
    for kind in ("valid", "invalid"):
        folder = os.path.join(root, kind)
        for dirpath, _dirs, files in os.walk(folder):
            for f in sorted(files):
                if not f.endswith(".unio"):
                    continue
                path = os.path.join(dirpath, f)
                rel = os.path.relpath(path, root)
                text = open(path, encoding="utf-8").read()
                _tree, err = check_source(text, grammar)
                count += 1
                if kind == "valid":
                    if err:
                        failures += 1
                        print(f"FAIL {rel}: {err[1]}:{err[2]}: {err[0]}: {err[3]}")
                    else:
                        print(f"ok   {rel}")
                    continue
                exp = read_expectation(text)
                if exp is None:
                    failures += 1
                    print(f"FAIL {rel}: missing '# expect-error: LINE [CODE]'")
                elif err is None:
                    failures += 1
                    print(f"FAIL {rel}: parsed, but an error was expected on line {exp[0]}")
                elif err[1] != exp[0] or (exp[1] and err[0] != exp[1]):
                    failures += 1
                    want = f"line {exp[0]}" + (f" {exp[1]}" if exp[1] else "")
                    print(f"FAIL {rel}: expected {want}, got line {err[1]} {err[0]}: {err[3]}")
                else:
                    print(f"ok   {rel} ({err[0]} on line {err[1]})")
    print(f"{count - failures}/{count} passed")
    return failures == 0


def main(argv):
    grammar = load_grammar()
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    if argv[0] == "--self-check":
        problems = self_check(*grammar)
        for p in problems:
            print("grammar:", p)
        print("grammar is consistent" if not problems else f"{len(problems)} problem(s)")
        return 1 if problems else 0
    if argv[0] == "--corpus":
        problems = self_check(*grammar)
        for p in problems:
            print("grammar:", p)
        ok = run_corpus(argv[1] if len(argv) > 1 else os.path.join(HERE, "tests"), grammar)
        return 0 if ok and not problems else 1
    tree_mode = argv[0] == "--tree"
    files = argv[1:] if tree_mode else argv
    status = 0
    for path in files:
        tree, err = check_source(open(path, encoding="utf-8").read(), grammar, tree_mode)
        if err:
            status = 1
            print(f"{path}:{err[1]}:{err[2]}: {err[0]}: {err[3]}")
        elif tree_mode:
            print("\n".join(format_tree(tree)))
        else:
            print(f"{path}: ok")
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
