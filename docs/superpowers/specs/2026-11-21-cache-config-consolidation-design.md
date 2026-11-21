# 缓存配置与磁盘回收收敛设计

- 日期：2026-11-21
- 状态：已批准（2026-11-21）
- 相关文档：`docs/design/04-config-and-storage.md`、`docs/design/06-low-spec-optimization.md`、`docs/design/18-low-spec-deployment.md`

## 1. 意图

缓存设置页现在有 6 张卡、约 30 个字段。问题不是字段多，而是同一件事散在多处：

- 6 个字段不生效，但界面可填。
- 2 组同义键。
- GC 的 5 个内部参数全部外露。
- 自动估算开关放在 CDN 分区，却同时影响磁盘上限。
- 自动上限只显示数字，不显示来源和限制因素。

目标是「一个概念一个键」：

- 用户只看到需要的字段。
- 默认自动；需要时才手动。
- 自动值说明依据。
- 旧键直接移除，模型里只保留新键。
- 索引预算由缓存上限推导；配置里不出现固定总量。
- 只保留磁盘缓存。切片正文太大，不适合放内存。

成功标准：

1. 缓存区主界面不超过 2 张卡，约 10 个字段。
2. 配置模型没有同义键，也没有不生效键。
3. 自动上限显示「受什么限制」。
4. 配置模型只保留新键；旧键不读取，不迁移。
5. 索引总量来自磁盘上限，没有独立的固定总量。
6. 只有一种缓存后端：磁盘。
7. 现有功能不回退。

非目标：

- 不引入配置档位（profile）。
- 不增加分组级或频道级缓存上限。
- 不改变预取、Remux、DASH 的行为。
- 不新增环境变量。
- 不改变 CDN 面板的人工配置流程。
- 内存清理审计（DASH 索引、TTL 缓存、观众队列、订阅缓存、EPG）另起后续任务，见 §16。

反面目标：

- 只把分裂藏进折叠区，不在模型层收敛。
- 同时保留「档位」和「自定义字段」两套表达。
- 不做说明就改变默认行为。

## 2. 现状与冲突

| 问题 | 位置 |
| --- | --- |
| 磁盘上限有两个同义键，取较小值 | `cache.disk_max_mb`、`cache.gc.max_disk_mb` |
| 切片 TTL 按 CDN 开关分成两个键 | `cache.ttl.segment_secs`、`cdn.segment_max_age_secs` |
| 索引上限有两套，且其中一套是固定条目总数 | `cache.segments.global_max`、`cache.segment_max_entries`（后者不消费） |
| 两套后端语义 | `cache.backend` 在磁盘/内存之间切换，配置与回收分支翻倍；单频道切片窗口可达数十 MB，正文不适合放内存 |
| 6 个不生效键 | `segment_cache_max_mb`、`m3u8_max_entries`、`segment_max_entries`、`ttl.refresh_ratio`、`ttl.fallback_mpd_secs`、`segments.retain_count` |
| GC 内部参数外露 | `interval_secs`、`scan_limit`、`disk_budget_ratio`、`reserve_mb`、`min_mb` |
| 自动估算开关放错分区 | `cdn.auto_tune` |
| 自动值缺来源 | `/cache/stats` 只返回结果与粗粒度 `source` |

冲突：`docs/design/04-config-and-storage.md` §11.5 有意保留兼容键，目标是「配了没反应」时仍能导入。

用户决定采用 **方案 B：直接移除旧键**，且项目未正式使用，不做旧配置迁移。模型只保留新键；旧键不读取。

## 3. 方案比较

### 方案 1：极简模型（采纳）

- 一个概念一个键。
- GC 内部参数改为代码常量，单轮删除数按上限推导。
- 并发频道数改为实测活跃频道数，最低 3。
- 索引预算由磁盘上限推导，不设固定总量。
- 只保留磁盘后端；切片正文不进入进程内存。
- 进程内只保留活跃索引，按 600 秒活动窗口淘汰。
- 旧键直接移除，不迁移。

### 方案 2：极简 + 高级折叠（未采纳）

可见部分同方案 1，但保留 GC 的 5 个内部参数，放在 `cache.gc.advanced`。

