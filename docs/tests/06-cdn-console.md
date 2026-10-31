## 1. 目的

本文档证明 CDN 边缘缓存按设计工作。切片被缓存。清单、订阅清单、面板接口不被缓存。切片字节不被改写。清单下发 `https://` 切片地址。

本文档对应 `PROGRESS.md` §6 的 E1 与 `docs/design/06-low-spec-optimization.md` §2 的 T0。

## 2. 前置条件

### 2.1 机器与服务

| 项 | 要求 |
| --- | --- |
| 操作系统 | Linux。amd64 或 arm64 |
| 服务 | `tv-buddy` 已安装并运行 |
| 默认安装目录 | `/opt/tv-buddy`（`install.sh:17`） |
| 默认数据库路径 | `/opt/tv-buddy/tv-buddy.db`（`packaging/tv-buddy.service`） |
| 默认监听 | `[::]:19899`（`src/config/model.rs:130`） |
| 日志文件 | `/opt/tv-buddy/tv-buddy.log`（`src/logging.rs:163`） |
| 面板 | `[panel].enabled = true`，默认成立（`src/config/model.rs:158-169`） |
| 缓存后端 | `[cache].backend = "disk"`（`src/config/model.rs:251-270`） |

服务必须以数据库路径作为第一个参数启动。默认取 `tv-buddy.db`。systemd 单元已按此写法生成。

检查服务状态：

```bash
systemctl status tv-buddy --no-pager
ss -lntp | grep 19899
tail -n 20 /opt/tv-buddy/tv-buddy.log
```

预期：`active (running)`。19899 在监听。日志无 `监听失败`。

### 2.2 测试分组

选一个分组做全部验证。分组必须满足以下条件：

| 字段 | 取值 |
| --- | --- |
| `mode` | `proxy` |
| `format` | `hls` |
| `access` | `public` 或 `open`。不能是 `archived` |
| `channels` | 至少 1 个 |

`mode` 与 `format` 的取值定义见 `src/config/model.rs:870-915`。`proxy` 模式才产生本地伪装的切片路径（`src/proxy/media.rs:335`）。`redirect`、`rewrite`、`passthrough` 模式把客户端送到上游，CDN 不参与切片缓存。

### 2.3 工具

VPS 上必须有：

| 工具 | 用途 |
| --- | --- |
| `curl` | 发请求 |
| `jq` | 解析 JSON |
| `sha256sum` | 比对字节。没有时用 `shasum -a 256` |
| `grep`、`sed`、`awk` | 文本处理 |

### 2.4 面板口令

默认用户名 `admin`。默认口令 `admin`。默认存储值是 `sha256("admin")`（`src/config/model.rs:158-169`）。如果你改过口令，用实际值。

登录失败有锁定。默认窗口 60 秒，免费失败 15 次，锁 300 秒（`src/config/model.rs:726-756`）。不要反复试错。

### 2.5 CDN 控制台

四个域名已接入同一个控制台。四个域名回源到本服务。回源协议与端口由控制台决定。仓库没有 CDN 供应商名称、控制台 URL、控制台 API（`docs/design/06-low-spec-optimization.md:56-60`）。

控制台字段名未在仓库出现。本文档用设计文档的字段名。你在控制台里找同义项，并把实际字段名记进记录模板。

### 2.6 本文档未验证的项

| 项 | 原因 | 处理 |
| --- | --- | --- |
| CDN 供应商名称、控制台 URL、菜单层级、按钮文案 | 仓库无控制面 | 按实际控制台执行，记录实际名称 |
| 控制台字段名 | 仓库无控制面 | 用设计字段名找同义项 |
| 缓存状态头名与 `HIT`/`MISS`/`BYPASS` 取值 | 本服务不发送该头（`src/http/routes/stream.rs:1388-1393`） | 按供应商文档判定，记录实际头名与值 |
| 通配符 `*` 是否跨多段 | 仓库无定义 | 按第 3.8 节补规则 |
| 缓存键是否含查询串、Cookie、UA、请求头 | 仓库无控制面 | 第 3.7 节取消 Cookie；其余按失败表排查 |
| 控制台 TTL 单位 | 仓库只产出秒（`src/cache_estimate.rs:30-42`） | 见第 3.6 节的换算 |
| 「忽略 Set-Cookie」「缓存带 Cookie 的响应」是否存在 | 取决于 CDN | 三选一，记录实际选项 |
| VPS 上实际的 `[cdn]` 取值 | 仓库没有运行时数据库 | 第 3.2 节读取实际值 |
| CDN 是否把 `Host` 改写为源站地址 | 仓库无控制面 | 第 3.14 节验证；失败时配 `public_base` |
| 控制台实际 TTL | 只能由控制台证明 | 第 3.15 节按过期时间验证 |

