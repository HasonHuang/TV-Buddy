# 02 · Docker 部署

> 状态：操作指南 ｜ 最后更新：2026-11-22
> 适用：已有 Docker Engine 与 Compose v2 的 Linux 主机。
> 相关：[`README.md`](README.md)、[`03-nginx.md`](03-nginx.md)、[`05-operations.md`](05-operations.md)

## 1. 前提

| 项 | 要求 |
| --- | --- |
| Docker | Docker Engine 与 Compose v2 |
| 架构 | `linux/amd64` 或 `linux/arm64` |
| 端口 | 宿主机端口映射到容器 `9900` |
| 数据 | 命名卷或绑定挂载到 `/data` |
| 网络 | 拉取镜像需要外网 |

镜像内是静态 musl 二进制。镜像不含 Node 与 Rust 运行时。

## 2. Compose 部署

仓库根目录提供 `docker-compose.yml`。它使用已发布镜像，不在本地构建。

```bash
docker compose up -d
docker compose logs -f
```

| 项 | 值 |
| --- | --- |
| 服务名 | `tv-buddy` |
| 容器名 | `tv-buddy` |
| 镜像 | `docker-compose.yml` 中的已发布镜像。生产环境请改为固定 tag |
| 端口 | `${TV_BUDDY_PORT:-9900}` 映射到容器 `9900` |
| 数据卷 | 命名卷 `tv-buddy-data` 挂到 `/data` |
| 初始化进程 | `init: true`。回收 ffmpeg、python3 等子进程 |
| 日志 | `json-file`，单文件 10 MB，最多 3 个 |

读取初始口令：

```bash
docker compose exec tv-buddy cat /data/cache/admin_initial_credentials.txt
```

打开面板：

```text
http://<主机>:9900/page/panel
```

账号是 `admin`。口令来自初始口令文件。首次登录后立即修改口令。

旧地址 `/panel` 与 `/health` 在本次主版本内仍可用。新部署请使用新地址。

## 3. 配置

Compose 使用的环境变量：

| 变量 | 作用 |
| --- | --- |
| `TV_BUDDY_INITIAL_PASSWORD` | 首次启动的 admin 口令。只在用户表为空时生效。留空时随机生成 |
| `TV_BUDDY_PORT` | 宿主机端口。默认 `9900` |
| `RUST_LOG` | 日志级别。例如 `tv_buddy=debug` |

首次启动时指定口令：

```bash
TV_BUDDY_INITIAL_PASSWORD='<口令>' docker compose up -d
```

已有数据卷不会改口令。要改已有口令，使用面板或 §3.2。

### 3.1 改端口

```bash
TV_BUDDY_PORT=8080 docker compose up -d
```

Compose 会重建容器。数据卷保留。

只允许本机访问时，编辑 `docker-compose.yml`：

```yaml
ports:
  - "127.0.0.1:9900:9900"
```

然后执行 `docker compose up -d`。也可以设置 `TV_BUDDY_PORT=127.0.0.1:9900`。

### 3.2 改口令

在面板打开「用户与设备」，在 `admin` 行点「重置密码」。

无法打开面板时，在容器内改库：

```bash
printf '%s\n' '<新口令>' | docker compose exec -T tv-buddy \
  /opt/tv-buddy/tv-buddy passwd admin --db /data/tv-buddy.db --stdin
docker compose restart
```

### 3.3 数据位置

| 项 | 容器路径 |
| --- | --- |
| 数据库 | `/data/tv-buddy.db` |
| 日志 | `/data/tv-buddy.log` |
| 缓存 | `/data/cache` |
| 初始口令 | `/data/cache/admin_initial_credentials.txt` |

数据库保存配置、用户与令牌。备份时必须包含数据库。

### 3.4 绑定挂载权限

容器用户是 `tvbuddy`，UID/GID 为 `10001`。使用绑定挂载时，先设置目录属主：

```bash
sudo mkdir -p /srv/tv-buddy
sudo chown -R 10001:10001 /srv/tv-buddy
```

然后挂载：

```yaml
volumes:
  - /srv/tv-buddy:/data
```

目录属主错误时，容器会启动失败或无法写数据库。

## 4. 镜像构建

需要自行构建时：

```bash
docker build -t tv-buddy .
docker build --build-arg TV_BUDDY_VERSION=<版本> -t tv-buddy:<版本> .
```

需要镜像内置 ffmpeg 与 ffprobe 时：

```bash
docker build --build-arg WITH_FFMPEG=1 -t tv-buddy:ffmpeg .
```

构建使用多阶段镜像。构建阶段需要 Node 与 Rust 工具链。运行阶段只包含二进制与基础系统包。