不采纳原因：模型仍多 5 个键，只是把分裂藏起来。用户目标是模型收敛，不是界面收敛。

### 方案 3：档位预设（未采纳）

`cache.profile = auto/low/large/custom`。档位和自定义字段同时存在，是第二套表达。不做。

## 4. 新配置模型

### 4.1 JSON 结构

```json
{
  "cache": {
    "disk_path": "./cache",
    "limit_mb": 0,
    "auto_tune": true,
    "ttl": {
      "sub_url_secs": 1800,
      "segment_secs": 0
    },
    "segments": {
      "per_channel_max": 0
    },
    "gc": {
      "max_age_secs": 1800
    },
    "prefetch": { },
    "dash": { },
    "remux": { },
    "disguise": { }
  },
  "cdn": {
    "enabled": false,
    "shared_cache_hits": false,
    "dont_cache_empty": false,
    "empty_status": 502,
    "public_base": null
  }
}
```

### 4.2 字段

| 路径 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `cache.disk_path` | string | `"./cache"` | 缓存根目录；改后需重启 |
| `cache.limit_mb` | u64 | `0` | 磁盘缓存总上限，包含正文与索引；`0` = 自动公式 |
| `cache.auto_tune` | bool | `true` | 被动采样开关；关闭后沿用上次实测，仍按当前磁盘空间计算上限 |
| `cache.ttl.sub_url_secs` | u64 | `1800` | 订阅地址缓存有效期 |
| `cache.ttl.segment_secs` | u64 | `0` | 切片缓存有效期；`0` = 自动（实测值，无数据用 600） |
| `cache.segments.per_channel_max` | u64 | `0` | 单频道索引条目上限；`0` = 4096 |
| `cache.gc.max_age_secs` | u64 | `1800` | 过期删除；`0` = 不按时间删除 |
| `cdn.enabled` | bool | `false` | CDN 响应头兜底开关；不再控制 TTL 取值 |

`cache.prefetch`、`cache.dash`、`cache.remux`、`cache.disguise` 保持不变。

### 4.3 删除的键

项目未正式使用，不做迁移。以下键不再读取。保存时从 `extra` 删除，避免回写。

| 删除的键 | 替代 |
| --- | --- |
| `cache.disk_max_mb` | `cache.limit_mb` |
| `cache.gc.max_disk_mb` | `cache.limit_mb` |
| `cache.memory_max_mb` | 无；内存后端已删除 |
| `cache.backend` | 无；只保留磁盘后端 |
| `cache.segments.global_max` | 无；索引预算由 `cache.limit_mb` 与活动时间推导 |
| `cdn.segment_max_age_secs` | `cache.ttl.segment_secs` |
| `cdn.auto_tune` | `cache.auto_tune` |
| `cache.gc.enabled` | 无；磁盘回收常开 |
| `cache.gc.interval_secs` | 无；固定 60 秒 |
| `cache.gc.scan_limit` | 无；按上限推导 |
| `cache.gc.disk_budget_ratio` | 无；固定 0.8 |
| `cache.gc.reserve_mb` | 无；固定 100 MB |
| `cache.gc.min_mb` | 无；固定 50 MB |
| `cache.segment_cache_max_mb` | 无；从未消费 |
| `cache.m3u8_max_entries` | 无；从未消费 |
| `cache.segment_max_entries` | 无；从未消费 |
| `cache.ttl.refresh_ratio` | 无；从未消费 |
| `cache.ttl.fallback_mpd_secs` | 无；从未消费 |
| `cache.segments.retain_count` | 无；从未消费 |

## 5. 容量与回收

### 5.1 常量

| 名称 | 值 | 说明 |
| --- | --- | --- |
| `TOTAL_RATIO` | 0.7 | 单个缓存最多占文件系统 70% |
| `DISK_BUDGET_RATIO` | 0.8 | 自动上限最多占可用空间 80% |
| `RESERVE_MB` | 100 | 给系统、日志、其它缓存预留 |
| `MIN_CAP_MB` | 50 | 自动上限的下限 |
| `GC_INTERVAL_SECS` | 60 | 扫描周期；限制两次扫描之间的磁盘峰值 |
| `GC_SCAN_FLOOR` | 500 | 单轮最少删除文件数 |
| `GC_SCAN_MAX` | 5000 | 单轮最多删除文件数 |
| `SEGMENT_EST_MB` | 4 | 单轮删除数推导用的平均切片大小 |
| `MIN_CONCURRENT_CHANNELS` | 3 | 工作集公式的并发频道下限 |
| `INDEX_RATIO` | 0.10 | 磁盘索引占缓存上限的比例 |
| `INDEX_TTL_SECS` | 600 | 进程内索引的活动窗口；超时条目不再保留 |

