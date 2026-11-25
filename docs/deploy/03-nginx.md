# 03 · NGINX 反向代理

> 状态：操作指南 ｜ 最后更新：2026-11-22
> 适用：需要域名、HTTPS 或统一入口的部署。
> 相关：[`01-install-linux.md`](01-install-linux.md)、[`02-docker.md`](02-docker.md)、[`04-cdn.md`](04-cdn.md)

## 1. 作用

NGINX 放在 TV Buddy 前面。它处理域名与 TLS，再把请求转发到本机 `9900`。

```text
观众 → NGINX :80/:443 → TV Buddy 127.0.0.1:9900 → 上游片源
```

使用 NGINX 时，不要让公网直接访问 `9900`。

## 2. 自动配置

安装脚本可以安装 NGINX 并写入配置：

```bash
sudo ./install.sh --nginx
```

脚本执行以下动作：

1. 用系统包管理器安装 NGINX。支持 `apk`、`apt-get`、`dnf`、`yum`。
2. 选择默认站点文件。Alpine 用 `/etc/nginx/http.d/default.conf`，Debian 系用 `/etc/nginx/sites-available/default`，其余用 `/etc/nginx/conf.d/default.conf`。
3. 备份原文件为 `<文件>.bak.<时间>`。
4. 写入随包 NGINX 配置。发布包中是 `nginx.default.conf`，仓库源是 `scripts/nginx/default.conf`。
5. 执行 `nginx -t`。失败时恢复原文件。
6. 启动或重启 NGINX。

使用 `--no-start` 时，脚本只写入配置，不启动 NGINX。

### 2.1 模板中的其他项目

仓库模板 `scripts/nginx/default.conf` 同时代理另一个项目 `sontv`：

```nginx
upstream sontv {
    server 127.0.0.1:9911;
}

location ~ ^/(sub|proxy|play)$ {
    proxy_pass http://sontv;
}
```

只运行 TV Buddy 时，删除 `upstream sontv` 与这个 `location`。否则精确访问 `/sub`、`/proxy`、`/play` 会返回 502。

TV Buddy 的订阅路径是 `/sub/<令牌>/playlist.m3u`。它不匹配上面的精确 `/sub`，所以仍会转到 9900。v2026.60 的 `/api/sub/...` 是别名，也会转到 9900。

### 2.2 模板中的监听地址

模板监听 `80`，并使用 `$host`。标准端口下可以正常工作。

访问非标准端口时，把 `Host $host` 改为 `Host $http_host`。否则生成的订阅与播放地址会丢失端口。

## 3. 手动配置

### 3.1 写入配置文件

按发行版选择路径：

| 系统 | 路径 |
| --- | --- |
| Debian / Ubuntu | `/etc/nginx/sites-available/tv-buddy.conf`，再软链到 `sites-enabled` |
| Alpine | `/etc/nginx/http.d/tv-buddy.conf` |
| RHEL / Fedora | `/etc/nginx/conf.d/tv-buddy.conf` |

配置内容：

```nginx
upstream tvbuddy {
    server 127.0.0.1:9900;
    keepalive 32;
}

server {
    listen      80;
    listen      [::]:80;
    server_name _;

    client_max_body_size 64m;

    location / {
        proxy_pass http://tvbuddy;

        proxy_http_version 1.1;
        proxy_set_header Connection "";

        proxy_set_header Host              $http_host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_connect_timeout 5s;
        proxy_send_timeout    300s;
        proxy_read_timeout    300s;

        proxy_buffering         off;
        proxy_request_buffering off;

        proxy_redirect off;
    }
}
```

主机没有 IPv6 时，删除 `listen [::]:80;`。

已有其他默认站点时，先停用它。否则请求可能落到旧站点。

### 3.2 校验与重载

```bash
sudo nginx -t
sudo systemctl reload nginx
```

OpenRC 系统使用：

```bash
sudo rc-service nginx restart
```

