#!/bin/sh
# The release build of unio and its installed layout (plan section 12, release conditions 1 to
# 3; docs/runtime.md 8.4):
#
#     PREFIX/bin/unio                  the compiler, with the standard library embedded
#     PREFIX/lib/unio/libunio.a        the runtime that `unio build` links (unio.lib on Windows)
#     PREFIX/share/doc/unio/           LICENSE-MIT, LICENSE-APACHE, THIRD-PARTY-NOTICES
#
# Usage, from anywhere: sh .github/scripts/release_build.sh PREFIX
#
# The toolchain is the one compiler/rust-toolchain.toml pins and the crates those of
# Cargo.lock (--locked). The paths of the checkout and of CARGO_HOME, which reach the binary
# through the panic locations of the crates, are remapped to /unio and /cargo, so that a build
# in another directory or by another user gives the same bytes (C-4, R-6).
#
# sh .github/scripts/release_build.sh --print-rustflags [SYSTEM] prints the RUSTFLAGS the build
# would use on SYSTEM (by default `uname -s`, e.g. MINGW64_NT-10.0 for Windows) and builds
# nothing; CI checks with it that /Brepro reaches the link on Windows.
set -eu
print_flags=no
system=$(uname -s)
if [ $# -ge 1 ] && [ "$1" = --print-rustflags ]; then
    [ $# -le 2 ] || { echo "usage: release_build.sh --print-rustflags [SYSTEM]" >&2; exit 2; }
    print_flags=yes
    system=${2:-$system}
else
    [ $# -eq 1 ] || { echo "usage: release_build.sh PREFIX" >&2; exit 2; }
    mkdir -p "$1"
    prefix=$(cd "$1" && pwd)
fi
root=$(cd "$(dirname "$0")/../.." && pwd)
cargo_home=${CARGO_HOME:-$HOME/.cargo}
if [ "$print_flags" = no ]; then
    # The runner of macOS keeps rustup elsewhere and has no ~/.cargo before the first build.
    mkdir -p "$cargo_home"
    cargo_home=$(cd "$cargo_home" && pwd)
fi
if command -v cygpath >/dev/null 2>&1; then
    # Git Bash on Windows: rustc sees the paths in the form of Windows.
    root_seen=$(cygpath -w "$root")
    cargo_home_seen=$(cygpath -w "$cargo_home")
    # Cargo joins the paths of the registry to CARGO_HOME as it is given: give it the form
    # that the remapping below names, not a mix of / and \ that the prefix would not match.
    CARGO_HOME=$cargo_home_seen
    export CARGO_HOME
else
    root_seen=$root
    cargo_home_seen=$cargo_home
fi
flags="--remap-path-prefix=$root_seen=/unio --remap-path-prefix=$cargo_home_seen=/cargo"
case "$system" in
    MINGW* | MSYS* | CYGWIN*)
        exe=unio.exe; lib=unio.lib
        # link.exe writes the link time into the PE header and a new signature of the PDB into
        # the debug directory unless it is told /Brepro, which puts a hash of the content in
        # both; the executable names its PDB by file name only (/PDBALTPATH:%_PDB%, which
        # rustc passes as well), and the PDB stays in target/ and is not part of the release.
        # The options are spelled with - so that Git Bash does not take them for paths.
        flags="$flags -C link-arg=-Brepro -C link-arg=-PDBALTPATH:%_PDB%"
        ;;
    *) exe=unio; lib=libunio.a ;;
esac
if [ "$print_flags" = yes ]; then
    printf '%s\n' "$flags"
    exit 0
fi
cd "$root/compiler"
RUSTFLAGS=$flags cargo build --release --locked -p unio
# A CARGO_TARGET_DIR outside the checkout is not remapped: keep the default for the release.
target=${CARGO_TARGET_DIR:-target}
mkdir -p "$prefix/bin" "$prefix/lib/unio" "$prefix/share/doc/unio"
cp "$target/release/$exe" "$prefix/bin/$exe"
cp "$target/release/$lib" "$prefix/lib/unio/$lib"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/THIRD-PARTY-NOTICES" "$prefix/share/doc/unio/"
echo "installed into $prefix"