## 3. 步骤

### 3.1 取得面板会话

```bash
BASE=http://127.0.0.1:19899
PANEL_USER=admin
PANEL_PASS=admin
ADMIN=$(curl -sS -X POST "$BASE/panel/api/login" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"$PANEL_USER\",\"password\":\"$PANEL_PASS\"}" | jq -r .token)
printf 'token=%s\n' "$ADMIN"
```

预期：打印 36 位 UUID。`token=` 后为空表示登录失败。

会话头同时接受 `Authorization: Bearer <token>` 与 `X-Token: <token>`（`src/http/ext.rs:23-36`）。本文档统一用 Bearer。

### 3.2 读取生效配置

```bash
curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/config" \
  | jq '{cdn, cache_backend: .cache.backend, disk_path: .cache.disk_path, prefetch: .cache.prefetch.enabled, gc: .cache.gc.enabled, token_enabled: .auth.token_enabled}'
```

预期输出含以下键：

```json
{
  "cdn": {
    "enabled": false,
    "auto_tune": true,
    "segment_max_age_secs": 0,
    "shared_cache_hits": false,
    "dont_cache_empty": false,
    "empty_status": 502
  },
  "cache_backend": "disk",
  "disk_path": "./cache",
  "prefetch": true,
  "gc": true,
  "token_enabled": false
}
```

`public_base` 未出现表示未配置。默认值来自 `src/config/model.rs`。低配示例配置把 `cdn.enabled` 设为 `true`（`docs/design/config-low-spec.json`）。以本机实际值为准。

### 3.3 选择测试分组与频道

```bash
curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/config" \
  | jq -r '.groups[]
      | select(.mode=="proxy" and .format=="hls" and .access!="archived" and (.channels|length)>0)
      | [.id, .mode, .format, .access, .channels[0].id] | @tsv'
```

预期：至少一行，五列分别为 `gid`、`mode`、`format`、`access`、`cid`。

选定第一行，写入变量：

```bash
GID=<第一列的 gid>
CID=<第五列的 cid>
EXT=m3u8
```

分组 ID 与频道 ID 会出现在订阅清单的入口 URL 中（`src/playlist.rs:110-117`）。

### 3.4 读取 CDN 推荐值

```bash
curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/cache/stats" | jq .recommend
```

预期输出含 14 个键：

```json
{
  "ttl_secs": 600,
  "disk_cap_mb": 150,
  "source": "fallback",
  "ttl_manual": false,
  "disk_manual": false,
  "groups": 0,
  "updated_secs_ago": null,
  "working_mb": null,
  "disk_total_mb": null,
  "disk_free_mb": null,
  "seg_files": 0,
  "seg_bytes": 0,
  "gc_enabled": true,
  "auto_tune": true
}
```

数值是示例。判定看键与范围：
- `ttl_secs` 是整数。
- `source` 是 `measured`、`manual`、`fallback` 之一。
- `ttl_secs` 在 300–900 之间，或 `source=manual`。

依据：服务端每次 `GET /panel/api/cache/stats` 都重新计算（`src/http/routes/admin.rs:68`、`src/cache_gc.rs:283-333`）。

面板同样显示该卡片。打开面板 → 「缓存状态」。卡片标题「CDN 推荐值」（`frontend/src/lowcfg.tsx:73`）。系统设置页也显示同一卡片（`frontend/src/settings.tsx:779`）。

徽标三值（`frontend/src/lowcfg.tsx:31-35`）：

| 值 | 显示 |
| --- | --- |
| `measured` | 实测 |
| `manual` | 手动配置 |
| `fallback` | 回退值 |

`manual` 不一定是 TTL 手填。磁盘上限手填也会让 `source=manual`（`src/cache_gc.rs:290-298`）。

### 3.5 触发一次估算（`source=fallback` 时）

首次安装没有实测数据。先播放一个频道，或执行估算：

```bash
curl -sS -X POST -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/cdn/estimate" \
  | jq '{ok, groups, failed, recommend: (.recommend | {ttl_secs, source, groups, updated_secs_ago})}'
```

