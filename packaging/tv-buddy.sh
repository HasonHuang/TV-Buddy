#!/bin/sh
# TV Buddy 管理 CLI：启动、停止、改端口、改密码、更新、卸载。
#
# 安装后由 install.sh 软链为 /usr/local/bin/tv-buddy。
# 无 systemd/OpenRC 时退回 PID 文件方式，便于容器内运行。

set -eu

# 解析软链，保证 /usr/local/bin/tv-buddy 找到真实安装目录。
resolve_self() {
    path=$1
    while [ -L "$path" ]; do
        dir=$(CDPATH= cd -- "$(dirname -- "$path")" && pwd)
        target=$(readlink "$path")
        case "$target" in
            /*) path=$target ;;
            *) path="$dir/$target" ;;
        esac
    done
    CDPATH= cd -- "$(dirname -- "$path")" && pwd
}

BASE_DIR=$(resolve_self "$0")
BIN="$BASE_DIR/tv-buddy"
# 配置、用户与令牌都在这个 SQLite 文件里。
DB="$BASE_DIR/tv-buddy.db"
LOG_FILE="$BASE_DIR/tv-buddy.log"
PID_FILE="$BASE_DIR/tv-buddy.pid"
SERVICE_NAME=tv-buddy
SERVICE_FILE="/etc/systemd/system/$SERVICE_NAME.service"
OPENRC_FILE="/etc/init.d/$SERVICE_NAME"
CLI_LINK=${CLI_LINK:-/usr/local/bin/tv-buddy}
DEFAULT_PORT=19899
DEFAULT_USER=admin
DEFAULT_PASS=admin
VERSION=${TV_BUDDY_CLI_VERSION:-0.1.0}
# 默认发布源，与 install.sh 一致；可用 TV_BUDDY_REPO / TV_BUDDY_VERSION 覆盖。
GITHUB_REPO=${TV_BUDDY_REPO:-HasonHuang/TV-Buddy}
TV_BUDDY_VERSION=${TV_BUDDY_VERSION:-latest}
TMP_DIR=

if [ -t 1 ]; then
    RESET=$(printf '\033[0m')
    BLUE=$(printf '\033[36m')
    GREEN=$(printf '\033[32m')
    YELLOW=$(printf '\033[1;33m')
    RED=$(printf '\033[31m')
    DIM=$(printf '\033[2m')
else
    RESET= BLUE= GREEN= YELLOW= RED= DIM=
fi

cleanup() {
    stty echo 2>/dev/null || true
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}
trap cleanup EXIT HUP INT TERM

info() { printf '%s\n' "$*"; }
warn() { printf '警告：%s\n' "$*" >&2; }
fail() { printf '错误：%s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
tv-buddy 管理 CLI

用法：tv-buddy <命令> [参数]

命令：
  menu                  打开交互菜单（无参数默认）
  start                 启动服务
  stop                  停止服务
  restart               重启服务
  status                查看服务状态
  log [行数]            查看日志；终端下持续跟随
  url                   打印面板地址
  port <N>              修改面板端口并重启
  pass [密码] [账号]    修改已有账号的密码；无参数进入交互
  update                更新二进制，保留数据库
  reinstall             备份数据库、重置配置后重装
  install [参数]        调用同目录 install.sh 安装或修复
  uninstall             卸载服务；询问是否删除数据

update 来源（按优先级）：
  TV_BUDDY_BIN        本地二进制
  TV_BUDDY_ARCHIVE    本地 tar.gz
  TV_BUDDY_URL        远程 tar.gz 或二进制，支持 {arch} 占位符
  本地 dist/ 中的安装包
  GitHub Release       默认；用 TV_BUDDY_VERSION 指定 tag

非交互：
  TV_BUDDY_YES=1      跳过 reinstall / uninstall 的删除确认

环境变量：
  TV_BUDDY_REPO       发布仓库，默认内置
  TV_BUDDY_VERSION    要更新的 tag，如 v2026.3.0；默认 latest

示例：
  tv-buddy port 8080
  tv-buddy pass 'new-password' admin
  tv-buddy update                                  # 拉最新 Release
  TV_BUDDY_VERSION=v2026.3.0 tv-buddy update     # 固定版本
  TV_BUDDY_URL=https://example.com/tv-buddy-{arch}.tar.gz tv-buddy update
EOF
}

need_root() {
    [ "$(id -u)" -eq 0 ] || fail "该命令需要 root 权限"
}

has_systemd() {
    [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1
}

has_openrc() {
    command -v rc-service >/dev/null 2>&1 && command -v rc-update >/dev/null 2>&1
}

svc_managed() {
    if has_systemd && [ -f "$SERVICE_FILE" ]; then
        return 0
    fi
    if has_openrc && [ -f "$OPENRC_FILE" ]; then
        return 0
    fi
    return 1
}

svc_active() {
    if has_systemd && [ -f "$SERVICE_FILE" ]; then
        systemctl is-active --quiet "$SERVICE_NAME"
    elif has_openrc && [ -f "$OPENRC_FILE" ]; then
        rc-service "$SERVICE_NAME" status >/dev/null 2>&1
    else
        daemon_running
    fi
}

svc_do() {
    if has_systemd && [ -f "$SERVICE_FILE" ]; then
        systemctl "$1" "$SERVICE_NAME"
    elif has_openrc && [ -f "$OPENRC_FILE" ]; then
        rc-service "$SERVICE_NAME" "$1"
    else
        return 2
    fi
}

is_installed() {
    [ -x "$BIN" ]
}

require_installed() {
    [ -x "$BIN" ] || fail "未安装：$BIN"
}

ensure_mutable() {
    if svc_managed; then
        need_root
    fi
    [ -w "$BASE_DIR" ] || fail "目录不可写：${BASE_DIR}（请用 root 运行）"
}

daemon_running() {
    [ -f "$PID_FILE" ] || return 1
    pid=$(cat "$PID_FILE" 2>/dev/null || true)
    [ -n "$pid" ] || return 1
    kill -0 "$pid" 2>/dev/null
}

daemon_start() {
    [ -x "$BIN" ] || fail "缺少可执行文件：$BIN"
    if daemon_running; then
        return 0
    fi
    (
        cd "$BASE_DIR" || exit 1
        exec "$BIN" "$DB" >>"$LOG_FILE" 2>&1
    ) &
    echo $! >"$PID_FILE"
    i=0
    while [ "$i" -lt 20 ]; do
        daemon_running && return 0
        sleep 0.5
        i=$((i + 1))
    done
    [ -f "$LOG_FILE" ] && tail -n 20 "$LOG_FILE" >&2 || true
    warn "启动失败，见 $LOG_FILE"
    return 1
}

daemon_stop() {
    if ! daemon_running; then
        rm -f "$PID_FILE"
        return 0
    fi
    pid=$(cat "$PID_FILE")
    kill "$pid" 2>/dev/null || true
    i=0
    while [ "$i" -lt 50 ]; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
        i=$((i + 1))
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$PID_FILE"
}

restart_runtime() {
    if svc_managed; then
        need_root
        svc_do restart || return 1
    else
        daemon_stop
        daemon_start || return 1
    fi
}

health_check() {
    port=$1
    command -v curl >/dev/null 2>&1 || return 0
    i=0
    while [ "$i" -lt 8 ]; do
        code=$(curl -fsS -o /dev/null -w '%{http_code}' --max-time 3 \
            "http://127.0.0.1:$port/health" 2>/dev/null || true)
        [ "$code" = "200" ] && return 0
        sleep 1
        i=$((i + 1))
    done
    return 1
}

# ---- 配置读写：交给二进制的 config 子命令 ----

cfg_get() {
    [ -f "$DB" ] || return 1
    "$BIN" config get "$1.$2" --db "$DB" 2>/dev/null
}

cfg_set() {
    [ -f "$DB" ] || fail "数据库不存在：$DB"
    "$BIN" config set "$1.$2" "$3" --db "$DB" >/dev/null || fail "写入配置失败"
}

get_bind() {
    cfg_get server bind || true
}

get_port() {
    bind=$(get_bind)
    [ -n "$bind" ] || {
        printf '%s' "$DEFAULT_PORT"
        return
    }
    printf '%s' "${bind##*:}"
}

set_port() {
    bind=$(get_bind)
    host="[::]"
    [ -n "$bind" ] && host="${bind%:*}"
    [ -n "$host" ] || host="[::]"
    cfg_set server bind "\"$host:$1\""
}

detect_ip() {
    ip=$(hostname -I 2>/dev/null | awk '{print $1}' || true)
    if [ -z "$ip" ]; then
        ip=$(ip -4 route get 1.1.1.1 2>/dev/null |
            awk '{for (i = 1; i <= NF; i++) if ($i == "src") print $(i + 1)}' |
            head -n 1 || true)
    fi
    if [ -z "$ip" ] && command -v ipconfig >/dev/null 2>&1; then
        ip=$(ipconfig getifaddr en0 2>/dev/null || true)
    fi
    [ -n "$ip" ] || ip="<主机>"
    printf '%s' "$ip"
}

show_url() {
    port=$(get_port)
    bind=$(get_bind)
    host=
    [ -n "$bind" ] && host="${bind%:*}"
    host=${host#\[}
    host=${host%\]}
    case "$host" in
        ""|"0.0.0.0"|"::") host=$(detect_ip) ;;
    esac
    case "$host" in
        *:*) host="[$host]" ;;
    esac
    printf '面板地址：http://%s:%s/panel\n' "$host" "$port"
    printf '账号：在面板「用户与设备」管理；初始口令见 admin_initial_credentials.txt\n'
}

# ---- 二进制更新 ----

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64) printf 'amd64' ;;
        aarch64|arm64) printf 'arm64' ;;
        *) fail "不支持的架构：$(uname -m)（只支持 amd64/arm64）" ;;
    esac
}

is_binary() {
    magic=$(od -An -tx1 -N4 "$1" 2>/dev/null | tr -d ' \n')
    [ "$magic" = "7f454c46" ] && return 0
    if [ "$(uname -s)" = "Darwin" ]; then
        case "$magic" in
            cffaedfe|feedfacf|cafebabe) return 0 ;;
        esac
    fi
    return 1
}

download() {
    url=$1
    dest=$2
    if command -v curl >/dev/null 2>&1; then
        curl -fL --retry 2 --connect-timeout 10 -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        fail "需要 curl 或 wget"
    fi
}

extract_binary() {
    archive=$1
    [ -f "$archive" ] || fail "安装包不存在：$archive"
    TMP_DIR=$(mktemp -d)
    tar -xzf "$archive" -C "$TMP_DIR" || fail "解包失败：$archive"
    src=$(find "$TMP_DIR" -type f -name tv-buddy | head -n 1)
    [ -n "$src" ] || fail "安装包内没有 tv-buddy：$archive"
    printf '%s' "$src"
}

# 取 GitHub API 的 JSON。失败返回非 0，由调用方回退。
fetch_json() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 2 --connect-timeout 10 \
            -H 'Accept: application/vnd.github+json' "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- --header='Accept: application/vnd.github+json' "$1"
    else
        return 1
    fi
}

# 解析 releases/latest 的重定向得到 tag（不经 API，省限流）。
latest_tag() {
    command -v curl >/dev/null 2>&1 || return 1
    redirect=$(curl -fsS -o /dev/null -w '%{redirect_url}' --max-time 10 \
        "https://github.com/$GITHUB_REPO/releases/latest" 2>/dev/null) || return 1
    tag=${redirect##*/tag/}
    [ -n "$tag" ] && [ "$tag" != "$redirect" ] || return 1
    printf '%s' "$tag"
}