`scan_limit = clamp(cap_mb / SEGMENT_EST_MB, GC_SCAN_FLOOR, GC_SCAN_MAX)`。

### 5.2 并发频道数

工作集不再固定按 3 个频道计算。

- `SegmentIndex` 记录每个频道最近一次活动时间。
- 活跃频道数（`channels`）= 最近一个在线窗口（`[stats].online_window_secs`，默认 90 秒）内活动过的频道数。
- 参与计算的频道数（`working_channels`）= `max(MIN_CONCURRENT_CHANNELS, channels)`。
- 这个信号不依赖播放 Token，也不依赖 CDN。
- 记录点有两处：`media_entry`（清单）与 `serve_rest`（切片分发）。proxy、remux、dash 三种模式都覆盖。

工作集：

```
working_mb = round10(working_channels × N × seg_mb × 1.5)
```

`N` 与 `seg_mb` 仍取各分组的最大值。多分组时偏保守。

### 5.3 自动上限

```
working    = 实测工作集（MB）；无实测时为回退值 150 MB
budget_mb  = max(0, free_mb × 0.8 − 100)
ceiling_mb = max(total_mb × 0.7, 50)
target_mb  = clamp(min(working, budget_mb), 50, ceiling_mb)
```

回退值 150 MB 只是工作集的替身，不绕过公式夹取。无实测且 `budget_mb` 或 `ceiling_mb` 更小时，`limited_by` 报告真实限制因素；只有回退值本身是限制时才标 `no_data`。

`statvfs` 失败时：

- 上限 = `max(MIN_CAP_MB, 实测工作集)`；无工作集用 150 MB。
- 不进入紧急模式：读不到可用空间时不做激进删除。
- 不再出现「页面显示工作集、回收按 150 MB」的不一致。
- `limited_by = space_unknown`。

`cache.auto_tune = false` 只停止新采样，不冻结上限：

- 沿用上次实测值；无实测时用 150 MB。
- 仍按当前 `statvfs` 计算 `budget_mb` 与 `ceiling_mb`。
- 页面显示「沿用上次估算」，不写成「停在关闭时的值」。

### 5.4 限制来源

`recommend` 与回收计划共用同一个结果：

| `limited_by` | 条件 |
| --- | --- |
| `manual` | `cache.limit_mb` 非 0 |
| `ceiling` | `min(working, budget) > ceiling` |
| `floor` | `min(working, budget) < 50` |
| `budget` | `budget < working`，且未触底、未触顶 |
| `no_data` | `statvfs` 正常，没有实测工作集，且回退值 150 MB 未触及其他限制 |
| `working` | 有实测工作集，且它是限制因素 |
| `space_unknown` | `statvfs` 失败 |

`no_data` 的 `working` 取回退值 150 MB，再参与同一公式。优先级：

1. `cache.limit_mb` 非 0 时，始终 `manual`。
2. `statvfs` 失败时，`space_unknown`。
3. `min(working, budget) > ceiling` 时，`ceiling`。
4. `min(working, budget) < 50` 时，`floor`；提示文案同时说明原始原因是工作集还是可用空间。
5. `budget < working` 时，`budget`。
6. 没有实测工作集时，`no_data`。
7. 其余为 `working`。

默认 `cache.limit_mb = 0`，即自动计算总磁盘上限；手动指定非 0 值时以手动值为准。

回收计划使用同一个 `cap_mb` 和 `emergency`：

- 容量判断的占用值 = `seg_bytes` + `seg_url` 的实际字节，与页面「已用」一致。
- `emergency = free_mb < RESERVE_MB`。
- 紧急模式下忽略过期门槛，删到 `cap_mb / 2`。
- `cache.gc.max_age_secs = 0` 时跳过过期删除，只按容量和紧急模式回收。

