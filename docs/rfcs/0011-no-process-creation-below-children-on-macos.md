# RFC 0011: On macOS, under a process limit, the children of a program start no processes

- Status: accepted (decided by the project lead on 2026-11-13; implemented in `compiler/unio-runtime/src/sandbox/mod.rs`, `macos.rs`, and `tree.rs`, see Verification; the macOS tests run only in CI)
- Author: Unio project, with Claude
- Date: 2026-11-13
- Affects: requirements (SB-3) / runtime documentation (docs/runtime.md 9.5)
- Russian localization: [0011-no-process-creation-below-children-on-macos.md](../ru/rfcs/0011-no-process-creation-below-children-on-macos.md)

## Summary

On macOS, when the policy sets `limits.processes`, a child of `process.run` and everything it runs may create no process. The program itself still starts its children through `process.run`, each counted by the runtime before it starts, so the number of processes the program causes is exactly the number of its calls that the runtime let through, never more than the limit. A child that tries `fork`, `vfork`, or `posix_spawn` gets `EAGAIN`. Linux and Windows are unchanged: there the system counts the whole tree. Because the kernel exempts root from the rule, a program with that limit does not start on macOS when its real or effective user is root (code 103), and a child may not execute a set-user-ID or set-group-ID file. The search of the tree that the runtime made every 5 ms is removed.

## Motivation

`limits.processes = N` counts every process the program starts and every process a child or one of its descendants creates (docs/runtime.md 9.5). Linux makes each creation wait for the runtime (a seccomp filter with a listener), Windows lets the Job Object of a child hold at most the processes left. macOS has no such mechanism without root, so the runtime searched the tree of a child every 5 ms (`over_limit` in `compiler/unio-runtime/src/sandbox/tree.rs`, called from the loop of `run` in `externs/process.rs`) and stopped a tree found over the limit. A process that started and ended between two searches was not counted, and a burst of creations ran for up to one interval before the tree was stopped:

```unio
import std.process

## Under limits.processes = 3, with process = ["sh"] in the manifest.
public function main():
    let script = "for n in 1 2 3 4 5 6 7 8; do true & done; wait"
    let output = try process.run("sh", ["-c", script], timeout_seconds = 10) on ProcessError as e:
        print(e.reason)
        return
    print("status {output.status}")
```

Under a limit of three, the eight short subshells could all run on macOS, since each may end before the next search. The release review of 0.1 lists this as a blocker ([release-0.1.md](../release-0.1.md)). The project lead decided on 2026-11-13 that on macOS the descendants of a program may not start processes when a limit is set, so that the limit is exact.

## Why existing constructs are not enough

No language construct is involved. SB-3 named a number of processes without saying what is counted or how; the meaning has to be stated for each system for the limit to be checkable, and on macOS it now differs in what a child may do.

## Design

### What the program may start

The program starts processes only through `process.run`. Before each call starts its child, the runtime counts it (`may_run` in `compiler/unio-runtime/src/sandbox/mod.rs`); once N calls have started a child, the next call fails with a `ProcessError` of kind `PermissionDenied` and the reason `denied by the sandbox: the policy limits the program to N processes`, and no process is created. This was already exact and does not change.

### What a child may start

In the child, after the runtime forks it and before it runs its program, the runtime sets both the soft and the hard `RLIMIT_NPROC` of the child to 0 (`deny_process_creation`, called from `limit_child`). `fork1` in the XNU kernel serves `fork`, `vfork`, and `posix_spawn` alike and fails with `EAGAIN` when the number of processes of the real user of the caller, counting the new one, exceeds the soft limit of the caller; with a limit of 0 that holds for every creation. A process that is not root cannot raise its hard limit, and the limit is inherited across `fork` and `exec`, set-user-ID programs included. So the child cannot start a grandchild, and no grandchild exists to start more. A child can still replace itself with `exec` (the profile allows only the listed programs, docs/runtime.md 9.5), which creates no process. If the limit cannot be set, the spawn fails and the child never runs its program.

### What a denied descendant sees

`fork` and `vfork` return -1 with `errno` set to `EAGAIN` ("Resource temporarily unavailable"), and `posix_spawn` returns `EAGAIN`; it is the error Linux gives past the limit. A shell reports that it cannot fork: `sh` on macOS is `bash`, which retries a fork that fails with `EAGAIN` with growing pauses for about 15 seconds before it reports `fork: Resource temporarily unavailable`, so a call that runs a shell should allow for that in its `timeout_seconds`. The call of `process.run` itself returns normally with the status and output of the child.

