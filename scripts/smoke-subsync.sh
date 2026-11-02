#!/usr/bin/env bash
# 订阅托管冒烟：同步、幂等、上游差异、固定、防回流、空清单与删除。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19942}
UP=${UP:-19943}
WORK=$(mktemp -d)
mkdir -p "$WORK/up"
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"

write_m3u() {
  cat > "$WORK/up/sync.m3u" <<M3U
#EXTM3U
#EXTINF:-1 tvg-id="c1" group-title="G",C1
$1
#EXTINF:-1 tvg-id="c2" group-title="G",C2
http://127.0.0.1:$UP/c2.m3u8
M3U
}

write_m3u "http://127.0.0.1:$UP/c1.m3u8"
cat > "$WORK/up/agg.m3u" <<M3U
#EXTM3U
#EXTINF:-1 tvg-id="a1" group-title="聚合组",聚合频道
http://127.0.0.1:$UP/a1.m3u8
M3U

python3 -m http.server "$UP" --directory "$WORK/up" >"$WORK/up.log" 2>&1 &
UP_PID=$!
trap 'kill ${PID:-} $UP_PID 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$UP/sync.m3u" >/dev/null 2>&1 && break
  sleep 0.1
done

cat > "$CFG" <<JSON
{
  "server": {
    "bind": "127.0.0.1:$PORT",
    "log_level": "warn"
  },
  "cache": {
    "disk_path": "$WORK/cache"
  },
  "groups": [
    {
      "id": "m1",
      "name": "手工组",
      "mode": "proxy",
      "channels": [
        {
          "id": "manual1",
          "name": "手工频道",
          "url": "http://127.0.0.1:9/manual.m3u8"
        }
      ]
    }
  ],
  "playlist": {
    "merge_ttl_secs": 60,
    "merge_position": "bottom",
    "sync_enabled": false,
    "sources": [
      {
        "id": "s1",
        "name": "上游",
        "url": "http://127.0.0.1:$UP/sync.m3u",
        "enabled": true,
        "mode": "sync",
        "unlisted_groups": "keep"
      },
      {
        "id": "s2",
        "name": "聚合",
        "url": "http://127.0.0.1:$UP/agg.m3u",
        "enabled": true,
        "mode": "aggregate",
        "unlisted_groups": "keep"
      }
    ]
  }
}
JSON

"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
PID=$!
B="http://127.0.0.1:$PORT"
for _ in $(seq 1 80); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 0.1
done

fail=0
check() { if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"; else printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"; fail=1; fi; }

T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
A=(-H "Authorization: Bearer $T" -H 'Content-Type: application/json')

status_json() {
  curl -s "${A[@]}" "$B/panel/api/subs/status" \
    | python3 -c 'import sys,json;d=json.load(sys.stdin)["sources"];print(json.dumps(next(x for x in d if x["id"]=="s1")))'
}
field() { status_json | python3 -c "import sys,json;print(json.load(sys.stdin).get('$1'))"; }

wait_done() {
  sleep 0.4
  for _ in $(seq 1 200); do
    [ "$(field syncing)" = "False" ] && return
    sleep 0.1
  done
}

LAST_TRIGGER=0
cooldown() {
  local now elapsed
  now=$(date +%s)
  elapsed=$((now - LAST_TRIGGER))
  if [ "$LAST_TRIGGER" -ne 0 ] && [ "$elapsed" -lt 11 ]; then
    sleep $((11 - elapsed))
  fi
  LAST_TRIGGER=$(date +%s)
}

sync_s1() {
  cooldown
  curl -s -o /dev/null "${A[@]}" -X POST "$B/panel/api/subs/sync" -d '{"id":"s1"}'
  wait_done
}

export_json() { curl -s "${A[@]}" "$B/panel/api/groups/export"; }

gid_of_sync() {
  export_json | python3 -c 'import sys,json;g=json.load(sys.stdin);print(next(x["id"] for x in g if x.get("origin")=="sync"))'
}
# 同步分组的 id。首次同步后取一次。
GID=""

channel_json() {
  export_json | python3 -c "
import sys,json
g=json.load(sys.stdin)
for group in g:
    for channel in group['channels']:
        if channel['name']=='$1':
            print(json.dumps({'gid':group['id'],'cid':channel['id'],'channel':channel}))
            break
"
}

