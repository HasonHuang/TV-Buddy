#!/usr/bin/env bash
# 路径契约冒烟：自起服务，检查页面、静态资源与旧别名。
set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19916}
WORK=$(mktemp -d)
CFG="$WORK/config.json"
DB="$WORK/tv-buddy.db"

cat > "$CFG" <<JSON
{
  "server": { "bind": "127.0.0.1:$PORT", "log_level": "warn" },
  "cache": { "disk_path": "$WORK/cache" },
  "auth": { "token_enabled": false }
}
JSON

"$BIN" config import "$CFG" --db "$DB" >/dev/null || exit 1
"$BIN" "$DB" >"$WORK/server.log" 2>&1 &
PID=$!
trap 'kill "$PID" 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$WORK"' EXIT

B="http://127.0.0.1:$PORT"
for _ in $(seq 1 100); do
    curl -fsS "$B/health" >/dev/null 2>&1 && break
    sleep 0.1
done

check() {
    code=$(curl -s -o /dev/null -w '%{http_code}' "$B$1")
    [ "$code" = "$2" ] || {
        echo "FAIL $1: $code != $2"
        exit 1
    }
}

check /page/panel 200
check /page/unban 200
check /static/panel/panel.html 200
check /logo/does-not-exist.png 404
check /static/logo/does-not-exist.png 404
