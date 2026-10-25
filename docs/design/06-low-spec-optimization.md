# 低配置 + 多 CDN 优化设计

> 场景：1–5 个用户自用。1 vCPU / 64–128 MB 内存 / 1 GB 磁盘 / 大带宽。
> VPS 与源站同在美国西部。
> 链路：大陆用户 → 4 家 CDN（同一控制面板）→ VPS → HLS 源站（4–9 Mbps）。
>
> 状态：代码侧已实施。T0（CDN 控制台配置）未完成，见 §2。
> 最后更新：2026-10-25。

本设计修三处缺陷：

1. 四家 CDN 全部返回 `BYPASS`。边缘缓存未生效。
2. 源站切片带 `Set-Cookie`。多数 CDN 因此拒绝共享缓存。
3. 切片正文（`<cache>/seg_bytes/`）与磁盘索引（`<cache>/seg_url/`）只写不删。
   1 GB 磁盘会被占满。

范围外：切片 URL、伪装扩展名轮换、清单响应头、路由表。这些不改。
本设计不引入嵌入式数据库（`[cache].backend` 保持 `disk`），也不新增环境变量。

---

## 0. 三级缓存与名词

```
用户 → ① CDN 边缘缓存 → ② VPS 磁盘缓存（L1）→ ③ 源站
       命中：就近返回       命中：本地磁盘读      同机房，快
```

| 名字 | 是什么 | 设计 |
| --- | --- | --- |
| ① CDN 边缘 | 每家 CDN 的缓存。用户就近命中 | 打开。TTL 用自动推荐值 |
| ② L1（磁盘） | 每次拉清单时，把窗口内的切片抓到 VPS 磁盘。已存在则跳过 | 保持。增加分组与频道粒度覆盖 |

L2（内存滚动预热）取消。它只把 L1 的数据再存一份进内存，收益接近 0。

**磁盘上限**：VPS 本地 `<cache>/seg_bytes` 与 `seg_url` 的总量上限。
上限不写死。默认值由**可用磁盘空间**与片源工作集算出（§3.3）。
例：1 GB 磁盘、可用 800 MB、片源 4.26 MB/片 × 7 片 × 3 频道 → 上限约 **140 MB**。

---

## 1. 现状与目标

| 现象 | 目标 |
| --- | --- |
| 清单、切片、面板路径全部返回 `x-cache: BYPASS` 与 `cache-control: no-cache` | 切片走边缘缓存；清单与面板保持不缓存 |
| 每个响应都注入新的 `set-cookie` | 媒体路径不注入 Cookie |
| 源站切片 MIME 伪装为图片，两次请求字节相同 | 不转码、不改写字节 |
| 清单中的切片地址是 `http://` | 下发 `X-Forwarded-Proto: https`，或配 `public_base` |

样本参数：`TARGETDURATION:8`，窗口 7 片 × 8s = **56s**，单片约 **4.26 MB**。
下文样本值都用这组参数。

---

## 2. CDN 缓存配置（T0，人工）

> 四家 CDN 用同一个控制面板。面板支持「应用到全部域名」时，配置一次。否则逐域重复。
> 四家的规则内容完全相同。

### 2.1 要改的五处

| # | 设置项 | 设成什么 | 为什么 |
| --- | --- | --- | --- |
| 1 | 缓存过期时间 / 缓存规则 | 媒体路径用**自动推荐 TTL** | 覆盖窗口加两次重试余量。更长的值只多占边缘存储 |
| 2 | 是否遵循源站缓存头 | **关闭**（强制 TTL） | 四家能力不一致。强制 TTL 最容易移植 |
| 3 | Set-Cookie | 媒体路径**关闭 Cookie 注入**，或勾选「忽略 Set-Cookie」，或「缓存带 Cookie 的响应」 | 现在的每个响应都带新 Cookie，CDN 会跳过缓存 |
| 4 | 图片优化 / 自动 WebP / 内容压缩改写 | **全部关闭**，至少对媒体路径关闭 | 切片是 TS 字节，只做 MIME 伪装。转码后「命中但播不动」 |
| 5 | 回源请求头 | 下发 `X-Forwarded-Proto: https` | 否则清单一直给 `http://` |

