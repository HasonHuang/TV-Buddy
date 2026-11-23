#!/bin/sh
# TV Buddy 一键安装脚本。
#
# 用法：
#   直接安装：sudo sh install.sh                  # 默认从 GitHub Release 下载同架构包
#   指定版本：sudo sh install.sh --version v2026.3.0
#   发布包目录：sudo ./install.sh                 # 用旁边的已解包文件，不再下载
#   仓库根目录：sudo ./install.sh --local         # 改用 dist/ 中最新的同架构包
#   指定安装包：sudo ./install.sh --archive /path/to/tv-buddy-<version>-linux-<arch>.tar.gz
#   低配机器：  sudo ./install.sh --low-spec   # 直接写入省内存/省 CPU 的参数
#   高配机器：  sudo ./install.sh --no-low-spec # 直接跳过低配参数
#   不指定时：  安装前询问是否使用低配参数
#   反向代理：  sudo ./install.sh --nginx       # 安装 NGINX 并写入反向代理配置
#               sudo ./install.sh --no-nginx    # 不安装 NGINX
#   不指定时：  安装前询问是否安装 NGINX
#   远程安装包：sudo ./install.sh --archive https://example.com/tv-buddy.tar.gz
#
# 只支持 Linux amd64/arm64。脚本幂等；已有数据库不覆盖。
# 装完即启动服务（`--no-start` 可关闭）。

set -eu

PREFIX=${PREFIX:-/opt/tv-buddy}
ARCHIVE=${ARCHIVE:-}
CLI_LINK=${CLI_LINK:-/usr/local/bin/tv-buddy}
# 默认发布源，可用 --repo 或 TV_BUDDY_REPO 覆盖。
GITHUB_REPO=${TV_BUDDY_REPO:-HasonHuang/TV-Buddy}
VERSION=${TV_BUDDY_VERSION:-latest}
DO_SERVICE=1
DO_LINK=1
DO_START=1
USE_DIST=0
# 空值表示尚未选择，安装前询问。1 用低配，0 不用。
LOW_SPEC=
# 空值表示尚未选择，安装前询问。1 安装 NGINX，0 不安装。
DO_NGINX=
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
  --low-spec         使用低配参数（省内存与 CPU），不询问
  --no-low-spec      不使用低配参数，不询问
  --nginx            安装 NGINX 并写入反向代理配置，不询问
  --no-nginx         不安装 NGINX，不询问
  --no-start         安装后不启动服务（默认会启动）
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

# 安装前询问是否使用低配参数。`--low-spec` 与 `--no-low-spec` 跳过询问。
# 无交互终端时默认不使用。
ask_low_spec() {
    [ -z "$LOW_SPEC" ] || return 0

    # 优先读控制终端，兼容 `curl ... | sh` 的安装方式。
    # 用 `true` 探测：`:` 是特殊内建命令，重定向失败会让整个脚本退出。
    if { true </dev/tty; } 2>/dev/null; then
        exec 3</dev/tty
    elif [ -t 0 ]; then
        exec 3<&0
    else
        LOW_SPEC=0
        echo "无交互终端，默认不使用低配参数。需要时加 --low-spec。"
        return 0
    fi

    mem_mb=$(awk '/^MemTotal:/ { printf "%d", $2 / 1024 }' /proc/meminfo 2>/dev/null) || mem_mb=
    cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null) || cores=
    [ -n "$cores" ] || cores=$(nproc 2>/dev/null) || cores=
    case "$mem_mb" in ''|*[!0-9]*) mem_mb= ;; esac
    case "$cores" in ''|*[!0-9]*) cores= ;; esac
    [ -n "$mem_mb" ] || mem_mb=未知
    [ -n "$cores" ] || cores=未知

    echo "检测到：内存 ${mem_mb} MB，CPU ${cores} 核。"
    printf '是否使用低配参数（省内存与 CPU）？[y/N] '
    IFS= read -r answer <&3 || answer=
    exec 3<&-
    case "$answer" in
        [Yy]|[Yy][Ee][Ss]) LOW_SPEC=1 ;;
        *) LOW_SPEC=0 ;;
    esac
    if [ "$LOW_SPEC" = 1 ]; then
        echo "低配模式：启用"
    else
        echo "低配模式：不启用"
    fi
}

# 安装前询问是否安装 NGINX。`--nginx` 与 `--no-nginx` 跳过询问。
# 无交互终端时默认不安装。
ask_nginx() {
    [ -z "$DO_NGINX" ] || return 0

    if { true </dev/tty; } 2>/dev/null; then
        exec 3</dev/tty
    elif [ -t 0 ]; then
        exec 3<&0
    else
        DO_NGINX=0
        echo "无交互终端，默认不安装 NGINX。需要时加 --nginx。"
        return 0
    fi

    if command -v nginx >/dev/null 2>&1; then
        printf 'NGINX 已安装。是否写入反向代理配置（监听 80，备份原配置）？[y/N] '
    else
        printf '是否安装 NGINX 并配置反向代理（监听 80，备份原配置）？[y/N] '
    fi
    IFS= read -r answer <&3 || answer=
    exec 3<&-
    case "$answer" in
        [Yy]|[Yy][Ee][Ss]) DO_NGINX=1 ;;
        *) DO_NGINX=0 ;;
    esac
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
        --no-start)
            DO_START=0
            shift
            ;;
        --low-spec)
            LOW_SPEC=1
            shift
            ;;
        --no-low-spec)
            LOW_SPEC=0
            shift
            ;;
        --nginx)
            DO_NGINX=1
            shift
            ;;
        --no-nginx)
            DO_NGINX=0
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

