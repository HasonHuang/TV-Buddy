#!/usr/bin/env bash
# DASH 录制冒烟：动态 MPD → 轨道切片 → ffmpeg 并轨 → MKV 可下载。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19970}
UPORT=${UPORT:-19971}
WORK=$(mktemp -d)
UP_PID=
PID=
trap 'kill ${PID:-} ${UP_PID:-} 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "跳过：未安装 ffmpeg"
  exit 0
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "跳过：未安装 python3"
  exit 0
fi

ffmpeg -y -loglevel error \
  -f lavfi -i testsrc=size=320x240:rate=25 \
  -f lavfi -i sine=frequency=440:sample_rate=48000 \
  -t 12 -c:v libx264 -preset ultrafast -g 25 -c:a aac \
  -f dash -seg_duration 2 -use_template 1 -use_timeline 1 "$WORK/live.mpd"

python3 - "$WORK/live.mpd" <<'PY'
import pathlib
import re
import sys
import time

path = pathlib.Path(sys.argv[1])
text = path.read_text()
text = text.replace('type="static"', 'type="dynamic"', 1)
text = re.sub(r'\s*mediaPresentationDuration="[^"]*"', '', text, count=1)
now = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())
text = text.replace(
    'minBufferTime="PT4.0S">',
    'minBufferTime="PT4.0S"\n'
    f'\tavailabilityStartTime="{now}"\n'
    f'\tpublishTime="{now}"\n'
    '\tminimumUpdatePeriod="PT2S"\n'
    '\ttimeShiftBufferDepth="PT1M">',
    1,
)
path.write_text(text)
PY

python3 -m http.server "$UPORT" --bind 127.0.0.1 --directory "$WORK" >"$WORK/up.log" 2>&1 &
UP_PID=$!

cat > "$WORK/config.toml" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"
timezone_offset_hours = 8

[cache]
disk_path = "$WORK/cache"

[[groups]]
id = "dh"
name = "DASH 组"
mode = "dash"
access = "open"
format = "mpd"
[[groups.channels]]
id = "c1"
name = "DASH 录制频道"
url = "http://127.0.0.1:$UPORT/live.mpd"
TOML

"$BIN" "$WORK/config.toml" >"$WORK/server.log" 2>&1 &
PID=$!
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 80); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

SESSION=$(curl -fsS -X POST "$B/panel/api/login" \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')

read -r START END < <(python3 - <<'PY'
import time
now = int(time.time()) + 8 * 3600
fmt = lambda value: time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(value))
print(fmt(now - 2), fmt(now + 28))
PY
)

curl -fsS -X POST "$B/panel/api/schedule" \
  -H "authorization: Bearer $SESSION" \
  -H 'content-type: application/json' \
  -d "{\"name\":\"dash smoke\",\"group_id\":\"dh\",\"channel_id\":\"c1\",\"start_time\":\"$START\",\"end_time\":\"$END\",\"format\":\"mkv\"}" \
  >"$WORK/add.json"

STATUS=
FILE=
for _ in $(seq 1 100); do
  curl -fsS "$B/panel/api/schedule" -H "authorization: Bearer $SESSION" >"$WORK/list.json"
  read -r STATUS FILE < <(python3 - "$WORK/list.json" <<'PY'
import json
import sys
task = json.load(open(sys.argv[1]))[0]
print(task.get("status", ""), task.get("output_file", ""))
PY
)
  case "$STATUS" in
    finished|failed) break ;;
  esac
  sleep 1
done

if [ "$STATUS" != "finished" ]; then
  echo "FAIL DASH 录制状态：${STATUS:-timeout}"
  cat "$WORK/list.json" 2>/dev/null || true
  cat "$WORK/server.log" 2>/dev/null || true
  exit 1
fi

curl -fsS "$B/panel/api/schedule/download/$FILE" \
  -H "authorization: Bearer $SESSION" -o "$WORK/out.mkv"
if [ ! -s "$WORK/out.mkv" ]; then
  echo "FAIL DASH 产物为空"
  exit 1
fi
if command -v ffprobe >/dev/null 2>&1; then
  DURATION=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$WORK/out.mkv")
  if ! python3 - "$DURATION" <<'PY'
import sys
sys.exit(0 if float(sys.argv[1]) > 0 else 1)
PY
  then
    echo "FAIL DASH 产物时长：$DURATION"
    exit 1
  fi
fi
printf 'ok   DASH 录制：%s（%s 字节）\n' "$FILE" "$(wc -c <"$WORK/out.mkv" | tr -d ' ')"