另外两项必须确认：**不缓存 3xx**（`redirect` 模式会 302 到带时效的上游地址）；
**不缓存非 200**。

### 2.2 规则表

顺序很重要。命中即止。

**规则 A：不缓存**（放在最前）

`/panel*`、`/epg*`、`/subs/*`、`/api/*`、`/unban*`、`/share*`、`/health`、`/metrics`、
`/playlist.m3u`、`/*/playlist.m3u`、`/*/*.m3u8`、`/*/*.mpd`、`/*/*/index.m3u8`、`/*/*/index.txt`、
`/*/*/video/*`、`/*/*/mpd/*`

**规则 B：缓存**（放在后面）

`/*/*/*` 中的其余路径。包括伪装切片、remux 的 `seg_*`、`timeshift/*`、DASH 轨道切片。

**不要按扩展名写规则**。伪装扩展名在 jpg/png/gif/bmp/webp 之间轮换。关闭伪装时又是 ts。
控制台只支持扩展名时，改用路径前缀 `/{gid}/{cid}/`，逐分组配置。

### 2.3 操作步骤

1. 打开统一面板，选中全部 4 个域名。
2. 加入**规则 A**。
3. 加入**规则 B**。强制 TTL，忽略源站头，忽略 Set-Cookie。
4. 在回源配置中加请求头 `X-Forwarded-Proto: https`。
5. 关闭图片处理、页面优化、智能压缩。
6. 保存，然后**刷新全部缓存**（Purge All）。
7. 按 §2.4 验证。四个域名各做一遍。

### 2.4 验收命令

```bash
# 1) 清单必须不缓存
curl -s -o /dev/null -D - "https://<域名>/<分组>/<变体>.m3u8" | grep -iE 'x-cache|cache-control'

# 2) 切片必须缓存。连续两次，第二次应为 HIT 且出现 Age
SEG="https://<域名>/<分组>/<变体>/<从清单复制的切片路径>"
for i in 1 2; do curl -s -o /dev/null -D - "$SEG" | grep -iE '^(x-cache|age|cache-control|content-length)'; done

# 3) 字节未被改写
curl -s "$SEG" | shasum -a 256

# 4) 面板与 API 不被缓存
curl -s -o /dev/null -D - "https://<域名>/panel/api/status" | grep -iE 'x-cache|cache-control'
```

期望结果：清单与面板返回 `BYPASS`。切片第二次返回 `HIT` 并带 `Age`。两次哈希一致。

### 2.5 常见失败对照

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 切片仍 `BYPASS` | 规则未生效 / 未刷新缓存 / Set-Cookie 未关 | 检查 §2.1 第 1、3 项 |
| `MISS` 不转 `HIT` | 缓存键包含 Cookie 或请求头，或 TTL 太短 | 缓存键去掉 Cookie。TTL 用推荐值 |
| `HIT` 但花屏或卡住 | 图片优化改写了字节 | 关闭图片处理。比对哈希 |
| 清单被缓存 | 规则顺序颠倒 | 把 A 调到最前 |
| 换台 404 | 302 被缓存 | 关闭 3xx 缓存 |
| 清单仍给 `http://` | 未下发 XFP | 加请求头，或配服务端 `public_base` |

---

## 3. 推荐值

### 3.1 为什么不能写死

片源参数不同：窗口 48–100s，单片 1.1–9 MB。
固定 TTL 会在窗口内过期，或长期多占边缘存储（24 小时可堆到几十 GB/频道）。
固定磁盘上限同理：按 4.26 MB/片配 150 MB 合理，按 1.1 MB/片浪费，按 9 MB/片不够。
所以 TTL 与磁盘上限必须来自**当前片源的参数**。

### 3.2 取数方式

默认用**被动实测**。不额外请求。清单解析得到 T/N/W，切片响应头得到
`Content-Length`，按分组滑动统计。代价是首次播放前没有数据。

首次安装、还没有播放时用回退值：**TTL 600s / 磁盘 150 MB**。
想装完立刻拿数字去配 CDN 时，点一次「立即估算」。它探测最多 3 个分组，
每组 1 个频道，只做一次。

