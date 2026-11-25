#!/bin/bash
# Builds the release binary and packs a single-file distribution.
# The rh-server binary embeds all assets (static, templates, language.json)
# and resolves its own data directory (RH_DATA_DIR env, datapath.ini,
# exe-dir data/, ~/rh-data), so no launcher script or payload files are
# needed — the tarball contains only the binary.
#
# Usage: ./rust/build-release.sh [version-tag]
set -eu

cd "$(dirname "$0")/.."   # repo root

# Linuxbrew's pkg-config (if on PATH) shadows the system one but does not
# know the Debian/Ubuntu system .pc directories; make sure they are searched
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig${PKG_CONFIG_PATH:+:${PKG_CONFIG_PATH}}"

VERSION_TAG="${1:-$(git describe --tags --always 2>/dev/null || echo dev)}"

echo "== building release binary =="
cargo build --release --manifest-path rust/server/Cargo.toml

echo "== packing x86_64 =="
OUT="dist/rh-server-rust-${VERSION_TAG}"
rm -rf "$OUT" "dist/rh-server-rust-${VERSION_TAG}.tar.gz"
mkdir -p "$OUT"
cp rust/target/release/rh-server "$OUT/"
tar -czf "dist/rh-server-rust-${VERSION_TAG}.tar.gz" -C dist "rh-server-rust-${VERSION_TAG}"
echo "Bundle: dist/rh-server-rust-${VERSION_TAG}.tar.gz"
du -sh "dist/rh-server-rust-${VERSION_TAG}.tar.gz"

# Cross-compiled aarch64 (Pi) bundle when the target toolchain is
# installed. Otherwise build natively on the Pi with this same script.
if rustup target list --installed 2>/dev/null | grep -q '^aarch64-unknown-linux-gnu$'; then
    echo "== building aarch64 (Raspberry Pi) binary =="
    cargo build --release --manifest-path rust/server/Cargo.toml --target aarch64-unknown-linux-gnu
    PI_OUT="dist/rh-server-rust-${VERSION_TAG}-aarch64"
    rm -rf "$PI_OUT" "dist/rh-server-rust-${VERSION_TAG}-aarch64.tar.gz"
    mkdir -p "$PI_OUT"
    cp rust/target/aarch64-unknown-linux-gnu/release/rh-server "$PI_OUT/"
    tar -czf "dist/rh-server-rust-${VERSION_TAG}-aarch64.tar.gz" -C dist "rh-server-rust-${VERSION_TAG}-aarch64"
    echo "Bundle: dist/rh-server-rust-${VERSION_TAG}-aarch64.tar.gz"
else
    echo "(aarch64 target not installed; skipping Pi bundle)"
fi
