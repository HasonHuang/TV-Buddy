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

# --- 访问控制卡片（A3）：`[auth]` 段可读可写，且写回后键面齐全 ---
curl -s -H "$AUTH" "$B/panel/api/config" > "$WORK/cfg.json"
check "配置含 [auth]" "1" \
  "$(python3 -c 'import json,sys;print(1 if "auth" in json.load(open(sys.argv[1])) else 0)' "$WORK/cfg.json")"
check "auth 有 23 个键" "1" \
  "$(python3 -c '
import json,sys
want = {
  "token_enabled","token_ttl_hours","token_failure_url","random_id_enabled","ip_guard_enabled",
  "max_sessions_per_ip","session_ttl_secs","limit_url","ban_url","whitelist",
  "l1_window_secs","l1_limit","l2_window_secs","l2_limit","cooldown_secs",
  "violation_window_secs","violation_decay_secs","ban_threshold","ban_duration_secs",
  "auth_fail_window_secs","auth_fail_free_per_window","auth_fail_ban_threshold",
  "auth_fail_ban_duration_secs","auth_fail_ban_repeat_duration_secs",
  "auth_fail_ban_escalate_window_secs",
}
auth = json.load(open(sys.argv[1]))["auth"]
missing = want - set(auth)
print(1 if not missing else 0)
' "$WORK/cfg.json")"
check "未消费键的默认值与目标一致" "1" \
  "$(python3 -c '
import json,sys
auth = json.load(open(sys.argv[1]))["auth"]
expect = {"l1_window_secs":30,"l1_limit":30,"l2_window_secs":30,"l2_limit":8,
          "cooldown_secs":3,"violation_window_secs":300,"violation_decay_secs":1800,
          "ban_threshold":3,"ban_duration_secs":2592000,
          "auth_fail_ban_duration_secs":3600,
          "auth_fail_ban_repeat_duration_secs":2592000,
          "auth_fail_ban_escalate_window_secs":86400}
print(1 if all(auth.get(k) == v for k,v in expect.items()) else 0)
' "$WORK/cfg.json")"

# 写白名单 + 改 IP 管控开关，回读确认落盘（面板卡片就是这么做的）。
python3 - "$WORK/cfg.json" "$WORK/put.json" <<'PYEOF'
import json, sys
cfg = json.load(open(sys.argv[1]))
cfg["auth"]["whitelist"] = ["203.0.113.7", "10.0.0.0/8", "*"]
cfg["auth"]["ip_guard_enabled"] = True
json.dump(cfg, open(sys.argv[2], "w"))
PYEOF
check "写白名单 PUT 200" "200" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$B/panel/api/config" -H "$AUTH" \
    -H 'content-type: application/json' --data-binary "@$WORK/put.json")"
curl -s -H "$AUTH" "$B/panel/api/config" > "$WORK/after.json"
check "白名单已落盘" "3" \
  "$(python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))["auth"]["whitelist"]))' "$WORK/after.json")"
check "IP 管控已开启" "True" \
  "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["auth"]["ip_guard_enabled"])' "$WORK/after.json")"
check "开启 IP 管控后 /unban/whoami 可用" "True" \
  "$(curl -s -H 'X-Forwarded-For: 203.0.113.7' "$B/unban/whoami" | \
    python3 -c 'import json,sys;print(json.load(sys.stdin).get("enabled"))')"

# 前端已内嵌访问控制卡片（ASCII needle：esbuild 会转义中文）。
ASSET=$(curl -s "$B/panel" | grep -oE '/__assets/assets/panel-[A-Za-z0-9_.-]+\.js' | head -1)
PJS=$(curl -s "$B$ASSET")
for needle in 'auth_fail_window_secs' 'auth_fail_ban_escalate_window_secs' \
  'max_sessions_per_ip' 'violation_decay_secs' 'ip_guard_enabled'; do
  case "$PJS" in
    *"$needle"*) printf 'ok   %s\n' "安全页包含 $needle" ;;
    *) printf 'FAIL 安全页缺少 %s\n' "$needle"; fail=1 ;;
  esac
done

if [ "$fail" = "0" ]; then echo "安全面冒烟测试全部通过"; else echo "安全面冒烟测试有失败项"; exit 1; fi
