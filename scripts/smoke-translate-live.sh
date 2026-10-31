#!/usr/bin/env bash
# 真实 MTranServer 联调：字幕翻译、自检、EPG 探测。
# 需要外网，默认使用 https://translate.ipacel.cc/translate。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
case "$BIN" in
    /*) ;;
    *) BIN="$PWD/$BIN" ;;
esac
URL=${TRANSLATE_URL:-https://translate.ipacel.cc/translate}
TOKEN=${TRANSLATE_TOKEN:-}
[ -x "$BIN" ] || {
    echo "缺少 $BIN；先运行 cargo build"
    exit 1
}

WORK=$(mktemp -d)
PORT=$((22000 + ($$ % 500) * 2))
UP=$((PORT + 1))
mkdir -p "$WORK/cache/subtitles" "$WORK/up"
printf '1\n00:00:01,000 --> 00:00:03,000\nHello world\n' >"$WORK/cache/subtitles/live.srt"

python3 - "$WORK/up/epg.xml" <<'PY'
import sys, time
now = int(time.time())
def fmt(ts):
    return time.strftime("%Y%m%d%H%M%S +0000", time.gmtime(ts))
xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="live">
  <channel id="c1"><display-name lang="zh">测试频道</display-name></channel>
  <programme start="{fmt(now - 1800)}" stop="{fmt(now + 1800)}" channel="c1">
    <title>Hello world</title>
  </programme>
</tv>
"""
open(sys.argv[1], "w", encoding="utf-8").write(xml)
PY

{
    printf '%s\n' '{'
    printf '  "server": { "bind": "127.0.0.1:%s" },\n' "$PORT"
    printf '  "cache": { "disk_path": "%s/cache" },\n' "$WORK"
    printf '%s\n' '  "epg": {'
    printf '%s\n' '    "enabled": true, "keep_days": 3, "future_days": 1, "keep_yesterday": false,'
    printf '%s\n' '    "sources": ['
    printf '%s\n' '      { "id": "s1", "name": "本地源", "kind": "xml",'
    printf '        "url": "http://127.0.0.1:%s/epg.xml" }\n' "$UP"
    printf '%s\n' '    ],'
    printf '%s\n' '    "translate": {'
    printf '%s\n' '      "enabled": true, "target": "zh-cn", "source": "auto",'
    printf '      "url": "%s"' "$URL"
    [ -n "$TOKEN" ] && printf ', "mtran_token": "%s"' "$TOKEN"
    printf '\n    }\n  },\n'
    printf '%s\n' '  "subtitle": {'
    printf '%s\n' '    "translate": {'
    printf '%s\n' '      "enabled": true, "target": "zh-cn", "source": "auto",'
    printf '      "url": "%s"' "$URL"
    [ -n "$TOKEN" ] && printf ', "token": "%s"' "$TOKEN"
    printf '\n    }\n  },\n'
    printf '%s\n' '  "groups": ['
    printf '%s\n' '    { "id": "g1", "name": "分组", "mode": "proxy", "access": "public",'
    printf '%s\n' '      "channels": ['
    printf '%s\n' '        { "id": "c1", "name": "测试频道", "url": "http://127.0.0.1:9/live.m3u8" }'
    printf '%s\n' '      ] }'
    printf '%s\n' '  ]'
    printf '%s\n' '}'
} >"$WORK/config.json"

cleanup() {
    if [ -f "$WORK/pid" ]; then
        kill "$(cat "$WORK/pid")" 2>/dev/null || true
    fi
    if [ -f "$WORK/up.pid" ]; then
        kill "$(cat "$WORK/up.pid")" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT HUP INT TERM

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
echo $! >"$WORK/up.pid"
for _ in $(seq 1 50); do
    curl -fsS --max-time 2 "http://127.0.0.1:$UP/epg.xml" >/dev/null 2>&1 && break
    sleep 0.1
done

"$BIN" config import "$WORK/config.json" --db "$WORK/tv-buddy.db" >/dev/null || exit 1
(
    cd "$WORK" || exit 1
    exec "$BIN" tv-buddy.db >server.log 2>&1
) &
echo $! >"$WORK/pid"
for _ in $(seq 1 20); do
    curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 && break
    sleep 1
done
curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" >/dev/null || {
    cat "$WORK/server.log"
    exit 1
}

echo "==> MTranServer 自检：$URL"
curl -sS --max-time 90 -X POST "http://127.0.0.1:$PORT/subtitle/mtran/test" \
    -H 'Content-Type: application/json' --data '{"text":"Hello world"}' >"$WORK/test.json"
python3 - "$WORK/test.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
print(json.dumps(body, ensure_ascii=False))
if not body.get("ok") or not body.get("result"):
    raise SystemExit("MTranServer 自检失败")
PY

TOKEN_JSON=$(curl -fsS --max-time 10 -X POST "http://127.0.0.1:$PORT/panel/api/login" \
    -H 'Content-Type: application/json' --data '{"username":"admin","password":"admin"}')
SESSION=$(printf '%s' "$TOKEN_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')

echo "==> EPG 采集与翻译"
for _ in $(seq 1 90); do
    [ "$(curl -s -o "$WORK/epg.xml" -w '%{http_code}' "http://127.0.0.1:$PORT/epg.xml")" = "200" ] && break
    sleep 0.5
done
grep -q '你好' "$WORK/epg.xml" || {
    cat "$WORK/epg.xml"
    exit 1
}
grep -q '你好' "$WORK/cache/epg/.translate/cache.json" || {
    cat "$WORK/cache/epg/.translate/cache.json" 2>/dev/null || true
    exit 1
}
echo "EPG 已生成译文，缓存已写入"

curl -fsS --max-time 90 -X POST "http://127.0.0.1:$PORT/epg/translate/run" \
    -H "Authorization: Bearer $SESSION" >"$WORK/run.json"
python3 - "$WORK/run.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
print(json.dumps(body, ensure_ascii=False))
if not body.get("ok") or body.get("translated", 0) < 1:
    raise SystemExit("EPG 翻译运行失败")
PY

echo "==> EPG 翻译探测"
curl -fsS --max-time 90 -X POST "http://127.0.0.1:$PORT/epg/translate/probe" \
    -H "Authorization: Bearer $SESSION" >"$WORK/probe.json"
python3 - "$WORK/probe.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
print(json.dumps(body, ensure_ascii=False))
if not body.get("ok"):
    raise SystemExit("EPG 翻译探测失败")
PY

echo "==> 字幕翻译"
curl -fsS --max-time 90 -X POST "http://127.0.0.1:$PORT/panel/api/subtitle/translate" \
    -H "Authorization: Bearer $SESSION" -H 'Content-Type: application/json' \
    --data '{"file":"live.srt"}' >"$WORK/result.json"
DOWNLOAD=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["download"])' "$WORK/result.json")
curl -fsS --max-time 10 "http://127.0.0.1:$PORT$DOWNLOAD" \
    -H "Authorization: Bearer $SESSION" >"$WORK/output.srt"
grep -q '你好' "$WORK/output.srt" || {
    cat "$WORK/output.srt"
    exit 1
}
echo "==> 输出"
cat "$WORK/output.srt"
echo "真实翻译联调通过"
