# 01 · Linux 安装

> 状态：操作指南 ｜ 最后更新：2026-11-22
> 适用：Linux amd64/arm64。macOS 见 §11。
> 相关：[`README.md`](README.md)、[`05-operations.md`](05-operations.md)

## 1. 前提

| 项 | 要求 |
| --- | --- |
| 操作系统 | Linux |
| 架构 | `x86_64`/`amd64` 或 `aarch64`/`arm64` |
| 权限 | 注册系统服务与安装 NGINX 需要 root 或 sudo |
| 端口 | TCP `9900` 可用。可用 `tv-buddy port <N>` 修改 |
| 外网 | 可以访问 GitHub Release |
| 磁盘 | 至少保留缓存空间。低配要求见 [`../design/18-low-spec-deployment.md`](../design/18-low-spec-deployment.md) |
| 可选 | systemd 或 OpenRC。没有时用 PID 文件方式启动 |

`install.sh` 只支持 Linux。其他平台见 §11。

## 2. 一键安装

```bash
curl -fsSL https://raw.githubusercontent.com/HasonHuang/TV-Buddy/main/install.sh | sudo sh
```

脚本执行以下步骤：

1. 读取 CPU 架构。选择 `amd64` 或 `arm64` 安装包。
2. 从最新 Release 下载安装包。
3. 校验同名 `.sha256` 文件。没有该文件时跳过校验。
4. 解包并安装到 `/opt/tv-buddy`。
5. 创建命令软链 `/usr/local/bin/tv-buddy`。
6. 注册 systemd 或 OpenRC 服务。
7. 启动服务。

已有数据库不会被覆盖。

### 2.1 交互问题

安装前脚本询问两件事：

| 问题 | 回答 `y` | 回答 `n` 或回车 |
| --- | --- | --- |
| 是否使用低配参数 | 写入省内存、省 CPU 的参数 | 使用常规参数 |
| 是否安装 NGINX | 安装 NGINX 并写入反代配置 | 不安装 NGINX |

脚本会显示检测到的内存与 CPU 核数。没有交互终端时，两项都默认 `n`。

### 2.2 非交互安装

在命令后加参数：

```bash
curl -fsSL https://raw.githubusercontent.com/HasonHuang/TV-Buddy/main/install.sh \
  | sudo sh -s -- --low-spec --no-nginx
```

`--nginx` 与 `--low-spec` 都会跳过对应询问。

## 3. 手动安装

### 3.1 使用发布包

从 Release 下载对应架构的包：

```bash
tar -xzf tv-buddy-<版本>-linux-<arch>.tar.gz
cd tv-buddy-<版本>-linux-<arch>
sudo ./install.sh
```

发布包包含二进制、`install.sh`、管理脚本与服务单元。

### 3.2 使用归档或 URL

```bash
sudo ./install.sh --archive /path/to/tv-buddy-<版本>-linux-<arch>.tar.gz
sudo ./install.sh --archive https://example.com/tv-buddy-<版本>-linux-<arch>.tar.gz
```

### 3.3 使用本地构建产物

在仓库根目录执行：

```bash
scripts/build-release.sh
sudo ./install.sh --local
```

`--local` 使用 `dist/` 中最新的同架构安装包。它不从 Release 下载。

## 4. 安装选项

| 选项 | 作用 |
| --- | --- |
| `--prefix <目录>` | 安装目录。默认 `/opt/tv-buddy`。必须是绝对路径，不能含空白 |
| `--archive <路径或 URL>` | 指定 tar.gz 安装包 |
| `--version <tag>` | 从 Release 安装指定版本。默认最新 |
| `--repo <owner/name>` | Release 所在仓库。默认内置 |
| `--local` | 使用本地 `dist/` 中的安装包 |
| `--no-service` | 不注册 systemd/OpenRC 服务 |
| `--no-link` | 不创建 `/usr/local/bin/tv-buddy` 软链 |
| `--no-start` | 安装后不启动服务 |
| `--low-spec` | 写入低配参数。不询问 |
| `--no-low-spec` | 不写入低配参数。不询问 |
| `--nginx` | 安装 NGINX 并写入反代配置。不询问 |
| `--no-nginx` | 不安装 NGINX。不询问 |
| `-h`、`--help` | 显示帮助 |