### 5.5 单一磁盘后端

- 只保留磁盘后端。切片正文写入 `seg_bytes`，不进入进程内存。
- 删除 `cache.backend`、`cache.memory_max_mb` 与全部内存缓存预算。
- 理由：单个频道窗口可达数十 MB；放进内存会挤压运行时、EPG 与其它缓存。
- 进程内只保留 `SegmentIndex` 元数据，按 `INDEX_TTL_SECS` 淘汰。
- 磁盘索引 `seg_url` 与正文共用 `cache.limit_mb`，索引子预算 10%。

### 5.6 索引预算

**磁盘索引**

- 磁盘索引预算 = `cache.limit_mb × INDEX_RATIO`（10%）；正文至少保留 90%。
- 磁盘索引 `seg_url` 与正文 `seg_bytes` 共用 `cache.limit_mb`。回收先检查索引子预算，再检查总量。
- 没有独立的索引配置项。

**进程内索引**

- 不属于磁盘预算。
- 按 `INDEX_TTL_SECS` 淘汰超过 600 秒没有刷新的条目。
- `per_channel_max` 仍是单频道条目上限。
- 不设总条目配置。

**为什么不用固定总量**

- 固定 64 MB 或 5 万条与缓存上限无关。上限改大或改小时，索引预算不会跟着变。
- 按上限比例推导后，小盘和大盘各自得到合适的索引大小。

### 5.7 实测依据

- 服务空闲常驻内存：release 构建、空配置，约 28 MB（macOS 实测；Linux 会有差异）。
- 进程内索引：150 字符 URL、32 字符键，10 万条约 56 MB，约 558 B/条；按 600 秒活动窗口淘汰。
- 磁盘索引 JSON：150 字符 URL，4096 条约 901 KB；450 条约 99 KB；7 条约 1.5 KB。
- 10% 子预算在 140 MB 磁盘上限下约 14 MB，可放约 15 个满历史频道，或约 140 个只播过 1 小时的频道。
- 扫描周期 60 秒的峰值：3 个频道约 100 MB，10 个频道约 335 MB。原来 300 秒分别是约 500 MB 和约 1.7 GB，足以在 1 GB 或 2 GB 盘上顶满。

## 6. 旧配置策略

项目未正式使用，不做迁移。

- 旧键不再读取。旧配置按新默认值运行：`cache.limit_mb = 0`、`cache.ttl.segment_secs = 0`、`cache.gc.max_age_secs = 1800`。旧 `cache.segments.global_max` 不再读取，索引预算由上限推导。旧 `cache.backend` 不再读取，统一使用磁盘后端。
- 保存时从各层 `extra` 删除已知旧键，避免旧键回写数据库。
- 删除清单见 4.3。
- 不提供 CLI `config set` 旧路径的兼容。文档和界面只使用新路径。
- 不做回滚兼容。升级前备份数据库。

## 7. 接口

### 7.1 `GET /panel/api/cache/stats`

`recommend` 块调整：

```json
{
  "ttl_secs": 300,
  "ttl_source": "manual",
  "disk_cap_mb": 140,
  "cap_source": "auto",
  "limited_by": "working",
  "channels": 2,
  "working_channels": 3,
  "working_mb": 140,
  "budget_mb": 279000,
  "ceiling_mb": 260000,
  "gc_last_run_secs_ago": 120,
  "groups": 2,
  "updated_secs_ago": 30,
  "disk_total_mb": 372330,
  "disk_free_mb": 349043,
  "segment_body_bytes": 130000000,
  "segment_index_bytes": 2000000,
  "segment_index_limit_bytes": 14000000,
  "segment_cache_bytes": 132000000,
  "segment_body_files": 35,
  "auto_tune": true
}
```

