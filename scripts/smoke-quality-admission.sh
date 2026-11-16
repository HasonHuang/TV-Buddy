#!/usr/bin/env bash
# 质量准入容器验收：同步门控、周期移出、安全阀、源停用/删除。
#
# 依赖：curl、jq、python3、docker（含 compose 子命令）。
# 容器使用 tests/manual/local/docker-compose.yml，端口 19900；伪源监听 19932。
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
COMPOSE="$ROOT/tests/manual/local/docker-compose.yml"
B=${B:-http://127.0.0.1:19900}
MEDIA_PORT=${MEDIA_PORT:-19932}
M="http://127.0.0.1:$MEDIA_PORT"
SRC_URL="http://host.docker.internal:$MEDIA_PORT/source.m3u8"
CONTAINER=${CONTAINER:-tvb-local}

for tool in curl jq python3 docker; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "缺少 $tool，请先安装" >&2
    exit 1
  fi
done
if ! docker compose version >/dev/null 2>&1; then
  echo "缺少 docker compose" >&2
  exit 1
fi

WORK=$(mktemp -d)
MOCK_PID=""
cleanup() {
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

fail=0
check() {
  if [ "$2" = "$3" ]; then
    printf 'ok   %s\n' "$1"
  else
    printf 'FAIL %s（期望 %s，实际 %s）\n' "$1" "$2" "$3"
    fail=1
  fi
}

python3 "$ROOT/scripts/quality_mock_source.py" \
  --port "$MEDIA_PORT" \
  --public-base "http://host.docker.internal:$MEDIA_PORT" \
  >"$WORK/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do
  curl -fsS "$M/ok.m3u8" >/dev/null 2>&1 && break
  sleep 0.1
done
if ! curl -fsS "$M/ok.m3u8" >/dev/null 2>&1; then
  echo "伪源启动失败，日志见 $WORK/mock.log" >&2
  exit 1
fi

echo "重建容器镜像…"
docker compose -f "$COMPOSE" down --remove-orphans >"$WORK/compose-down.log" 2>&1 || true
if ! docker compose -f "$COMPOSE" up -d --build >"$WORK/compose.log" 2>&1; then
  echo "容器启动失败" >&2
  tail -80 "$WORK/compose.log" >&2
  exit 1
fi
for _ in $(seq 1 180); do
  curl -fsS "$B/health" >/dev/null 2>&1 && break
  sleep 1
done
if ! curl -fsS "$B/health" >/dev/null 2>&1; then
  echo "服务未就绪" >&2
  docker logs "$CONTAINER" --tail 80 >&2 || true
  exit 1
fi

login() {
  curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
    -d '{"username":"admin","password":"admin"}' | jq -r '.token // ""'
}
TOKEN=$(login)
AUTH="Authorization: Bearer $TOKEN"
check "登录拿到 token" "yes" "$([ -n "$TOKEN" ] && echo yes || echo no)"
if [ -z "$TOKEN" ]; then
  exit 1
fi

getcfg() { curl -fsS -H "$AUTH" "$B/panel/api/config"; }
putcfg() {
  curl -s -o "$WORK/put.out" -w '%{http_code}' -X PUT "$B/panel/api/config" \
    -H "$AUTH" -H 'content-type: application/json' --data-binary "@$1"
}
subs_row() {
  curl -s -H "$AUTH" "$B/panel/api/subs/status" |
    jq -c --arg id "$1" '.sources[] | select(.id == $id)'
}
quality_status() { curl -s -H "$AUTH" "$B/panel/api/quality/status"; }

# 等待同步结束。伪源含慢速频道，一次同步至少约 2 秒，1 秒后轮询不会错过启动。
wait_sync() {
  local id=$1
  sleep 1
  for _ in $(seq 1 300); do
    local row syncing queued error
    row=$(subs_row "$id")
    syncing=$(echo "$row" | jq -r '.syncing // false')
    queued=$(echo "$row" | jq -r '.queued // false')
    if [ "$syncing" = "false" ] && [ "$queued" = "false" ]; then
      error=$(echo "$row" | jq -r '.error // empty')
      if [ -n "$error" ]; then
        echo "同步失败：$error" >&2
        return 1
      fi
      return 0
    fi
    sleep 0.2
  done
  echo "同步超时" >&2
  return 1
}

request_sync() {
  local id=$1 code
  # force=true 绕过订阅源正文缓存，确保 /control 切换后的清单立即生效。
  code=$(curl -s -o "$WORK/sync.json" -w '%{http_code}' -X POST "$B/panel/api/subs/sync" \
    -H "$AUTH" -H 'content-type: application/json' \
    -d "{\"id\":\"$id\",\"force\":true}")
  check "触发同步 $id" "202" "$code"
  wait_sync "$id"
}

run_quality() {
  local code attempt
  for attempt in 1 2; do
    code=$(curl -s -o "$WORK/run.json" -w '%{http_code}' -X POST "$B/panel/api/quality/run" \
      -H "$AUTH" -H 'content-type: application/json' -d '{"scope":"all","run_mode":"quick"}')
    if [ "$code" = "202" ]; then
      break
    fi
    if [ "$code" = "409" ]; then
      curl -s -X POST "$B/panel/api/quality/cancel" -H "$AUTH" >/dev/null
      sleep 1
      continue
    fi
    break
  done
  check "发起校验 202" "202" "$code"
  [ "$code" = "202" ] || return 1
  for _ in $(seq 1 600); do
    if [ "$(quality_status | jq -r '.run.finished // false')" = "true" ]; then
      return 0
    fi
    sleep 0.2
  done
  echo "校验超时" >&2
  return 1
}

results_json() {
  curl -s -H "$AUTH" "$B/panel/api/quality/results?source_id=qsrc&limit=200"
}
result_count() {
  local name=$1 action=$2 reason=${3:-}
  results_json |
    jq --arg n "$name" --arg a "$action" --arg r "$reason" \
      '[.items[] | select(.name == $n and .action == $a and ($r == "" or .reason == $r))] | length'
}
group_names() {
  getcfg | jq -r '[(.groups // [])[] | .channels[] | select(.source_id == "qsrc") | .name] | sort | join(",")'
}
all_names() {
  getcfg | jq -r '[(.groups // [])[] | .channels[] | .name] | sort | join(",")'
}

# 1) 初始规则：质量开启、门控开启、安全阀开启但阈值高到不会干扰移出测试。
getcfg >"$WORK/base.json"
jq --arg url "$SRC_URL" '
  .quality.enabled = true
  | .quality.schedule = "interval"
  | .quality.interval_hours = 8760
  | .quality.probe_mode = "speed"
  | .quality.timeout_secs = 10
  | .quality.speed_timeout_secs = 10
  | .quality.sample_bytes_kb = 4096
  | .quality.sample_secs = 2
  | .quality.sample_segments = 1
  | .quality.max_channels_per_run = 0
  | .quality.budget_mb_per_run = 64
  | .quality.gate_time_budget_secs = 120
  | .quality.guard_enabled = true
  | .quality.max_remove_ratio = 0.5
  | .quality.min_remove_count = 10
  | .quality.result_retention_days = 7
  | .quality.rules.min_speed_mib = 1.0
  | .quality.rules.max_delay_ms = 0
  | .quality.rules.min_resolution = ""
  | .quality.rules.max_resolution = ""
  | .quality.rules.supply = false
  | .quality.rules.retain = false
  | .quality.rules.host_cache = false
  | .playlist.sync_enabled = false
  | .playlist.sources = [{
      "id": "qsrc",
      "name": "质量源",
      "mode": "sync",
      "enabled": true,
      "url": $url,
      "group_mode": "keep"
    }]
  | .groups = []
' "$WORK/base.json" >"$WORK/init.json"
check "写入质量与源配置" "200" "$(putcfg "$WORK/init.json")"

# 2) 首次同步：只准入正常频道；慢速与 404 写 rejected。
request_sync qsrc || true
check "首次门控只留正常频道" "正常频道" "$(group_names)"
check "慢速频道被拒绝" "1" "$(result_count 慢速频道 rejected speed_low)"
check "404 频道被拒绝" "1" "$(result_count 不可达频道 rejected unreachable)"

# 3) 关闭源门控同步，三个频道直接进入分组。
getcfg >"$WORK/c.json"
jq '(.playlist.sources[] | select(.id == "qsrc") | .quality) = {"enabled": false}' \
  "$WORK/c.json" >"$WORK/off.json"
check "关闭源门控" "200" "$(putcfg "$WORK/off.json")"
request_sync qsrc || true
check "门控关闭后三个频道都在" "不可达频道,慢速频道,正常频道" "$(group_names)"

# 4) 重新开启门控并手动校验，慢速与 404 立即移出。
getcfg >"$WORK/c.json"
jq '(.playlist.sources[] | select(.id == "qsrc") | .quality) = {"enabled": true}' \
  "$WORK/c.json" >"$WORK/on.json"
check "重新开启门控" "200" "$(putcfg "$WORK/on.json")"
run_quality || true
check "手动校验移出慢速与 404" "正常频道" "$(group_names)"
check "慢速记录为移出" "1" "$(result_count 慢速频道 removed speed_low)"
check "404 记录为移出" "1" "$(result_count 不可达频道 removed unreachable)"

# 5) 慢速频道恢复为正常地址，重新同步后再次准入。
curl -fsS "$M/control?slow=ok" >/dev/null
request_sync qsrc || true
check "恢复后慢速频道重新准入" "慢速频道,正常频道" "$(group_names)"

# 6) 安全阀：先把慢速频道切回慢速地址，再关闭门控把三个频道都放进分组。
curl -fsS "$M/control?slow=slow" >/dev/null
getcfg >"$WORK/c.json"
jq '(.playlist.sources[] | select(.id == "qsrc") | .quality) = {"enabled": false}' \
  "$WORK/c.json" >"$WORK/off.json"
check "安全阀准备：关闭门控" "200" "$(putcfg "$WORK/off.json")"
request_sync qsrc || true
check "安全阀准备：三个频道都在" "不可达频道,慢速频道,正常频道" "$(group_names)"

getcfg >"$WORK/c.json"
jq '
  (.playlist.sources[] | select(.id == "qsrc") | .quality) = {"enabled": true}
  | .quality.min_remove_count = 2
  | .quality.max_remove_ratio = 0.5
' "$WORK/c.json" >"$WORK/guard.json"
check "开启门控并降低安全阀阈值" "200" "$(putcfg "$WORK/guard.json")"
run_quality || true
check "安全阀触发后成员不变" "不可达频道,慢速频道,正常频道" "$(group_names)"
check "安全阀标记暂停" "true" "$(quality_status | jq -r '.run.paused_by_guard // false')"
check "暂停移出结果" "1" "$(result_count 慢速频道 removal_paused speed_low)"

# 7) 关闭安全阀重跑，移出照常执行。
getcfg >"$WORK/c.json"
jq '.quality.guard_enabled = false' "$WORK/c.json" >"$WORK/noguard.json"
check "关闭安全阀" "200" "$(putcfg "$WORK/noguard.json")"
run_quality || true
check "关闭安全阀后移出两个失败频道" "正常频道" "$(group_names)"

# 8) 停用保留 / 移出。
curl -s -X POST "$B/panel/api/playlist/sources/qsrc/disable" \
  -H "$AUTH" -H 'content-type: application/json' -d '{"keep_channels":true}' >/dev/null
check "停用保留频道" "正常频道" "$(group_names)"
check "源已停用" "false" "$(subs_row qsrc | jq -r '.enabled')"

curl -s -X POST "$B/panel/api/playlist/sources/qsrc/enable" -H "$AUTH" >/dev/null
request_sync qsrc || true
check "重新启用后频道保留" "正常频道" "$(group_names)"

curl -s -X POST "$B/panel/api/playlist/sources/qsrc/disable" \
  -H "$AUTH" -H 'content-type: application/json' -d '{"keep_channels":false}' >/dev/null
check "停用移出频道" "" "$(group_names)"

curl -s -X POST "$B/panel/api/playlist/sources/qsrc/enable" -H "$AUTH" >/dev/null
request_sync qsrc || true
check "再次启用恢复频道" "正常频道" "$(group_names)"

# 9) 删除随源移出。
CODE=$(curl -s -o "$WORK/delete.json" -w '%{http_code}' -X DELETE \
  "$B/panel/api/playlist/sources/qsrc?keep_content=0" -H "$AUTH")
check "删除源并移出" "200" "$CODE"
check "删除后没有该源频道" "" "$(all_names)"

# 10) 重建源，删除时保留为本地频道。
getcfg >"$WORK/c.json"
jq --arg url "$SRC_URL" '
  .playlist.sources += [{
    "id": "qsrc",
    "name": "质量源",
    "mode": "sync",
    "enabled": true,
    "url": $url,
    "group_mode": "keep"
  }]
' "$WORK/c.json" >"$WORK/readd.json"
check "重建源配置" "200" "$(putcfg "$WORK/readd.json")"
request_sync qsrc || true
check "重建后准入正常频道" "正常频道" "$(group_names)"

CODE=$(curl -s -o "$WORK/delete.json" -w '%{http_code}' -X DELETE \
  "$B/panel/api/playlist/sources/qsrc?keep_content=1" -H "$AUTH")
check "删除源并保留频道" "200" "$CODE"
check "频道转为本地" "" \
  "$(getcfg | jq -r '[(.groups // [])[] | .channels[] | select(.name == "正常频道") | .source_id] | join(",")')"
check "源已删除" "0" \
  "$(getcfg | jq '[(.playlist.sources // [])[] | select(.id == "qsrc")] | length')"

# 11) 低配内存：容器硬限制 128 MB。
MEM=$(docker stats --no-stream --format '{{.MemUsage}}' "$CONTAINER" 2>/dev/null | awk '{print $1}')
MEM_MIB=$(echo "$MEM" | awk '
  /GiB$/ { v=$1; sub("GiB","",v); print v*1024; next }
  /MiB$/ { v=$1; sub("MiB","",v); print v; next }
  /KiB$/ { v=$1; sub("KiB","",v); print v/1024; next }
  { print 0 }
')
check "容器内存低于 128 MB" "yes" \
  "$(awk -v v="$MEM_MIB" 'BEGIN { print (v > 0 && v < 128) ? "yes" : "no" }')"
printf 'info 容器内存：%s\n' "${MEM:-未知}"

if [ "$fail" != "0" ]; then
  echo "--- compose tail ---"
  tail -60 "$WORK/compose.log" || true
  echo "--- container log ---"
  docker logs "$CONTAINER" --tail 100 2>&1 || true
  echo "--- mock log ---"
  tail -20 "$WORK/mock.log" || true
  echo "smoke-quality-admission: FAIL"
  exit 1
fi
echo "smoke-quality-admission: PASS"
