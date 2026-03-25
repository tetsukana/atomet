#!/bin/bash
# BR2_ROOTFS_POST_BUILD_SCRIPT として Buildroot から呼ばれる。
# squashfs 生成前に実行されるため、ここでインストールしたファイルは rootfs に含まれる。
# $1 = output/target ディレクトリ
set -e

TARGET_DIR="$1"
WORKSPACE=/src
BUILDROOT_DIR=/build/buildroot-2024.02
RUST_TARGET="mipsel-unknown-linux-gnu"
CROSS_GCC="$BUILDROOT_DIR/output/host/usr/bin/mipsel-ingenic-linux-gnu-gcc"
STAGING_DIR="$BUILDROOT_DIR/output/host/mipsel-ingenic-linux-gnu/sysroot"

# Rust ツールチェーンのキャッシュ先 (Docker ボリューム /build/ に永続化)
export RUSTUP_HOME=/build/rustup
export CARGO_HOME=/build/cargo
export PATH="$CARGO_HOME/bin:$PATH"

# isvp-sys build.rs が参照する環境変数
export ATOMET_BUILDROOT_DIR="$BUILDROOT_DIR"

# Buildroot デフォルトの NTP init スクリプトを削除 (S42ntpd で管理)
rm -f "$TARGET_DIR/etc/init.d/S49ntp"

# init スクリプトの実行権限を保証
chmod +x "$TARGET_DIR/etc/init.d/S"* 2>/dev/null || true

# =============================================================================
# go2rtc バイナリ (mipsle)
# =============================================================================
GO2RTC_VERSION="1.9.14"
GO2RTC_DST="$TARGET_DIR/usr/bin/go2rtc"
if [ ! -f "$GO2RTC_DST" ]; then
    echo "=== Downloading go2rtc v$GO2RTC_VERSION (linux_mipsle) ==="
    curl -fL \
        "https://github.com/AlexxIT/go2rtc/releases/download/v${GO2RTC_VERSION}/go2rtc_linux_mipsle" \
        -o "$GO2RTC_DST"
    chmod +x "$GO2RTC_DST"
    echo "=== go2rtc installed to $GO2RTC_DST ==="
fi

# =============================================================================
# Rust ビルド (増分)
# =============================================================================
RUST_STAMP="$BUILDROOT_DIR/output/.rust_rebuild.stamp"
RUST_HASH=$(find "$WORKSPACE/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) | sort | xargs md5sum | md5sum | cut -d' ' -f1)
SAVED_RUST_HASH=$(cat "$RUST_STAMP" 2>/dev/null || echo "")

if [ "$RUST_HASH" != "$SAVED_RUST_HASH" ] || [ ! -f "$TARGET_DIR/usr/bin/atometd" ]; then
    echo "=== Building atometd (Rust, target: $RUST_TARGET) ==="

    # rustup がなければインストール (nightly: mipsel は tier 3 のため -Z build-std が必要)
    if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
        echo "--- Installing rustup ---"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
            sh -s -- -y --default-toolchain nightly --no-modify-path
    fi

    rustup default nightly
    rustup component add rust-src

    cd "$WORKSPACE"
    CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_GNU_LINKER="$CROSS_GCC" \
    OPENSSL_DIR="$STAGING_DIR/usr" \
    OPENSSL_LIB_DIR="$STAGING_DIR/usr/lib" \
    OPENSSL_INCLUDE_DIR="$STAGING_DIR/usr/include" \
        cargo build -p atometd --release --target "$RUST_TARGET" -Z build-std=std,panic_abort

    install -D -m 755 \
        "$WORKSPACE/target/$RUST_TARGET/release/atometd" \
        "$TARGET_DIR/usr/bin/atometd"

    for lib in libimp.so libsysutils.so libalog.so libaudioProcess.so; do
        if [ -f "$WORKSPACE/crates/isvp-sys/lib/$lib" ]; then
            install -D -m 755 "$WORKSPACE/crates/isvp-sys/lib/$lib" "$TARGET_DIR/usr/lib/$lib"
        fi
    done

    echo "$RUST_HASH" > "$RUST_STAMP"
    echo "=== atometd installed to $TARGET_DIR/usr/bin/ ==="
else
    echo "=== Rust sources unchanged: skipping build ==="
fi

# =============================================================================
# Web (Svelte) ビルド (増分)
# =============================================================================
WEB_STAMP="$BUILDROOT_DIR/output/.web_rebuild.stamp"
WEB_HASH=$(find "$WORKSPACE/web/src" "$WORKSPACE/web/static" \
    "$WORKSPACE/web/package.json" "$WORKSPACE/web/svelte.config.js" \
    "$WORKSPACE/web/vite.config.ts" -type f 2>/dev/null | sort | xargs md5sum | md5sum | cut -d' ' -f1)
SAVED_WEB_HASH=$(cat "$WEB_STAMP" 2>/dev/null || echo "")

if [ "$WEB_HASH" != "$SAVED_WEB_HASH" ] || [ ! -d "$TARGET_DIR/var/www/atomet" ]; then
    echo "=== Building web (Svelte) ==="
    # /src はWindowsホストマウントのためシンボリックリンク不可。
    # node_modules を /build 上のコピーで管理し、--no-bin-links を回避する。
    WEB_BUILD_DIR=/build/web_build
    rm -rf "$WEB_BUILD_DIR"
    cp -r "$WORKSPACE/web" "$WEB_BUILD_DIR"
    cd "$WEB_BUILD_DIR"
    npm ci --cache /build/npm_cache
    npm run build

    mkdir -p "$TARGET_DIR/var/www/atomet"
    cp -r "$WEB_BUILD_DIR/build/." "$TARGET_DIR/var/www/atomet/"
    rm -rf "$WEB_BUILD_DIR"

    echo "$WEB_HASH" > "$WEB_STAMP"
    echo "=== Web built and installed to $TARGET_DIR/var/www/atomet/ ==="
else
    echo "=== Web sources unchanged: skipping build ==="
fi
