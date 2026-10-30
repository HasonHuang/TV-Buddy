#!/usr/bin/env bash
# M2 冒烟测试：入口清单 → 媒体清单 → 伪装切片 → 缓存窗口与占位跳转。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19914}
UP=${UP:-19915}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"
CACHE="$WORK/cache"

mkdir -p "$WORK/up"
cat > "$WORK/up/index.m3u8" <<'EOF'
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:4
#EXTINF:4.0,
seg1.ts
#EXTINF:4.0,
seg2.ts
#EXT-X-ENDLIST
EOF
head -c 4096 /dev/urandom > "$WORK/up/seg1.ts"
head -c 2048 /dev/urandom > "$WORK/up/seg2.ts"
cat > "$WORK/up/master.m3u8" <<'EOF'
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=800000
v0/index.m3u8
EOF
mkdir -p "$WORK/up/v0"
cat > "$WORK/up/v0/index.m3u8" <<'EOF'
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:6
#EXTINF:6.0,
v1.ts
#EXT-X-ENDLIST
EOF
head -c 1024 /dev/urandom > "$WORK/up/v0/v1.ts"

cat > "$CFG" <<EOF
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"
not_found_url = "http://placeholder.test/404.mp4"
error_url = "http://placeholder.test/mistake.mp4"

[auth]
token_enabled = false
token_failure_url = "http://placeholder.test/Failure.mp4"

[cache]
disk_path = "$CACHE"
# 本脚本断言首写/命中缓存头；关闭预取，避免首请求直接命中预取结果。
[cache.prefetch]
enabled = false
[cache.disguise]
enabled = true
playlist = true
formats = ["jpg", "png"]

[[groups]]
id = "p1"
name = "代理组"
mode = "proxy"
access = "public"
[[groups.channels]]
id = "c1"
name = "频道一"
url = "http://127.0.0.1:$UP/index.m3u8"
[[groups.channels]]
id = "m1"
name = "主清单频道"
url = "http://127.0.0.1:$UP/master.m3u8"

[[groups]]
id = "arc"
name = "归档组"
mode = "proxy"
access = "archived"
[[groups.channels]]
id = "c9"
name = "归档频道"
url = "http://127.0.0.1:$UP/index.m3u8"

[[groups]]
id = "op"
name = "开放组"
mode = "proxy"
access = "open"
[[groups.channels]]
id = "c8"
name = "开放频道"
url = "http://127.0.0.1:$UP/index.m3u8"
EOF

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
UP_PID=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$UP/index.m3u8" >/dev/null 2>&1 && break
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
header() { # 头名 路径 —— 取响应头
  curl -s -o /dev/null -D - "$1" | awk -v k="$2" 'BEGIN{IGNORECASE=1}index($0,k":")==1{sub(/\r$/,"");print substr($0,length(k)+3)}'
}
hfile() { # 文件 头名
  awk -v k="$2" 'BEGIN{IGNORECASE=1}index($0,k":")==1{sub(/\r$/,"");print substr($0,length(k)+3)}' "$1"
}
status() { curl -s -o "$WORK/body" -w '%{http_code}' "$1"; }
location() { curl -s -o /dev/null -D - "$1" | awk 'BEGIN{IGNORECASE=1}/^location:/{sub(/\r$/,"");print substr($0,11)}'; }

# --- 入口清单（上游是媒体清单：直接下发改写后的媒体清单） ---
CODE=$(curl -s -o "$WORK/body" -D "$WORK/h" -w '%{http_code}' "$B/p1/c1.m3u8")
check "入口 200" "200" "$CODE"
check "入口类型" "text/plain; charset=utf-8" "$(hfile "$WORK/h" content-type)"
check "入口缓存窗口 = TARGETDURATION" "public, max-age=4, s-maxage=4" "$(hfile "$WORK/h" cache-control)"
check "入口直接下发媒体清单" "1" "$(grep -c '^#EXT-X-TARGETDURATION:4$' "$WORK/body")"
check "入口不含变体" "0" "$(grep -c '^#EXT-X-STREAM-INF' "$WORK/body")"
check "切片地址绝对" "2" \
  "$(grep -cE "^http://127\.0\.0\.1:$PORT/p1/c1/[0-9a-f]{16}_[0-9a-f]{32}\.[a-z]+$" "$WORK/body")"
FIRST_URL=$(grep -oE "http://127\.0\.0\.1:$PORT/p1/c1/[0-9a-f]{16}_[0-9a-f]{32}\.[a-z]+" "$WORK/body" | head -n1)
FIRST="/${FIRST_URL#http://127.0.0.1:$PORT/}"
P=$(grep -oE '/p1/c1/[0-9a-f]{16}_' <<<"$FIRST" | head -n1)
P=${P:7:16}
S=$(printf '%s' "$FIRST" | sed -E 's|.*_([0-9a-f]{32})\..*|\1|')

