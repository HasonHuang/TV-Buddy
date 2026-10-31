#!/usr/bin/env bash
# M1 冒烟测试：分组/频道 CRUD、订阅导出、access 过滤、播放 Token。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19912}
WORK=$(mktemp -d)
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"

cat > "$CFG" <<JSON
{
  "server": {
    "bind": "127.0.0.1:$PORT",
    "log_level": "warn",
    "m3u_header_lines": [
      "#EXTVLCOPT:http-user-agent=ReaTest/1.0"
    ]
  },
  "auth": {
    "token_enabled": false
  },
  "groups": [
    {
      "id": "pub",
      "name": "公开组",
      "mode": "proxy",
      "access": "public",
      "channels": [
        {
          "id": "c1",
          "name": "频道一",
          "url": "http://example.invalid/a.m3u8"
        }
      ]
    },
    {
      "id": "hid",
      "name": "隐藏组",
      "mode": "proxy",
      "access": "hidden",
      "channels": [
        {
          "id": "c2",
          "name": "频道二",
          "url": "http://example.invalid/b.m3u8"
        }
      ]
    },
    {
      "id": "op",
      "name": "开放组",
      "mode": "proxy",
      "access": "open",
      "channels": [
        {
          "id": "c3",
          "name": "频道三",
          "url": "http://example.invalid/c.m3u8"
        }
      ]
    },
    {
      "id": "arc",
      "name": "归档组",
      "mode": "proxy",
      "access": "archived",
      "channels": [
        {
          "id": "c4",
          "name": "频道四",
          "url": "http://example.invalid/d.m3u8"
        }
      ]
    },
    {
      "id": "pass",
      "name": "直通组",
      "mode": "passthrough",
      "access": "public",
      "channels": [
        {
          "id": "c5",
          "name": "频道五",
          "url": "http://example.invalid/live.m3u8"
        }
      ]
    }
  ],
  "epg": {
    "enabled": true,
    "cron": "* * * * *"
  }
}
JSON

"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
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
code() { curl -s -o "$WORK/body" -w '%{http_code}' "$@"; }
jget() { python3 -c "import sys,json;d=json.load(open('$WORK/body'));print($1)"; }

T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
A=(-H "Authorization: Bearer $T" -H 'Content-Type: application/json')

# --- 导出 ---
check "导出总表 200" "200" "$(code "${A[@]}" "$B/panel/api/groups/export")"
check "导出总表是数组" "5" "$(jget 'len(d)')"
check "导出总表首项 id" "pub" "$(jget 'd[0]["id"]')"
check "单组导出 200" "200" "$(code "${A[@]}" "$B/panel/api/groups/pub/export")"
check "单组导出 id" "pub" "$(jget 'd["id"]')"
check "缺组导出 404" "404" "$(code "${A[@]}" "$B/panel/api/groups/ghost/export")"
check "缺组导出错误串" "Group not found" "$(jget 'd["error"]')"

# --- 分组 CRUD ---
check "未登录拒绝建组" "401" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/groups" \
    -H 'Content-Type: application/json' -d '{"id":"x"}')"
check "新建分组 200" "200" \
  "$(code -X POST "${A[@]}" -d '{"id":"new1","name":"新组","mode":"proxy","access":"hidden"}' "$B/panel/api/groups")"
check "重复 id 409" "409" \
  "$(code -X POST "${A[@]}" -d '{"id":"new1"}' "$B/panel/api/groups")"
check "重复 id 错误串" "Group id already exists" "$(jget 'd["error"]')"
check "落库含新组" "1" "$("$BIN" config get groups --db "$DB" | grep -c '"id":"new1"' || true)"
check "空 id 400" "400" \
  "$(code -X POST "${A[@]}" -d '{"name":"无 id"}' "$B/panel/api/groups")"

check "局部改名 200" "200" \
  "$(code -X PUT "${A[@]}" -d '{"name":"改名后"}' "$B/panel/api/groups/new1")"
code "${A[@]}" "$B/panel/api/groups/new1/export" >/dev/null
check "改名生效" "改名后" "$(jget 'd["name"]')"
check "改名后 mode 保留" "proxy" "$(jget 'd["mode"]')"
check "改缺组 404" "404" \
  "$(code -X PUT "${A[@]}" -d '{"name":"x"}' "$B/panel/api/groups/ghost")"
check "改缺组错误串" "Group not found" "$(jget 'd["error"]')"

# --- 频道 CRUD ---
check "新建频道 200" "200" \
  "$(code -X POST "${A[@]}" -d '{"id":"n1","name":"新频道","url":"http://example.invalid/n1.m3u8"}' \
    "$B/panel/api/groups/new1/channels")"
check "重复频道 409" "409" \
  "$(code -X POST "${A[@]}" -d '{"id":"n1"}' "$B/panel/api/groups/new1/channels")"
