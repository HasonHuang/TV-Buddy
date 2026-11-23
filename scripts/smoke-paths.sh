#!/usr/bin/env bash
# 路径契约冒烟：自起服务，检查页面、静态资源、新路径与旧路径 404。
set -euo pipefail

cd "$(dirname "$0")/.."

BIN=${BIN:-target/debug/tv-buddy}
PORT=${PORT:-19960}
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
    curl -fsS "$B/api/health" >/dev/null 2>&1 && break
    sleep 0.1
done

check_method() {
    method=$1
    path=$2
    want=$3
    code=$(curl -s -o /dev/null -w '%{http_code}' -X "$method" "$B$path")
    [ "$code" = "$want" ] || {
        echo "FAIL $method $path: $code != $want"
        exit 1
    }
}

check() {
    check_method GET "$1" "$2"
}

# 首页固定 302 到 `/page`。
check_method GET / 302

# 内嵌页面。`/page/panel` 已删除。
check /page 200
check /page/ 200
check /page/unban 200
check /page/panel 404
check /static/panel/panel.html 200
check /static/logo/does-not-exist.png 404
check /static/assets/does-not-exist.png 404

# 路径面分离：`/api/play` 只给清单，`/media` 只给媒体数据。
check /api/play/g/c/seg.png 404
check /media/g/c/index.m3u8 404
check /media/g/c/video/index.txt 404

# 公开接口新前缀。未登录时面板与 whoami 返回 401，坏令牌订阅返回 404。
check /api/health 200
check /api/panel/status 401
check /api/sub/nope/playlist.m3u 404
check /api/unban/whoami 401
check_method POST /api/unban 401
# `/api/unban` 只有 POST；页面入口是 `/page/unban`。
check_method GET /api/unban 405

# 旧路径已全部删除，必须返回 404。
check /health 404
check /panel 404
check /unban 404
check_method GET /panel/api/status 404