## 4. 关键设置

| 设置 | 值 | 原因 |
| --- | --- | --- |
| `proxy_pass` | `http://tvbuddy`，不带 URI | 原样转发完整路径与查询串 |
| `Host` | `$http_host` | 订阅、EPG 与播放地址由 Host 推导。必须保留端口 |
| `X-Forwarded-Proto` | `$scheme` | 决定生成的地址是 `http` 还是 `https` |
| `X-Forwarded-For` | `$proxy_add_x_forwarded_for` | 后端取最后一项作为客户端地址 |
| `X-Real-IP` | `$remote_addr` | XFF 缺失时的回退地址 |
| `proxy_http_version` | `1.1` | upstream keepalive 需要 |
| `Connection` | 空字符串 | 清空后 upstream keepalive 生效 |
| `proxy_buffering` | `off` | HLS/DASH 切片是流式响应 |
| `proxy_request_buffering` | `off` | 请求体立即转发 |
| `proxy_read_timeout` | `300s` | 订阅解析脚本上限 300 秒 |
| `client_max_body_size` | `64m` | 面板备份恢复的上限 |
| `proxy_redirect` | `off` | 保留播放模式 302 的上游地址 |

注意：

- 本项目没有 WebSocket 或 SSE。不要添加 `Upgrade` 头。
- 不要启用 `proxy_cache`。清单必须不缓存。切片缓存由项目自己管理。
- 不要给切片配置 gzip。默认 `gzip_types` 只含文本，保持默认即可。
- NGINX 默认透传 `Range`。切片请求可以返回 206。不要改写 Range 头。

## 5. HTTPS

把 `server` 改为 TLS 监听。`location` 内容保持不变：

```nginx
server {
    listen      443 ssl;
    listen      [::]:443 ssl;
    http2       on;
    server_name tv.example.com;

    ssl_certificate     /etc/letsencrypt/live/tv.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/tv.example.com/privkey.pem;
    ssl_protocols       TLSv1.2 TLSv1.3;

    client_max_body_size 64m;

    location / {
        proxy_pass http://tvbuddy;

        proxy_http_version 1.1;
        proxy_set_header Connection "";

        proxy_set_header Host              $http_host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        proxy_connect_timeout 5s;
        proxy_send_timeout    300s;
        proxy_read_timeout    300s;

        proxy_buffering         off;
        proxy_request_buffering off;

        proxy_redirect off;
    }
}

server {
    listen      80;
    listen      [::]:80;
    server_name tv.example.com;
    return 301 https://$host$request_uri;
}
```

`http2 on;` 需要 NGINX 1.25.1 或更高版本。旧版本改用：

```nginx
listen 443 ssl http2;
```

`X-Forwarded-Proto $scheme` 必须是 `https`。否则面板生成的订阅地址与清单仍是 `http://`。

使用 certbot 申请证书时，先完成 HTTP-01 验证，再启用 80 跳转。或者保留验证路径：

```nginx
location /.well-known/acme-challenge/ {
    root /var/www/html;
}
```

## 6. Docker 场景

### 6.1 NGINX 在宿主机

把容器端口只发布到回环地址：

```yaml
ports:
  - "127.0.0.1:9900:9900"
```

upstream 使用：

```nginx
upstream tvbuddy {
    server 127.0.0.1:9900;
    keepalive 32;
}
```

### 6.2 NGINX 在同一 Compose 网络

不要发布 9900。upstream 使用服务名：

```nginx
upstream tvbuddy {
    server tv-buddy:9900;
    keepalive 32;
}
```

两个服务必须在同一 Docker 网络。Compose 默认网络满足该条件。

### 6.3 不要改容器内监听地址

容器内保持 `[::]:9900`。改为 `127.0.0.1:9900` 后，宿主机的端口映射无法访问容器。

## 7. 限制源站访问

非 Docker 部署时，在面板修改监听地址：