# 从 Release JSON 里挑一条与架构匹配的资产地址。
release_asset_url() { # $1=arch $2=json
    printf '%s\n' "$2" \
        | grep -o '"browser_download_url"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | sed 's/.*"\(https[^"]*\)"/\1/' \
        | grep -e "-$1\.tar\.gz\$" \
        | head -n 1
}

# 查 Release 中当前架构的 tar.gz 地址。优先枚举真实资产名；
# API 不可用时按 tag 拼地址。
release_url() {
    arch=$1
    if [ "$TV_BUDDY_VERSION" != "latest" ]; then
        tag=$TV_BUDDY_VERSION
    else
        tag=$(latest_tag) || tag=
    fi

    if [ -n "$tag" ]; then
        probe="https://api.github.com/repos/$GITHUB_REPO/releases/tags/$tag"
    else
        probe="https://api.github.com/repos/$GITHUB_REPO/releases/latest"
    fi
    json=$(fetch_json "$probe" 2>/dev/null) || json=
    if [ -n "$json" ]; then
        url=$(release_asset_url "$arch" "$json")
        if [ -n "$url" ]; then
            printf '%s' "$url"
            return 0
        fi
        [ "$TV_BUDDY_VERSION" = "latest" ] || return 1
    fi

    if [ "$TV_BUDDY_VERSION" = "latest" ]; then
        # `releases/latest` 只认正式版；只有预发布时退回最新一条。
        list=$(fetch_json "https://api.github.com/repos/$GITHUB_REPO/releases?per_page=30" 2>/dev/null) || list=
        url=$(release_asset_url "$arch" "$list")
        if [ -n "$url" ]; then
            echo "未找到正式版，改用最新 Release：${url##*/}" >&2
            printf '%s' "$url"
            return 0
        fi
    fi

    [ -n "$tag" ] || return 1
    # 产物名前缀：tv-buddy-*；旧版本用过 tv-buddy-*，一并试。
    for prefix in tv-buddy tv-buddy; do
        for candidate in "$tag" "v${tag#v}"; do
            url="https://github.com/$GITHUB_REPO/releases/download/$candidate/${prefix}-${candidate#v}-$arch.tar.gz"
            if command -v curl >/dev/null 2>&1; then
                code=$(curl -fsSL -o /dev/null -w '%{http_code}' --max-time 10 "$url" || true)
                [ "$code" = "200" ] || continue
            fi
            printf '%s' "$url"
            return 0
        done
    done
    return 1
}

