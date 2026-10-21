#!/bin/sh
# IPTV Proxy 管理 CLI：启动、停止、改端口、改密码、更新、卸载。
#
# 安装后由 install.sh 软链为 /usr/local/bin/iptv-proxy。
# 无 systemd/OpenRC 时退回 PID 文件方式，便于容器内运行。

set -eu

# 解析软链，保证 /usr/local/bin/iptv-proxy 找到真实安装目录。
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
BIN="$BASE_DIR/iptv-proxy"
CFG="$BASE_DIR/config.toml"
CFG_EXAMPLE="$BASE_DIR/config.toml.example"
LOG_FILE="$BASE_DIR/iptv-proxy.log"
PID_FILE="$BASE_DIR/iptv-proxy.pid"
SERVICE_NAME=iptv-proxy
SERVICE_FILE="/etc/systemd/system/$SERVICE_NAME.service"
OPENRC_FILE="/etc/init.d/$SERVICE_NAME"
CLI_LINK=${CLI_LINK:-/usr/local/bin/iptv-proxy}
DEFAULT_PORT=19899
DEFAULT_USER=admin
DEFAULT_PASS=admin
TMP_DIR=

cleanup() {
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
iptv-proxy 管理 CLI

用法：iptv-proxy <命令> [参数]

命令：
  start                 启动服务
  stop                  停止服务
  restart               重启服务
  status                查看服务状态
  log [行数]            查看日志；终端下持续跟随
  url                   打印面板地址与账号
  port <N>              修改面板端口并重启
  pass <密码> [账号]    修改面板密码，可同时改账号
  update                更新二进制，保留配置
  reinstall             备份配置后重装
  uninstall             卸载服务；询问是否删除数据

update 来源（按优先级）：
  IPTV_PROXY_BIN        本地二进制
  IPTV_PROXY_ARCHIVE    本地 tar.gz
  IPTV_PROXY_URL        远程 tar.gz 或二进制，支持 {arch} 占位符

非交互：
  IPTV_PROXY_YES=1      跳过 reinstall / uninstall 的删除确认

示例：
  iptv-proxy port 8080
  iptv-proxy pass 'new-password' admin
  IPTV_PROXY_URL=https://example.com/iptv-proxy-{arch}.tar.gz iptv-proxy update
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

svc_do() {
    if has_systemd && [ -f "$SERVICE_FILE" ]; then
        systemctl "$1" "$SERVICE_NAME"
    elif has_openrc && [ -f "$OPENRC_FILE" ]; then
        rc-service "$SERVICE_NAME" "$1"
    else
        return 2
    fi
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
    [ -f "$CFG" ] || fail "缺少配置文件：$CFG"
    if daemon_running; then
        return 0
    fi
    ( cd "$BASE_DIR" && "$BIN" "$CFG" >>"$LOG_FILE" 2>&1 & echo $! >"$PID_FILE" )
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

# ---- config.toml 读写：按段落定位，避免改到同名字段 ----

cfg_get() {
    [ -f "$CFG" ] || return 1
    awk -v s="[$1]" -v k="$2" '
        function trim(x) {
            sub(/^[ \t]+/, "", x)
            sub(/[ \t\r]+$/, "", x)
            return x
        }
        {
            t = trim($0)
            if (t ~ /^\[/) {
                inx = (t == s)
                next
            }
            if (!inx) next
            if (t ~ "^" k "[ \t]*=") {
                sub(/^[^=]*=[ \t]*/, "", t)
                sub(/[ \t]*#.*$/, "", t)
                t = trim(t)
                if (t ~ /^".*"$/) {
                    t = substr(t, 2, length(t) - 2)
                }
                print t
                exit
            }
        }' "$CFG"
}

cfg_set() {
    section=$1
    key=$2
    value=$3
    [ -f "$CFG" ] || fail "配置文件不存在：$CFG"
    tmp=$(mktemp "$CFG.XXXXXX") || fail "无法创建临时文件"
    awk -v s="[$section]" -v k="$key" -v v="$value" '
        function trim(x) {
            sub(/^[ \t]+/, "", x)
            sub(/[ \t\r]+$/, "", x)
            return x
        }
        BEGIN { inx = 0; done = 0; found = 0 }
        {
            t = trim($0)
            if (t ~ /^\[/) {
                if (inx && !done) {
                    print k " = " v
                    done = 1
                }
                inx = (t == s)
                if (inx) found = 1
                print
                next
            }
            if (inx && !done && t ~ "^" k "[ \t]*=") {
                print k " = " v
                done = 1
                next
            }
            print
        }
        END {
            if (inx && !done) {
                print k " = " v
            }
            if (!found) {
                print ""
                print s
                print k " = " v
            }
        }' "$CFG" >"$tmp" || {
        rm -f "$tmp"
        fail "写入配置失败"
    }
    chmod 600 "$tmp"
    mv -f "$tmp" "$CFG"
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

sha256hex() {
    if command -v sha256sum >/dev/null 2>&1; then
        printf '%s' "$1" | sha256sum | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        printf '%s' "$1" | shasum -a 256 | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        printf '%s' "$1" | openssl dgst -sha256 2>/dev/null | awk '{print $NF}'
    else
        fail "缺少 sha256sum / shasum / openssl"
    fi
}

toml_escape() {
    printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
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
    user=$(cfg_get panel username || true)
    printf '面板地址：http://%s:%s/panel\n' "$host" "$port"
    printf '账号：%s\n' "${user:-$DEFAULT_USER}"
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
    src=$(find "$TMP_DIR" -type f -name iptv-proxy | head -n 1)
    [ -n "$src" ] || fail "安装包内没有 iptv-proxy：$archive"
    printf '%s' "$src"
}

update_source() {
    if [ -n "${IPTV_PROXY_BIN:-}" ]; then
        [ -f "$IPTV_PROXY_BIN" ] || fail "二进制不存在：$IPTV_PROXY_BIN"
        printf '%s' "$IPTV_PROXY_BIN"
        return
    fi
    if [ -n "${IPTV_PROXY_ARCHIVE:-}" ]; then
        extract_binary "$IPTV_PROXY_ARCHIVE"
        return
    fi
    if [ -n "${IPTV_PROXY_URL:-}" ]; then
        arch=$(detect_arch)
        url=$(printf '%s' "$IPTV_PROXY_URL" | sed "s/{arch}/$arch/g")
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
        latest=$(ls -t "$dir"/iptv-proxy-*-"$arch".tar.gz 2>/dev/null | head -n 1 || true)
        if [ -n "$latest" ]; then
            extract_binary "$latest"
            return
        fi
    done
    fail "未配置更新源：设置 IPTV_PROXY_BIN、IPTV_PROXY_ARCHIVE 或 IPTV_PROXY_URL"
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
    new="$BASE_DIR/.iptv-proxy.new.$$"
    cp "$src" "$new" || {
        warn "复制二进制失败"
        return 1
    }
    chmod 0755 "$new"
    mv -f "$new" "$BIN"
}

# ---- 命令实现 ----

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
            systemctl is-active --quiet "$SERVICE_NAME" || exit 1
        else
            rc-service "$SERVICE_NAME" status 2>/dev/null || exit 1
        fi
    elif daemon_running; then
        info "运行中（pid $(cat "$PID_FILE")）"
    else
        info "未运行"
        exit 1
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
        [ -t 0 ] || fail "用法：iptv-proxy port <N>"
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

cmd_pass() {
    require_installed
    password=${1:-}
    account=${2:-}
    if [ -z "$password" ]; then
        [ -t 0 ] || fail "用法：iptv-proxy pass <密码> [账号]"
        printf '新密码：'
        stty -echo 2>/dev/null || true
        read -r password
        stty echo 2>/dev/null || true
        printf '\n'
    fi
    [ "${#password}" -ge 4 ] || fail "密码太短（至少 4 位）"
    ensure_mutable
    hash=$(sha256hex "$password")
    cfg_set panel password_sha256 "\"$hash\""
    if [ -n "$account" ]; then
        cfg_set panel username "\"$(toml_escape "$account")\""
    fi
    restart_runtime || fail "密码已写入配置，但重启失败；见 $LOG_FILE"
    info "密码已更新，旧登录会话失效"
    [ -n "$account" ] && info "账号已改为：$account"
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
        warn "二进制已更新，但健康检查未通过；用 iptv-proxy log 查看"
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
    if [ "${IPTV_PROXY_YES:-}" = "1" ]; then
        :
    else
        if [ -t 0 ]; then
            printf '重装会备份配置并清空后重来，确认继续？[y/N] '
        fi
        if read -r answer && { [ "$answer" = "y" ] || [ "$answer" = "Y" ]; }; then
            :
        else
            info "已取消（非交互重装可设 IPTV_PROXY_YES=1）"
            return
        fi
    fi
    old_port=$(get_port)
    old_user=$(cfg_get panel username || true)
    [ -n "$old_user" ] || old_user=$DEFAULT_USER

    # 先取更新源，避免停服后才发现下载地址不可用。
    new_bin=
    if [ -n "${IPTV_PROXY_BIN:-}${IPTV_PROXY_ARCHIVE:-}${IPTV_PROXY_URL:-}" ]; then
        new_bin=$(update_source)
    fi

    if svc_managed; then
        need_root
        svc_do stop || true
    else
        daemon_stop
    fi
    cp -f "$CFG" "$CFG.bak" 2>/dev/null || true
    info "配置已备份：$CFG.bak"

    if [ -n "$new_bin" ]; then
        install_binary "$new_bin" || warn "二进制未更新，继续使用当前版本"
    fi

    rm -f "$CFG"
    if [ -f "$CFG_EXAMPLE" ]; then
        cp "$CFG_EXAMPLE" "$CFG"
    else
        : >"$CFG"
    fi
    chmod 600 "$CFG"

    port=${IPTV_PROXY_PORT:-$(read_default "面板端口" "$old_port")}
    user=${IPTV_PROXY_USER:-$(read_default "面板账号" "$old_user")}
    password=${IPTV_PROXY_PASS:-}
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
    cfg_set panel username "\"$(toml_escape "$user")\""
    cfg_set panel password_sha256 "\"$(sha256hex "$password")\""
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

    if [ -L "$CLI_LINK" ] && [ "$(readlink "$CLI_LINK")" = "$BASE_DIR/iptv-proxy.sh" ]; then
        rm -f "$CLI_LINK"
        info "已移除命令：$CLI_LINK"
    fi

    answer=N
    if [ "${IPTV_PROXY_YES:-}" = "1" ]; then
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

cmd=${1:-}
[ "$#" -gt 0 ] && shift
case "$cmd" in
    ""|menu|help|-h|--help) usage ;;
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
    uninstall|remove) cmd_uninstall ;;
    *)
        usage >&2
        fail "未知命令：$cmd"
        ;;
esac