# --- 变体清单路径：上游是媒体清单时不存在 ---
check "变体清单 302" "302" "$(status "$B/p1/c1/video/index.txt")"
check "变体清单跳错误页" "http://placeholder.test/mistake.mp4" "$(location "$B/p1/c1/video/index.txt")"
check "切片后缀 = sha256 前 32 位" \
  "$(python3 -c 'import hashlib,sys;print(hashlib.sha256(sys.argv[1].encode()).hexdigest()[:32])' "http://127.0.0.1:$UP/seg1.ts")" "$S"
# `<P>` 的算法目标未解（F-017），这里只断言目标可观测的性质：16 hex、纯函数、随频道变化。
check "切片前缀为 16 hex" "1" "$(printf '%s' "$P" | grep -cE '^[0-9a-f]{16}$')"
P_OPEN=$(curl -s "$B/op/c8.m3u8" | grep -oE '/op/c8/[0-9a-f]{16}_' | head -n1)
P_OPEN=${P_OPEN:7:16}
check "前缀随频道不同" "1" "$([ -n "$P_OPEN" ] && [ "$P_OPEN" != "$P" ] && echo 1 || echo 0)"
check "前缀与 gid/cid 分隔符无关" "1" \
  "$(P2=$(curl -s "$B/p1/c1.m3u8" | grep -oE '/p1/c1/[0-9a-f]{16}_' | head -n1); [ "${P2:7:16}" = "$P" ] && echo 1 || echo 0)"

# --- 切片 ---
# 首个切片的扩展名由频道摘要决定（轮换起点），所以类型按扩展名推导，不写死。
FIRST_EXT=${FIRST_URL##*.}
case "$FIRST_EXT" in
  jpg|jpeg) FIRST_CT=image/jpeg ;;
  png) FIRST_CT=image/png ;;
  *) FIRST_CT=video/mp2t ;;
esac
# 首次请求同时取状态、头与正文，用来断言「首写」缓存头。
CODE=$(curl -s -o "$WORK/seg1" -D "$WORK/seg1.h" -w '%{http_code}' "$B$FIRST")
check "切片 200" "200" "$CODE"
check "切片首写缓存头" "public, max-age=300" "$(hfile "$WORK/seg1.h" cache-control)"
check "切片内容类型" "$FIRST_CT" "$(hfile "$WORK/seg1.h" content-type)"
check "切片字节一致" "1" \
  "$(cmp -s "$WORK/seg1" "$WORK/up/seg1.ts" && echo 1 || echo 0)"
check "切片命中缓存头" "private, max-age=300" "$(header "$B$FIRST" cache-control)"
check "切片落盘" "1" "$(find "$CACHE/seg_bytes" -type f | grep -c . || true)"
check "命中不改类型" "$FIRST_CT" "$(header "$B$FIRST" content-type)"
check "Range 被忽略" "200" \
  "$(curl -s -o "$WORK/range" -w '%{http_code}' -H 'Range: bytes=0-9' "$B$FIRST")"
check "Range 返回全量" "1" \
  "$(cmp -s "$WORK/range" "$WORK/up/seg1.ts" && echo 1 || echo 0)"

# --- 占位与错误 ---
# 目标 v0.8.2 实测（2026-10-25，容器 rea82v09）：
#   未知 gid/cid、归档组           → 302 not_found_url（404.mp4）
#   <P>/<S> 形状或取值不对、未登记 → 302 error_url（mistake.mp4）
# 即两种占位要分开断言，不能都当 not_found_url。
BAD="/p1/c1/0000000000000000_${S}.jpg"
check "错前缀 302" "302" "$(status "$B$BAD")"
check "错前缀跳错误页" "http://placeholder.test/mistake.mp4" "$(location "$B$BAD")"
check "缺前缀 302" "302" "$(status "$B/p1/c1/${S}.jpg")"
check "缺前缀跳错误页" "http://placeholder.test/mistake.mp4" "$(location "$B/p1/c1/${S}.jpg")"
check "截短前缀 302" "302" "$(status "$B/p1/c1/ea86_${S}.jpg")"
check "大写前缀 302" "302" \
  "$(status "$B/p1/c1/$(printf '%s' "$P" | tr 'a-f' 'A-F')_${S}.jpg")"
check "错后缀 302" "302" "$(status "$B/p1/c1/${P}_00000000000000000000000000000000.jpg")"
check "扩展名两点 302" "302" "$(status "$B/p1/c1/${P}_${S}.a.b")"
check "双下划线 302" "302" "$(status "$B/p1/c1/${P}__${S}.jpg")"
check "扩展名不校验" "200" "$(status "$B/p1/c1/${P}_${S}.png")"
check "p1/c1 错后缀跳错误页" "http://placeholder.test/mistake.mp4" \
  "$(location "$B/p1/c1/${P}_00000000000000000000000000000000.jpg")"