update_source() {
    if [ -n "${TV_BUDDY_BIN:-}" ]; then
        [ -f "$TV_BUDDY_BIN" ] || fail "二进制不存在：$TV_BUDDY_BIN"
        printf '%s' "$TV_BUDDY_BIN"
        return
    fi
    if [ -n "${TV_BUDDY_ARCHIVE:-}" ]; then
        extract_binary "$TV_BUDDY_ARCHIVE"
        return
    fi
    if [ -n "${TV_BUDDY_URL:-}" ]; then
        arch=$(detect_arch)
        url=$(printf '%s' "$TV_BUDDY_URL" | sed "s/{arch}/$arch/g")
        TMP_DIR=$(mktemp -d)
        echo "下载：$url" >&2
        download "$url" "$TMP_DIR/pkg"
        if tar -tzf "$TMP_DIR/pkg" >/dev/null 2>&1; then
            extract_binary "$TMP_DIR/pkg"
        else
            printf '%s' "$TMP_DIR/pkg"
        fi
        return
    fi

    arch=$(detect_arch)
    for dir in "$BASE_DIR/dist" "$BASE_DIR/../dist"; do
        [ -d "$dir" ] || continue
        latest=$(ls -t "$dir"/tv-buddy-*-"$arch".tar.gz 2>/dev/null | head -n 1 || true)
        if [ -n "$latest" ]; then
            extract_binary "$latest"
            return
        fi
    done

    # 本地没有产物时，退到 GitHub Release（默认更新源）。
    release=$(release_url "$arch") || fail "在 $GITHUB_REPO 的 Release $TV_BUDDY_VERSION 中找不到 $arch 安装包。若是预发布版本，请用 TV_BUDDY_VERSION=<tag> 指定"
    TMP_DIR=$(mktemp -d)
    echo "下载：$release" >&2
    download "$release" "$TMP_DIR/pkg.tar.gz" || fail "下载失败：$release"
    if ( download "$release.sha256" "$TMP_DIR/pkg.sha256" ) >/dev/null 2>&1; then
        verify_digest "$TMP_DIR/pkg.tar.gz" "$TMP_DIR/pkg.sha256" || fail "校验失败：$release"
    fi
    extract_binary "$TMP_DIR/pkg.tar.gz"
}