# 安装动作开始前询问一次低配选择。
ask_low_spec
# 安装动作开始前询问一次 NGINX。
ask_nginx

if [ "$DO_NGINX" = 1 ] && [ "$(id -u)" -ne 0 ]; then
    fail "安装或配置 NGINX 需要 root；可加 --no-nginx 跳过"
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
        | grep -e "-linux-$1\.tar\.gz\$" \
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
    # 产物名：tv-buddy-<version>-linux-<arch>.tar.gz
    for candidate in "$tag" "v${tag#v}"; do
        url="https://github.com/$GITHUB_REPO/releases/download/$candidate/tv-buddy-${candidate#v}-linux-$arch.tar.gz"
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

# 找随包分发的 NGINX 配置。发布包放在顶层；仓库里在 scripts/nginx/。
nginx_conf_src() {
    for candidate in "$ASSET_DIR/nginx.default.conf" "$SRC_DIR/scripts/nginx/default.conf"; do
        if [ -f "$candidate" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done
    return 1
}

# 用发行版的包管理器安装 NGINX。已安装时直接返回 0。
install_nginx_package() {
    command -v nginx >/dev/null 2>&1 && return 0
    echo "安装 NGINX..."
    if command -v apk >/dev/null 2>&1; then
        apk add --no-cache nginx
    elif command -v apt-get >/dev/null 2>&1; then
        DEBIAN_FRONTEND=noninteractive apt-get update -qq
        DEBIAN_FRONTEND=noninteractive apt-get install -y nginx
    elif command -v dnf >/dev/null 2>&1; then
        dnf install -y nginx
    elif command -v yum >/dev/null 2>&1; then
        yum install -y nginx
    else
        echo "找不到 apk/apt-get/dnf/yum，无法安装 NGINX。"
        return 1
    fi
}

# 恢复 NGINX 配置。没有备份时删除写入的文件。
restore_nginx_conf() {
    if [ -n "$2" ]; then
        cp -p "$2" "$1"
    else
        rm -f "$1"
    fi
    echo "已恢复原配置：$1"
}

# 写入反向代理配置，替换发行版的默认站点。
# 原文件备份为 <文件>.bak.<时间>。`nginx -t` 失败时恢复原文件。
configure_nginx() {
    src=$(nginx_conf_src) || {
        echo "警告：找不到 NGINX 配置（nginx.default.conf）。跳过反向代理配置。"
        return 1
    }

    # Alpine 用 http.d；Debian 系用 sites-available；其余用 conf.d。
    if [ -d /etc/nginx/http.d ]; then
        conf_dir=/etc/nginx/http.d
        conf_name=default.conf
    elif [ -d /etc/nginx/sites-available ]; then
        conf_dir=/etc/nginx/sites-available
        conf_name=default
    else
        conf_dir=/etc/nginx/conf.d
        conf_name=default.conf
    fi
    conf="$conf_dir/$conf_name"

    install -d "$conf_dir"
    backup=
    if [ -e "$conf" ]; then
        backup="$conf.bak.$(date +%Y%m%d%H%M%S)"
        cp -p "$conf" "$backup" || {
            echo "警告：无法备份 $conf"
            return 1
        }
    fi

    install -m 0644 "$src" "$conf" || {
        echo "警告：无法写入 $conf"
        restore_nginx_conf "$conf" "$backup"
        return 1
    }

    # Debian 系靠 sites-enabled 里的软链启用站点。
    if [ "$conf_dir" = /etc/nginx/sites-available ]; then
        if [ ! -e /etc/nginx/sites-enabled/default ]; then
            install -d /etc/nginx/sites-enabled
            ln -s ../sites-available/default /etc/nginx/sites-enabled/default
        fi
    fi

    output=$(nginx -t 2>&1) || {
        echo "警告：NGINX 配置校验失败。"
        printf '%s\n' "$output"
        restore_nginx_conf "$conf" "$backup"
        return 1
    }
    # 已有别的默认站点时，请求不会落到本配置。nginx 只警告，不报错。
    if printf '%s\n' "$output" | grep -q "conflicting server name"; then
        echo "警告：NGINX 已有其他默认站点，配置冲突。"
        restore_nginx_conf "$conf" "$backup"
        return 1
    fi
    echo "NGINX 配置：$conf"
    if [ -n "$backup" ]; then
        echo "原配置备份：$backup"
    fi
    return 0
}

# 启动 NGINX 并设置开机自启。失败只警告，不中断安装。
start_nginx() {
    if command -v systemctl >/dev/null 2>&1; then
        systemctl enable nginx >/dev/null 2>&1 || true
        if systemctl restart nginx; then
            echo "NGINX：已启动（systemd）"
            return 0
        fi
        echo "警告：NGINX 启动失败，见 systemctl status nginx"
        return 1
    elif command -v rc-update >/dev/null 2>&1; then
        rc-update add nginx default >/dev/null 2>&1 || true
        if rc-service nginx restart; then
            echo "NGINX：已启动（OpenRC）"
            return 0
        fi
        echo "警告：NGINX 启动失败，见 /var/log/messages"
        return 1
    fi
    echo "警告：未发现 systemd/OpenRC；请手动启动 NGINX"
    return 1
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
elif [ -x "$SCRIPT_DIR/tv-buddy" ]; then
    # 已解包的发布目录：包内容就在旁边，直接用，不必再下载。
    SRC_DIR=$SCRIPT_DIR
    BIN_SRC=$SCRIPT_DIR/tv-buddy
elif [ "$USE_DIST" = 1 ]; then
    # 显式要求用本地构建产物（仓库根目录的开发流程）。
    LATEST=$(ls -t "$SCRIPT_DIR"/dist/tv-buddy-*-linux-"$ARCH".tar.gz 2>/dev/null | head -n 1 || true)
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
if [ -f "$SRC_DIR/packaging/tv-buddy.sh" ]; then
    ASSET_DIR=$SRC_DIR/packaging
else
    ASSET_DIR=$SRC_DIR
fi
[ -f "$ASSET_DIR/tv-buddy.sh" ] || fail "缺少 tv-buddy.sh"

install -d "$PREFIX" "$PREFIX/cache"
install -m 0755 "$BIN_SRC" "$PREFIX/tv-buddy"
install -m 0755 "$ASSET_DIR/tv-buddy.sh" "$PREFIX/tv-buddy.sh"

# 配置、用户与令牌都在一个 SQLite 文件里。
DB="$PREFIX/tv-buddy.db"
if [ -f "$DB" ]; then
    echo "保留已有数据库：$DB"
fi

if [ "$LOW_SPEC" = 1 ]; then
    for pair in \
        "cache.segments.per_channel_max 200" \
        "cache.prefetch.concurrency 2" \
        "auth.token_enabled true" \
        "auth.ip_guard_enabled true" \
        "cdn.enabled true"; do
        # shellcheck disable=SC2086
        set -- $pair
        "$PREFIX/tv-buddy" config set "$1" "$2" --db "$DB" >/dev/null \
            || fail "写入低配参数失败：$1"
    done
    echo "已写入低配参数：$DB"
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

# 启动失败不算安装失败：文件已就位，用户可改完配置再启。
STARTED=0
if [ "$DO_START" = 1 ]; then
    case "$SERVICE_KIND" in
        systemd)
            if systemctl restart tv-buddy; then STARTED=1; else
                echo "警告：systemctl restart tv-buddy 失败，见 systemctl status tv-buddy"
            fi
            ;;
        openrc)
            if rc-service tv-buddy restart; then STARTED=1; else
                echo "警告：rc-service tv-buddy restart 失败，见 /var/log/messages"
            fi
            ;;
        *)
            if "$PREFIX/tv-buddy.sh" restart; then STARTED=1; else
                echo "警告：服务启动失败，见 $PREFIX/tv-buddy.log"
            fi
            ;;
    esac
