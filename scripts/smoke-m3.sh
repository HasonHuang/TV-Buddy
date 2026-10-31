#!/usr/bin/env bash
# M3 冒烟测试：直连模式（redirect / rewrite / passthrough）与 remux / dash 就绪语义。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19916}
UP=${UP:-19917}
WORK=$(mktemp -d)
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"

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
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S">
  <Period id="0">
    <AdaptationSet contentType="video" maxWidth="640" maxHeight="360">
      <SegmentTemplate timescale="1000" media="dash-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="0"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" codecs="avc1.42c01e" bandwidth="1000000" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>
EOF
head -c 2048 /dev/urandom > "$WORK/up/seg1.ts"
head -c 2048 /dev/urandom > "$WORK/up/dash-1.m4s"

cat > "$CFG" <<JSON
{
  "health": {
    "enabled": false
  },
  "server": {
    "bind": "127.0.0.1:$PORT",
    "log_level": "warn",
    "error_url": "http://placeholder.test/mistake.mp4"
  },
  "auth": {
    "token_enabled": false,
    "token_failure_url": "http://placeholder.test/Failure.mp4"
  },
  "groups": [
    {
      "id": "rd",
      "name": "直连组",
      "mode": "redirect",
      "access": "public",
      "redirect_ttl": 300,
      "redirect_mode": "direct",
      "channels": [
        {
          "id": "c1",
          "name": "直连频道",
          "url": "http://127.0.0.1:$UP/direct.m3u8"
        }
      ]
    },
    {
      "id": "rb",
      "name": "选画质组",
      "mode": "redirect",
      "access": "public",
      "redirect_ttl": 300,
      "redirect_mode": "best",
      "channels": [
        {
          "id": "c2",
          "name": "选画质频道",
          "url": "http://127.0.0.1:$UP/master.m3u8"
        }
      ]
    },
    {
      "id": "rw",
      "name": "改写组",
      "mode": "rewrite",
      "access": "public",
      "channels": [
        {
          "id": "c3",
          "name": "改写频道",
          "url": "http://127.0.0.1:$UP/rw.m3u8"
        }
      ]
    },
    {
      "id": "pt",
      "name": "透传组",
      "mode": "passthrough",
      "access": "public",
      "channels": [
        {
          "id": "c4",
          "name": "透传频道",
          "url": "http://127.0.0.1:$UP/pt.m3u8"
        }
      ]
    },
    {
      "id": "rm",
      "name": "转封装组",
      "mode": "remux",
      "access": "public",
      "channels": [
        {
          "id": "c5",
          "name": "转封装频道",
          "url": "http://127.0.0.1:$UP/seg1.ts"
        }
      ]
    },
    {
      "id": "dh",
      "name": "DASH 组",
      "mode": "dash",
      "access": "public",
      "format": "mpd",
      "channels": [
        {
          "id": "c6",
          "name": "DASH 频道",
          "url": "http://127.0.0.1:$UP/main.mpd"
        },
        {
          "id": "c7",
          "name": "DASH 失败频道",
          "url": "http://127.0.0.1:1/dead.mpd"
        }
      ]
    },
    {
      "id": "mp",
      "name": "代理 MPD 组",
      "mode": "proxy",
      "access": "public",
      "channels": [
        {
          "id": "m1",
          "name": "代理 MPD 频道",
          "url": "http://127.0.0.1:$UP/main.mpd"
        }
      ]
    }
  ]
}
JSON

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
UP_PID=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$UP/seg1.ts" >/dev/null 2>&1 && break
  sleep 0.1
done
"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
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
check "改写缓存头" "no-cache, must-revalidate" "$(hfile "$WORK/h" cache-control)"
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
check "dash 类型" "application/vnd.apple.mpegurl" "$(hfile "$WORK/h" content-type)"
check "dash 主清单" "1" "$(grep -c '^#EXT-X-STREAM-INF:' "$WORK/body")"
check "dash 轨道" "1" "$(grep -c '/dh/c6/video_360p/index.txt' "$WORK/body")"
check "dash 媒体清单 200" "200" "$(status "$B/dh/c6/video_360p/index.txt")"
check "dash 媒体清单切片" "1" "$(grep -c '#EXTINF:2.000000,' "$WORK/body")"
check "dash 未就绪 503" "503" "$(status "$B/dh/c7.m3u8")"
check "dash 未就绪文案" "timeout waiting for master playlist" "$(cat "$WORK/body")"

# --- Proxy + MPD ---
CODE=$(curl -s -o "$WORK/body" -D "$WORK/h" -w '%{http_code}' "$B/mp/m1.mpd")
check "proxy MPD 200" "200" "$CODE"
# 契约：`application/dash+xml; charset=utf-8`（`stream.rs` 的 MPD_CONTENT_TYPE）。
check "proxy MPD 类型" "application/dash+xml; charset=utf-8" "$(hfile "$WORK/h" content-type)"
# 契约：`<BaseURL>` 改成本地反代 `/{gid}/{cid}/mpd/{token}/{Base64URL(上游基址)}/`
# （见 `04` §3 的「proxy MPD 反代」行），不是裸的 `/mpd/`。
check "proxy MPD 改写 BaseURL" "1" \
  "$(grep -cE "<BaseURL>http://127.0.0.1:$PORT/mp/m1/mpd/[A-Za-z0-9_-]+/[A-Za-z0-9_-]+/</BaseURL>" "$WORK/body")"
check "proxy MPD 切片 200" "200" "$(status "$B/mp/m1/mpd/dash-1.m4s")"
check "proxy MPD 切片非空" "yes" "$([ -s "$WORK/body" ] && echo yes || echo no)"

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
# 播放令牌来自 users.db 的设备令牌，公开清单不再下发。
ADMIN_ID=$(curl -s -H "Authorization: Bearer $T" "$B/panel/api/users" |
  python3 -c 'import sys,json;print(json.load(sys.stdin)["users"][0]["id"])')
PLAYTOKEN=$(curl -s -X POST -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{}' "$B/panel/api/users/$ADMIN_ID/tokens" |
  python3 -c 'import sys,json;print(json.load(sys.stdin)["token"]["token"])')
check "直连无 Token 302" "302" "$(status "$B/rd/c1.m3u8")"
check "直连无 Token 跳 failure" "http://placeholder.test/Failure.mp4" "$(location "$B/rd/c1.m3u8")"
check "直连带 Token 302 上游" "http://127.0.0.1:$UP/direct.m3u8" "$(location "$B/rd/c1.m3u8?token=$PLAYTOKEN")"
check "透传无 Token 302" "302" "$(status "$B/pt/c4.m3u8")"
check "透传无 Token 跳 failure" "http://placeholder.test/Failure.mp4" "$(location "$B/pt/c4.m3u8")"
check "透传带 Token 指上游" "http://127.0.0.1:$UP/pt.m3u8" "$(location "$B/pt/c4.m3u8?token=$PLAYTOKEN")"

if [ "$fail" = "0" ]; then
  printf 'M3 冒烟测试全部通过\n'
else
  printf 'M3 冒烟测试有失败项\n'
  exit 1
fi
