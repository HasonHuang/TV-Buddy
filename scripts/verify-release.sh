#!/usr/bin/env bash
# 校验发布产物：静态链接、能启动、/health 通。
# 用法：scripts/verify-release.sh [amd64|arm64]
set -euo pipefail

cd "$(dirname "$0")/.."
ARCH=${1:-arm64}
TARBALL=$(ls -1 dist/iptv-proxy-*-"$ARCH".tar.gz 2>/dev/null | tail -1)
[ -n "$TARBALL" ] || { echo "找不到 arch=$ARCH 的发布包"; exit 1; }
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
tar -C "$WORK" -xzf "$TARBALL"
DIR=$(find "$WORK" -maxdepth 1 -type d -name 'iptv-proxy-*' | head -1)

cat > "$DIR/config.toml" <<'EOF'
[server]
bind = "127.0.0.1:19899"
log_level = "warn"

[cache]
disk_path = "/tmp/cache"

[[groups]]
id = "g"
name = "组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c"
name = "频道"
url = "http://127.0.0.1:1/x.m3u8"
EOF

# 静态判定：ELF 程序头里不应有 PT_INTERP（值为 3）。
python3 - "$DIR/iptv-proxy" <<'PY'
import struct
import sys

path = sys.argv[1]
with open(path, "rb") as handle:
    head = handle.read(64)
    if head[:4] != b"\x7fELF" or head[4] != 2 or head[5] != 1:
        sys.exit("不是 ELF64 小端文件")
    phoff = struct.unpack_from("<Q", head, 0x20)[0]
    phentsize = struct.unpack_from("<H", head, 0x36)[0]
    phnum = struct.unpack_from("<H", head, 0x38)[0]
    handle.seek(phoff)
    for _ in range(phnum):
        entry = handle.read(phentsize)
        if struct.unpack_from("<I", entry, 0)[0] == 3:
            sys.exit("存在 PT_INTERP：动态链接")
print("静态链接 OK")
PY

docker run --rm --platform "linux/$ARCH" -v "$DIR":/w -w /w alpine:3.20 sh -c '
set -e
./iptv-proxy /w/config.toml >/w/run.log 2>&1 &
pid=$!
ok=0
for _ in $(seq 1 50); do
  if body=$(wget -qO- http://127.0.0.1:19899/health 2>/dev/null); then ok=1; break; fi
  sleep 0.1
done
[ "$ok" = "1" ] || { echo "健康检查失败"; cat /w/run.log; kill $pid 2>/dev/null || true; exit 1; }
[ "$body" = "ok" ] || { echo "健康检查返回异常：$body"; kill $pid 2>/dev/null || true; exit 1; }
echo "/health = ok"

# 内嵌前端：面板页引用的脚本必须含新分区。
page=$(wget -qO- http://127.0.0.1:19899/panel)
asset=$(printf "%s" "$page" | grep -o "/__assets/[^\"]*panel-[^\"]*\.js" | head -1)
[ -n "$asset" ] || { echo "面板缺少脚本引用"; kill $pid 2>/dev/null || true; exit 1; }
wget -qO /tmp/panel.js "http://127.0.0.1:19899$asset"
grep -q "security/token" /tmp/panel.js || { echo "面板资源不含安全分区"; kill $pid 2>/dev/null || true; exit 1; }
echo "面板资源 = $asset"

# 台标接口可用。
logos=$(wget -qO- http://127.0.0.1:19899/logo/api/list)
printf "%s" "$logos" | grep -q count || { echo "台标列表异常：$logos"; kill $pid 2>/dev/null || true; exit 1; }
echo "/logo/api/list = ok"

kill $pid 2>/dev/null || true
'
