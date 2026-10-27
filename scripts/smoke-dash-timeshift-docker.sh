#!/usr/bin/env bash
# DASH 回看冒烟（容器版）：宿主 ffmpeg 无 DASH 解复用器时使用。
#
# 需要 Linux arm64 运行时的 ffmpeg。默认镜像自带 ffmpeg（dash 解复用器 + libx264）。
# 二进制默认取 target/dash-arm64/debug/tv-buddy；缺失时用 rust:1-alpine 构建。
#
# 用法：scripts/smoke-dash-timeshift-docker.sh
# 变量：IMAGE（默认 charmingcheung000/mpd-hls:latest）、BIN、RUST_IMAGE
set -euo pipefail

cd "$(dirname "$0")/.."
IMAGE=${IMAGE:-charmingcheung000/mpd-hls:latest}
RUST_IMAGE=${RUST_IMAGE:-rust:1-alpine}
BIN=${BIN:-target/dash-arm64/debug/tv-buddy}
TARGET_DIR=${TARGET_DIR:-target/dash-arm64}

if ! command -v docker >/dev/null 2>&1; then
  echo "跳过：未安装 docker"
  exit 0
fi

if [ ! -x "$BIN" ]; then
  echo "==> 构建 linux/arm64 调试二进制"
  TOOLCHAIN=$(docker run --rm --entrypoint sh "$RUST_IMAGE" -c 'ls /usr/local/rustup/toolchains | head -1')
  docker run --rm \
    -v "$PWD":/src -w /src \
    -v "$HOME/.cargo/registry":/usr/local/cargo/registry \
    -v "$HOME/.cargo/git":/usr/local/cargo/git \
    -e CARGO_HOME=/usr/local/cargo \
    -e "CARGO_TARGET_DIR=/src/$TARGET_DIR" \
    -e "RUSTUP_TOOLCHAIN=$TOOLCHAIN" \
    "$RUST_IMAGE" cargo build --offline
fi

echo "==> 容器内跑断言"
docker run --rm \
  -v "$PWD":/src \
  --entrypoint python3 \
  "$IMAGE" /src/scripts/dash-smoke-container.py "/src/$BIN"