预期：

```json
{
  "ok": true,
  "groups": 3,
  "failed": [],
  "recommend": {
    "ttl_secs": 300,
    "source": "measured",
    "groups": 3,
    "updated_secs_ago": 0
  }
}
```

`groups` 是成功探测的分组数，范围 0–3（`src/http/routes/cdn.rs:22`）。`failed` 是失败明细。全部失败也返回 200，`recommend.source` 回退 `fallback`（`src/http/routes/cdn.rs:27-39`）。

并发估算返回 409。响应文案固定为「已有估算正在进行，请稍后再试」（`src/http/routes/cdn.rs:34-36`）。

估算探测最多 3 个非归档分组，每组第 1 个频道（`src/http/routes/cdn.rs:22`、`:43-57`）。

### 3.6 控制台改五处

打开控制台。选中全部四个域名。控制台支持「应用到全部域名」时，本步只做一次。

| # | 设计字段名 | 设定值 | 依据 |
| --- | --- | --- | --- |
| 1 | 缓存过期时间 / 缓存规则 | 媒体路径填推荐 TTL。TTL 填在缓存规则上，不填在不缓存规则上 | `docs/design/06-low-spec-optimization.md:65` |
| 2 | 是否遵循源站缓存头 | 关闭。改用强制 TTL | `docs/design/06-low-spec-optimization.md:66` |
| 3 | Set-Cookie | 三选一：关闭 Cookie 注入；忽略 Set-Cookie；缓存带 Cookie 的响应。优先第一条 | `docs/design/06-low-spec-optimization.md:67` |
| 4 | 图片优化 / 自动 WebP / 内容压缩改写 | 全部关闭。至少对媒体路径关闭 | `docs/design/06-low-spec-optimization.md:68` |
| 5 | 回源请求头 | 增加 `X-Forwarded-Proto: https` | `docs/design/06-low-spec-optimization.md:69` |

另加两项确认：

| 项 | 设定值 |
| --- | --- |
| 不缓存 3xx | 开启 |
| 不缓存非 200 | 开启 |

依据：`docs/design/06-low-spec-optimization.md:71-72`。

TTL 取值：

```bash
curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/cache/stats" | jq -r .recommend.ttl_secs
```

预期：打印纯整数。例如 `600`。

面板按钮「复制 TTL」复制同一整数。复制内容不带单位、不带换行、不带 `s`（`frontend/src/lowcfg.tsx:63`）。成功后 1800 ms 内按钮显示「已复制」（`frontend/src/lowcfg.tsx:81-83`）。

控制台单位换算：
- 单位是秒：填复制的整数。
- 单位是分钟：填 `TTL / 60`。600 秒填 10。300 秒填 5。900 秒填 15。

控制台单位未在仓库定义（第 2.6 节）。记录实际单位。

### 3.7 控制台加两条规则

顺序固定。命中即止。规则 A 在最前。

规则 A，不缓存：

```text
/panel*
/epg*
/subs/*
/api/*
/unban*
/share*
/health
/metrics
/sub/*
/*/*.m3u8
/*/*.mpd
/*/*/index.m3u8
/*/*/index.txt
/*/*/video/*
/*/*/mpd/*
```

规则 B，缓存：

```text
/*/*/*
```

依据：`docs/design/06-low-spec-optimization.md:76-86`。

不要按扩展名写规则。伪装扩展名在 `jpg`、`png`、`gif`、`bmp`、`webp` 之间轮换（`src/config/model.rs:526-549`、`src/proxy/media.rs:159-162`）。控制台只支持扩展名时，改用路径前缀 `/{gid}/{cid}/`，逐分组配置（`docs/design/06-low-spec-optimization.md:88-89`）。

规则 A 中的 `/metrics` 与 `/share*` 在本实现没有对应路由。保留这两条不影响结果。本实现只有 `/api/share/pull`（`src/http.rs:38`）。

### 3.8 本实现的路径补丁

本实现的路径与设计稿有三处深度差异。先加 BYPASS，再加 CACHE。控制台不支持多段通配符时，按 `/{gid}/{cid}/` 前缀逐分组配置。

规则 A 补充，不缓存：

