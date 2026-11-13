# The Unio compiler

The front end of the Unio compiler, written in Rust (requirements, section 10): the lexer, the
parser, the syntax tree, diagnostics and their catalog, the formatter, the outline, the static
checks of [docs/semantics.md](../docs/semantics.md), `unio symbol`, and the interpreter of
[docs/runtime.md](../docs/runtime.md) behind `unio run` and `unio test`, which run every program
in the sandbox of SB-1, and `unio build`, which compiles a program to one native executable
through Cranelift (docs/runtime.md 8.4). These are sprints S1 to S5 of the plan, the sandbox
part of S6, and its native code generation for Linux on x86-64 and AArch64, macOS on AArch64,
and Windows on x86-64.

## Build and test

The front end, the interpreter and the runtime use the Rust standard library only. The code
generator of `unio build` uses Cranelift (`cranelift-codegen`, `cranelift-frontend`,
`cranelift-module`, `cranelift-object`, a stable release pinned to exact versions, with the
crates they need recorded in `Cargo.lock`), the one dependency that requirements section 10 names;
an executable contains none of it, because the linker drops what the runtime does not call. On
Windows the compiler also uses `find-msvc-tools`; see "The linker on Windows" below; signatures
use `ml-dsa`, see "Signatures" below. The
sandbox calls Landlock, seccomp, resource limits and Seatbelt through `extern` declarations in
`unio-runtime/src/sandbox/`, as the runtime already does for `fsync`, `getpwuid` and
`MoveFileExW`, so `libc` and `windows-sys` are not dependencies (R-6). The package commands
(`unio add`, `remove`, `update`, `install`, `publish`; [docs/packages.md](../docs/packages.md))
add none besides `ml-dsa`: SHA-384, the archive format (uncompressed ustar), the HTTP/1.1 client
of the download worker, semantic versions, the resolver, the signature envelopes, and the trust
of registries are written with the standard library in `unio/src/package/`,
`unio/src/envelope.rs`, and `unio/src/packages.rs`. There is no TLS in 0.1, by a decision of
2026-11-12: packages travel over HTTP and are installed only with a valid signature and hash;
TLS comes with the networking of 0.2 (docs/status.md, gap 12).

The library crate `unio` is also built as a static library, `target/<profile>/libunio.a` (on
Windows `unio.lib`): the runtime that `unio build` links into every executable, found through
`UNIO_RUNTIME_LIB`, next to the `unio` binary (as `cargo build` leaves it), or in `lib/unio/`
beside the `bin/` directory of an installed `unio`. The static library holds the whole crate, the
front end and Cranelift included (about 80 MB in a release build against 12 MB for `unio`), and
the linker takes from it only what an executable calls, so it is installed next to the compiler
rather than embedded in it; embedding would also need a second build, because the binary and the
static library are made by the same `cargo build`. The installed layout, made by
`sh .github/scripts/release_build.sh PREFIX` with the pinned toolchain and the lock file:

```text
PREFIX/bin/unio                  the compiler, with the standard library embedded in it
PREFIX/lib/unio/libunio.a        the runtime of `unio build` (unio.lib on Windows)
PREFIX/share/doc/unio/           LICENSE-MIT, LICENSE-APACHE, THIRD-PARTY-NOTICES
```

The script remaps the paths of the checkout and of `CARGO_HOME`, which reach the binary only
through the panic locations of the crates, to `/unio` and `/cargo`
(`RUSTFLAGS=--remap-path-prefix=...`), so that builds in two directories or by two users give the
same bytes (C-4, R-6; the CI job `reproducible` compares two runners). An executable is made for the machine the
compiler runs on, since it links that machine's runtime. Linking needs the system linker: the C
compiler on Linux (`cc`, or `CC`); on macOS the Xcode command line tools (`clang` and the SDK,
found by `xcrun`, or `CC`); on Windows the Visual Studio Build Tools with the C++ workload
(`link.exe` and a Windows SDK, found without a developer command prompt).

```sh
cd compiler
cargo build --release          # target/release/unio
cargo test                     # corpus, repository sources, diagnostics, fuzzing, interpreter
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test --release -p unio --test tools -- --ignored   # Unio tools against the Python checkers
cargo test -p unio --test native                          # native executables against unio run
```

The workspace has two crates: `unio` (this front end, the interpreter, and the command) and
`unio-runtime` (values, faults, the release stack, and the externals; see its README).

`UNIO_FUZZ_ITERATIONS=100000 cargo test --release --test fuzz` runs a longer mutation run.

Some tests need a program besides Rust: `python3` (the tests of leaving the process group in
`tests/sandbox.rs` and `unio-runtime/tests/process.rs`), a C compiler (`cc`, or `CC`, for the C
helper of `tests/sandbox.rs` and for linking in `tests/native.rs` and `tests/install.rs`), and
`rustc`; the native tests also need a machine for which executables are made. Without one a
test notes `skipped:` on standard error, which `cargo test` hides, and passes. With
`UNIO_REQUIRE_PREREQUISITES=1`, which CI sets on every job, such a test fails instead, so a
release check cannot pass without running its body (R-7a, SB-8, SB-15). No test and no CI job
allows the weaker sandbox (`allow_weaker`, `--allow-weaker-sandbox`): Linux, macOS, and Windows
have every mechanism, and `tests/sandbox.rs` (`release_checks_refuse_the_weaker_sandbox`)
checks it.

```sh
UNIO_REQUIRE_PREREQUISITES=1 cargo test   # as CI runs it: a missing prerequisite is a failure
```


### The linker on Windows

`unio build` links native executables with the system linker: `cc` on Linux, `clang` of the
Xcode SDK on macOS, and `link.exe` of Microsoft Visual C++ (MSVC) on Windows. Windows has no
`link.exe` on its search path outside a developer command prompt, so the compiler finds it
with [`find-msvc-tools`](https://github.com/rust-lang/cc-rs) (the code rustc and `cc` use for
the same task). It is a dependency only on Windows (`[target.'cfg(windows)'.dependencies]` in
`unio/Cargo.toml`), pinned to `=0.1.15`, licensed `MIT OR Apache-2.0`, with no dependencies of
its own; `Cargo.lock` records its checksum, and an update is a deliberate change of both.

- What must be installed: the Visual Studio Build Tools (or Visual Studio) with the workload
  "Desktop development with C++", which brings MSVC and a Windows SDK. When `link.exe` is not
  found, `unio build` stops with a diagnostic that says what to install; it never installs
  anything itself.
