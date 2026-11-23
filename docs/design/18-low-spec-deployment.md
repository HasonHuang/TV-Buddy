# 18 · 低配置部署

> 状态：已实施（T0 待人工）｜ 最后更新：2026-11-21
> 场景：1–5 个用户自用。1 vCPU / 128 MB 内存 / 1 GB 磁盘 / 大带宽。
> 相关：[`06-low-spec-optimization.md`](06-low-spec-optimization.md)、[`10-low-vs-high-spec.md`](10-low-vs-high-spec.md)、[`config-low-spec.json`](config-low-spec.json)

## 1. 适用场景

TV Buddy 可在 **1 vCPU / 128 MB 内存 / 1 GB 磁盘** 的实例上运行。前提是前面挂 CDN，把播放流量挡在边缘。

1 GB 磁盘是硬约束。切片正文只写不删，磁盘回收常开。

## 2. 安装

安装脚本会询问是否使用低配参数。加 `--low-spec` 跳过询问：

```bash
sudo ./install.sh --low-spec
```

脚本写入 5 个键：

| 键 | 默认（常规） | 低配 | 作用 |
| --- | --- | --- | --- |
| `cache.segments.per_channel_max` | `0`（内置 4096） | `200` | 每频道切片索引条数上限。降低内存占用 |
| `cache.prefetch.concurrency` | `4` | `2` | 预取并发。降低开播与切台的 CPU 峰值 |
| `auth.token_enabled` | `true` | `true` | 播放凭证。每台设备只能播放授权频道 |
| `auth.ip_guard_enabled` | `true` | `true` | IP 限流、播放会话上限与封禁 |
| `cdn.enabled` | `false` | `true` | 外部 CDN 未生效时，本机补齐缓存头与错误状态 |

只有前 2 项减负。后 3 项改变安全或分发行为。

面板里的「低配模式」只回显，不改变参数。不要把它当作切换开关。

各键的详细差异见 [`10-low-vs-high-spec.md`](10-low-vs-high-spec.md) §1。

## 3. 已安装用户如何切换

用面板操作。保存后立即生效，不需要重启。

| 顺序 | 面板路径 | 改为 |
| --- | --- | --- |
| 1 | 系统设置 → 缓存与回收 → 高级设置 → 单频道索引条目上限 | `200` |
| 2 | 系统设置 → 缓存与回收 → 高级设置 → 预载配置 → 拉取并发 | `2` |
| 3 | 系统设置 → 分发与伪装 → CDN 兜底 → 启用 CDN 兜底 | 开启 |
| 4 | 安全管控 → 访问与链接 → IP 管控 | 开启 |
| 5 | 安全管控 → 访问与链接 → 播放凭证 | 开启 |

只想省资源时，做前 2 项。

已有数据库保留原值。新的默认值只影响新建配置。

默认 `auth.max_sessions_per_ip = 1`。同一出口 IP 同时只保留 1 路播放会话。多人共用网络时调大。

详见 [`10-low-vs-high-spec.md`](10-low-vs-high-spec.md) §5.0。

## 4. 两个硬前提

缺一条就会在 1 GB 磁盘上出问题。

| # | 前提 | 说明 |
| --- | --- | --- |
| 1 | 磁盘回收常开 | 切片正文只写不删，全靠它回收。没有关闭开关 |
| 2 | CDN 侧强制 TTL + 忽略源站头 + 忽略 Cookie | 四家 CDN 的规则内容相同。见 §6 |

## 5. 服务端配置

`--low-spec` 写入 5 个键。完整样例见 [`config-low-spec.json`](config-low-spec.json)。

```json
{
  "server": { "bind": "[::]:9900", "log_level": "info" },
  "panel": { "enabled": true },
  "auth": { "token_enabled": true, "ip_guard_enabled": true },
  "cache": {
    "disk_path": "./cache",
    "limit_mb": 0,
    "auto_tune": true,
    "segments": { "per_channel_max": 200 },
    "prefetch": { "enabled": true, "concurrency": 2 },
    "gc": { "max_age_secs": 1800 },
    "ttl": { "sub_url_secs": 1800, "segment_secs": 0 }
  },
  "cdn": {
    "enabled": true,
    "shared_cache_hits": false,
    "dont_cache_empty": false,
    "empty_status": 502
  }
}
```

