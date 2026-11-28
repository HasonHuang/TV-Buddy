# 05 · 运维

> 状态：操作指南 ｜ 最后更新：2026-11-22
> 适用：已安装的 TV Buddy。
> 相关：[`01-install-linux.md`](01-install-linux.md)、[`02-docker.md`](02-docker.md)、[`03-nginx.md`](03-nginx.md)、[`06-backup.md`](06-backup.md)

## 1. 服务管理

安装后使用 `tv-buddy` 命令。默认命令软链是 `/usr/local/bin/tv-buddy`。

```bash
sudo tv-buddy status
sudo tv-buddy start
sudo tv-buddy stop
sudo tv-buddy restart
```

| 命令 | 作用 |
| --- | --- |
| `tv-buddy` / `menu` | 打开交互菜单 |
| `start` | 启动服务 |
| `stop` | 停止服务 |
| `restart` | 重启服务 |
| `status` | 查看状态 |
| `log [行数]` | 查看日志。终端下持续跟随 |
| `url` | 打印面板地址与账号 |
| `port <N>` | 修改端口并重启 |
| `pass` / `passwd [口令] [账号]` | 修改口令。无参数进入交互 |
| `version` | 显示版本 |
| `update` | 更新二进制与管理脚本。保留数据库 |
| `reinstall` | 备份数据库、重置配置后重装；默认保留原口令 |
| `install [参数]` | 调用安装脚本 |
| `uninstall` | 卸载服务。询问是否删除数据 |

`reload` 是 `restart` 的兼容别名，会重启服务，不重读配置。重读配置用面板「重读配置」。

没有 systemd 或 OpenRC 时，命令使用 PID 文件管理进程。

直接使用服务管理器：

```bash
# systemd
sudo systemctl status tv-buddy
sudo systemctl restart tv-buddy

# OpenRC
sudo rc-service tv-buddy status
sudo rc-service tv-buddy restart
```

## 2. 日志

```bash
sudo tv-buddy log 200
```

在终端中，命令持续跟随日志。非终端环境只打印指定行数。

日志文件默认位置：

```text
/opt/tv-buddy/tv-buddy.log
```

systemd 环境也可以使用：

```bash
sudo journalctl -u tv-buddy -n 200 --no-pager
```

日志级别在面板或配置中设置。常用值：

| 值 | 用途 |
| --- | --- |
| `info` | 默认 |
| `debug` | 排障 |
| `warn` | 只记录警告与错误 |

长期使用 `debug` 会增加日志量与磁盘占用。

## 3. 健康检查

```bash
curl -fsS http://127.0.0.1:9900/api/health
```

期望返回 `ok`。

修改过端口时，替换 `9900`。监听地址是 `127.0.0.1` 时，只能在主机内检查。

Docker 环境：

```bash
docker compose ps
docker compose exec tv-buddy wget -qO- http://127.0.0.1:9900/api/health
```

## 4. 修改端口与监听地址

### 4.1 命令行

```bash
sudo tv-buddy port 8080
```

命令保留原监听主机，只改端口，然后重启服务。

### 4.2 面板

1. 打开面板「系统设置」页，找到「服务器」卡片。
2. 修改「监听地址」。
3. 保存。
4. 执行 `sudo tv-buddy restart`。

该字段标记为「需重启」。保存后不会立即生效。

放在 NGINX 后面时，使用 `127.0.0.1:9900`。Docker 部署不要使用该值。

### 4.3 Docker

修改 `docker-compose.yml` 的端口，或设置 `TV_BUDDY_PORT`：

```bash
TV_BUDDY_PORT=8080 docker compose up -d
```

### 4.4 直接改配置

`config` 是服务二进制的子命令，不是管理脚本的命令。用完整路径：

```bash
sudo /opt/tv-buddy/tv-buddy config get server.bind --db /opt/tv-buddy/tv-buddy.db
sudo /opt/tv-buddy/tv-buddy config set server.log_level debug --db /opt/tv-buddy/tv-buddy.db
```

- `config get` 默认隐藏密钥。`--reveal` 显示原文。`config get .` 打印整份配置。
- `config set` 写入未知键时告警；`--allow-extra` 关闭告警。
- 写入用 `revision` 条件保存。并发冲突时 `set` 重试；路径含数组下标时不重试。`import` / `reset` 冲突时报错。
- `import --force` 与 `reset --force` 覆盖前把当前配置写到 `<库>.bak.json`（权限 0600）。
- 写入后服务不会自动读取。请在面板点「重读配置」，或执行 `sudo tv-buddy restart`。

`--db` 省略时使用当前目录下的 `tv-buddy.db`。不在安装目录执行时必须显式指定。

## 5. 修改口令

```bash
sudo tv-buddy pass '<新口令>' admin
```

