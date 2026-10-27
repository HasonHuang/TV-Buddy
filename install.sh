#!/bin/sh
# TV Buddy 一键安装脚本。
#
# 用法：
#   直接安装：sudo sh install.sh                  # 默认从 GitHub Release 下载同架构包
#   指定版本：sudo sh install.sh --version v2026.3.0
#   发布包目录：sudo ./install.sh                 # 用旁边的已解包文件，不再下载
#   仓库根目录：sudo ./install.sh --local         # 改用 dist/ 中最新的同架构包
#   指定安装包：sudo ./install.sh --archive /path/to/tv-buddy-<version>-<arch>.tar.gz
#   远程安装包：sudo ./install.sh --archive https://example.com/tv-buddy.tar.gz
#
# 只支持 Linux amd64/arm64。脚本幂等；已有 config.toml 不覆盖。

set -eu

PREFIX=${PREFIX:-/opt/tv-buddy}
ARCHIVE=${ARCHIVE:-}
CLI_LINK=${CLI_LINK:-/usr/local/bin/tv-buddy}
# 默认发布源，可用 --repo 或 TV_BUDDY_REPO 覆盖。
GITHUB_REPO=${TV_BUDDY_REPO:-HasonHuang/TV-Buddy}
VERSION=${TV_BUDDY_VERSION:-latest}
DO_SERVICE=1
DO_LINK=1
DO_START=0
USE_DIST=0
TMP_DIR=
DOWNLOADED=
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

usage() {
    cat <<'EOF'
用法：install.sh [选项]

不带 --archive 且当前目录不是发布包时，从 GitHub Release 下载同架构安装包。

选项：
  --prefix <目录>    安装目录（默认 /opt/tv-buddy）
  --archive <路径>   指定 tar.gz 安装包；也接受 http(s) URL
  --version <tag>    从 Release 安装指定版本，如 v2026.3.0（默认取最新）
  --repo <owner/name> Release 所在仓库（默认内置）
  --local            不从 Release 下载，改用本地 dist/ 中的安装包
  --no-service       不注册 systemd/OpenRC 服务
  --no-link          不创建 tv-buddy 命令软链
  --start            安装后立即启动服务
  -h, --help         显示本帮助

环境变量：
  PREFIX             同 --prefix
  ARCHIVE            同 --archive
  TV_BUDDY_VERSION 同 --version
  TV_BUDDY_REPO    同 --repo
  CLI_LINK           命令软链位置（默认 /usr/local/bin/tv-buddy）
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

ensure_tmp() {
    if [ -z "$TMP_DIR" ]; then
        TMP_DIR=$(mktemp -d)
    fi
}

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
        --version)
            [ "$#" -ge 2 ] || fail "--version 缺少参数"
            VERSION=$2
            shift 2
            ;;
        --repo)
            [ "$#" -ge 2 ] || fail "--repo 缺少参数"
            GITHUB_REPO=$2
            shift 2
            ;;
        --local)
            USE_DIST=1
            shift
            ;;
        --no-service)
            DO_SERVICE=0
            shift
            ;;
        --no-link)
            DO_LINK=0
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
case "$CLI_LINK" in
    /*) ;;
    *) fail "CLI_LINK 必须是绝对路径：$CLI_LINK" ;;
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

# 取 GitHub API 的 JSON 到标准输出。失败时返回非 0，由调用方决定是否回退。
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

# 解析 `releases/latest` 的重定向地址，得到 tag（不经 API，省掉限流）。
latest_tag() {
    command -v curl >/dev/null 2>&1 || return 1
    redirect=$(curl -fsS -o /dev/null -w '%{redirect_url}' --max-time 10 \
        "https://github.com/$GITHUB_REPO/releases/latest" 2>/dev/null) || return 1
    tag=${redirect##*/tag/}
    [ -n "$tag" ] && [ "$tag" != "$redirect" ] || return 1
    printf '%s' "$tag"
}

# 从 Release 列表里挑一条与架构匹配的资产地址。
# `releases/latest` 只认正式版；仓库只有预发布（tag 带 -beta/-rc）时会 404，
# 这时退回列表里最新的一条（含预发布）。
release_asset_url() { # $1=arch $2=json
    printf '%s\n' "$2" \
        | grep -o '"browser_download_url"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | sed 's/.*"\(https[^"]*\)"/\1/' \
        | grep -e "-$1\.tar\.gz\$" \
        | head -n 1
}

