#!/usr/bin/env python3
"""Writes or checks THIRD-PARTY-NOTICES: the crates of compiler/Cargo.lock that are compiled
into the shipped `unio` binary and into its runtime library `libunio.a` (`unio.lib`).

The crates are read from `cargo metadata --format-version 1 --locked`: those that the
workspace crates reach through normal dependencies, for any target. Build dependencies (the
code generators of Cranelift) and procedural macros run only while compiling and are not part
of either file, so they are left out. For each crate the file gives its name, version, licence
expression, upstream repository, the targets it is compiled for, its authors, the text of
every licence or copyright file it ships with its copyright lines (the MIT licence asks that
they accompany copies; the plain Apache License 2.0 text is given once, in LICENSE-APACHE),
and the NOTICE files the crate ships (Apache-2.0, section 4 (d)).

Usage, from the repository root:

    python3 .github/scripts/third_party_notices.py          # write the file
    python3 .github/scripts/third_party_notices.py --check  # fail if it is stale

Plan section 12, release condition 4. Standard library only, Python 3.9 or newer.
"""

import json
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WORKSPACE = os.path.join(ROOT, "compiler")
OUTPUT = os.path.join(ROOT, "THIRD-PARTY-NOTICES")

HEADER = """\
Third-party notices for Unio
============================

Unio is licensed under the MIT licence or the Apache License 2.0, at your option
(LICENSE-MIT, LICENSE-APACHE). The `unio` binary and its runtime library `libunio.a`
(`unio.lib` on Windows), which `unio build` links into every executable, also contain the
third-party code listed below. Each part is used under the licence named for it; the texts
of the MIT licence and of the Apache License 2.0 are in LICENSE-MIT and LICENSE-APACHE.
"Apache-2.0 WITH LLVM-exception" is the Apache License 2.0 with the LLVM exception, which
waives the attribution conditions of sections 4 (a), 4 (b), and 4 (d) for code that is
compiled or linked into an object form, as in an executable of `unio build`.

Besides the crates, both files contain the Rust standard library of the pinned toolchain
(compiler/rust-toolchain.toml; MIT OR Apache-2.0, https://github.com/rust-lang/rust), and an
executable of `unio build` contains what the system linker adds: the C runtime start files
and the system libraries of the platform (glibc on Linux, libSystem on macOS, the Microsoft
C runtime on Windows), under their own licences.

This file is generated from compiler/Cargo.lock by .github/scripts/third_party_notices.py;
CI fails when it is stale. Do not edit it by hand.
"""


def metadata():
    command = ["cargo", "metadata", "--format-version", "1", "--locked"]
    # Run inside the workspace, so that rustup uses compiler/rust-toolchain.toml.
    result = subprocess.run(command, cwd=WORKSPACE, stdout=subprocess.PIPE, check=True)
    return json.loads(result.stdout.decode("utf-8"))


def notice_files(package):
    """The NOTICE files of a crate, as it was downloaded from its registry."""
    directory = os.path.dirname(package["manifest_path"])
    try:
        names = os.listdir(directory)
    except OSError:
        return None
    return sorted(n for n in names if n.upper().startswith("NOTICE"))


def licence_files(package):
    """The licence and copyright files a crate ships, by name."""
    directory = os.path.dirname(package["manifest_path"])
    try:
        names = os.listdir(directory)
    except OSError:
        return None
    prefixes = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "UNLICENSE")
    return sorted(n for n in names if n.upper().startswith(prefixes) and
                  os.path.isfile(os.path.join(directory, n)))


def is_apache_text(text):
    """The plain text of the Apache License 2.0, which LICENSE-APACHE already reproduces."""
    return "Apache License" in text and "Version 2.0" in text and \
        "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION" in text


def shipped(data):
    """The packages reached from the workspace through normal dependencies, with the targets
    they are compiled for: None for every target, or the set of cfg conditions."""
    packages = {p["id"]: p for p in data["packages"]}
    nodes = {n["id"]: n for n in data["resolve"]["nodes"]}
    members = set(data["workspace_members"])
    conditions = {}
    queue = [(member, None) for member in sorted(members)]
    while queue:
        package, condition = queue.pop()
        known = conditions.get(package, set())
        if package in conditions and (known is None or condition in known):
            continue
        if condition is None:
            conditions[package] = None
        else:
            conditions[package] = known | {condition}
        for dep in nodes[package]["deps"]:
            target = packages[dep["pkg"]]
            if any("proc-macro" in t["kind"] for t in target["targets"]):
                continue
            for kind in dep["dep_kinds"]:
                if kind["kind"] is not None:
                    continue
                inner = kind["target"]
                queue.append((dep["pkg"], condition if inner is None else (condition or inner)))
    return [
        (packages[p], conditions[p])
        for p in conditions
        if p not in members
    ]


def render(data):
    rows = sorted(shipped(data), key=lambda pc: (pc[0]["name"], pc[0]["version"]))
    lines = [HEADER.rstrip("\n")]
    for package, condition in rows:
        notices = notice_files(package)
        if notices is None:
            notice = "unknown (the crate is not downloaded)"
        elif notices:
            notice = ", ".join(notices) + " (reproduced below)"
        else:
            notice = "none in the crate"
        targets = "all" if condition is None else ", ".join(sorted(condition))
        lines.append("")
        lines.append("{} {}".format(package["name"], package["version"]))
        lines.append("  Licence:  {}".format(package.get("license") or "see the crate"))
        lines.append("  Upstream: {}".format(
            package.get("repository") or "https://crates.io/crates/" + package["name"]))
        lines.append("  Targets:  {}".format(targets))
        lines.append("  NOTICE:   {}".format(notice))
        authors = package.get("authors") or []
        if authors:
            lines.append("  Authors:  {}".format("; ".join(authors)))
        licences = licence_files(package)
        if not licences:
            lines.append("  Licence files: none in the crate; the licence of its manifest applies, "
                         "with the copyright of its authors above")
        for name in licences or []:
            path = os.path.join(os.path.dirname(package["manifest_path"]), name)
            with open(path, encoding="utf-8", errors="replace") as f:
                text = f.read().rstrip()
            if is_apache_text(text):
                lines.append("  {}: the Apache License 2.0, as in LICENSE-APACHE".format(name))
                continue
            lines.append("")
            lines.append("  {}:".format(name))
            lines.extend(("  | " + line).rstrip() for line in text.splitlines())
        for name in notices or []:
            path = os.path.join(os.path.dirname(package["manifest_path"]), name)
            with open(path, encoding="utf-8") as f:
                text = f.read().rstrip()
            lines.append("")
            lines.extend(("  | " + line).rstrip() for line in text.splitlines())
    return "\n".join(lines) + "\n"


def main(argv):
    check = "--check" in argv[1:]
    text = render(metadata())
    if check:
        try:
            with open(OUTPUT, encoding="utf-8") as f:
                current = f.read()
        except OSError:
            current = None
        if current != text:
            print("THIRD-PARTY-NOTICES is stale: run python3 .github/scripts/third_party_notices.py",
                  file=sys.stderr)
            return 1
        print("THIRD-PARTY-NOTICES is up to date")
        return 0
    with open(OUTPUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print("wrote THIRD-PARTY-NOTICES")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
