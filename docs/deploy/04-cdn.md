# 04 · CDN 配置

> 状态：操作指南 ｜ 最后更新：2026-11-22
> 适用：在 TV Buddy 前使用外部 CDN。
> 相关：[`../design/18-low-spec-deployment.md`](../design/18-low-spec-deployment.md)、[`../design/06-low-spec-optimization.md`](../design/06-low-spec-optimization.md)、[`../tests/06-cdn-console.md`](../tests/06-cdn-console.md)

本文把 `.m3u8` 与 `.mpd` 索引文件称为「清单」。本文把媒体分片称为「切片」。

CDN 缓存状态常用三个值：`HIT` 表示命中，`MISS` 表示未命中，`BYPASS` 表示不缓存。

## 1. 何时使用

| 场景 | 是否建议 CDN |
| --- | --- |
| 源站带宽小。观众多。 | 建议 |
| 观众分布在不同地区。 | 建议 |
| 多台设备重复播放同一频道。 | 建议 |
| 单机自用。观众很少。 | 不必要 |

CDN 缓存切片。同一个边缘节点对每个新切片通常只回源一次。后续请求由该节点响应。

## 2. 前提

| 项 | 要求 |
| --- | --- |
| 分组模式 | `proxy`。`format` 为 `hls` |
| 源站入口 | 公网域名，或 CDN 可以访问的源站地址 |
| 源站端口 | 默认 `9900`。经 NGINX 时用 `443` |
| Host | 回源时保留观众访问的 Host。不要改写成源站 IP |
| 协议头 | 回源请求加 `X-Forwarded-Proto: https` |
| 面板 | 可以读取 CDN 推荐 TTL |
| 时间 | 源站与 CDN 时间正确。时间偏差会影响 TTL 与缓存 |

`redirect`、`rewrite`、`passthrough` 模式把观众送到上游。CDN 不缓存本项目的切片。

## 3. 架构

```text
观众 → CDN 边缘 → 回源 → TV Buddy:9900 → 上游片源
```

每个 CDN 域名使用自己的 Host 访问源站。切片地址跟随请求 Host。不同域名的缓存不会混在一起。

回源必须保留 Host。CDN 把 Host 改成源站 IP 时，生成的切片地址可能无法播放。此时在服务端设置固定的对外基址，见 §7。

## 4. 控制台设置

不同 CDN 的字段名不同。按同义项配置。支持「应用到全部域名」时，只做一次。

### 4.1 五个设置

| # | 设置项 | 设为 | 原因 |
| --- | --- | --- | --- |
| 1 | 缓存过期时间 / 缓存规则 | 媒体路径使用推荐 TTL | 覆盖窗口加两次重试余量 |
| 2 | 是否遵循源站缓存头 | 关闭。使用强制 TTL | 各家能力不一致。强制 TTL 最容易移植 |
| 3 | Set-Cookie | 媒体路径关闭 Cookie 注入。或选择忽略 Set-Cookie | 每个响应带新 Cookie 时，CDN 会跳过缓存 |
| 4 | 图片优化 / 自动 WebP / 内容压缩改写 | 全部关闭。至少对媒体路径关闭 | 切片是 TS 字节。改写后「命中但无法播放」 |
| 5 | 回源请求头 | 增加 `X-Forwarded-Proto: https` | 否则清单一直下发 `http://` |

### 4.2 两个确认

| 项 | 设为 |
| --- | --- |
| 不缓存 3xx | 开启 |
| 不缓存非 200 | 开启 |

`redirect` 模式会返回 302 到带时效的上游地址。缓存 302 会导致切换频道返回 404。

## 5. 推荐 TTL

不要写死长期 TTL。片源窗口不同。固定值要么提前过期，要么长期占用边缘存储。

### 5.1 从面板读取

1. 打开面板「缓存状态」页。
2. 找到「CDN 推荐值」卡片。
3. 点「复制」。

复制内容是纯整数，单位是秒。

### 5.2 从接口读取

