#!/usr/bin/env bash
# M0 冒烟测试：健康检查、登录、状态、配置读写、鉴权、兜底。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19911}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"
DB="$WORK/tv-buddy.db"

cat > "$CFG" <<EOF
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[[groups]]
id = "test1"
name = "test1"
mode = "proxy"

[[groups.channels]]
id = "c1"
name = "C1"
url = "http://example.invalid/x.m3u8"

[epg]
enabled = true
cron = "* * * * *"
EOF

"$BIN" import-config "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" &
PID=$!
trap 'kill $PID 2>/dev/null || true; wait $PID 2>/dev/null || true; rm -rf "$WORK"' EXIT

B="http://127.0.0.1:$PORT"
for _ in $(seq 1 50); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

fail=0
check() { # 名称 期望 实际
  if [ "$2" = "$3" ]; then
    printf 'ok   %s\n' "$1"
  else
    printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"
    fail=1
  fi
}

code=$(curl -s -o "$WORK/health" -w '%{http_code}' "$B/health")
check "健康检查 200 ok" "200 ok" "$code $(cat "$WORK/health")"

code=$(curl -s -o /dev/null -w '%{http_code}' "$B/panel/api/status")
check "未登录拒绝" "401" "$code"

code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/login" \
  -H 'Content-Type: application/json' -d '{"username":"admin","password":"bad"}')
check "错误口令拒绝" "401" "$code"

T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
check "登录返回 token" "1" "$([ -n "$T" ] && echo 1 || echo 0)"

V=$(curl -s -H "Authorization: Bearer $T" "$B/panel/api/status" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("version",""))')
check "状态版本" "0.1.0" "$V"

code=$(curl -s -o /dev/null -w '%{http_code}' -H "X-Token: $T" "$B/panel/api/status")
check "X-Token 可用" "200" "$code"

C=$(curl -s -H "Authorization: Bearer $T" "$B/panel/api/config")
GID=$(printf '%s' "$C" | python3 -c 'import sys,json;print(json.load(sys.stdin)["groups"][0]["id"])')
check "配置含分组" "test1" "$GID"

printf '%s' "$C" | python3 -c '
import json, sys
cfg = json.load(sys.stdin)
cfg["server"]["log_level"] = "debug"
print(json.dumps(cfg))
' > "$WORK/put.json"

code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$B/panel/api/config" \
  -H "Authorization: Bearer $T" -H 'Content-Type: application/json' --data-binary "@$WORK/put.json")
check "配置保存成功" "200" "$code"
check "改动已入数据库" "debug" "$("$BIN" config get server.log_level --db "$DB")"
check "未建模段保留" "* * * * *" "$("$BIN" config get epg.cron --db "$DB")"

code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$B/panel/api/config" \
  -H "Authorization: Bearer $T" -H 'Content-Type: application/json' -d '{"groups":"bad"}')
check "非法配置拒绝" "400" "$code"

code=$(curl -s -o /dev/null -w '%{http_code}' "$B/panel")
check "面板页 200" "200" "$code"

code=$(curl -s -o /dev/null -w '%{http_code}' "$B/definitely/missing")
check "未知路径 302" "302" "$code"

code=$(curl -s -o /dev/null -w '%{http_code}' "$B/panel/api/nope")
check "未知接口 404" "404" "$code"

printf '[server]\nbind = "127.0.0.1:1"\nbind = "127.0.0.1:2"\n' > "$WORK/bad.toml"
if "$BIN" import-config "$WORK/bad.toml" --db "$WORK/bad.db" >/dev/null 2>&1; then
  printf 'FAIL 重复键未报错\n'
  fail=1
else
  printf 'ok   重复键被拒绝\n'
fi

# 旧的 TOML 路径不能再当启动参数。
if "$BIN" "$WORK/bad.toml" >/dev/null 2>&1; then
  printf 'FAIL 旧配置路径未被拒绝\n'
  fail=1
else
  printf 'ok   旧配置路径被拒绝\n'
fi

if [ "$fail" = "0" ]; then
  printf 'M0 冒烟测试全部通过\n'
else
  printf 'M0 冒烟测试有失败项\n'
  exit 1
fi
