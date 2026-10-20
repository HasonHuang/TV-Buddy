#!/usr/bin/env bash
# 录制任务与直播开关冒烟。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19934}
WORK=$(mktemp -d)

cat > "$WORK/config.toml" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

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
url = "http://127.0.0.1:1/x.m3u8"
TOML

"$BIN" "$WORK/config.toml" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 60); do curl -fsS "$B/health" >/dev/null 2>&1 && break; sleep 0.1; done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
count() { python3 -c 'import json,sys;d=json.load(sys.stdin);print(len(d) if isinstance(d,list) else d.get(sys.argv[1],""))' "${1:-}"; }

T=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')
A="Authorization: Bearer $T"
J='[{"name":"测试","group_id":"g","channel_id":"c1","start_time":"2099-01-01T00:00:00","end_time":"2099-01-01T00:10:00","format":"mkv"}]'

check "未登录列表 401" "401" "$(code "$B/panel/api/schedule")"
check "新增 200" "200" "$(code -X POST "$B/panel/api/schedule" -H "$A" -H 'content-type: application/json' -d "${J:1:${#J}-2}")"
check "列表 1" "1" "$(curl -s -H "$A" "$B/panel/api/schedule" | count)"
check "停止 200" "200" "$(code -X POST "$B/panel/api/schedule/0/stop" -H "$A")"
check "删除 200" "200" "$(code -X DELETE "$B/panel/api/schedule/0" -H "$A")"
check "下载缺失 404" "404" "$(code -H "$A" "$B/panel/api/schedule/download/none.mkv")"

check "点亮 200" "200" "$(code -X POST "$B/panel/api/livestream/g/c1/start" -H "$A")"
check "直播列表 1" "1" "$(curl -s -H "$A" "$B/panel/api/livestream" | count count)"
check "熄灭 200" "200" "$(code -X POST "$B/panel/api/livestream/g/c1/stop" -H "$A")"

if [ "$fail" = "0" ]; then echo "录制冒烟测试全部通过"; else echo "录制冒烟测试有失败项"; exit 1; fi
