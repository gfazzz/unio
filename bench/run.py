#!/usr/bin/env python3
"""Run the Unio benchmarks against equivalent programs in other languages.

Usage:
    python3 bench/run.py [--runs N] [--only NAME,...] [--json FILE] [--check-floor] [--unio PATH]
                         [--native]

With --native, each Unio program is also built with `unio build --release` and runs as a native
executable ("Unio native"); the ratios and the floor then refer to the native executable.

The rules of the comparison are in bench/README.md. Only the Python standard library is used.
"""

import argparse
import json
import os
import platform
import random
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BENCH = ROOT / "bench"
DATA = BENCH / "data"
BUILD = BENCH / "build"

# Requirement 7.1: text and JSON processing at least 0.5x of CPython 3.13, that is,
# Unio may take at most twice the time of Python. The benchmarks this floor applies to:
FLOOR = 0.5
FLOOR_BENCHMARKS = ("wc", "json", "text", "loop")

# RFC 0003: native executables take at most twice the time of the same algorithm in Go;
# the target is parity. Gated for release 0.1: wc and loop; for 0.2: every benchmark.
GO_LIMIT = 2.0
GO_GATED = ("wc", "loop")
# Peak resident memory: at most twice Go's peak plus 8 MB for the runtime, on the same gate.
GO_MEMORY_ALLOWANCE = 8_000_000

VOCABULARY = (
    "the quick brown fox jumps over lazy dog and then runs into forest where "
    "unio values text list map option result error test module function return "
    "мир текст слово данные строка поток файл время"
).split()