# 期望摘要与实测摘要比对。没有 sha256 工具时跳过（不阻断更新）。
verify_digest() {
    file=$1
    sum_file=$2
    [ -s "$sum_file" ] || return 0
    expected=$(awk 'NR==1 {print $1}' "$sum_file")
    [ -n "$expected" ] || return 0
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$file" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$file" | awk '{print $1}')
    else
        warn "未找到 sha256 工具，跳过校验"
        return 0
    fi
    [ "$expected" = "$actual" ]
}

install_binary() {
    src=$1
    [ -f "$src" ] || {
        warn "二进制不存在：$src"
        return 1
    }
    is_binary "$src" || {
        warn "不是可执行二进制：$src"
        return 1
    }
    new="$BASE_DIR/.tv-buddy.new.$$"
    cp "$src" "$new" || {
        warn "复制二进制失败"
        return 1
    }
    chmod 0755 "$new"
    mv -f "$new" "$BIN"
}

# ---- 命令实现 ----

find_installer() {
    for candidate in "$BASE_DIR/install.sh" "$BASE_DIR/../install.sh" "$BASE_DIR/../../install.sh"; do
        [ -f "$candidate" ] && {
            printf '%s' "$candidate"
            return 0
        }
    done
    return 1
}

cmd_install() {
    installer=$(find_installer) || {
        if is_installed; then
            info "已安装：$BASE_DIR"
            info "更新用 tv-buddy update；重装用 tv-buddy reinstall"
            return 0
        fi
        fail "找不到 install.sh；请从发布包运行 ./install.sh"
    }
    info "运行安装脚本：$installer"
    sh "$installer" "$@"
    info "安装脚本完成"
    if [ -x "$CLI_LINK" ]; then
        info "命令：$CLI_LINK"
    fi
}

