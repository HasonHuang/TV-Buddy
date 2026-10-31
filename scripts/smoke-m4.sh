#!/usr/bin/env bash
# M4 冒烟测试：热重载、看门狗冷却、缓存状态与清空、运行指标、运行日志。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."

# 日志要落在 $WORK，所以先切 cwd；BIN 必须是绝对路径。
BIN=${BIN:-target/debug/tv-buddy}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
PORT=${PORT:-19918}
UP=${UP:-19919}
WORK=$(mktemp -d)
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"
CACHE="$WORK/cache"

mkdir -p "$WORK/up"
cat > "$WORK/up/live.m3u8" <<'EOF'
#EXTM3U
#EXTINF:4.0,
seg1.ts
EOF
head -c 1024 /dev/urandom > "$WORK/up/seg1.ts"
head -c 1024 /dev/urandom > "$WORK/up/dash-1.m4s"
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

write_config() { # 参数 1：DASH 频道地址
  cat > "$CFG" <<JSON
{
  "server": {
    "bind": "127.0.0.1:$PORT",
    "log_level": "info"
  },
  "cache": {
    "disk_path": "$CACHE"
  },
  "groups": [
    {
      "id": "pg",
      "name": "代理组",
      "mode": "proxy",
      "access": "public",
      "channels": [
        {
          "id": "c1",
          "name": "代理频道",
          "url": "http://127.0.0.1:$UP/live.m3u8"
        }
      ]
    },
    {
      "id": "dg",
      "name": "DASH 组",
      "mode": "dash",
      "access": "public",
      "format": "mpd",
      "channels": [
        {
          "id": "d1",
          "name": "DASH 频道",
          "url": "$1"
        }
      ]
    }
  ]
}
JSON
}

write_config "http://127.0.0.1:1/dead.mpd"

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
UP_PID=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$UP/live.m3u8" >/dev/null 2>&1 && break
  sleep 0.1
done
# 在 $WORK 里启动，日志文件落在 $WORK/tv-buddy.log。
"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
(cd "$WORK" && exec "$BIN" "$DB" >"$WORK/server.log" 2>&1) &
PID=$!
trap '{ kill $PID $UP_PID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"; } 2>/dev/null' EXIT

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
code() { curl -s -o "$WORK/body" -w '%{http_code}' "$@"; }
jget() { python3 -c "import sys,json;d=json.load(open('$WORK/body'));print($1)"; }

T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
A=(-H "Authorization: Bearer $T" -H 'Content-Type: application/json')

# --- 重载：管理 CLI 改库，面板接口生效 ---
check "重载前 2 条" "2" "$(curl -s "$B/playlist.m3u" | grep -c '^#EXTINF')"
python3 - <<PY
import json
path = "$CFG"
cfg = json.load(open(path, encoding="utf-8"))
cfg["groups"].append({
    "id": "hot",
    "name": "热重载组",
    "mode": "proxy",
    "access": "public",
    "channels": [
        {
            "id": "h1",
            "name": "热重载频道",
            "url": "http://127.0.0.1:$UP/live.m3u8",
        }
    ],
})
json.dump(cfg, open(path, "w", encoding="utf-8"), ensure_ascii=False)
PY
"$BIN" config import "$CFG" --db "$DB" --force >/dev/null || exit 1
check "改库后未生效" "2" "$(curl -s "$B/playlist.m3u" | grep -c '^#EXTINF')"
check "重载接口 200" "200" "$(code -X POST "${A[@]}" "$B/panel/api/reload")"
for _ in $(seq 1 50); do
  [ "$(status "$B/hot/h1.m3u8")" = "200" ] && break
  sleep 0.2
done
check "重载新组可播" "200" "$(status "$B/hot/h1.m3u8")"
check "重载后 3 条" "3" "$(curl -s "$B/playlist.m3u" | grep -c '^#EXTINF')"
check "重载已记日志" "1" "$(grep -c '配置已重载' "$WORK/tv-buddy.log")"

# 坏配置：导入失败，库与快照都不变
printf '{"server":{"bind":123}}\n' > "$CFG.bad"
if "$BIN" config import "$CFG.bad" --db "$DB" --force >/dev/null 2>&1; then
  check "坏配置被拒" "拒绝" "接受"
else
  check "坏配置被拒" "拒绝" "拒绝"
fi
check "坏配置保留旧快照" "3" "$(curl -s "$B/playlist.m3u" | grep -c '^#EXTINF')"

# --- 看门狗 ---
for _ in $(seq 1 6); do
  status "$B/dg/d1.m3u8" >/dev/null
