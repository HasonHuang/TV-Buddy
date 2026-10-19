#!/usr/bin/env bash
# M3 冒烟测试：直连模式（redirect / rewrite / passthrough）与 remux / dash 就绪语义。
set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19916}
UP=${UP:-19917}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"

mkdir -p "$WORK/up/1080" "$WORK/up/360"
cat > "$WORK/up/direct.m3u8" <<'EOF'
#EXTM3U
#EXTINF:4.0,
seg1.ts
EOF
cat > "$WORK/up/rw.m3u8" <<'EOF'
#EXTM3U
#EXTINF:4.0,
seg1.ts
#EXTINF:4.0,
sub/seg2.ts
EOF
cat > "$WORK/up/pt.m3u8" <<'EOF'
#EXTM3U
#EXTINF:4.0,
seg1.ts
EOF
cat > "$WORK/up/master.m3u8" <<'EOF'
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=640x360
360/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x1080
1080/index.m3u8
EOF
cat > "$WORK/up/360/index.m3u8" <<'EOF'
#EXTM3U
#EXTINF:4.0,
seg1.ts
EOF
cat > "$WORK/up/1080/index.m3u8" <<'EOF'
#EXTM3U
#EXTINF:4.0,
seg1.ts
EOF
cat > "$WORK/up/main.mpd" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
  <Period id="0"><AdaptationSet><Representation id="v0" bandwidth="1000000"/></AdaptationSet></Period>
</MPD>
EOF
head -c 2048 /dev/urandom > "$WORK/up/seg1.ts"

cat > "$CFG" <<EOF
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"
error_url = "http://placeholder.test/mistake.mp4"

[auth]
token_enabled = false
token_failure_url = "http://placeholder.test/Failure.mp4"

[[groups]]
id = "rd"
name = "直连组"
mode = "redirect"
access = "public"
redirect_ttl = 300
redirect_mode = "direct"
[[groups.channels]]
id = "c1"
name = "直连频道"
url = "http://127.0.0.1:$UP/direct.m3u8"

[[groups]]
id = "rb"
name = "选画质组"
mode = "redirect"
access = "public"
redirect_ttl = 300
redirect_mode = "best"
[[groups.channels]]
id = "c2"
name = "选画质频道"
url = "http://127.0.0.1:$UP/master.m3u8"

[[groups]]
id = "rw"
name = "改写组"
mode = "rewrite"
access = "public"
[[groups.channels]]
id = "c3"
name = "改写频道"
url = "http://127.0.0.1:$UP/rw.m3u8"

[[groups]]
id = "pt"
name = "透传组"
mode = "passthrough"
access = "public"
[[groups.channels]]
id = "c4"
name = "透传频道"
url = "http://127.0.0.1:$UP/pt.m3u8"

[[groups]]
id = "rm"
name = "转封装组"
mode = "remux"
access = "public"
[[groups.channels]]
id = "c5"
name = "转封装频道"
url = "http://127.0.0.1:$UP/seg1.ts"

[[groups]]
id = "dh"
name = "DASH 组"
mode = "dash"
access = "public"
format = "mpd"
[[groups.channels]]
id = "c6"
name = "DASH 频道"
url = "http://127.0.0.1:$UP/main.mpd"
[[groups.channels]]
id = "c7"
name = "DASH 失败频道"
url = "http://127.0.0.1:1/dead.mpd"
EOF

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
UP_PID=$!
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
status() { curl -s -o "$WORK/body" -w '%{http_code}' "$1"; }
location() { curl -s -o /dev/null -D - "$1" | awk 'BEGIN{IGNORECASE=1}/^location:/{sub(/\r$/,"");print substr($0,11)}'; }
hfile() { awk -v k="$2" 'BEGIN{IGNORECASE=1}index($0,k":")==1{sub(/\r$/,"");print substr($0,length(k)+3)}' "$1"; }
hits() { grep -c "$1" "$WORK/up.log" || true; }

