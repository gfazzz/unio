#!/bin/sh
# Installs the Rust toolchain that compiler/rust-toolchain.toml pins, with its components, and
# checks that cargo in compiler/ uses exactly that version (plan section 12, release
# condition 3). rustup finds the toolchain file from the working directory, so every cargo
# command of CI runs inside compiler/.
set -eu
cd "$(dirname "$0")/../../compiler"
channel=$(tr -d '\r' < rust-toolchain.toml | sed -n 's/^channel = "\(.*\)"$/\1/p')
# rustup 1.28 installs the toolchain of the file with `toolchain install`; older ones on `show`.
rustup toolchain install 2>/dev/null || rustup show
version=$(rustc --version)
echo "$version"
cargo --version
case "$version" in
    "rustc $channel "*) ;;
    *) echo "compiler/rust-toolchain.toml pins $channel, and cargo uses $version" >&2; exit 1 ;;
esac