| 键 | 说明 |
| --- | --- |
| `limit_mb = 0` | 自动。1 GB 盘通常落在 100–150 MB。非 0 为手动上限，优先于自动值 |
| `auto_tune = true` | 被动采样。改为 `false` 只停止采样，不冻结上限 |
| `max_age_secs = 1800` | 过期删除阈值。`0` 关闭超龄删除；`1..=59` 拒绝 |
| `segment_secs = 0` | 切片 TTL 用实测值，无数据用 600 秒。手动值在 CDN 开/关下都生效 |
| `empty_status = 502` | 在强制 TTL 下挡住「200 + 空体」 |

磁盘回收常开，扫描周期固定 60 秒。磁盘索引占上限的 10% 子预算；进程内索引 600 秒
无活动即淘汰。CDN 不下发 `X-Forwarded-Proto` 时，在 `cdn` 里加
`"public_base": "https://cdn.example"`。
公式与推荐值来源见 [`06-low-spec-optimization.md`](06-low-spec-optimization.md) §3。

## 6. CDN 控制台设置

五个设置项，四个域名各做一遍。面板支持「应用到全部域名」时做一次。

| # | 设置项 | 设成 | 原因 |
| --- | --- | --- | --- |
| 1 | 缓存过期时间 | 媒体路径用推荐 TTL | 覆盖窗口加两次重试余量 |
| 2 | 是否遵循源站缓存头 | 关闭（强制 TTL） | 各家能力不一致，强制 TTL 最容易移植 |
| 3 | Set-Cookie | 媒体路径关闭注入 | 每个响应带新 Cookie 时 CDN 跳过缓存 |
| 4 | 图片优化 / 自动 WebP / 压缩改写 | 全部关闭 | 切片是 TS 字节，转码后会「命中但播不动」 |
| 5 | 回源请求头 | 加 `X-Forwarded-Proto: https` | 否则清单一直给 `http://` |

另外确认：**不缓存 3xx**（`redirect` 模式会 302 到带时效的上游地址）、**不缓存非 200**。

缓存规则分四层，顺序不能颠倒：

```text
规则 A：不缓存（放最前，迁移期保留）
  /panel*  /epg*  /subs/*  /sub/*  /api/*  /unban*  /share*  /health  /metrics
  /*/*.m3u8  /*/*.mpd
  /*/*/index.m3u8  /*/*/index.txt  /*/*/video/*  /*/*/mpd/*

规则 B：缓存（迁移期保留）
  /*/*/*

新增不缓存（放在规则 B 之前）
  /api/*  /sub/*  /page/*  /logo/api/*  /__assets/*  /subtitle/*  /l/*
  /*/*/*/index.txt  /*/*/*/index.m3u8  /*/*/dash-timeshift/*/playlist*

新增缓存（放在兜底之前）
  /media/*  /static/panel/*  /static/logo/*  /static/assets/*

兜底不缓存（放最后）
  /*
```

**不要按扩展名写规则。** 伪装扩展名在 jpg/png/gif/bmp/webp 之间轮换。控制台只支持扩展名时，改用路径前缀 `/media/` 或 `/static/assets/`。旧路径用 `/{gid}/{cid}/` 前缀，逐分组配置。

完整规则、路径补丁与验收命令见 [`../deploy/04-cdn.md`](../deploy/04-cdn.md) §6 与 [`docs/tests/06-cdn-console.md`](../tests/06-cdn-console.md)。

## 7. 推荐值从哪来

TTL 与磁盘上限不能写死。片源窗口在 48–100 秒，单片 1.1–9 MB。固定值要么提前过期，要么长期多占边缘存储。

服务默认被动实测，不向源站额外发请求。首次安装还没有播放数据时用回退值：**TTL 600s / 磁盘 150 MB**。

获取推荐值的两个入口：

| 入口 | 位置 |
| --- | --- |
| 推荐 TTL（只读，带复制按钮） | 面板 → 缓存状态页 →「CDN 推荐值」卡片 |
| 「立即估算」 | 同一张「CDN 推荐值」卡片右上角 |

「立即估算」探测最多 3 个分组、每组 1 个频道，只执行一次。并发请求返回 409。

也可以直接调用接口：

```bash
TOKEN='<面板会话令牌>'
curl -X POST -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:9900/api/panel/cdn/estimate
```

接口需要面板会话；未登录返回 `401`。

公式与取值示例见 [`06-low-spec-optimization.md`](06-low-spec-optimization.md) §3。

## 8. 验收