# 查 Release 里与当前架构匹配的 tar.gz。优先枚举真实资产名；
# API 不可用（限流/无 curl）时按 tag 拼下载地址。
release_url() {
    arch=$1
    tag=
    if [ "$VERSION" != "latest" ]; then
        tag=$VERSION
    else
        tag=$(latest_tag) || tag=
    fi

    probe=
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
        # 指定了版本却没有该架构的包：版本/架构不对，不必再猜地址。
        [ "$VERSION" = "latest" ] || return 1
    fi

    if [ "$VERSION" = "latest" ]; then
        # 没有正式版（或正式版里没有该架构的包）：退回最新一条 release，含预发布。
        list=$(fetch_json "https://api.github.com/repos/$GITHUB_REPO/releases?per_page=30" 2>/dev/null) || list=
        url=$(release_asset_url "$arch" "$list")
        if [ -n "$url" ]; then
            echo "未找到正式版，改用最新 Release：${url##*/}" >&2
            printf '%s' "$url"
            return 0
        fi
    fi

    [ -n "$tag" ] || return 1
    # 产物名：tv-buddy-<version>-<arch>.tar.gz
    for candidate in "$tag" "v${tag#v}"; do
        url="https://github.com/$GITHUB_REPO/releases/download/$candidate/tv-buddy-${candidate#v}-$arch.tar.gz"
        if command -v curl >/dev/null 2>&1; then
            code=$(curl -fsSL -o /dev/null -w '%{http_code}' --max-time 10 "$url" || true)
            [ "$code" = "200" ] || continue
        fi
        printf '%s' "$url"
        return 0
    done
    return 1
}

extract_archive() {
    archive=$1
    ensure_tmp
    # 解包到独立子目录，避免覆盖 TMP_DIR 里已下载的 pkg.tar.gz。
    EXTRACT_DIR="$TMP_DIR/extract"
    rm -rf "$EXTRACT_DIR"
    mkdir -p "$EXTRACT_DIR"
    tar -xzf "$archive" -C "$EXTRACT_DIR" || fail "解包失败：$archive"
    BIN_SRC=$(find "$EXTRACT_DIR" -type f -name tv-buddy -perm -u+x | head -n 1)
    [ -n "$BIN_SRC" ] || fail "安装包内没有 tv-buddy：$archive"
    SRC_DIR=$(dirname "$BIN_SRC")
}

# 下载 tar.gz 及其同名 .sha256 到 TMP_DIR，结果写入全局 DOWNLOADED。
# 用全局变量而非 stdout：命令替换会进子 shell，TMP_DIR 的赋值带不回来，
# 临时目录就没人清理了。
fetch_release_archive() {
    url=$1
    ensure_tmp
    echo "下载：$url" >&2
    download "$url" "$TMP_DIR/pkg.tar.gz" || fail "下载失败：$url"
    if ( download "$url.sha256" "$TMP_DIR/pkg.tar.gz.sha256" ) >/dev/null 2>&1; then
        :
    else
        rm -f "$TMP_DIR/pkg.tar.gz.sha256"
    fi
    DOWNLOADED="$TMP_DIR/pkg.tar.gz"
}

if [ -n "$ARCHIVE" ]; then
    case "$ARCHIVE" in
        http://*|https://*)
            fetch_release_archive "$ARCHIVE"
            ARCHIVE=$DOWNLOADED
            ;;
    esac
    [ -f "$ARCHIVE" ] || fail "安装包不存在：$ARCHIVE"
    verify_digest "$ARCHIVE"
    extract_archive "$ARCHIVE"
elif [ -x "$SCRIPT_DIR/tv-buddy" ] && [ -f "$SCRIPT_DIR/config.toml.example" ]; then
    # 已解包的发布目录：包内容就在旁边，直接用，不必再下载。
    SRC_DIR=$SCRIPT_DIR
    BIN_SRC=$SCRIPT_DIR/tv-buddy
elif [ "$USE_DIST" = 1 ]; then
    # 显式要求用本地构建产物（仓库根目录的开发流程）。
    LATEST=$(ls -t "$SCRIPT_DIR"/dist/tv-buddy-*-"$ARCH".tar.gz 2>/dev/null | head -n 1 || true)
    [ -n "$LATEST" ] || fail "未找到本地安装包。先运行 scripts/build-release.sh ${ARCH}，或去掉 --local 直接从 Release 安装"
    verify_digest "$LATEST"
    extract_archive "$LATEST"
