#!/usr/bin/env bash
# 用户功能冒烟：多用户、可见范围、每用户订阅与 EPG、面板分权、全局订阅停用。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19931}
UP=${UP:-19932}
WORK=$(mktemp -d)
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"

mkdir -p "$WORK/up"
# 节目时间用「现在」，否则落在 `keep_days` / `future_days` 窗口外会被裁掉。
python3 - "$WORK/up/epg.xml" <<'PY'
import sys, time

now = int(time.time())
start = time.strftime("%Y%m%d%H%M%S", time.gmtime(now - 3600)) + " +0000"
stop = time.strftime("%Y%m%d%H%M%S", time.gmtime(now + 3600)) + " +0000"
body = f"""<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="up">
  <channel id="alpha"><display-name lang="zh">频道甲</display-name></channel>
  <channel id="beta"><display-name lang="zh">频道乙</display-name></channel>
  <programme start="{start}" stop="{stop}" channel="alpha">
    <title lang="zh">甲的节目</title>
  </programme>
  <programme start="{start}" stop="{stop}" channel="beta">
    <title lang="zh">乙的节目</title>
  </programme>
</tv>
"""
open(sys.argv[1], "w", encoding="utf-8").write(body)
PY

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
UP_PID=$!
trap 'kill ${UP_PID:-0} ${PID:-0} 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$UP/epg.xml" >/dev/null 2>&1 && break
  sleep 0.1
done

cat > "$CFG" <<JSON
{
  "server": { "bind": "127.0.0.1:$PORT", "log_level": "warn" },
  "auth": { "token_enabled": true, "token_failure_url": "http://placeholder.test/Failure.mp4" },
  "cache": { "disk_path": "$WORK/cache" },
  "epg": {
    "enabled": true,
    "public_viewer_enabled": false,
    "keep_days": 3,
    "future_days": 365,
    "keep_yesterday": false,
    "match_by_name": false,
    "sources": [{
      "id": "s1",
      "name": "本地源",
      "kind": "xml",
      "url": "http://127.0.0.1:$UP/epg.xml"
    }]
  },
  "groups": [
    {
      "id": "ga",
      "name": "甲组",
      "mode": "proxy",
      "access": "public",
      "channels": [
        { "id": "alpha", "name": "频道甲", "tvg_id": "alpha", "url": "http://127.0.0.1:9/a.m3u8" }
      ]
    },
    {
      "id": "gb",
      "name": "乙组",
      "mode": "proxy",
      "access": "public",
      "channels": [
        { "id": "beta", "name": "频道乙", "tvg_id": "beta", "url": "http://127.0.0.1:9/b.m3u8" }
      ]
    },
    {
      "id": "gz",
      "name": "归档组",
      "mode": "proxy",
      "access": "archived",
      "channels": [
        { "id": "gamma", "name": "频道丙", "url": "http://127.0.0.1:9/c.m3u8" }
      ]
    }
  ]
}
JSON

"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
PID=$!
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 60); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }
code() { curl -s -o "$WORK/body" -w '%{http_code}' "$@"; }
jget() { python3 -c "import sys,json;d=json.load(open('$WORK/body'));print($1)"; }

# --- 管理员登录 ---
AT=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
A=(-H "Authorization: Bearer $AT" -H 'Content-Type: application/json')
check "管理员登录" "admin" "$(curl -s "${A[@]}" "$B/panel/api/me" | python3 -c 'import sys,json;print(json.load(sys.stdin)["user"]["role"])')"

# --- 建普通用户：只看 ga，且只勾 alpha ---
BODY='{"username":"viewer","password":"viewer123","role":"user","allowed_group_ids":["ga"],"channel_filter_enabled":true,"allowed_channel_ids":["ga/alpha"],"web_ui_access":true}'
check "建用户 200" "200" "$(code -X POST "${A[@]}" -d "$BODY" "$B/panel/api/users")"
check "建用户返回 role" "user" "$(jget 'd["user"]["role"]')"
UID_=$(jget 'd["user"]["id"]')

check "普通用户至少一个分组" "400" \
  "$(code -X POST "${A[@]}" -d '{"username":"nogroup","password":"x123","role":"user","allowed_group_ids":[]}' "$B/panel/api/users")"
check "重名用户 400" "400" \
  "$(code -X POST "${A[@]}" -d '{"username":"viewer","password":"x123","role":"user","allowed_group_ids":["ga"]}' "$B/panel/api/users")"

# --- 普通用户登录 ---
VT=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"viewer","password":"viewer123"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
V=(-H "Authorization: Bearer $VT" -H 'Content-Type: application/json')
check "普通用户登录" "user" "$(curl -s "${V[@]}" "$B/panel/api/me" | python3 -c 'import sys,json;print(json.load(sys.stdin)["user"]["role"])')"

# --- 面板分权 ---
check "普通用户读配置 403" "403" "$(code "${V[@]}" "$B/panel/api/config")"
check "普通用户改配置 403" "403" "$(code -X PUT "${V[@]}" -d '{}' "$B/panel/api/config")"
check "普通用户列分组 403" "403" "$(code "${V[@]}" "$B/panel/api/groups/export")"
check "普通用户列用户 403" "403" "$(code "${V[@]}" "$B/panel/api/users")"
check "普通用户重载 403" "403" "$(code -X POST "${V[@]}" "$B/panel/api/reload")"
check "普通用户看自己的订阅 200" "200" "$(code "${V[@]}" "$B/panel/api/me")"

