#!/usr/bin/env bash
# 面板健康与 DASH 探测冒烟：/panel/api/health、dash-timeshift/status、dash/scan-url。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19926}
UPORT=${UPORT:-19927}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"

cat > "$WORK/manifest.mpd" <<'MPD'
<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT30S">
  <Period>
    <AdaptationSet contentType="video" maxWidth="1280" maxHeight="720">
      <Representation id="v0" codecs="avc1.4d401f" bandwidth="2500000"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="zh">
      <Representation id="a0" codecs="mp4a.40.2" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>
MPD

python3 -m http.server "$UPORT" --bind 127.0.0.1 --directory "$WORK" >"$WORK/upstream.log" 2>&1 &
UPID=$!

cat > "$CFG" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"
TOML

"$BIN" "$CFG" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill $PID $UPID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
B="http://127.0.0.1:$PORT"

for _ in $(seq 1 60); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }
field() { python3 -c 'import json,sys;print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }

TOKEN=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))')
AUTH="Authorization: Bearer $TOKEN"

check "健康 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -H "$AUTH" "$B/panel/api/health")"
check "健康 ok" "True" "$(curl -s -H "$AUTH" "$B/panel/api/health" | field ok)"
check "回看状态 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -H "$AUTH" "$B/panel/api/dash-timeshift/status")"
check "回看未启用" "False" "$(curl -s -H "$AUTH" "$B/panel/api/dash-timeshift/status" | field enabled)"

BODY="{\"url\":\"http://127.0.0.1:$UPORT/manifest.mpd\"}"
check "探测 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/dash/scan-url" \
  -H "$AUTH" -H 'content-type: application/json' -d "$BODY")"
check "轨道数 2" "2" "$(curl -s -X POST "$B/panel/api/dash/scan-url" -H "$AUTH" \
  -H 'content-type: application/json' -d "$BODY" | field count)"
check "非 HTTP 400" "400" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/dash/scan-url" \
  -H "$AUTH" -H 'content-type: application/json' -d '{"url":"file:///etc/passwd"}')"

if [ "$fail" = "0" ]; then echo "面板探测冒烟测试全部通过"; else echo "面板探测冒烟测试有失败项"; exit 1; fi