| 路径形状 | 源码 | 规则 |
| --- | --- | --- |
| `/api/share/pull` | `src/http.rs:38` | `/api/share/*` |
| `/logo/api/*` | `src/http.rs:35` | `/logo/api/*` |
| `/__assets/*` | `src/http.rs:59` | `/__assets/*` |
| `/subtitle/*` | `src/http.rs:171-175` | `/subtitle/*` |
| `/{gid}/{cid}/{track}/index.txt` | `src/dash_hls.rs:998-1000` | `/*/*/*/index.txt` |
| `/{gid}/{cid}/{track}/index.m3u8` | `src/dash_hls.rs:998-1000` | `/*/*/*/index.m3u8` |
| `/{gid}/{cid}/dash-timeshift/{track}/playlist` | `src/dash_timeshift.rs:7` | `/*/*/dash-timeshift/*/playlist` |
| `/{gid}/{cid}/dash-timeshift/{track}/playlist.txt` | `src/dash_timeshift.rs:7` | `/*/*/dash-timeshift/*/playlist.txt` |
| `/{gid}/{cid}/dash-timeshift/{track}/playlist.m3u8` | `src/dash_timeshift.rs:7` | `/*/*/dash-timeshift/*/playlist.m3u8` |

规则 B 补充，缓存：

| 路径形状 | 源码 | 规则 |
| --- | --- | --- |
| `/{gid}/{cid}/timeshift/<16位十六进制>.<ext>` | `src/timeshift.rs:7` | `/*/*/timeshift/*` |
| `/{gid}/{cid}/{track}/<文件名>` | `src/dash_hls.rs:860-871` | `/*/*/*/*` |
| `/{gid}/{cid}/dash-timeshift/{track}/<文件名>` | `src/dash_timeshift.rs:7` | `/*/*/dash-timeshift/*/*` |
| `/assets/*` | `src/http.rs:60` | `/assets/*` |
| `/logo/*` | `src/http.rs:36` | `/logo/*` |

`/*/*/*/*` 会匹配四段路径。`/panel*` 与 `/epg*` 已在规则 A 中先行命中。顺序不能反。

### 3.9 保存并刷新

1. 保存控制台规则。
2. 执行「刷新全部缓存」。控制台可能写作 Purge All。
3. 四个域名各做一遍。支持「应用到全部域名」时，第 3.6 至 3.8 节只做一次。

依据：`docs/design/06-low-spec-optimization.md:93-99`。

### 3.10 准备验证变量

```bash
CDN=https://<你的 CDN 域名>
TOKEN_ENABLED=$(curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/config" | jq -r .auth.token_enabled)
PLAY=""
if [ "$TOKEN_ENABLED" = "true" ]; then
  PLAY=$(curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/security/token" | jq -r .current_value)
fi
MANIFEST="$CDN/$GID/$CID.$EXT"
if [ -n "$PLAY" ]; then MANIFEST="$MANIFEST?token=$PLAY"; fi
printf 'MANIFEST=%s\n' "$MANIFEST"
```

预期：`MANIFEST` 指向 CDN 域名。`PLAY` 为空或为非空 Token。

`[auth].token_enabled` 默认 `false`（`src/config/model.rs:726-756`）。Token 开启且分组不是 `open` 时，入口 URL 必须带 `?token=`（`src/playlist.rs:120-127`）。切片 URL 不带查询串，也不需要 Token。

### 3.11 验证清单 BYPASS

```bash
curl -sS -o /dev/null -D - "$MANIFEST" | sed -n '1,25p'
```

预期：
- 状态行是 `HTTP/2 200` 或 `HTTP/1.1 200`。
- CDN 缓存状态头为 `BYPASS`。
- `cache-control` 为 `no-cache, no-store`。直播清单由源站给出该值（`src/http/routes/stream.rs:621-630`）。

CDN 缓存状态头的头名随供应商。记录实际头名与值。

### 3.12 验证切片由 CDN 缓存

从清单取第一片：

```bash
PLAYLIST=$(curl -sS "$MANIFEST")
FIRST=$(printf '%s\n' "$PLAYLIST" | grep -v '^#' | grep -v '^$' | head -n1)
if printf '%s' "$FIRST" | grep -qE '/index\.(txt|m3u8)(\?|$)'; then
  FIRST=$(curl -sS "$FIRST" | grep -v '^#' | grep -v '^$' | head -n1)
fi
SEG_PATH=$(printf '%s\n' "$FIRST" | sed -E 's#^[a-z]+://[^/]+##')
SEG_CDN="$CDN$SEG_PATH"
printf 'SEG_PATH=%s\n' "$SEG_PATH"
printf 'SEG_CDN=%s\n' "$SEG_CDN"
```

