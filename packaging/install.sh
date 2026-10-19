#!/bin/sh
# 安装到 /opt/iptv-proxy（幂等；已有 config.toml 不覆盖）。
#
# 用法：sudo ./install.sh [二进制路径]
# 环境变量：PREFIX 覆盖安装目录。

set -eu

PREFIX=${PREFIX:-/opt/iptv-proxy}
SRC=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BIN_SRC=${1:-$SRC/iptv-proxy}

[ -f "$BIN_SRC" ] || { echo "缺少二进制：$BIN_SRC"; exit 1; }

install -d "$PREFIX" "$PREFIX/cache"
install -m 0755 "$BIN_SRC" "$PREFIX/iptv-proxy"
install -m 0755 "$SRC/iptv-proxy.sh" "$PREFIX/iptv-proxy.sh"
if [ -f "$PREFIX/config.toml" ]; then
    echo "保留已有配置：$PREFIX/config.toml"
else
    install -m 0644 "$SRC/config.toml.example" "$PREFIX/config.toml"
    echo "已生成配置：$PREFIX/config.toml（请修改后重启）"
fi

if command -v systemctl >/dev/null 2>&1; then
    install -m 0644 "$SRC/iptv-proxy.service" /etc/systemd/system/iptv-proxy.service
    systemctl daemon-reload
    systemctl enable iptv-proxy >/dev/null 2>&1 || true
    echo "systemd：systemctl start iptv-proxy"
elif command -v rc-update >/dev/null 2>&1; then
    install -m 0755 "$SRC/iptv-proxy.openrc" /etc/init.d/iptv-proxy
    rc-update add iptv-proxy default >/dev/null 2>&1 || true
    echo "OpenRC：rc-service iptv-proxy start"
else
    echo "未发现 systemd/OpenRC；用 $PREFIX/iptv-proxy.sh start 启动"
fi

echo "安装完成：$PREFIX"