```bash
ADMIN='<面板会话令牌>'
curl -sS -H "Authorization: Bearer $ADMIN" \
  http://127.0.0.1:9900/api/panel/cache/stats \
  | jq -r .recommend.ttl_secs
```

首次安装还没有播放数据时，使用回退值 **600 秒**。

需要立即估算时：

```bash
curl -sS -X POST -H "Authorization: Bearer $ADMIN" \
  http://127.0.0.1:9900/api/panel/cdn/estimate
```

估算探测最多 3 个分组，每组 1 个频道。并发请求返回 `409`。

### 5.3 单位换算

| 控制台单位 | 填法 |
| --- | --- |
| 秒 | 直接填复制值。例如 `600` |
| 分钟 | 用秒数除以 60。600 秒填 `10` |
| 小时 | 用秒数除以 3600 |

推荐值的计算方式见 [`../design/06-low-spec-optimization.md`](../design/06-low-spec-optimization.md) §3。结果范围是 300 到 900 秒。

## 6. 缓存规则

规则顺序固定。命中即止。

### 6.1 迁移期规则

迁移期保留现有规则 A 与 B。不要删除。新规则叠加在 A、B 之后。

规则 A，不缓存：

```text
/panel*
/epg*
/subs/*
/sub/*
/api/*
/unban*
/share*
/health
/metrics
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

新增规则，不缓存：

```text
/api/*
/page/*
/l/*
/logo/api/*
```

新增规则，缓存：

```text
/media/*
/static/panel/*
/static/logo/*
/static/assets/*
```

兜底规则，不缓存：

```text
/*
```

兜底规则必须放在最后。否则媒体路径不会被缓存。

### 6.2 迁移后规则

一个主版本后删除旧规则 A 与 B，只保留这组规则。

不缓存：

```text
/api/*
/page/*
/
```

缓存：

```text
/media/*
/static/panel/*
/static/logo/*
/static/assets/*
```

### 6.3 本实现的路径补丁（迁移期）

本实现的旧路径深度与通用设计不同。先加规则 A 的补充，再加规则 B 的补充。迁移后这些补丁随旧路径一起删除。

规则 A 补充，不缓存：

| 路径形状 | 规则 |
| --- | --- |
| `/api/share/*` | `/api/share/*` |
| `/logo/api/*` | `/logo/api/*` |
| `/__assets/*` | `/__assets/*` |
| `/subtitle/*` | `/subtitle/*` |
| `/{gid}/{cid}/{track}/index.txt` | `/*/*/*/index.txt` |
| `/{gid}/{cid}/{track}/index.m3u8` | `/*/*/*/index.m3u8` |
| `/{gid}/{cid}/dash-timeshift/{track}/playlist` | `/*/*/dash-timeshift/*/playlist` |
| `/{gid}/{cid}/dash-timeshift/{track}/playlist.txt` | `/*/*/dash-timeshift/*/playlist.txt` |
| `/{gid}/{cid}/dash-timeshift/{track}/playlist.m3u8` | `/*/*/dash-timeshift/*/playlist.m3u8` |

规则 B 补充，缓存：

| 路径形状 | 规则 |
| --- | --- |
| `/{gid}/{cid}/timeshift/<16位十六进制>.<ext>` | `/*/*/timeshift/*` |
| `/{gid}/{cid}/{track}/<文件名>` | `/*/*/*/*` |
| `/{gid}/{cid}/dash-timeshift/{track}/<文件名>` | `/*/*/dash-timeshift/*/*` |
| `/assets/*` | `/assets/*` |
| `/logo/*` | `/logo/*` |

`/*/*/*/*` 会匹配四段路径。规则 A 的 `/api/*`、`/page/*`、`/panel*` 与 `/epg*` 先行命中。顺序不能反。

### 6.4 不要按扩展名写规则

切片伪装扩展名在 `jpg`、`png`、`gif`、`bmp`、`webp` 之间轮换。关闭伪装时又是 `ts`。

控制台只支持扩展名时，改用路径前缀 `/media/` 或 `/static/assets/`。旧路径用 `/{gid}/{cid}/` 前缀。逐分组配置。

不同控制台的通配符语义不同。仓库不定义 `*` 是否跨多段。若控制台不支持多段通配符，按 `/{gid}/{cid}/` 前缀配置。

### 6.5 保存与刷新

1. 保存控制台规则。
2. 执行「刷新全部缓存」。控制台可能写作 Purge All。
3. 多个域名各做一遍。支持「应用到全部域名」时只做一次。
4. 按 §8 验证。

## 7. 服务端兜底

`[cdn]` 默认关闭。关闭时，响应头与状态码保持不变。

| 键 | 默认 | 作用 | 何时需要 |
| --- | --- | --- | --- |
| `enabled` | `false` | 开启 CDN 兜底 | 需要下面的键生效时 |
| `shared_cache_hits` | `false` | 命中路径的 `private` 改为 `public` | CDN 只能使用「遵循源站头」时 |
| `dont_cache_empty` | `false` | 空体加 `no-store` | CDN 跟随源站头时 |
| `empty_status` | `502` | 上游失败时返回该状态码 | 强制 TTL 时必须靠它挡住「200 + 空体」 |
| `public_base` | 无 | 固定对外基址，例如 `https://cdn.example` | CDN 不下发 `X-Forwarded-Proto` 时 |

注意：

- `public_base` 只在 `cdn.enabled = true` 时生效。
- 切片 TTL 由 `[cache.ttl].segment_secs` 控制，不由 `[cdn]` 控制。`0` 表示使用实测值。
- `empty_status` 只改变 4xx 或 5xx。填 `0` 或 2xx 时状态码仍为 200。
- 强制 TTL 时，建议设置 `empty_status = 502`。播放器收到 5xx 会重试。这比收到损坏的切片更好。

生效方式：

| 修改方式 | 生效时间 |
| --- | --- |
| 面板保存 | 立即。下一请求按新值处理 |
| `tv-buddy config set` 直接写库 | 运行中的进程不会自动读取。执行 `tv-buddy restart` 或 `POST /api/panel/reload` |

配置细节见 [`../design/04-config-and-storage.md`](../design/04-config-and-storage.md) §11.1。

## 8. 验证

### 8.1 准备变量

```bash
CDN='https://<CDN域名>'
MANIFEST="$CDN/api/play/<gid>/<cid>.m3u8?token=<令牌>"
```

`token` 只在入口清单需要。切片 URL 不带 token。旧路径 `/<gid>/<cid>.m3u8` 仍可用，但 CDN 规则应优先覆盖新路径。

### 8.2 清单必须不缓存

```bash
curl -sS -o /dev/null -D - "$MANIFEST" \
  | grep -iE 'x-cache|cache-control'
```

期望：

- 状态码是 200。
- CDN 缓存状态是 `BYPASS`。
- `cache-control` 为 `no-cache, no-store`。

CDN 缓存状态头的头名随供应商。记录实际头名与值。

### 8.3 切片必须缓存

```bash
PLAYLIST=$(curl -sS "$MANIFEST")
FIRST=$(printf '%s\n' "$PLAYLIST" | grep -v '^#' | grep -v '^$' | head -n1)
if printf '%s' "$FIRST" | grep -qE '/index\.(txt|m3u8)(\?|$)'; then
  FIRST=$(curl -sS "$FIRST" | grep -v '^#' | grep -v '^$' | head -n1)
fi
SEG_PATH=$(printf '%s' "$FIRST" | sed -E 's#^[a-z]+://[^/]+##')
SEG="$CDN$SEG_PATH"
for i in 1 2; do
  curl -sS -o /dev/null -D - "$SEG" \
    | grep -iE '^(x-cache|age|cache-control|content-length)'
done
```

期望：切片路径以 `/media/` 开头。第二次请求返回 `HIT` 并带 `Age`。

### 8.4 字节不能被改写

```bash
hash_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum
  else
    shasum -a 256
  fi
}
CDN1=$(curl -sS "$SEG" | hash_stdin | awk '{print $1}')
CDN2=$(curl -sS "$SEG" | hash_stdin | awk '{print $1}')
ORIGIN=$(curl -sS "http://127.0.0.1:9900$SEG_PATH" | hash_stdin | awk '{print $1}')
printf 'cdn1=%s\ncdn2=%s\norigin=%s\n' "$CDN1" "$CDN2" "$ORIGIN"
```

期望：三个哈希一致。

### 8.5 面板与接口必须不缓存

```bash
curl -sS -o /dev/null -D - "$CDN/api/panel/status" \
  | grep -iE 'x-cache|cache-control'
curl -sS -o /dev/null -D - "$CDN/page/panel" \
  | grep -iE 'x-cache|cache-control'
```

期望：两个请求都是 `BYPASS`。

### 8.6 清单必须下发 https

```bash
curl -sS "$MANIFEST" | grep -E '^https?://' | head
```

期望：切片地址以 `https://` 开头。切片地址的路径以 `/media/` 开头。

### 8.7 台标必须缓存

```bash
LOGO="$CDN/static/logo/<已有台标文件>"
for i in 1 2; do
  curl -sS -o /dev/null -D - "$LOGO" \
    | grep -iE '^(x-cache|age|cache-control|content-length)'
done
```

期望：第二次请求返回 `HIT` 并带 `Age`。台标响应头为 `public, max-age=86400`。

完整验收流程见 [`../tests/06-cdn-console.md`](../tests/06-cdn-console.md)。

## 9. 失败对照

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 切片一直 `BYPASS` | 规则未生效、未刷新缓存、Cookie 未关 | 检查 §4.1 第 1、3 项。执行 Purge All |
| `MISS` 不转 `HIT` | 缓存键包含 Cookie 或请求头，或 TTL 太短 | 缓存键去掉 Cookie。使用推荐 TTL |
| `HIT` 但画面损坏或卡住 | 图片优化或压缩改写了字节 | 关闭图片优化、WebP、压缩。比对哈希 |
| 清单被缓存 | 规则顺序颠倒 | 规则 A 放在最前 |
| 切换频道返回 404 | 302 被缓存 | 关闭 3xx 缓存 |
| 清单仍给 `http://` | 缺少 `X-Forwarded-Proto` | 回源加 `X-Forwarded-Proto: https`，或设置 `public_base` |
| 切片 404 | 切片路径被缓存成了旧清单地址 | Purge。确认规则 A 不缓存清单 |
| 回源 502 | CDN 回源地址或端口错误 | 检查源站域名、端口、防火墙 |
| 面板被缓存 | 规则 A 缺少 `/page/*` 或 `/api/*` | 补规则 A |
| 新切片一直 `BYPASS` | 缺少 `/media/*` 缓存规则 | 补缓存规则。确认兜底规则在最后 |
| 缓存命中率低 | 每个 POP 各自 Miss，或伪装路径未命中 | 检查规则覆盖。多 POP 场景属正常 |

## 10. 多 CDN

POP 指 CDN 边缘节点。

| 项 | 要求 |
| --- | --- |
| 规则 | 每个 CDN 使用同一套规则 |
| Host | 每个域名使用自己的 Host 访问源站 |
| 回源头 | 每个域名都加 `X-Forwarded-Proto: https` |
| 刷新 | 每个域名分别刷新缓存 |
| 边缘位置 | 对大陆用户，SG/JP 通常优于 US/RU |
| DNS | 优先使用 SG/JP 节点 |

每个新切片在每个活跃 POP 各 Miss 一次。源站缓存写入后，其他 POP 不再回源。

## 11. 安全

- 不缓存面板、API、订阅与清单。
- 不缓存 3xx 与非 200。
- 播放令牌只放在入口清单。切片地址不带令牌。
- 公网回源使用 HTTPS。内网回源可以使用 HTTP，但要限制来源 IP。
- 尽量只允许 CDN 回源 IP 访问源站。
- 不要在 CDN 开启图片优化、自动 WebP 或内容压缩改写。
- 定期检查缓存命中率与源站带宽。异常回源可能是规则失效或缓存键包含 Cookie。