### 3.3 公式

**TTL**

```
T      = #EXT-X-TARGETDURATION
N      = 清单中 #EXTINF 的条数
W      = Σ #EXTINF 的时长
seg_mb = 最近切片的 Content-Length / 1 MB（滑动平均）

TTL = clamp( ceil60(W + 2T), 300, 900 )
```

样本：T=8、N=7、W=56s → `ceil60(72) = 120` → clamp 后为 **300s**。

- `W + 2T` 是窗口加两片余量。结果低于 300s 时取 300s，覆盖重连与回退。
- 全局值取各分组的最大值。
- 「最佳」的定义：不产生过期重取前提下的最小值。更长的 TTL 不增加命中，只增加边缘存储。

**磁盘上限**

```
working = round10( 3 × N × seg_mb × 1.5 )   # 3 个并发频道的工作集，加 50% 余量
free    = statvfs(cache_dir).available      # 每次扫描重新取
budget  = free × 0.8 − 100 MB               # 占比 0.8，预留 100 MB
cap     = clamp( min(working, budget), 50 MB, total × 0.7 )
```

样本（1 GB 磁盘）：

| 场景 | free | budget | working | cap |
| --- | --- | --- | --- | --- |
| 空闲 800 MB | 800 | 540 | 134 | **140 MB**（受工作集限制） |
| 空闲 300 MB | 300 | 140 | 134 | **140 MB** |
| 空闲 120 MB | 120 | 负值 | 134 | **50 MB**（取下限），同时触发清理 |
| 9 Mbps × 12 片 × 3 频道 | 800 | 540 | 486 | **486 MB**（受工作集限制） |

- 不用固定 250 MB。1 GB 磁盘上 250 MB 可能超过剩余空间的一半。
- `total × 0.7` 只是防呆上限（1 GB 磁盘为 716 MB）。实际起作用的是工作集与可用空间。
- `working` 列是取整到 10 MB 后的值。`seg_mb` 与 `cap` 按十进制 MB（10⁶）计算。
- 每次 GC 扫描都重算 `cap`。
- `statvfs` 失败时回退 150 MB。
- 其它缓存目录（`sub_url`、EPG raw、DASH、timeshift/remux）由 `reserve_mb` 覆盖。

### 3.4 生效方式与开关

| 消费方 | 用法 |
| --- | --- |
| CDN 控制台 | 应用不能写第三方面板。缓存状态页显示推荐 TTL，附「复制」按钮。你复制后填入统一面板 |
| `[cdn].segment_max_age_secs = 0` | 运行时取实测值。填数则覆盖 |
| `[cache.gc].max_disk_mb = 0` | 运行时取 §3.3 的公式值。填数则覆盖 |

- 统计按分组存在内存，重启后清零并重新累积。同时落盘 `<cache>/cache_estimate.json`，重启后可延续。
- 更新节流：每 5 分钟最多刷新一次推荐值。GC 上限在下个扫描周期生效。
- 无数据时用回退值，面板标注「回退值」。

`[cdn].auto_tune = false` 关闭被动实测：

- 不再累计统计，不再周期写盘。只沿用上次落盘的值。没有则用回退值。
- 代价：片源变化后推荐值不跟随。需要点「立即估算」或手填。
- GC 不受影响。`max_disk_mb=0` 时用旧估算值或回退值。

---

## 4. 切片缓存的实际用途

直播播放器只取**当前窗口**内的新切片。缓存服务的是**重复请求**：重连、seek back、
多 CDN 多 POP 扇出、同频道多设备。回看与时移走 `timeshift` 路径，才复用过去的切片。

---

## 5. 多 CDN 注意点

1. **规则逐家一致**。按「强制 TTL + 忽略源站头 + 忽略 Cookie」这套最低要求配置。
   做不到的用服务端开关兜底（§6）。
2. **切片地址跟随 Host**。`self_base` 由请求的 Host 推导。每个 CDN 用自己的域名访问时，
   切片指向该域名，缓存不会串。
