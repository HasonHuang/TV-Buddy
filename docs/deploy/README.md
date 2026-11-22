# 部署文档（docs/deploy）

> 状态：操作指南 ｜ 最后更新：2026-11-22

本目录写 TV Buddy 的部署与运维步骤。设计依据在 [`../design/`](../design/)。本目录不重复设计论证。

## 1. 选择部署方式

| 场景 | 方式 | 文档 |
| --- | --- | --- |
| Linux VPS。使用 systemd 或 OpenRC。 | 二进制安装 | [`01-install-linux.md`](01-install-linux.md) |
| 已有 Docker 与 Compose。 | Docker Compose | [`02-docker.md`](02-docker.md) |
| 需要域名、HTTPS 或统一入口。 | 加 NGINX | [`03-nginx.md`](03-nginx.md) |
| 源站带宽小。观众分散。 | 加 CDN | [`04-cdn.md`](04-cdn.md) |
| 日常启停、升级、备份、排障。 | 运维 | [`05-operations.md`](05-operations.md) |

通常只选一种安装方式。若同时运行两种方式，必须使用不同端口与不同数据目录。

## 2. 最短部署路径

1. 安装服务。见 01 或 02。
2. 读取初始口令。
3. 打开面板 `http://<主机>:9900/page/panel`。
4. 用账号 `admin` 与初始口令登录。
5. 立即修改口令。
6. 在面板添加分组与频道。
7. 按需配置 NGINX 与 CDN。

一键安装命令：

```bash
curl -fsSL https://raw.githubusercontent.com/HasonHuang/TV-Buddy/main/install.sh | sudo sh
```

## 3. 通用前提

| 项 | 要求 |
| --- | --- |
| 操作系统 | Linux amd64/arm64。macOS arm64 只能手动运行 |
| 权限 | 注册系统服务与安装 NGINX 需要 root 或 sudo |
| 默认端口 | TCP `9900` |
| 外网 | 安装与更新需要访问 GitHub Release |
| 磁盘 | 切片缓存需要磁盘空间。低配场景见 [`../design/18-low-spec-deployment.md`](../design/18-low-spec-deployment.md) |
| 可选组件 | `ffmpeg`、`ffprobe`、Python 3、NGINX、CDN |

## 4. 默认路径与入口

默认安装目录是 `/opt/tv-buddy`。自定义目录时，替换下表的路径。

| 项 | 默认值 |
| --- | --- |
| 安装目录 | `/opt/tv-buddy` |
| 数据库 | `/opt/tv-buddy/tv-buddy.db` |
| 日志 | `/opt/tv-buddy/tv-buddy.log` |
| 缓存 | `/opt/tv-buddy/cache` |
| 初始口令 | `<安装目录>/cache/admin_initial_credentials.txt`。默认 `/opt/tv-buddy/cache/admin_initial_credentials.txt` |
| 管理命令 | `/usr/local/bin/tv-buddy` |
| 面板 | `http://<主机>:9900/page/panel` |
| 健康检查 | `http://<主机>:9900/api/health` |
| 用户订阅 | `http://<主机>:9900/api/sub/<令牌>/playlist.m3u` |
| Docker 数据目录 | `/data` |

数据库保存配置、用户与令牌。备份时必须包含数据库。

旧地址 `/panel`、`/health` 与 `/sub/...` 在本次主版本内仍可用。新部署请使用上表地址。

## 5. 占位符约定

本文使用以下占位符。执行命令时，替换为实际值。

| 占位符 | 含义 |
| --- | --- |
| `<主机>` | 服务器 IP 或域名 |
| `<域名>` | 对外访问域名 |
| `<令牌>` | 用户订阅令牌或播放令牌 |
| `<版本>` | 发布版本，例如 `v2026.3.0` |
| `<gid>` | 分组 ID |
| `<cid>` | 频道 ID |
| `<路径>` | 实际文件路径 |

命令默认使用默认安装目录与 root 权限。自定义目录时，替换路径。

## 6. 相关文档

| 文档 | 内容 |
| --- | --- |
| [`../design/18-low-spec-deployment.md`](../design/18-low-spec-deployment.md) | 低配部署：参数、CDN 控制台、验收与排障 |
| [`../design/06-low-spec-optimization.md`](../design/06-low-spec-optimization.md) | CDN 缓存、推荐 TTL、磁盘回收的设计依据 |
| [`../design/04-config-and-storage.md`](../design/04-config-and-storage.md) | 配置键与存储细节 |
| [`../tests/06-cdn-console.md`](../tests/06-cdn-console.md) | CDN 控制台完整验收流程 |
| [`../design/03-http-contract.md`](../design/03-http-contract.md) | 路由表与接口契约 |