- 删除：`source`、`ttl_manual`、`disk_manual`、`gc_enabled`、`seg_bytes`；`backend` 也从统计中去掉，统一磁盘后端。
- 新增：`ttl_source`、`cap_source`、`limited_by`、`channels`、`working_channels`、`budget_mb`、`ceiling_mb`、`gc_last_run_secs_ago`、`segment_body_bytes`、`segment_index_bytes`、`segment_index_limit_bytes`、`segment_cache_bytes`；`seg_files` 改名为 `segment_body_files`，`seg_bytes` 改名为 `segment_body_bytes`。
- `ttl_source`：`manual` / `measured` / `fallback`。
- `cap_source`：`manual` / `auto`。
- `limited_by`：`manual` / `working` / `budget` / `ceiling` / `floor` / `no_data` / `space_unknown`。
- `channels`：在线窗口内的活跃频道数；空闲时可以为 0。
- `working_channels`：参与上限计算的频道数，等于 `max(3, channels)`；前端用它说明「按几个频道估算」。

`POST /panel/api/cdn/estimate` 复用同一 `recommend` 块，字段同步。

### 7.2 配置路径

新增：

- `cache.limit_mb`
- `cache.auto_tune`

删除 4.3 表中的全部路径。

## 8. 前端

### 8.1 设置页布局

缓存区改为 2 张主卡 + 1 个高级折叠：

```
缓存容量与回收
  缓存目录 [path]
  缓存上限 [自动 / 手动] [140] MB
  自动估算 [开启 / 关闭]
  过期删除 [开启 / 关闭] [1800] 秒
  已用 132 MB / 上限 140 MB · 索引 2 MB / 14 MB · 上次回收 2 分钟前
  [清空本地缓存]

缓存有效期
  订阅地址有效期 [1800] 秒
  切片有效期 [自动 / 手动] [300] 秒

高级设置 · 预载、Remux、单频道索引
  单频道索引条目上限
  预载配置
  Remux
```

### 8.2 自动与手动

新增 `AutoNum` 和 `ToggleNum` 组件：

- `AutoNum`：`Select` 选择「自动 / 手动」，后接数字输入。
  - 自动：输入框禁用，显示当前自动值。
  - 手动：输入框启用，`min = 1`，避免输入 0 又变回自动。
  - 切到手动时，用当前自动值预填；无自动值时用回退值。
- `ToggleNum`：开关控制是否启用，启用时显示数字输入。
  - 过期删除：关闭 = `max_age_secs = 0`；开启默认 1800，最小 60。
- 单频道索引上限用 `AutoNum`：自动 = 4096，手动可改。没有全局索引总量输入框。

所有「0 = 特殊含义」在界面上变成显式控件。0 只留在配置层。

### 8.3 自动值提示

自动上限下方显示：

```
自动 140 MB · 受播放需要限制
```

提示内容：

```
三个值取最小：播放需要约 140 MB；
磁盘可用空间允许约 279 GB；
总容量 70% 约 260 GB。
```

手动上限低于工作集时：

```
低于播放需要 140 MB，换台可能回源变慢。
```

手动磁盘上限超过磁盘总量时：

```
超过磁盘总量，容量上限不会触发，只会按过期规则删除。
```

自动估算关闭时：

```
已关闭自动估算，沿用上次实测值；上限仍按当前磁盘空间计算。
```

无历史数据时：

```
无历史估算，按回退值 150 MB 计算。
```

读不到磁盘空间时：

```
无法读取磁盘空间，按播放需要 140 MB；无实测时用 150 MB。
```

### 8.4 单一磁盘后端

- 设置页不显示缓存位置选择。
- 状态行显示磁盘占用与索引子预算。
- 缓存状态页同样只显示磁盘。

### 8.5 缓存状态页

两个概念分开：

- **CDN 推荐值**：只显示 CDN 侧切片 TTL、来源、最近采样与「复制缓存时长」。
- **本机缓存**：显示本机上限、限制因素、活跃频道、预计需要、实际占用、上次回收。
- 「立即估算」放在页头，或作为两张卡共用的动作。一次估算同时更新 TTL 和本机上限。
- `cdn.enabled = false` 时，CDN 推荐值卡片显示「CDN 兜底未启用」，推荐值仍可复制。

字段改名：

| 现在 | 改为 |
| --- | --- |
| 硬盘上限 / 占用上限 | 本机缓存上限 |
| 实测占用 | 预计需要 |
| 切片占用 | 实际占用 |
| 被动实测 / 自动推荐 | 自动估算 |
| 超龄删除 | 过期删除 |