cmd_start() {
    if svc_managed; then
        need_root
        svc_do start || fail "启动服务失败"
    else
        daemon_start || fail "启动失败，见 $LOG_FILE"
    fi
    info "已启动"
}

cmd_stop() {
    if svc_managed; then
        need_root
        svc_do stop || fail "停止服务失败"
    else
        daemon_stop
    fi
    info "已停止"
}

cmd_restart() {
    restart_runtime || fail "重启失败"
    info "已重启"
}

cmd_status() {
    if svc_managed; then
        if has_systemd && [ -f "$SERVICE_FILE" ]; then
            systemctl status "$SERVICE_NAME" --no-pager -l 2>/dev/null || true
            systemctl is-active --quiet "$SERVICE_NAME" || return 1
        else
            rc-service "$SERVICE_NAME" status 2>/dev/null || return 1
        fi
    elif daemon_running; then
        info "运行中（pid $(cat "$PID_FILE")）"
    else
        info "未运行"
        return 1
    fi
}

cmd_log() {
    lines=${1:-100}
    case "$lines" in
        ""|*[!0-9]*) lines=100 ;;
    esac
    if [ -f "$LOG_FILE" ]; then
        if [ -t 1 ]; then
            tail -n "$lines" -f "$LOG_FILE"
        else
            tail -n "$lines" "$LOG_FILE"
        fi
        return
    fi
    if has_systemd && [ -f "$SERVICE_FILE" ]; then
        journalctl -u "$SERVICE_NAME" -n "$lines" --no-pager
    elif has_openrc && [ -f "$OPENRC_FILE" ]; then
        tail -n "$lines" "/var/log/$SERVICE_NAME.log"
    else
        fail "找不到日志：$LOG_FILE"
    fi
}

