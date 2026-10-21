#!/bin/sh
# IPTV Proxy 一键安装脚本。
#
# 用法：
#   发布包目录：sudo ./install.sh
#   仓库根目录：sudo ./install.sh              # 自动使用 dist/ 中最新的同架构包
#   指定安装包：sudo ./install.sh --archive /path/to/iptv-proxy-<version>-<arch>.tar.gz
#   远程安装包：sudo ./install.sh --archive https://example.com/iptv-proxy.tar.gz
#
# 只支持 Linux amd64/arm64。脚本幂等；已有 config.toml 不覆盖。

set -eu

PREFIX=${PREFIX:-/opt/iptv-proxy}
ARCHIVE=${ARCHIVE:-}
DO_SERVICE=1
DO_START=0
TMP_DIR=
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

usage() {
    cat <<'EOF'
用法：install.sh [选项]

选项：
  --prefix <目录>    安装目录（默认 /opt/iptv-proxy）
  --archive <路径>   指定 tar.gz 安装包；也接受 http(s) URL
  --no-service       不注册 systemd/OpenRC 服务
  --start            安装后立即启动服务
  -h, --help         显示本帮助

环境变量：
  PREFIX             同 --prefix
  ARCHIVE            同 --archive
EOF
}

fail() {
    echo "错误：$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}
trap cleanup EXIT HUP INT TERM

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix)
            [ "$#" -ge 2 ] || fail "--prefix 缺少参数"
            PREFIX=$2
            shift 2
            ;;
        --archive)
            [ "$#" -ge 2 ] || fail "--archive 缺少参数"
            ARCHIVE=$2
            shift 2
            ;;
        --no-service)
            DO_SERVICE=0
            shift
            ;;
        --start)
            DO_START=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "未知参数：$1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case "$PREFIX" in
    /*) ;;
    *) fail "安装目录必须是绝对路径：$PREFIX" ;;
esac
case "$PREFIX" in
    *" "*|*"	"*) fail "安装目录不能含空白：$PREFIX" ;;
esac
# 供 sed 替换服务单元中的默认目录。
PREFIX_SED=$(printf '%s' "$PREFIX" | sed 's/[\\&|]/\\&/g')

[ "$(uname -s)" = "Linux" ] || fail "只支持 Linux"

case "$(uname -m)" in
    x86_64|amd64) ARCH=amd64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    *) fail "不支持的架构：$(uname -m)（只支持 amd64/arm64）" ;;
esac

if [ "$DO_SERVICE" = 1 ] && [ "$(id -u)" -ne 0 ]; then
    fail "注册系统服务需要 root；可改用 sudo，或加 --no-service"
fi

# 校验同名 .sha256 文件。缺失时跳过。
verify_digest() {
    file=$1
    sum_file="$file.sha256"
    [ -f "$sum_file" ] || return 0
    expected=$(awk 'NR==1 {print $1}' "$sum_file")
    [ -n "$expected" ] || return 0
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$file" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$file" | awk '{print $1}')
    else
        echo "未找到 sha256 工具，跳过校验"
        return 0
    fi
    [ "$expected" = "$actual" ] || fail "校验失败：$file"
    echo "校验通过：$(basename "$file")"
}

download() {
    url=$1
    dest=$2
    if command -v curl >/dev/null 2>&1; then
        curl -fL --retry 2 --connect-timeout 10 -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "$dest" "$url"
    else
        fail "需要 curl 或 wget 下载安装包"
    fi
}

extract_archive() {
    archive=$1
    TMP_DIR=$(mktemp -d)
    tar -xzf "$archive" -C "$TMP_DIR" || fail "解包失败：$archive"
    BIN_SRC=$(find "$TMP_DIR" -type f -name iptv-proxy -perm -u+x | head -n 1)
    [ -n "$BIN_SRC" ] || fail "安装包内没有 iptv-proxy：$archive"
    SRC_DIR=$(dirname "$BIN_SRC")
}

if [ -n "$ARCHIVE" ]; then
    case "$ARCHIVE" in
        http://*|https://*)
            TMP_DIR=$(mktemp -d)
            echo "下载：$ARCHIVE"
            download "$ARCHIVE" "$TMP_DIR/pkg.tar.gz"
            if ( download "$ARCHIVE.sha256" "$TMP_DIR/pkg.tar.gz.sha256" ) >/dev/null 2>&1; then
                :
            else
                rm -f "$TMP_DIR/pkg.tar.gz.sha256"
            fi
            ARCHIVE="$TMP_DIR/pkg.tar.gz"
            ;;
    esac
    [ -f "$ARCHIVE" ] || fail "安装包不存在：$ARCHIVE"
    verify_digest "$ARCHIVE"
    extract_archive "$ARCHIVE"
elif [ -x "$SCRIPT_DIR/iptv-proxy" ] && [ -f "$SCRIPT_DIR/config.toml.example" ]; then
    # 已解包的发布目录。
    SRC_DIR=$SCRIPT_DIR
    BIN_SRC=$SCRIPT_DIR/iptv-proxy
else
    # 仓库根目录：使用 dist/ 中最新的同架构包。
    LATEST=$(ls -t "$SCRIPT_DIR"/dist/iptv-proxy-*-"$ARCH".tar.gz 2>/dev/null | head -n 1 || true)
    [ -n "$LATEST" ] || fail "未找到安装包。先运行 scripts/build-release.sh $ARCH，或用 --archive 指定 tar.gz"
    verify_digest "$LATEST"
    extract_archive "$LATEST"
fi

# 发布包把资源放在顶层；仓库里在 packaging/。
if [ -f "$SRC_DIR/packaging/config.toml.example" ]; then
    ASSET_DIR=$SRC_DIR/packaging
else
    ASSET_DIR=$SRC_DIR
fi
[ -f "$ASSET_DIR/config.toml.example" ] || fail "缺少 config.toml.example"
[ -f "$ASSET_DIR/iptv-proxy.sh" ] || fail "缺少 iptv-proxy.sh"

install -d "$PREFIX" "$PREFIX/cache"
install -m 0755 "$BIN_SRC" "$PREFIX/iptv-proxy"
install -m 0755 "$ASSET_DIR/iptv-proxy.sh" "$PREFIX/iptv-proxy.sh"
install -m 0644 "$ASSET_DIR/config.toml.example" "$PREFIX/config.toml.example"

if [ -f "$PREFIX/config.toml" ]; then
    echo "保留已有配置：$PREFIX/config.toml"
else
    install -m 0644 "$ASSET_DIR/config.toml.example" "$PREFIX/config.toml"
    echo "已生成配置：$PREFIX/config.toml（请修改后重启）"
fi

SERVICE_KIND=none
if [ "$DO_SERVICE" = 1 ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -f "$ASSET_DIR/iptv-proxy.service" ]; then
        sed "s|/opt/iptv-proxy|$PREFIX_SED|g" "$ASSET_DIR/iptv-proxy.service" \
            > /etc/systemd/system/iptv-proxy.service
        systemctl daemon-reload || true
        systemctl enable iptv-proxy >/dev/null 2>&1 || true
        SERVICE_KIND=systemd
    elif command -v rc-update >/dev/null 2>&1 && [ -f "$ASSET_DIR/iptv-proxy.openrc" ]; then
        sed "s|/opt/iptv-proxy|$PREFIX_SED|g" "$ASSET_DIR/iptv-proxy.openrc" \
            > /etc/init.d/iptv-proxy
        chmod 0755 /etc/init.d/iptv-proxy
        rc-update add iptv-proxy default >/dev/null 2>&1 || true
        SERVICE_KIND=openrc
    else
        echo "未发现 systemd/OpenRC；用 $PREFIX/iptv-proxy.sh start 启动"
    fi
fi

if [ "$DO_START" = 1 ]; then
    case "$SERVICE_KIND" in
        systemd) systemctl restart iptv-proxy ;;
        openrc) rc-service iptv-proxy restart ;;
        *) "$PREFIX/iptv-proxy.sh" restart ;;
    esac
fi

echo "安装完成：$PREFIX"
echo "配置：$PREFIX/config.toml"
echo "面板：http://<主机>:19899/panel（默认 admin / admin，请及时修改）"
case "$SERVICE_KIND" in
    systemd) echo "启动：systemctl start iptv-proxy" ;;
    openrc) echo "启动：rc-service iptv-proxy start" ;;
    *) echo "启动：$PREFIX/iptv-proxy.sh start" ;;
esac