else
    # 默认：从 GitHub Release 下载同架构安装包。
    URL=$(release_url "$ARCH") || fail "在 $GITHUB_REPO 的 Release $VERSION 中找不到 $ARCH 安装包。若最新版本是预发布（tag 带 -beta/-rc），/releases/latest 不含它，请用 --version <tag> 指定；也可用 --archive 直接给安装包地址"
    fetch_release_archive "$URL"
    verify_digest "$DOWNLOADED"
    extract_archive "$DOWNLOADED"
fi

# 发布包把资源放在顶层；仓库里在 packaging/。
if [ -f "$SRC_DIR/packaging/config.toml.example" ]; then
    ASSET_DIR=$SRC_DIR/packaging
else
    ASSET_DIR=$SRC_DIR
fi
[ -f "$ASSET_DIR/config.toml.example" ] || fail "缺少 config.toml.example"
[ -f "$ASSET_DIR/tv-buddy.sh" ] || fail "缺少 tv-buddy.sh"

install -d "$PREFIX" "$PREFIX/cache"
install -m 0755 "$BIN_SRC" "$PREFIX/tv-buddy"
install -m 0755 "$ASSET_DIR/tv-buddy.sh" "$PREFIX/tv-buddy.sh"
install -m 0644 "$ASSET_DIR/config.toml.example" "$PREFIX/config.toml.example"

if [ -f "$PREFIX/config.toml" ]; then
    echo "保留已有配置：$PREFIX/config.toml"
else
    install -m 0644 "$ASSET_DIR/config.toml.example" "$PREFIX/config.toml"
    echo "已生成配置：$PREFIX/config.toml（请修改后重启）"
fi

if [ "$DO_LINK" = 1 ]; then
    link_dir=$(dirname "$CLI_LINK")
    if [ -e "$CLI_LINK" ] && [ ! -L "$CLI_LINK" ]; then
        echo "跳过命令软链：$CLI_LINK 已存在且不是软链"
    elif [ -d "$link_dir" ] || mkdir -p "$link_dir" 2>/dev/null; then
        if [ -w "$link_dir" ] || [ "$(id -u)" -eq 0 ]; then
            if ln -sf "$PREFIX/tv-buddy.sh" "$CLI_LINK"; then
                echo "命令：$CLI_LINK"
            else
                echo "警告：无法创建命令软链：$CLI_LINK"
            fi
        else
            echo "警告：无权限创建命令软链：$CLI_LINK"
        fi
    else
        echo "警告：无法创建目录：$link_dir"
    fi
fi

SERVICE_KIND=none
if [ "$DO_SERVICE" = 1 ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -f "$ASSET_DIR/tv-buddy.service" ]; then
        sed "s|/opt/tv-buddy|$PREFIX_SED|g" "$ASSET_DIR/tv-buddy.service" \
            > /etc/systemd/system/tv-buddy.service
        systemctl daemon-reload || true
        systemctl enable tv-buddy >/dev/null 2>&1 || true
        SERVICE_KIND=systemd
    elif command -v rc-update >/dev/null 2>&1 && [ -f "$ASSET_DIR/tv-buddy.openrc" ]; then
        sed "s|/opt/tv-buddy|$PREFIX_SED|g" "$ASSET_DIR/tv-buddy.openrc" \
            > /etc/init.d/tv-buddy
        chmod 0755 /etc/init.d/tv-buddy
        rc-update add tv-buddy default >/dev/null 2>&1 || true
        SERVICE_KIND=openrc
    else
        echo "未发现 systemd/OpenRC；用 $PREFIX/tv-buddy.sh start 启动"
    fi
fi

if [ "$DO_START" = 1 ]; then
    case "$SERVICE_KIND" in
        systemd) systemctl restart tv-buddy ;;
        openrc) rc-service tv-buddy restart ;;
        *) "$PREFIX/tv-buddy.sh" restart ;;
    esac
fi

echo "安装完成：$PREFIX"
echo "配置：$PREFIX/config.toml"
echo "面板：http://<主机>:19899/panel（默认 admin / admin，请及时修改）"
case "$SERVICE_KIND" in
    systemd) echo "启动：systemctl start tv-buddy" ;;
    openrc) echo "启动：rc-service tv-buddy start" ;;
    *) echo "启动：$PREFIX/tv-buddy.sh start" ;;
esac