- How the toolchain is chosen: run `unio build` from a developer command prompt
  ("x64 Native Tools Command Prompt" of the chosen Visual Studio, or after `vcvarsall.bat x64`)
  to use exactly that installation and its Windows SDK, as named by `VCINSTALLDIR`,
  `VSCMD_ARG_TGT_ARCH`, `LIB` and `PATH`; without one, the newest installed Visual Studio with
  the C++ workload is used, as `vswhere` reports it. Outside a developer command prompt,
  `VCToolsVersion` (an MSVC toolset, as in `14.44.35207`) and `WindowsSdkDir` with
  `WindowsSDKVersion` (as in `10.0.26100.0\`) restrict that search to one toolset and one
  Windows SDK; CI pins them this way (see "Pinned linkers and SDKs" below), because in Git Bash
  a developer prompt's `PATH` can put `/usr/bin/link.exe` of coreutils before that of MSVC.
- Reproducibility: pinning the search library does not pin what it finds. A build is
  reproducible on the same `link.exe` and the same Windows SDK, so every build records their
  versions in the program image of the executable, next to the identity of `unio` (C-4, R-6):
  the MSVC toolset from the directory of `link.exe` (`VC\Tools\MSVC\<version>`, else its
  banner) and the Windows SDK from `LIB` (`Windows Kits\10\lib\<version>\um`), as in
  `x86_64-pc-windows-msvc; link.exe: MSVC 14.51.36231; sdk: Windows 10.0.26100.0`; on Linux and
  macOS the record names the compiler and its version, and on macOS the SDK.
  `unio build --print-toolchain` prints the identity a build would record, and
  `unio::native::image::made_by_executable` reads it from an executable (docs/runtime.md 8.4).
  `/Brepro` removes the time stamp, so the same inputs and the same toolchain give the same
  bytes.
- Its role is temporary: it belongs to the bootstrap compiler in Rust. It is reviewed when the
  compiler moves to tools of its own (the self-hosted compiler of concept section 10, by 1.0),
  or earlier if a linker is shipped with Unio; until then it is, with `ml-dsa` (see
  "Signatures"), the only dependency besides Cranelift. It is called by the `unio` tool only (`compiler/unio/src/main.rs`), not by the
  library whose static form is the runtime, so it is not part of what `unio build` puts into
  programs.

### Pinned linkers and SDKs

Plan section 12, release condition 3. Rust is pinned by `rust-toolchain.toml` and the crates by
`Cargo.lock`; the C compiler, the linker, and the SDK that link `unio`, `libunio.a`, and every
executable of `unio build` come from the machine, so CI pins them as well:

- Every job of `.github/workflows/ci.yml` runs on a named runner image, never on a `-latest`
  label, which moves to another system without a change in the repository: `ubuntu-24.04`,
  `ubuntu-24.04-arm`, `macos-26`, and `windows-2025-vs2026` (Windows Server 2025 with Visual
  Studio 2026). These were the images of the `-latest` labels on 2026-11-12, except that
  `windows-latest` is spelled with the Visual Studio of its image.
- `.github/scripts/expected_toolchain.py select`, a step of every job that builds or links,
  selects the toolchain inside that image by exact name and version and passes it to the
  later steps (`GITHUB_ENV`): on Linux `CC=gcc-13` (GCC 13.3.0 of Ubuntu 24.04 with binutils
  2.42), also as the linker of rustc; on macOS Xcode 26.6 with the macOS 26.5 SDK
  (`sudo xcode-select -s /Applications/Xcode_26.6.app/Contents/Developer`, and
  `DEVELOPER_DIR`); on Windows the MSVC toolset 14.44 (of Visual Studio 2022 17.14, installed
  on the image as a fixed component beside the newer toolsets of Visual Studio 2026, so that an
  update of Visual Studio does not move it) and the Windows SDK 10.0.26100.0, through
  `VCToolsVersion`, `WindowsSdkDir`, and `WindowsSDKVersion`. The step fails when a pinned part
  is not on the image, or when the Universal CRT that `find-msvc-tools` would take comes from
  another SDK.
- `.github/scripts/expected_toolchain.py check UNIO`, the step "The linker and SDK are the
  pinned ones" of the jobs `release`, `install`, and `native-linux-arm`, compares the record of
  `unio build --print-toolchain` with the expected one written in the script, and the parts the
  record does not name: the Debian versions of `gcc-13`, `binutils`, and `libc6-dev` on Linux,
  `xcodebuild -version` on macOS, and the selected toolset on Windows. It prints the image
  (`ImageOS`, `ImageVersion`) and fails on any difference. An expected record may hold `*` for a
  part not yet read from CI (the build number of Apple clang, the build of the MSVC toolset);
  the step then passes with a warning that prints the record to write in its place, and the
  condition is met only when no record holds `*`.

`python3 .github/scripts/expected_toolchain.py show` prints the pins. Hosted runner images are
updated every week, and a security update of a compiler package changes its version string:
the check then fails and names both records. Updating a pin is a deliberate change of one
commit: choose the image or the toolchain, change `PINS` in the script (and the runner labels
when the image changes), run CI, copy the actual records printed by the check into `PINS`,
and say in the commit why the toolchain changed, since every executable built after it
records another identity. An image or a component that leaves the hosted runners is replaced
the same way, never by going back to a `-latest` label.

### Signatures

The signatures of packages, registries, and release manifests are ML-DSA-65 of FIPS 204
([docs/cryptography.md](../docs/cryptography.md), sections 4 and 7; T-8, T-8a, R-6a). The
compiler does not implement the primitive itself (cryptography.md, section 5: no invented
primitives); it uses one vetted implementation in pure Rust, the crate
[`ml-dsa`](https://github.com/RustCrypto/signatures/tree/master/ml-dsa) of RustCrypto:

- Pinned to `=0.1.2` in `unio/Cargo.toml`, licensed `Apache-2.0 OR MIT`, with `Cargo.lock`
  recording its checksum. No default features: no random number generator (`unio key new` reads
  the seed from the system itself, and signing is deterministic) and no PKCS#8; the feature
  `zeroize` clears the expanded keys when they are dropped. It is used by the library crate
  (`unio/src/envelope.rs`), so `libunio.a` contains it, but an executable of `unio build` does
  not: the runtime never calls it, and the linker drops it.
- Its dependencies, each licensed `MIT OR Apache-2.0` (in one order or the other):
  `crypto-common`, `hybrid-array`, `ctutils`, `cmov`, `typenum`, `module-lattice`,
  `num-traits` (with `autocfg` at build time only), `shake`, `digest`, `keccak`, `cfg-if`,
  `cpufeatures` (which uses `libc` only on AArch64 Linux), `sponge-cursor`, `signature`, and
  `zeroize`, without procedural macros. `THIRD-PARTY-NOTICES` lists them
  (`.github/scripts/third_party_notices.py`).
- Why this crate: it implements the final FIPS 204, is maintained by the RustCrypto project with
  the Wycheproof vectors in its own tests, needs no C compiler or system library, and is small
  enough to read. Unio checks it itself (`unio/tests/signatures.rs`): the ACVP known-answer
  vectors of NIST in `unio/tests/data/fips204` (their source and the NIST notice are in the
  `SOURCE` file there) and interoperability with an independent implementation, the crate
  `fips204` (`=0.5.0`, `MIT OR Apache-2.0`), which is a dev-dependency only, like `sha2`
  (`=0.10.9`), which checks Unio's own SHA-384; neither is part of `unio` or `libunio.a`.
- Its role is temporary, as the bootstrap compiler's: it is reviewed when the compiler moves to
  tools of its own (by 1.0), and a Unio implementation replaces it only after it passes the same
  checks and an external review. No independent review of it has been made yet, so no
  post-quantum compliance is claimed (R-6c).

The release job of CI writes the SBOM of `unio` (`.github/scripts/sbom.py`, CycloneDX 1.5 from
`Cargo.lock`, the same crates as `THIRD-PARTY-NOTICES`) into `share/doc/unio/sbom.cdx.json`, and
signs the installed layout with `unio release sign` using a key made for that run only. The
release key belongs to the operator, stays offline, and is never given to CI: the operator signs
a release with it after verifying the artifacts and the reproducible build
(`unio release sign --key release.key --subject unio-0.1.0-linux-x86_64 --sequence N --out
RELEASE FILE...`, checked by `unio release verify RELEASE --public release.pub --subject ...
--at-least N`).

## Commands

| Command | What it does |
|---|---|
| `unio check [--diagnostics=json] [--lang=en\|ru] [--profile=application\|core] FILE...` | Parses the files and the modules they import, then runs the checks of docs/semantics.md, sections 1–8, and reports every error. The semantic checks run only when every module parses. With `--profile=core`, every function of the package that needs a layer above `core` is E0084 with its chain of calls (C-21). Exit code 0 without errors, 1 with errors, 2 on wrong usage or an unreadable file |
| `unio symbol [--json] MODULE.NAME` | Signature, documentation, declaration site, usages, and for a function its error set, derived effects, the lowest layer it needs and the functions it calls, plus the content hash of every function and type (C-11 to C-11b). The module is found from a prefix of the name: `std.x` in the standard library, otherwise in the package of the current directory; prelude names such as `List.map` work too |
| `unio fmt [--check] FILE...` | Writes each file in the one style of [docs/style.md](../docs/style.md). With `--check`, writes nothing, lists the files that differ, and exits with 1 if there are any. A file with syntax errors is reported and left unchanged |
| `unio fmt --migrate [--from=VERSION] [--check] [FILE...]` | Applies the migrations of incompatible language changes (R-9): the unambiguous fixes of the registered diagnostics, then the formatting; without files, lists the registry ([docs/tools.md](../docs/tools.md), section 6) |
| `unio outline [--json] FILE` | Lists the declarations with their signatures and lines; members of structs, enums, errors, interfaces, and extend blocks are indented (C-12) |
| `unio parse --ast FILE` | Prints the syntax tree as an S-expression without spans; the form is stable and is used to compare parses |
| `unio explain CODE` | Prints the explanation of a diagnostic code with an example of the error and the example fixed (C-9) |
| `unio run [--policy FILE] [--allow-weaker-sandbox] FILE [-- ARGS...]` | Checks the program like `unio check` and, without errors, runs `public function main()` of FILE with ARGS as `std.args.list()`, in the sandbox described below. The sandbox options come before FILE, or between FILE and `--`. Exit code: the result of `main` (0 without one), 101 after a fault with its trace on standard error, 102 on exhaustion (heap, stack, wall or CPU time of the policy), 103 when the policy or the manifest cannot be read or parsed or the sandbox cannot be applied, 1 if the program does not compile |
| `unio build [--release] FILE [-o OUT]`, `unio build --print-toolchain` | Checks the program like `unio run` and, without errors, compiles it to one executable OUT (by default the name of FILE without `.unio`, with `.exe` on Windows) for the machine the compiler runs on: Linux on x86-64 or AArch64, macOS on AArch64, Windows on x86-64; elsewhere it stops with a message naming these. `--release` optimises the generated code. The executable runs `main` with its arguments as `std.args.list()`, in the sandbox of the manifest of FILE's package narrowed by `UNIO_POLICY`, and ends with the codes of `unio run`, or with 104 when its program image is of another format, was made by another compiler than its runtime, or is damaged. `$CWD`, `$HOME` and `$TEMP` in the manifest are those of the run; a relative entry of `files.read` or `files.write` stops the build, since an installed executable has no directory of its manifest. A construct not compiled natively yet stops the build with its place (docs/runtime.md 8.4). The image of the executable records the identity of the linker and the SDK (their names and versions); `--print-toolchain` prints it without building. A missing linker stops the build with a message that says what to install. Exit code 0 when built, 1 otherwise, 2 on wrong usage |
| `unio test [--json] [--seed N] [FILTER] FILE...` | Runs every `test` block of the files, each in a fresh state (runtime 6.2), or those whose name contains FILTER. A test passes, fails with the fault that ended it, or fails because it does not compile (T-5a): a diagnostic inside a test block fails only that test, one outside fails the whole file. Text: `ok`/`FAIL`, name and time per test, then the totals; `--json` gives an array with file, name, status, time, output and message (T-4). Each file's tests run in the sandbox of its package, with the same options and codes as `unio run`. The effects analysis counts exactly the tests that run: those of the given files that FILTER selects, not those of imported modules or of the standard library unless their files are given; `unio check` and `unio run` count no test (RFC 0005, runtime 6.2). Exit code 0 when every test passed, 1 otherwise The examples of documentation comments run as tests named `doc: NAME (line N)`; a failing property prints its replay, `--seed N` gives every property that seed, and `--json` adds the replay records ([docs/tools.md](../docs/tools.md), sections 2 and 4) |
| `unio add`, `remove`, `update`, `install`, `publish`, `unio key`, `unio registry`, `unio release` | Dependencies, the signed registry, keys, and release manifests: [docs/packages.md](../docs/packages.md), sections 3 and 7, and "Signatures" above |
| `unio lsp [--policy FILE] [--allow-weaker-sandbox] [--lang=en\|ru]` | The language server over LSP on standard input and output (T-10): diagnostics of the open documents with their codes and a link to the explanation (`unio-explain:/CODE`, answered by the request `unio/explain`), go to definition, hover with the type of a name or an expression and, for a function, its signature, documentation, errors, effects, and layer as `unio symbol` reports them, type hints for bindings written without a type, completion (names in scope, members by type, module members, keywords), formatting as `unio fmt`, and rename of every reference in the package (refused for the standard library, another package, a module, or a keyword). Every diagnostic carries the revision of its document, its fixes and, for E0061, the expected type; `textDocument/codeAction` offers the fixes only at that revision, as edits that name it ([docs/tools.md](../docs/tools.md), section 3). Every analysis runs in a worker of the tools with the `[compile]` limits; unsaved buffers reach it on its standard input (SB-13, docs/runtime.md 9.7). The editors start it: [editors/README.md](../editors/README.md) |
| `unio repl [--policy FILE] [--allow-weaker-sandbox] [--lang=en\|ru]` | A checked interactive session (T-13): each input is checked as a module of the package in a worker that runs nothing, and runs in the sandbox of `unio run` in a separate process; bindings are kept by value, `:type` runs nothing, and fixes apply only at their revision ([docs/tools.md](../docs/tools.md), section 1) |
| `unio api-diff [--json] [--callers DIR]... OLD [NEW]` | The public API of a package, or the changes from OLD to NEW with their level, the semantic-version verdict (exit code 1 when NEW's version is too small), the affected places of the caller packages, and unambiguous migrations (T-15, [docs/tools.md](../docs/tools.md), section 5) |

The language of messages comes from `--lang`, then from the environment variable `UNIO_LANG`,
and is English otherwise (C-10). Codes and the JSON format do not depend on it.

### The sandbox

The permissions of a program are the `[permissions]` of the `unio.toml` nearest above its file
(T-6), narrowed by the policy of `--policy FILE` or `UNIO_POLICY` (SB-2, SB-3). Without a
manifest, or with one that has no `[permissions]`, a program has only its arguments and standard
streams. `unio test` uses the same rule: a test that needs files declares them in its package.

```toml
# A policy. Without [permissions] it narrows nothing; with it, a kind it does not list is denied.
allow_weaker = false        # true is the same as --allow-weaker-sandbox (SB-5)

[permissions]
files.read = ["$CWD/src/**"]

[limits]
heap_bytes = 268_435_456    # the runtime's count of live values; 102 when exceeded; at least
                            # 2 MiB, of which 1 MiB is the reserve for release (C-17)
wall_seconds = 60           # 102 when exceeded
cpu_seconds = 30            # 102 when exceeded; each child gets RLIMIT_CPU
processes = 4               # process.run calls; more fail with PermissionDenied
file_size_bytes = 1_048_576 # writes over it fail; RLIMIT_FSIZE on Linux and macOS

[compile]                   # the workers of the tools, not the program (RFC 0006)
memory_bytes = 268_435_456  # each worker; default 2 GiB; at least 192 MiB, or 103 before compiling
wall_seconds = 120          # each worker; default 600
cpu_seconds = 60            # each worker; RLIMIT_CPU; no default
```

The `[limits]` table holds for the program only; the `[compile]` table holds for every worker
of the tools alike: `check`, `fmt`, `outline`, `symbol`, `parse`, the compile steps of `run` and
`test`, and both steps of `build` (the compile step and the link step), each worker separately.
A worker over one of them is stopped with 102 (on Windows the end at the CPU limit is not yet reported as 102). The tool never raises a limit of the operator: a
`compile.memory_bytes` below the 192 MiB a worker needs to start and compile is refused with 103
before anything is compiled (docs/runtime.md 9.7). `unio.toml` sets neither table.

A permission of the policy that the manifest does not have is a warning and has no effect.
Unknown keys are errors. Paths in either file may start with `$HOME`, `$CWD` or `$TEMP`, end
with `/**` for a subtree, and are otherwise relative to the file's directory; links are resolved
before a path is compared, so a link cannot lead out of a scope.

Two layers enforce the permissions. The runtime checks every external function of `std.files`,
`std.env` and `std.process` and reports a denied operation as `IoError` or `ProcessError` of kind
`PermissionDenied` naming the permission, `none` from `env.get`, and `false` from `files.exists`
and `files.is_directory` (SB-6, S-8). The operating system restricts the thread that runs the
program, its threads and every child process (SB-1, SB-4), after the sources are loaded and
before `main` or the first test:

| System | Mechanisms | Not covered |
|---|---|---|
| Linux | `no_new_privs`; Landlock (ABI 1 to 7: files, TCP from ABI 4, signals and abstract sockets from ABI 6); seccomp-bpf: no sockets at all, no `ptrace`, `process_vm_*`, `mount`, `unshare`, `setns`, namespace flags of `clone`, `io_uring`, `bpf`, `perf_event_open`, `userfaultfd`, `keyctl`, `open_by_handle_at`, `kexec`, modules, `TIOCSTI`; `RLIMIT_CORE` 0, `RLIMIT_FSIZE` | Landlock grants a scope of one written file on its directory, because the atomic commit creates a temporary file next to the target; scope roots that do not exist at the start grant nothing; with a `process` permission, children may start with `execve` only the listed programs, the interpreters of listed scripts and the dynamic loader, and read the library directories (`/lib*`, `/usr/lib*`, `/usr/local/lib`), the libraries the listed programs need, `/etc/ld.so.cache` and `/etc/localtime`; through the loader a child can run any ELF program it can read, and a listed interpreter runs any script it can read (docs/runtime.md 9.5); `stat` is not restricted by Landlock |
| macOS | a Seatbelt profile (`sandbox_init`, deny by default) and the same resource limits | Seatbelt applies to the whole process once, so `unio test` grants the union of its files' permissions plus reading their sources and the standard library; the runtime still keeps each file to its own; children may start only the listed programs and the interpreters of listed scripts, and a listed interpreter runs any script it can read |
| Windows | two AppContainers of their own for each run, fresh profiles without capabilities, so no network, not even the loopback, deleted when the program ends (`unio-runtime/src/sandbox/windows.rs`): the reader holds grants on exactly the read scopes, the files of the listed programs and the working directory, the thread of the program impersonates its token, and every child is created in it; the writer holds the write grants and its token is taken only for a write of the runtime, after the runtime's checks. A Job Object holds the memory backstop of `limits.heap_bytes`; each child joins, while still suspended, a Job Object of its own with kill-on-close, no breakaway, and the process limit (docs/runtime.md 9.4, 9.5) | reading is wider than the scopes by what every application package may read (the system directories, `Program Files`), and a child can run every program there; a process of an AppContainer can always write the folder of its own profile, which is private to the run and deleted with it; the thread of the program could return to the token of the process with `RevertToSelf`, which no Unio program can call (docs/runtime.md 9.4) |

A missing mechanism (no Landlock or seccomp in the kernel, Landlock below ABI 3) stops the start
with 103; with `--allow-weaker-sandbox` or `allow_weaker = true` the program runs and the missing
restrictions are printed (SB-5). On macOS and Windows nothing is missing: an AppContainer that
cannot be set up stops the start with 103 whatever the operator allows. The program itself
cannot lift any of this.

A child of `process.run` gets only the variables granted under `env` and `PATH` (on Windows also
`SystemRoot`); it runs in a process group of its own on Unix, which no descendant can leave on
Linux, and on Windows in a Job Object with kill-on-close that it joins while still suspended,
and what is left of that tree is stopped when the child ends, times out, or overflows its
output; on macOS descendants that left the group are found through their parents and the
marker of the profile (passed in CI on `macos-latest`, run `43164795419` on `b18ce5e`). The deadline covers reading the outputs; each output holds at most
16 MiB (`OutputTooLarge`) and counts against the heap limit (docs/runtime.md 9.5). Reads whose
size the program decides (`read_bytes(max_bytes)`, whole files, `Text.repeat`) check the heap
limit before they allocate, so a refused allocation ends with 102 instead of an abort
(docs/runtime.md 3.4, 3.5).

Before `main`, before each test, and in an executable, the runtime sets aside a reserve of 1 MiB
for `release` (C-17, docs/runtime.md 3.6–3.7): allocated, its pages written, and counted within
`limits.heap_bytes`, so a program may use the limit minus 1 MiB, and a policy whose limit is
below 2 MiB is refused with code 103. The reserve opens only when a fault is created, before its
message and trace are allocated; then the allocator of the runtime
(`unio-runtime/src/reserve.rs`, the global allocator of every build) serves the fault, its trace,
and the `release` calls of the unwinding from the reserved pages, so they run on a full heap. A
`release` that goes beyond the reserve ends with 102 and skips the remaining releases, with or
without a heap limit and whatever room the heap has; exhaustion never opens it. The test hook `UNIO_TEST_HEAP_FREE=N` counts the heap as
full but for `N` bytes before `main`, so tests reach a fault on a full heap exactly.

## What is implemented

- The lexer of grammar section L: INDENT, NEWLINE, and DEDENT, implicit continuation inside
  brackets, comments and documentation comments kept as trivia with spans, keywords and
  contextual keywords, the reserved word `uses`, numbers, and text with recursive interpolation.
  It reports E0001–E0003, E0005–E0011, and E0032 on the same lines as `spec/check_grammar.py`,
  and continues after an error.
- The parser of grammar sections F, S, E, P, and T, including `named` parameters and function
  shapes (RFC 0001), the pattern test `is`, lambda parameters without types, and one-line
  branches (RFC 0002). It follows the ordered choices of the EBNF, implements the generic-call
  rule, and after an error skips to the end of the line and over the block that follows, so that
  one file reports several errors (C-2).
- The syntactic constraints that the EBNF states in comments: chained comparisons (E0020), a
  positional argument after a named one (E0023), an inline handler without a block or not at the
  end of its statement (E0024), an invalid assignment target (E0030), and a rest element that is
  not last (E0034).
- Diagnostics in the text form of C-7 and the JSON form of C-8, in English and Russian (C-10),
  with a fix where one is obvious (a tab replaced by spaces, a missing ':').
- The diagnostic catalog of R-12 in `src/catalog.rs`: every message, explanation, example of the
  error, and fixed example; the review checklist is [DIAGNOSTICS.md](DIAGNOSTICS.md).
- The formatter of `src/format.rs`: layout, blank lines, sorted imports, spaces, line breaking at
  the outermost group that ends last, one-line branches (T-1a), comments placed by their source
  lines, literals and parentheses as written. Every `.unio` source of the repository is in this
  style, and CI checks it with `unio fmt --check`.
- The outline of `src/outline.rs` in text and JSON.

Diagnostic codes added by the front end:

| Code | Meaning |
|---|---|
| E0012 | Unexpected token: the message names what the grammar expects at that place |
| E0013 | Code nested too deeply (expressions, patterns, types, blocks, or texts inside interpolations) |

Lexical diagnostics come first, in the order they are found, followed by syntax diagnostics in
source order. A syntax error on a line next to a lexical error is not reported, because it is
nearly always a consequence of it.

## Name and type checks (sprint S3)

`src/program.rs` loads a program: the given files, the modules they import, and the standard
library. The standard library is chosen by its origin only (RFC 0004, `trusted_stdlib`): the
directory `UNIO_STDLIB` set by the operator; else the `stdlib/` of the checkout when the compiler
runs from its `compiler/target/` (so that `unio test stdlib/...` and edits of the library work
without a rebuild); else the copy embedded in the compiler by `build.rs`, which an installed
`unio` uses and whose modules have paths below `<unio>/stdlib`, read from the compiler's memory
and never from a disk (`src/vfs.rs`). The compiler records no path of its build. A `stdlib`
directory of a package is never used. A package is the directory with
`unio.toml`, otherwise the directory of the file; `import a.b` loads `a/b.unio` from the package
root and `import std.x` loads `stdlib/<layer>/x.unio`. The `prelude.unio` and `builtin/` files of
the three layers form the prelude of the `application` profile. It then collects declarations,
resolves imports and cycles, the types of signatures, fields and variants, and checks every
`extend ... with` against its interface.

`src/check.rs` checks the bodies of functions, interface defaults, constants, and tests:
scopes of local names, types with local inference and expected types, calls with positional,
named and default arguments and RFC 0001 labels, generic functions with inferred arguments and
constraints, methods through `extend` blocks, interfaces and defaults, functions of a type,
struct and variant construction, operators through interfaces, `or` on `Option`, `?.`, indexing,
`in`, interpolation, lambdas typed from the expected function type, patterns, and `is`.

Codes reported: E0040, E0041, E0044 to E0050, E0051 to E0067, E0069, E0071, E0072, E0086, E0088, E0089. The
codes of RFC 0001 have their own cases: a key, a bound, a bounded method or `<` on a type built
from IEEE `Float` is E0045 (semantics 2.9, 3.3, 4.2); a `std.json` conversion of a type with a
private field, also through a generic function that passes its parameter on, is E0046 (3.8); a
`Range` used as a `List` is E0047 with the fix `.to_list()` (4.9); an import under `deps.` is
E0048 (1.9); an `external function` outside an `unsafe module` of the standard library is E0086
(8.5); a mistake in the manifest `unio.toml` of a reported package is E0088 at its line of
the manifest, checked by `src/package/manifest.rs` (`check`) and reported as a file of its own
by `unio check` and the language server (8.6); `unsafe_modules` that does not name exactly
the modules marked `unsafe module` is E0089 at its line (`src/program.rs`,
`package_manifest_diagnostics`). Where a
type cannot be determined, the checker uses an unknown type that is compatible with every type,
so that one mistake is reported once. The arguments of generic interfaces are compared in
implementation checks, constraints and assignability (`Iterator[Int]` is not `Iterator[Text]`).

Two readings of docs/semantics.md were chosen where the document is silent: the public error
types of a module imported with `import a.b` are visible by their short names (`on IoError` after
`import std.files`, as the reference programs and concept section 3.2 write them), and a variant
declared or imported in a module hides a prelude name of the same spelling for rule L-6a.

## Errors, values, resources, layers and effects (sprint S4)

`src/check.rs` also follows errors, places and control flow (docs/semantics.md, sections 5–7):

- Errors: a call that can fail needs `try` or an enclosing `try:` (E0073); every error that
  leaves a function is handled or listed in `fails with`, with a fix that adds it (E0074); an
  inline handler must not complete (E0075); `try` before something that cannot fail is E0076.
  The `fails with` of function types is compared: a callback that fails with more than the
  expected type allows is E0061.
- Values: assignment to an immutable name (E0077); a change through a place that does not start
  at a `var` name, a `var` parameter or a `using` name, of a field not declared `var`, or a
  `var self` method on such a place (E0078); `var` arguments (E0079, E0042).
- Control flow: a reachable end of a function with a result (E0082), `break` and `continue`
  outside a loop (E0083), `match` exhaustiveness over enums, `Option`, `Result`, `Bool`, tuples
  and list patterns, with the missing cases named in the message (E0068).
- Results: a dropped `Result` or `std.process.Output`, or a local holding one that is never
  read, is E0043 (L-30); `let _ = ...` drops it on purpose. Other results may be dropped.
- Resources: created only as a `using` initializer, the return of a function that returns a
  resource, or a field of another resource (E0080); a `using` name must not leave its block,
  directly or captured by a lambda (E0081). A parameter of a resource type may be used, compared,
  and passed on to a parameter of a resource type or of a generic type bounded by `Resource`
  (`is_same`), but storing, returning or capturing it is E0081 too (semantics 7.2).
- Coverage of `match` is searched with a budget of 10,000 pattern rows per `match`, and the
  alternatives (`|`) of one tuple or list pattern expand to at most 256 rows; a column in which
  no case names a constructor is skipped in one step. A `match` that exceeds either limit and has
  no `case _` is not taken as covered: E0068 says that its patterns combine into more cases than
  the checker enumerates (semantics 6.5). Before S5, such a pattern counted as matching anything.

`src/effects.rs` derives effects and layers from the call graph that the checks record (section
8): an external function of `std.env`, `std.process` or `std.net` has the effect of its module,
an external of `std.files` has the scopes `files.read` and `files.write` that the "Permission:"
sentence of its documentation names ("Permission: none" for an operation on a descriptor that is
already open; a missing sentence counts as both scopes, and a test requires the sentence on every
external of `std.files`), every function of the `alloc` layer has `allocates`, and a function has
the union of what it calls. The derivation is conservative (semantics 8.4): a call through an
interface method, written or implicit (operators, `==`, comparisons, interpolation, `for`, the end
of `using`, `in`), has the union of every implementation of the method in the program, or the
method of the value's type when it is known; the effects of a function value are carried by the
calls of the value (RFC 0005): a lambda's body is a holder of its own and naming a function as a
value has no effect, a call through a value has the effects of the values a flow analysis finds
for it (named functions, lambdas, locals, constants, the values stored in a field, the returns of
a named function), a call through a parameter is a latent effect replaced at each call by the
argument (`unio symbol` shows it as `argument effects`), and any other value has every function
value of the program of a matching type, which under `--profile=core` or in a `core` module is
E0087 when one of them needs more than core; the sets are complete only for the closed program
checked (the sources `unio check` reads and the standard library), and a function value from
outside it is never treated as effect-free, a rule that stays when packages arrive (S7); a
constant counts for the functions
that use it, a parameter default for its function, and field defaults for the functions that
build the type; list, map and comprehension literals, interpolation, `+` on texts and lambdas that
capture values give `allocates` and the layer `alloc`. A library module that calls a higher
layer, or a `core` module that allocates, is E0084; `--profile=core` applies the rule to the
functions of a package, with the chain of calls down to the call or the allocating expression. A
call that needs an effect the `[permissions]` table of the package's `unio.toml` does not grant
is E0085, once per effect and module, with the chain; `files.read` and `files.write` are checked
separately against the keys of the same names (T-6), and `env`, `process` and `network` grant
themselves. A package without the table, or without a manifest, grants nothing (SB-2, SB-7); a
manifest with a mistake (E0088) is not compared.

`src/symbol.rs` and `src/sha.rs` compute the content hashes of C-11a and C-11b: SHA-384, with
the scheme version `unio-hash-v1` and a domain for each kind of input, over the S-expression of
the declaration (no spans, comments or layout) and the hashes of the declarations it names or
calls. Declarations that reference each other form one strongly connected component: its members
are sorted by identity, references inside it are written as identities and references outside it
as their hashes, and each member's hash binds the component hash and the member's identity.
Formatting, comments and the order of declarations do not change a hash; a change of the body,
of a name, or of anything referenced does.

## The interpreter (sprint S5)

`src/interp.rs` executes a checked program; `unio-runtime` supplies the values, faults, the
release stack and the 115 externals. The interpreter walks the checked syntax tree with the
program tables of the checker (resolved declarations, methods by type, signatures); runtime 8.1
describes a lowered representation with slots and explicit error edges, which is left to the
native runtime of S6, because the tree walk already gives the evaluation order of runtime 1 and
is the reference for it.

- Evaluation is strict and left to right (runtime 1.1), including assignment: the target place
  and its indices are resolved before the right-hand side. Defaults of parameters are evaluated
  at each call that omits them, in the module of the function and after the preceding
  arguments; externals get their defaults the same way.
- Values come from `unio_runtime::value`: reference counting with copy-on-write, so changing an
  unshared value happens in place. `var` arguments and `var self` receivers are taken out of
  their place for the call and written back afterwards, also when the call fails. A resource is a
  shared cell: its methods see one value, `release` runs once.
- Calls: functions and methods through the tables of the checker, interface values and default
  methods of interfaces, functions of a type (`T.f`), and lambdas as closures that capture the
  values of the names they use; named functions and closures implement `unio_runtime::Callable`,
  so that externals such as `expect_fault` can call them.
- Types: a value of a declared type carries the identity of its type, its name for a type of the
  standard library (whose names are unique there and which the externals use) and `module.Name`
  for a type of a package, so two types of one name in different modules keep their own methods,
  implementations, handlers and variants; the method table, the per-call-site method caches and
  the lowered handlers and patterns use the identity, in the interpreter and in native code.
  Interpolation and the generated `to_text` show the short name.
- Errors travel as `Unwind::Error` to the nearest `try ... on` or `try:` handler that names their
  type or `Error`; faults (`Unwind::Fault`) carry the trace of function, file and line of every
  frame (C-14), run the `release` of every open `using` block on the way out (C-17), from the
  reserve set aside before `main`, and end `unio run` with code 101, the diagnostic written line
  by line; exhaustion of the stack, or of the heap limit when a policy sets one, ends it with
  102 without running `release` (C-18). An
  exit code of `main` outside 0–100 is a fault (C-15).
- Top-level constants are evaluated on first use, once, which is dependency order (runtime 6.3).
- Externals are called by their binding-table names (`Text.slice`, `std.files.system_rename`,
  bare names for the prelude); `std.json.to_value` and `std.json.from_value` are generated from
  the static type: structs, enums, options, lists, maps and sets, with the strict decoding of
  S-17 (unknown fields and variants, missing required fields, `Float` for `Int` are
  `DecodeError` with a JSON Pointer path; defaults are fresh).
- `unio run` and `unio test` run the program on a thread with a 1 GiB stack, so that the frame
  limit of the runtime, not the native stack, decides exhaustion.

Decisions made where docs/runtime.md is silent:

- `try e` unwraps a `Result` value only when `e` is not a call of a function with `fails with`;
  `a or b` keeps an `Option` when `b` is `none` or `some(...)`, and gives the inner value
  otherwise, following semantics 4.3.
- Iterating a `Map` yields its keys in insertion order; iterating a `Text` yields its characters
  as one-character texts; `Range` iteration needs no allocation.
- The generated `to_text` of an error is `Name(field = value, ...)`, with each field as its own
  `to_text` writes it (texts without quotes).
- A `match` without a matching case cannot happen after E0068, so it is an internal fault.

Speed, measured on the reference tools in a release build on one core: the whole repository's
113 test blocks run in 0.9 s; `tools/check_grammar.unio --corpus` takes 3.7 s, the parse check of
every source 30 s, and `tools/check_api.unio` 29 s, against 0.1 s, 0.2 s and 1.4 s for the Python
checkers. A loop of a million iterations and 250,000 recursive calls takes 0.7 s. The binding
table is built once per process, frames hold shared names, and method lookup does not allocate;
the remaining cost is the tree walk and allocation of environments, which S6 replaces.

## Native code (sprint S6)

`src/native/` compiles the lowered form of `src/lower.rs` (docs/runtime.md 8.4):

- `units.rs` lists the bodies of a program (functions, constants, defaults, lambdas) in one
  order and the lowered statements, nodes, patterns and literals each passes to the runtime, in
  one canonical walk that the build and the executable share.
- `image.rs` writes and reads the program image: a versioned binary format (a mark, the format
  version, the identity of the compiler from `build.rs`, a hash of the sources of both crates)
  with the declarations the run needs, the lowered bodies, the declared permissions, and the
  diagnostic metadata of traces (the package-relative file of each module, the line of each
  function); every read is bounds-checked, and an image this runtime cannot run ends the
  executable with code 104.
- `codegen.rs` translates every body with Cranelift: locals of type `Int`, `Float` and `Bool`
  unboxed in registers with inline overflow and zero checks (left out only where the bounds of
  a counted loop or of a tested parameter prove the result in range), direct calls with scalars
  and the depth of the call stack in registers, a frame pushed with one store, the current line
  stored only before a call that can read it, leading `if c: return v` clauses of a callee
  decided at the call, temporaries
  on the stack released on every path out, and unwinding by status to `try`, `on` and `using`.
  It refuses a unit rather than generate doubtful code: a construct it cannot compile yet, or an
  internal inconsistency it detects, stops the build with the place.
- `support.rs` is the runtime side: the context of native code, the functions it calls for values
  that own memory (methods, fields, construction, patterns, iteration, places), the hooks
  through which the interpreter's dispatch and the constants and defaults enter native code, and
  `unio_native_main`, the start of an executable.
- `mod.rs` loads, checks, lowers, writes the image, compiles for the target of the machine
  (`Target`: the Cranelift triple, the object format, the name of the runtime library), and
  gives the linker and its arguments for that system. Its unit test generates the objects of
  reference programs for all four targets on any machine.

The executable reads its image, builds the interpreter's tables from the declarations
(`Interp::prepared`, without the front end, so the parser, the checker and the lowering are not
linked into it), installs the lowered bodies and their native code, fills the reference tables
of its bodies, applies the sandbox, and runs `main`; the interpreter then only dispatches and
never evaluates lowered code. An empty program starts in about 2.6 ms, against 7 ms when the
executable rebuilt its tables from embedded sources. Timings and memory against Python, Go and
Rust are in [bench/README.md](../bench/README.md).

## Tests

The tests were written before the code (concept, section 1.2):

- `tests/corpus.rs`: every file in `spec/tests/valid` parses without diagnostics; every file in
  `spec/tests/invalid` reports its first error on the line, and with the code, in its
  `# expect-error:` comment.
- `tests/sources.rs`: every `.unio` file in `docs/examples`, `docs/ru/examples`, `examples`,
  `stdlib`, and `tools` parses; the AST dump is stable.
- `tests/diagnostics.rs`: several errors in one run, the text and JSON formats, both languages,
  and the syntactic constraint codes.
- `tests/fuzz.rs`: deterministic mutations of the corpus and the sources never make the front end
  panic, and extreme nesting gives E0013 instead of a stack overflow (R-1).
- `tests/format.rs`: a golden test for each rule of docs/style.md; for every valid corpus file
  and every repository source, the formatted text parses, has the same tree with imports sorted
  (T-2), formats to itself (T-1), and keeps every comment in order; a generated 1,000-line source
  formats within 50 ms in a release build (`cargo test --release --test format thousand`, T-3;
  a debug build is allowed 1 s).
- `tests/outline.rs`: the text and JSON forms of the outline.
- `tests/catalog.rs`: every code has an explanation and a pair of examples that the front end
  confirms, the semantic codes through the whole check; messages follow the template and name no
  compiler internals.
- `tests/repl.rs`, `tests/doctest.rs`, `tests/property.rs`, `tests/api_diff.rs`, `tests/migrate.rs`: `unio repl` (checks, bindings by value, the sandbox and the policy, fixes by revision), documentation tests, property replay and `--seed`, `unio api-diff` with callers and migrations, and `unio fmt --migrate` on the catalog examples of every registered migration ([docs/tools.md](../docs/tools.md))
- `tests/symbol.rs`: `unio symbol` in text and JSON, content hashes that ignore layout, comments
  and order and follow changes through references and mutual recursion, the core profile and
  the permissions of the manifest, allocating expressions under the core profile and in a
  `core` module of the library, and effects that reach a manifest through an interface method
  and through a function value kept in a constant or a parameter default (semantics 8.4), and
  the rules of RFC 0005: a stored value has no effect, a parameter carries the effects of the
  argument to the caller, the library's higher-order functions do the same, a call through a
  field has the stored values and is E0087 under the core profile, a returned lambda counts where
  it is called.
- `tests/semantic.rs`: the semantic corpus in `spec/tests/semantic` (one focused file per code in
  `invalid/`, with `# expect-error: LINE CODE`, and programs in `valid/` that must be clean), the
  exit criterion of S3 and S4 that every `.unio` source of the repository passes `unio check`, and
  random renaming of names in real programs that must never make the checker panic (R-1). The
  semantic corpus is checked by the compiler only; `spec/check_grammar.py` checks syntax.
- `tests/interpreter.rs`: every `test` block of `stdlib`, `examples`, `docs/examples`,
  `docs/ru/examples` and `tools` runs and passes under `unio test` (the exit criterion of S5);
  failing and non-compiling tests and the filter; the reference utilities `wc`, `grep_lite`,
  `json_query`, `csv_sum`, `log_tail`, `rename --dry-run` and `todo` (with `TODO_FILE` in a
  temporary directory) end to end with their output and exit codes; a fault that releases two
  `using` blocks in reverse order and ends with 101 and its trace, exit codes of `main` inside and
  outside 0–100, and recursion that ends with 102; two modules with a struct, an error and an
  enum of the same names (`tests/programs/same_names`), whose methods, interface
  implementations, handlers and variants stay apart under `unio run` and `unio test`; the
  reserve for `release` (C-17): on a heap filled by the test hook, a fault releases an
  `open_atomic` file and a resource that builds 2,000 texts, with the whole trace and no 102; a
  fault inside a `release` still releases the rest; a `release` that exhausts the reserve ends
  with 102 and leaves the temporary file; a real fill under 8 MiB runs no `release`; the same in
  `unio test` with `expect_fault`. The runtime unit tests check that the reserve is set aside
  before `main`, opens only for a fault and before the fault is built, and serves the path from
  its pages; `the_reserve_peak_is_measured` (`unio/src/interp.rs`) measures its peak use.
- `tests/sandbox.rs` (SB-8): programs that try forbidden actions through `std.files`, `std.env`
  and `std.process` (reading, listing, resolving, writing, copying, renaming, creating outside
  the scopes, `..` and links out of a scope, hidden variables, unlisted programs), with no
  permissions at all, and through child processes that read, write, connect and start
  grandchildren (Linux and macOS); a policy that narrows and warns, through `--policy` and
  `UNIO_POLICY`; the limits of heap, wall and CPU time (102), processes and file size; code 103
  for an unreadable or invalid policy or manifest and for a heap limit below 2 MiB; tests that run in their package's sandbox; a
  child that sees only the granted environment; a read whose `max_bytes` exceeds the heap limit,
  which ends with 102; and `list_tree` over a cycle of links (Unix). `unio-runtime/tests` cover
  the process tree (a grandchild that holds the output, a timeout that stops the grandchild),
  output over 16 MiB, captured output over the heap limit, renaming that never replaces a file
  or a directory, and reads and `Text.repeat` over the heap limit.
  `unio-runtime` has unit tests for the TOML subset, scopes, narrowing, the runtime checks,
  Landlock and seccomp on a thread, and Seatbelt in a child process.
- `tests/native.rs` (Linux on x86-64 and AArch64, macOS on AArch64, Windows on x86-64; skipped
  with a note elsewhere): every reference program that the interpreter tests run, with the same
  inputs, is built with `unio build --release` and
  gives the same standard output, standard error and exit code as `unio run`: the utilities,
  the task list, faults of every kind with their traces, exit codes, exhaustion of the stack, the
  benchmarks, the four tools over the repository, and the types of one name in two modules of
  `tests/programs/same_names`; two builds give identical bytes, also of one project
  copied into two directories and built with another home and temporary directory; traces name
  files relative to their package, natively and in `unio run`; a program with errors is not
  built; the executable applies the manifest of its package and, moved elsewhere without its
  project, the same permissions, while a manifest with a relative entry is refused; an image of
  another format, of another compiler, or damaged is refused with code 104; on a heap filled
  by the test hook, the executable releases from the reserve after a fault, as `unio run` does,
  and runs no `release` on exhaustion (C-17, C-18).
- `tests/tools.rs` (ignored by default, run in CI in a release build): `tools/check_docs.unio`,
  `tools/check_grammar.unio` (corpus, self-check, sources, invalid sources, parse trees) and
  `tools/check_api.unio` under `unio run` give the same output and exit code as the Python
  checkers.

## Not done yet

- Known limits of the effects: a call through an interface counts every implementation in the
  program when the type of the value is not known, not only those whose values can reach it. A
  call through a function value is exact for named functions, lambdas, locals, constants, returns
  and parameters (RFC 0005), but estimated elsewhere: a field has every value stored in it in the
  whole program; an element of a collection, a pattern binding, a lambda parameter and the result
  of a call through a value have every function value of a matching type (parameter count and
  types; results and errors are not compared); a parameter that a function keeps has every
  argument passed for it anywhere; a lambda returned from a function does not keep the arguments
  of that call. These estimates can reject a call no run makes: under `--profile=core` it is
  E0087, and in a package it can be E0085 (semantics 8.4); usages of types are
  listed per referencing declaration. Defaults of parameters and fields are checked like any
  expression (names and types, E0056 or E0061).
- Known limits of `process.run` (docs/runtime.md 9.5): on Linux a program that needs `setsid`
  or `setpgid` in a child fails there, and without seccomp (the weaker sandbox) a process that
  leaves the group outlives the call; on macOS a descendant that left the group after its parent
  ended is found only by the marker of the profile, so not without a sandbox; the macOS and
  Windows trees are checked on those systems in CI only (they passed on `macos-latest` and
  `windows-latest`, run `43164795419` on `b18ce5e`), and the Windows fail-closed path has no test; on Unix an interrupt from the terminal does not reach a
  child in its own group (the workers of the tools are stopped with their trees). Heap exhaustion that the
  system reports late (overcommit and its out-of-memory killer) ends the process without 102;
  outside the path of a fault, which the reserve serves, a small allocation the system refuses
  inside the Rust standard library aborts.
- Known limits of the RFC 0001 checks: a struct with a `Float` field used as a key is rejected
  at the calls that need `Hash` (E0062, E0056), not at the type; E0048 covers a dependency that
  is not declared, not locked, not installed, or has no such module; conflicts of versions are
  reported by `unio add` and `unio update`, not by the compiler (T-14, docs/packages.md). E0046
  is static; at run time decoding a struct with a private field also fails with `DecodeError`,
  while encoding has no such second guard.
- Known limits of the language server (T-10): rename does not follow the implementations of an
  interface method, and a definition in the standard library embedded in the compiler has no
  file to open; a module is renamed with its file, not by the server. Completion and hover in a
  document with syntax errors blank the lines of the errors first, so names on those lines are
  not known; rename needs every module of the package to parse.
- Known limits of the workers of the tools (docs/runtime.md 9.7): on Windows the tool creates
  each worker in an AppContainer of its own with grants on exactly its scopes and no network,
  and the worker joins a Job Object (memory, CPU, one active process); a worker that is not in
  an AppContainer stops with 103 whatever the operator allows; `stat` is not restricted
  anywhere; the link step of `build` runs the system linker, whose memory depends on
  the toolchain and on `libunio.a` (about 130 MiB of address space with the library of a release
  build, about 530 MiB with the 290 MB library of a debug build, on Linux x86-64), so a
  `compile.memory_bytes` above the minimum can still be too small for it; the tool still reads the manifest and the policy itself for the sandboxes
  of `run` and `test`; the language server (SB-13), the package operations (SB-12) and the
  archives of SB-15 do not exist yet.
- The rest of S6 (cross-compilation, `--target`, docs/runtime.md 8.6), and the other
  commands of requirement 6.1. Packages, signatures, the language server, the R-10 control set
  and the cryptography policy are requirements only; none is implemented
  ([docs/status.md](../docs/status.md)).
