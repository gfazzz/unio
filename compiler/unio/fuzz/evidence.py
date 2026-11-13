#!/usr/bin/env python3
"""The evidence of the fuzzing of R-1, from the runs of .github/workflows/fuzz.yml.

stage: after one stage of fuzzing, writes OUT/stage.json and OUT/stage.md from the summary of
the supervisor (WORK/summary.json, compiler/unio/fuzz/supervisor.rs), the commit, the commands,
the environment (runner image, toolchain), whether the corpus of the stage before was restored,
and the corpus of every target; the Markdown also goes to the job summary of GitHub.

aggregate: after all stages, reads the stage.json of every stage (DIR/*/stage.json) and writes
one report, OUT/fuzz-DATE.json and OUT/fuzz-DATE.md, in the form of bench/results/. The report
is evidence for R-1 only when every check holds: all stages ran on the same commit, each for
the full time with the corpus of the stage before, no finding and no failure of the
supervisor, every target of targets.rs instrumented and run in every stage, and at least HOURS of wall time
per target over the stages. The exit code is 1 otherwise.

Usage:
    evidence.py stage --stage N --work DIR --minutes M --restored first|yes|no --out DIR
                      [--command TEXT]...
    evidence.py aggregate --stages DIR --sha SHA [--count 5] [--minutes 300] [--hours 24]
                          --out DIR

Only the standard library; Python 3.9 or newer.
"""

import argparse
import datetime
import json
import os
import re
import subprocess
import sys

ENVIRONMENT = [
    "GITHUB_REPOSITORY",
    "GITHUB_REF",
    "GITHUB_RUN_ID",
    "GITHUB_RUN_ATTEMPT",
    "GITHUB_WORKFLOW_REF",
    "GITHUB_EVENT_NAME",
    "RUNNER_NAME",
    "RUNNER_OS",
    "RUNNER_ARCH",
    "ImageOS",
    "ImageVersion",
]


def all_targets():
    """The targets of the fuzzer, from TARGETS in targets.rs."""
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "targets.rs")
    with open(path, encoding="utf-8") as handle:
        found = re.search(r"pub const TARGETS: &\[&str\] = &\[([^\]]*)\]", handle.read())
    return re.findall(r'"([^"]+)"', found.group(1)) if found else []


def output_of(command, cwd):
    try:
        done = subprocess.run(
            command, cwd=cwd, capture_output=True, text=True, timeout=60, check=False
        )
        return done.stdout.strip()
    except (OSError, subprocess.SubprocessError) as error:
        return "unavailable: {}".format(error)


def processors():
    try:
        return len(os.sched_getaffinity(0))
    except AttributeError:
        return os.cpu_count()


def count_files(path):
    try:
        return sum(1 for entry in os.scandir(path) if entry.is_file())
    except OSError:
        return 0


def write(path, text):
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def job_summary(text):
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(text)


def hours(seconds):
    return "{:.2f} h".format(seconds / 3600.0)


