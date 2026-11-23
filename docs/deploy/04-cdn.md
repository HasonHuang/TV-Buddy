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

### 6.1 规则

不缓存，放在最前：

```text
/api/*
/page*
```

缓存，放在其后：

```text
/media/*
/static/panel/*
/static/logo/*
/static/assets/*
```

不要加兜底 `/*` 规则。

说明：

- `/api/*` 覆盖面板 API、订阅、清单、解封接口、健康检查与分享接口。订阅与清单都不缓存。
- `/media/*` 只包含切片、初始化段、时移切片、DASH 切片与 MPD 代理数据。
- `/static/*` 包含台标、面板构建产物与本地素材。`/static/assets/*` 里的 HLS 清单也按静态资源缓存。
- `/page*` 覆盖 `/page` 与 `/page/unban`。它返回 HTML。CDN 不缓存。不要写成 `/page/*`。部分控制台不会用 `/page/*` 匹配裸 `/page`。
- 根路径 `/` 默认 `302` 到 `/page`。§4.2 要求不缓存 3xx，所以 `/` 不入缓存。不要把 `/` 写成前缀规则；部分控制台会把它当成全部路径。
- 不设兜底 `/*` 规则。源站对未知路径返回 `404`。§4.2 要求不缓存非 200。

不要颠倒顺序。缓存规则放在不缓存规则之前时，`/api/*` 与 `/page*` 会被缓存。

### 6.2 不要按扩展名写规则

切片伪装扩展名在 `jpg`、`png`、`gif`、`bmp`、`webp` 之间轮换。关闭伪装时又是 `ts`。按扩展名写规则会漏掉部分切片。

控制台只支持扩展名时，改用路径前缀 `/media/` 或 `/static/assets/`。

不同控制台的通配符语义不同。仓库不定义 `*` 是否跨多段。若控制台按单段匹配，改用「路径前缀」规则，不要用通配符规则。

### 6.3 保存与刷新

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

`token` 只在入口清单需要。切片 URL 不带 token。清单路径是 `/api/play/...`，由 `/api/*` 不缓存规则覆盖。切片路径以 `/media/` 开头，由 `/media/*` 缓存规则覆盖。

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
curl -sS -o /dev/null -D - "$CDN/page" \
  | grep -iE 'x-cache|cache-control'
```

期望：两个请求都是 `BYPASS`。第二个请求验证 `/page*` 规则覆盖裸 `/page`。

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

期望：第二次请求返回 `HIT` 并带 `Age`。源站台标头为 `public, max-age=3600`。控制台使用强制 TTL 时，以控制台 TTL 为准。

完整验收流程见 [`../tests/06-cdn-console.md`](../tests/06-cdn-console.md)。

## 9. 失败对照

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 切片一直 `BYPASS` | 规则未生效、未刷新缓存、Cookie 未关 | 检查 §4.1 第 1、3 项。执行 Purge All |
| `MISS` 不转 `HIT` | 缓存键包含 Cookie 或请求头，或 TTL 太短 | 缓存键去掉 Cookie。使用推荐 TTL |
| `HIT` 但画面损坏或卡住 | 图片优化或压缩改写了字节 | 关闭图片优化、WebP、压缩。比对哈希 |
| 清单被缓存 | 规则顺序颠倒 | 把 `/api/*` 不缓存规则放在最前 |
| 切换频道返回 404 | 302 被缓存 | 关闭 3xx 缓存 |
| 清单仍给 `http://` | 缺少 `X-Forwarded-Proto` | 回源加 `X-Forwarded-Proto: https`，或设置 `public_base` |
| 切片 404 | 切片路径被缓存成了清单地址 | Purge。确认 `/api/*` 不缓存、`/media/*` 缓存 |
| 回源 502 | CDN 回源地址或端口错误 | 检查源站域名、端口、防火墙 |
| 面板被缓存 | 不缓存规则缺少 `/page*` 或 `/api/*` | 补不缓存规则 |
| 新切片一直 `BYPASS` | 缺少 `/media/*` 缓存规则 | 补缓存规则 |
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