预期：`SEG_PATH` 以 `/$GID/$CID/` 开头。路径含 `_`（伪装切片）、`seg_`（remux 切片）或 `timeshift/`（时移切片）。

依据：伪装切片路径是 `/{gid}/{cid}/{P}_{S}.{ext}`（`src/proxy/media.rs:335`）。`{P}` 是频道摘要前 16 位小写十六进制（`src/proxy/media.rs:65-67`）。`{S}` 是上游绝对 URL 的 SHA-256 前 32 位小写十六进制（`src/proxy/media.rs:70-72`）。

连续请求两次：

```bash
for i in 1 2; do
  echo "--- request $i"
  curl -sS -o /dev/null -D - "$SEG_CDN" | sed -n '1,30p'
done
```

预期：
- 第 1 次：缓存状态 `MISS`。
- 第 2 次：缓存状态 `HIT`，且 `Age` 存在。
- 两次 `content-type` 相同。
- 两次 `content-length` 相同，或都为 chunked。

伪装切片的 `content-type` 白名单：`jpg/jpeg→image/jpeg`、`png→image/png`、`gif→image/gif`、`bmp→image/bmp`、`webp→image/webp`，其余一律 `video/mp2t`（`src/proxy/media.rs:147-155`）。

### 3.13 验证字节未被改写

```bash
CDN_HASH_1=$(curl -sS "$SEG_CDN" | sha256sum | awk '{print $1}')
CDN_HASH_2=$(curl -sS "$SEG_CDN" | sha256sum | awk '{print $1}')
ORIGIN_HASH=$(curl -sS "$BASE$SEG_PATH" | sha256sum | awk '{print $1}')
printf 'cdn1=%s\ncdn2=%s\norigin=%s\n' "$CDN_HASH_1" "$CDN_HASH_2" "$ORIGIN_HASH"
```

预期：三个哈希完全一致。

`sha256sum` 不存在时，改用 `shasum -a 256`，取第一列。

### 3.14 验证清单下发 https 地址

```bash
curl -sS "$MANIFEST" | grep -v '^#' | grep -v '^$' | head -n1
```

预期：首行以 `https://<你的 CDN 域名>/` 开头。

依据：本服务用 `x-forwarded-proto` 与 `Host` 推导对外基址（`src/http/routes/stream.rs:1220-1240`）。`X-Forwarded-Proto` 缺省值是 `http`（`src/http/routes/stream.rs:1229-1232`）。CDN 改写 `Host` 为源站地址时，地址也会错。

### 3.15 验证源站切片响应头

本步验证源站，不验证 CDN。先清本地缓存，让下一次请求回源：

```bash
curl -sS -X POST -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/cache/flush" | jq .
```

预期：`{"ok":true,"removed":<整数>}`。清缓存删除 `m3u8`、`seg_url`、`seg_bytes`、`sub_url` 四个目录（`src/http/routes/admin.rs:18`）。`seg_url` 索引同时被删，所以必须重取清单。

```bash
ORIGIN_MANIFEST="$BASE/$GID/$CID.$EXT"
if [ -n "$PLAY" ]; then ORIGIN_MANIFEST="$ORIGIN_MANIFEST?token=$PLAY"; fi
PLAYLIST_ORIGIN=$(curl -sS "$ORIGIN_MANIFEST")
LAST=$(printf '%s\n' "$PLAYLIST_ORIGIN" | grep -v '^#' | grep -v '^$' | tail -n1)
if printf '%s' "$LAST" | grep -qE '/index\.(txt|m3u8)(\?|$)'; then
  LAST=$(curl -sS "$LAST" | grep -v '^#' | grep -v '^$' | tail -n1)
fi
LAST_PATH=$(printf '%s\n' "$LAST" | sed -E 's#^[a-z]+://[^/]+##')
echo "--- first"
curl -sS -o /dev/null -D - "$BASE$LAST_PATH" | grep -iE '^(cache-control|content-type|content-length)'
echo "--- second"
curl -sS -o /dev/null -D - "$BASE$LAST_PATH" | grep -iE '^(cache-control|content-type|content-length)'
```

预期：

