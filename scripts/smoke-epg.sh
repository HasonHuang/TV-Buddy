#!/usr/bin/env bash
# EPG 冒烟测试：启动采集、三件公开产物、面板状态与手动刷新。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
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
check "EPG 生成者" "1" "$(grep -c 'generator-info-name="tv-buddy"' "$WORK/body")"
check "EPG 含匹配频道" "1" "$(grep -c '<channel id="c1">' "$WORK/body")"
check "EPG 含窗口内节目" "1" "$(grep -c '新闻' "$WORK/body")"
check "EPG 过滤旧节目" "0" "$(grep -c '老节目' "$WORK/body")"

check "EPG gzip 200" "200" "$(curl -s -o "$WORK/body.gz" -w '%{http_code}' "$B/epg.xml.gz")"
check "EPG gzip 类型" "application/gzip" "$(header "$B/epg.xml.gz" content-type)"
check "EPG gzip 魔数" "1f8b" "$(od -An -tx1 -N2 "$WORK/body.gz" | tr -d ' ')"

check "EPG 浏览页 200" "200" "$(curl -s -o "$WORK/page.html" -w '%{http_code}' "$B/epg")"
check "EPG 浏览页含前端资源" "1" "$(grep -c '/assets/epg-' "$WORK/page.html")"

check "EPG 频道接口 200" "200" "$(curl -s -o "$WORK/channels.json" -w '%{http_code}' "$B/epg/api/channels")"
# 公开 EPG API 的字段以 F-023 为准：`channels` 用 `total`，`programmes`/`search` 没有 `count`。
check "EPG 频道接口计数" "1" "$(python3 scripts/epg_assert.py "$WORK/channels.json" channels)"
check "EPG 节目接口 200" "200" "$(curl -s -o "$WORK/prog.json" -w '%{http_code}' "$B/epg/api/programmes/c1")"
check "EPG 节目含标题" "1" "$(python3 scripts/epg_assert.py "$WORK/prog.json" programmes)"
curl -s "$B/epg/api/search?q=%E6%96%B0%E9%97%BB" > "$WORK/search.json"
check "EPG 搜索命中" "1" "$(python3 scripts/epg_assert.py "$WORK/search.json" search)"

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

# --- 采集节奏与翻译设置（A5） ---
A="Authorization: Bearer $TOKEN"

check "cron 预览合法" "True" \
  "$(curl -s -X POST "$B/panel/api/epg/cron-preview" -H "$A" -H 'content-type: application/json' \
    -d '{"expr":"*/30 * * * *"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["ok"])')"
check "cron 预览返回 3 个时间点" "3" \
  "$(curl -s -X POST "$B/panel/api/epg/cron-preview" -H "$A" -H 'content-type: application/json' \
    -d '{"expr":"*/30 * * * *"}' | python3 -c 'import json,sys;print(len(json.load(sys.stdin).get("next") or []))')"
check "cron 非法表达式不通过" "False" \
  "$(curl -s -X POST "$B/panel/api/epg/cron-preview" -H "$A" -H 'content-type: application/json' \
    -d '{"expr":"not a cron"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["ok"])')"

check "翻译状态 200" "200" \
  "$(curl -s -o "$WORK/tr.json" -w '%{http_code}' -H "$A" "$B/panel/api/epg/translate/status")"
check "翻译默认关闭" "False" \
  "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["enabled"])' "$WORK/tr.json")"
check "翻译状态有 cache 计数" "0" \
  "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["cache"])' "$WORK/tr.json")"

# 改写 `[epg.translate]` 后回读（面板翻译卡片就是这么保存的）。
curl -s -H "$A" "$B/panel/api/config" > "$WORK/cfg.json"
python3 - "$WORK/cfg.json" "$WORK/put.json" <<'PYEOF'
import json, sys
cfg = json.load(open(sys.argv[1]))
# 夹具里没有 `[epg.translate]`（它落在 EpgCfg.extra 里），面板保存时也是从空表开始建。
tr = cfg["epg"].setdefault("translate", {})
tr["enabled"] = True
tr["mtran_url"] = "http://127.0.0.1:9"
tr["mtran_batch"] = 25
tr["targets"] = ["zh-cn", "en"]
cfg["epg"]["cron"] = "*/30 * * * *"
json.dump(cfg, open(sys.argv[2], "w"))
PYEOF
check "写翻译设置 200" "200" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$B/panel/api/config" -H "$A" \
    -H 'content-type: application/json' --data-binary "@$WORK/put.json")"
curl -s -H "$A" "$B/panel/api/config" > "$WORK/after.json"
check "翻译已启用" "True" \
  "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["epg"]["translate"]["enabled"])' "$WORK/after.json")"
check "批量大小已落盘" "25" \
  "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["epg"]["translate"]["mtran_batch"])' "$WORK/after.json")"
check "多目标已落盘" "zh-cn,en" \
  "$(python3 -c 'import json,sys;print(",".join(json.load(open(sys.argv[1]))["epg"]["translate"]["targets"]))' "$WORK/after.json")"
check "cron 已落盘" "*/30 * * * *" \
  "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["epg"]["cron"])' "$WORK/after.json")"

# 不可达的 MTranServer：测试接口要报告失败而不是 5xx。
check "MTran 不可达时报告失败" "False" \
  "$(curl -s -X POST "$B/panel/api/epg/translate/mtran/test" -H "$A" \
    -H 'content-type: application/json' -d '{"text":"hello"}' | \
    python3 -c 'import json,sys;print(json.load(sys.stdin).get("ok"))')"

# 前端已内嵌这两张卡片（ASCII needle）。
ASSET=$(curl -s -H "$A" "$B/panel" | grep -oE '/__assets/assets/panel-[A-Za-z0-9_.-]+\.js' | head -1)
PJS=$(curl -s "$B$ASSET")
for needle in 'epg/cron-preview' 'epg/translate/run' 'epg/translate/clear-engine' \
  'epg/translate/mtran/test' 'mtran_max_per_round'; do
  case "$PJS" in
    *"$needle"*) printf 'ok   %s\n' "EPG 页包含 $needle" ;;
    *) printf 'FAIL EPG 页缺少 %s\n' "$needle"; fail=1 ;;
  esac
done

if [ "$fail" = "0" ]; then
  echo "EPG 冒烟测试全部通过"
else
  echo "EPG 冒烟测试有失败项"
  exit 1
fi