口令至少 4 位。修改后旧登录会话立即失效。

无参数时进入交互模式：

```bash
sudo tv-buddy pass
```

也可以在面板打开「用户与设备」，在 `admin` 行点「重置密码」。

初始口令文件只在首次启动生成。文件丢失时，用 `pass` 命令重设。账号不存在时，`pass` 会失败。先在面板创建账号。

Docker 环境：

```bash
printf '%s\n' '<新口令>' | docker compose exec -T tv-buddy \
  /opt/tv-buddy/tv-buddy passwd admin --db /data/tv-buddy.db --stdin
docker compose restart   # 让现有会话失效。本版起，新口令在下次登录时生效。
```

`--db` 必须是数据库文件，不要传 `/data` 目录。口令必须经标准输入传入：用 `-T` 加 `printf` 管道。`docker exec` 不带 `-i` 时标准输入为空，命令会报「密码太短」。
容器内没有管理脚本。不要执行 `./tv-buddy update`；升级用 `docker compose pull`。

## 6. 备份

备份与恢复的完整步骤见 [`06-backup.md`](06-backup.md)。

## 7. 升级

### 7.1 管理脚本更新（推荐）

```bash
sudo tv-buddy update
```

默认从 GitHub Release 获取最新正式版。

指定版本：

```bash
sudo TV_BUDDY_VERSION=v2026.3.0 tv-buddy update
```

预发布版本的 tag 含 `-`。`latest` 通常不指向预发布版本。必须指定 tag。

其他更新源按以下优先级读取：

| 顺序 | 环境变量 | 内容 |
| --- | --- | --- |
| 1 | `TV_BUDDY_BIN` | 本地二进制 |
| 2 | `TV_BUDDY_ARCHIVE` | 本地 tar.gz |
| 3 | `TV_BUDDY_URL` | 远程 tar.gz 或二进制。支持 `{arch}` 占位符 |
| 4 | 本地 `dist/` | 安装目录或上级目录中的安装包 |
| 5 | GitHub Release | 默认 |

更新流程：

1. 备份旧二进制。
2. 停止服务。
3. 替换二进制。
4. 启动服务。
5. 检查 `/api/health`。

新二进制启动失败时，脚本恢复旧二进制。启动成功但健康检查失败时，脚本只告警，不自动回滚。此时查看日志并手动处理。

归档更新（`TV_BUDDY_ARCHIVE`、`TV_BUDDY_URL`、本地 `dist/`、GitHub Release）会同时刷新管理脚本。服务单元在 `/etc` 下，不在更新范围；发布包变更了服务单元时，重跑包内 `install.sh`。`TV_BUDDY_BIN` 只更新二进制。

### 7.1.1 直接更新二进制

不经过管理脚本时，二进制自带 `update`：

```bash
sudo /opt/tv-buddy/tv-buddy update --check
sudo /opt/tv-buddy/tv-buddy update --version v2026.10 --restart
```

参数：

| 参数 | 作用 |
| --- | --- |
| `--check` | 只查询最新版本，不下载、不替换 |
| `--version <tag>` | 指定 Release tag |
| `--repo <owner/name>` | 指定仓库 |
| `--archive <路径或 URL>` | 使用指定安装包 |
| `--restart` | 替换后尝试重启 systemd/OpenRC 服务 |
| `--force` | 同版本也替换 |

行为：

- 只替换当前二进制，不更新管理脚本与服务单元。
- 旧二进制备份为 `<二进制>.bak`。
- 下载 GitHub Release 时校验同名 `.sha256`；没有 `.sha256` 时告警并跳过。
- 容器内替换只对当前容器有效，且通常没有写 `/opt/tv-buddy` 的权限。生产升级用 `docker compose pull`。

### 7.2 Docker

```bash
docker compose pull
docker compose up -d --force-recreate
```

数据卷保留。生产环境固定镜像 tag。

### 7.3 升级后检查

```bash
sudo tv-buddy status
curl -fsS http://127.0.0.1:9900/api/health
sudo tv-buddy log 100
```

再打开面板，检查版本、分组、用户与缓存状态。

## 8. 重装

重装会重置配置。用户、令牌与现有口令保留。

```bash
sudo tv-buddy reinstall
```

执行动作：

1. 备份数据库为 `tv-buddy.db.bak`。
2. 停止服务。
3. 重置配置。
4. 询问端口与新口令。口令回车表示保留现有口令。
5. 启动服务。

非交互重装：

```bash
TV_BUDDY_YES=1 TV_BUDDY_PORT=9900 \
  sudo -E tv-buddy reinstall
```

不提供 `TV_BUDDY_PASS` 时不改口令。要把口令改成指定值：