## 5. ffmpeg 与 Python

运行镜像默认不包含 ffmpeg 与 Python。

| 能力 | 方式 |
| --- | --- |
| ffmpeg / ffprobe | 面板「系统状态」或「视频素材」页安装。文件写入 `/data/tools`，随数据卷保留 |
| 构建时内置 ffmpeg | 加 `--build-arg WITH_FFMPEG=1` |
| Python 3 | 镜像不含。EPG 脚本源需要时，自建镜像并安装 python3。见 [`../design/12-python-scripts.md`](../design/12-python-scripts.md) |

面板安装的 ffmpeg 与 ffprobe 优先于系统 `PATH`。删除后回退系统版本。

## 6. 反向代理

### 6.1 NGINX 在宿主机

只发布到回环地址：

```yaml
ports:
  - "127.0.0.1:9900:9900"
```

NGINX upstream 写 `127.0.0.1:9900`。完整配置见 [`03-nginx.md`](03-nginx.md)。

### 6.2 NGINX 在同一 Compose 网络

不要发布 9900。upstream 写服务名：

```nginx
upstream tvbuddy {
    server tv-buddy:9900;
    keepalive 32;
}
```

两个服务必须在同一 Docker 网络。Compose 默认创建的网络满足该条件。

### 6.3 不要改容器内监听地址

容器内保持 `[::]:9900`。改为 `127.0.0.1:9900` 后，宿主机端口映射无法访问容器。

## 7. 升级

```bash
docker compose pull
docker compose up -d --force-recreate
```

数据卷保留。升级前先备份。

生产环境固定镜像 tag。不要长期使用 `latest`。

预发布版本通常不更新 `latest` tag。请把 `image` 改为具体版本号。

## 8. 停止与删除

停止容器。保留数据卷：

```bash
docker compose stop
```

删除容器与网络。保留数据卷：

```bash
docker compose down
```

删除容器、网络与数据卷：

```bash
docker compose down -v
```

`-v` 会删除数据库、缓存、台标与 EPG。只在确认要清空数据时使用。

## 9. 使用 docker run

不使用 Compose 时：

```bash
docker run -d --name tv-buddy --init \
  -p 9900:9900 \
  -v tv-buddy-data:/data \
  -e TV_BUDDY_INITIAL_PASSWORD='<口令>' \
  <镜像名>:<版本>
```

| 选项 | 作用 |
| --- | --- |
| `--init` | 回收子进程。ffmpeg 与 python3 需要 |
| `-p 9900:9900` | 映射端口。放在 NGINX 后面时改为 `-p 127.0.0.1:9900:9900` |
| `-v tv-buddy-data:/data` | 持久化数据 |
| `-e TV_BUDDY_INITIAL_PASSWORD=...` | 首次启动口令。只在用户表为空时生效 |

`docker run` 不会自动更新。升级时拉取新镜像，删除旧容器，再用同一数据卷启动。

读取初始口令：

```bash
docker exec tv-buddy cat /data/cache/admin_initial_credentials.txt
```

## 10. 验证

```bash
docker compose ps
curl -fsS http://127.0.0.1:9900/api/health
```

期望结果：

| 检查 | 期望 |
| --- | --- |
| `docker compose ps` | 服务为 `running`，健康状态为 `healthy` |
| `/api/health` | 返回 `ok` |
| 面板 | 可以打开并登录 |
| 数据卷 | `/data/tv-buddy.db` 存在 |

查看日志：

```bash
docker compose logs -f --tail=200
```

## 11. 故障排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 容器启动后退出 | 数据库权限、端口或配置错误 | `docker compose logs --tail=200` |
| 绑定挂载无法写入 | 目录属主不是 `10001:10001` | `sudo chown -R 10001:10001 <目录>` |
| 找不到初始口令文件 | 首次启动未完成，或数据卷错误 | 检查日志与 `/data/cache/`。文件只在首次启动生成 |
| 面板无法访问 | 端口未映射或防火墙 | `docker compose ps`、`docker port tv-buddy`、防火墙规则 |
| NGINX 返回 502 | upstream 地址错误，或不在同一网络 | 宿主机用 `127.0.0.1:9900`；同网络用 `tv-buddy:9900` |
| ffmpeg 不存在 | 镜像默认不含 | 面板安装，或用 `WITH_FFMPEG=1` 重新构建 |
| Python 脚本失败 | 镜像不含 python3 | 自建镜像并安装 python3 |
| 磁盘占满 | 缓存上限过高，或 Docker 日志过大 | 调整缓存上限，检查 `docker system df` |
| 预发布版本拉不到 | 预发布版本不更新 `latest` | 使用具体版本 tag |
