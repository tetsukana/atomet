#!/bin/bash
# Rust クレートのみビルド (docker compose run --rm builder rust_build)
# Buildroot のフルビルドが1回以上完了している前提 (クロスコンパイラ + OpenSSL が必要)
set -e

WORKSPACE=/src
BUILDROOT_DIR=/build/buildroot-2024.02
RUST_TARGET="mipsel-unknown-linux-gnu"
CROSS_GCC="$BUILDROOT_DIR/output/host/usr/bin/mipsel-ingenic-linux-gnu-gcc"
STAGING_DIR="$BUILDROOT_DIR/output/host/mipsel-ingenic-linux-gnu/sysroot"

export RUSTUP_HOME=/build/rustup
export CARGO_HOME=/build/cargo
export PATH="$CARGO_HOME/bin:$PATH"

# isvp-sys build.rs が参照する環境変数
export ATOMET_BUILDROOT_DIR="$BUILDROOT_DIR"

# クロスコンパイラの存在確認
if [ ! -x "$CROSS_GCC" ]; then
    echo "ERROR: Cross compiler not found. Run a full build first: docker compose run --rm builder docker_build"
    exit 1
fi

# rustup がなければインストール
if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
    echo "--- Installing rustup ---"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --default-toolchain nightly --no-modify-path
fi

rustup default nightly
rustup component add rust-src

echo "=== Building atometd (Rust only) ==="

echo "TARGET: $RUST_TARGET"

cd "$WORKSPACE"
CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_GNU_LINKER="$CROSS_GCC" \
OPENSSL_DIR="$STAGING_DIR/usr" \
OPENSSL_LIB_DIR="$STAGING_DIR/usr/lib" \
OPENSSL_INCLUDE_DIR="$STAGING_DIR/usr/include" \
    cargo build -p atometd --release --target "$RUST_TARGET" -Z build-std=std,panic_abort

OUTPUT="$WORKSPACE/output/atometd"
install -D -m 755 "target/$RUST_TARGET/release/atometd" "$OUTPUT"

echo "=== Done: $OUTPUT ==="
echo "Deploy: scp output/atometd root@atomet.local:/media/mmc/"
