# Reference utilities

The ten programs of requirements section 9.1. They are tests, benchmarks, and material for the reading trial, and they grow with the language. `todo` lives in [docs/examples/](../docs/examples/todo.unio); the other nine are here. Each file states its usage and exit codes in its documentation comment and has `test` blocks for its logic, written before the code (concept, section 1.2).

| Utility | File | What it does | Permissions it needs |
|---|---|---|---|
| `todo` | [../docs/examples/todo.unio](../docs/examples/todo.unio) | Task list in a JSON file | `files` for the storage file, `env` for `TODO_FILE` |
| `wc` | [wc.unio](wc.unio) | Counts lines, words, and bytes | `files.read` |
| `grep-lite` | [grep_lite.unio](grep_lite.unio) | Lines of files that contain a pattern | `files.read` |
| `json-query` | [json_query.unio](json_query.unio) | Selects a value from JSON by a dotted path | `files.read` |
| `rename` | [rename.unio](rename.unio) | Renames files by replacing a part of their names, all or nothing | `files.read`, `files.write` |
| `env-check` | [env_check.unio](env_check.unio) | Checks environment variables and program versions from a list | `files.read`, `env`, `process` for the listed programs |
| `csv-sum` | [csv_sum.unio](csv_sum.unio) | Sums numeric columns of a CSV file | `files.read` |
| `backup` | [backup.unio](backup.unio) | Copies a directory tree with exclusions, each file atomically | `files.read` for the source, `files.write` for the target |
| `log-tail` | [log_tail.unio](log_tail.unio) | Last lines of a log, filtered by level and text | `files.read` |
| `release-notes` | [release_notes.unio](release_notes.unio) | Markdown release notes from `git log` | `process = ["git"]` |

## Programs of the reference customer

Scenario programs of tanstaafl, a product written with AI ([../docs/customers.md](../docs/customers.md)). They follow the same rules and are checked the same way; the gaps they find are recorded in that document.

| Program | File | What it does | Permissions it needs |
|---|---|---|---|
| `risk-register` | [risk_register.unio](risk_register.unio) | Ranks audit findings by annual loss in rubles with ROSI; findings without a norm reference are rejected, unapproved ones are drafts | `files.read` |
| `event-journal` | [event_journal.unio](event_journal.unio) | Event journal of a site agent: pending, acknowledgement, compaction with the 72-hour autonomy window | `files.read`, `files.write` for the journal |

All these programs use only the standard library in [stdlib/](../stdlib) and the `application` profile. `spec/check_grammar.py` and `spec/check_api.py` check them in CI; once the interpreter of stage 3 runs, their tests run as well.