```bash
TV_BUDDY_YES=1 TV_BUDDY_PORT=9900 TV_BUDDY_PASS='<口令>' \
  sudo -E tv-buddy reinstall
```

不会把口令重置为 `admin`。

重装用于配置损坏或需要恢复默认值。正常升级使用 `update`。

## 9. 卸载

```bash
sudo tv-buddy uninstall
```

脚本执行以下动作：

1. 停止服务。
2. 删除 systemd 或 OpenRC 服务单元。
3. 删除命令软链。
4. 询问是否删除安装目录。

默认保留安装目录。数据库、缓存、台标与 EPG 都保留。重新安装可以恢复。

**危险：** `TV_BUDDY_YES=1` 会自动删除数据，不再询问。

```bash
TV_BUDDY_YES=1 sudo -E tv-buddy uninstall
```

只在确认要清空数据时使用。

Docker 环境：

```bash
docker compose down      # 保留数据卷
docker compose down -v   # 删除数据卷
```

## 10. Docker 运维

| 任务 | 命令 |
| --- | --- |
| 查看状态 | `docker compose ps` |
| 查看日志 | `docker compose logs -f --tail=200` |
| 重启 | `docker compose restart` |
| 停止 | `docker compose stop` |
| 启动 | `docker compose start` |
| 更新 | `docker compose pull && docker compose up -d --force-recreate` |
| 读取口令 | `docker compose exec tv-buddy cat /data/cache/admin_initial_credentials.txt` |
| 进入容器 | `docker compose exec tv-buddy sh` |

Docker 环境的备份与恢复见 [`06-backup.md`](06-backup.md) §4。

## 11. 安全清单

| # | 项 | 要求 |
| --- | --- | --- |
| 1 | 初始口令 | 首次登录后立即修改 |
| 2 | HTTPS | 公网部署必须使用 HTTPS |
| 3 | 端口 | 不要对公网开放 `9900`。放在 NGINX 后面 |
| 4 | 监听地址 | 非 Docker 部署可设为 `127.0.0.1:9900` |
| 5 | 防火墙 | 只开放 `80` 与 `443` |
| 6 | 播放凭证 | 保持 `auth.token_enabled = true` |
| 7 | IP 管控 | 保持 `auth.ip_guard_enabled = true` |
| 8 | 面板 | 限制访问来源。不要在公网无保护暴露 |
| 9 | 备份 | 定期备份数据库。备份包异地保存 |
| 10 | 主机 | systemd 与 OpenRC 服务以 root 运行。限制主机登录用户 |
| 11 | 更新 | 定期升级，修复已知问题 |
| 12 | CDN | 公网回源使用 HTTPS。只允许 CDN 回源 IP 访问源站 |

## 12. 故障排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 服务无法启动 | 端口占用、权限、监听地址或数据库损坏 | `sudo tv-buddy log 200`。检查端口与目录权限 |
| 面板无法打开 | 服务未启动、防火墙、监听地址 | `tv-buddy status`、`ss -lntp \| grep 9900`、防火墙规则 |
| 登录失败 | 口令错误，或失败次数过多被临时锁定 | 等待锁定结束。用 `tv-buddy pass` 重设口令 |
| 修改配置后不生效 | 配置需要重启，或 `config set` 未触发热载 | 面板保存。或执行 `tv-buddy restart`、`POST /api/panel/reload` |
| 播放 404 | 令牌错误、分组不可见、频道归档或上游失败 | 从面板重新复制订阅地址。检查分组权限与上游 |
| 播放 502 | 上游片源失败，或 NGINX upstream 错误 | 检查日志与 [`03-nginx.md`](03-nginx.md) |
| 磁盘占满 | 缓存上限过高、日志过大、备份过多 | 调整缓存上限。清理日志与旧备份 |
| 内存占用高 | 预取并发高，或频道数多 | 使用低配参数。降低 `cache.prefetch.concurrency` |
| 更新失败 | 下载、校验或启动失败 | 启动失败时脚本会恢复旧二进制。查看日志与网络 |
| 初始口令文件不存在 | 文件只在首次启动生成 | `sudo tv-buddy pass '<新口令>' admin` |
| NGINX 502 | 后端未启动或 upstream 地址错误 | 见 [`03-nginx.md`](03-nginx.md) §10 |
| IP 管控误封 | 前置 CDN 未配置真实 IP | 见 [`03-nginx.md`](03-nginx.md) §8 |
| Docker 容器退出 | 数据目录权限、端口或配置错误 | `docker compose logs --tail=200`。检查 `/data` 属主 |
| ffmpeg 或 python3 不存在 | 镜像默认不含 | 面板安装 ffmpeg。Python 需自建镜像 |
| 节目单时间错误 | 时区或系统时间错误 | 设置 `server.timezone_offset_hours`。校准系统时间 |
