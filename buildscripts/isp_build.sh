#!/bin/bash
# Build tx-isp-t31.ko from source (with libt31-firmware.a)
# Usage: docker compose run --rm builder isp_build
set -e

WORKSPACE=/src
ISP_DIR="$WORKSPACE/tx-isp-t31"
BUILDROOT_DIR=/build/buildroot-2024.02
LINUX_DIR="$BUILDROOT_DIR/output/build/linux-custom"
CROSS="$BUILDROOT_DIR/output/host/usr/bin/mipsel-ingenic-linux-gnu-"

if [ ! -f "$LINUX_DIR/Module.symvers" ]; then
    echo "ERROR: Kernel not built yet. Run 'docker_build' first."
    exit 1
fi

echo "=== Building tx-isp-t31.ko ==="

make -C "$LINUX_DIR" \
    M="$ISP_DIR" \
    CROSS_COMPILE="$CROSS" \
    ARCH=mips \
    modules

mkdir -p "$WORKSPACE/output"
cp "$ISP_DIR/tx-isp-t31.ko" "$WORKSPACE/output/"

echo "=== Done ==="
ls -lh "$WORKSPACE/output/tx-isp-t31.ko"