| `[cdn].enabled` | 第 1 次 | 第 2 次 |
| --- | --- | --- |
| `false`（默认） | `public, max-age=300` | `private, max-age=300` |
| `true` | `public, max-age=<TTL>` | `private, max-age=<TTL>`，`shared_cache_hits=true` 时为 `public` |

`[cache.ttl].segment_secs` 默认 300（`src/config/model.rs:399-416`）。`[cdn].enabled=false` 时用它（`src/http/routes/stream.rs:47-55`）。`[cdn].enabled=true` 时用 `[cdn].segment_max_age_secs`、实测值或 600（`src/http/routes/stream.rs:57-63`）。

预取默认开启（`src/config/model.rs:287-298`）。多数切片会先落到本地磁盘。人工请求得到 `private` 属正常现象。本步用最后一��，`skip_tail=1` 默认不预取最后一片。清单只有 1 片时，第 1 次也可能是本地命中。记录该情况，继续。

### 3.16 验证 TTL 到期

本步可选。等待时间较长。

```bash
TTL=$(curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/cache/stats" | jq -r .recommend.ttl_secs)
sleep $((TTL + 60))
curl -sS -o /dev/null -D - "$SEG_CDN" | sed -n '1,25p'
```

预期：缓存状态为 `MISS`，或 `Age` 大于 `TTL`。仍为 `HIT` 且 `Age` 小于 `TTL` 表示控制台未按填写值生效。

### 3.17 验证面板与订阅清单

面板接口：

```bash
curl -sS -o /dev/null -D - -H "Authorization: Bearer $ADMIN" "$CDN/panel/api/status" | sed -n '1,25p'
```

预期：`200`。缓存状态 `BYPASS`。未带会话时返回 401（`src/http/ext.rs:23-36`、`src/http/error.rs:64`）。带会话时返回 200。

订阅清单：

```bash
curl -sS -o /dev/null -D - "$CDN/sub/<令牌>/playlist.m3u" | sed -n '1,25p'
```

预期：`200`。缓存状态 `BYPASS`。源站不给 `Cache-Control`，所以可能没有该头。

面板静态资源：

```bash
curl -sS -o /dev/null -D - "$CDN/__assets/panel.js" | sed -n '1,25p'
```

预期：`BYPASS`。源站给 `no-cache`（`src/http/static_assets.rs:23-27`）。文件名以实际构建产物为准。

### 3.18 验证换台不 404

连续请求两个不同频道入口：

```bash
curl -sS -o /dev/null -D - "$CDN/$GID/$CID.$EXT" | sed -n '1,15p'
curl -sS -o /dev/null -D - "$CDN/$GID/$CID.$EXT" | sed -n '1,15p'
```

预期：两次都是 `200`。第二次不是缓存的 302。

302 响应没有 `Cache-Control`（`src/http/routes/stream.rs:1537-1549`）。只能靠「不缓存 3xx」保护。

### 3.19 四个域名重复

对第 2、3、4 个域名重复第 3.10 至 3.18 节。每个域名单独记录。

### 3.20 结束

退出面板会话：

```bash
curl -sS -X POST -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/logout" | jq .
```

预期：`{"ok":true}`。

## 4. 判定

| ID | 检查 | 通过 | 不通过 | 依据 |
| --- | --- | --- | --- | --- |
| J1 | 面板会话 | `ADMIN` 为非空 UUID | 空值，或登录返回非 200 | `src/http/routes/panel.rs:37-91` |
| J2 | 推荐值字段 | 14 个键齐全，`ttl_secs` 为整数 | 缺键，或 `ttl_secs` 非整数 | `src/cache_gc.rs:283-333` |
| J3 | TTL 范围 | `measured` 时 300–900；`fallback` 时 600；`manual` 时为配置值 | 超出范围 | `src/cache_estimate.rs:30-42` |
| J4 | 清单 BYPASS | CDN 缓存状态为 `BYPASS` | `HIT`、`MISS`、`STALE` | `docs/design/06-low-spec-optimization.md:118` |
| J5 | 切片第 2 次 | `HIT`，且 `Age` 存在 | 仍为 `MISS` 或 `BYPASS` | `docs/design/06-low-spec-optimization.md:118` |
| J6 | 切片字节 | 三个 SHA-256 完全相同 | 任一不同 | `docs/design/06-low-spec-optimization.md:112-118` |
| J7 | 面板接口 | `BYPASS` | 非 `BYPASS` | `docs/design/06-low-spec-optimization.md:115-118` |
| J8 | 订阅清单 | `BYPASS` | 非 `BYPASS` | `docs/design/06-low-spec-optimization.md:118` |
| J9 | 清单地址 | 首行以 `https://<CDN 域名>/` 开头 | `http://`，或指向源站 | `src/http/routes/stream.rs:1220-1240` |
| J10 | 源站头 | `[cdn].enabled=false` 时 `max-age=300`；`true` 时 `max-age=<TTL>` | 值不符 | `src/http/routes/stream.rs:47-70` |
| J11 | 换台 | 两次 `200`，第二次不是缓存 302 | 第二次 404 或 302 | `src/http/routes/stream.rs:1537-1549` |
| J12 | TTL 到期 | `TTL+60` 秒后 `MISS`，或 `Age > TTL` | 仍 `HIT` 且 `Age < TTL` | 控制台侧，第 2.6 节 |
| J13 | 四域名 | 四个域名 J4–J9 全部通过 | 任一域名失败 | `docs/design/06-low-spec-optimization.md:146-150` |

