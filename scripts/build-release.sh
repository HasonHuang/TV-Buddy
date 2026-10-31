#!/usr/bin/env bash
# 构建发布产物：两个 musl 目标 → dist/tv-buddy-<version>-<arch>.tar.gz
#
# 用 Docker + rust:1-alpine：Alpine 镜像的宿主三元组本身就是 *-unknown-linux-musl，
# 因此直接 `cargo build --release` 就得到静态 musl 二进制。
# 依赖来自宿主 ~/.cargo 缓存（挂载进容器，`--offline` 构建）。
#
# 用法：scripts/build-release.sh [amd64] [arm64]
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=$(python3 -c 'import json,subprocess;print(json.loads(subprocess.check_output(["cargo","metadata","--no-deps","--format-version","1"]))["packages"][0]["version"])')
IMAGE=${IMAGE:-rust:1-alpine}
OUT=dist
mkdir -p "$OUT"

[ -f frontend/dist/index.html ] || { echo "缺少 frontend/dist，先跑 scripts/build-frontend.sh"; exit 1; }

package() { # 参数：架构（amd64/arm64）
  local arch=$1 platform=$1 target_dir
  case "$arch" in
    amd64) target_dir=target/release ;;
    arm64) target_dir=target/release ;;
  esac
  echo "==> 构建 linux/$platform"
  # 镜像默认工具链是 "stable"，容器无外网，rustup 会卡在同步。改用已装的具体工具链。
  local toolchain
  toolchain=$(docker run --rm --platform "linux/$platform" --entrypoint sh "$IMAGE" -c 'ls /usr/local/rustup/toolchains | head -1')
  docker run --rm --platform "linux/$platform" \
    -v "$PWD":/src -w /src \
    -v "$HOME/.cargo/registry":/usr/local/cargo/registry \
    -v "$HOME/.cargo/git":/usr/local/cargo/git \
    -e CARGO_HOME=/usr/local/cargo \
    -e CARGO_TARGET_DIR=/src/target/pack-$platform \
    -e RUSTUP_TOOLCHAIN="$toolchain" \
    "$IMAGE" cargo build --release --offline

  local stage="$OUT/tv-buddy-$VERSION-$platform"
  rm -rf "$stage"
  install -d "$stage"
  install -m 0755 "target/pack-$platform/release/tv-buddy" "$stage/tv-buddy"
  install -m 0755 packaging/tv-buddy.sh "$stage/tv-buddy.sh"
  install -m 0755 install.sh "$stage/install.sh"
  install -m 0644 packaging/tv-buddy.service "$stage/tv-buddy.service"
  install -m 0755 packaging/tv-buddy.openrc "$stage/tv-buddy.openrc"
  tar -C "$OUT" -czf "$OUT/tv-buddy-$VERSION-$platform.tar.gz" "tv-buddy-$VERSION-$platform"
  ( cd "$OUT" && shasum -a 256 "tv-buddy-$VERSION-$platform.tar.gz" > "tv-buddy-$VERSION-$platform.tar.gz.sha256" )
  echo "    产物：$OUT/tv-buddy-$VERSION-$platform.tar.gz"
}

if [ "$#" -gt 0 ]; then
  arches=("$@")
else
  arches=(amd64 arm64)
fi

for arch in "${arches[@]}"; do
  package "$arch"
done