check "重复频道错误串" "Channel id already exists" "$(jget 'd["error"]')"
check "局部改频道 200" "200" \
  "$(code -X PUT "${A[@]}" -d '{"name":"改名频道"}' "$B/panel/api/groups/new1/channels/n1")"
code "${A[@]}" "$B/panel/api/groups/new1/export" >/dev/null
check "改频道保 url" "http://example.invalid/n1.m3u8" "$(jget 'd["channels"][0]["url"]')"
check "改频道改名" "改名频道" "$(jget 'd["channels"][0]["name"]')"
check "改缺频道 404" "404" \
  "$(code -X PUT "${A[@]}" -d '{"name":"x"}' "$B/panel/api/groups/new1/channels/nope")"
check "改缺频道错误串" "Channel not found" "$(jget 'd["error"]')"
check "删缺频道 404" "404" \
  "$(code -X DELETE "${A[@]}" "$B/panel/api/groups/new1/channels/nope")"
check "删频道 200" "200" \
  "$(code -X DELETE "${A[@]}" "$B/panel/api/groups/new1/channels/n1")"
check "删组 200" "200" "$(code -X DELETE "${A[@]}" "$B/panel/api/groups/new1")"
check "再删组 404" "404" "$(code -X DELETE "${A[@]}" "$B/panel/api/groups/new1")"

# --- 总订阅 ---
check "总订阅 200" "200" "$(curl -s -o "$WORK/body" -w '%{http_code}' "$B/playlist.m3u")"
check "总订阅类型" "application/x-mpegurl; charset=utf-8" \
  "$(curl -s -o "$WORK/body" -w '%{content_type}' "$B/playlist.m3u")"
# `[epg].enabled = true` 时首行带 `x-tvg-url`（S-EPG-01），不是裸 `#EXTM3U`。
check "总订阅首行带 EPG 头" "#EXTM3U x-tvg-url=\"http://127.0.0.1:$PORT/epg.xml.gz\"" \
  "$(head -n1 "$WORK/body")"
check "总订阅头行" "#EXTVLCOPT:http-user-agent=ReaTest/1.0" "$(sed -n '2p' "$WORK/body")"
check "总订阅含公开组" "1" "$(grep -c "http://127.0.0.1:$PORT/pub/c1.m3u8" "$WORK/body")"
check "总订阅含直通组" "1" "$(grep -c 'http://example.invalid/live.m3u8' "$WORK/body")"
check "总订阅排隐藏组" "0" "$(grep -c '/hid/' "$WORK/body")"
check "总订阅排开放组" "0" "$(grep -c '/op/' "$WORK/body")"
check "总订阅排归档组" "0" "$(grep -c '/arc/' "$WORK/body")"
check "总订阅条目数" "2" "$(grep -c '^#EXTINF' "$WORK/body")"

# --- 分组订阅 ---
check "分组订阅 200" "200" "$(curl -s -o "$WORK/body" -w '%{http_code}' "$B/pub/playlist.m3u")"
check "分组订阅 EXTINF" "1" "$(grep -c '^#EXTINF:-1 tvg-id="c1"' "$WORK/body")"
check "归档组订阅 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/arc/playlist.m3u")"
check "开放组订阅 200" "200" "$(curl -s -o "$WORK/body" -w '%{http_code}' "$B/op/playlist.m3u")"
check "缺组订阅 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/ghost/playlist.m3u")"

# --- 播放 Token ---
curl -s "${A[@]}" "$B/panel/api/config" > "$WORK/cfg.json"
python3 -c '
import json
cfg = json.load(open("'"$WORK"'/cfg.json"))
cfg["auth"]["token_enabled"] = True
json.dump(cfg, open("'"$WORK"'/put.json", "w"))
'
check "开启 Token 200" "200" \
  "$(code -X PUT "${A[@]}" --data-binary "@$WORK/put.json" "$B/panel/api/config")"
curl -s -o "$WORK/body" "$B/pub/playlist.m3u"
# 全局播放令牌已废弃：公开订阅不再带 `?token=`。
# 可用地址只能从 `/sub/<设备令牌>/playlist.m3u` 获取。
check "订阅不再下发 Token" "0" "$(grep -c 'token=' "$WORK/body" || true)"
check "开放组不带 Token" "0" "$(curl -s "$B/op/playlist.m3u" | grep -c 'token=' || true)"
check "直通组不带 Token" "0" "$(curl -s "$B/playlist.m3u" | grep -c 'example.invalid/live.m3u8?token' || true)"

# --- 未建模段保留 ---
check "未建模段保留" "* * * * *" "$("$BIN" config get epg.cron --db "$DB")"

if [ "$fail" = "0" ]; then
  printf 'M1 冒烟测试全部通过\n'
else
  printf 'M1 冒烟测试有失败项\n'
  exit 1
fi