1. 打开面板「系统设置」页，找到「服务器」卡片。
2. 把「监听地址」改为 `127.0.0.1:9900`。
3. 保存。
4. 执行 `sudo tv-buddy restart`。

Docker 部署使用端口发布限制：

```yaml
ports:
  - "127.0.0.1:9900:9900"
```

防火墙只开放 `80` 与 `443`。不要开放 `9900`。

## 8. 前置 CDN 时取真实 IP

后端取 `X-Forwarded-For` 的最后一项。NGINX 的 `$proxy_add_x_forwarded_for` 会把直连对端追加到最后。

单层 NGINX 时，这个值就是客户端地址。

CDN 回源时，直连对端是 CDN 节点。若不处理，IP 管控会把所有观众当成同一个 IP。默认 `max_sessions_per_ip = 1` 会导致播放会话互相中断。

启用 `real_ip` 模块：

```nginx
# 填 CDN 的回源网段。可写多条。
set_real_ip_from 10.0.0.0/8;
real_ip_header X-Forwarded-For;
real_ip_recursive on;
```

Cloudflare 使用：

```nginx
set_real_ip_from <Cloudflare 网段>;
real_ip_header CF-Connecting-IP;
real_ip_recursive on;
```

设置后，`$remote_addr` 是真实观众。后端取到的最后一项也正确。

多层代理时，每一层都必须追加 `X-Forwarded-For`。不要清空该头。

## 9. 验证

```bash
# 健康检查
curl -I http://127.0.0.1/api/health

# 根路径。默认跳转到 /page/panel
curl -I http://127.0.0.1/

# 面板
curl -I http://127.0.0.1/page/panel

# 订阅清单。确认地址是公网域名与 https
curl -s 'https://tv.example.com/sub/<令牌>/playlist.m3u' | head

# 切片 Range 透传
curl -s -o /dev/null -w '%{http_code}\n' \
  -H 'Range: bytes=0-1023' \
  'https://tv.example.com/media/<gid>/<cid>/<切片路径>'
```

期望结果：

| 检查 | 期望 |
| --- | --- |
| `/api/health` | `ok` |
| `/` | `302`。`Location` 为 `/page/panel` |
| `/page/panel` | 页面可以打开。未登录时显示登录表单 |
| 订阅清单 | 地址是 `https://tv.example.com`，不是 `http://127.0.0.1:9900` |
| 切片 Range | `206` |

旧地址 `/health`、`/panel` 与 `/api/sub/...` 在本次主版本内仍可用。新部署请使用新地址。

## 10. 故障排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 502 Bad Gateway | upstream 地址错误，或后端未启动 | 检查 `tv-buddy status`、`ss -lntp \| grep 9900`、upstream 地址 |
| 502 且后端只监听 IPv6 | 系统开启 `bindv6only` | upstream 改为 `[::1]:9900`，或把监听地址改为 `0.0.0.0:9900` |
| 清单地址是 `http://` | 缺少 `X-Forwarded-Proto` | 加 `proxy_set_header X-Forwarded-Proto $scheme;` |
| 清单地址丢失端口 | 使用 `$host`，不是 `$http_host` | 改为 `Host $http_host;` |
| 上传备份返回 413 | `client_max_body_size` 太小 | 设为 `64m` |
| IP 管控误封 | 没有配置 `real_ip` | 见 §8 |
| 切片 404 | `proxy_pass` 带了 URI，路径被改写 | 使用 `proxy_pass http://tvbuddy;`，不要加 `/` |
| 播放卡顿 | 开启了缓冲或压缩 | `proxy_buffering off;`，不要给媒体配置 gzip |
| `nginx -t` 报默认站点冲突 | 已有其他 `default_server` | 停用旧站点，或给本配置唯一 `server_name` |
| HTTPS 无法申请证书 | 80 已全部跳转到 HTTPS | 先申请证书，或保留 ACME 验证路径 |
