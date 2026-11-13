#!/bin/sh
# Builds the coverage-guided fuzzer of R-1 (fuzz/main.rs) with the coverage instrumentation of
# LLVM's SanitizerCoverage, which the stable toolchain of compiler/rust-toolchain.toml runs:
# inline 8-bit counters on every edge of the crates of the workspace and their dependencies.
# The build has debug assertions and overflow checks on, so that the fuzzer also finds what
# they catch, and no LTO, which would run the instrumentation twice. It prints the path of the
# executable.
#
# Usage, from anywhere: sh compiler/unio/fuzz/build.sh
#
# --target names the host: with it, RUSTFLAGS reach only the code of the target, not the build
# scripts, which would not link against the counters. The build goes to target/fuzz, apart
# from the other builds.
set -eu
compiler=$(cd "$(dirname "$0")/../.." && pwd)
cd "$compiler"
host=$(rustc -vV | sed -n 's/^host: //p')
RUSTFLAGS="-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=3 -Cllvm-args=-sanitizer-coverage-inline-8bit-counters" \
CARGO_PROFILE_RELEASE_LTO=false \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true \
CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true \
CARGO_TARGET_DIR="$compiler/target/fuzz" \
    cargo build --release --locked -p unio --example fuzz --target "$host" >&2
exe="$compiler/target/fuzz/$host/release/examples/fuzz"
[ -x "$exe" ] || exe="$exe.exe"
echo "$exe"
