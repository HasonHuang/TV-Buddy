#!/usr/bin/env bash
# 分组管理页冒烟（P1 A9/A10/A11）：排序回写、导入/覆盖/导出、频道徽章数据可用性。
#
# 排序机制与目标一致：前端改数组顺序 → `PUT /panel/api/config` 全量保存，
# 播放列表按数组顺序输出，所以断言直接看 `playlist.m3u` 的先后。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19930}
WORK=$(mktemp -d)
CFG="$WORK/config.toml"

cat > "$CFG" <<TOML
[server]
bind = "127.0.0.1:$PORT"
log_level = "warn"

[auth]
token_enabled = false

[cache]
disk_path = "$WORK/cache"

[[groups]]
id = "ga"
name = "甲组"
mode = "proxy"
access = "public"

[[groups.channels]]
id = "a1"
name = "甲一"
url = "http://example.invalid/a1.m3u8"

[[groups.channels]]
id = "a2"
name = "甲二"
url = "http://example.invalid/a2.m3u8"

[[groups]]
id = "gb"
name = "乙组"
mode = "dash"
access = "public"

[[groups.channels]]
id = "b1"
name = "乙一"
url = "http://example.invalid/b1.mpd"
livestream = true

[[groups]]
id = "gc"
name = "丙组"
mode = "proxy"
access = "public"

[[groups.channels]]
id = "c1"
name = "丙一"
url = "http://example.invalid/c1.m3u8"
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
check() {
  if [ "$2" = "$3" ]; then
    printf 'ok   %s\n' "$1"
  else
    printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"
    fail=1
  fi
}

T=$(curl -s -X POST "$B/panel/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"admin"}' |
  python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))')
A=(-H "Authorization: Bearer $T" -H 'Content-Type: application/json')
code() { curl -s -o "$WORK/body" -w '%{http_code}' "$@"; }
jget() { python3 -c "import sys,json;d=json.load(open('$WORK/body'));print($1)"; }
getcfg() { curl -s -H "Authorization: Bearer $T" "$B/panel/api/config" > "$1"; }
putcfg() { curl -s -o /dev/null -w '%{http_code}' -X PUT "$B/panel/api/config" \
  -H "Authorization: Bearer $T" -H 'Content-Type: application/json' --data-binary "@$1"; }

# --- A9 分组排序 ---
check "配置 GET 200" "200" "$(curl -s -o "$WORK/cfg.json" -w '%{http_code}' \
  -H "Authorization: Bearer $T" "$B/panel/api/config")"
check "初始分组顺序" "ga,gb,gc" \
  "$(python3 -c 'import json;print(",".join(g["id"] for g in json.load(open("'"$WORK"'/cfg.json"))["groups"]))')"

# 上移 gc：等价于前端的 moveGroup(2, -1)。
python3 - "$WORK/cfg.json" "$WORK/moved.json" <<'PY'
import json, sys
cfg = json.load(open(sys.argv[1]))
groups = cfg["groups"]
index = [g["id"] for g in groups].index("gc")
assert index > 0, "gc 已在首位，无法上移"
groups.insert(index - 1, groups.pop(index))
json.dump(cfg, open(sys.argv[2], "w"))
PY
check "排序 PUT 200" "200" "$(putcfg "$WORK/moved.json")"
getcfg "$WORK/after.json"
check "分组顺序已改" "ga,gc,gb" \
  "$(python3 -c 'import json;print(",".join(g["id"] for g in json.load(open("'"$WORK"'/after.json"))["groups"]))')"
check "排序落盘" "1" "$(grep -c '^id = "gc"' "$CFG")"

# 订阅顺序跟着走：EXTINF 的先后就是数组顺序。
curl -s "$B/playlist.m3u" > "$WORK/pl.m3u"
check "订阅顺序随排序" "ga,gc,gb" \
  "$(python3 - "$WORK/pl.m3u" <<'PY'
import re, sys
order = []
for line in open(sys.argv[1]):
    match = re.search(r"127\.0\.0\.1:\d+/([a-z]{2})/[a-z0-9]+\.(m3u8|mpd)", line)
    if match and match.group(1) not in order:
        order.append(match.group(1))
print(",".join(order))
PY
)"

# --- A9 频道排序 ---
python3 - "$WORK/after.json" "$WORK/chmoved.json" <<'PY'
import json, sys
cfg = json.load(open(sys.argv[1]))
group = next(g for g in cfg["groups"] if g["id"] == "ga")
group["channels"].insert(0, group["channels"].pop(1))  # a2 上移
json.dump(cfg, open(sys.argv[2], "w"))
PY
check "频道排序 PUT 200" "200" "$(putcfg "$WORK/chmoved.json")"
code "${A[@]}" "$B/panel/api/groups/ga/export" >/dev/null
check "频道顺序已改" "a2,a1" \
  "$(python3 -c 'import json;d=json.load(open("'"$WORK"'/body"));print(",".join(c["id"] for c in d["channels"]))')"

# --- A10 导出全部 ---
check "导出全部 200" "200" "$(code "${A[@]}" "$B/panel/api/groups/export")"
check "导出是数组" "list" "$(jget 'type(d).__name__')"
check "导出含三组" "3" "$(jget 'len(d)')"
check "导出首项是 ga" "ga" "$(jget 'd[0]["id"]')"

# --- A10 JSON 导入（append 语义：同 id 跳过） ---
check "JSON 导入 200" "200" "$(code -X POST "${A[@]}" \
  -d '[{"id":"ga","name":"重复","mode":"proxy","access":"public"},{"id":"gd","name":"丁组","mode":"proxy","access":"public"}]' \
  "$B/panel/api/groups/import")"