「CDN 推荐值」这个名字保留，不再改名。

新增显示：

- 缓存上限来源：自动 / 手动。
- 限制因素：播放需要 / 可用空间 / 总容量 70% / 下限 / 无数据 / 空间读取失败。
- 活跃频道数，以及参与计算的频道数：例如「活跃 0 · 按 3 个估算」。
- 上次回收时间。
- 自动估算开关与数据年龄。
- 实际占用 = 切片正文 + 切片索引，取 `segment_cache_bytes`；正文取 `segment_body_bytes`（原 `seg_bytes`）。
- 本机缓存显示正文加索引合计，以及索引已用 / 子预算（`segment_index_limit_bytes`）。

保留：

- 「立即估算」按钮。
- 「复制缓存时长」按钮。
- 清空缓存的确认、忙碌状态与结果提示。

### 8.6 可访问性与反馈

- 自动 / 手动使用现有 `Select`，带 `aria-label="{字段}取值方式"`。
- 禁用输入降低透明度并显示 `cursor: not-allowed`，不能只靠颜色区分。
- 提示文本用 `aria-describedby` 关联输入。
- 高级折叠沿用 `<details className="block">`，summary 写明内容与条目数。
- 移动端 summary 最小高度 44px。
- 保存与清空按钮在忙碌时禁用并显示进度文本；结果使用 `role="status"`，错误使用 `role="alert"`。
- 数字输入保持 `type="number"`，移动端使用数字键盘。

## 9. 后端改动

| 文件 | 改动 |
| --- | --- |
| `src/config/model.rs` | 删除旧字段与 `segments.global_max`；新增 `limit_mb`、`auto_tune`；调整默认值；`normalize` 清理旧键 |
| `src/config.rs` | 校验 `disk_path`、`max_age_secs`（0 或 ≥60） |
| `src/config/store.rs` | 无需改逻辑；补默认值与旧键清理测试 |
| `src/cache_estimate.rs` | `working_mb(channels)`；删除固定并发常量；补 TTL 解析辅助函数 |
| `src/cache_memory.rs` | 删除文件；内存后端不再保留 |
| `src/cache_gc.rs` | `CapPlan` 与 `limited_by`；GC 常量；扫描数推导；活跃频道；`max_age_secs = 0`；`gc_last_run`；统一 `statvfs` 回退；磁盘索引 10% 子预算 |
| `src/proxy/segments.rs` | 记录频道最近活动时间；`note_channel`；`active_channels(window)`；索引文件字节统计与按预算淘汰；`INDEX_TTL` 淘汰；删除 `global_limit`、`memory_budget_entries`、`ENTRY_BYTES_EST`；`clear` 同步清理 |
| `src/state.rs` | 新配置路径；`auto_tune` 同步；`gc_last_run_epoch` 原子字段；删除内存缓存初始化；自动磁盘上限 |
| `src/http/routes/stream.rs` | 切片 TTL 统一解析；`media_entry` 与 `serve_rest` 记录活跃频道；CDN 只控制响应头范围 |
| `src/dash_hls.rs` | 使用同一 TTL 解析；切片分发不额外记录（由 `serve_rest` 覆盖） |
| `src/http/routes/admin.rs` | 缓存统计增加 `segment_body_bytes`、`segment_index_bytes`、`segment_cache_bytes`；推荐块新字段 |
| `src/http/routes/cdn.rs` | 复用推荐块，无需额外逻辑 |
| `src/backup.rs` | 无需改逻辑；旧键清理在 `normalize` |
| `docs/design/04-config-and-storage.md` | 更新缓存字段表与 §11.5 |
| `docs/design/06-low-spec-optimization.md` | 更新 §3.3、§3.4、§6.2、§8 的配置示例 |
| `docs/design/18-low-spec-deployment.md` | 更新命令与字段名 |
| `docs/design/config-low-spec.json` | 更新示例配置 |
| `docs/tests/05-low-spec-acceptance.md` | `seg_bytes` 改读 `segment_body_bytes`；补活跃频道与限制来源断言 |

## 10. 测试

