#!/usr/bin/env bash
# 台标库冒烟：上传、公开取图与列表、删除。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19918}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"

cat > "$CFG" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[cache]
disk_path = "$WORK/cache"

[[groups]]
id = "g1"
name = "分组"
mode = "proxy"
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
header() { curl -s -o /dev/null -D - "$1" | awk -v k="$2" 'BEGIN{IGNORECASE=1}index($0,k":")==1{sub(/\r$/,"");print substr($0,length(k)+3)}'; }

check "初始列表为空" "0" "$(curl -s "$B/logo/api/list" | python3 -c 'import json,sys;print(json.load(sys.stdin)["count"])')"

TOKEN=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))')

PNG='iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='
BODY="{\"items\":[{\"name\":\"cctv1.png\",\"data\":\"data:image/png;base64,$PNG\"}]}"
STATUS=$(curl -s -o "$WORK/up.json" -w '%{http_code}' -X POST "$B/panel/api/logo/upload" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d "$BODY")
check "上传 200" "200" "$STATUS"
check "上传落库" "cctv1.png" "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["added"][0]["file"])' "$WORK/up.json")"

check "取图 200" "200" "$(curl -s -o "$WORK/logo.png" -w '%{http_code}' "$B/logo/cctv1.png")"
check "取图类型" "image/png" "$(header "$B/logo/cctv1.png" content-type)"
check "取图缓存头" "public, max-age=86400" "$(header "$B/logo/cctv1.png" cache-control)"
check "取图魔数" "89504e47" "$(od -An -tx1 -N4 "$WORK/logo.png" | tr -d ' ')"
check "列表计数" "1" "$(curl -s "$B/logo/api/list" | python3 -c 'import json,sys;print(json.load(sys.stdin)["count"])')"
check "缺文件 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/logo/missing.png")"
check "越界名 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/logo/..%2F..%2Fconfig.toml")"
check "越界扩展 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/logo/evil.exe")"

check "台标页 200" "200" "$(curl -s -o "$WORK/page.html" -w '%{http_code}' "$B/logo")"
check "页面含资源" "1" "$(grep -c '/assets/logo-' "$WORK/page.html")"

DEL=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/delete" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"file":"cctv1.png"}')
check "删除 200" "200" "$DEL"
check "删除后 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/logo/cctv1.png")"

if [ "$fail" = "0" ]; then echo "台标库冒烟测试全部通过"; else echo "台标库冒烟测试有失败项"; exit 1; fi
