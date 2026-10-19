#!/bin/sh
# 启停脚本：iptv-proxy.sh {start|stop|restart|status|log}
# 布局见 docs/design/04-config-and-storage.md §6。

set -eu

BASE_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BIN="$BASE_DIR/iptv-proxy"
CFG="$BASE_DIR/config.toml"
PID_FILE="$BASE_DIR/iptv-proxy.pid"
LOG_FILE="$BASE_DIR/iptv-proxy.log"

running() {
    [ -f "$PID_FILE" ] || return 1
    pid=$(cat "$PID_FILE" 2>/dev/null || true)
    [ -n "$pid" ] || return 1
    kill -0 "$pid" 2>/dev/null
}

start() {
    if running; then
        echo "已在运行（pid $(cat "$PID_FILE")）"
        return 0
    fi
    [ -x "$BIN" ] || { echo "缺少可执行文件：$BIN"; exit 1; }
    [ -f "$CFG" ] || { echo "缺少配置文件：$CFG"; exit 1; }
    ( cd "$BASE_DIR" && "$BIN" "$CFG" >>"$LOG_FILE" 2>&1 & echo $! >"$PID_FILE" )
    sleep 1
    if running; then
        echo "已启动（pid $(cat "$PID_FILE")）"
    else
        echo "启动失败，见 $LOG_FILE"
        exit 1
    fi
}

stop() {
    if ! running; then
        echo "未运行"
        rm -f "$PID_FILE"
        return 0
    fi
    pid=$(cat "$PID_FILE")
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$PID_FILE"
    echo "已停止"
}

status() {
    if running; then
        echo "运行中（pid $(cat "$PID_FILE")）"
    else
        echo "未运行"
        exit 1
    fi
}

case "${1:-}" in
    start) start ;;
    stop) stop ;;
    restart) stop; start ;;
    status) status ;;
    log) tail -n "${2:-50}" "$LOG_FILE" ;;
    *)
        echo "用法：$0 {start|stop|restart|status|log [行数]}"
        exit 2
        ;;
esac
