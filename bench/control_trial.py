#!/usr/bin/env python3
"""Check B of the R-10 control set: the harness of the agent trial (plan section 12, condition 2).

The harness makes no model calls. The orchestrator runs one fresh agent per program and attempt
between `prepare` (or the previous `apply`) and the next `apply`; each agent receives one prompt
file and answers with the full corrected source, using no tools.

    control_trial.py prepare HIDDEN TRIAL --model ID
        Moves the reference fixes (*.fixed.*) and the hidden checks (*.check.unio) of
        spec/tests/control to HIDDEN, a directory outside the repository that the agents are not
        told about, records the model, the compiler commit and the checksums of the
        infrastructure in TRIAL/state.json, and writes TRIAL/prompts/attempt-1/NAME.txt.
    control_trial.py apply TRIAL ANSWERS ATTEMPT
        Applies ANSWERS/NAME.txt to a copy of each program still open at that attempt, runs
        `unio check` and the hidden checks from HIDDEN, records pass or fail, and writes the
        prompts of the next attempt (with the new diagnostics) for the programs that failed.
        A name listed in ANSWERS/VOID (one per line) failed its audit, for example because the
        agent called a tool: that attempt counts as failed.
    control_trial.py report TRIAL [--out DIR]
        Writes the results after attempts 1 and 3, per category and in total, as JSON and
        Markdown into DIR (default bench/results/).
    control_trial.py restore TRIAL
        Moves the reference fixes and hidden checks back into spec/tests/control.

Only the Python standard library is used. Run from the repository root.
"""

import datetime
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
CONTROL = os.path.join(ROOT, "spec", "tests", "control")
UNIO = os.environ.get("UNIO", os.path.join(ROOT, "compiler", "target", "release", "unio"))
CATEGORIES = ["types", "errors", "resources", "style"]
ATTEMPTS = 3
HEADERS = ("# expect-error:", "# category:", "# profile:", "# compiler-bug:")

# The fixed instruction of every prompt; plan section 12, condition 2 quotes it.
INSTRUCTION = """\
You are given a program in the Unio language and the diagnostics that `unio check
--diagnostics=json` reports for it. Fix the program so that `unio check` reports no error and
the program does what its code and comments say it should do. Change only what the error
needs; do not delete the code that is wrong or work around the check. Do not use any tools:
answer only with the full corrected source. For a program of one file, answer with the whole
file. For a package, answer with every file you change, each starting with a line
`=== FILE: PATH ===` where PATH is the path shown in this prompt. Write no other text."""


def fail(message):
    print(f"control_trial: {message}", file=sys.stderr)
    sys.exit(2)


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read().replace("\r\n", "\n")


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        h.update(f.read())
    return h.hexdigest()


def tree_digest(paths):
    h = hashlib.sha256()
    for path in sorted(paths):
        h.update(path.encode())
        h.update(digest(path).encode())
    return h.hexdigest()


def is_helper(name):
    return ".fixed." in name or name.endswith(".check.unio")


def headers(path):
    """(line, code, category, core) of a faulty file, or None."""
    lines = read(path).split("\n")
    if not lines[0].startswith("# expect-error:") or len(lines) < 2:
        return None
    words = lines[0][len("# expect-error:"):].split()
    category = lines[1][len("# category:"):].strip() if lines[1].startswith("# category:") else None
    core = any(l.startswith("# profile:") and l.split(":", 1)[1].strip() == "core"
               for l in lines[2:4])
    return int(words[0]), words[1], category, core


