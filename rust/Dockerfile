# RotorHazard server (Rust) — container build
#
# Build:  docker build -t rh-server-rust -f rust/Dockerfile .
# Run:    docker run -p 5000:5000 -v /home/pi/rh-data:/data rh-server-rust
#
# Multi-arch note: build on the target architecture (or use buildx) —
# the release profile is CPU-specific.

FROM rust:1-slim AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY rust/server/Cargo.toml rust/server/Cargo.lock ./rust/server/
# pre-build dependencies for better layer caching
RUN mkdir -p rust/server/src && echo "fn main() {}" > rust/server/src/main.rs \
    && cargo build --release --manifest-path rust/server/Cargo.toml \
    || true
COPY rust/server/src rust/server/src
RUN touch rust/server/src/main.rs \
    && cargo build --release --manifest-path rust/server/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/rust/target/release/rh-server /usr/local/bin/rh-server
# runtime data (config.json, database.db) — mount a volume here
ENV RH_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 5000
CMD ["rh-server"]
