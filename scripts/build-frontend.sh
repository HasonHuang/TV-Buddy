#!/usr/bin/env bash
# 构建前端产物到 frontend/dist，供 rust-embed 嵌入。
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v pnpm >/dev/null 2>&1; then
  echo "缺少 pnpm。请先安装 Node.js 与 pnpm。" >&2
  exit 1
fi

cd frontend
pnpm install
pnpm build
echo "前端产物就绪：frontend/dist"