def generate_text(path, size, seed):
    """Lines of 4 to 16 words separated by single spaces; ASCII and Cyrillic; no empty lines."""
    rng = random.Random(seed)
    lines = []
    written = 0
    while written < size:
        line = " ".join(rng.choice(VOCABULARY) for _ in range(rng.randint(4, 16)))
        lines.append(line)
        written += len(line.encode()) + 1
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def generate_json(path, size, seed):
    """An array of records with integers, texts with escapes, booleans, null, and nesting."""
    rng = random.Random(seed)
    records = []
    written = 0
    while written < size:
        record = {
            "id": len(records),
            "name": " ".join(rng.choice(VOCABULARY) for _ in range(rng.randint(1, 4))),
            "note": rng.choice(["plain", "with \"quotes\"", "tab\there", "line\nbreak", "back\\slash"]),
            "score": rng.randint(-1000000, 1000000),
            "active": rng.random() < 0.5,
            "parent": None if rng.random() < 0.3 else rng.randint(0, 1000),
            "tags": [rng.choice(VOCABULARY) for _ in range(rng.randint(0, 5))],
            "point": {"x": rng.randint(-500, 500), "y": rng.randint(-500, 500)},
        }
        records.append(record)
        written += len(json.dumps(record, ensure_ascii=False, separators=(", ", ": "))) + 3
    path.write_text(json.dumps(records, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")


def ensure_data():
    DATA.mkdir(exist_ok=True)
    files = {
        "wc": (DATA / "text-20mb.txt", lambda p: generate_text(p, 20_000_000, 1)),
        "text": (DATA / "text-4mb.txt", lambda p: generate_text(p, 4_000_000, 2)),
        # A .txt name, so that the documentation checkers, which read every .json file of the
        # checkout, do not read the generated document.
        "json": (DATA / "json-10mb.txt", lambda p: generate_json(p, 10_000_000, 3)),
    }
    for _, (path, make) in files.items():
        if not path.exists():
            print(f"generating {path.relative_to(ROOT)}", file=sys.stderr)
            make(path)
    return {name: str(path) for name, (path, _) in files.items()}


def version(command):
    try:
        out = subprocess.run(command, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    # The JVM prints "Picked up JAVA_TOOL_OPTIONS: ..." before its version when that is set.
    lines = [line.strip() for line in (out.stdout + out.stderr).splitlines()
             if line.strip() and not line.startswith("Picked up ")]
    return lines[0] if lines else None


def find_unio(explicit):
    candidates = [explicit] if explicit else [
        ROOT / "compiler" / "target" / "release" / "unio",
        ROOT / "compiler" / "target" / "release" / "unio.exe",
    ]
    for candidate in candidates:
        if candidate and Path(candidate).exists():
            return str(candidate)
    return None


class Languages:
    """How to build and run each language; a language whose tools are missing is skipped."""

    def __init__(self, unio, native=False):
        self.unio = unio
        self.native = native
        self.python = sys.executable
        self.versions = {}
        self.missing = {}
        self.versions["Unio"] = ("unio interpreter, commit " + (version(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"]) or "?")) if unio else None
        self.versions["Python"] = f"Python {platform.python_version()}"
        for name, tool, args in (
            ("Ruby", "ruby", ["--version"]),
            ("JavaScript", "node", ["--version"]),
            ("Go", "go", ["version"]),
            ("Java", "java", ["-version"]),
            ("Rust", "rustc", ["--version"]),
        ):
            path = shutil.which(tool)
            if path is None or (name == "Java" and shutil.which("javac") is None):
                self.missing[name] = f"{tool} not found"
                continue
            self.versions[name] = version([path] + args)
            if name == "JavaScript":
                self.versions[name] = "Node.js " + (self.versions[name] or "")
        if unio is None:
            self.missing["Unio"] = "build compiler/ with cargo build --release first"
        if native:
            if unio is None:
                self.missing["Unio native"] = "build compiler/ with cargo build --release first"
            else:
                self.versions["Unio native"] = "unio build --release, commit " + (
                    version(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"]) or "?")

    def command(self, language, source, data):
        """The command that runs the program, after building it outside the measurement."""
        source = Path(source)
        args = [data] if data else []
        if language == "Unio":
            return [self.unio, "run", str(source), "--"] + args if args else [self.unio, "run", str(source)]
        if language == "Unio native":
            BUILD.mkdir(exist_ok=True)
            binary = BUILD / (source.stem + "-unio")
            subprocess.run([self.unio, "build", "--release", str(source), "-o", str(binary)],
                           check=True)
            return [str(binary)] + args
        if language == "Python":
            return [self.python, str(source)] + args
        if language == "Ruby":
            return ["ruby", str(source)] + args
        if language == "JavaScript":
            return ["node", str(source)] + args
        BUILD.mkdir(exist_ok=True)
        if language == "Go":
            binary = BUILD / (source.stem + "-go")
            subprocess.run(["go", "build", "-o", str(binary), str(source)], check=True,
                           env=dict(os.environ, GO111MODULE="off"))
            return [str(binary)] + args
        if language == "Rust":
            binary = BUILD / (source.stem + "-rs")
            subprocess.run(["rustc", "-C", "opt-level=3", "--edition", "2021", "-o", str(binary),
                            str(source)], check=True)
            return [str(binary)] + args
        if language == "Java":
            classes = BUILD / ("java-" + source.stem)
            classes.mkdir(exist_ok=True)
            subprocess.run(["javac", "-d", str(classes), str(source)], check=True)
            return ["java", "-cp", str(classes), source.stem] + args
        raise ValueError(language)


EXTENSIONS = {"Unio": ".unio", "Unio native": ".unio", "Python": ".py", "Ruby": ".rb", "JavaScript": ".js", "Go": ".go",
              "Java": ".java", "Rust": ".rs"}
ORDER = ["Unio", "Unio native", "Python", "Ruby", "JavaScript", "Go", "Java", "Rust"]


def programs(name, languages):
    """(language, variant, source): 'same' is the algorithm of the Unio program; 'built-in' is
    the language's own library for the task, where the benchmark has one (JSON)."""
    directory = BENCH / name
    found = []
    for language in ORDER:
        if language == "Unio native" and not languages.native:
            continue
        for path in sorted(directory.iterdir()):
            if path.suffix != EXTENSIONS[language]:
                continue
            variant = "built-in" if "builtin" in path.stem.lower() else "same"
            found.append((language, variant, path))
    return found


_LAUNCHER = []


def launcher():
    """bench/build/peak_rss, built once from bench/peak_rss.c; None without a C compiler or
    on Windows, and then memory is not measured."""
    if not _LAUNCHER:
        binary = BUILD / "peak_rss"
        compiler = shutil.which("cc")
        if os.name == "posix" and compiler:
            BUILD.mkdir(exist_ok=True)
            done = subprocess.run([compiler, "-O2", "-o", str(binary), str(BENCH / "peak_rss.c")])
            _LAUNCHER.append(str(binary) if done.returncode == 0 else None)
        else:
            _LAUNCHER.append(None)
    return _LAUNCHER[0]


def run_once(command, cwd):
    """Runs the command; returns (exit code, stdout, stderr, wall seconds, peak RSS in bytes).

    The launcher adds well under a millisecond to the wall time; without it the peak is None."""
    tool = launcher()
    start = time.perf_counter()
    done = subprocess.run(([tool] if tool else []) + command, capture_output=True, cwd=cwd)
    elapsed = time.perf_counter() - start
    stderr = done.stderr.decode("utf-8", "replace")
    peak = None
    if tool:
        head, _, last = stderr.rstrip("\n").rpartition("\n")
        if last.startswith("peak_rss "):
            peak = int(last.split()[1])
            stderr = head
    return done.returncode, done.stdout.decode("utf-8", "replace"), stderr, elapsed, peak


def measure(command, runs, cwd, memory=None):
    """Best wall time of the runs and the output; the smallest peak RSS goes into `memory`."""
    best = None
    output = None
    for _ in range(runs):
        code, stdout, stderr, elapsed, peak = run_once(command, cwd)
        if code != 0:
            return None, (stderr.strip().splitlines() or ["exit " + str(code)])[-1]
        output = stdout.strip()
        best = elapsed if best is None else min(best, elapsed)
        if memory is not None and peak is not None:
            memory.append(peak)
    return best, output


def reference(languages):
    """The Unio column that ratios and the floor refer to."""
    return "Unio native" if languages.native else "Unio"


def tool_rows(languages, runs):
    """(e): the project tools of tools/ in Unio against the Python checkers of spec/."""
    sources = sorted(
        [str(p.relative_to(ROOT)) for p in (ROOT / "docs" / "examples").glob("*.unio")]
        + [str(p.relative_to(ROOT)) for p in (ROOT / "docs" / "ru" / "examples").glob("*.unio")]
        + [str(p.relative_to(ROOT)) for p in (ROOT / "examples").glob("*.unio")]
        + [str(p.relative_to(ROOT)) for p in (ROOT / "stdlib").rglob("*.unio")]
        + [str(p.relative_to(ROOT)) for p in (ROOT / "tools").glob("*.unio")]
    )
    pairs = [
        ("check_grammar (all sources)", ["tools/check_grammar.unio", "--"] + sources,
         ["spec/check_grammar.py"] + sources),
        ("check_grammar --corpus", ["tools/check_grammar.unio", "--", "--corpus"],
         ["spec/check_grammar.py", "--corpus"]),
        ("check_api", ["tools/check_api.unio"], ["spec/check_api.py"]),
        ("check_docs", ["tools/check_docs.unio"], [".github/scripts/check_docs.py"]),
    ]
    rows = []
    for name, unio_args, python_args in pairs:
        row = {"benchmark": "tools", "case": name, "times": {}, "notes": {}}
        if languages.unio:
            t, out = measure([languages.unio, "run"] + unio_args, runs, ROOT)
            row["times"]["Unio"] = t
            if t is None:
                row["notes"]["Unio"] = out
        if languages.native and languages.unio:
            BUILD.mkdir(exist_ok=True)
            source = ROOT / unio_args[0]
            binary = BUILD / (source.stem + "-unio")
            subprocess.run([languages.unio, "build", "--release", str(source), "-o", str(binary)],
                           check=True)
            t, out = measure([str(binary)] + unio_args[2:], runs, ROOT)
            row["times"]["Unio native"] = t
            if t is None:
                row["notes"]["Unio native"] = out
        t, out = measure([languages.python] + python_args, runs, ROOT)
        row["times"]["Python"] = t
        if t is None:
            row["notes"]["Python"] = out
        rows.append(row)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--runs", type=int, default=3, help="best of N runs (default 3)")
    parser.add_argument("--only", default="wc,json,text,loop,tools", help="benchmarks to run")
    parser.add_argument("--json", default=str(BENCH / "results.json"), help="where to write JSON")
    parser.add_argument("--check-floor", action="store_true",
                        help="exit 1 if Unio is slower than 0.5x Python on wc, json, text, loop")
    parser.add_argument("--check-go", action="store_true",
                        help="exit 1 if Unio is above the time or memory limits against Go of RFC 0003")
    parser.add_argument("--unio", default=None, help="path to the unio binary")
    parser.add_argument("--native", action="store_true",
                        help="also build the Unio programs with unio build --release and compare those")
    options = parser.parse_args()
    selected = options.only.split(",")

    unio = find_unio(options.unio)
    languages = Languages(unio, options.native)
    data = ensure_data()

    print("Versions:")
    for language in ORDER:
        if language in languages.versions and languages.versions[language]:
            print(f"  {language:<11} {languages.versions[language]}")
        else:
            print(f"  {language:<11} skipped: {languages.missing.get(language, 'not found')}")
    print(f"  {'Machine':<11} {platform.platform()}, {os.cpu_count()} CPUs")
    print(f"Best of {options.runs} runs, wall time of the whole process.\n")

    rows = []
    failures = []
    for name in ("wc", "json", "text", "loop"):
        if name not in selected:
            continue
        outputs = {}
        for language, variant, source in programs(name, languages):
            row_case = "same algorithm" if variant == "same" else "built-in library"
            row = next((r for r in rows if r["benchmark"] == name and r["case"] == row_case), None)
            if row is None:
                row = {"benchmark": name, "case": row_case, "times": {}, "notes": {}}
                rows.append(row)
            if language in languages.missing:
                row["notes"][language] = "skipped: " + languages.missing[language]
                continue
            try:
                command = languages.command(language, source, data.get(name))
            except subprocess.CalledProcessError as e:
                row["notes"][language] = f"build failed: {e}"
                failures.append(f"{name}/{language}: build failed")
                continue
            peaks = []
            t, out = measure(command, options.runs, ROOT, peaks)
            if peaks:
                row.setdefault("memory", {})[language] = min(peaks)
            if t is None:
                row["notes"][language] = "failed: " + out
                failures.append(f"{name}/{language}: {out}")
                continue
            row["times"][language] = t
            outputs[f"{language} ({variant})"] = out
            print(f"  {name:<5} {row_case:<17} {language:<11} {t:8.3f} s  -> {out}", file=sys.stderr)
        if len(set(outputs.values())) > 1:
            failures.append(f"{name}: outputs differ: {outputs}")
        for row in rows:
            if row["benchmark"] == name:
                row["output"] = next(iter(outputs.values()), None)
        if name == "json":
            for row in rows:
                if row["benchmark"] == name and row["case"] == "built-in library":
                    for language in ("Java", "Rust"):
                        row["notes"].setdefault(language, "no JSON in the standard library")
    if "tools" in selected:
        rows.extend(tool_rows(languages, options.runs))

    print_table(rows, reference(languages))

    floor_failures = []
    for row in rows:
        if row["benchmark"] in FLOOR_BENCHMARKS and row["case"] == "same algorithm":
            ref = reference(languages)
            u, p = row["times"].get(ref), row["times"].get("Python")
            if u is None or p is None:
                floor_failures.append(f"{row['benchmark']}: no time for {ref} or Python")
            elif p / u < FLOOR:
                floor_failures.append(f"{row['benchmark']}: {ref} {u:.3f} s is {p / u:.2f}x Python {p:.3f} s")

    go_failures = []
    print(f"\n{reference(languages)} against Go (RFC 0003: time at most {GO_LIMIT}x, target 1x; peak memory"
          f" at most {GO_LIMIT}x + 8 MB; gated: {', '.join(GO_GATED)}):")
    for row in rows:
        if row["benchmark"] in FLOOR_BENCHMARKS and row["case"] == "same algorithm":
            ref = reference(languages)
            u, g = row["times"].get(ref), row["times"].get("Go")
            if u is None or g is None:
                continue
            gated = row["benchmark"] in GO_GATED
            memory = row.get("memory", {})
            um, gm = memory.get(ref), memory.get("Go")
            memory_text = ""
            if um is not None and gm is not None:
                limit = GO_LIMIT * gm + GO_MEMORY_ALLOWANCE
                memory_text = f"   memory {um / gm:5.2f}x Go ({um / 1e6:.1f} of at most {limit / 1e6:.1f} MB)"
                if gated and um > limit:
                    go_failures.append(f"{row['benchmark']}: {ref} peak {um / 1e6:.1f} MB is above"
                                       f" {limit / 1e6:.1f} MB (2x Go {gm / 1e6:.1f} MB + 8 MB)")
            print(f"  {row['benchmark']:<5} time {u / g:5.2f}x Go{memory_text}{'  (gated)' if gated else ''}")
            if gated and u / g > GO_LIMIT:
                go_failures.append(f"{row['benchmark']}: {ref} {u:.3f} s is {u / g:.2f}x Go {g:.3f} s")
    print_memory(rows)

    report = {
        "versions": languages.versions,
        "skipped": languages.missing,
        "machine": platform.platform(),
        "runs": options.runs,
        "floor": FLOOR,
        "rows": rows,
        "failures": failures,
        "floor_failures": floor_failures,
        "go_limit": GO_LIMIT,
        "go_gated": list(GO_GATED),
        "go_failures": go_failures,
    }
    Path(options.json).write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"\nJSON written to {options.json}")
    for failure in failures:
        print("error: " + failure, file=sys.stderr)
    for failure in floor_failures:
        print(f"{'error' if options.check_floor else 'note'}: below the {FLOOR}x floor of 7.1: {failure}",
              file=sys.stderr)
    for failure in go_failures:
        print(f"{'error' if options.check_go else 'note'}: above {GO_LIMIT}x Go (RFC 0003): {failure}",
              file=sys.stderr)
    if failures or (options.check_floor and floor_failures) or (options.check_go and go_failures):
        return 1
    return 0


def print_memory(rows):
    """Peak resident memory of each process in MB, the smallest of the runs."""
    measured = [r for r in rows if r.get("memory")]
    if not measured:
        return
    present = [l for l in ORDER if any(l in r["memory"] for r in measured)]
    header = f"{'peak memory, MB':<34}" + "".join(f"{l:>13}" for l in present)
    print("\n" + header)
    print("-" * len(header))
    for row in measured:
        cells = []
        for language in present:
            m = row["memory"].get(language)
            cells.append(f"{'—':>13}" if m is None else f"{m / 1e6:>13.1f}")
        print(f"{row['benchmark'] + ': ' + row['case']:<34}" + "".join(cells))


def print_table(rows, ref="Unio"):
    """Times in seconds; 'x' is the speed of Unio (the `ref` column) relative to the language:
    above 1 means Unio is faster, so Python 0.50x is the floor of requirement 7.1."""
    present = [l for l in ORDER if any(l in r["times"] for r in rows)]
    header = f"{'benchmark':<34}" + "".join(f"{l:>17}" for l in present)
    print(header)
    print("-" * len(header))
    for row in rows:
        cells = []
        u = row["times"].get(ref)
        for language in present:
            t = row["times"].get(language)
            if t is None:
                note = row["notes"].get(language, "")
                cells.append(f"{'—' if not note else 'n/a':>17}")
            elif language == ref or u is None:
                cells.append(f"{t:>15.3f} s")
            else:
                cells.append(f"{t:>8.3f} s {t / u:>5.2f}x")
        print(f"{row['benchmark'] + ': ' + row['case']:<34}" + "".join(cells))
    print(f"\nNx after a time is that language's time divided by {ref}'s: above 1, {ref} is faster.")
    notes = sorted({(l, n) for r in rows for l, n in r["notes"].items()})
    for language, note in notes:
        print(f"  {language}: {note}")


if __name__ == "__main__":
    sys.exit(main())
