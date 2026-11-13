# Release 0.1: what the project needs now

Russian localization: [ru/release-0.1.md](ru/release-0.1.md).

Release 0.1 "Core" is **not published and is blocked**. This page lists what stands between the current `main` and the release: the checks a candidate must pass, the work in progress, and the tasks only people can do. The normative conditions are in [plan.md](plan.md) section 12 (closure and release conditions) and the per-requirement facts in [status.md](status.md); this page is the working summary of both and changes with them.

## Where it stands

- The last full review was of the candidate `a61b961` (2026-11-13). Its CI was not green: three jobs failed (the Windows native link, `std.path` on Windows, the CPU-limit classification on Linux arm64), so the release build, the installation without a checkout and the independent reproducible build were skipped on that commit. Success of older commits does not carry over to a candidate.
- Closure conditions 1–3 (the release reserve C-17, the R-10 control set and its model trial, the decision on C-13) are met. The R-10 trial fixed 161 of 200 programs after one attempt and 192 of 200 within three ([report](../bench/results/control_trial-2026-11-12.md)).
- Release conditions 1–5 were met on earlier commits (installation, reproducibility on Linux x86-64, pinned toolchain, licences, file-access races on Linux, macOS and Windows) and must be shown again on the new candidate.

## Blockers being fixed

| Blocker | What is done about it |
|---|---|
| Windows: `bcrypt.lib` missing when linking native executables | fixed, awaiting CI: the system libraries are those rustc reports, checked by a test; a native test of the secure random functions |
| Windows: `std.path` tests assume POSIX paths; `starts_with(path, ".")` mishandles `\a` and `C:a` | fixed, awaiting CI: the platform contract of `std.path` written in [stdlib.md](stdlib.md) and tested on each platform |
| Linux arm64: a CPU-limit kill reported by wall time as a fault (101 instead of 102) | fixed, awaiting CI: classification by the CPU time the system recorded for the worker |
| `limits.heap_bytes` counts an estimate of some values | fixed, awaiting CI: a hard limit by [RFC 0010](rfcs/0010-a-hard-heap-limit.md); the allocator counts every block the program requests, with an OS limit behind it on Linux and Windows |
| macOS: the process limit is a search every 5 ms, so a short excess is possible | fixed, awaiting CI: by [RFC 0011](rfcs/0011-no-process-creation-below-children-on-macos.md) the children of a program may start no process when a limit is set (`RLIMIT_NPROC` of 0 in each child, so every creation fails with `EAGAIN`), and `process.run` counts each child before it starts; a program run as root does not start with the limit (code 103) and a child may not execute a set-user-ID or set-group-ID file, so the search every 5 ms is removed |
| Sandbox tests that return early when Python or a C compiler is missing count as passed; two worker-isolation tests do not run on Windows; Windows tests and CI still allow the weaker sandbox | fixed, awaiting CI: CI sets `UNIO_REQUIRE_PREREQUISITES=1` on every job, so a test that misses python3, a C compiler, rustc, or a native target fails instead of passing, and the test jobs install python3 and check the C compiler; the download and unpack worker tests run on Windows in the AppContainer and Job Object (compiled for Windows here, run on CI only); no Windows test or CI job allows the weaker sandbox, and `release_checks_refuse_the_weaker_sandbox` checks it |
| The fuzzer could report "no findings" after a worker died before recording its input | fixed, awaiting the 24-hour run: such a death fails the run; every target must work its full time; a final job checks all stages |
| Release artifacts and installation do not cover Linux arm64; the independent build is compared only on Linux x86-64 | fixed, awaiting CI: 0.1 ships on the four platforms of R-8; `release`, `install` and `reproducible` run on all four ([plan.md](plan.md) section 12, conditions 1 and 2); on Windows `unio.exe` is linked with `/Brepro`, so it no longer carries the link time, and a test checks the flag |
| [status.md](status.md) still names an older snapshot and has rows that disagree with the code | every partial row is confirmed with evidence, fixed, or deferred by an accepted RFC; no deferral is assumed |

## Checks a candidate must pass

All on one fixed commit, after the blockers above:

1. **The CI matrix is green on every job** ([ci.yml](../.github/workflows/ci.yml)): Linux x86-64, Linux arm64, macOS arm64 and Windows x86-64, including the sandbox suites SB-8 and SB-15 (R-7a), the control set (R-10 check A), the benchmarks and limits against Go (RFC 0003), the pause of freeing (RFC 0007), the pinned toolchain records, the release build, the installed `unio` without a checkout, and the independent reproducible build.
2. **The sanitizers** ([sanitizers.yml](../.github/workflows/sanitizers.yml)): the runtime under ASan and MSan and every Unio test through an ASan build.
3. **24 hours of fuzzing** ([fuzz.yml](../.github/workflows/fuzz.yml), started by hand): five stages of 300 minutes on the same commit, every target its full time, and the `fuzz-evidence` report saying "met" (R-1). It is copied to `bench/results/` with the release.
4. **The local checks** of [AGENTS.md](../AGENTS.md) and [compiler/README.md](../compiler/README.md) for anyone preparing a change: `cargo fmt --check`, `cargo clippy --all-targets -D warnings` for the three host targets, `cargo test --workspace`, `unio fmt --check`, `unio check` and `unio test` over the sources, and the four Python checks.

## What only people can do

- **Review the external functions (R-5).** Two people review all 115 external functions of the standard library against [extern-review.md](extern-review.md), on the full SHA of the candidate, record their findings, and check the fixes again. The automatic part (sanitizers) is not a substitute; the release is not published while either reviewer column is pending.
- **Sign the release with the release key.** The production key is kept offline and never given to CI (CI signs only with a key of its run). The release manager signs the verified bytes after installation and reproducibility are shown, and the published set is checked against the public key from an independent trusted channel ([packages.md](packages.md), [cryptography.md](cryptography.md)).
- **Decide deferrals.** A requirement that is not met by the release is either fixed or deferred by an RFC that the project lead accepts; nothing is deferred by default.
- **Run the release.** Start the fuzzing workflow on the final candidate, check its evidence, publish the release notes with the results of 7.1 and of R-10.

## Help that is welcome

- Running the tests on machines other than the CI images (other Linux distributions, older macOS, Windows with developer mode), and reporting differences.
- Reviewing the sandbox ([runtime.md](runtime.md) section 9) and the cryptography ([cryptography.md](cryptography.md) section 7) independently; reports go through [SECURITY.md](../SECURITY.md).
- Writing programs in Unio and reporting diagnostics that are unclear or wrong; [How Unio works with AI](ai.md) describes the loop an agent uses.

Accepted decisions: TLS is not part of 0.1 (packages are authenticated by ML-DSA-65 signatures, downloads are not private); reference counting replaces a tracing GC in 0.1 (RFC 0007); the limits of compile workers are separate from program limits (RFC 0006); the heap limit is hard (RFC 0010).