def stage(args):
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
    compiler = os.path.join(root, "compiler")
    summary_path = os.path.join(args.work, "summary.json")
    try:
        with open(summary_path, encoding="utf-8") as handle:
            summary = json.load(handle)
        error = None
    except (OSError, ValueError) as problem:
        summary = None
        error = "no summary of the supervisor: {}".format(problem)
    corpus = {}
    if summary:
        for target in summary["targets"]:
            corpus[target] = count_files(os.path.join(args.work, "corpus", target))
    sha = os.environ.get("GITHUB_SHA") or output_of(["git", "rev-parse", "HEAD"], root)
    record = {
        "format": 1,
        "stage": args.stage,
        "sha": sha,
        "minutes": args.minutes,
        "restored": args.restored,
        "commands": args.command or [],
        "environment": {name: os.environ.get(name) for name in ENVIRONMENT},
        "processors": processors(),
        "toolchain": {
            "rustc": output_of(["rustc", "-vV"], compiler),
            "cargo": output_of(["cargo", "-V"], compiler),
        },
        "corpus_files": corpus,
        "fuzz": summary,
        "error": error,
    }
    record["ok"] = bool(
        summary
        and summary.get("ok")
        and (args.restored == "yes" or (args.restored == "first" and args.stage == 1))
    )
    os.makedirs(args.out, exist_ok=True)
    write(os.path.join(args.out, "stage.json"), json.dumps(record, indent=2, sort_keys=True) + "\n")
    lines = [
        "## Fuzzing, stage {} (R-1)".format(args.stage),
        "",
        "Commit `{}`, {} minutes planned, corpus of the stage before: {}. Result: **{}**.".format(
            sha, args.minutes, args.restored, "no finding" if record["ok"] else "failed"
        ),
        "",
        "Runner image {} {}, {} processors; {}.".format(
            os.environ.get("ImageOS") or "unknown",
            os.environ.get("ImageVersion") or "",
            record["processors"],
            record["toolchain"]["rustc"].splitlines()[0] if record["toolchain"]["rustc"] else "rustc unknown",
        ),
        "",
    ]
    if summary:
        lines += [
            "| Target | Wall | CPU | Executions | Corpus | Edges | Restarts | Findings | Result |",
            "|---|---|---|---|---|---|---|---|---|",
        ]
        for name, target in sorted(summary["targets"].items()):
            cpu = target.get("cpu_seconds")
            lines.append(
                "| {} | {} | {} | {} | {} | {} of {} | {} | {} | {} |".format(
                    name,
                    hours(target["wall_seconds"]),
                    "unknown" if cpu is None else hours(cpu),
                    target["execs"],
                    corpus.get(name, 0),
                    target["edges"],
                    target["counters"],
                    target["starts"] - 1,
                    len(target["findings"]),
                    "ok" if target["ok"] else "failed",
                )
            )
        lines.append("")
        for failure in summary.get("failures", []):
            lines.append("- Failure: {}".format(failure))
        for finding in summary.get("findings", []):
            lines.append("- Finding: `crashes/{}`".format(finding))
    else:
        lines.append("- {}".format(error))
    text = "\n".join(lines).rstrip() + "\n"
    write(os.path.join(args.out, "stage.md"), text)
    job_summary(text)
    return 0


