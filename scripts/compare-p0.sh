#!/usr/bin/env bash
# P0 对拍：同一份夹具下，对比目标 v0.8.2（容器 iptv82）与本实现。
#
# 做法：容器 PID 1 就是目标进程（`/app/iptv-proxy /app/config.toml`）。
# 因此先备份容器内配置，再写入同一键面的夹具配置，重启容器加载，
# 跑完后恢复原配置并停止容器（恢复到运行前的状态）。
#
# 依赖：Docker（容器 iptv82 与 /app/iptv-proxy 已存在）、python3。
# 用法：scripts/compare-p0.sh
set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/release/iptv-proxy}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
UP=${UP:-9999}
TARGET_PORT=${TARGET_PORT:-19899}
OUR_PORT=${OUR_PORT:-19898}
CONTAINER=${CONTAINER:-iptv82}
WORK=$(mktemp -d)
ORIGIN="$WORK/origin"
BASE_CFG="$WORK/base.toml"
OUR_CFG="$WORK/config.toml"
TARGET_CFG="$WORK/cmp-config.toml"
BACKUP=/app/config.cmp-backup.toml

# --- 夹具：伪上游（HLS 媒体清单 + 两个切片） ---
mkdir -p "$ORIGIN"
cat > "$ORIGIN/live.m3u8" <<'EOF'
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg1.ts
#EXTINF:4.0,
seg2.ts
#EXT-X-ENDLIST
EOF
head -c 4096 /dev/urandom > "$ORIGIN/seg1.ts"
head -c 2048 /dev/urandom > "$ORIGIN/seg2.ts"
mkdir -p "$ORIGIN/v0"
cat > "$ORIGIN/master.m3u8" <<'EOF'
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360
360/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x1080
v0/index.m3u8
EOF
cat > "$ORIGIN/v0/index.m3u8" <<'EOF'
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:6
#EXTINF:6.0,
v1.ts
#EXT-X-ENDLIST
EOF
head -c 1024 /dev/urandom > "$ORIGIN/v0/v1.ts"

# --- 取出容器内配置当作键面模板，只改 bind / disk_path / 上游主机名 ---
docker start "$CONTAINER" >/dev/null 2>&1 || true
sleep 2
docker exec "$CONTAINER" cat /app/config.toml > "$BASE_CFG"

build_cfg() { # 参数：1 bind 2 disk_path 3 上游前缀 4 输出文件
  python3 - "$1" "$2" "$3" "$BASE_CFG" "$4" <<'PY'
import sys

bind, disk, upstream, base_path, out_path = sys.argv[1:6]
head = open(base_path, encoding="utf-8").read().split("[[groups]]")[0]

# 统一夹具：关 EPG、开 Token、固定头行。
patches = {
    ("server", "bind"): f'bind = "{bind}"',
    ("server", "m3u_header_lines"): 'm3u_header_lines = ["#EXTVLCOPT:http-user-agent=ReaCompare/1.0"]',
    ("cache", "disk_path"): f'disk_path = "{disk}"',
    ("auth", "token_enabled"): "token_enabled = true",
    ("epg", "enabled"): "enabled = false",
}
section = ""
result = []
for line in head.splitlines():
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        section = stripped.strip("[]")
        result.append(line)
        continue
    key = stripped.split("=")[0].strip()
    patch = patches.get((section, key))
    result.append(patch if patch else line)
head = "\n".join(result).rstrip() + "\n"
if "[epg]" not in head:
    head += "\n[epg]\nenabled = false\n"

groups = f"""
[[groups]]
id = "pg"
name = "代理组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/live.m3u8"

[[groups]]
id = "pm"
name = "主清单组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/master.m3u8"

[[groups]]
id = "hd"
name = "隐藏组"
mode = "proxy"
access = "hidden"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/live.m3u8"

[[groups]]
id = "op"
name = "开放组"
mode = "proxy"
access = "open"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/live.m3u8"

[[groups]]
id = "ar"
name = "归档组"
mode = "proxy"
access = "archived"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/live.m3u8"

[[groups]]
id = "rg"
name = "直连组"
mode = "redirect"
access = "public"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/live.m3u8"

[[groups]]
id = "rw"
name = "改写组"
mode = "rewrite"
access = "public"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/live.m3u8"

[[groups]]
id = "pt"
name = "透传组"
mode = "passthrough"
access = "public"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "{upstream}/live.m3u8"
"""
open(out_path, "w", encoding="utf-8").write(head + groups)
PY
}

build_cfg "127.0.0.1:$OUR_PORT" "$WORK/cache" "http://127.0.0.1:$UP" "$OUR_CFG"
build_cfg "[::]:$TARGET_PORT" "/app/cmp-cache" "http://host.docker.internal:$UP" "$TARGET_CFG"

# --- 备份容器内配置，装入夹具后重启容器 ---
docker exec "$CONTAINER" sh -c "[ -f $BACKUP ] || cp /app/config.toml $BACKUP"
docker cp "$TARGET_CFG" "$CONTAINER:/app/config.toml" >/dev/null
docker exec "$CONTAINER" rm -rf /app/cmp-cache
docker restart "$CONTAINER" >/dev/null

cleanup() {
  kill ${ORIGIN_PID:-0} ${OUR_PID:-0} 2>/dev/null || true
  docker exec "$CONTAINER" sh -c "cp $BACKUP /app/config.toml && rm -rf /app/cmp-cache" 2>/dev/null || true
  docker stop "$CONTAINER" >/dev/null 2>&1 || true
  wait 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

python3 -m http.server "$UP" --directory "$ORIGIN" >"$WORK/origin.log" 2>&1 &
ORIGIN_PID=$!
(cd "$WORK" && exec "$BIN" "$OUR_CFG" >"$WORK/ours.log" 2>&1) &
OUR_PID=$!

ready() { # 参数：base
  for _ in $(seq 1 150); do
    curl -fsS "$1/health" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
ready "http://127.0.0.1:$TARGET_PORT" || {
  echo "目标未就绪；容器日志："
  docker logs --tail 20 "$CONTAINER" 2>&1 || true
  exit 1
}
ready "http://127.0.0.1:$OUR_PORT" || {
  echo "本实现未就绪；日志："
  cat "$WORK/ours.log"
  exit 1
}

# --- 对拍 ---
set +e
TARGET_BASE="http://127.0.0.1:$TARGET_PORT" OUR_BASE="http://127.0.0.1:$OUR_PORT" \
  python3 scripts/compare_p0.py
code=$?
set -e
exit "$code"