| 层 | 用例 |
| --- | --- |
| 配置模型 | 新默认值；旧键不读取；保存时从 `extra` 删除旧键 |
| 配置模型 | 新配置的 0 语义：`limit_mb` 自动、`segment_secs` 自动、`max_age_secs` 关闭 |
| 配置校验 | `disk_path` 为空；`max_age` 在 1–59 之间拒绝 |
| 估算 | `working_mb(3)`、`working_mb(6)`；无窗口返回 None |
| 回收 | `limited_by` 七种来源；手动优先；无数据且未触及其他限制时 `no_data`；`statvfs` 失败 `space_unknown` |
| 回收 | 活跃频道改变上限；扫描数推导；`max_age=0` 不按时间删 |
| 回收 | 回退值参与夹取：无实测且可用空间低时标 `floor` 或 `budget`；`statvfs` 失败且工作集 30 MB 时上限 50 MB |
| 回收 | `auto_tune=false` 停止采样，但上限仍随当前可用空间变化 |
| 回收 | 磁盘索引子预算 = 总上限 10%；超子预算先删最旧索引文件 |
| 索引 | 磁盘索引目录超过子预算时按文件 mtime 淘汰最旧；`per_channel_max` 仍生效 |
| 索引 | 进程内索引超过 600 秒未刷新即淘汰；磁盘索引回退仍可用 |
| 索引 | 活跃频道计数；窗口过期；`clear` 后为 0 |
| 接口 | `/cache/stats` 推荐块与占用字段；`channels` 与 `working_channels` 分开；`/cdn/estimate` 同步 |
| 契约 | 默认配置切片 TTL 变为自动；手动 `cache.ttl.segment_secs` 在 CDN 开/关下一致；`seg_bytes` 不再返回 |
| 前端 | `pnpm build`；自动/手动切换；高级折叠；保存与清空反馈 |

命令：

```
cargo fmt --check
cargo clippy --all-targets
cargo test
pnpm build
```

## 11. 行为变化

1. 默认自动计算总磁盘上限（`limit_mb = 0`）；手动指定非 0 值时以手动值为准。
2. 新装默认切片 TTL 从 300 秒变为自动（无实测时 600 秒）。项目未正式使用，旧配置不迁移。
3. 磁盘回收常开，不再有开关。
4. GC 扫描周期固定 60 秒；单轮删除数按上限推导。
5. 自动上限按实测活跃频道数计算，最低 3 个。大盘不会永远停在 140 MB。
6. `statvfs` 失败时，页面与回收统一按工作集。
7. 只保留磁盘后端；删除 `cache.backend`、`cache.memory_max_mb` 与 `MemoryStore`。切片正文不进入进程内存。
8. 6 个不生效键与 2 组同义键从模型删除。
9. 「已用」显示切片正文加索引的合计。
10. 活跃频道覆盖 proxy、remux、dash 三种模式。
11. 磁盘索引占 10% 子预算；进程内索引 600 秒无活动淘汰。
12. 删除 `cache.segments.global_max`；索引预算由上限推导，不再有固定总量。
13. 缓存状态页保留「CDN 推荐值」；本机缓存单独展示，两个概念不再混在一张卡里。
14. 无实测时回退值 150 MB 参与公式夹取；只有它本身是限制因素时才显示 `no_data`。
15. `auto_tune=false` 只停止新采样；上限仍按当前磁盘空间计算，不冻结。
16. `/cache/stats` 删除 `seg_bytes`，改用 `segment_body_bytes`；新增 `channels` 与 `working_channels`。

## 12. 风险与回退

| 风险 | 处理 |
| --- | --- |
| 活跃频道统计低估 | HLS 播放器会周期刷新清单；`media_entry` 与 `serve_rest` 都记录活动。窗口默认 90 秒。最低按 3 个频道兜底 |
| 短时间换台多，活跃频道数偏高 | 上限只增不减到下一轮；多留的缓存不会影响播放 |
| 扫描数按 4 MB/片估算 | 夹在 500–5000；实际偏差只影响单轮删除速度 |
| 60 秒扫描间隔内占用上涨 | 文档说明上限是扫描后的目标，不是实时硬墙 |
| 旧键残留 | 旧键不读取；保存时从 `extra` 删除 |
| 回滚到旧版本 | 不做兼容；升级前备份数据库 |
| 移除 GC 内部参数不满足特殊部署 | 先用固定值；如出现真实需求，再按「一个键」原则加入 |
| 索引预算按文件实际大小记账 | 以 `seg_url` 目录的实际字节为准，不按条目估算；偏差只影响正文与索引的分配，总上限不变 |
| 600 秒活动窗口过短 | 旧条目从进程内索引淘汰；磁盘索引仍可回退 |
| 无实测且磁盘快满 | 回退值 150 MB 仍受可用空间与下限夹取；页面标 `floor` 或 `budget`，并说明原始原因 |
| `auto_tune=false` 后上限变化 | 只停止新采样；上限仍随可用空间变化，文案已说明 |