fi

# NGINX 失败不影响 tv-buddy 安装。
NGINX_CONF_OK=0
NGINX_STARTED=0
if [ "$DO_NGINX" = 1 ]; then
    if install_nginx_package && configure_nginx; then
        NGINX_CONF_OK=1
        if [ "$DO_START" = 1 ]; then
            if start_nginx; then
                NGINX_STARTED=1
            fi
        fi
    fi
fi

echo "安装完成：$PREFIX"
echo "数据：$PREFIX/tv-buddy.db"
echo "面板：http://<主机>:9900/page（账号 admin；初始口令见下）"
echo "初始口令：$PREFIX/cache/admin_initial_credentials.txt（首次启动后生成，权限 600）"
if [ "$STARTED" = 1 ]; then
    echo "状态：服务已启动"
else
    case "$SERVICE_KIND" in
        systemd) echo "启动：systemctl start tv-buddy" ;;
        openrc) echo "启动：rc-service tv-buddy start" ;;
        *) echo "启动：$PREFIX/tv-buddy.sh start" ;;
    esac
fi

if [ "$DO_NGINX" = 1 ]; then
    if [ "$NGINX_CONF_OK" != 1 ]; then
        echo "NGINX：未配置。见上方警告"
    elif [ "$NGINX_STARTED" = 1 ]; then
        echo "NGINX：反向代理已配置并启动，入口 http://<主机>/page"
    elif [ "$DO_START" = 0 ]; then
        echo "NGINX：反向代理已配置，未启动（--no-start）"
    else
        echo "NGINX：反向代理已配置，启动失败。见上方警告"
    fi
fi