# 1) 预检与首次同步
PV=$(curl -s "${A[@]}" -X POST "$B/panel/api/subs/preview" -d '{"id":"s1"}')
check "预检新增 2 条" "2" "$(printf '%s' "$PV" | python3 -c 'import sys,json;print(json.load(sys.stdin)["sync"]["add"])')"
check "预检不写配置" "0" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) if g.get("origin")=="sync"))')"
sync_s1
check "首次新增 2 条" "2" "$(field added)"
check "首次分组 1 个" "1" "$(field groups)"
check "状态无错误" "None" "$(field error)"
GID=$(gid_of_sync)
check "分组归属 sync" "1" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) if g.get("origin")=="sync"))')"

# 订阅包含同步频道，播放地址带令牌。
PTOK=$(curl -s "${A[@]}" "$B/panel/api/me" | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
SUB=$(curl -s "$B/sub/$PTOK/playlist.m3u")
check "订阅含同步频道" "1" "$(printf '%s' "$SUB" | grep -c ',C1$' || true)"
check "订阅含手工频道" "1" "$(printf '%s' "$SUB" | grep -c ',手工频道$' || true)"
check "托管频道走令牌入口" "1" "$(printf '%s' "$SUB" | PTOK="$PTOK" python3 -c '
import os, sys
lines = sys.stdin.read().splitlines()
wanted = {"C1", "C2", "手工频道"}
ok = True
for i, line in enumerate(lines):
    if line.startswith("#EXTINF") and line.rsplit(",", 1)[-1] in wanted:
        url = lines[i + 1] if i + 1 < len(lines) else ""
        ok = ok and ("/l/" + os.environ["PTOK"] + "/") in url
print(1 if ok else 0)
')"

# 2) 幂等
sync_s1
check "再次同步零变更" "0/0/0" "$(field added)/$(field updated)/$(field removed)"

# 3) 上游变化：改 C1 地址、删 C2、加 C3
cat > "$WORK/up/sync.m3u" <<M3U
#EXTM3U
#EXTINF:-1 tvg-id="c1" group-title="G",C1
http://127.0.0.1:$UP/c1-new.m3u8
#EXTINF:-1 tvg-id="c3" group-title="G",C3
http://127.0.0.1:$UP/c3.m3u8
M3U
sync_s1
check "上游变化计数" "1/1/1" "$(field added)/$(field updated)/$(field removed)"
check "C1 地址已更新" "1" "$(export_json | python3 -c 'import sys,json;g=json.load(sys.stdin);print(sum(1 for x in g for c in x["channels"] if c["name"]=="C1" and "c1-new" in c["url"]))')"

# 4) 手工改名后固定。上游删掉 C1，同步后名称保留。
C1=$(channel_json "C1")
C1_GID=$(printf '%s' "$C1" | python3 -c 'import sys,json;print(json.load(sys.stdin)["gid"])')
C1_CID=$(printf '%s' "$C1" | python3 -c 'import sys,json;print(json.load(sys.stdin)["cid"])')
curl -s -o /dev/null "${A[@]}" -X PUT "$B/panel/api/groups/$C1_GID/channels/$C1_CID" \
  -d '{"name":"C1 手工名"}'
check "改名后固定" "False" "$(channel_json "C1 手工名" | python3 -c 'import sys,json;print(json.load(sys.stdin)["channel"].get("sync_follow", True))')"
python3 - "$WORK/up/sync.m3u" <<'PY'
import sys
path = sys.argv[1]
lines = open(path, encoding="utf-8").read().splitlines()
out, skip = [], False
for line in lines:
    if line.startswith("#EXTINF") and ",C1" in line:
        skip = True
        continue
    if skip and line.startswith("http"):
        skip = False
        continue
    out.append(line)
open(path, "w", encoding="utf-8").write("\n".join(out) + "\n")
PY
sync_s1
check "固定频道保留" "1" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) for c in g["channels"] if c["name"]=="C1 手工名"))')"
check "固定频道计入跳过" "1" "$(field skipped_removals)"