done
check "冷却后仍 503" "503" "$(status "$B/dg/d1.m3u8")"
check "冷却文案" "timeout waiting for master playlist" "$(cat "$WORK/body")"
# 日志里是「DASH 连续失败，进入冷却」加字段 `cooldown_secs=3600`，冷却秒数不在文案里。
# 字段名与值之间插着 ANSI 复位码，所以分开断言。
check "冷却已记录日志" "1" "$(grep -c 'DASH 连续失败，进入冷却' "$WORK/tv-buddy.log")"
check "冷却秒数字段在场" "1" "$(grep -c 'cooldown_secs' "$WORK/tv-buddy.log")"
check "冷却秒数为 3600" "1" "$(grep 'cooldown_secs' "$WORK/tv-buddy.log" | grep -c '3600')"
# 修好上游并重载 → 立即解除冷却（S-MODE-17）
python3 - <<PY
import json
path = "$CFG"
cfg = json.load(open(path, encoding="utf-8"))
for group in cfg["groups"]:
    for channel in group.get("channels", []):
        if channel.get("url") == "http://127.0.0.1:1/dead.mpd":
            channel["url"] = "http://127.0.0.1:$UP/main.mpd"
json.dump(cfg, open(path, "w", encoding="utf-8"), ensure_ascii=False)
PY
"$BIN" config import "$CFG" --db "$DB" --force >/dev/null || exit 1
curl -s -o /dev/null -X POST "${A[@]}" "$B/panel/api/reload"
check "改配置解除冷却" "200" "$(status "$B/dg/d1.m3u8")"

# --- 缓存接口 ---
check "切片预热 200" "200" "$(status "$B/pg/c1.m3u8")"
FIRST_URL=$(grep -oE "http://127\.0\.0\.1:$PORT/pg/c1/[0-9a-f]{16}_[0-9a-f]{32}\.[a-z]+" "$WORK/body" | head -n1)
FIRST="/${FIRST_URL#http://127.0.0.1:$PORT/}"
check "切片抓取 200" "200" "$(status "$B$FIRST")"
check "缓存状态 200" "200" "$(code "${A[@]}" "$B/panel/api/cache/stats")"
check "缓存条目 ≥ 1" "1" \
  "$(python3 -c 'import json;d=json.load(open("'"$WORK"'/body"));print(1 if d["entries"]["seg_bytes"]>=1 and d["bytes"]>0 else 0)')"
check "缓存清空 200" "200" "$(code -X POST "${A[@]}" "$B/panel/api/cache/flush")"
check "清空计数 ≥ 1" "1" "$(jget '1 if d["removed"]>=1 else 0')"
check "清空后缓存为空" "0" \
  "$(code "${A[@]}" "$B/panel/api/cache/stats" >/dev/null; jget 'd["entries"]["seg_bytes"] + d["entries"]["seg_url"]')"
check "清空后索引重建" "200" "$(status "$B/pg/c1.m3u8")"
check "清空后切片重取" "200" "$(status "$B$FIRST")"

# --- 指标与日志 ---
check "指标 200" "200" "$(code "${A[@]}" "$B/panel/api/stats")"
check "指标有请求数" "1" "$(jget '1 if d["requests"]>0 else 0')"
check "指标含分组" "1" "$(jget '1 if any(g["id"]=="pg" for g in d["groups"]) else 0')"
check "指标含切片数" "1" "$(jget '1 if d["segments"]>=2 else 0')"
check "日志 200" "200" "$(code "${A[@]}" "$B/panel/api/logs?limit=50")"
check "日志有内容" "1" "$(jget '1 if d["total"]>0 and len(d["lines"])>0 else 0')"
check "日志含清空记录" "1" "$(jget '1 if any("缓存已清空" in line for line in d["lines"]) else 0')"
check "日志文件存在" "1" "$([ -s "$WORK/tv-buddy.log" ] && echo 1 || echo 0)"
# 面板按级别过滤（A4）：环形缓冲的行带 ANSI 颜色，前端先剥掉再取级别词。
# 这里复刻同一套抽取逻辑，确保每一行都能判出级别（否则该级别永远筛不出来）。
cat > "$WORK/level.py" <<'PYEOF'
import json, re, sys
lines = json.load(open(sys.argv[1]))["lines"]
ansi = re.compile("\u001b\\[[0-9;]*m")
seen = set()
for line in lines:
    match = re.search(r"\s(TRACE|DEBUG|INFO|WARN|ERROR)\s", ansi.sub("", line))
    if not match:
        print(0)
        raise SystemExit
    seen.add(match.group(1).lower())
print(1 if seen else 0)
PYEOF
check "日志行都能判出级别" "1" "$(python3 "$WORK/level.py" "$WORK/body")"

if [ "$fail" = "0" ]; then
  printf 'M4 冒烟测试全部通过\n'
else
  printf 'M4 冒烟测试有失败项\n'
  exit 1
fi
