#!/usr/bin/env bash
# 素材冒烟：ffmpeg 检测、视频与字幕上传/列表/下载/删除、解析脚本。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19925}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"

cat > "$CFG" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"
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
count() { python3 -c 'import json,sys;print(json.load(sys.stdin)["count"])'; }

TOKEN=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))')
AUTH="Authorization: Bearer $TOKEN"

check "未登录上传 401" "401" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/assets/upload" \
  -H 'content-type: application/json' -d '{"items":[]}')"

curl -s -H "$AUTH" "$B/panel/api/assets/ffmpeg-status" > "$WORK/ffmpeg.json"
check "ffmpeg 状态可读" "1" "$(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(1 if "installed" in d else 0)' "$WORK/ffmpeg.json")"

BODY='{"items":[{"name":"花絮.mp4","data":"data:video/mp4;base64,AAAA"}]}'
check "上传视频 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/assets/upload" \
  -H "$AUTH" -H 'content-type: application/json' -d "$BODY")"
check "视频列表 1" "1" "$(curl -s -H "$AUTH" "$B/panel/api/assets/list" | count)"
check "视频下载 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -H "$AUTH" "$B/panel/api/assets/raw/%E8%8A%B1%E7%B5%AE.mp4")"
check "视频类型" "video/mp4" "$(curl -s -o /dev/null -D - -H "$AUTH" "$B/panel/api/assets/raw/%E8%8A%B1%E7%B5%AE.mp4" | awk 'BEGIN{IGNORECASE=1}index($0,"content-type:")==1{sub(/\r$/,"");print substr($0,15)}')"

check "删除视频 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/assets/remove" \
  -H "$AUTH" -H 'content-type: application/json' -d '{"file":"花絮.mp4"}')"
check "删除后列表 0" "0" "$(curl -s -H "$AUTH" "$B/panel/api/assets/list" | count)"

SUB='{"items":[{"name":"第1集.srt","data":"data:application/x-subrip;base64,MSAwOjAwOjAw"}]}'
check "上传字幕 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/subtitle/upload" \
  -H "$AUTH" -H 'content-type: application/json' -d "$SUB")"
check "字幕列表 1" "1" "$(curl -s -H "$AUTH" "$B/panel/api/subtitle/list" | count)"

check "脚本保存 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/resolver/script" \
  -H "$AUTH" -H 'content-type: application/json' -d '{"name":"demo","content":"print(1)"}')"
check "脚本列表 1" "1" "$(curl -s -H "$AUTH" "$B/panel/api/resolver/scripts" | count)"
check "脚本删除 200" "200" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/resolver/remove" \
  -H "$AUTH" -H 'content-type: application/json' -d '{"name":"demo.py"}')"

if [ "$fail" = "0" ]; then echo "素材冒烟测试全部通过"; else echo "素材冒烟测试有失败项"; exit 1; fi