环境变量与选项等价：

| 环境变量 | 等价选项 |
| --- | --- |
| `PREFIX` | `--prefix` |
| `ARCHIVE` | `--archive` |
| `TV_BUDDY_VERSION` | `--version` |
| `TV_BUDDY_REPO` | `--repo` |
| `CLI_LINK` | 命令软链位置。默认 `/usr/local/bin/tv-buddy` |

使用 `--no-service` 且不使用 NGINX 时，可以不使用 root。此时必须指定可写目录：

```bash
./install.sh --prefix "$HOME/tv-buddy" --no-service
```

## 5. 安装结果

默认安装目录结构：

```text
/opt/tv-buddy/
  tv-buddy                            # 服务二进制
  tv-buddy.db                         # 配置、用户、令牌。首次启动生成
  tv-buddy.log                        # 运行日志
  cache/                              # 缓存与初始口令
    admin_initial_credentials.txt     # 首次启动生成，权限 600
  tv-buddy.sh                         # 管理脚本
/usr/local/bin/tv-buddy               # 软链到 tv-buddy.sh
```

服务单元：

| 系统 | 文件 | 服务名 |
| --- | --- | --- |
| systemd | `/etc/systemd/system/tv-buddy.service` | `tv-buddy` |
| OpenRC | `/etc/init.d/tv-buddy` | `tv-buddy` |

服务单元设置工作目录为安装目录。数据库路径作为第一个参数传入。

`cache.disk_path` 默认是 `./cache`。这是相对路径。手工运行二进制时，先进入安装目录。否则缓存会落到当前目录。

## 6. 首次启动与登录

安装默认会启动服务。使用 `--no-start` 时，手动启动：

```bash
sudo tv-buddy start
```

没有注册服务时：

```bash
sudo /opt/tv-buddy/tv-buddy.sh start
```

查看面板地址与初始口令位置：

```bash
sudo tv-buddy url
```

输出包含三行：

```text
面板地址：http://<主机>:9900/page/panel
账号：admin
初始口令：<安装目录>/cache/admin_initial_credentials.txt
```

初始口令文件：

| 项 | 说明 |
| --- | --- |
| 路径 | `<安装目录>/cache/admin_initial_credentials.txt`。默认 `/opt/tv-buddy/cache/admin_initial_credentials.txt` |
| 生成时机 | 首次启动。数据库没有用户时生成 |
| 权限 | `600` |
| 内容 | 账号与随机口令。文件只生成一次 |

读取口令：

```bash
sudo cat /opt/tv-buddy/cache/admin_initial_credentials.txt
```

打开面板 `http://<主机>:9900/page/panel`。使用账号 `admin` 与初始口令登录。

`http://<主机>:9900/` 默认跳转到 `/page/panel`。未登录时，页面显示登录表单。没有独立登录页。

**立即修改口令：**

```bash
sudo tv-buddy pass '<新口令>' admin
```

口令至少 4 位。修改后旧登录会话立即失效。

初始口令文件丢失时，用上面的命令重设口令。文件不会再生成。

## 7. 低配安装

加 `--low-spec` 写入 5 个键：

| 键 | 常规默认 | 低配值 | 作用 |
| --- | --- | --- | --- |
| `cache.segments.per_channel_max` | `0`（内置 4096） | `200` | 降低每频道切片索引条数 |
| `cache.prefetch.concurrency` | `4` | `2` | 降低预取并发与 CPU 峰值 |
| `auth.token_enabled` | `true` | `true` | 开启播放凭证 |
| `auth.ip_guard_enabled` | `true` | `true` | 开启 IP 限流、会话上限与封禁 |
| `cdn.enabled` | `false` | `true` | 开启 CDN 兜底响应头与错误状态 |

只有前 2 项减少资源消耗。后 3 项改变安全或分发行为。