## 13. 构建器决定

以下是设计评审后由实现方决定的事项。用户可以在批准前否决。

1. **`limited_by` 增加 `no_data` 与 `space_unknown`。** 回退值 150 MB 只是工作集替身，仍参与公式夹取；`space_unknown` 时上限取 `max(50, 工作集或 150)`。手动上限优先。
2. **活跃频道在 `media_entry` 与 `serve_rest` 记录。** 覆盖 proxy、remux、dash。不要求各模式单独实现。
3. **「已用」显示正文加索引合计。** 新增 `segment_body_bytes`、`segment_index_bytes`、`segment_cache_bytes`。
4. **索引预算由缓存上限推导。** 磁盘索引子预算 10%，正文至少 90%。按 `seg_url` 文件实际字节记账，超预算按文件 mtime 淘汰最旧。删除 `cache.segments.global_max`。
5. **进程内索引按 600 秒活动窗口淘汰。** 磁盘索引文件继续受 10% 子预算限制。
6. **默认上限自动计算，手动可覆盖。** `cache.limit_mb = 0` 表示自动；非 0 表示手动，优先于自动估算。
7. **`auto_tune=false` 不冻结上限。** 只停止新采样；沿用上次实测，仍按当前 `statvfs` 计算。
8. **`channels` 与 `working_channels` 分开。** `channels` 是活跃频道数，空闲时可为 0；`working_channels = max(3, channels)`。
9. **删除 `seg_bytes`。** 统一用 `segment_body_bytes`；同步前端与验收脚本。

## 14. 实施顺序

1. 配置模型、默认值、校验、旧键清理与单测。
2. 估算函数与切片索引的活跃频道统计。
3. 索引预算：磁盘索引子预算、按文件实际字节记账、活动时间淘汰。
4. 回收计划、`limited_by`、常量、推荐块与单测。
5. TTL 统一解析与契约测试。
6. `/cache/stats` 占用字段与推荐字段。
7. 设置页与缓存状态页改造。
8. 文档与示例配置更新。
9. 全量验证：fmt、clippy、test、build。

## 15. 实测结论与参数

| 项 | 结论 | 依据 |
| --- | --- | --- |
| 默认上限 | 自动计算；手动可指定 | 统一默认值，不按机器大小分档 |
| 磁盘索引预算 | 总上限 × 10% | 140 MB 上限下约 14 MB，可放约 15 个满历史频道或约 140 个 1 小时频道 |
| 正文/索引分配 | 索引子预算 10%，正文至少 90% | 索引远小于正文；固定比例无需共享记账 |
| GC 扫描周期 | 60 秒 | 3 频道峰值约 100 MB；10 频道约 335 MB。300 秒分别是约 500 MB 和约 1.7 GB |

默认配置对所有机器一致。128 MB 机器也使用同一默认值；缓存正文只写磁盘。

## 16. 后续任务：内存清理审计

以下项目不在本次范围，另起设计文档处理：

- DASH 会话 `entries` 按保留窗口或最大条目数裁剪（`src/dash_hls.rs`）。
- DASH 时移 `seen` 随切片一起裁剪（`src/dash_timeshift.rs`）。
- `TtlCache`、`AddressCache`、解析脚本缓存删除过期条目。
- 在线观众队列增加条数上限或改为聚合计数。
- `SourceCache` 增加正文总量上限。
- EPG 解析结果缓存的大小说明与上限。
- 备份恢复、EPG 采集、质量探测等一次性缓冲的上限。