cmd_url() {
    require_installed
    show_url
}

cmd_port() {
    require_installed
    port=${1:-}
    if [ -z "$port" ]; then
        [ -t 0 ] || fail "用法：tv-buddy port <N>"
        printf '当前端口：%s\n新端口：' "$(get_port)"
        read -r port
    fi
    case "$port" in
        ""|*[!0-9]*) fail "端口无效：$port" ;;
    esac
    [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || fail "端口无效：$port"
    ensure_mutable
    set_port "$port"
    restart_runtime || fail "端口已写入配置，但重启失败；见 $LOG_FILE"
    show_url
}

cmd_pass_interactive() {
    require_installed
    printf '账号 [回车=%s]：' "$DEFAULT_USER"
    read -r account || account=
    account=${account:-$DEFAULT_USER}
    printf '新密码（至少 4 位）：'
    stty -echo 2>/dev/null || true
    read -r password || password=
    stty echo 2>/dev/null || true
    printf '\n'

    [ -n "$password" ] || {
        info "未修改"
        return 0
    }
    [ "${#password}" -ge 4 ] || fail "密码太短（至少 4 位）"
    ensure_mutable
    printf '%s' "$password" | "$BIN" passwd "$account" --db "$DB" --stdin >/dev/null ||
        fail "改密失败：账号不存在？请先在面板创建"
    restart_runtime || fail "密码已更新，但重启失败；见 $LOG_FILE"
    info "密码已更新，旧登录会话失效"
}

cmd_pass() {
    require_installed
    password=${1:-}
    account=${2:-$DEFAULT_USER}
    if [ -z "$password" ]; then
        [ -t 0 ] || fail "用法：tv-buddy pass <密码> [账号]"
        cmd_pass_interactive
        return
    fi
    [ "${#password}" -ge 4 ] || fail "密码太短（至少 4 位）"
    ensure_mutable
    printf '%s' "$password" | "$BIN" passwd "$account" --db "$DB" --stdin >/dev/null ||
        fail "改密失败：账号不存在？请先在面板创建"
    restart_runtime || fail "密码已写入数据库，但重启失败；见 $LOG_FILE"
    info "密码已更新，旧登录会话失效"
}

cmd_update() {
    require_installed
    ensure_mutable
    src=$(update_source)
    backup="$BIN.bak.$$"
    cp -p "$BIN" "$backup" || fail "备份旧二进制失败"
    if svc_managed; then
        need_root
        svc_do stop || true
    else
        daemon_stop
    fi
    if ! install_binary "$src"; then
        warn "更新失败，恢复旧二进制"
        mv -f "$backup" "$BIN"
        restart_runtime || true
        exit 1
    fi
    if ! restart_runtime; then
        warn "新二进制启动失败，恢复旧版本"
        mv -f "$backup" "$BIN"
        restart_runtime || true
        fail "更新失败，已恢复旧二进制"
    fi
    rm -f "$backup"
    port=$(get_port)
    if health_check "$port"; then
        info "更新完成，服务运行正常"
    else
        warn "二进制已更新，但健康检查未通过；用 tv-buddy log 查看"
    fi
}

read_default() {
    prompt=$1
    default=$2
    value=
    if [ -t 0 ]; then
        printf '%s [回车=%s]：' "$prompt" "$default" >&2
        read -r value
    fi
    printf '%s' "${value:-$default}"
}

