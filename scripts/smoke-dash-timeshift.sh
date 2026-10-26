#!/usr/bin/env bash
# DASH 时移冒烟：按轨道录制 MPD 切片 → 回看清单 → 切片。
#
# 目标契约（2026-10-26 实测）见 `src/dash_timeshift.rs` 顶部注释。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19940}
UPORT=${UPORT:-19941}
WORK=$(mktemp -d)

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "跳过：未安装 ffmpeg"
  exit 0
fi
if ! ffmpeg -hide_banner -muxers 2>/dev/null | awk '{print $2}' | grep -qx dash; then
  echo "跳过：当前 ffmpeg 未编译 DASH 复用器"
  exit 0
fi

ffmpeg -loglevel error -f lavfi -i testsrc=size=320x240:rate=25 -t 16 \
  -c:v libx264 -preset ultrafast -g 50 -f dash -seg_duration 2 \
  -use_template 1 -use_timeline 1 "$WORK/out.mpd"
python3 -m http.server "$UPORT" --bind 127.0.0.1 --directory "$WORK" >"$WORK/up.log" 2>&1 &
UPID=$!

cat > "$WORK/config.toml" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"

[[groups]]
id = "dg"
name = "DASH 组"
mode = "dash"
access = "open"
[[groups.channels]]
id = "c1"
name = "DASH 频道"
url = "http://127.0.0.1:$UPORT/out.mpd"
dash_timeshift = true
dash_timeshift_hours = 4
TOML

"$BIN" "$WORK/config.toml" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID $UPID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 60); do curl -fsS "$B/health" >/dev/null 2>&1 && break; sleep 0.1; done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }

NOW=$(date +%s)
SEEK="$(date -r $((NOW - 900)) +%Y%m%d%H%M%S)-$(date -r $((NOW + 60)) +%Y%m%d%H%M%S)"
LIST="$B/dg/c1/dash-timeshift/video_240p/playlist.txt?playseek=$SEEK"

CODE=$(curl -s -o "$WORK/list.m3u8" -w '%{http_code}' "$LIST")
check "时移清单 200" "200" "$CODE"
check "VOD 类型" "1" "$(grep -c '#EXT-X-PLAYLIST-TYPE:VOD' "$WORK/list.m3u8" || true)"
check "ENDLIST" "1" "$(grep -c '#EXT-X-ENDLIST' "$WORK/list.m3u8" || true)"
check "PROGRAM-DATE-TIME" "1" "$(grep -c '#EXT-X-PROGRAM-DATE-TIME:' "$WORK/list.m3u8" || true)"
check "EXT-X-MAP" "1" "$(grep -c '#EXT-X-MAP:URI=' "$WORK/list.m3u8" || true)"

SEG=$(grep -o '/dg/c1/dash-timeshift/video_240p/[0-9]*[.][a-z]*' "$WORK/list.m3u8" | head -1)
check "切片 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' "$B$SEG")"
check "切片缓存头" "1" "$(curl -s -o /dev/null -D - "$B$SEG" | tr -d '\r' | grep -ci '^cache-control: private, max-age=3600')"
check "切片类型为图片" "1" "$(curl -s -o /dev/null -D - "$B$SEG" | tr -d '\r' | grep -ci '^content-type: image/')"

check "缺 playseek 400" "400" "$(curl -s -o /dev/null -w '%{http_code}' "$B/dg/c1/dash-timeshift/video_240p/playlist.txt")"
check "格式错 400" "400" "$(curl -s -o /dev/null -w '%{http_code}' "$B/dg/c1/dash-timeshift/video_240p/playlist.txt?playseek=abc")"
check "不重叠 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/dg/c1/dash-timeshift/video_240p/playlist.txt?playseek=20200101000000-20200101000100")"
check "未知轨道 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/dg/c1/dash-timeshift/nosuch/playlist.txt?playseek=$SEEK")"
check "未知切片 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/dg/c1/dash-timeshift/video_240p/999.jpg")"

if [ "$fail" = "0" ]; then echo "DASH 时移冒烟测试全部通过"; else echo "DASH 时移冒烟测试有失败项"; tail -20 "$WORK/server.log"; exit 1; fi
