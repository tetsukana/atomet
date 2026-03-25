#!/bin/bash
# WSL2/Linux でローカルビルドするスクリプト
# 使い方: bash buildscripts/build_local.sh
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$WORKSPACE/build"
BUILDROOT_VERSION="buildroot-2024.02"
BUILDROOT_DIR="$BUILD_DIR/$BUILDROOT_VERSION"

echo "=== atomet local build ==="
echo "WORKSPACE: $WORKSPACE"

# Buildroot ダウンロード
if [ ! -d "$BUILDROOT_DIR" ]; then
  mkdir -p "$BUILD_DIR"
  echo "Downloading Buildroot..."
  curl -o - "https://buildroot.org/downloads/$BUILDROOT_VERSION.tar.gz" | tar zxvf - -C "$BUILD_DIR"
fi

# カスタムパッケージをコピー
cp -pR "$WORKSPACE/custompackages/package/"* "$BUILDROOT_DIR/package/"

# カーネル設定に絶対パスを追加
KERNEL_CONFIG="$BUILDROOT_DIR/../atomet_kernel.config"
cp "$WORKSPACE/configs/kernel.config" "$KERNEL_CONFIG"
echo "CONFIG_CROSS_COMPILE=\"$BUILDROOT_DIR/output/host/usr/bin/mipsel-ingenic-linux-gnu-\"" >> "$KERNEL_CONFIG"
echo "CONFIG_INITRAMFS_SOURCE=\"$BUILDROOT_DIR/output/images/initramfs.cpio\"" >> "$KERNEL_CONFIG"

# Buildroot defconfig 適用
cd "$BUILDROOT_DIR"
make defconfig BR2_DEFCONFIG="$WORKSPACE/configs/atomet_defconfig"

# initramfs ビルド
"$WORKSPACE/buildscripts/make_initramfs.sh"
make rootfs-initramfs

# Linux + 全体ビルド
make linux-rebuild
make

echo "=== Done ==="
ls -lh output/images/
