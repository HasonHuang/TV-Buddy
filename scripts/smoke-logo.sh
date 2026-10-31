#!/usr/bin/env bash
# 台标库冒烟：上传、公开取图与列表、删除。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19918}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"
DB="$WORK/tv-buddy.db"

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
[[groups.channels]]
id = "c1"
name = "频道"
url = "http://127.0.0.1:9/none.m3u8"
TOML

"$BIN" import-config "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
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
check "页面含资源" "yes" "$(grep -q '/assets/logo-' "$WORK/page.html" && echo yes || echo no)"

DEL=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/delete" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"file":"cctv1.png"}')
check "删除 200" "200" "$DEL"
check "删除后 404" "404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/logo/cctv1.png")"

# 第二个台标：上传后未使用。
BODY2="{\"items\":[{\"name\":\"hunan.png\",\"data\":\"data:image/png;base64,$PNG\"}]}"
STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/upload" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d "$BODY2")
check "上传第二个 200" "200" "$STATUS"

# 前一段已删除 cctv1.png；重新上传以便应用与重命名。
STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/upload" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d "$BODY")
check "重传第一个 200" "200" "$STATUS"

# 应用到频道 g1/c1。
APPLY=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/apply" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"gid":"g1","cid":"c1","file":"cctv1.png"}')
check "应用台标 200" "200" "$APPLY"
check "订阅含台标" "1" "$(curl -s "$B/playlist.m3u" | grep -c 'tvg-logo="/logo/cctv1.png"' || true)"

# 匹配报告：hunan.png 未使用。
curl -s -H "Authorization: Bearer $TOKEN" "$B/panel/api/logo/match-report" > "$WORK/report.json"
check "报告未使用 1" "1" "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["unused"])' "$WORK/report.json")"

# 重命名为中文名。
RENAME=$(curl -s -o "$WORK/rename.json" -w '%{http_code}' -X POST "$B/panel/api/logo/rename" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"file":"cctv1.png","name":"央视一套"}')
check "重命名 200" "200" "$RENAME"
check "新文件名" "央视一套.png" "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["logo"]["file"])' "$WORK/rename.json")"
check "新文件可取" "200" "$(curl -s -o /dev/null -w '%{http_code}' "$B/logo/%E5%A4%AE%E8%A7%86%E4%B8%80%E5%A5%97.png")"

# 推荐接口。
SUGGEST=$(curl -s -o "$WORK/suggest.json" -w '%{http_code}' -X POST "$B/panel/api/logo/suggest" \
  -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"threshold":0.1}')
check "推荐 200" "200" "$SUGGEST"

# --- 动效与还原（A2）：`/panel/api/logo/animate` 输出固定 10 帧无损动画 WebP ---
# 用 ASCII 名，避免 URL 编码干扰断言。
UP2="{\"items\":[{\"name\":\"anim.png\",\"ext\":\"png\",\"data\":\"data:image/png;base64,$PNG\"}]}"
check "上传待动效台标 200" "200" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/upload" \
    -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d "$UP2")"

check "动效 200" "200" \
  "$(curl -s -o "$WORK/anim.json" -w '%{http_code}' -X POST "$B/panel/api/logo/animate" \
    -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d '{"file":"anim.png","effect":"breathe","quality":"80"}')"
check "动效 10 帧" "10" "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["frames"])' "$WORK/anim.json")"
check "动效总时长 2220ms" "2220" \
  "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["total_ms"])' "$WORK/anim.json")"
check "动效后体积变大" "1" \
  "$(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(1 if d["bytes_after"]>d["bytes_before"] else 0)' "$WORK/anim.json")"
check "动效已备份原图" "1" \
  "$(curl -s -H "Authorization: Bearer $TOKEN" "$B/panel/api/logo/list" | \
    python3 -c 'import json,sys;print(1 if any(l["file"]=="anim.png" and l["has_original"] for l in json.load(sys.stdin)["logos"]) else 0)')"

check "未知动效 400" "400" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/animate" \
    -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d '{"file":"anim.png","effect":"nope"}')"
check "缺 file 400" "400" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/animate" \
    -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"effect":"breathe"}')"
# `quality` 必须是字符串：整数会被拒（S-LOGO-03 的目标行为）。
check "整数 quality 400" "400" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/animate" \
    -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d '{"file":"anim.png","effect":"breathe","quality":80}')"

check "还原 200" "200" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/logo/restore" \
    -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"file":"anim.png"}')"
# `.orig/` 备份保留在磁盘上（还原是复制回来，不删备份），所以 `has_original` 仍为 true。
# 断言点改成「文件内容回到原图大小」。
check "还原回原图大小" "70" \
  "$(curl -s -H "Authorization: Bearer $TOKEN" "$B/panel/api/logo/list" | \
    python3 -c 'import json,sys;print(next(l["bytes"] for l in json.load(sys.stdin)["logos"] if l["file"]=="anim.png"))')"

# --- 前端接线（A2）：动效与浏览器侧转码参数已内嵌 ---
# 台标页的实现在 modulepreload 的第二个 chunk（入口只做挂载），所以把所有 logo-*.js 拼起来查。
check "台标页引用 logo 资源" "yes" \
  "$(curl -s "$B/logo" | grep -q '/__assets/assets/logo-' && echo yes || echo no)"
LJS=$(for asset in $(curl -s "$B/logo" | grep -oE '/__assets/assets/logo-[A-Za-z0-9_.-]+\.js'); do
  curl -s "$B$asset"
done)
for needle in 'logo/animate' 'logo/restore' 'toBlob' 'image/webp'; do
  case "$LJS" in
    *"$needle"*) printf 'ok   %s\n' "台标页包含 $needle" ;;
    *) printf 'FAIL 台标页缺少 %s\n' "$needle"; fail=1 ;;
  esac
done

if [ "$fail" = "0" ]; then echo "台标库冒烟测试全部通过"; else echo "台标库冒烟测试有失败项"; exit 1; fi
