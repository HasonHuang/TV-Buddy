#!/usr/bin/env bash
# CLI 冒烟：端口、密码、启动停止、更新、重装、卸载保留数据。
# 首次安装的初始口令固定为 admin。
export TV_BUDDY_INITIAL_PASSWORD=admin

set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-target/debug/tv-buddy}
[ -x "$BIN" ] || {
    echo "缺少 ${BIN}；先运行 cargo build"
    exit 1
}

WORK=$(mktemp -d)
PORT=$((20000 + ($$ % 500) * 2))
NEW_PORT=$((PORT + 1))
REINSTALL_PORT=$((PORT + 2))
CLI_LINK="$WORK/bin/tv-buddy"
DB="$WORK/tv-buddy.db"

cleanup() {
    if [ -f "$WORK/tv-buddy.pid" ]; then
        kill "$(cat "$WORK/tv-buddy.pid")" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT HUP INT TERM

cp "$BIN" "$WORK/tv-buddy"
cp packaging/tv-buddy.sh "$WORK/tv-buddy.sh"
chmod +x "$WORK/tv-buddy" "$WORK/tv-buddy.sh"
mkdir -p "$WORK/bin"

# 先建库并把端口挪到空闲段，避免与默认 19899 冲突。
"$WORK/tv-buddy" config set server.bind "[::]:$PORT" --db "$DB" >/dev/null

CLI="$WORK/tv-buddy.sh"
export CLI_LINK

fail() {
    echo "失败：$*" >&2
    exit 1
}

contains() {
    grep -qF "$2" "$1" || fail "$1 缺少：$2"
}

cfgval() {
    "$WORK/tv-buddy" config get "$1" --db "$DB" 2>/dev/null || true
}

# 等到端口应答，再用给定口令登录。
login_ok() { # 参数：端口 口令
    i=0
    while [ "$i" -lt 40 ]; do
        curl -fsS -o /dev/null "http://127.0.0.1:$1/health" 2>/dev/null && break
        sleep 0.2
        i=$((i + 1))
    done
    curl -fsS -o /dev/null -X POST "http://127.0.0.1:$1/panel/api/login" \
        -H 'content-type: application/json' \
        -d "{\"username\":\"admin\",\"password\":\"$2\"}"
}

"$CLI" --help >/dev/null
"$CLI" start >/dev/null
"$CLI" status | grep -q '运行中' || fail "status 未显示运行中"

printf '0\n' | "$CLI" >"$WORK/menu.out"
contains "$WORK/menu.out" "tv-buddy 安装 · 管理"
contains "$WORK/menu.out" "1) 更新二进制"
printf '6\n0\n' | "$CLI" menu >"$WORK/menu-status.out"
contains "$WORK/menu-status.out" "运行中"
printf 'x\n0\n' | "$CLI" menu >"$WORK/menu-invalid.out" 2>&1
contains "$WORK/menu-invalid.out" "无效输入"
printf 'u\nn\n0\n' | "$CLI" menu >"$WORK/menu-cancel.out" 2>&1
contains "$WORK/menu-cancel.out" "已取消"
[ -x "$WORK/tv-buddy" ] || fail "菜单取消卸载后二进制丢失"
"$CLI" stop >/dev/null
printf '6\n3\n0\n' | "$CLI" menu >"$WORK/menu-recover.out" 2>&1
contains "$WORK/menu-recover.out" "未运行"
contains "$WORK/menu-recover.out" "已启动"
"$CLI" status | grep -q '运行中' || fail "菜单启动后未运行"

if "$CLI" port 70000 >/dev/null 2>&1; then
    fail "非法端口未被拒绝"
fi
if "$CLI" pass 123 >/dev/null 2>&1; then
    fail "过短密码未被拒绝"
fi
if "$CLI" pass good-pass ghost >/dev/null 2>&1; then
    fail "不存在的账号未被拒绝"
fi

"$CLI" port "$NEW_PORT" >/dev/null
[ "$(cfgval server.bind)" = "[::]:$NEW_PORT" ] || fail "端口未写入数据库：$(cfgval server.bind)"
"$CLI" status | grep -q '运行中' || fail "改端口后未运行"
# 只取一次输出。`grep -q` 会提前关闭管道，令 printf 得到 SIGPIPE，与 pipefail 冲突。
url_out=$("$CLI" url)
case "$url_out" in
    *":$NEW_PORT/panel"*) ;;
    *) fail "url 端口不对：$url_out" ;;
esac
# 初始口令文件按 `cache.disk_path` 解析；默认相对路径要拼成绝对路径。
case "$url_out" in
    *"初始口令：$WORK/cache/admin_initial_credentials.txt"*) ;;
    *) fail "url 未给出凭据文件绝对路径：$url_out" ;;
esac
login_ok "$NEW_PORT" admin || fail "改端口后无法登录"

"$CLI" pass cli-pass >/dev/null
login_ok "$NEW_PORT" cli-pass || fail "改密后无法登录"

cp "$BIN" "$WORK/update-bin"
TV_BUDDY_BIN="$WORK/update-bin" "$CLI" update >/dev/null
"$CLI" status | grep -q '运行中' || fail "更新后未运行"

TV_BUDDY_YES=1 TV_BUDDY_PORT="$REINSTALL_PORT" TV_BUDDY_USER=admin TV_BUDDY_PASS=reinstall-pass \
    "$CLI" reinstall >/dev/null
[ "$(cfgval server.bind)" = "[::]:$REINSTALL_PORT" ] || fail "重装后端口不对"
login_ok "$REINSTALL_PORT" reinstall-pass || fail "重装后无法登录"

ln -s "$CLI" "$CLI_LINK"
printf 'N\n' | "$CLI" uninstall >/dev/null
[ -f "$DB" ] || fail "卸载 N 后数据库未保留"
[ -x "$WORK/tv-buddy" ] || fail "卸载 N 后二进制未保留"
[ ! -e "$CLI_LINK" ] || fail "卸载后命令软链未移除"

echo "CLI 冒烟通过：$PORT → ${NEW_PORT}，改密、更新、重装、卸载保留数据"
