#!/usr/bin/env bash
# 录制冒烟：调度到期 → 合并切片 → 产物可下载。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19938}
UPORT=${UPORT:-19939}
WORK=$(mktemp -d)

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "跳过：未安装 ffmpeg"
  exit 0
fi

ffmpeg -loglevel error -f lavfi -i testsrc=size=320x240:rate=25 -t 12 \
  -c:v libx264 -preset ultrafast -g 25 -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_filename "$WORK/seg%d.ts" "$WORK/live.m3u8"
python3 -m http.server "$UPORT" --bind 127.0.0.1 --directory "$WORK" >"$WORK/up.log" 2>&1 &
UPID=$!

cat > "$WORK/config.toml" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"
timezone_offset_hours = 8

[cache]
disk_path = "$WORK/cache"

[[groups]]
id = "g"
name = "组"
mode = "proxy"
access = "open"
[[groups.channels]]
id = "c1"
name = "频道"
url = "http://127.0.0.1:$UPORT/live.m3u8"
TOML

"$BIN" "$WORK/config.toml" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID $UPID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 60); do curl -fsS "$B/health" >/dev/null 2>&1 && break; sleep 0.1; done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }

T=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')
A="Authorization: Bearer $T"
START=$(python3 -c 'import datetime;print((datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(hours=8)-datetime.timedelta(seconds=5)).strftime("%Y-%m-%dT%H:%M:%S"))')
END=$(python3 -c 'import datetime;print((datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(hours=8)+datetime.timedelta(seconds=15)).strftime("%Y-%m-%dT%H:%M:%S"))')
BODY="{\"name\":\"冒烟\",\"group_id\":\"g\",\"channel_id\":\"c1\",\"start_time\":\"$START\",\"end_time\":\"$END\",\"format\":\"mkv\"}"
check "建任务 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/schedule" -H "$A" -H 'content-type: application/json' -d "$BODY")"

STATUS=""
FILE=""
for _ in $(seq 1 60); do
  sleep 1
  JSON=$(curl -s -H "$A" "$B/panel/api/schedule")
  STATUS=$(printf '%s' "$JSON" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d[0]["status"] if d else "")')
  FILE=$(printf '%s' "$JSON" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d[0]["output_file"] if d else "")')
  [ "$STATUS" = "finished" ] && break
done
check "任务完成" "finished" "$STATUS"
check "有产物" "yes" "$([ -n "$FILE" ] && echo yes || echo no)"
check "下载 200" "200" "$(curl -s -o "$WORK/out.bin" -w '%{http_code}' -H "$A" "$B/panel/api/schedule/download/$FILE")"
check "产物非空" "yes" "$([ -s "$WORK/out.bin" ] && echo yes || echo no)"

if [ "$fail" = "0" ]; then echo "录制冒烟测试全部通过"; else echo "录制冒烟测试有失败项"; tail -5 "$WORK/server.log"; exit 1; fi
