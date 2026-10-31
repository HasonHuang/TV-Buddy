#!/usr/bin/env bash
# 分享冒烟：双重门禁、Token 同步、封禁汇总、follower 禁止本地轮换。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
MPORT=${MPORT:-19930}
FPORT=${FPORT:-19931}
WORK=$(mktemp -d)

cat > "$WORK/master.toml" <<TOML
[server]
bind = "127.0.0.1:$MPORT"
log_level = "warn"

[cache]
disk_path = "$WORK/master-cache"

[share]
enabled = true
role = "master"
key = "REAKEY"
trust_ips = ["127.0.0.1"]
poll_secs = 5
follower_urls = ["http://127.0.0.1:$FPORT"]
TOML

cat > "$WORK/follower.toml" <<TOML
[server]
bind = "127.0.0.1:$FPORT"
log_level = "warn"

[cache]
disk_path = "$WORK/follower-cache"

[share]
enabled = true
role = "follower"
key = "REAKEY"
trust_ips = ["127.0.0.1"]
poll_secs = 5
master_url = "http://127.0.0.1:$MPORT"
TOML

"$BIN" import-config "$WORK/master.toml" --db "$WORK/master.db" >/dev/null || exit 1
"$BIN" "$WORK/master.db" >"$WORK/master.log" 2>&1 &
MPID=$!
"$BIN" import-config "$WORK/follower.toml" --db "$WORK/follower.db" >/dev/null || exit 1
"$BIN" "$WORK/follower.db" >"$WORK/follower.log" 2>&1 &
FPID=$!
trap 'kill $MPID $FPID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT

M="http://127.0.0.1:$MPORT"
F="http://127.0.0.1:$FPORT"
for _ in $(seq 1 80); do
  curl -fsS "$M/health" >/dev/null 2>&1 && curl -fsS "$F/health" >/dev/null 2>&1 && break
  sleep 0.1
done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }
status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

MTOK=$(curl -s -X POST "$M/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))')
FTOK=$(curl -s -X POST "$F/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))')

check "缺 key 403" "403" "$(status "$M/api/share/pull")"
check "错 key 403" "403" "$(status "$M/api/share/pull?key=WRONG")"
check "正确 key 200" "200" "$(status "$M/api/share/pull?key=REAKEY")"
check "POST 405" "405" "$(status -X POST "$M/api/share/pull?key=REAKEY")"
# 全局播放令牌已废弃：轮换端点不存在。
check "follower 轮换端点已移除" "404" "$(status -X POST "$F/panel/api/security/token/rotate" -H "Authorization: Bearer $FTOK")"

# 封禁后等待一个周期（follower 启动延迟 5s + 轮询 5s）。
curl -s -X POST "$M/panel/api/security/ip/ban" -H "Authorization: Bearer $MTOK" \
  -H 'content-type: application/json' -d '{"ip":"203.0.113.9","seconds":3600}' >/dev/null

# 令牌不再跨实例同步：载荷里的 token 恒为 null。
check "载荷不含令牌" "1" "$(curl -s "$M/api/share/pull?key=REAKEY" | python3 -c 'import json,sys;print(1 if json.load(sys.stdin)["token"] is None else 0)')"

BAN=""
for _ in $(seq 1 40); do
  sleep 0.5
  BAN=$(curl -s "$F/panel/api/security/ip/stats" -H "Authorization: Bearer $FTOK" \
    | python3 -c 'import json,sys;d=json.load(sys.stdin);print(1 if any(e["ip"]=="203.0.113.9" for e in d["entries"]) else 0)')
  [ "$BAN" = "1" ] && break
done
check "封禁已同步" "1" "$BAN"

if [ "$fail" = "0" ]; then echo "分享冒烟测试全部通过"; else echo "分享冒烟测试有失败项"; exit 1; fi
