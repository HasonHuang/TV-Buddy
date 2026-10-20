#!/usr/bin/env bash
# 安全面冒烟：Token 查看/轮换、IP 统计、封禁与解封。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19920}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"

cat > "$CFG" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"

[auth]
token_enabled = true

[[groups]]
id = "g1"
name = "分组"
mode = "proxy"
[[groups.channels]]
id = "c1"
name = "频道"
url = "http://127.0.0.1:9/none.m3u8"
TOML

"$BIN" "$CFG" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"

for _ in $(seq 1 60); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }

TOKEN=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))')
AUTH="Authorization: Bearer $TOKEN"

curl -s -H "$AUTH" "$B/panel/api/security/token" > "$WORK/token.json"
check "Token 长度 48" "48" "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))["current"]))' "$WORK/token.json")"
FIRST=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["current"])' "$WORK/token.json")

curl -s -X POST -H "$AUTH" "$B/panel/api/security/token/rotate" > "$WORK/rotate.json"
SECOND=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["token"])' "$WORK/rotate.json")
check "轮换换新值" "1" "$([ "$FIRST" != "$SECOND" ] && echo 1 || echo 0)"
check "订阅含新 Token" "1" "$(curl -s "$B/playlist.m3u" | grep -c "$SECOND" || true)"

check "公开请求 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 7.7.7.7' "$B/health")"
check "封禁 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "$AUTH" -H 'content-type: application/json' \
  -d '{"ip":"7.7.7.7","seconds":60}' "$B/panel/api/security/ip/ban")"
check "封禁后 403" "403" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 7.7.7.7' "$B/playlist.m3u")"
check "面板不受封禁" "401" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 7.7.7.7' "$B/panel/api/status")"

curl -s -H "$AUTH" "$B/panel/api/security/ip/stats" > "$WORK/stats.json"
check "统计含封禁项" "1" "$(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(1 if any(e["ip"]=="7.7.7.7" and e["banned"] for e in d["entries"]) else 0)' "$WORK/stats.json")"

check "解封 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "$AUTH" -H 'content-type: application/json' \
  -d '{"ip":"7.7.7.7"}' "$B/panel/api/security/ip/unban")"
check "解封后 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Forwarded-For: 7.7.7.7' "$B/playlist.m3u")"

# 批量解封。
for ip in 8.8.8.8 9.9.9.9; do
  curl -s -o /dev/null -X POST -H "$AUTH" -H 'content-type: application/json' \
    -d "{\"ip\":\"$ip\",\"seconds\":60}" "$B/panel/api/security/ip/ban"
done
curl -s -o "$WORK/batch.json" -X POST -H "$AUTH" -H 'content-type: application/json' \
  -d '{"ips":["8.8.8.8","9.9.9.9"]}' "$B/panel/api/security/ip/unban-batch"
check "批量解封 2 个" "2" "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["removed"])' "$WORK/batch.json")"

if [ "$fail" = "0" ]; then echo "安全面冒烟测试全部通过"; else echo "安全面冒烟测试有失败项"; exit 1; fi