单项判定只用三种取值：通过、不通过、未验证。未验证必须写缺什么。

## 5. 记录模板

复制以下内容到 `docs/tests/runs/<日期>.md`。日期格式 `YYYY-MM-DD`。

```markdown
# CDN 面板配置记录 YYYY-MM-DD

## 环境

- VPS：
- 服务版本：
- 配置路径：
- 测试分组：gid=... cid=... mode=... format=... access=...
- `[cdn]`：enabled=... auto_tune=... segment_max_age_secs=... shared_cache_hits=...
- `[cache].backend`：
- 推荐值：ttl_secs=... source=... disk_cap_mb=... groups=... updated_secs_ago=...
- 控制台厂商与入口：
- 控制台字段实际名称：
  - 第 1 项：
  - 第 2 项：
  - 第 3 项：
  - 第 4 项：
  - 第 5 项：
  - 不缓存 3xx：
  - 不缓存非 200：
- 缓存状态头名：
- 控制台 TTL 单位：

## 规则

- 规则 A 条数：
- 规则 B 条数：
- 补充 BYPASS 条数：
- 补充 CACHE 条数：
- 保存时间（UTC）：
- Purge All 时间（UTC）：

## 域名 1：<域名>

| ID | 结论 | 实际现象 |
| --- | --- | --- |
| J1 | | |
| J2 | | |
| J3 | | |
| J4 | | |
| J5 | | |
| J6 | | |
| J7 | | |
| J8 | | |
| J9 | | |
| J10 | | |
| J11 | | |
| J12 | | |

清单响应头：

```text
<粘贴 curl -D - 输出>
```

切片第 1 次响应头：

```text
<粘贴>
```

切片第 2 次响应头：

```text
<粘贴>
```

哈希：

```text
cdn1=
cdn2=
origin=
```

清单首行：

```text
<粘贴>
```

## 域名 2：<域名>

（同上）

## 域名 3：<域名>

（同上）

## 域名 4：<域名>

（同上）

## 总结

- 通过项：
- 不通过项：
- 未验证项与缺少的条件：
```

## 6. 失败时怎么报

### 6.1 停止条件

一个域名出现 J4、J5、J6、J9 中任一项不通过，停止该域名的后续检查。先收集证据。不要改规则后立刻重试。

### 6.2 收集内容

在同一时间窗口内执行：

```bash
date -u +%FT%TZ
curl -sS -o /dev/null -D - "$MANIFEST"
curl -sS -o /dev/null -D - "$SEG_CDN"
curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/logs?limit=500" | jq -r '.lines[]'
tail -n 200 /opt/tv-buddy/tv-buddy.log
curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/cache/stats" | jq .recommend
curl -sS -H "Authorization: Bearer $ADMIN" "$BASE/panel/api/config" \
  | jq '{cdn, cache: {backend: .cache.backend, disk_path: .cache.disk_path, ttl: .cache.ttl, prefetch: .cache.prefetch, gc: .cache.gc}, token_enabled: .auth.token_enabled}'
```

`GET /panel/api/logs` 读取进程内环形缓冲，默认 200 行，上限 500 行（`src/http/routes/admin.rs:133-148`）。

日志文件是 `tv-buddy.log`，位于配置文件同目录（`src/logging.rs:163-173`）。

### 6.3 附带的文件

