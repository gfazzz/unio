# R-10 control trial (2026-11-12)

Model: `claude-opus-5-5`. Compiler commit: `5563349d0cce705d0357378cd9226914998fefc1`. Protocol: [plan.md](../../docs/plan.md) section 12, closure condition 2.

## After attempt 1

| Category | Fixed | Programs | Share |
|---|---|---|---|
| types | 42 | 54 | 77.8% |
| errors | 37 | 50 | 74.0% |
| resources | 38 | 49 | 77.6% |
| style | 44 | 47 | 93.6% |
| total | 161 | 200 | 80.5% |

## After attempt 3

| Category | Fixed | Programs | Share |
|---|---|---|---|
| types | 52 | 54 | 96.3% |
| errors | 46 | 50 | 92.0% |
| resources | 48 | 49 | 98.0% |
| style | 46 | 47 | 97.9% |
| total | 192 | 200 | 96.0% |

Criterion (at least 160 of 200 within 3 attempts): met.

## How the trial ran

- One fresh agent per program and attempt, through the agents of a Claude Code session (no API key, as the project lead decided). Each agent read its prompt file and wrote its answer file at paths it was given; the transcripts of all 255 agent runs (200, 39 and 16) were audited and none made another tool call. The only further call in each transcript is the session's own hand-back of the agent's final word to the orchestrator, which reads and writes nothing.
- The reference fixes and hidden checks were moved out of the trial checkout before the first prompt and restored after the third attempt (400 of 400 files, byte-identical); the compiler and the harness ran from a separate checkout of commit `5563349` with its own build and did not change during the trial.
- The harness runs `unio check` and `unio test` without a time limit of its own, and the first run of the attempt-1 check did not finish in 30 minutes. Without changing the harness, the checks ran under an external policy with `limits.wall_seconds = 30` and `compile.wall_seconds = 120`, which only bounds time and grants or removes no permission; the reference fix of a permission program passes under it as without it.
- Not fixed after three attempts: `bank_method_without_try` (E0012), `private_std_field` (E0056), and six programs whose answers check clean but fail their hidden tests: `condition_not_bool`, `guard_case_not_exhaustive`, `inline_handler_without_block`, `match_result_missing_error`, `nested_option_enum_match`, `resource_in_list_literal`.
- Found during the trial: `unio check` accepts a manifest with `files.read = true` (not a list of scopes), which only `unio run` and `unio test` reject; five attempt-1 answers failed that way. To be fixed after the trial, so that the check reports it.
- 80% is the criterion of this control set; it does not show the same success on arbitrary programs.