def programs():
    """Every program: name, kind, directory or file, the faulty file relative to the program."""
    found = []
    for name in sorted(os.listdir(CONTROL)):
        path = os.path.join(CONTROL, name)
        if os.path.isdir(path):
            faulty = None
            for dirpath, _, files in os.walk(path):
                for f in files:
                    full = os.path.join(dirpath, f)
                    if (f.endswith(".unio") or f == "unio.toml") and not is_helper(f) \
                            and headers(full):
                        faulty = os.path.relpath(full, path)
            if faulty is None:
                fail(f"{name}: no faulty file with headers")
            found.append(dict(name=name, package=True, faulty=faulty))
        elif name.endswith(".unio") and not is_helper(name):
            found.append(dict(name=name[:-len(".unio")], package=False, faulty=name))
    for p in found:
        line, code, category, core = headers(program_file(p, p["faulty"]))
        p.update(line=line, code=code, category=category, core=core)
        # A faulty manifest (E0088) is checked through main.unio beside it, which the hidden
        # check joins.
        p["module"] = (os.path.join(os.path.dirname(p["faulty"]), "main.unio")
                       if os.path.basename(p["faulty"]) == "unio.toml" else p["faulty"])
    return found


def program_dir(p):
    return os.path.join(CONTROL, p["name"]) if p["package"] else CONTROL


def program_file(p, relative):
    return os.path.join(program_dir(p), relative)


def program_sources(p):
    """The files of a program shown to the agent, relative to the program."""
    if not p["package"]:
        return [p["faulty"]]
    base = program_dir(p)
    found = []
    for dirpath, _, files in os.walk(base):
        for f in files:
            if not is_helper(f):
                found.append(os.path.relpath(os.path.join(dirpath, f), base))
    return sorted(found)


def run(args, cwd):
    return subprocess.run(args, cwd=cwd, capture_output=True, text=True)


def check_json(directory, files, core):
    args = [UNIO, "check", "--diagnostics=json"] + (["--profile=core"] if core else []) + files
    result = run(args, directory)
    try:
        diagnostics = json.loads(result.stdout or "[]")
    except json.JSONDecodeError:
        diagnostics = None
    return result, diagnostics


def hidden_files():
    found = []
    for dirpath, _, files in os.walk(CONTROL):
        for f in files:
            if is_helper(f):
                found.append(os.path.relpath(os.path.join(dirpath, f), CONTROL))
    return sorted(found)


def load_state(trial):
    path = os.path.join(trial, "state.json")
    if not os.path.exists(path):
        fail(f"{trial}: no state.json; run prepare first")
    return json.loads(read(path))


def save_state(trial, state):
    write(os.path.join(trial, "state.json"), json.dumps(state, indent=2, sort_keys=True) + "\n")


def infrastructure(hidden):
    """Checksums of what must not change during the trial."""
    sources = [os.path.join(dirpath, f) for dirpath, _, files in os.walk(CONTROL) for f in files]
    stored = [os.path.join(dirpath, f) for dirpath, _, files in os.walk(hidden) for f in files]
    return {
        "unio": digest(UNIO),
        "control": tree_digest(sources),
        "hidden": tree_digest(stored),
        "harness": digest(os.path.abspath(__file__)),
    }


def prompt(p, sources, history):
    """The prompt of one program: the instruction, the source, and the earlier attempts."""
    parts = [INSTRUCTION, ""]
    profile = " --profile=core" if p["core"] else ""
    parts.append(f"Program: {p['name']} (checked with `unio check --diagnostics=json{profile}`)")
    parts.append("")
    for relative, text in sources:
        parts.append(f"=== FILE: {relative} ===")
        parts.append(text.rstrip("\n"))
    parts.append("")
    for entry in history:
        parts.append(f"--- Attempt {entry['attempt']}: your answer ---")
        parts.append(entry["answer"].rstrip("\n"))
        parts.append(f"--- Attempt {entry['attempt']}: result ---")
        parts.append(entry["feedback"])
        parts.append("")
    if not history:
        parts.append("Diagnostics:")
        parts.append(p["diagnostics"])
    return "\n".join(parts) + "\n"