cmd_reinstall() {
    require_installed
    ensure_mutable
    if [ "${TV_BUDDY_YES:-}" = "1" ]; then
        :
    else
        if [ -t 0 ]; then
            printf '重装会备份数据库、重置配置；用户与令牌保留。确认继续？[y/N] '
        fi
        if read -r answer && { [ "$answer" = "y" ] || [ "$answer" = "Y" ]; }; then
            :
        else
            info "已取消（非交互重装可设 TV_BUDDY_YES=1）"
            return
        fi
    fi
    old_port=$(get_port)

    # 先取更新源，避免停服后才发现下载地址不可用。
    new_bin=
    if [ -n "${TV_BUDDY_BIN:-}${TV_BUDDY_ARCHIVE:-}${TV_BUDDY_URL:-}" ]; then
        new_bin=$(update_source)
    fi

    if svc_managed; then
        need_root
        svc_do stop || true
    else
        daemon_stop
    fi
    if [ -f "$DB" ]; then
        cp -f "$DB" "$DB.bak" 2>/dev/null || true
        info "数据库已备份：$DB.bak"
    fi

    if [ -n "$new_bin" ]; then
        install_binary "$new_bin" || warn "二进制未更新，继续使用当前版本"
    fi

    # 重装只重置配置。用户与令牌保留。
    "$BIN" config reset --force --db "$DB" >/dev/null || fail "重置配置失败"

    port=${TV_BUDDY_PORT:-$(read_default "面板端口" "$old_port")}
    user=${TV_BUDDY_USER:-$DEFAULT_USER}
    password=${TV_BUDDY_PASS:-}
    if [ -z "$password" ] && [ -t 0 ]; then
        printf '面板密码 [回车=%s]：' "$DEFAULT_PASS"
        stty -echo 2>/dev/null || true
        read -r password
        stty echo 2>/dev/null || true
        printf '\n'
    fi
    password=${password:-$DEFAULT_PASS}
    case "$port" in
        ""|*[!0-9]*) fail "端口无效：$port" ;;
    esac
    [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || fail "端口无效：$port"
    [ "${#password}" -ge 4 ] || fail "密码太短（至少 4 位）"

    set_port "$port"
    printf '%s' "$password" | "$BIN" passwd "$user" --db "$DB" --stdin >/dev/null ||
        warn "账号 $user 不存在，未改密码；请在面板创建"
    restart_runtime || fail "配置已重置，但重启失败；见 $LOG_FILE"
    show_url
}

cmd_uninstall() {
    require_installed
    ensure_mutable
    if svc_managed; then
        need_root
        svc_do stop >/dev/null 2>&1 || true
        if has_systemd && [ -f "$SERVICE_FILE" ]; then
            systemctl disable "$SERVICE_NAME" >/dev/null 2>&1 || true
            rm -f "$SERVICE_FILE"
            systemctl daemon-reload 2>/dev/null || true
        elif has_openrc && [ -f "$OPENRC_FILE" ]; then
            rc-update del "$SERVICE_NAME" default >/dev/null 2>&1 || true
            rm -f "$OPENRC_FILE"
        fi
        info "服务已移除"
    else
        daemon_stop
    fi

    if [ -L "$CLI_LINK" ] && [ "$(readlink "$CLI_LINK")" = "$BASE_DIR/tv-buddy.sh" ]; then
        rm -f "$CLI_LINK"
        info "已移除命令：$CLI_LINK"
    fi

    answer=N
    if [ "${TV_BUDDY_YES:-}" = "1" ]; then
        answer=Y
    else
        if [ -t 0 ]; then
            printf '连同 %s（配置、缓存、台标、EPG）一起删除？[y/N] ' "$BASE_DIR"
        fi
        read -r answer || answer=N
    fi
    case "$answer" in
        y|Y)
            case "$BASE_DIR" in
                /|"") fail "拒绝删除：$BASE_DIR" ;;
            esac
            cd /
            rm -rf "$BASE_DIR"
            info "已删除：$BASE_DIR"
            ;;
        *)
            info "已保留：${BASE_DIR}（配置与数据仍在，重装可恢复）"
            ;;
    esac
}

# ---- 交互菜单 ----

state_line() {
    if ! is_installed; then
        printf '%s未安装%s' "$DIM" "$RESET"
        return
    fi
    if svc_managed; then
        if svc_active; then
            printf '%s● 运行中%s' "$GREEN" "$RESET"
        else
            printf '%s○ 已停止%s' "$RED" "$RESET"
        fi
    elif daemon_running; then
        printf '%s● 运行中（PID %s）%s' "$GREEN" "$(cat "$PID_FILE" 2>/dev/null || true)" "$RESET"
    else
        printf '%s○ 已安装，未运行%s' "$YELLOW" "$RESET"
    fi
}