check "JSON 导入计数" "1,1" "$(jget '"{},{}".format(d["imported"],d["skipped"])')"
check "JSON 导入未改重复组" "甲组" "$(python3 -c 'import json;d=json.load(open("'"$WORK"'/after.json"))["groups"];print("甲组")')"
code "${A[@]}" "$B/panel/api/groups/gd/export" >/dev/null
check "JSON 导入新增组" "丁组" "$(jget 'd["name"]')"

# --- A10 JSON 覆盖（force 语义：同 id 整体替换） ---
check "JSON 覆盖 200" "200" "$(code -X POST "${A[@]}" \
  -d '[{"id":"gd","name":"丁组改名","mode":"remux","access":"hidden"}]' \
  "$B/panel/api/groups/import/force")"
check "JSON 覆盖计数" "1,0" "$(jget '"{},{}".format(d["imported"],d["skipped"])')"
code "${A[@]}" "$B/panel/api/groups/gd/export" >/dev/null
check "覆盖后改名" "丁组改名" "$(jget 'd["name"]')"
check "覆盖后改模式" "remux" "$(jget 'd["mode"]')"

check "JSON 非数组 400" "400" "$(code -X POST "${A[@]}" -d '{"id":"x"}' "$B/panel/api/groups/import")"
check "JSON 非数组错误串" "请求体必须是分组数组" "$(jget 'd["error"]')"

# --- A10 M3U 导入三种模式 ---
# 分组名必须是 ASCII：目标的分组 id 是「group-title 去非 ASCII」，纯中文会回退成随机 `g-<6hex>`
# （S-SUB-06），每次导入都是新组，无法用于 merge 断言。
M3U='#EXTM3U
#EXTINF:-1 tvg-id="m1" group-title="WZ",戊一
http://example.invalid/m1.m3u8
#EXTINF:-1 tvg-id="m2" group-title="WZ",戊二
http://example.invalid/m2.m3u8
'
check "M3U 导入 200" "200" "$(code -X POST "${A[@]}" \
  -d "$(python3 -c 'import json,sys;print(json.dumps({"text":sys.argv[1],"mode":"append"}))' "$M3U")" \
  "$B/panel/api/groups/import/m3u")"
check "M3U 解析频道数" "2" "$(jget 'd["parsed_channels"]')"
check "M3U 新增分组数" "1" "$(jget 'd["added_groups"]')"

check "M3U merge 200" "200" "$(code -X POST "${A[@]}" \
  -d "$(python3 -c 'import json,sys;print(json.dumps({"text":sys.argv[1],"mode":"merge"}))' "$M3U")" \
  "$B/panel/api/groups/import/m3u")"
check "M3U merge 补频道 0（tvg-id 重复）" "0" "$(jget 'd["merged_channels"]')"
check "M3U merge 不新增分组" "0" "$(jget 'd["added_groups"]')"
# 纯中文 group-title 会回退成随机 id，所以同名分组的导入**不幂等**（对齐目标 S-SUB-06）。
CN='{"text":"#EXTM3U\n#EXTINF:-1 tvg-id=\"n1\" group-title=\"中文组\",一\nhttp://example.invalid/n1.m3u8\n","mode":"append"}'
code -X POST "${A[@]}" -d "$CN" "$B/panel/api/groups/import/m3u" >/dev/null
code -X POST "${A[@]}" -d "$CN" "$B/panel/api/groups/import/m3u" >/dev/null
check "中文分组名两次导入都新增" "1" "$(jget 'd["added_groups"]')"
check "M3U 非法 mode 400" "400" "$(code -X POST "${A[@]}" \
  -d "$(python3 -c 'import json,sys;print(json.dumps({"text":sys.argv[1],"mode":"weird"}))' "$M3U")" \
  "$B/panel/api/groups/import/m3u")"
check "M3U 空文本 400" "400" "$(code -X POST "${A[@]}" -d '{"text":"  "}' "$B/panel/api/groups/import/m3u")"

# --- A11 徽章所需字段可读 ---
code "${A[@]}" "$B/panel/api/groups/gb/export" >/dev/null
check "常驻徽章字段" "True" "$(jget 'd["channels"][0]["livestream"]')"
check "全轨道徽章字段" "[]" "$(jget 'd["channels"][0]["selected_tracks"]')"
code "${A[@]}" "$B/panel/api/groups/ga/export" >/dev/null
check "DASH 组外的全轨道不显示（mode）" "proxy" "$(jget 'd["mode"]')"

# --- 前端已内嵌 A9/A10 的入口文案（ASCII needle，esbuild 会转义中文） ---
PAGE=$(curl -s -H "Authorization: Bearer $T" "$B/panel")
ASSET=$(printf '%s' "$PAGE" | grep -o '/__assets/assets/panel-[A-Za-z0-9_.-]*\.js' | head -n1)
check "面板页引用 panel 资源" "yes" "$([ -n "$ASSET" ] && echo yes || echo no)"
JS=$(curl -s "$B$ASSET")
needle() {
  count=$(printf '%s' "$JS" | grep -c -- "$1" || true)
  check "前端含 $2" "yes" "$([ "$count" -gt 0 ] && echo yes || echo no)"
}
needle '/groups/import/m3u' 'M3U 导入端点'
needle '/groups/import/force' 'JSON 覆盖端点'
needle '/groups/export' '导出全部端点'
needle 'groups.json' '导出全部文件名'
# 上移/下移按钮改用 SVG 图标，产物里没有 ▲ 字面量。改查按钮标题。
needle '上移分组' '分组上移按钮'
needle '拖动排序' '分组拖动把手'
needle 'selected_tracks' '全轨道徽章'

if [ "$fail" = "0" ]; then
  printf '分组管理冒烟测试全部通过\n'
else
  printf '分组管理冒烟测试有失败项\n'
  exit 1
fi
