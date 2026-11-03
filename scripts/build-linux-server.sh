#!/usr/bin/env bash
# Builds ledger-server for Linux (x86_64 or aarch64) from a Mac, in Docker: a Debian container cross-compiles it,
# without Tauri (`--no-default-features --features server`), so the server needs no GUI libraries.
#
#   scripts/build-linux-server.sh [x86_64|aarch64]     → src-tauri/target/linux/<arch>-unknown-linux-gnu/release/ledger-server
#
# Built against Debian bookworm's glibc (2.36), so it runs on any newer distribution (Ubuntu 24.04, 26.04, …).
# The image and the cargo download cache are kept between runs; the first build takes a few minutes.

set -euo pipefail

ARCH="${1:-x86_64}"
case "$ARCH" in
	x86_64) GCC_PKG=gcc-x86-64-linux-gnu LIBC_PKG=libc6-dev-amd64-cross GCC=x86_64-linux-gnu-gcc ;;
	aarch64) GCC_PKG=gcc-aarch64-linux-gnu LIBC_PKG=libc6-dev-arm64-cross GCC=aarch64-linux-gnu-gcc ;;
	*) echo "build-linux-server: unknown architecture $ARCH (x86_64 or aarch64)" >&2; exit 1 ;;
esac
TARGET="$ARCH-unknown-linux-gnu"
TRIPLE_ENV=$(echo "$TARGET" | tr 'a-z-' 'A-Z_')
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="ledger-linux-builder:$ARCH"

docker info >/dev/null 2>&1 || { echo "build-linux-server: Docker isn't running" >&2; exit 1; }

# A builder image with the cross compiler, made once.
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
	docker build -t "$IMAGE" - <<EOF
FROM rust:1-bookworm
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends $GCC_PKG $LIBC_PKG && rm -rf /var/lib/apt/lists/*
RUN rustup target add $TARGET
EOF
fi

docker run --rm \
	-v "$ROOT/src-tauri:/src" \
	-v ledger-cargo-registry:/usr/local/cargo/registry \
	-w /src \
	-e "CARGO_TARGET_${TRIPLE_ENV}_LINKER=$GCC" \
	-e "CC_${TARGET//-/_}=$GCC" \
	-e "CARGO_TARGET_DIR=/src/target/linux" \
	"$IMAGE" \
	cargo build --release --locked --no-default-features --features server --bin ledger-server --target "$TARGET"

echo "$ROOT/src-tauri/target/linux/$TARGET/release/ledger-server"
