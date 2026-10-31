#!/usr/bin/env bash
# 备份/恢复冒烟：导出 gzip、改动配置、恢复、拒绝非法内容。
# 首次安装的初始口令固定为 admin，供脚本登录。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19919}
WORK=$(mktemp -d)
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"

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
      "id": "g1",
      "name": "原组",
      "mode": "proxy"
    }
  ]
}
JSON

"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
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
groups() { python3 -c 'import json,sys;print(",".join(g["id"] for g in json.load(sys.stdin)["groups"]))'; }

TOKEN=$(curl -s -X POST "$B/panel/api/login" -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' | python3 -c 'import json,sys;print(json.load(sys.stdin).get("token",""))')
AUTH="Authorization: Bearer $TOKEN"

STATUS=$(curl -s -o "$WORK/backup.tar.gz" -D "$WORK/backup.head" -w '%{http_code}' -H "$AUTH" "$B/panel/api/backup?include=all")
check "备份 200" "200" "$STATUS"
check "备份类型" "application/gzip" "$(awk 'BEGIN{IGNORECASE=1}index($0,"content-type:")==1{sub(/\r$/,"");print substr($0,15)}' "$WORK/backup.head")"
check "备份文件名" "1" "$(awk 'BEGIN{IGNORECASE=1}/^content-disposition:/&&/tv-buddy-backup-/{n=1}END{print n+0}' "$WORK/backup.head")"
check "备份魔数" "1f8b" "$(od -An -tx1 -N2 "$WORK/backup.tar.gz" | tr -d ' ')"
check "备份含配置" "1" "$(tar tzf "$WORK/backup.tar.gz" 2>/dev/null | grep -c 'tv-buddy-backup/config.json' || true)"
check "备份包只有一份配置" "1" "$(tar tzf "$WORK/backup.tar.gz" 2>/dev/null | grep -c 'config' || true)"

IMPORT='[{"id":"tmp","name":"临时组","mode":"proxy"}]'
curl -s -o /dev/null -X POST "$B/panel/api/groups/import" -H "$AUTH" -H 'content-type: application/json' -d "$IMPORT"
check "导入后含临时组" "g1,tmp" "$(curl -s -H "$AUTH" "$B/panel/api/config" | groups)"

STATUS=$(curl -s -o "$WORK/restore.json" -w '%{http_code}' -X POST "$B/panel/api/restore" \
  -H "$AUTH" -H 'content-type: application/gzip' --data-binary "@$WORK/backup.tar.gz")
check "恢复 200" "200" "$STATUS"
check "恢复已重载" "True" "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["config_reloaded"])' "$WORK/restore.json")"
check "恢复后只剩原组" "g1" "$(curl -s -H "$AUTH" "$B/panel/api/config" | groups)"

STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/panel/api/restore" \
  -H "$AUTH" -H 'content-type: application/gzip' --data-binary 'not gzip')
check "非法内容 400" "400" "$STATUS"

if [ "$fail" = "0" ]; then echo "备份恢复冒烟测试全部通过"; else echo "备份恢复冒烟测试有失败项"; exit 1; fi
