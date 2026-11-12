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
set -eu
[ $# -eq 1 ] || { echo "usage: release_build.sh PREFIX" >&2; exit 2; }
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$1"
prefix=$(cd "$1" && pwd)
cargo_home=${CARGO_HOME:-$HOME/.cargo}
cargo_home=$(cd "$cargo_home" && pwd)
if command -v cygpath >/dev/null 2>&1; then
    # Git Bash on Windows: rustc sees the paths in the form of Windows.
    root_seen=$(cygpath -w "$root")
    cargo_home_seen=$(cygpath -w "$cargo_home")
else
    root_seen=$root
    cargo_home_seen=$cargo_home
fi
cd "$root/compiler"
RUSTFLAGS="--remap-path-prefix=$root_seen=/unio --remap-path-prefix=$cargo_home_seen=/cargo" \
    cargo build --release --locked -p unio
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) exe=unio.exe; lib=unio.lib ;;
    *) exe=unio; lib=libunio.a ;;
esac
# A CARGO_TARGET_DIR outside the checkout is not remapped: keep the default for the release.
target=${CARGO_TARGET_DIR:-target}
mkdir -p "$prefix/bin" "$prefix/lib/unio" "$prefix/share/doc/unio"
cp "$target/release/$exe" "$prefix/bin/$exe"
cp "$target/release/$lib" "$prefix/lib/unio/$lib"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$root/THIRD-PARTY-NOTICES" "$prefix/share/doc/unio/"
echo "installed into $prefix"
