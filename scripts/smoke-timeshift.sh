#!/usr/bin/env bash
# 回看冒烟：点亮 livestream → 缓冲 → VOD 清单 → 伪装切片。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19936}
UPORT=${UPORT:-19937}
WORK=$(mktemp -d)

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "跳过：未安装 ffmpeg（仅用于生成测试流）"
  exit 0
fi

ffmpeg -loglevel error -f lavfi -i testsrc=size=320x240:rate=25 -t 16 \
  -c:v libx264 -preset ultrafast -g 50 -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_filename "$WORK/seg%d.ts" "$WORK/live.m3u8"
python3 -m http.server "$UPORT" --bind 127.0.0.1 --directory "$WORK" >"$WORK/up.log" 2>&1 &
UPID=$!

cat > "$WORK/config.toml" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"

[[groups]]
id = "ts"
name = "回看组"
mode = "proxy"
access = "open"
[[groups.channels]]
id = "c1"
name = "回看频道"
url = "http://127.0.0.1:$UPORT/live.m3u8"
livestream = true
timeshift_hours = 4
TOML

"$BIN" "$WORK/config.toml" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID $UPID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 60); do curl -fsS "$B/health" >/dev/null 2>&1 && break; sleep 0.1; done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }

NOW=$(date +%s)
URL="$B/ts/c1.m3u8?start=$((NOW - 60))&end=$((NOW + 60))"
CODE=0
for _ in $(seq 1 30); do
  CODE=$(curl -s -o "$WORK/list.m3u8" -w '%{http_code}' "$URL")
  grep -q '#EXT-X-PLAYLIST-TYPE:VOD' "$WORK/list.m3u8" && break
  sleep 0.5
done
check "回看清单 200" "200" "$CODE"
check "VOD 类型" "1" "$(grep -c '#EXT-X-PLAYLIST-TYPE:VOD' "$WORK/list.m3u8" || true)"
check "含结束标记" "1" "$(grep -c '#EXT-X-ENDLIST' "$WORK/list.m3u8" || true)"
check "含回看切片" "yes" "$(grep -q '/ts/c1/timeshift/[0-9a-f]*[.]' "$WORK/list.m3u8" && echo yes || echo no)"

SEG=$(grep -o '/ts/c1/timeshift/[0-9a-f]*[.][a-z]*' "$WORK/list.m3u8" | head -1)
check "切片 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' "$B$SEG")"
check "切片类型为图片" "1" "$(curl -s -o /dev/null -D - "$B$SEG" | grep -ci '^content-type: image/')"
check "切片缓存头" "1" "$(curl -s -o /dev/null -D - "$B$SEG" | grep -ci '^cache-control: private, max-age=3600')"

curl -s -o "$WORK/live.m3u8" "$B/ts/c1.m3u8?start=0&end=60"
check "越界落回实时" "0" "$(grep -c '#EXT-X-PLAYLIST-TYPE:VOD' "$WORK/live.m3u8" || true)"

if [ "$fail" = "0" ]; then echo "回看冒烟测试全部通过"; else echo "回看冒烟测试有失败项"; exit 1; fi