### Why not Seatbelt

The decision named the Seatbelt profile, and its rules were checked first. They cannot carry it:

- `sandbox_init` applies a profile to the whole process, and every process it starts inherits that profile; a process that already runs under a profile cannot apply another one. Apple's developer support states that a child of a sandboxed process cannot be given a different sandbox (Apple Developer Forums, thread 747880), and the runtime already relies on the profile being applied once (`macos.rs`, docs/runtime.md 9.4).
- The program forks its children itself, so its profile must allow `process-fork`, and the children run under exactly that profile. The profile language has no documented filter on `process-fork` that tells the program from its children; `process-exec` controls which file a process may execute, not whether it may create a process, and the children must execute their listed programs.

Denying `process-fork` to the children would need a process outside the profile of the program to fork them and apply a second profile before they run (see Alternatives). The Seatbelt profile of the program is unchanged by this RFC: it still allows `process-fork` for `process.run` and still restricts what a child may execute.

### Root and set-ID files

`fork1` does not check `RLIMIT_NPROC` for a real user of 0. Two ways lead to it, and both are closed:

- **A program run as root.** On macOS, when the policy sets `limits.processes` and the real or the effective user of the process is 0, the sandbox is not applied and the program does not start: code 103, before `main`, with the reason `the sandbox cannot hold limits.processes for this program: on macOS the limit is exact only because a child may create no process, which the system does not enforce for root, and the program runs with real user R and effective user E (RFC 0011). Run it as a user other than root, or without limits.processes`. An effective user of 0 counts too, since it can make its real user 0. The check is made each time the sandbox of a program is entered, so in `unio test` per file, and in a built executable as in `unio run` (`root_refusal` in `sandbox/mod.rs`). The test hook `UNIO_TEST_AS_ROOT=1` makes the runtime take both users as 0; it can only refuse a start.
- **A set-user-ID or set-group-ID file.** A child of a program that is not root becomes root only by executing a set-user-ID file owned by root and setting its real user from its effective one. So under the limit, before each call of `process.run` starts its child, the runtime checks every file the child may execute: the files the Seatbelt profile allows with `process-exec`, which the sandbox resolved once with links resolved (the listed programs and the interpreters of listed scripts, docs/runtime.md 9.5; in `unio test` those of every program of the run, since the profile is their union). Each is opened without following a link as its last name and checked through the descriptor with `fstat` (with `lstat` if it cannot be opened for reading); a file whose mode has the set-user-ID or the set-group-ID bit fails the call with `PermissionDenied` and the reason `denied by the sandbox: under a limit of processes on macOS a child may not execute the set-user-ID or set-group-ID file F (RFC 0011)`, and so does a path that is a link now or not a regular file. A file that does not exist is skipped. All files are checked, not only the one the call names, because a child may execute any of them (a listed `sh` can run a listed set-user-ID program). The set-group-ID bit grants no user, but it is refused as well, so that no file changes the credentials of a child. The program is then executed by the path the sandbox resolved and checked, not searched again through `PATH`, with its name as written as `argv[0]` (`child_command`). In the child, after the limit is set and right before it executes, the same files are opened and checked again, and each must still be the same file (device and inode) with neither bit; otherwise the start fails with `EPERM` and the child never runs its program (`recheck_executables`).

What is left is a file that changes after the last check: a listed file replaced, or its bits set, while a child runs. macOS has no `fexecve`, so the kernel resolves the path once more when it executes. Setting the set-user-ID bit on a file owned by root, or putting such a file at a listed path, needs root or write access to that path; the program and its children write no file at the level of the system (docs/runtime.md 9.2, the profile has no `file-write` but `/dev/null`), and the writer process cannot set either bit. So the limit is exact against everything the program and its children can do; a root process outside the sandbox that changes the listed files is outside the threat model of SB-9.

The kernel closes that case as well. Seatbelt forbids executing a set-user-ID or set-group-ID file under every profile, whatever its rules: on the macos-26 runner the call is denied with `EPERM` and the log records `deny(1) forbidden-exec-sugid` (CI run 43397303584, test `a_set_id_file_is_not_executed_under_a_process_limit_on_macos`, which now checks that a listed set-ID file is refused even without a limit). So a file swapped after the last check still does not execute as set-ID; the runtime's check stays because it fails the call before the child starts, with a reason that names the file and the rule.

