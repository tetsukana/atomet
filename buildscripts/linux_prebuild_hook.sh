#!/bin/bash
# Buildroot の linux.mk から呼ばれる pre-build フック
# linux_makefile.patch を Buildroot 2024.02 に適用した場合に動作する
# 参照: patches/buildroot/linux_makefile.patch
set -e

WORKSPACE=${GITHUB_WORKSPACE:-/src}
"$WORKSPACE/buildscripts/make_initramfs.sh"
