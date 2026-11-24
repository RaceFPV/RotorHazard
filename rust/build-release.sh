#!/bin/bash
# Builds the release binary and assembles a distributable bundle under
# dist/ with everything the server needs at runtime (same layout rule as
# the Python install: assets live next to the program).
#
# Usage: ./rust/build-release.sh [version-tag]
set -eu

cd "$(dirname "$0")/.."   # repo root

VERSION_TAG="${1:-$(git describe --tags --always 2>/dev/null || echo dev)}"
OUT="dist/rh-server-rust-${VERSION_TAG}"

echo "== building release binary =="
cargo build --release --manifest-path rust/server/Cargo.toml

echo "== assembling ${OUT} =="
rm -rf "$OUT"
mkdir -p "$OUT"
cp rust/target/release/rh-server "$OUT/"
cp LICENSE "$OUT/" 2>/dev/null || true

cat > "$OUT/run.sh" <<'EOF'
#!/bin/bash
# Launches the RustHazard server from this bundle.
# DATA_DIR defaults to ./data next to this bundle.
cd "$(dirname "$0")"
export RH_STATIC_DIR="${RH_STATIC_DIR:-$PWD/static}"
export RH_TEMPLATES_DIR="${RH_TEMPLATES_DIR:-$PWD/templates}"
export RH_PROGRAM_DIR="${RH_PROGRAM_DIR:-$PWD}"
export RH_DATA_DIR="${RH_DATA_DIR:-$PWD/data}"
exec ./rh-server
EOF
chmod +x "$OUT/run.sh"

cat > "$OUT/README.txt" <<'EOF'
RustHazard server (release bundle)

Run:      ./run.sh            (data in ./data; port from config.json, default 5000)
Config:   data/config.json    (created on first run; default admin/rotorhazard)
Data:     data/database.db    (same schema as the Python server)

Environment:
  RH_PORT          override HTTP port
  RH_NODES         mock node count (default 8)
  RH_DATA_DIR      data directory (default ./data inside the bundle)
  MOCK_NODE_SIGNAL config.json GENERAL key; 2 = simulated RSSI (default in
                   first-run configs), 0 = idle mocks
EOF

echo "== packing =="
tar -czf "dist/rh-server-rust-${VERSION_TAG}.tar.gz" -C dist "rh-server-rust-${VERSION_TAG}"
echo "Bundle: dist/rh-server-rust-${VERSION_TAG}.tar.gz"
du -sh "$OUT" "dist/rh-server-rust-${VERSION_TAG}.tar.gz"

# Cross-compiled aarch64 (Pi) bundle when the target toolchain is
# installed (rustup target add aarch64-unknown-linux-gnu + a GNU linker).
# Otherwise build natively on the Pi with this same script.
if rustup target list --installed 2>/dev/null | grep -q '^aarch64-unknown-linux-gnu$'; then
    echo "== building aarch64 (Raspberry Pi) binary =="
    cargo build --release --manifest-path rust/server/Cargo.toml --target aarch64-unknown-linux-gnu
    PI_OUT="dist/rh-server-rust-${VERSION_TAG}-aarch64"
    rm -rf "$PI_OUT"
    mkdir -p "$PI_OUT"
    cp rust/target/aarch64-unknown-linux-gnu/release/rh-server "$PI_OUT/"
    cp "$OUT/run.sh" "$OUT/README.txt" "$PI_OUT/"
    cp LICENSE "$PI_OUT/" 2>/dev/null || true
    tar -czf "dist/rh-server-rust-${VERSION_TAG}-aarch64.tar.gz" -C dist "rh-server-rust-${VERSION_TAG}-aarch64"
    echo "Bundle: dist/rh-server-rust-${VERSION_TAG}-aarch64.tar.gz"
else
    echo "(aarch64 target not installed; skipping Pi bundle)"
fi