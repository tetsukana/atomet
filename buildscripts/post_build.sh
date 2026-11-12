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
# Out-of-tree カーネルモジュール (tx-isp-t31, sensor_gc2053_t31)
# =============================================================================
LINUX_DIR="$BUILDROOT_DIR/output/build/linux-custom"
CROSS="$BUILDROOT_DIR/output/host/usr/bin/mipsel-ingenic-linux-gnu-"
ISP_DIR="$WORKSPACE/drivers/tx-isp-t31"
SENSOR_DIR="$WORKSPACE/drivers/sensors-t31/gc2053"
DRIVER_INSTALL_DIR="$TARGET_DIR/lib/modules/atomet"

DRIVER_STAMP="$BUILDROOT_DIR/output/.drivers_rebuild.stamp"
DRIVER_HASH=$(find "$ISP_DIR" "$SENSOR_DIR" -type f \
    \( -name '*.c' -o -name '*.h' -o -name 'Makefile' -o -name '*.a' \) 2>/dev/null \
    | sort | xargs md5sum | md5sum | cut -d' ' -f1)
KSYM_HASH=$(md5sum "$LINUX_DIR/Module.symvers" 2>/dev/null | cut -d' ' -f1)
CURRENT_DRIVER_HASH="${DRIVER_HASH}_${KSYM_HASH}"
SAVED_DRIVER_HASH=$(cat "$DRIVER_STAMP" 2>/dev/null || echo "")

if [ ! -f "$LINUX_DIR/Module.symvers" ]; then
    echo "WARNING: $LINUX_DIR/Module.symvers missing, skipping driver build"
elif [ "$CURRENT_DRIVER_HASH" != "$SAVED_DRIVER_HASH" ] \
    || [ ! -f "$DRIVER_INSTALL_DIR/tx-isp-t31.ko" ] \
    || [ ! -f "$DRIVER_INSTALL_DIR/sensor_gc2053_t31.ko" ]; then
    # Clean stale artifacts to avoid linking against object files built for a
    # different kernel/toolchain. (The repo unfortunately has committed .o/.ko
    # files that would otherwise be reused.)
    echo "=== Cleaning driver build artifacts ==="
    make -C "$LINUX_DIR" M="$ISP_DIR" CROSS_COMPILE="$CROSS" ARCH=mips clean || true
    make -C "$LINUX_DIR" M="$SENSOR_DIR" CROSS_COMPILE="$CROSS" ARCH=mips clean || true
    # Belt-and-suspenders: remove any artifacts the module Makefile's clean rule
    # might miss (libt31-firmware.a is a source artifact, keep it).
    find "$ISP_DIR" "$SENSOR_DIR" \
        \( -name '*.o' -o -name '*.ko' -o -name '*.mod.c' \
           -o -name 'Module.symvers' -o -name 'modules.order' \
           -o -name '.*.cmd' -o -name '.tmp_versions' \) \
        -not -path '*/include/*' \
        -exec rm -rf {} + 2>/dev/null || true

    echo "=== Building tx-isp-t31.ko ==="
    make -C "$LINUX_DIR" M="$ISP_DIR" CROSS_COMPILE="$CROSS" ARCH=mips modules

    echo "=== Building sensor_gc2053_t31.ko ==="
    make -C "$LINUX_DIR" M="$SENSOR_DIR" CROSS_COMPILE="$CROSS" ARCH=mips \
        KBUILD_EXTRA_SYMBOLS="$ISP_DIR/Module.symvers" \
        EXTRA_CFLAGS="-I$ISP_DIR/include" \
        modules

    install -D -m 644 "$ISP_DIR/tx-isp-t31.ko" \
        "$DRIVER_INSTALL_DIR/tx-isp-t31.ko"
    install -D -m 644 "$SENSOR_DIR/sensor_gc2053_t31.ko" \
        "$DRIVER_INSTALL_DIR/sensor_gc2053_t31.ko"

    echo "$CURRENT_DRIVER_HASH" > "$DRIVER_STAMP"
    echo "=== Drivers installed to $DRIVER_INSTALL_DIR ==="
else
    echo "=== Drivers unchanged: skipping build ==="
