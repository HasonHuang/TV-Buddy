#!/usr/bin/env bash
# 系统设置页冒烟：整份配置往返、字段路径存在性、U-10 生效、口令哈希契约、前端资源已内嵌。
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/iptv-proxy}
PORT=${PORT:-19928}
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
name = "示例"
mode = "proxy"
access = "public"

[[groups.channels]]
id = "c1"
name = "频道"
url = "http://127.0.0.1:9/live.m3u8"
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

login() {
  curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
    -d "{\"username\":\"admin\",\"password\":\"$1\"}" |
    python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))'
}

TOKEN=$(login admin)
AUTH="Authorization: Bearer $TOKEN"
check "登录拿到 token" "yes" "$([ -n "$TOKEN" ] && echo yes || echo no)"

canon() { python3 -c 'import json,sys;print(json.dumps(json.load(open(sys.argv[1])),sort_keys=True,ensure_ascii=False))' "$1"; }
getcfg() { curl -s -H "$AUTH" "$B/panel/api/config" > "$1"; }
putcfg() {
  curl -s -o "$WORK/put.out" -w '%{http_code}' -X PUT "$B/panel/api/config" \
    -H "$AUTH" -H 'content-type: application/json' --data-binary "@$1"
}

# 1) 整份配置往返：GET → PUT 原样 → GET，两次必须逐字节等价（不丢未知键、不改类型）。
getcfg "$WORK/a.json"
check "配置 PUT 200" "200" "$(putcfg "$WORK/a.json")"
getcfg "$WORK/b.json"
if [ "$(canon "$WORK/a.json")" = "$(canon "$WORK/b.json")" ]; then
  printf 'ok   %s\n' "配置往返一致（GET→PUT→GET）"
else
  printf 'FAIL 配置往返不一致\n'
  diff <(canon "$WORK/a.json" | tr ',' '\n') <(canon "$WORK/b.json" | tr ',' '\n') | head -10
  fail=1
fi

# 2) 前端字段级表单绑定的路径必须在配置里存在且类型正确。
path_type() {
  python3 - "$WORK/b.json" "$1" <<'PY'
import json,sys
node=json.load(open(sys.argv[1]))
for key in sys.argv[2].split('.'):
    if not isinstance(node,dict) or key not in node:
        print('missing'); raise SystemExit
    node=node[key]
if isinstance(node,bool): print('bool')
elif isinstance(node,(int,float)): print('number')
elif isinstance(node,str): print('string')
elif isinstance(node,list): print('array')
else: print(type(node).__name__)
PY
}
while read -r path want; do
  [ -n "$path" ] || continue
  check "路径 $path 是 $want" "$want" "$(path_type "$path")"
done <<'PATHS'
server.bind string
server.timezone_offset_hours number
server.catchup_tpl string
server.log_level string
server.m3u_header_lines array
panel.username string
panel.password_sha256 string
cache.disguise.enabled bool
cache.disguise.formats array
health.interval_secs number
stats.online_window_secs number
cache.backend string
cache.disk_max_mb number
cache.memory_max_mb number
cache.prefetch.mpd_readahead number
cache.gc.max_disk_mb number
cache.gc.disk_budget_ratio number
cdn.segment_max_age_secs number
cache.segments.per_channel_max number
subtitle.style.enabled bool
subtitle.style.font_size string
subtitle.style.color string
subtitle.style.position number
subtitle.translate.enabled bool
subtitle.translate.targets array
subtitle.translate.target string
subtitle.translate.translation_first bool
subtitle.translate.set_default bool
subtitle.translate.timeout_ms number
subtitle.translate.token string
PATHS

# 3) U-10：设了 [cache].disk_max_mb 之后，推荐值必须变成手动源并等于该值。
python3 - "$WORK/b.json" "$WORK/c.json" <<'PY'
import json,sys
cfg=json.load(open(sys.argv[1]))
cfg['cache']['disk_max_mb']=33
cfg['cache']['memory_max_mb']=2
cfg['cache']['segments']['global_max']=0
json.dump(cfg,open(sys.argv[2],'w'),ensure_ascii=False)
PY
check "写入 U-10 配置 200" "200" "$(putcfg "$WORK/c.json")"
STATS=$(curl -s -H "$AUTH" "$B/panel/api/cache/stats")
check "推荐值来源 manual" "True" "$(echo "$STATS" | python3 -c 'import json,sys;print(json.load(sys.stdin)["recommend"]["disk_manual"])')"
check "推荐上限 = 33" "33" "$(echo "$STATS" | python3 -c 'import json,sys;print(json.load(sys.stdin)["recommend"]["disk_cap_mb"])')"

# 4) 口令：客户端算出的 SHA-256 必须能被登录接受（设置页「新密码」靠这条）。
NEW=$(python3 -c 'import hashlib;print(hashlib.sha256(b"newpass-123").hexdigest())')
python3 - "$WORK/c.json" "$WORK/d.json" "$NEW" <<'PY'
import json,sys
cfg=json.load(open(sys.argv[1]))
cfg['panel']['password_sha256']=sys.argv[3]
json.dump(cfg,open(sys.argv[2],'w'),ensure_ascii=False)
PY
check "改口令 200" "200" "$(putcfg "$WORK/d.json")"
check "新口令可登录" "yes" "$([ -n "$(login newpass-123)" ] && echo yes || echo no)"
check "旧口令被拒" "no" "$([ -n "$(login admin)" ] && echo yes || echo no)"

# 5) 面板资源已内嵌：新设置页的字符串出现在下发的前端包里。
#
# 只查 ASCII 串：esbuild 默认把非 ASCII 转成 \uXXXX，中文串查不到。
ASSET=$(curl -s "$B/panel" | grep -oE '/__assets/assets/panel-[A-Za-z0-9_.-]+\.js' | head -1)
check "面板 HTML 引用入口 js" "yes" "$([ -n "$ASSET" ] && echo yes || echo no)"
BUNDLE=$(curl -s "$B$ASSET")
for needle in \
  'panel.password_sha256' \
  'cache.disguise.formats' \
  'cache.prefetch.mpd_readahead' \
  'cache.gc.disk_budget_ratio' \
  'cache.segments.per_channel_max' \
  'cdn.segment_max_age_secs' \
  'DraftCtx'
do
  case "$BUNDLE" in
    *"$needle"*) printf 'ok   %s\n' "前端包含 $needle" ;;
    *) printf 'FAIL 前端缺少 %s\n' "$needle"; fail=1 ;;
  esac
done

if [ "$fail" = "0" ]; then echo "系统设置页冒烟测试全部通过"; else echo "系统设置页冒烟测试有失败项"; exit 1; fi