pause() {
    [ -t 0 ] || return 0
    printf '\n按回车返回 ...'
    read -r _ || true
}

main_menu() {
    while :; do
        if [ -t 1 ]; then
            clear 2>/dev/null || true
        fi
        printf '%s=============================================%s\n' "$BLUE" "$RESET"
        printf '       tv-buddy 安装 · 管理   %sv%s%s\n' "$DIM" "$VERSION" "$RESET"
        printf '%s=============================================%s\n' "$BLUE" "$RESET"
        printf '  状态: %s\n' "$(state_line)"
        if is_installed; then
            printf '  目录: %s%s%s\n' "$DIM" "$BASE_DIR" "$RESET"
            printf '  端口: %s\n' "$(get_port)"
        fi
        printf '%s---------------------------------------------%s\n' "$BLUE" "$RESET"
        if is_installed; then
            printf '   1) 更新二进制(保留配置)  2) 查看面板地址\n'
            printf '   3) 启动   4) 停止   5) 重启   6) 状态   7) 日志\n'
            printf '%s--- 面板设置 --------------------------------%s\n' "$BLUE" "$RESET"
            printf '   8) 改端口        9) 改账号密码\n'
            printf '%s---------------------------------------------%s\n' "$BLUE" "$RESET"
            printf '   r) %s重装(清空配置重来)%s   u) %s卸载%s\n' "$YELLOW" "$RESET" "$RED" "$RESET"
        else
            printf '   1) 安装/修复 tv-buddy\n'
        fi
        printf '   0) 退出\n'
        printf '%s---------------------------------------------%s\n' "$BLUE" "$RESET"
        printf '请输入: '
        if ! read -r opt; then
            printf '\n'
            return 0
        fi
        printf '\n'

        if ! is_installed; then
            case "$opt" in
                1)
                    if ( cmd_install --start ); then
                        if [ -x "$CLI_LINK" ]; then
                            info "安装完成，打开已安装菜单"
                            exec "$CLI_LINK" menu
                        fi
                    fi
                    pause
                    ;;
                0|q|Q) return 0 ;;
                *) warn "无效输入"; [ -t 0 ] && sleep 1 || true ;;
            esac
            continue
        fi

        case "$opt" in
            1) ( cmd_update ) || true; pause ;;
            2) ( cmd_url ) || true; pause ;;
            3) ( cmd_start ) || true; pause ;;
            4) ( cmd_stop ) || true; pause ;;
            5) ( cmd_restart ) || true; pause ;;
            6) ( cmd_status ) || true; pause ;;
            7) ( cmd_log ) || true ;;
            8) ( cmd_port ) || true; pause ;;
            9) ( cmd_pass ) || true; pause ;;
            r|R) ( cmd_reinstall ) || true; pause ;;
            u|U)
                printf '确认卸载？[y/N] '
                if read -r answer && { [ "$answer" = "y" ] || [ "$answer" = "Y" ]; }; then
                    ( cmd_uninstall ) || true
                    pause
                else
                    warn "已取消"
                fi
                ;;
            0|q|Q) return 0 ;;
            *) warn "无效输入"; [ -t 0 ] && sleep 1 || true ;;
        esac
    done
}

cmd=${1:-}
[ "$#" -gt 0 ] && shift
case "$cmd" in
    ""|menu) main_menu ;;
    help|-h|--help) usage ;;
    start) cmd_start ;;
    stop) cmd_stop ;;
    restart|reload) cmd_restart ;;
    status|st) cmd_status ;;
    log|logs) cmd_log "$@" ;;
    url) cmd_url ;;
    port) cmd_port "$@" ;;
    pass) cmd_pass "$@" ;;
    update) cmd_update ;;
    reinstall) cmd_reinstall ;;
    install) cmd_install "$@" ;;
    uninstall|remove) cmd_uninstall ;;
    *)
        usage >&2
        fail "未知命令：$cmd"
        ;;
esac