3. **边缘位置**。对大陆用户，SG/JP 通常优于 US/RU。建议 DNS 优先 SG/JP。
4. **多 POP 放大**。每个新切片在每个活跃 POP 各 Miss 一次。
   L1 落盘后，第 2 到第 N 个 POP 不再打源站。

---

## 6. 服务端兜底

### 6.1 `[cdn]`（默认关）

| 开关 | 作用 | 何时需要 |
| --- | --- | --- |
| `shared_cache_hits` | 命中路径的 `private` 改为 `public` | 某家只能用「遵循源站头」时 |
| `segment_max_age_secs` | 切片头 TTL。0 = 自动 | 同上 |
| `dont_cache_empty` | 空体加 `no-store` | CDN 跟随源站头时 |
| `empty_status = 502` | 上游失败时返回 502 | **强制 TTL 时必开**。`no-store` 会被忽略 |
| `public_base` | 固定的对外基址 | CDN 不下发 XFP 时 |

`empty_status` 只在取 4xx 或 5xx 时改状态码。填 `0` 或 2xx 时状态码仍为 200，
只有 `dont_cache_empty` 生效。

取舍：强制 TTL 会把「200 + 空体」也缓存一个 TTL 周期。用 502 让它不被缓存。
播放器收到 5xx 会重试。这比收到坏片 5 分钟更好。

### 6.2 磁盘回收（1 GB 磁盘的硬前提）

```toml
[cache.gc]
enabled = true
max_disk_mb = 0          # 0 = 按 §3.3 自动
max_age_secs = 1800
interval_secs = 300
scan_limit = 500         # 单次扫描上限，避免 1 vCPU 抖动
disk_budget_ratio = 0.8  # 最多占可用空间的 80%
reserve_mb = 100         # 给系统、日志、其它缓存目录预留
min_mb = 50              # 自动值下限
```

实现：按 mtime 删旧文件。顺序是超龄 → 超量 LRU → 磁盘见底时紧急清理。
紧急模式忽略 `max_age_secs`，删到 `cap / 2`。

**不用「按频道整组淘汰」**。`segment_path` 的文件名经过 `slug()`。标识超过 48 字符时
会被替换为 SHA-256，无法反解回 gid/cid。而直播切片的 mtime 本身就是热度信号，
LRU 等价且更简单。

磁盘索引 `seg_url/*.json` 同样要有上限。`[cache.segments].per_channel_max = 0` 时用内置
**4096** 条，大于 0 时按配置。超出后按 `Entry.at`（登记时间）丢最旧。旧格式条目没有 `at`
时按 0 处理，优先淘汰。

不要用文件名后缀判断新旧。该后缀是 `sha256(上游 URL)` 的前 32 位十六进制，是哈希不是序号。

---

## 7. 预取：只保留 L1

L1 的默认值不变：`enabled=true`、`concurrency=4`、`prefetch_limit=0`、`skip_tail=1`。

`limit=0` 的代价不高：已存在的文件会跳过。稳态下每次清单刷新只多抓 1–2 片。
只有开播与切台时抓满整个窗口（约 30–70 MB）。低配 profile 只把 `concurrency` 降到 2。

分组与频道可以覆盖：

```toml
[[groups]]
id = "hot"
[groups.prefetch]        # 缺省 = 继承全局
enabled = true           # 三态：未设置 / true / false
limit = 6
concurrency = 2
```

合并顺序：**频道 > 分组 > 全局**。实现与 `Config::proxy_for` 相同。

---

## 8. 配置与面板

缓存状态页只读，显示推荐值。系统设置页可编辑三段配置。字段旁显示「自动（实测 300s）」，
也可以改为手动值。

```toml
[cdn]
enabled = true
auto_tune = true              # false = 关闭被动实测
segment_max_age_secs = 0      # 0 = 自动推荐
shared_cache_hits = false     # 四家都能忽略源站头时不需要
dont_cache_empty = false      # 强制 TTL 下无效，用 502
empty_status = 502
# public_base = "https://<域名>"
```

