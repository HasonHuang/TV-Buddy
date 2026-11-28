# 06 · 备份与恢复

> 状态：操作指南 ｜ 最后更新：2026-11-28
> 适用：二进制安装与 Docker 部署的 TV Buddy。
> 相关：[`01-install-linux.md`](01-install-linux.md)、[`02-docker.md`](02-docker.md)、[`05-operations.md`](05-operations.md)

## 1. 备份方式

| 方式 | 内容 | 适用 |
| --- | --- | --- |
| 面板备份 | 配置、EPG 配置、台标、所选资源 | 修改配置前快速回退 |
| 完整备份 | 数据库、缓存、日志、工具 | 迁移、重装、灾难恢复 |

面板备份不含用户、令牌与播放记录。这些数据在数据库中。完整备份必须包含数据库。

必须在服务停止后备份完整数据。运行中复制 SQLite 文件，可能得到损坏的数据库。

## 2. 面板备份与恢复

面板「系统设置」页的「备份与维护」区域提供「备份与恢复」。

备份：

1. 打开面板「系统设置」页。
2. 在「备份与维护」区域点「备份与恢复」。
3. 下载备份包。

恢复：

1. 打开面板「系统设置」页。
2. 在「备份与维护」区域选择 `.tar.gz` 备份包。
3. 确认恢复。

恢复会覆盖当前配置。恢复前先下载一份当前备份。

## 3. 二进制安装

### 3.1 完整备份

先停止服务：

```bash
sudo tv-buddy stop
```

再备份安装目录：

```bash
sudo tar -C /opt -czf /root/tv-buddy-backup-$(date +%F).tar.gz tv-buddy
```

再启动服务：

```bash
sudo tv-buddy start
```

备份内容：

| 路径 | 内容 |
| --- | --- |
| `tv-buddy.db` | 配置、用户、令牌、分组与频道 |
| `tv-buddy.db-wal` | 未合并的数据库事务。存在时必须一起备份 |
| `tv-buddy.db-shm` | SQLite 共享内存文件。存在时一起备份 |
| `cache/` | 台标、EPG、脚本、解析器、切片缓存 |
| `tv-buddy.log` | 日志。可选 |

不要只复制运行中的数据库文件。写入过程中复制可能得到损坏的数据库。

缓存很大时，先在面板「缓存状态」页清空切片缓存，再备份。配置与用户不受影响。

### 3.2 完整恢复

**危险：** 下面的命令会替换当前安装目录。

先停止服务，再保留旧目录并解包：

```bash
sudo tv-buddy stop
sudo mv /opt/tv-buddy /opt/tv-buddy.old
sudo tar -C /opt -xzf /root/tv-buddy-backup-<日期>.tar.gz
sudo tv-buddy start
```

确认服务正常后，再删除 `/opt/tv-buddy.old`。

## 4. Docker 部署

Docker 部署的数据在 `/data`。默认使用命名卷 `tv-buddy-data`。设置 `TV_BUDDY_DATA` 时，数据在宿主机目录。见 [`02-docker.md`](02-docker.md) §3.4。

### 4.1 命名卷备份

先停止服务。服务运行中复制数据库，会得到不一致的文件。

```bash
docker compose stop
```

启动临时容器，把卷备份到当前目录：

```bash
docker run --rm \
  -v tv-buddy-data:/data \
  -v "$PWD":/backup \
  alpine tar czf /backup/tv-buddy-data-$(date +%F).tar.gz -C /data .
```

再启动服务：

```bash
docker compose start
```

检查备份包：

```bash
tar tzf tv-buddy-data-$(date +%F).tar.gz | head
```

备份包的内容见 §3.1。备份包由 root 创建。需要时改属主：

```bash
sudo chown "$(id -u):$(id -g)" tv-buddy-data-*.tar.gz
```

### 4.2 绑定挂载备份

先停止服务，再直接备份数据目录：

```bash
docker compose stop
sudo tar -C /opt/tv-buddy/data -czf "$PWD/tv-buddy-data-$(date +%F).tar.gz" .
docker compose start
```

### 4.3 命名卷恢复

**危险：** 下面的命令会删除当前数据卷内容。

```bash
docker compose stop
docker run --rm \
  -v tv-buddy-data:/data \
  -v "$PWD":/backup \
  alpine sh -c 'rm -rf /data/* && tar xzf /backup/<备份包>.tar.gz -C /data'
docker compose start
```

只在确认要覆盖数据时使用。

### 4.4 绑定挂载恢复

**危险：** 下面的命令会清空数据目录。

先停止服务，再清空目录并解包：

```bash
docker compose stop
sudo find /opt/tv-buddy/data -mindepth 1 -delete
sudo tar -C /opt/tv-buddy/data -xzf "$PWD/<备份包>.tar.gz"
sudo chown -R 10001:10001 /opt/tv-buddy/data
docker compose start
```

## 5. 检查与存放

恢复后检查：

| 检查 | 方法 |
| --- | --- |
| 服务状态 | `docker compose ps` 或 `systemctl status tv-buddy` |
| 健康检查 | `curl http://127.0.0.1:9900/api/health` |
| 面板登录 | 打开 `/page/panel` |
| 数据存在 | 数据库文件存在，频道列表正常 |

备份包必须异地保存。同一磁盘故障会同时损坏数据与备份。

定期恢复一次测试备份包。不要只验证文件存在。