# --- Redirect（直接跳转） ---
check "直连 302" "302" "$(status "$B/rd/c1.m3u8")"
check "直连指向上游" "http://127.0.0.1:$UP/direct.m3u8" "$(location "$B/rd/c1.m3u8")"
check "直连不抓上游" "0" "$(hits '/direct.m3u8 ')"
check "直连可重复" "302" "$(status "$B/rd/c1.m3u8")"

# --- Redirect（解析最佳画质） ---
check "选画质 302" "302" "$(status "$B/rb/c2.m3u8")"
L1=$(location "$B/rb/c2.m3u8")
check "选画质挑最高码率" "http://127.0.0.1:$UP/1080/index.m3u8" "$L1"
check "选画质结果一致" "http://127.0.0.1:$UP/1080/index.m3u8" "$(location "$B/rb/c2.m3u8")"
check "选画质缓存地址" "1" "$(hits '/master.m3u8 ')"

# --- Rewrite ---
CODE=$(curl -s -o "$WORK/body" -D "$WORK/h" -w '%{http_code}' "$B/rw/c3.m3u8")
check "改写 200" "200" "$CODE"
check "改写类型" "text/plain; charset=utf-8" "$(hfile "$WORK/h" content-type)"
check "改写切片绝对化" "1" "$(grep -c "^http://127.0.0.1:$UP/seg1.ts$" "$WORK/body")"
check "改写相对地址解析" "1" "$(grep -c "^http://127.0.0.1:$UP/sub/seg2.ts$" "$WORK/body")"
check "改写不含本地路径" "0" "$(grep -c '/rw/c3/' "$WORK/body")"
check "改写复用 200" "200" "$(status "$B/rw/c3.m3u8")"
check "改写复用缓存" "1" "$(hits '/rw.m3u8 ')"

# --- Passthrough ---
check "透传 302" "302" "$(status "$B/pt/c4.m3u8")"
check "透传指向上游" "http://127.0.0.1:$UP/pt.m3u8" "$(location "$B/pt/c4.m3u8")"
check "透传不抓上游" "0" "$(hits '/pt.m3u8 ')"

# --- Remux ---
CODE=$(curl -s -o "$WORK/body" -D "$WORK/h" -w '%{http_code}' "$B/rm/c5.m3u8")
check "remux 503" "503" "$CODE"
check "remux 预热文案" "remux warming up" "$(cat "$WORK/body")"
check "remux Retry-After" "2" "$(hfile "$WORK/h" retry-after)"
check "remux 保持预热" "503" "$(status "$B/rm/c5.m3u8")"

# --- DASH ---
CODE=$(curl -s -o "$WORK/body" -D "$WORK/h" -w '%{http_code}' "$B/dh/c6.m3u8")
check "dash 就绪 200" "200" "$CODE"
check "dash 类型" "application/dash+xml" "$(hfile "$WORK/h" content-type)"
check "dash 正文" "1" "$(grep -c '<MPD' "$WORK/body")"
check "dash 未就绪 503" "503" "$(status "$B/dh/c7.m3u8")"
check "dash 未就绪文案" "timeout waiting for master playlist" "$(cat "$WORK/body")"

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
PLAYTOKEN=$(curl -s "$B/playlist.m3u" | grep -oE 'token=[0-9a-f]{48}' | head -n1 | cut -d= -f2)
check "直连无 Token 302" "302" "$(status "$B/rd/c1.m3u8")"
check "直连无 Token 跳 failure" "http://placeholder.test/Failure.mp4" "$(location "$B/rd/c1.m3u8")"
check "直连带 Token 302 上游" "http://127.0.0.1:$UP/direct.m3u8" "$(location "$B/rd/c1.m3u8?token=$PLAYTOKEN")"
check "透传免 Token" "302" "$(status "$B/pt/c4.m3u8")"
check "透传免 Token 指上游" "http://127.0.0.1:$UP/pt.m3u8" "$(location "$B/pt/c4.m3u8")"

if [ "$fail" = "0" ]; then
  printf 'M3 冒烟测试全部通过\n'
else
  printf 'M3 冒烟测试有失败项\n'
  exit 1
fi
