# 低配与高配环境的运行参数差异

> 写作风格：ASD-STE100 简化技术写作。一句一个意思。主动语态。用简单现在时。

## 1. 结论

低配示例 `config-low-spec.toml` 与默认值的**真实差异只有 4 项**。

其余键都显式写出默认值。作用是文档，不是调优。

| 键 | 默认（高配） | 低配示例 | 性质 |
| --- | --- | --- | --- |
| `cache.segments.per_channel_max` | `0`（内置 4096） | `200` | 内存 |
| `cache.prefetch.concurrency` | `4` | `2` | CPU |
| `auth.token_enabled` | `false` | `true` | 安全 |
| `auth.ip_guard_enabled` | `false` | `true` | 安全 |

`[cdn].enabled` 在低配示例里是 `true`，默认是 `false`。这是**打开功能**，不是减负。

`[server].low_resource_mode` 低配示例没有设置。它只出现在 `/panel/api/status` 的回显里，
不改变任何行为（复刻目标行为）。不要把它当调优开关。

## 2. 按资源类型看参数

### 2.1 内存

| 键 | 默认 | 作用 |
| --- | --- | --- |
| `cache.segments.per_channel_max` | `0` = 内置 4096 | 每个频道的切片索引条数上限。低配设 200 |
| `cache.segments.global_max` | `0` = 不限 | 全局索引条数上限 |
| `cache.memory_max_mb` | `0` = 不限 | `backend = "memory"` 时的切片正文字典预算 |
| `cache.memory_max_mb`（磁盘后端） | — | 索引字节预算，每条约 256 B |

低配示例把 `per_channel_max` 压到 200。按 40 个频道算，索引约 40 × 200 × 256 B ≈ 2 MB。

### 2.2 CPU

| 键 | 默认 | 低配 | 作用 |
| --- | --- | --- | --- |
| `cache.prefetch.concurrency` | `4` | `2` | 预取并发。开播瞬间的 CPU 峰值由它决定 |
| `cache.gc.scan_limit` | `500` | `500` | 单轮 GC 最多处理的文件数 |
| `cache.dash.fetch_concurrency` | `20` | 未调 | DASH 切片并发 |
| `health.concurrency` | `3` | 未调 | 健康探测并发 |

1 vCPU 上真正要压的是 `prefetch.concurrency`。DASH 的 `fetch_concurrency = 20`
只影响 DASH 组，低配环境通常不开 DASH。

### 2.3 磁盘

| 键 | 默认 | 低配 | 作用 |
| --- | --- | --- | --- |
| `cache.gc.enabled` | `true` | `true` | 切片回收总开关。低配必须开 |
| `cache.gc.max_disk_mb` | `0` = 自动 | `0` | `0` 时按「可用空间 × 0.8 − 100 MB」与工作集取小 |
| `cache.gc.max_age_secs` | `1800` | `1800` | 切片最长存活 |
| `cache.gc.interval_secs` | `300` | `300` | 回收周期 |
| `cache.gc.reserve_mb` | `100` | `100` | 给系统留的余量 |
| `cache.gc.min_mb` | `50` | `50` | 上限的下限。再小也留 50 MB |

`[cache]` 的 3 个兼容键（`segment_cache_max_mb`、`m3u8_max_entries`、`segment_max_entries`）
目标与本实现都不消费。不要用它们限容。

### 2.4 网络与上游

| 键 | 默认 | 低配示例 | 作用 |
| --- | --- | --- | --- |
| `cdn.enabled` | `false` | `true` | 外部 CDN 侧兜底 |
| `cdn.auto_tune` | `true` | `true` | 被动实测 TTL。有少量后台开销 |
| `health.enabled` | `true` | 未设 | 周期探测所有频道上游 |
| `health.interval_secs` | `300` | 未设 | 探测周期 |

低配环境的上游请求预算很紧。健康探测每 300 秒对所有频道打一次上游，
频道多时这是可观的额外请求。

### 2.5 后台任务（功能级）

这些不靠参数调优，靠**关掉整个功能**才省得下来。

| 功能 | 开销 | 是否有开关 |
| --- | --- | --- |
| 定时录制 | ffmpeg 子进程，持续转封装，磁盘写入 | 无（只能不创建任务） |
| 回看缓冲（`livestream`） | 每个开机频道持续抓取切片并落盘 | 无（只能不点亮） |
| DASH 时移（`dash_timeshift`） | 逐轨道抓切片，磁盘与网络双高 | 无 |
| 台标动效 | 一次性 CPU 峰值 | 无 |
| EPG 采集 | 周期拉取和解析 XMLTV | `epg.enabled` |
| EPG 翻译 | 批量 HTTP 调用 | `epg.translate.enabled` |
| 字幕翻译 | 按需 HTTP 调用 | `subtitle.translate.enabled` |
| 多 VPS 分享 | 周期轮询对端 | `share.enabled` |

## 3. 对功能开关的启示

低配环境最需要能关掉的是 **§2.5 里没有开关的三项**：定时录制、回看缓冲、DASH 时移。

它们的特点是：

