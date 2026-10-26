#!/usr/bin/env bash
# 指标面冒烟：`/status` 字段、`online_viewers` 语义、`/cache/stats`、`/health` 条目。
#
# 目标契约见 `docs/findings/dynamic/11-security-and-metrics.md` §6.1。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19942}
UPORT=${UPORT:-19943}
WORK=$(mktemp -d)
UPID=""

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "跳过：未安装 ffmpeg（仅用于生成测试流）"
  exit 0
fi

ffmpeg -loglevel error -f lavfi -i testsrc=size=160x120:rate=10 -t 6 \
  -c:v libx264 -preset ultrafast -g 20 -f hls -hls_time 2 -hls_list_size 0 \
  -hls_segment_filename "$WORK/seg%d.ts" "$WORK/live.m3u8"
python3 -m http.server "$UPORT" --bind 127.0.0.1 --directory "$WORK" >"$WORK/up.log" 2>&1 &
UPID=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$UPORT/live.m3u8" >/dev/null 2>&1 && break
  sleep 0.1
done

cat > "$WORK/config.toml" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"

[health]
enabled = true
interval_secs = 10
timeout_secs = 2
concurrency = 2

[auth]
token_enabled = true

[[groups]]
id = "mx"
name = "指标组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c1"
name = "可用频道"
url = "http://127.0.0.1:$UPORT/live.m3u8"
[[groups.channels]]
id = "c2"
name = "连不通频道"
url = "http://127.0.0.1:1/dead.m3u8"
[[groups.channels]]
id = "c3"
name = "404 频道"
url = "http://127.0.0.1:$UPORT/missing.m3u8"
TOML

"$BIN" "$WORK/config.toml" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID $UPID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 60); do curl -fsS "$B/health" >/dev/null 2>&1 && break; sleep 0.1; done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }
py() { python3 -c "$1"; }

T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' | py 'import sys,json;print(json.load(sys.stdin)["token"])')
AUTH="Authorization: Bearer $T"

STATUS=$(curl -s -H "$AUTH" "$B/panel/api/status")
for field in uptime_secs cache_backend active_sessions online_viewers group_count channel_count \
  version mem_rss_mb cpu_seconds disk_cache_mb recording_mb low_resource_mode poll_interval_secs \
  traffic_in_bytes traffic_out_bytes whole_file_committed whole_file_budget whole_file_peak \
  whole_file_hits whole_file_fallbacks entry_exts cpu_cores; do
  check "status 有 $field" "1" "$(printf '%s' "$STATUS" | py "import sys,json;d=json.load(sys.stdin);print(1 if '$field' in d else 0)")"
done
check "traffic_in_bytes 恒 0" "0" "$(printf '%s' "$STATUS" | py 'import sys,json;print(json.load(sys.stdin)["traffic_in_bytes"])')"
check "whole_file_budget 512MB" "536870912" "$(printf '%s' "$STATUS" | py 'import sys,json;print(json.load(sys.stdin)["whole_file_budget"])')"
check "entry_exts 按分组" "1" "$(printf '%s' "$STATUS" | py 'import sys,json;print(1 if json.load(sys.stdin)["entry_exts"].get("mx")==".m3u8" else 0)')"
check "初始在线观众 0" "0" "$(printf '%s' "$STATUS" | py 'import sys,json;print(json.load(sys.stdin)["online_viewers"])')"

TK=$(curl -s -H "$AUTH" "$B/panel/api/security/token" | py 'import sys,json;d=json.load(sys.stdin);print(d.get("current_value") or d.get("current") or "")')
curl -s -o /dev/null "$B/mx/c1.m3u8?token=$TK"
check "播放后在线观众 1" "1" "$(curl -s -H "$AUTH" "$B/panel/api/status" | py 'import sys,json;print(json.load(sys.stdin)["online_viewers"])')"
curl -s -o /dev/null "$B/mx/c1.m3u8?token=$TK"
check "同 IP 重复不增" "1" "$(curl -s -H "$AUTH" "$B/panel/api/status" | py 'import sys,json;print(json.load(sys.stdin)["online_viewers"])')"
curl -s -o /dev/null -H 'X-Forwarded-For: 9.9.9.9' "$B/mx/c1.m3u8?token=$TK"
check "换 IP 增到 2" "2" "$(curl -s -H "$AUTH" "$B/panel/api/status" | py 'import sys,json;print(json.load(sys.stdin)["online_viewers"])')"

CACHE=$(curl -s -H "$AUTH" "$B/panel/api/cache/stats")
for field in backend disk_bytes m3u8_entries segment_entries segment_url_entries sub_url_entries \
  dash_disk_bytes timeshift_disk_bytes recording_disk_bytes livestream_disk_bytes; do
  check "cache/stats 有 $field" "1" "$(printf '%s' "$CACHE" | py "import sys,json;d=json.load(sys.stdin);print(1 if '$field' in d else 0)")"
done

# 探测跑满一轮：间隔 10 秒。
# 等探测跑出成功结果（首轮可能早于上游就绪）。
ENTRIES=""
for _ in $(seq 1 60); do
  ENTRIES=$(curl -s -H "$AUTH" "$B/panel/api/health")
  if [ "$(printf '%s' "$ENTRIES" | py 'import sys,json;d=json.load(sys.stdin).get("entries") or {};print(1 if (d.get("mx/c1") or {}).get("ok") else 0)')" = "1" ]; then
    break
  fi
  sleep 0.5
done
check "health 有 3 条" "3" "$(printf '%s' "$ENTRIES" | py 'import sys,json;print(len(json.load(sys.stdin).get("entries") or {}))')"
check "健康频道 ok" "True" "$(printf '%s' "$ENTRIES" | py 'import sys,json;print(json.load(sys.stdin)["entries"]["mx/c1"]["ok"])')"
check "连不通频道 not ok" "False" "$(printf '%s' "$ENTRIES" | py 'import sys,json;print(json.load(sys.stdin)["entries"]["mx/c2"]["ok"])')"
check "连不通 latency 为 null" "None" "$(printf '%s' "$ENTRIES" | py 'import sys,json;print(json.load(sys.stdin)["entries"]["mx/c2"]["latency_ms"])')"
check "连不通错误文案" "连接失败" "$(printf '%s' "$ENTRIES" | py 'import sys,json;print(json.load(sys.stdin)["entries"]["mx/c2"]["error"])')"
check "404 频道 http_status" "404" "$(printf '%s' "$ENTRIES" | py 'import sys,json;print(json.load(sys.stdin)["entries"]["mx/c3"]["http_status"])')"

if [ "$fail" = "0" ]; then echo "指标面冒烟测试全部通过"; else echo "指标面冒烟测试有失败项"; tail -20 "$WORK/server.log"; exit 1; fi
