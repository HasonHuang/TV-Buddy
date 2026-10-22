#!/usr/bin/env bash
# EPG 冒烟测试：启动采集、三件公开产物、面板状态与手动刷新。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19916}
UP=${UP:-19917}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"
mkdir -p "$WORK/up"

python3 - "$WORK/up/epg.xml" <<'PY'
import sys, time
now = int(time.time())

def fmt(ts):
    return time.strftime("%Y%m%d%H%M%S +0000", time.gmtime(ts))

xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="src">
  <channel id="cctv1"><display-name lang="zh">测试频道</display-name></channel>
  <programme start="{fmt(now - 1800)}" stop="{fmt(now + 1800)}" channel="cctv1">
    <title lang="zh">新闻</title>
  </programme>
  <programme start="19900101000000 +0000" stop="19900101010000 +0000" channel="cctv1">
    <title>老节目</title>
  </programme>
</tv>
"""
open(sys.argv[1], "w", encoding="utf-8").write(xml)
PY

cat > "$CFG" <<EOF
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"

[epg]
enabled = true
keep_days = 3
future_days = 1
keep_yesterday = false

[[epg.sources]]
id = "s1"
name = "本地源"
kind = "xml"
url = "http://127.0.0.1:$UP/epg.xml"

[[groups]]
id = "g1"
name = "分组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c1"
name = "测试频道"
url = "http://127.0.0.1:$UP/live.m3u8"
EOF

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
UP_PID=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$UP/epg.xml" >/dev/null 2>&1 && break
  sleep 0.1
done

"$BIN" "$CFG" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID $UP_PID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT

B="http://127.0.0.1:$PORT"
for _ in $(seq 1 60); do
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
header() { curl -s -o /dev/null -D - "$1" | awk -v k="$2" 'BEGIN{IGNORECASE=1}index($0,k":")==1{sub(/\r$/,"");print substr($0,length(k)+3)}' ; }

# 启动采集是异步的，等产物出现。
for _ in $(seq 1 60); do
  [ "$(curl -s -o /dev/null -w '%{http_code}' "$B/epg.xml")" = "200" ] && break
  sleep 0.2
done

check "EPG XML 200" "200" "$(curl -s -o "$WORK/body" -w '%{http_code}' "$B/epg.xml")"
check "EPG XML 类型" "application/xml; charset=utf-8" "$(header "$B/epg.xml" content-type)"
check "EPG 生成者" "1" "$(grep -c 'generator-info-name="iptv-proxy"' "$WORK/body")"
check "EPG 含匹配频道" "1" "$(grep -c '<channel id="c1">' "$WORK/body")"
check "EPG 含窗口内节目" "1" "$(grep -c '新闻' "$WORK/body")"
check "EPG 过滤旧节目" "0" "$(grep -c '老节目' "$WORK/body")"

check "EPG gzip 200" "200" "$(curl -s -o "$WORK/body.gz" -w '%{http_code}' "$B/epg.xml.gz")"
check "EPG gzip 类型" "application/gzip" "$(header "$B/epg.xml.gz" content-type)"
check "EPG gzip 魔数" "1f8b" "$(od -An -tx1 -N2 "$WORK/body.gz" | tr -d ' ')"

check "EPG 浏览页 200" "200" "$(curl -s -o "$WORK/page.html" -w '%{http_code}' "$B/epg")"
check "EPG 浏览页含前端资源" "1" "$(grep -c '/assets/epg-' "$WORK/page.html")"

check "EPG 频道接口 200" "200" "$(curl -s -o "$WORK/channels.json" -w '%{http_code}' "$B/epg/api/channels")"
check "EPG 频道接口计数" "1" "$(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(1 if d["count"]>=1 and d["channels"][0]["id"]=="c1" else 0)' "$WORK/channels.json")"
check "EPG 节目接口 200" "200" "$(curl -s -o "$WORK/prog.json" -w '%{http_code}' "$B/epg/api/programmes/c1")"
check "EPG 节目含标题" "1" "$(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(1 if d["count"]==1 and d["programmes"][0]["title"]=="新闻" else 0)' "$WORK/prog.json")"
check "EPG 搜索命中" "1" "$(curl -s "$B/epg/api/search?q=%E6%96%B0%E9%97%BB" | python3 -c 'import json,sys;print(1 if json.load(sys.stdin)["count"]==1 else 0)')"

COOKIE=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' -D - -o "$WORK/login.json" | awk 'BEGIN{IGNORECASE=1}/^set-cookie:/{sub(/\r$/,"");print substr($0,13)}')
TOKEN=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("token",""))' "$WORK/login.json")

check "EPG 状态 200" "200" "$(curl -s -o "$WORK/status.json" -w '%{http_code}' -H "Authorization: Bearer $TOKEN" "$B/panel/api/epg/status")"
check "EPG 状态已匹配频道" "1" "$(python3 -c 'import json,sys;print(1 if json.load(open(sys.argv[1]))["channels"]>=1 else 0)' "$WORK/status.json")"
check "EPG 刷新 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $TOKEN" "$B/panel/api/epg/refresh")"

check "EPG 映射设置 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"gid":"g1","cid":"c1","source_id":"s1","source_channel_id":"cctv1"}' "$B/panel/api/epg/mapping")"
check "EPG 刷新后映射" "1" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $TOKEN" "$B/panel/api/epg/refresh" >/dev/null; curl -s -H "Authorization: Bearer $TOKEN" "$B/panel/api/epg/mappings" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(1 if d["mappings"]["g1/c1"]["source_id"]=="s1" else 0)')"
check "EPG 匹配报告 200" "200" "$(curl -s -o "$WORK/report.txt" -w '%{http_code}' -H "Authorization: Bearer $TOKEN" "$B/panel/api/epg/match-report")"
check "EPG 报告含 manual" "1" "$(grep -c 'manual' "$WORK/report.txt")"
check "EPG 映射清除 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"gid":"g1","cid":"c1","source_id":"","source_channel_id":""}' "$B/panel/api/epg/mapping")"

if [ "$fail" = "0" ]; then
  echo "EPG 冒烟测试全部通过"
else
  echo "EPG 冒烟测试有失败项"
  exit 1
fi