| 文件 | 内容 |
| --- | --- |
| `docs/tests/runs/<日期>.md` | 第 5 节记录模板，含实际响应头与哈希 |
| `/opt/tv-buddy/tv-buddy.log` | 服务日志，至少失败前后 200 行 |
| 控制台规则截图或文本 | 规则顺序与五处设置的实际字段名 |

不要粘贴面板会话 Token、播放 Token、分组上游地址。

### 6.4 常见现象对照

| 现象 | 先查 | 依据 |
| --- | --- | --- |
| 切片仍 `BYPASS` | 规则未生效；未 Purge；Set-Cookie 未关 | `docs/design/06-low-spec-optimization.md:124` |
| `MISS` 不转 `HIT` | 缓存键含 Cookie 或请求头；TTL 太短 | `docs/design/06-low-spec-optimization.md:125` |
| `HIT` 但花屏或卡住 | 图片优化改写了字节。比对哈希 | `docs/design/06-low-spec-optimization.md:126` |
| 清单被缓存 | 规则顺序颠倒。把 A 调到最前 | `docs/design/06-low-spec-optimization.md:127` |
| 换台 404 | 302 被缓存。关闭 3xx 缓存 | `docs/design/06-low-spec-optimization.md:128` |
| 清单给 `http://` | 未下发 XFP；或 CDN 改写了 Host | `docs/design/06-low-spec-optimization.md:129` |
| 源站命中返回 `private` | 只能「遵循源站头」的 CDN 不缓存。改用强制 TTL，或开 `[cdn].shared_cache_hits=true` | `src/http/routes/stream.rs:57-70` |
| 上游失败被缓存成 200 空体 | 开 `[cdn].enabled=true`，设 `empty_status=502` | `src/http/routes/stream.rs:1579-1594` |
| 订阅清单被缓存 | 源站已发 `Cache-Control: private, no-store`。检查规则 A 是否仍 BYPASS `/sub/*` | `src/http/routes/sub.rs:60-66` |
| 切片 URL 每 5 分钟变一次 | 正常。时间窗签名会让路径变。缓存键必须包含 URI 路径 | `src/sign.rs` |
| 切片返回 `302` | 签名过期或密钥被改。播放器重拉清单即恢复 | `src/http/routes/stream.rs` |
| 两个用户拿到不同切片 URL | 时间窗不同或密钥不同。检查系统时钟与 `slice_sign_window_secs` | `src/sign.rs` |
| 面板 JS/CSS 被缓存 | `/__assets/*` 不在规则 A。补 BYPASS | `src/http/static_assets.rs:23-27` |
| `/logo/api/*` 被缓存 | 该路径不是媒体，且不在规则 A。补 BYPASS | `src/http.rs:35` |
| DASH 轨道清单被缓存 | 四段路径不在 `/*/*/index.txt`。补 BYPASS | `src/dash_hls.rs:998-1000` |
| 时移切片不缓存 | 四段路径不在 `/*/*/*`。补 CACHE | `src/timeshift.rs:7` |
| DASH 时移清单被缓存 | 五段路径不在规则 A。补 BYPASS | `src/dash_timeshift.rs:7` |
| `HIT` 但 `content-length` 变化 | CDN 压缩或转码。关闭内容压缩改写与图片优化 | `docs/design/06-low-spec-optimization.md:68` |
| 源站 `max-age` 不是推荐 TTL | `[cdn].enabled=false`。此时源站用 `[cache.ttl].segment_secs`，默认 300 | `src/http/routes/stream.rs:47-55` |
| `source` 显示「手动配置」但 TTL 未手填 | 磁盘上限手填也会置 `manual` | `src/cache_gc.rs:290-298` |
| 估算返回 409 | 已有估算在跑。等待后重试一次 | `src/http/routes/cdn.rs:34-36` |
| 估算全失败 | 上游不可达或清单无 `#EXTINF`。看 `failed[].error` | `src/http/routes/cdn.rs:96-118` |
| 面板接口返回 401 | 未带会话。缓存头检查仍有效，但不能把 200 当预期 | `src/http/ext.rs:23-36` |

### 6.5 报给开发者时写清

1. 失败的检查 ID（J1–J13）。
2. 域名。
3. 实际响应头原文。
4. 实际哈希。
5. 控制台实际字段名与取值。
6. 服务版本。
7. 日志文件路径。

开发者按 `docs/tests/README.md` 第 5 节在 `PROGRESS.md` 立缺口。