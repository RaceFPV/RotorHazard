#!/bin/bash
# Builds release binaries into dist/ as plain executables — no archives.
#   dist/rh-server-linux-x86_64
#   dist/rh-server-linux-aarch64   (when the aarch64 target is installed)
# The binary embeds all assets (static, templates, language.json) and
# resolves its own data directory (RH_DATA_DIR env, datapath.ini,
# exe-dir data/, ~/rh-data), so a single file is the whole install.
#
# Usage: ./rust/build-release.sh
set -eu

cd "$(dirname "$0")/.."   # repo root

# Linuxbrew's pkg-config (if on PATH) shadows the system one but does not
# know the Debian/Ubuntu system .pc directories; make sure they are searched
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig${PKG_CONFIG_PATH:+:${PKG_CONFIG_PATH}}"

mkdir -p dist

echo "== building x86_64 release binary =="
cargo build --release --manifest-path rust/server/Cargo.toml
cp rust/target/release/rh-server dist/rh-server-linux-x86_64
echo "Built: dist/rh-server-linux-x86_64"

# Cross-compiled aarch64 (Pi) binary when the target toolchain is
# installed. Otherwise build natively on the Pi with this same script.
if rustup target list --installed 2>/dev/null | grep -q '^aarch64-unknown-linux-gnu$'; then
    echo "== building aarch64 (Raspberry Pi) binary =="
    cargo build --release --manifest-path rust/server/Cargo.toml --target aarch64-unknown-linux-gnu
    cp rust/target/aarch64-unknown-linux-gnu/release/rh-server dist/rh-server-linux-aarch64
    echo "Built: dist/rh-server-linux-aarch64"
else
    echo "(aarch64 target not installed; skipping Pi binary)"
fi