# 5) 删除同步频道后写入排除。上游加回，同步不回流。
C3=$(channel_json "C3")
C3_GID=$(printf '%s' "$C3" | python3 -c 'import sys,json;print(json.load(sys.stdin)["gid"])')
C3_CID=$(printf '%s' "$C3" | python3 -c 'import sys,json;print(json.load(sys.stdin)["cid"])')
curl -s -o /dev/null "${A[@]}" -X DELETE "$B/panel/api/groups/$C3_GID/channels/$C3_CID"
check "删除写入排除" "1" "$(curl -s "${A[@]}" "$B/panel/api/playlist" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(sum(1 for s in d["sources"] if s["id"]=="s1" and any(x.startswith("id:c3") for x in s.get("exclude",[]))))')"
printf '#EXTINF:-1 tvg-id="c3" group-title="G",C3\nhttp://127.0.0.1:%s/c3.m3u8\n' "$UP" >> "$WORK/up/sync.m3u"
sync_s1
check "删除后不回流" "0" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) for c in g["channels"] if c["name"]=="C3"))')"

# 6) 抓取失败不改配置。用第二个源验证。
curl -s -o /dev/null "${A[@]}" -X POST "$B/panel/api/playlist/sources" \
  -d "{\"id\":\"bad\",\"name\":\"坏源\",\"url\":\"http://127.0.0.1:1/bad.m3u\",\"mode\":\"sync\",\"unlisted_groups\":\"keep\"}"
curl -s -o /dev/null "${A[@]}" -X POST "$B/panel/api/subs/sync" -d '{"id":"bad"}'
sleep 1
check "坏源记录错误" "1" "$(curl -s "${A[@]}" "$B/panel/api/subs/status" | python3 -c 'import sys,json;d=json.load(sys.stdin)["sources"];print(1 if next(x for x in d if x["id"]=="bad")["error"] else 0)')"
check "坏源不改内容" "1" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) for c in g["channels"] if c["name"]=="C1 手工名"))')"

# 7) 空清单默认拦截。开启 sync_allow_empty 后清空。
: > "$WORK/up/sync.m3u"
sync_s1
check "空清单待确认" "True" "$(field needs_review)"
check "空清单未清空" "0/0/0" "$(field added)/$(field updated)/$(field removed)"
check "空清单内容保留" "1" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) for c in g["channels"] if c["name"]=="C1 手工名"))')"
curl -s "${A[@]}" "$B/panel/api/playlist" > "$WORK/playlist.json"
python3 - "$WORK/playlist.json" <<'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path, encoding="utf-8"))
data["sync_allow_empty"] = True
json.dump(data, open(path, "w", encoding="utf-8"), ensure_ascii=False)
PY
curl -s -o /dev/null "${A[@]}" -X PUT "$B/panel/api/playlist/settings" --data-binary "@$WORK/playlist.json"
sync_s1
check "空清单保留固定频道" "1" "$(field channels)"
check "空同步分组已删除" "0" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) if g.get("origin")=="sync"))')"
check "手工分组保留" "1" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) if g["id"]=="m1"))')"

# 8) 删除源：默认 409。保留为手工后来源字段清空。
write_m3u "http://127.0.0.1:$UP/c1-new.m3u8"
sync_s1
check "恢复同步" "2" "$(field channels)"
check "删除未选方式 409" "409" "$(curl -s -o /dev/null -w '%{http_code}' "${A[@]}" -X DELETE "$B/panel/api/playlist/sources/s1")"
check "409 带 content_exists" "1" "$(curl -s "${A[@]}" -X DELETE "$B/panel/api/playlist/sources/s1" | grep -c 'content_exists' || true)"
curl -s -o /dev/null "${A[@]}" -X DELETE "$B/panel/api/playlist/sources/s1?keep_content=1"
check "保留为手工" "1" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) for c in g["channels"] if c["name"]=="C2" and not c.get("source_id")))')"

# 9) 聚合源行为不变。聚合频道只在订阅里，不物化。
check "聚合频道不物化" "0" "$(export_json | python3 -c 'import sys,json;print(sum(1 for g in json.load(sys.stdin) for c in g["channels"] if c["name"]=="聚合频道"))')"
SUB=$(curl -s "$B/sub/$PTOK/playlist.m3u")
check "聚合频道进订阅" "1" "$(printf '%s' "$SUB" | grep -c '聚合频道' || true)"

# 10) 鉴权：未登录状态接口 401。
check "状态接口需登录" "401" "$(curl -s -o /dev/null -w '%{http_code}' "$B/panel/api/subs/status")"

if [ "$fail" = "0" ]; then echo "订阅托管冒烟测试全部通过"; else echo "订阅托管冒烟测试有失败项"; exit 1; fi