def cmd_prepare(args):
    if len(args) != 4 or args[2] != "--model":
        fail("usage: prepare HIDDEN TRIAL --model ID")
    hidden, trial, model = os.path.abspath(args[0]), os.path.abspath(args[1]), args[3]
    if hidden.startswith(ROOT + os.sep):
        fail("HIDDEN must lie outside the repository")
    if os.path.exists(os.path.join(trial, "state.json")):
        fail(f"{trial} already holds a trial")
    if not os.path.isfile(UNIO):
        fail(f"no compiler at {UNIO}; build it with cargo build --release")
    found = programs()
    if len(found) < 200:
        fail(f"the control set has {len(found)} programs, fewer than 200")
    moved = hidden_files()
    if not moved:
        fail("no reference fixes or hidden checks in spec/tests/control")
    for relative in moved:
        target = os.path.join(hidden, relative)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        shutil.move(os.path.join(CONTROL, relative), target)
    commit = run(["git", "rev-parse", "HEAD"], ROOT).stdout.strip()
    state = {
        "model": model,
        "commit": commit,
        "started": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "hidden": hidden,
        "moved": moved,
        "infrastructure": infrastructure(hidden),
        "instruction": INSTRUCTION,
        "programs": {},
    }
    for p in found:
        directory = os.path.dirname(program_file(p, p["module"]))
        result, diagnostics = check_json(directory, [os.path.basename(p["module"])], p["core"])
        text = json.dumps(diagnostics, indent=2, ensure_ascii=False) if diagnostics is not None \
            else result.stdout + result.stderr
        p["diagnostics"] = text
        sources = [(s, read(program_file(p, s))) for s in program_sources(p)]
        write(os.path.join(trial, "prompts", "attempt-1", p["name"] + ".txt"), prompt(p, sources, []))
        state["programs"][p["name"]] = {
            "category": p["category"], "code": p["code"], "package": p["package"],
            "faulty": p["faulty"], "core": p["core"], "diagnostics": text,
            "attempts": [], "passed_at": None,
        }
    save_state(trial, state)
    print(f"prepared {len(found)} prompts in {os.path.join(trial, 'prompts', 'attempt-1')}; "
          f"{len(moved)} files moved to {hidden}")


def parse_answer(text, p):
    """The files of an answer: {relative path: text}. Code fences around the answer are removed."""
    text = text.replace("\r\n", "\n")
    fence = re.match(r"^\s*```[A-Za-z]*\n(.*?)\n```\s*$", text, re.S)
    if fence:
        text = fence.group(1) + "\n"
    if "=== FILE:" not in text:
        if p["package"]:
            return None, "a package answer names its files with '=== FILE: PATH ==='"
        return {p["faulty"]: text}, None
    files = {}
    current = None
    for line in text.split("\n"):
        m = re.match(r"^=== FILE: (.+?) ===$", line)
        if m:
            current = m.group(1).strip()
            files[current] = []
        elif current is not None:
            files[current].append(line)
    allowed = set(program_sources(p))
    result = {}
    for path, lines in files.items():
        if path not in allowed:
            return None, f"the answer changes '{path}', which is not a file of the program"
        result[path] = "\n".join(lines).rstrip("\n") + "\n"
    return result, None


def strip_headers(text):
    lines = text.split("\n")
    skip = 0
    while skip < len(lines) and lines[skip].startswith(HEADERS):
        skip += 1
    return lines[skip:]


def changed_lines(program, answer):
    """The lines, numbered as in the program, between the common start and end of both."""
    all_lines = program.split("\n")
    skip = len(all_lines) - len(strip_headers(program))
    a = all_lines[skip:]
    b = strip_headers(answer)
    prefix = 0
    while prefix < min(len(a), len(b)) and a[prefix] == b[prefix]:
        prefix += 1
    suffix = 0
    while suffix < min(len(a), len(b)) - prefix and a[-1 - suffix] == b[-1 - suffix]:
        suffix += 1
    first = prefix + 1 + skip
    return first, max(len(a) - suffix + skip, first)