1. 一旦启用就持续占用 CPU 与磁盘。
2. 没有全局开关，只能逐频道设置。
3. 面板上没有统一的入口。

次要项是健康探测。它已经有 `health.enabled`，缺的是面板上的入口。

反过来，**切片缓存、预取、GC 这些不该做成开关键**。它们是播放链路的一部分。
关掉预取只会让每次重连都回源，反而更慢。调 `concurrency` 就够了。

## 4. 建议的面板开关清单

| 开关 | 控制的配置 | 为什么值得放面板 |
| --- | --- | --- |
| 定时录制 | `[features].recording` | ffmpeg 是最大开销 |
| 回看缓冲 | `[features].livestream` | 持续录制，磁盘杀手 |
| 频道探测 | `[health].enabled` | 已有配置 |
| 节目指南 | `[epg].enabled` | 已有配置 |
| 分享 | `[share].enabled` | 已有配置 |

前两项是 2026-10-31 新增的键。后三项原本就有。

统一语义：关闭后分区入口消失，接口返回 `403`。
与部署规格无关 —— 高配与低配行为一致，不做「低配才拦截」的分支。

## 5. 手动覆盖自适应参数

系统会按 §2 的公式自动算三个量。其中两个可以手动覆盖。

低配机器在**安装时**写入低配参数：

```sh
sudo ./install.sh --low-spec
```

它写入 §1 的 5 个差异键。也可以手工一条条改：

```sh
tv-buddy config set cache.segments.per_channel_max 200
tv-buddy config set cache.prefetch.concurrency 2
```

配置存在 `tv-buddy.db`。改完需要重启服务，或调 `POST /panel/api/reload`。

### 5.1 磁盘上限（可直接覆盖）

面板路径：**系统设置 → 切片缓存 → 磁盘上限**。配置键 `cache.gc.max_disk_mb`。

```toml
[cache.gc]
max_disk_mb = 0      # 0 = 走公式（默认）
max_disk_mb = 120    # 非 0 = 直接用这个值，公式不参与
```

代码位置：`src/cache_gc.rs` 的 `effective_cap_mb`。第一行就是
`if explicit_mb != 0 { return explicit_mb; }`。

公式的三个参数也能在面板改：

| 面板字段 | 配置键 | 默认 | 作用 |
| --- | --- | --- | --- |
| 预算比例 | `cache.gc.disk_budget_ratio` | `0.8` | 可用空间的乘数 |
| 磁盘预留 | `cache.gc.reserve_mb` | `100` | 给系统留的余量（MB） |
| 最小上限 | `cache.gc.min_mb` | `50` | 上限的下限（MB） |

生效时机：保存后下一轮 GC 生效。GC 周期由 `cache.gc.interval_secs` 决定，默认 300 秒。

### 5.2 工作集（不能直接改）

工作集是实测值，不是配置项。面板没有输入框。

公式：`round10(3 × N × 切片体积MB × 1.5)`。`N` 来自最近一次实测的窗口切片数。

想让它变小，只能间接影响：

- 减少同时开播的频道数。
- 缩短 DASH 清单窗口（`cache.dash.playlist_min_secs`）。
- 降低上游码率。

### 5.3 TTL 推荐值（只读，可用固定值替代）

「缓存状态」页显示实测推荐的切片 TTL。那个值给 CDN 控制台用，不是本机配置。

本机想固定 TTL，改 **系统设置 → CDN 兜底 → 强制 TTL 秒**（`cdn.segment_max_age_secs`）：

| 值 | 行为 |
| --- | --- |
| `0` | 用实测推荐值 |
| `300` | 固定 300 秒 |

### 5.4 其它可覆盖项

| 量 | 面板位置 | 配置键 |
| --- | --- | --- |
| 每频道索引条数 | 系统设置 → 切片缓存 | `cache.segments.per_channel_max` |
| 全局索引条数 | 同上 | `cache.segments.global_max` |
| 预取并发 | 系统设置 → 预载配置 | `cache.prefetch.concurrency` |
| GC 单轮扫描上限 | 系统设置 → 切片缓存 | `cache.gc.scan_limit` |
| 切片最长存活 | 同上 | `cache.gc.max_age_secs` |

### 5.5 常见操作

低配机器强制压到 80 MB：

```
系统设置 → 切片缓存 → 磁盘上限 = 80 → 保存
```

查看公式当前算出多少、工作集多大：**缓存状态**页的「CDN 推荐值」卡片。只读。

### 5.6 总表

| 自适应量 | 能否覆盖 | 入口 |
| --- | --- | --- |
| 磁盘上限 | 是，直接覆盖 | 系统设置 → 切片缓存 → 磁盘上限 |
| 公式参数 | 是 | 同一张卡 |
| 工作集 | 否 | 无入口，只能间接影响 |
| 实测 TTL | 否，可用固定值替代 | 系统设置 → CDN 兜底 → 强制 TTL |
| 每频道索引条数 | 是 | 系统设置 → 切片缓存 |
| 预取并发 | 是 | 系统设置 → 预载配置 |