### The search of the tree is removed

With the denial, the refusal of root, and the check of set-ID files, no process can appear in a tree besides the child, so the search every 5 ms (`over_limit` in `tree.rs`) is removed on macOS: `over_limit` reports `false` there, as on Linux. The search of the descendants that stops a tree (the group, the parents, the marker of the profile) stays, since a descendant can still leave the process group without starting a process (`setsid`).

### Normative text

SB-3 states that the number of processes counts every process the program starts and every process the trees of its children create, and that on macOS, under that limit, the children of the program may create no process and may not execute a set-user-ID or set-group-ID file, and a program whose real or effective user is root does not start (code 103). docs/runtime.md 9.5 describes the mechanism, the errors, and what is left.

## Migration

Not needed for sources. On macOS a program with `limits.processes` whose children start processes (a shell that runs a pipeline or another program, a build tool that runs a compiler) now fails in that child; it must start each program through its own call of `process.run`, or run without a process limit. A program with the limit that runs as root, or whose children may execute a set-ID file, must run as another user, drop the limit, or stop listing that file. On Linux and Windows nothing changes.

## Alternatives

- **Keep the search for root.** The first draft of this RFC kept the search every 5 ms for root and for a child made root, with its inexactness; the project lead asked on 2026-11-13 for an exact limit without exceptions.
- **A launcher outside the profile.** A process started before the sandbox applies, like the writer process (docs/runtime.md 9.2), would fork each child on request of the program, apply to it the profile of the program without `process-fork`, and pass its pipes back. Seatbelt would then carry the rule, also for root, and the program itself would need no `process-fork`. But the launcher runs without a profile and acts on requests of the program, so it would have to check each request as the writer does, and it would take over waiting for the children and stopping their trees, which the program now does through its process group, its marker, and `waitid`. It is a larger change and would still need the refusal of set-ID files, since a second profile does not change credentials.
- **Search more often.** A shorter interval narrows the window but does not close it.
- **Count with `RLIMIT_NPROC`.** It counts every process of the user, not of the tree, so it cannot be set to the processes left; set to 0 it denies, which is what this RFC uses.
- **Refuse `limits.processes` on macOS** with code 103: exact, but a program could then not limit its processes at all on macOS.
- **Endpoint Security or `NOTE_TRACK` of kqueue**: Endpoint Security needs an entitlement and root, and `NOTE_TRACK` no longer exists.

## Verification

- `compiler/unio-runtime/src/sandbox/mod.rs`: `a_child_without_process_creation_cannot_fork` (macOS): a child that sets the limit tries `fork` before it runs its program and gets `EAGAIN`; without the limit the same child forks. It fails under root instead of passing without checking.
- `compiler/unio/tests/sandbox.rs`: `a_child_under_a_process_limit_cannot_start_a_grandchild_on_macos` (macOS): a C child tries `fork` five times and `posix_spawn` once; without a limit all succeed; under `processes = 2` two calls of `process.run` run it, every creation fails with `EAGAIN`, and the third call is denied before it starts, so the program started two processes in all. A missing C compiler or a run as root fails the test. `a_child_that_starts_more_processes_than_the_limit_is_stopped_or_its_starts_fail` now expects on macOS that the shell cannot fork under either limit.
- `compiler/unio-runtime/src/sandbox/mod.rs`: `root_is_refused_a_limit_of_processes` (every system: the reason for real and effective users of 0) and `set_id_files_links_and_directories_are_refused_to_children` (every Unix: a set-user-ID file, a set-group-ID file, a link, and a directory are refused through an open descriptor, a missing file is skipped, a plain one passes with its device and inode). These two ran on Linux x86-64.
- `compiler/unio/tests/sandbox.rs`: `a_program_with_a_process_limit_is_refused_under_root_on_macos` (macOS: under the hook `UNIO_TEST_AS_ROOT=1` the program with `processes = 1` ends with 103 before `main`, without the limit it runs under the hook, and with the limit it runs without the hook) and `a_set_id_file_is_not_executed_under_a_process_limit_on_macos` (macOS: a set-user-ID and a set-group-ID copy of `/usr/bin/true` are listed with `sh`; under a limit both calls are refused with `PermissionDenied` naming the file, without a limit both run). A file system that drops the bit fails the test.
- The macOS tests run only on macOS, so only in CI; on Linux the code was built for `aarch64-apple-darwin` with `cargo clippy --release --all-targets -- -D warnings`.