def evaluate(p, state, files):
    """(passed, feedback for the agent, reason for the record) of one answer."""
    hidden = state["hidden"]
    work = tempfile.mkdtemp(prefix="unio-trial-")
    try:
        base = os.path.join(work, p["name"])
        if p["package"]:
            shutil.copytree(program_dir(p), base)
        else:
            os.makedirs(base)
            shutil.copy(program_file(p, p["faulty"]), os.path.join(base, p["faulty"]))
        for relative, text in files.items():
            write(os.path.join(base, relative), text)
        sources = sorted(os.path.relpath(os.path.join(d, f), base)
                         for d, _, fs in os.walk(base) for f in fs if f.endswith(".unio"))
        result, diagnostics = check_json(base, sources, p["core"])
        if diagnostics is None:
            return False, result.stdout + result.stderr, "unio check gave no JSON"
        errors = [x for x in diagnostics if x.get("severity") == "error"]
        if errors:
            text = json.dumps(diagnostics, indent=2, ensure_ascii=False)
            return False, "unio check --diagnostics=json reports:\n" + text, \
                f"{len(errors)} error(s), first {errors[0]['code']}"
        stem = os.path.splitext(p["module"])[0]
        check = read(os.path.join(hidden, (p["name"] + "/" if p["package"] else "") +
                                  (stem + ".check.unio" if p["package"] else p["name"] + ".check.unio")))
        module = os.path.join(base, p["module"])
        if check.startswith("# Form check"):
            m = re.search(r"^# changed lines: (\d+)-(\d+)$", check, re.M)
            first, last = changed_lines(read(program_file(p, p["faulty"])), read(module))
            if first < int(m.group(1)) or last > int(m.group(2)):
                return False, ("unio check is clean, but the answer changes more than the faulty "
                               "construct"), f"form check: lines {first}-{last} changed"
            fmt = run([UNIO, "fmt", "--check", p["module"]], base)
            if fmt.returncode != 0:
                return False, ("unio check is clean, but the program is not in the one style "
                               "(unio fmt --check fails)"), "form check: not formatted"
            return True, "", "form check holds"
        with open(module, "a", encoding="utf-8") as f:
            f.write("\n" + check)
        tested = run([UNIO, "test", "--json", os.path.basename(module)], os.path.dirname(module))
        try:
            results = json.loads(tested.stdout)
        except json.JSONDecodeError:
            return False, "unio check is clean, but the program does not pass its hidden tests", \
                "hidden tests gave no JSON"
        failed = [r for r in results if r.get("status") != "passed"]
        if failed or not results or tested.returncode != 0:
            return False, "unio check is clean, but the program does not pass its hidden tests", \
                f"{len(failed)} hidden test(s) failed"
        return True, "", f"{len(results)} hidden test(s) passed"
    finally:
        shutil.rmtree(work, ignore_errors=True)


def cmd_apply(args):
    if len(args) != 3:
        fail("usage: apply TRIAL ANSWERS ATTEMPT")
    trial, answers, attempt = os.path.abspath(args[0]), os.path.abspath(args[1]), int(args[2])
    state = load_state(trial)
    if not 1 <= attempt <= ATTEMPTS:
        fail(f"the attempt is 1 to {ATTEMPTS}")
    if infrastructure(state["hidden"]) != state["infrastructure"]:
        fail("the compiler, the control set, the hidden checks or the harness changed during the trial")
    void_path = os.path.join(answers, "VOID")
    void = set(read(void_path).split()) if os.path.exists(void_path) else set()
    found = {p["name"]: p for p in programs()}
    applied = 0
    for name, record in sorted(state["programs"].items()):
        if record["passed_at"] is not None or len(record["attempts"]) >= attempt:
            continue
        if len(record["attempts"]) != attempt - 1:
            fail(f"{name}: attempt {attempt} before attempt {len(record['attempts']) + 1}")
        p = found[name]
        p["diagnostics"] = record["diagnostics"]
        answer_path = os.path.join(answers, name + ".txt")
        answer = read(answer_path) if os.path.exists(answer_path) else ""
        if name in void:
            passed, feedback, reason = False, "the attempt was voided by the audit", "voided by the audit"
        elif not answer.strip():
            passed, feedback, reason = False, "no answer was given", "no answer"
        else:
            files, problem = parse_answer(answer, p)
            if problem:
                passed, feedback, reason = False, problem, problem
            else:
                passed, feedback, reason = evaluate(p, state, files)
        record["attempts"].append({"attempt": attempt, "passed": passed, "reason": reason,
                                   "answer": answer, "feedback": feedback})
        if passed:
            record["passed_at"] = attempt
        elif attempt < ATTEMPTS:
            sources = [(s, read(program_file(p, s))) for s in program_sources(p)]
            write(os.path.join(trial, "prompts", f"attempt-{attempt + 1}", name + ".txt"),
                  prompt(p, sources, record["attempts"]))
        applied += 1
    save_state(trial, state)
    passed = sum(1 for r in state["programs"].values() if r["passed_at"] is not None)
    print(f"attempt {attempt}: {applied} answer(s) applied; {passed} of "
          f"{len(state['programs'])} programs fixed so far")