| 配置 | 面板位置 | 接口 |
| --- | --- | --- |
| 推荐值展示（只读） | **缓存状态页**顶部摘要 | 扩展 `GET /panel/api/cache/stats` |
| `[cdn]` 编辑 | **系统设置页** →「CDN 缓存」卡片 | `GET/PUT /panel/api/config` |
| `[cache.gc]` 编辑 | **系统设置页** →「磁盘回收」卡片 | 同上 |
| `[cache.prefetch]` 编辑 | **系统设置页** →「预取」卡片 | 同上 |
| 分组与频道 `prefetch` | 分组 / 频道编辑对话框 | `PUT /groups/{gid}`、`POST` 或 `PUT /channels[/{cid}]` |
| 「立即估算」按钮 | 系统设置页的 CDN 卡片 | `POST /panel/api/cdn/estimate` |

`PUT /panel/api/config` 整份替换。前端必须先 GET，再改，再 PUT。

「立即估算」只运行一次。第二个并发请求返回 409，不排队等待。

---

## 9. 实施落点

| 模块 | 内容 |
| --- | --- |
| `src/config/model.rs` | `CdnCfg`、`CacheGcCfg`、`PrefetchOverrideCfg`；`Group` 与 `Channel` 的 `prefetch`；`prefetch_for(gid, cid)` 三级合并 |
| `src/cache_estimate.rs`（新） | 被动统计（清单 T/N/W、切片体积滑动平均）、公式、`cache_estimate.json`、「立即估算」 |
| `src/cache_gc.rs`（新） | `statvfs` 取空间、算上限、扫描与淘汰、紧急模式、推荐值输出 |
| `src/http/routes/stream.rs` | CDN 模式的响应头与空体状态码；`self_base` 优先 `public_base`；预取改用 `prefetch_for`；切片体积采样 |
| `src/http/routes/cdn.rs`（新） | `POST /panel/api/cdn/estimate` |
| `src/proxy/segments.rs` | 磁盘索引落盘上限（超量丢最旧） |
| `src/http/routes/admin.rs` | `cache/stats` 附带推荐值 |
| `frontend/src/{lowcfg,cache,panel,groups}.tsx` | 推荐卡片、三张表单、分组与频道的四态选择 |
| `tests/http_contract.rs` | 默认关闭时行为不变的回归测试 |

---

## 10. 实施与验收

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| S0 | §2 的五处改动 | **未做**（人工） |
| S1 | 配置层与面板（默认关） | 已完成。默认配置序列化不变 |
| S2 | `[cdn]` 兜底与 502 | 已完成 |
| S3 | 自动推荐（实测 / 公式 / 回退） | 已完成。失败回退 600/150 |
| S4 | 磁盘回收 | 已完成（单测）。待真机：造 200 MB 后回落到上限；`free < reserve` 时进入紧急清理；1 vCPU 不抖动 |
| S5 | 分组与频道预取覆盖 | 已完成 |
| S6 | 低配 profile | 待真机：64 MB 实例运行 24h，无 OOM，磁盘不涨 |

本地已跑：`cargo fmt --check`；`cargo clippy --all-targets` 无 error；
`cargo test` 共 **283 项**（201 单元 + 82 契约）；`pnpm build`；六个冒烟脚本。
`[cdn].enabled = false` 时，既有契约断言一行未改。

S4 与 S6 需要目标 VPS。本地只覆盖公式与淘汰逻辑：单测复现 §3.3 的四行取值，
并覆盖超龄、超量、紧急与 `scan_limit` 四类路径。

回滚：`[cdn].enabled = false`、`[cache.gc].enabled = false` 与分组 `prefetch.enabled = false`
在下一请求生效（配置热重载），不需要重启。

---

## 11. 风险

| 风险 | 缓解 |
| --- | --- |
| 某家 CDN 关不掉 Set-Cookie | 只把它当路由加速，其余三家负责缓存 |
| 强制 TTL 缓存空片 | `empty_status = 502` |
| 图片优化改写切片 | 关闭图片处理，并用哈希验收 |
| 采样期间源站不可用 | 回退 600/150，面板标注，可手动重采 |
| 参数随片源漂移 | 7 天自动重采，或手动重采 |
| 磁盘无回收被占满 | §6.2 的 GC |
| 清单给 `http://` | 下发 XFP，或配 `public_base` |
| 规则误伤清单 | 把 A 规则排在 B 规则前 |