# --- 每用户订阅 ---
PTOK=$(jget 'd["token"]' < <(curl -s "${V[@]}" "$B/panel/api/me"))
VSUB="$B/sub/$PTOK/playlist.m3u"
curl -s "$VSUB" > "$WORK/vpl.m3u"
check "订阅含授权频道" "1" "$(grep -c '/ga/alpha.m3u8?token=' "$WORK/vpl.m3u" || true)"
check "订阅不含未授权分组" "0" "$(grep -c '/gb/' "$WORK/vpl.m3u" || true)"
check "订阅不含归档组" "0" "$(grep -c '/gz/' "$WORK/vpl.m3u" || true)"
check "订阅条目数" "1" "$(grep -c '^#EXTINF' "$WORK/vpl.m3u" || true)"
check "订阅头指向自己的 EPG" \
  "#EXTM3U x-tvg-url=\"$B/sub/$PTOK/epg.xml.gz\"" "$(head -n1 "$WORK/vpl.m3u")"

# 管理员看到全部非归档频道。
ATOK=$(curl -s "${A[@]}" "$B/panel/api/me" | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
curl -s "$B/sub/$ATOK/playlist.m3u" > "$WORK/apl.m3u"
check "管理员订阅 2 条" "2" "$(grep -c '^#EXTINF' "$WORK/apl.m3u" || true)"
check "管理员仍不含归档组" "0" "$(grep -c '/gz/' "$WORK/apl.m3u" || true)"

# --- 每用户 EPG ---
curl -s -X POST "${A[@]}" "$B/panel/api/epg/refresh" >/dev/null || true
sleep 1
check "管理员 EPG 200" "200" "$(code "$B/sub/$ATOK/epg.xml")"
curl -s "$B/sub/$ATOK/epg.xml" > "$WORK/aepg.xml"
check "管理员 EPG 含两个频道" "2" "$(grep -c '<channel id=' "$WORK/aepg.xml" || true)"
check "用户 EPG 200" "200" "$(code "$B/sub/$PTOK/epg.xml")"
cp "$WORK/body" "$WORK/vepg.xml"
check "用户 EPG 只含授权频道" "1" "$(grep -c '<channel id=' "$WORK/vepg.xml" || true)"
check "用户 EPG 含 alpha" "1" "$(grep -c '<channel id="alpha"' "$WORK/vepg.xml" || true)"
check "用户 EPG 不含 beta" "0" "$(grep -c 'beta' "$WORK/vepg.xml" || true)"
check "用户 EPG gz 200" "200" "$(code "$B/sub/$PTOK/epg.xml.gz")"
check "用户 EPG gz 是 gzip" "1" \
  "$(python3 -c 'import sys;d=open(sys.argv[1],"rb").read(2);print(1 if d==b"\x1f\x8b" else 0)' "$WORK/body")"

# --- 全局订阅停用 ---
check "全局订阅 404" "404" "$(code "$B/playlist.m3u")"
check "全局分组订阅 404" "404" "$(code "$B/ga/playlist.m3u")"
check "全局订阅给出去向" "1" "$(grep -c '/sub/' "$WORK/body" || true)"
check "坏令牌订阅 404" "404" "$(code "$B/sub/nope/playlist.m3u")"
check "坏令牌 EPG 404" "404" "$(code "$B/sub/nope/epg.xml")"

# --- 令牌管理 ---
check "建设备令牌 200" "200" \
  "$(code -X POST "${A[@]}" -d '{"label":"电视","ttl_secs":3600}' "$B/panel/api/users/$UID_/tokens")"
DTOK=$(jget 'd["token"]["token"]')
check "设备令牌用途" "device" "$(jget 'd["token"]["purpose"]')"
check "设备令牌可订阅" "200" "$(code "$B/sub/$DTOK/playlist.m3u")"
check "撤销设备令牌 200" "200" "$(code -X DELETE "${A[@]}" "$B/panel/api/users/$UID_/tokens/$DTOK")"
check "撤销后订阅 404" "404" "$(code "$B/sub/$DTOK/playlist.m3u")"

# --- 改密踢会话 ---
check "重置口令 200" "200" \
  "$(code -X PUT "${A[@]}" -d '{"password":"newpass123"}' "$B/panel/api/users/$UID_/password")"
check "改密后旧会话失效" "401" "$(code "${V[@]}" "$B/panel/api/me")"
check "新口令可登录" "200" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
     -d '{"username":"viewer","password":"newpass123"}' "$B/panel/api/login")"

# --- 不能删最后一个管理员 ---
ADMIN_ID=$(curl -s "${A[@]}" "$B/panel/api/users" \
  | python3 -c 'import sys,json;print([u["id"] for u in json.load(sys.stdin)["users"] if u["role"]=="admin"][0])')
check "删最后管理员 400" "400" "$(code -X DELETE "${A[@]}" "$B/panel/api/users/$ADMIN_ID")"

# --- 删用户连带删令牌 ---
check "删普通用户 200" "200" "$(code -X DELETE "${A[@]}" "$B/panel/api/users/$UID_")"
check "删用户后其令牌失效" "404" "$(code "$B/sub/$PTOK/playlist.m3u")"

if [ "$fail" = "0" ]; then echo "用户功能冒烟测试全部通过"; else echo "用户功能冒烟测试有失败项"; exit 1; fi
