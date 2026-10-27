#!/usr/bin/env bash
# CLI 冒烟：端口、密码、启动停止、更新、重装、卸载保留数据。
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

sed "s/\[::\]:19899/[::]:$PORT/" packaging/config.toml.example >"$WORK/config.toml"
chmod 600 "$WORK/config.toml"

CLI="$WORK/tv-buddy.sh"
export CLI_LINK

fail() {
    echo "失败：$*" >&2
    exit 1
}

contains() {
    grep -qF "$2" "$1" || fail "$1 缺少：$2"
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

"$CLI" port "$NEW_PORT" >/dev/null
contains "$WORK/config.toml" "bind = \"[::]:$NEW_PORT\""
"$CLI" status | grep -q '运行中' || fail "改端口后未运行"
"$CLI" url | grep -q ":$NEW_PORT/panel" || fail "url 端口不对"

"$CLI" pass cli-pass admin2 >/dev/null
if command -v sha256sum >/dev/null 2>&1; then
    HASH=$(printf '%s' cli-pass | sha256sum | awk '{print $1}')
else
    HASH=$(printf '%s' cli-pass | shasum -a 256 | awk '{print $1}')
fi
contains "$WORK/config.toml" "password_sha256 = \"$HASH\""
contains "$WORK/config.toml" 'username = "admin2"'

cp "$BIN" "$WORK/update-bin"
TV_BUDDY_BIN="$WORK/update-bin" "$CLI" update >/dev/null
"$CLI" status | grep -q '运行中' || fail "更新后未运行"

TV_BUDDY_YES=1 TV_BUDDY_PORT="$REINSTALL_PORT" TV_BUDDY_USER=admin3 TV_BUDDY_PASS=reinstall-pass \
    "$CLI" reinstall >/dev/null
contains "$WORK/config.toml" "bind = \"[::]:$REINSTALL_PORT\""
contains "$WORK/config.toml" 'username = "admin3"'
if command -v sha256sum >/dev/null 2>&1; then
    HASH=$(printf '%s' reinstall-pass | sha256sum | awk '{print $1}')
else
    HASH=$(printf '%s' reinstall-pass | shasum -a 256 | awk '{print $1}')
fi
contains "$WORK/config.toml" "password_sha256 = \"$HASH\""

ln -s "$CLI" "$CLI_LINK"
printf 'N\n' | "$CLI" uninstall >/dev/null
[ -f "$WORK/config.toml" ] || fail "卸载 N 后配置未保留"
[ -x "$WORK/tv-buddy" ] || fail "卸载 N 后二进制未保留"
[ ! -e "$CLI_LINK" ] || fail "卸载后命令软链未移除"

echo "CLI 冒烟通过：$PORT → ${NEW_PORT}，更新、重装、卸载保留数据"