脚本直接写数据库。已有数据库也会被改写这 5 个键。其他键保留。

`--no-low-spec` 跳过低配写入。没有交互终端时默认不写入。

低配参数、切换步骤与 CDN 配置见 [`../design/18-low-spec-deployment.md`](../design/18-low-spec-deployment.md)。

## 8. 端口与监听地址

默认监听 `[::]:9900`。该地址同时接受 IPv4 与 IPv6。

修改端口：

```bash
sudo tv-buddy port 8080
```

命令保留原监听主机，只改端口，然后重启服务。

也可以在面板修改：

1. 打开面板「系统设置」页，找到「服务器」卡片。
2. 修改「监听地址」。
3. 保存。
4. 执行 `sudo tv-buddy restart`。该字段标记为「需重启」。

常用监听地址：

| 地址 | 含义 |
| --- | --- |
| `[::]:9900` | 同时监听 IPv4 与 IPv6 |
| `0.0.0.0:9900` | 只监听 IPv4 |
| `127.0.0.1:9900` | 只允许本机访问。适合放在 NGINX 后面 |

Docker 部署不要改为 `127.0.0.1:9900`。容器内的回环地址不接收宿主机的端口映射。

## 9. 验证

```bash
sudo tv-buddy status
curl -fsS http://127.0.0.1:9900/api/health
```

期望结果：

| 检查 | 期望 |
| --- | --- |
| `tv-buddy status` | 服务为运行状态 |
| `/api/health` | 返回 `ok` |
| 面板 | 可以打开并登录 |
| `ss -lntp \| grep 9900` | 端口在监听 |

## 10. 升级与卸载

升级二进制。保留数据库：

```bash
sudo tv-buddy update
```

卸载服务。脚本询问是否删除数据：

```bash
sudo tv-buddy uninstall
```

默认保留数据。详细步骤与风险见 [`05-operations.md`](05-operations.md)。

## 11. macOS

macOS 发布包只有二进制与运行说明。它没有 `install.sh`，也不注册系统服务。

```bash
tar -xzf tv-buddy-<版本>-macos-arm64.tar.gz
cd tv-buddy-<版本>-macos-arm64
xattr -d com.apple.quarantine ./tv-buddy
./tv-buddy ./tv-buddy.db
```

限制：

| 项 | 说明 |
| --- | --- |
| 签名 | 包未签名。首次运行会被 Gatekeeper 拦截 |
| 开机自启 | 不注册服务。需要时自行配置 launchd |
| EPG 脚本 | 默认调用 `python3`。请安装 Python 3，或修改脚本命令 |
| 容器 | Docker 镜像是 Linux。macOS 上运行 `linux/arm64` 镜像 |

## 12. 故障排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `注册系统服务需要 root` | 没有使用 sudo | 加 `sudo`，或加 `--no-service` |
| `只支持 Linux` | 在 macOS 或其他系统运行 `install.sh` | 使用 §11 手动运行二进制 |
| 端口被占用 | 其他进程使用 9900 | `ss -lntp \| grep 9900`，再用 `tv-buddy port` 改端口 |
| 服务启动失败 | 端口、权限、监听地址或数据库问题 | `sudo tv-buddy log 200`，检查日志与目录权限 |
| 面板无法访问 | 防火墙、监听地址、服务未启动 | 检查 `tv-buddy status` 与防火墙规则 |
| 初始口令文件不存在 | 文件只在首次启动、数据库无用户时生成 | 用 `sudo tv-buddy pass '<新口令>' admin` 重设 |
| 安装后数据库是旧的 | 安装脚本保留已有数据库 | 这是预期行为。需要重置时用 `sudo tv-buddy reinstall` |
| `--nginx` 配置失败 | `nginx -t` 失败，或已有默认站点冲突 | 脚本会恢复原配置。手动配置见 [`03-nginx.md`](03-nginx.md) |
| 更新失败 | 下载、校验或启动失败 | 启动失败时脚本会恢复旧二进制。查看 `sudo tv-buddy log` |
