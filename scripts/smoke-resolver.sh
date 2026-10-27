#!/usr/bin/env bash
# 解析脚本冒烟：stdout 覆盖频道 URL、TTL 缓存、失败转 error_url。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19928}
UPORT=${UPORT:-19929}
WORK=$(mktemp -d)
MARK="$WORK/probe.count"

printf '#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n' > "$WORK/live.m3u8"
python3 -m http.server "$UPORT" --bind 127.0.0.1 --directory "$WORK" >"$WORK/upstream.log" 2>&1 &
UPID=$!

mkdir -p "$WORK/cache/resolvers"
printf 'import sys\nopen(r"%s","a").write("x")\nprint("http://127.0.0.1:%s/live.m3u8")\n' "$MARK" "$UPORT" > "$WORK/cache/resolvers/probe.py"
printf 'print("not-a-url")\n' > "$WORK/cache/resolvers/bad.py"

cat > "$WORK/config.toml" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"
error_url = "http://placeholder.test/mistake.mp4"

[cache]
disk_path = "$WORK/cache"

[[groups]]
id = "rg"
name = "解析组"
mode = "proxy"
access = "open"
[[groups.channels]]
id = "c1"
name = "脚本频道"
url = "http://127.0.0.1:9/never"
resolver_script = "probe.py"
resolver_ttl = 60
[[groups.channels]]
id = "c2"
name = "坏脚本"
url = "http://127.0.0.1:9/never"
resolver_script = "bad.py"
TOML

"$BIN" "$WORK/config.toml" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID $UPID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"

for _ in $(seq 1 60); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }
marks() { tr -cd 'x' < "$1" | wc -c | tr -d ' '; }

check "脚本清单 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' "$B/rg/c1.m3u8")"
check "清单含 EXTINF" "1" "$(curl -s "$B/rg/c1.m3u8" | grep -c '#EXTINF')"
check "TTL 内只执行一次" "1" "$(marks "$MARK")"
check "坏脚本 302" "302" "$(curl -s -o /dev/null -w '%{http_code}' "$B/rg/c2.m3u8")"
check "302 指向 error_url" "http://placeholder.test/mistake.mp4" \
  "$(curl -s -o /dev/null -D - "$B/rg/c2.m3u8" | awk 'BEGIN{IGNORECASE=1}index($0,"location:")==1{sub(/\r$/,"");print substr($0,11)}')"

if [ "$fail" = "0" ]; then echo "解析脚本冒烟测试全部通过"; else echo "解析脚本冒烟测试有失败项"; exit 1; fi