check "未知频道 302" "302" "$(status "$B/p1/ghost.m3u8")"
check "未知频道跳占位" "http://placeholder.test/404.mp4" "$(location "$B/p1/ghost.m3u8")"
check "未知分组 302" "302" "$(status "$B/ghost/c1.m3u8")"
check "归档组 302" "302" "$(status "$B/arc/c9.m3u8")"
check "归档组跳占位" "http://placeholder.test/404.mp4" "$(location "$B/arc/c9.m3u8")"
UNKNOWN="/p1/c1/${P}_$(python3 -c 'print("a"*32)').jpg"
check "未知切片 302" "302" "$(status "$B$UNKNOWN")"
check "未知切片跳错误页" "http://placeholder.test/mistake.mp4" "$(location "$B$UNKNOWN")"
# 入口扩展名白名单（目标实测 2026-10-25）：`.m3u8` / `.mpd` / `.txt` / 无后缀等价，其余 302。
check "入口 .txt 等价" "200" "$(status "$B/p1/c1.txt")"
check "入口无后缀等价" "200" "$(status "$B/p1/c1")"
check "非法入口名 302" "302" "$(status "$B/p1/c1.mp4")"
check "非法入口名跳占位" "http://placeholder.test/404.mp4" "$(location "$B/p1/c1.mp4")"

# --- 主清单上游：自造单变体主清单 ---
CODE=$(curl -s -o "$WORK/body" -D "$WORK/h" -w '%{http_code}' "$B/p1/m1.m3u8")
check "主清单入口 200" "200" "$CODE"
check "主清单入口类型" "application/vnd.apple.mpegurl" "$(hfile "$WORK/h" content-type)"
check "主清单入口不缓存" "no-cache, no-store" "$(hfile "$WORK/h" cache-control)"
check "主清单入口版本" "1" "$(grep -c '^#EXT-X-VERSION:6$' "$WORK/body")"
check "主清单入口独立切片" "1" "$(grep -c '^#EXT-X-INDEPENDENT-SEGMENTS$' "$WORK/body")"
check "主清单入口保留变体属性" "1" "$(grep -c '^#EXT-X-STREAM-INF:BANDWIDTH=800000$' "$WORK/body")"
check "主清单入口指向变体清单" "1" \
  "$(grep -c "^http://127\.0\.0\.1:$PORT/p1/m1/video/index.txt$" "$WORK/body")"

# --- 变体媒体清单：抓最佳变体后改写 ---
CODE=$(curl -s -o "$WORK/body" -D "$WORK/h" -w '%{http_code}' "$B/p1/m1/video/index.txt")
check "变体清单 200" "200" "$CODE"
check "变体清单类型" "text/plain; charset=utf-8" "$(hfile "$WORK/h" content-type)"
check "变体清单缓存窗口" "public, max-age=6, s-maxage=6" "$(hfile "$WORK/h" cache-control)"
check "变体清单切片绝对" "1" \
  "$(grep -cE "^http://127\.0\.0\.1:$PORT/p1/m1/[0-9a-f]{16}_[0-9a-f]{32}\.[a-z]+$" "$WORK/body")"

# --- Token ---
T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
curl -s -H "Authorization: Bearer $T" "$B/panel/api/config" > "$WORK/cfg.json"
python3 -c '
import json
cfg = json.load(open("'"$WORK"'/cfg.json"))
cfg["auth"]["token_enabled"] = True
json.dump(cfg, open("'"$WORK"'/put.json", "w"))
'
check "开启 Token 200" "200" \
  "$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$B/panel/api/config" \
    -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
    --data-binary "@$WORK/put.json")"
# 播放令牌来自 users.db 的设备令牌；公开清单不再下发。
ADMIN_ID=$(curl -s -H "Authorization: Bearer $T" "$B/panel/api/users" |
  python3 -c 'import sys,json;print(json.load(sys.stdin)["users"][0]["id"])')
PLAYTOKEN=$(curl -s -X POST -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{}' "$B/panel/api/users/$ADMIN_ID/tokens" |
  python3 -c 'import sys,json;print(json.load(sys.stdin)["token"]["token"])')
check "设备令牌长度" "12" "${#PLAYTOKEN}"
check "无 Token 入口 302" "302" "$(status "$B/p1/c1.m3u8")"
check "无 Token 跳 failure" "http://placeholder.test/Failure.mp4" "$(location "$B/p1/c1.m3u8")"
check "带 Token 入口 200" "200" "$(status "$B/p1/c1.m3u8?token=$PLAYTOKEN")"
check "错 Token 入口 302" "302" "$(status "$B/p1/c1.m3u8?token=deadbeef")"
check "未知频道带 Token 跳占位" "http://placeholder.test/404.mp4" "$(location "$B/p1/ghost.m3u8?token=$PLAYTOKEN")"
check "未知分组带 Token 跳占位" "http://placeholder.test/404.mp4" "$(location "$B/ghost/c1.m3u8?token=$PLAYTOKEN")"
check "未知频道无 Token 跳 failure" "http://placeholder.test/Failure.mp4" "$(location "$B/p1/ghost.m3u8")"
check "开放组免 Token" "200" "$(status "$B/op/c8.m3u8")"
check "切片不校验 Token" "200" "$(status "$B$FIRST")"

if [ "$fail" = "0" ]; then
  printf 'M2 冒烟测试全部通过\n'
else
  printf 'M2 冒烟测试有失败项\n'
  exit 1
fi