```bash
# 0) 播放凭证。token_enabled=true 且分组不是 open 时必须带
PLAY='<播放令牌>'
MANIFEST="https://<域名>/api/play/token/$PLAY/<分组>/<变体>.m3u8"

# 1) 清单必须不缓存
curl -s -o /dev/null -D - "$MANIFEST" | grep -iE 'x-cache|cache-control'

# 2) 切片必须缓存。第二次应为 HIT 且带 Age
SEG="https://<域名>/media/<分组>/<变体>/<从清单复制的切片路径>"
for i in 1 2; do curl -s -o /dev/null -D - "$SEG" | grep -iE '^(x-cache|age|cache-control|content-length)'; done

# 3) 字节未被改写：CDN 两次 + 源站一次
hash_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum; else shasum -a 256; fi
}
CDN1=$(curl -s "$SEG" | hash_stdin | awk '{print $1}')
CDN2=$(curl -s "$SEG" | hash_stdin | awk '{print $1}')
ORIGIN=$(curl -s "http://127.0.0.1:9900/media/<分组>/<变体>/<切片路径>" | hash_stdin | awk '{print $1}')
printf 'cdn1=%s\ncdn2=%s\norigin=%s\n' "$CDN1" "$CDN2" "$ORIGIN"

# 4) 面板与 API 不被缓存
curl -s -o /dev/null -D - "https://<域名>/api/panel/status" | grep -iE 'x-cache|cache-control'
```

期望：清单与面板返回 `BYPASS`。切片第二次返回 `HIT` 并带 `Age`。三个哈希一致（CDN 两次 + 源站一次）。切片地址不带 token；只有清单入口需要 token。`sha256sum` 与 `shasum` 都没有时，先安装 `coreutils` 或 `perl`。

完整验收与记录模板见 [`docs/tests/06-cdn-console.md`](../tests/06-cdn-console.md)。磁盘回收与 24 小时运行验收见 [`docs/tests/05-low-spec-acceptance.md`](../tests/05-low-spec-acceptance.md)。

## 9. 排障

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 切片仍 `BYPASS` | 规则未生效、未刷新缓存、Cookie 未关 | 查设置项 1 与 3 |
| `MISS` 不转 `HIT` | 缓存键含 Cookie 或请求头，或 TTL 太短 | 缓存键去掉 Cookie，TTL 用推荐值 |
| `HIT` 但花屏或卡住 | 图片优化改写了字节 | 关闭图片处理，比对哈希 |
| 清单被缓存 | 规则顺序颠倒 | 把规则 A 调到最前 |
| 换台 404 | 302 被缓存 | 关闭 3xx 缓存 |
| 清单仍给 `http://` | 未下发 XFP | 加回源请求头，或配 `[cdn].public_base` |

更多现象见 [`06-low-spec-optimization.md`](06-low-spec-optimization.md) §2.5 与 [`docs/tests/06-cdn-console.md`](../tests/06-cdn-console.md) §6.4。

## 10. 回滚

低配 profile 只写入 5 个键（§2）。回滚时把省资源的 2 个键改回常规值，并按需关闭 CDN 兜底：

```bash
BIN=/opt/tv-buddy/tv-buddy
DB=/opt/tv-buddy/tv-buddy.db
sudo "$BIN" config set cache.segments.per_channel_max 0 --db "$DB"
sudo "$BIN" config set cache.prefetch.concurrency 4 --db "$DB"
# 不再使用 CDN 兜底时再执行
sudo "$BIN" config set cdn.enabled false --db "$DB"
sudo tv-buddy restart
```

- `auth.token_enabled` 与 `auth.ip_guard_enabled` 的常规默认值也是 `true`，不需要改。
- 磁盘回收常开（§4），没有关闭开关，也不属于低配 profile。
- `groups.0.prefetch.enabled` 只覆盖第一个分组，不能用来回滚全局预取。
- `per_channel_max = 0` 表示使用内置上限 4096。
- 不要省略 `--db`。省略时二进制会写当前目录的 `./tv-buddy.db`。

`config set` 直接写数据库，运行中的进程不会自动读取。执行 `sudo tv-buddy restart`，或在面板会话中调用 `POST /api/panel/reload`。面板里改配置则保存后立即生效。

路径按默认安装目录。自定义 `PREFIX` 时替换 `BIN` 与 `DB`。

详见 [`06-low-spec-optimization.md`](06-low-spec-optimization.md) §10。
