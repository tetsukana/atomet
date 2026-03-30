#!/bin/bash
# Build tx-isp-t31.ko and sensor_gc2053_t31.ko from source
# Usage: docker compose run --rm builder drivers_build
set -e

WORKSPACE=/src
DRIVERS_DIR="$WORKSPACE/drivers"
ISP_DIR="$DRIVERS_DIR/tx-isp-t31"
SENSOR_DIR="$DRIVERS_DIR/sensors-t31/gc2053"
BUILDROOT_DIR=/build/buildroot-2024.02
LINUX_DIR="$BUILDROOT_DIR/output/build/linux-custom"
CROSS="$BUILDROOT_DIR/output/host/usr/bin/mipsel-ingenic-linux-gnu-"

if [ ! -f "$LINUX_DIR/Module.symvers" ]; then
    echo "ERROR: Kernel not built yet. Run 'docker_build' first."
    exit 1
fi

# --- Build tx-isp-t31.ko ---
echo "=== Building tx-isp-t31.ko ==="

make -C "$LINUX_DIR" \
    M="$ISP_DIR" \
    CROSS_COMPILE="$CROSS" \
    ARCH=mips \
    modules

# --- Build sensor_gc2053_t31.ko ---
echo "=== Building sensor_gc2053_t31.ko ==="

make -C "$LINUX_DIR" \
    M="$SENSOR_DIR" \
    CROSS_COMPILE="$CROSS" \
    ARCH=mips \
    KBUILD_EXTRA_SYMBOLS="$ISP_DIR/Module.symvers" \
    EXTRA_CFLAGS="-I$ISP_DIR/include" \
    modules

# --- Copy outputs ---
mkdir -p "$WORKSPACE/output"
cp "$ISP_DIR/tx-isp-t31.ko" "$WORKSPACE/output/"
cp "$SENSOR_DIR/sensor_gc2053_t31.ko" "$WORKSPACE/output/"

echo "=== Done ==="
ls -lh "$WORKSPACE/output/tx-isp-t31.ko" "$WORKSPACE/output/sensor_gc2053_t31.ko"
echo "Deploy: scp output/*.ko root@atomet.local:/media/mmc/"