def aggregate(args):
    stages = {}
    problems = []
    try:
        entries = sorted(os.listdir(args.stages))
    except OSError:
        entries = []
    for entry in entries:
        path = os.path.join(args.stages, entry, "stage.json")
        if not os.path.isfile(path):
            continue
        with open(path, encoding="utf-8") as handle:
            record = json.load(handle)
        if record["stage"] in stages:
            problems.append("stage {} reported twice".format(record["stage"]))
        stages[record["stage"]] = record
    targets = {}
    toolchains = set()
    for number in range(1, args.count + 1):
        record = stages.get(number)
        if record is None:
            problems.append("stage {}: no report (it did not run or did not finish)".format(number))
            continue
        if record["sha"] != args.sha:
            problems.append("stage {}: commit {}, not {}".format(number, record["sha"], args.sha))
        if int(record["minutes"]) < args.minutes:
            problems.append(
                "stage {}: {} minutes planned, fewer than {}".format(number, record["minutes"], args.minutes)
            )
        wanted = "first" if number == 1 else "yes"
        if record["restored"] != wanted:
            problems.append(
                "stage {}: corpus of the stage before {}, expected {}".format(number, record["restored"], wanted)
            )
        toolchains.add(record["toolchain"]["rustc"])
        fuzz = record.get("fuzz")
        if not fuzz:
            problems.append("stage {}: {}".format(number, record.get("error") or "no summary"))
            continue
        if fuzz["planned_seconds"] < args.minutes * 60 - 1:
            problems.append(
                "stage {}: {:.0f} seconds planned, fewer than {}".format(
                    number, fuzz["planned_seconds"], args.minutes * 60
                )
            )
        for finding in fuzz["findings"]:
            problems.append("stage {}: finding crashes/{}".format(number, finding))
        for failure in fuzz["failures"]:
            problems.append("stage {}: {}".format(number, failure))
        for name, target in fuzz["targets"].items():
            total = targets.setdefault(
                name, {"wall_seconds": 0.0, "cpu_seconds": 0.0, "execs": 0, "stages": [], "edges": 0}
            )
            total["wall_seconds"] += target["wall_seconds"]
            if total["cpu_seconds"] is not None and target["cpu_seconds"] is not None:
                total["cpu_seconds"] += target["cpu_seconds"]
            else:
                total["cpu_seconds"] = None
            total["execs"] += target["execs"]
            total["edges"] = max(total["edges"], target["edges"])
            total["stages"].append(number)
            if target["counters"] == 0:
                problems.append("stage {}: {} ran without coverage instrumentation".format(number, name))
            if target["execs"] == 0:
                problems.append("stage {}: {} ran no input".format(number, name))
    if len(toolchains) > 1:
        problems.append("the stages ran on different toolchains")
    required = args.hours * 3600.0
    for name, total in sorted(targets.items()):
        if len(total["stages"]) != args.count:
            problems.append("{} ran in stages {} only".format(name, total["stages"]))
        if total["wall_seconds"] < required:
            problems.append(
                "{}: {} of wall time, fewer than {} h".format(name, hours(total["wall_seconds"]), args.hours)
            )
    for name in all_targets():
        if name not in targets:
            problems.append("the target {} did not run".format(name))
    met = not problems
    first = stages.get(1, {})
    started = (first.get("fuzz") or {}).get("started") or datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    date = started[:10]
    last = stages.get(args.count, {})
    report = {
        "requirement": "R-1",
        "criterion": "{} stages of at least {} minutes on one commit, no finding, at least {} h of wall time per target".format(
            args.count, args.minutes, args.hours
        ),
        "sha": args.sha,
        "started": started,
        "ended": (last.get("fuzz") or {}).get("ended"),
        "stages": [
            {
                "stage": number,
                "ok": record["ok"],
                "sha": record["sha"],
                "minutes": record["minutes"],
                "restored": record["restored"],
                "started": (record.get("fuzz") or {}).get("started"),
                "ended": (record.get("fuzz") or {}).get("ended"),
                "elapsed_seconds": (record.get("fuzz") or {}).get("elapsed_seconds"),
                "commands": record["commands"],
                "environment": record["environment"],
                "processors": record["processors"],
                "toolchain": record["toolchain"],
                "targets": (record.get("fuzz") or {}).get("targets"),
            }
            for number, record in sorted(stages.items())
        ],
        "targets": targets,
        "problems": problems,
        "met": met,
    }
    os.makedirs(args.out, exist_ok=True)
    base = os.path.join(args.out, "fuzz-{}".format(date))
    write(base + ".json", json.dumps(report, indent=2, sort_keys=True) + "\n")
    lines = [
        "# R-1 fuzzing ({})".format(date),
        "",
        "Commit: `{}`. Criterion: {}. Workflow: `.github/workflows/fuzz.yml`; harness: `compiler/unio/fuzz/`.".format(
            args.sha, report["criterion"]
        ),
        "",
        "## Stages",
        "",
        "| Stage | Started | Ended | Minutes | Corpus before | Runner image | Result |",
        "|---|---|---|---|---|---|---|",
    ]
    for item in report["stages"]:
        environment = item["environment"]
        lines.append(
            "| {} | {} | {} | {} | {} | {} {} | {} |".format(
                item["stage"],
                item["started"] or "",
                item["ended"] or "",
                item["minutes"],
                item["restored"],
                environment.get("ImageOS") or "",
                environment.get("ImageVersion") or "",
                "ok" if item["ok"] else "failed",
            )
        )
    lines += [
        "",
        "## Targets over all stages",
        "",
        "| Target | Wall | CPU | Executions | Edges | Stages |",
        "|---|---|---|---|---|---|",
    ]
    for name, total in sorted(targets.items()):
        lines.append(
            "| {} | {} | {} | {} | {} | {} |".format(
                name,
                hours(total["wall_seconds"]),
                "unknown" if total["cpu_seconds"] is None else hours(total["cpu_seconds"]),
                total["execs"],
                total["edges"],
                len(total["stages"]),
            )
        )
    lines += ["", "Criterion: {}.".format("met" if met else "not met")]
    if problems:
        lines += [""] + ["- {}".format(problem) for problem in problems]
    toolchain = next(iter(toolchains), "")
    if toolchain:
        lines += ["", "## Toolchain", "", "```text", toolchain, "```"]
    text = "\n".join(lines) + "\n"
    write(base + ".md", text)
    job_summary(text)
    print("fuzz evidence: {} ({}.json, {}.md)".format("met" if met else "not met", base, base))
    for problem in problems:
        print("  " + problem)
    return 0 if met else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="action", required=True)
    one = commands.add_parser("stage")
    one.add_argument("--stage", type=int, required=True)
    one.add_argument("--work", required=True)
    one.add_argument("--minutes", type=int, required=True)
    one.add_argument("--restored", choices=["first", "yes", "no"], required=True)
    one.add_argument("--command", action="append")
    one.add_argument("--out", required=True)
    every = commands.add_parser("aggregate")
    every.add_argument("--stages", required=True)
    every.add_argument("--sha", required=True)
    every.add_argument("--count", type=int, default=5)
    every.add_argument("--minutes", type=int, default=300)
    every.add_argument("--hours", type=float, default=24.0)
    every.add_argument("--out", required=True)
    args = parser.parse_args()
    return stage(args) if args.action == "stage" else aggregate(args)


if __name__ == "__main__":
    sys.exit(main())