fi

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
# Plate solver DB (solver-db.bin) — ホスト向けに gen をビルド・実行して rootfs へ
# =============================================================================
# Hipparcos カタログ (hip_main.dat) から database.bin を生成する。
# 生成物は solver クレートのソースが変わったときだけ作り直す (増分)。
# DB は無くても atometd は起動する (plate solve が無効になるだけ) ため、
# カタログ取得や生成に失敗してもビルドは止めず警告にとどめる。
# SD カード上の /media/mmc/config/solver-db.bin があればそちらが優先される。
HIP_URL="${HIP_CATALOG_URL:-https://cdsarc.cds.unistra.fr/ftp/cats/I/239/hip_main.dat.gz}"
SOLVER_WORK=/build/solver_db
SOLVER_DB_DST="$TARGET_DIR/usr/share/atomet/solver-db.bin"
SOLVER_STAMP="$SOLVER_WORK/.stamp"
SOLVER_HASH=$({ find "$WORKSPACE/crates/solver" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) | sort | xargs md5sum; echo "$HIP_URL"; stat -c '%n %s %Y' "$WORKSPACE"/data/hip_main.dat* "$WORKSPACE"/data/hyg.csv 2>/dev/null; } | md5sum | cut -d' ' -f1)
SAVED_SOLVER_HASH=$(cat "$SOLVER_STAMP" 2>/dev/null || echo "")

mkdir -p "$SOLVER_WORK"
if [ "$SOLVER_HASH" != "$SAVED_SOLVER_HASH" ] || [ ! -f "$SOLVER_WORK/database.bin" ]; then
    echo "=== Generating solver database ==="
    # NOTE: `set -e` は && / || の判定中は無効になるため、各コマンドに || exit 1 を付ける。
    (
        # ローカルに置いたカタログを優先 (CDS に届かない環境向け)。優先順:
        #   data/hip_main.dat > data/hip_main.dat.gz > data/hyg.csv > ダウンロード
        CATALOG=""
        if [ -f "$WORKSPACE/data/hip_main.dat" ]; then
            echo "--- Using local catalog: data/hip_main.dat ---"
            CATALOG="$WORKSPACE/data/hip_main.dat"
        elif [ -f "$WORKSPACE/data/hip_main.dat.gz" ]; then
            echo "--- Using local catalog: data/hip_main.dat.gz ---"
            gunzip -c "$WORKSPACE/data/hip_main.dat.gz" > "$SOLVER_WORK/hip_main.dat" || exit 1
            CATALOG="$SOLVER_WORK/hip_main.dat"
        elif [ -f "$WORKSPACE/data/hyg.csv" ]; then
            echo "--- Using local catalog: data/hyg.csv (HYG database) ---"
            CATALOG="$WORKSPACE/data/hyg.csv"
        else
            echo "--- Downloading Hipparcos catalog: $HIP_URL ---"
            curl -fL "$HIP_URL" -o "$SOLVER_WORK/hip_main.dat.gz" || exit 1
            gunzip -f "$SOLVER_WORK/hip_main.dat.gz" || exit 1
            CATALOG="$SOLVER_WORK/hip_main.dat"
        fi
        cd "$WORKSPACE" || exit 1
        # ホスト (x86_64) 向けにビルド。mipsel のクロスビルドとはターゲットディレクトリを分ける。
        CARGO_TARGET_DIR=/build/solver_gen_target \
            cargo build -p solver --bin gen --release || exit 1
        cd "$SOLVER_WORK" || exit 1
        rm -f database.bin
        /build/solver_gen_target/release/gen generate "$CATALOG" || exit 1
    ) && echo "$SOLVER_HASH" > "$SOLVER_STAMP" \
      || echo "WARNING: solver database generation failed — plate solving will be disabled unless /media/mmc/config/solver-db.bin exists"
else
    echo "=== Solver database unchanged: skipping generation ==="
fi

if [ -f "$SOLVER_WORK/database.bin" ]; then
    install -D -m 644 "$SOLVER_WORK/database.bin" "$SOLVER_DB_DST"
    echo "=== Solver database installed to $SOLVER_DB_DST ($(stat -c %s "$SOLVER_DB_DST") bytes) ==="
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