def summary(state, attempt):
    table = {c: [0, 0] for c in CATEGORIES}
    for record in state["programs"].values():
        row = table.setdefault(record["category"], [0, 0])
        row[1] += 1
        if record["passed_at"] is not None and record["passed_at"] <= attempt:
            row[0] += 1
    total = [sum(r[0] for r in table.values()), sum(r[1] for r in table.values())]
    return table, total


def cmd_report(args):
    if len(args) not in (1, 3) or (len(args) == 3 and args[1] != "--out"):
        fail("usage: report TRIAL [--out DIR]")
    trial = os.path.abspath(args[0])
    out = os.path.abspath(args[2]) if len(args) == 3 else os.path.join(ROOT, "bench", "results")
    state = load_state(trial)
    report = {"model": state["model"], "commit": state["commit"], "started": state["started"],
              "programs": len(state["programs"]), "criterion": "at least 160 of 200 within 3 attempts",
              "after": {}}
    lines = [f"# R-10 control trial ({state['started'][:10]})", "",
             f"Model: `{state['model']}`. Compiler commit: `{state['commit']}`. "
             f"Protocol: [plan.md](../../docs/plan.md) section 12, closure condition 2.", ""]
    for attempt in (1, ATTEMPTS):
        table, total = summary(state, attempt)
        report["after"][str(attempt)] = {"categories": {c: {"fixed": f, "programs": n}
                                                         for c, (f, n) in table.items()},
                                         "fixed": total[0], "programs": total[1]}
        lines += [f"## After attempt {attempt}", "", "| Category | Fixed | Programs | Share |",
                  "|---|---|---|---|"]
        for c, (f, n) in table.items():
            lines.append(f"| {c} | {f} | {n} | {100 * f / n if n else 0:.1f}% |")
        lines += [f"| total | {total[0]} | {total[1]} | "
                  f"{100 * total[0] / total[1] if total[1] else 0:.1f}% |", ""]
    final = report["after"][str(ATTEMPTS)]["fixed"]
    report["met"] = final >= 160
    lines.append(f"Criterion (at least 160 of 200 within {ATTEMPTS} attempts): "
                 f"{'met' if report['met'] else 'not met'}.")
    stamp = state["started"][:10]
    write(os.path.join(out, f"control_trial-{stamp}.json"), json.dumps(report, indent=2) + "\n")
    write(os.path.join(out, f"control_trial-{stamp}.md"), "\n".join(lines) + "\n")
    print(f"report written to {out} (control_trial-{stamp}.json and .md)")


def cmd_restore(args):
    if len(args) != 1:
        fail("usage: restore TRIAL")
    trial = os.path.abspath(args[0])
    state = load_state(trial)
    restored = 0
    for relative in state["moved"]:
        source = os.path.join(state["hidden"], relative)
        if os.path.exists(source):
            target = os.path.join(CONTROL, relative)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            shutil.move(source, target)
            restored += 1
    print(f"{restored} of {len(state['moved'])} files restored to spec/tests/control")


def main(argv):
    commands = {"prepare": cmd_prepare, "apply": cmd_apply, "report": cmd_report,
                "restore": cmd_restore}
    if not argv or argv[0] not in commands:
        print(__doc__)
        return 2
    commands[argv[0]](argv[1:])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
