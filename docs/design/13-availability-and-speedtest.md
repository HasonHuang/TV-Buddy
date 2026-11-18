# 13 · 可用性校验与测速筛选

> 历史参考。同步源成员语义已被 [`14-quality-admission-product.md`](14-quality-admission-product.md) 与 [`16-quality-admission-system.md`](16-quality-admission-system.md) 取代：不合格频道从“隐藏”改为“立即移出”，并取消淘汰池。

> 设计版本 v1.0 ｜ 2026-11-15 ｜ 状态：**方案 B 已选（设计待批准）**
> 参考实现：iptv-api。核对文件：`utils/speed.py`、`utils/channel_quality.py`、`utils/channel.py`、`utils/frozen.py`、`utils/channel_repository.py`。
> 上游：[`11-subscription-sync.md`](11-subscription-sync.md)、[`03-http-contract.md`](03-http-contract.md)、[`04-config-and-storage.md`](04-config-and-storage.md)、[`06-low-spec-optimization.md`](06-low-spec-optimization.md)。

## 1 需求

| 编号 | 需求 | 验收判据 |
| --- | --- | --- |
| R1 | 三级规则 | 订阅源、分组、频道都能设置规则。频道可覆盖分组，分组可覆盖源，源可覆盖全局。 |
| R2 | 分层探测 | 支持只探连通、解析清单、采样测速、解析轨道四档。每档只做必要的请求。 |
| R3 | 测速筛选 | 按最低速率、最低/最高分辨率、分辨率速率映射、最大延迟、最低帧率筛选。 |
| R4 | 结果可见 | 面板显示延迟、速率、分辨率、失败原因与最近校验时间。 |
| R5 | 自动下线与恢复 | 不合格频道按规则从订阅清单移除。恢复后自动回到清单。 |
| R6 | 源可用性 | 统计源的抓取失败与频道合格率。支持自动停用与手动恢复。 |
| R7 | 手动校验 | 可对全部、单源、单分组、单频道发起校验。可看进度与结果。 |
| R8 | 持久化 | 规则进配置。结果进 SQLite。重启后保留最近结果与历史。 |
| R9 | 低配可控 | 有并发、超时、单轮流量预算与单轮频道上限。播放繁忙时可降级或延后。 |
| R10 | 不破坏现有链路 | 默认关闭。关闭时行为与现在完全一致。 |

非目标：

| 项 | 原因 |
| --- | --- |
| 转码质量比较 | 只判可用性，不判画质主观好坏。 |
| 客户端侧测速 | 校验在服务端完成。播放器网络差异不在范围内。 |
| 写回上游 M3U | 上游清单只读。 |
| 按 ISP / 归属地筛选 | 第一版不做。字段可预留，规则不实现。 |
| 多实例分布式校验 | 单进程。校验任务由本机执行。 |
| 自动删除频道 | 默认只隐藏。删除必须人工确认。 |

## 2 参考行为对齐

iptv-api 在生成结果清单时做校验。本项目的输出是代理入口，不是上游地址清单。因此只对齐判定行为，不对齐部署形态。

| iptv-api 行为 | 本项目设计 | 说明 |
| --- | --- | --- |
| `open_speed_test` | `quality.enabled` | 总开关。默认关闭。 |
| `speed_test_mode`：`quick` / `full` / `manual` | `run_mode`：`quick` / `full`；`interval_secs=0` 表示只手动触发 | 不单独设 `manual` 枚举。 |
| `speed_test_target`、`output_urls_limit` | `quick_target`、`keep_candidates` | 快速模式达到目标后停止。 |
| `open_filter_speed`、`min_speed` | `rules.min_speed_mib` | 低于阈值判 `speed_low`。 |
| `resolution_speed_map` | `rules.resolution_speed_map` | 按分辨率取对应最低速率。 |
| `open_filter_resolution`、`min_resolution`、`max_resolution` | `rules.min_resolution`、`rules.max_resolution` | 分辨率未知时放行。与参考实现一致。 |
| `open_filter_ad` | `rules.filter_ad` | 短循环与广告关键字。 |
| `open_supply` | `rules.supply` | 全部不合格时保留可连通的最优项。 |
| `sort_by` | `rules.sort_by` | 空值保持配置顺序。 |
| 白名单不测速、始终保留 | `rules.retain` | 本项目仍测速，只保证不隐藏。 |
| `speed_test_filter_host` | `rules.host_cache` | 同一 Host 复用测量值。可达性仍逐频道判断。 |
| `utils/frozen.py` 连续失败冻结 | `consecutive_fails` + `next_check_at` | 失败退避，成功后清零。 |
| `channel_repository` 候选、测量、输出快照 | `quality_results`、`quality_history`、`quality_pins` | 只存结果，不改配置。 |
| `open_auto_disable_source` | 源自动停用 | 运行时标记，不写 `enabled`。 |
| 分辨率未知仍可合格 | 同 | 避免把无法探测当成不合格。 |
| `supply` 绕过阈值但不绕过连通 | 同 | 不可连通的频道不保留。 |

本项目与参考实现的差异：

| 项 | iptv-api | 本项目 | 理由 |
| --- | --- | --- | --- |
| 判定对象 | 上游 URL 清单 | 代理入口 | 本项目不把上游地址直接交给用户。 |
| 多地址 | 一个频道多个候选 URL | 一个频道一个 URL | 现有模型如此。多候选作为可选阶段。 |
| 规则范围 | 全局 + 白名单 | 全局 + 源 + 分组 + 频道 | 面板已有源、分组、频道三级容器。 |
| 校验时机 | 生成清单时同步执行 | 后台周期为主，手动触发；同步前校验可选 | 同步是定时任务，不应被测速拖住。 |
| 失败处理 | 不写入结果 | 从订阅清单隐藏，配置保留 | 隐藏可自动恢复，删除不可逆。 |

## 3 方案对比与总览

先比较三个整体方案。三者都能实现「按规则校验并筛选」，差别在职责边界、默认风险与交付成本。

### 3.1 方案 A：扩展现有健康探测

做法：

- 不新增配置段。扩展 `[health]`。
- 增加 `probe_mode`（连通 / 清单 / 测速）与阈值字段。
- 扩展现有 `Health::Entry`，增加速率与分辨率。
- 现有健康循环直接承担测速。扩展 `/panel/api/health` 与分组页徽章。
- 结果只放内存。

优点：

| 项 | 说明 |
| --- | --- |
| 改动最小 | 复用现有调度、状态、接口与前端状态点。 |
| 概念少 | 管理员只看到一个「健康探测」。 |
| 兼容直接 | `offline_on_fail` 与现有行为可以原样复用。 |
| 交付快 | P0 只需扩展一个模块与一个设置卡。 |

缺点：

| 项 | 说明 |
| --- | --- |
| 职责混杂 | 轻量探活与重测速共用一套周期与并发。默认值难以同时满足两者。 |
| 规则粒度弱 | 源 / 分组 / 频道覆盖需要把字段塞进 `Health`，模型会散。 |
| 没有历史 | 重启丢结果。无法回答「昨天为什么下线」。 |
| 默认风险 | 开启健康探测就自动变成测速，低配与大清单会意外消耗带宽。 |
| 源级能力弱 | 自动停用、合格率、手动任务仍要另建状态。 |

适用：

- 只想要「状态点 + 速率徽章」。
- 频道数小于 200。
- 不需要历史、分级规则与自动停用。

### 3.2 方案 B：独立质量校验子系统（推荐）

做法：

- 新增 `[quality]`。新增 `src/quality.rs` 与 `src/quality_store.rs`。
- 独立调度器、探测器、结果存储与 HTTP 接口。
- 复用 `Health` 的结果形状与订阅过滤入口。
- 规则分源 / 分组 / 频道 / 全局四级。
- 结果进 SQLite。订阅渲染只读内存快照。
- 现有 `[health]` 保持轻量。质量启用时接管探测，不重复跑。

优点：

| 项 | 说明 |
| --- | --- |
| 默认安全 | `quality.enabled=false`。不改变现有行为与成本。 |
| 规则完整 | 四级覆盖，逐字段继承。满足「按规则对源、分组、频道校验」。 |
| 可恢复 | 只隐藏不删除。成功自动恢复。失败有宽限与退避。 |
| 可观测 | 延迟、速率、分辨率、原因、历史、任务进度都可查。 |
| 低配可控 | 四档探测、并发、超时、单轮预算、单轮上限、播放繁忙时延后。 |
| 可分期 | 第一期只做连通与清单，行为接近方案 A。后续再加测速、`ffprobe`、去重。 |
| 与现有架构贴合 | 项目是常驻代理，不是批量生成器。后台校验与代理形态一致。 |

缺点：

| 项 | 说明 |
| --- | --- |
| 代码更多 | 新增模块、表、接口、前端页面与测试。 |
| 概念更多 | 面板会有「健康探测」与「质量校验」两个词。需要文案解释。 |
| 调度协作 | 必须处理与健康探测、订阅同步、录制的互斥与降级。 |
| 结果一致 | 订阅清单、我的订阅、面板三处必须共用同一判定，否则会漂移。 |

适用：

- 长期运行的代理服务。
- 多源、大清单、低配机器。
- 需要自动恢复、历史与分级规则。

### 3.3 方案 C：同步生成时内联校验

做法：

- 对齐 iptv-api 的生成流程。
- 订阅同步抓取 M3U 后，先对候选频道测速与筛选。
- 合格项才写入 `config.groups`。不合格项直接丢弃。
- 规则主要放全局与源。分组规则只能影响新建分组。
- 结果不保留周期状态。下次同步重新测。

优点：

| 项 | 说明 |
| --- | --- |
| 最贴近参考实现 | 校验发生在生成阶段，输出只含合格项。 |
| 订阅渲染简单 | 渲染时不查质量状态，不做隐藏判定。 |
| 配置干净 | 配置里没有长期隐藏的失败频道。 |
| 无周期负载 | 只在同步时集中消耗资源。 |

缺点：

| 项 | 说明 |
| --- | --- |
| 同步变慢 | 同步时长随频道数增长。大清单可能需要数十分钟。 |
| 无自动恢复 | 频道失败后要等下一次同步才会回来。中途恢复不可见。 |
| 失败放大 | 源站抖动或测速节点故障时，可能一次同步清空大量频道。 |
| 手工频道不覆盖 | 手工分组与手工频道不经过同步，无法被同一规则校验。 |
| 聚合源不覆盖 | 聚合源条目不在配置中，无法在同步阶段处理。 |
| 低配不友好 | 同步与测速叠加，CPU、内存与带宽峰值高。 |
| 结果不可解释 | 频道消失后，面板没有周期结果与历史，排障困难。 |

适用：

- 产品形态是「批量生成播放列表」，不是常驻代理。
- 清单小，按夜间批处理运行。
- 可以接受失败后等下一轮同步恢复。

### 3.4 对比

| 维度 | 方案 A：扩展健康探测 | 方案 B：独立质量子系统 | 方案 C：同步内联校验 |
| --- | --- | --- | --- |
| 与 iptv-api 判定行为对齐 | 部分 | 完整 | 最接近生成流程 |
| 与当前代理架构契合 | 中 | 高 | 低 |
| 默认风险 | 中。健康探测变重 | 低。默认关闭 | 高。同步失败影响内容 |
| 低配可用性 | 中 | 高。分档、预算、延后 | 低。同步阻塞 |
| 实现成本 | 低 | 中高 | 中 |
| 规则粒度 | 全局 + 现有下线开关 | 源 / 分组 / 频道 / 全局 | 全局 / 源为主 |
| 手工频道覆盖 | 是 | 是 | 否 |
| 聚合源覆盖 | 否 | P3 可加 | 否 |
| 结果与历史 | 无。仅内存 | 有。SQLite + 历史 | 可存，但无周期恢复 |
| 自动恢复 | 有。下一轮探测 | 有。下一轮 + 退避 | 仅下次同步 |
| 手动触发 | 无独立任务 | 有。范围、进度、取消 | 只能触发同步 |
| 对播放的影响 | 周期探测可能抢带宽 | 可配置预算与播放繁忙时延后 | 同步时集中抢带宽 |
| 运维解释成本 | 低 | 中 | 高 |
| 适用规模 | 小于 200 频道 | 任意。受预算控制 | 小清单，夜间批处理 |

### 3.5 推荐

推荐 **方案 B**。

理由：

1. 项目是常驻代理。频道内容是长期配置，不是一次生成的文件。周期校验与自动恢复更贴合。
2. 源站会抖动。只隐藏不删除，配合宽限与退避，可以避免一次故障清空清单。
3. 低配是明确约束。分档、预算、并发上限与播放繁忙时延后，只有独立子系统能干净实现。
4. 方案 B 可以分批交付。第一阶段只做「连通 + 清单」，成本接近方案 A。后续再补测速、`ffprobe`、历史与去重。
5. 方案 C 有价值的「同步门控」可以作为方案 B 的可选开关。它不是主路径，但能在源头减少无效频道落库。

不选方案 A 的原因：

- 默认成本会抬高。用户只想看状态点时，不应自动测速。
- 源级自动停用、历史、手动任务仍要另建状态。最终会演化成半个方案 B。
- 规则字段塞进 `[health]` 后，语义与校验都变复杂。

不选方案 C 的原因：

- 同步会阻塞。大清单与慢源会拖长同步，且失败后没有中途恢复。
- 手工频道与聚合源无法覆盖。规则会变成「只对同步内容生效」，与需求不符。
- 结果不可解释。频道消失后缺少周期结果与历史，面板难以排障。

实施建议：

| 阶段 | 采用方案 | 内容 |
| --- | --- | --- |
| P0 | B 的轻量形态 | `[quality]` + 连通档 + 规则解析。行为接近 A，但模型为 B 留好扩展点。 |
| P1 | B | 清单档、测速档、内存结果、订阅过滤、手动任务。 |
| P2 | B | SQLite 历史、源状态、自动停用、面板完整化。 |
| P3 | B + C 的可选门控 | `ffprobe`、广告、去重、同步门控、清理预览。 |

### 3.6 方案总览

```text
配置中的规则
  [quality] / source.quality / group.quality / channel.quality
        │
        ▼
有效规则解析（逐字段覆盖）
        │
        ▼
校验调度器
  ├─ 周期：按 next_check_at 挑选到期频道
  ├─ 手动：全部 / 单源 / 单分组 / 单频道
  ├─ 同步后：只校验新增或变更频道
  └─ 播放失败：短 TTL 触发单频道复测（可选）
        │
        ▼
探测器
  reachability → playlist → speed → full
        │
        ▼
结果
  内存快照（订阅渲染直接读）
  SQLite（面板、历史、重启恢复）
        │
        ▼
应用
  订阅清单：隐藏 / 标记 / 固定保留
  面板：状态点、徽章、进度、历史
  源状态：合格率、自动停用
```

### 3.7 关键决策

| 编号 | 决策 | 推荐 | 理由 | 落选方案 |
| --- | --- | --- | --- | --- |
| D1 | 校验子系统位置 | 新增 `[quality]`，复用 `Health` 状态结构 | 职责清楚。现有 `[health]` 保持轻量兼容 | 只扩展 `[health]`：字段混杂，默认探测变重 |
| D2 | 不合格处理 | 默认 `hide`，配置保留 | 可自动恢复。避免误删 | 同步时删除：不可逆，网络抖动会清空内容 |
| D3 | 结果存放 | SQLite + 内存快照 | 重启不丢。订阅渲染不查库 | 只放内存：重启后全部未知 |
| D4 | 校验时机 | 后台周期为主 | 不阻塞同步与播放 | 只在同步时校验：慢，且失败会阻断同步 |
| D5 | 多地址候选 | 第一版不做 | 现有模型一频道一地址。先交付规则与测速 | 改造模型：影响同步、播放、回滚 |
| D6 | 探测失败的处理 | `probe_error` 默认不隐藏 | 避免解析器缺陷造成误判 | 与 `unreachable` 同罚：误伤风险高 |
| D7 | 源自动停用 | 运行时标记 | 保留管理员意图。可一键恢复 | 改写 `source.enabled`：丢失用户设置 |

## 4 规则模型

### 4.1 全局 `[quality]`

```json
{
  "quality": {
    "enabled": false,
    "probe_mode": "reachability",
    "interval_secs": 900,
    "timeout_secs": 10,
    "concurrency": 2,
    "probe_concurrency": 1,
    "ffprobe_timeout_secs": 5,
    "ffprobe_sample_kb": 512,
    "speed_timeout_secs": 10,
    "sample_bytes_kb": 4096,
    "sample_secs": 2.0,
    "sample_segments": 2,
    "max_playlist_kb": 2048,
    "budget_mb_per_run": 64,
    "max_channels_per_run": 0,
    "max_ffprobe_per_run": 0,
    "ffprobe_min_speed_mib": 0.0,
    "history_mode": "changes",
    "history_keep_days": 7,
    "stale_keep_secs": 1800,
    "quick_target": 1,
    "auto_disable_after_fails": 3,
    "cleanup_fail_threshold": 3,
    "sort_by": [],
    "filter_ad": false,
    "ffprobe": false,
    "probe_passthrough": false,
    "defer_during_playback": true,
    "rules": {
      "min_speed_mib": 0.0,
      "fallback_speed_mib": 0.0,
      "max_delay_ms": 0,
      "min_resolution": "",
      "max_resolution": "",
      "resolution_speed_map": {},
      "min_fps": 0.0,
      "ip_family": "any",
      "require_video": false,
      "require_audio": false,
      "supply": false,
      "action": "hide",
      "retain": false,
      "dedupe": "off",
      "keep_candidates": 1,
      "host_cache": true
    }
  }
}
```

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | 校验总开关。关闭时不做周期探测、不隐藏、不触发源副作用。 |
| `probe_mode` | string | `reachability` | 探测档位：`reachability` / `playlist` / `speed` / `full`。 |
| `interval_secs` | u64 | `900` | 周期。下限 60。`0` 表示只手动运行。 |
| `timeout_secs` | u64 | `10` | 单频道总超时。 |
| `concurrency` | u32 | `2` | 网络并发。范围 1–32。 |
| `probe_concurrency` | u32 | `1` | `ffprobe` 并发与采集上限。范围 1–4。 |
| `ffprobe_timeout_secs` | u64 | `5` | 单次 `ffprobe` 进程超时。范围 1–30。 |
| `ffprobe_sample_kb` | u64 | `512` | 送给 `ffprobe` 的最大字节数。范围 64–2048。 |
| `speed_timeout_secs` | u64 | `10` | 测速请求超时。 |
| `sample_bytes_kb` | u64 | `4096` | 单流采样字节上限。 |
| `sample_secs` | f64 | `2.0` | 单流采样时长上限。 |
| `sample_segments` | u32 | `2` | HLS 每次采样的切片数。大清单可设为 1。 |
| `max_playlist_kb` | u64 | `2048` | 清单正文上限。 |
| `budget_mb_per_run` | u64 | `64` | 单轮总下载预算。`0` 表示不限。 |
| `max_channels_per_run` | u64 | `0` | 单轮频道上限。`0` 表示全部。 |
| `max_ffprobe_per_run` | u64 | `0` | 单轮 `ffprobe` 次数上限。`0` 表示不限。 |
| `ffprobe_min_speed_mib` | f64 | `0.0` | 速率达到该值才运行 `ffprobe`。`0` 表示不设门槛。 |
| `history_mode` | string | `changes` | `off` / `changes` / `hourly` / `all`。见 §6.2。 |
| `history_keep_days` | u64 | `7` | 历史保留天数。 |
| `stale_keep_secs` | u64 | `1800` | 连续失败后保留可见的宽限。默认两倍周期，避免单次抖动下线。`0` 表示立即隐藏。 |
| `quick_target` | u32 | `1` | 快速模式的候选组目标合格数。见 §5.1。 |
| `auto_disable_after_fails` | u32 | `3` | 源连续抓取失败多少次后自动停用。`0` 表示关闭。 |
| `cleanup_fail_threshold` | u32 | `3` | 清理失效频道所需连续 `unreachable` 次数。 |
| `sort_by` | array | `[]` | `speed` / `delay` / `resolution`。空值保持配置顺序。 |
| `filter_ad` | bool | `false` | 识别短循环与广告占位。 |
| `ffprobe` | bool | `false` | 启用分辨率识别。只在 `full` 档、清单无完整分辨率时运行。关闭时是硬上限。 |
| `probe_passthrough` | bool | `false` | 是否探测 `passthrough` 分组。 |
| `defer_during_playback` | bool | `true` | 有在线播放时降低并发或延后本轮。 |
| `rules.*` | object | 见下 | 判定阈值与动作。 |

`rules` 字段：

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `min_speed_mib` | f64 | `0.0` | 最低速率。`0` 表示不筛。 |
| `fallback_speed_mib` | f64 | `0.0` | 分辨率未知，或速率表未命中时的最低速率。`0` 表示使用 `min_speed_mib`。 |
| `max_delay_ms` | u64 | `0` | 最大首包延迟。`0` 表示不筛。 |
| `min_resolution` | string | 空 | 最低分辨率，如 `1280x720`。空表示不筛。 |
| `max_resolution` | string | 空 | 最高分辨率。空表示不筛。 |
| `resolution_speed_map` | map | `{}` | 分辨率到最低速率的映射。按像素面积向下取整。未命中时用兜底速率。 |
| `min_fps` | f64 | `0.0` | 最低帧率。`0` 表示不筛。 |
| `ip_family` | string | `any` | `any` / `ipv4` / `ipv6`。 |
| `require_video` | bool | `false` | 必须探测到视频轨道。 |
| `require_audio` | bool | `false` | 必须探测到音频轨道。 |
| `supply` | bool | `false` | 全部不合格时保留可连通的最优项。 |
| `action` | string | `hide` | `hide` / `mark` / `off`。 |
| `retain` | bool | `false` | 永不隐藏。仍记录结果。 |
| `dedupe` | string | `off` | `off` / `tvg_id` / `name`。同组去重。 |
| `keep_candidates` | u32 | `1` | 去重后每个频道保留的候选数。 |
| `host_cache` | bool | `true` | 复用同一 Host 的**测量值**，不复用可达性判定。见 §5.8。 |

### 4.2 三级覆盖

订阅源、分组、频道各增加一个可选的 `quality` 对象。对象只写需要覆盖的字段。

```json
{
  "playlist": {
    "sources": [
      {
        "id": "src-a",
        "quality": {
          "enabled": true,
          "min_speed_mib": 1.0,
          "action": "hide"
        }
      }
    ]
  },
  "groups": [
    {
      "id": "sports",
      "quality": {
        "min_speed_mib": 2.0,
        "min_resolution": "1920x1080"
      },
      "channels": [
        {
          "id": "c1",
          "quality": {
            "retain": true
          }
        }
      ]
    }
  ]
}
```

覆盖字段：

| 字段 | 适用层级 | 说明 |
| --- | --- | --- |
| `enabled` | 源 / 分组 / 频道 | 三态。`false` 表示该范围不校验、不隐藏。 |
| `min_speed_mib`、`fallback_speed_mib`、`max_delay_ms`、`min_fps` | 源 / 分组 / 频道 | 数值阈值。 |
| `min_resolution`、`max_resolution`、`resolution_speed_map` | 源 / 分组 / 频道 | 分辨率阈值。 |
| `ip_family`、`require_video`、`require_audio` | 源 / 分组 / 频道 | 轨道与协议要求。 |
| `supply`、`action`、`retain`、`dedupe`、`keep_candidates`、`host_cache` | 源 / 分组 / 频道 | 行为开关。 |
| `stale_keep_secs` | 源 / 分组 / 频道 | 失败宽限。 |
| `auto_disable_after_fails` | 源 | 源自动停用阈值。`0` 表示关闭。 |

调度参数（`interval_secs`、`concurrency`、`budget_mb_per_run` 等）只放全局。这样避免每个分组各跑一套调度。

### 4.3 有效规则解析

顺序：**频道 > 分组 > 源 > 全局**。逐字段取第一个非空值。

总开关 `quality.enabled` 是硬门。它为 `false` 时，任何覆盖都不能启用校验。

```text
effective.enabled =
  quality.enabled
  && (channel.quality.enabled
      ?? group.quality.enabled
      ?? source.quality.enabled
      ?? true)
```

启用后，覆盖可以局部开启或关闭。例如某个源关闭校验，某个分组仍可单独开启。

理由：

- 分组是运维边界。授权的用户、访问级别、播放模式都挂在分组上。分组规则应最贴近实际服务范围。
- 源规则是导入默认值。它约束新同步内容，但不推翻管理员对分组的局部调整。
- 频道规则用于例外。例如单个频道必须保留，或必须更严格。

面板必须显示每个字段的**有效值**与**来源层级**。避免管理员猜规则。

源规则的匹配条件：

- 只匹配 `channel.source_id == source.id` 的频道。
- 不使用 `group.source_id`。该字段只表示分组的创建来源，不表示独占（见 `11-subscription-sync.md` §3.3）。
- 手工频道不继承任何源规则。它只使用分组与全局规则。
- 合并分组中的频道各自按自己的 `source_id` 取源规则。同一分组内可以有不同的源规则。

结果与地址的绑定：

- `url_hash` 取配置里 `channel.url` 的 SHA-256 前 16 位。
- 计算前去掉末尾 `$` 描述段。使用解析脚本前的地址。
- 不使用解析脚本的输出。脚本输出可能轮换令牌，不能作为稳定键。
- 当前配置地址的 `url_hash` 与最近结果不一致时，该结果按 `unknown` 处理。旧结果不参与隐藏判定。
- 地址变化后，新地址在首次探测前保持可见。

### 4.4 判定结果与动作

| 结果 | 含义 | 默认是否隐藏 |
| --- | --- | --- |
| `pass` | 全部规则通过 | 否 |
| `retained` | `retain=true`。不判阈值 | 否 |
| `unreachable` | 连接失败、非 2xx、正文为空、超时 | 是。宽限后隐藏 |
| `speed_low` | 速率低于阈值 | 是 |
| `resolution_low` | 分辨率低于下限 | 是 |
| `resolution_high` | 分辨率高于上限 | 是 |
| `delay_high` | 首包延迟高于上限 | 是 |
| `fps_low` | 帧率低于下限 | 是 |
| `track_missing` | 缺少要求的视频或音频轨道 | 是 |
| `ip_mismatch` | 协议族不符合要求 | 是 |
| `ad` | 识别为短循环或广告占位 | 是 |
| `supply` | 不合格，但作为兜底保留 | 否 |
| `probe_error` | 探测程序错误、清单解析失败 | **否**。按未知处理 |
| `unknown` | 没有结果或结果已过期 | 否 |

动作语义：

| `action` | 订阅清单 | 面板 | 典型用途 |
| --- | --- | --- | --- |
| `hide` | 隐藏不合格频道 | 显示状态与原因 | 默认 |
| `mark` | 全部保留 | 显示状态与原因 | 先观察，不改变输出 |
| `off` | 全部保留 | 可看历史结果 | 局部停用 |

补充规则：

- `retain=true` 优先于 `hide`。但结果照常记录。参考实现的白名单完全不测速；本项目保留测速，便于发现源站问题。
- `supply=true` 时，若同一候选组全部失败，保留一个可连通的最优项，结果记为 `supply`。完全不可连通的项不保留。
- 分辨率未知时，不因分辨率规则隐藏。与参考实现一致。
- `ffprobe` 失败、超时或输出无法解析时，分辨率保持未知。判定使用兜底速率。不记 `probe_error`。
- `probe_error` 不隐藏。连续 3 次 `probe_error` 时，面板标黄，提醒管理员检查解析器或探测地址。
- `probe_error` 不计入连续失败，也不设置 `first_fail_at`。避免解析器缺陷让后续真实失败提前触发下线。
- `stale_keep_secs` 从**连续失败的第一次失败时间**（`first_fail_at`）开始计算。`unreachable` 与阈值失败都算失败。重复失败不重置起点。成功一次即清零。
  - 默认 `interval_secs=900`、`stale_keep_secs=1800` 时，需要连续两次失败，才会在第二次失败后 900 秒隐藏。
  - 单次失败不会隐藏。下一轮探测成功即恢复。
  - 想更快隐藏时，把 `stale_keep_secs` 设为小于 `interval_secs`。此时单次失败也会在下一次探测前隐藏。
  - 没有 `first_fail_at` 时用 `checked_at`。旧数据迁移时按此处理。
- 结果超过 2 个周期未更新时标 `stale`。`stale` 只影响显示，不改变隐藏判定。隐藏仍按 `first_fail_at`。
- `ffprobe=false` 时，依赖 `ffprobe` 的规则（`min_fps`、`require_video`、`require_audio`）不执行。结果带 `skipped_rules` 列表，面板显示警告。其他规则照常判定。

### 4.5 与现有字段的关系

| 现有能力 | 关系 |
| --- | --- |
| `[health].enabled` | 保留。`quality.enabled=false` 时，健康探测按原逻辑运行。 |
| `[health]` 周期、超时、并发 | 保留。`quality.enabled=true` 时，质量调度器接管探测，不再重复跑健康循环。 |
| `Health.Entry` | 保留原字段。质量调度器同时写入可达性结果。 |
| `offline_on_fail` | 保留。仍只看可达性，且仍受 `[health].enabled` 控制。 |
| `/panel/api/health` | 保留原字段。新增 `reason`、`speed_mib`、`resolution`、`fps`、`codecs`。 |
| 分组页状态点 | 保留。新增质量徽章。 |

质量调度器写入 `Health` 的规则：

| 质量结果 | `Health.Entry.ok` | `Health.Entry.error` |
| --- | --- | --- |
| `pass`、`supply`、`retained` | `true` | `null` |
| 阈值失败（`speed_low`、`resolution_*`、`delay_high`、`fps_low`、`track_missing`、`ip_mismatch`、`ad`） | `true` | `null` |
| `unreachable` | `false` | 原因文本 |
| `probe_error` | 保持上一次值 | 原因文本 |

说明：

- `Health` 只表示可达性。阈值失败不改变 `ok`。这样 `offline_on_fail` 的旧语义不变。
- 质量规则的下线由质量状态与 `action` 决定，不经过 `Health`。
- `[health].enabled=false` 时，`offline_on_fail` 不生效。质量规则仍按自己的开关工作。
- 两个开关都开启时，质量调度器取代健康循环。避免同一频道每轮探测两次。
- `interval_secs=0` 表示只手动运行。此时健康循环继续工作，保证仍有周期可达性数据。
- 质量关闭时，健康循环恢复原行为。

兼容规则：

- 新字段都有默认值。旧配置加载后 `quality.enabled=false`。
- `offline_on_fail` 与质量规则同时生效时，任一判定为隐藏即隐藏。
- `quality.enabled=false` 时，`quality` 覆盖对象被忽略。避免半开状态。
- 规则修改立即影响订阅渲染。渲染使用「当前规则 + 最近结果」。测量值等下一轮探测更新。

## 5 探测流程

### 5.1 四档探测

| 档位 | 请求 | 产出 | 成本 | 默认用途 |
| --- | --- | --- | --- | --- |
| `reachability` | 1 次 GET 或 HEAD。读首字节。 | HTTP 状态、延迟、重定向 | 极低 | 默认。等价现有健康探测 |
| `playlist` | 取清单，校验结构与首个切片可访问 | 清单类型、切片可达、分辨率元数据 | 低 | 需要发现坏清单 |
| `speed` | 清单 + 采样 2 个切片 | 速率、延迟、分辨率 | 中。消耗带宽 | 需要速率筛选 |
| `full` | `speed` + 按需分辨率识别 | 速率、分辨率 | 中高。本地进程解析已下载字节 | 需要识别清单缺少的分辨率 |

档位按 `probe_mode` 选择。规则需要某档数据时，自动提升到该档：

- 设置 `min_speed_mib`、`fallback_speed_mib`（大于 0）或 `resolution_speed_map` → 至少 `speed`。
- 设置 `filter_ad` → 至少 `playlist`。
- 设置 `min_fps`、`require_video`、`require_audio` → 至少 `full`。

`ffprobe=false` 是硬上限。它为 `false` 时，即使规则要求 `full`，也不运行 `ffprobe`。`probe_mode=full` 按 `speed` 执行。相关规则记入 `skipped_rules`，面板显示警告。

运行模式与探测档位分开：

| 概念 | 取值 | 作用 |
| --- | --- | --- |
| `probe_mode` | `reachability` / `playlist` / `speed` / `full` | 决定一次探测做多深。全局设置。 |
| 手动 `run_mode` | `quick` / `full` | 决定候选组测到什么时候停止。 |
| `manual` | `interval_secs=0` | 不自动运行，只接受手动触发。不单独设枚举。 |

`quick_target` 的语义：

- 去重关闭且一频道一地址时，`quick` 与 `full` 没有区别。每个频道只有一个候选，目标数 1 已满足。
- 去重开启时，按候选组生效。`quick` 达到 `quick_target` 个合格项后停止该组剩余候选。`full` 测完所有候选。
- 该字段不控制单轮频道总数。单轮上限用 `max_channels_per_run`，流量上限用 `budget_mb_per_run`。

### 5.2 地址解析

1. 如果频道有解析脚本，先调用 `resolver::resolve`。脚本失败记 `probe_error`，不记 `unreachable`。
2. 取频道级代理，其次分组级。使用 `state.http_for(gid, cid)`。
3. 合并请求头：全局默认 UA → 分组头 → 频道头。频道头优先。
4. 去掉地址末尾的 `$` 描述段。只保留 URL 主体。
5. 只接受 `http://` 与 `https://`。其他协议记 `probe_error`。
6. 跟随重定向。上限 5 次。最终地址作为后续请求基地址。
7. 记录首字节时间作为延迟。记录最终 HTTP 状态。

### 5.3 HLS

识别条件，满足任一即可：

- URL 含 `.m3u8`。
- `Content-Type` 含 `application/x-mpegurl`、`application/vnd.apple.mpegurl`、`audio/mpegurl`、`audio/x-mpegurl`。
- 正文以 `#EXTM3U` 开头。

主清单：

1. 解析 `#EXT-X-STREAM-INF`。
2. 读取 `BANDWIDTH`、`RESOLUTION`、`FRAME-RATE`。
3. 选择变体：
   - 设置了 `max_resolution` 时，先选不超过上限的最高码率变体。
   - 上限内没有变体时，选最高码率变体。结果记 `resolution_high`，不记 `probe_error`。该频道确实超过管理员设定的上限。
   - 未设置上限时，选最高码率变体。
   - 清单没有任何变体时，记 `probe_error`。
4. 记录清单的 `RESOLUTION`。宽度或高度为空时，`full` 档可用采集字节补充分辨率，见 §5.6。

媒体清单：

1. 解析切片地址。相对地址按清单地址解析。
2. 广告判定，见 §5.7。
3. 采样切片：取倒数第 2 个起、向前共 `sample_segments` 个。跳过最后一个，避免遇到未写完的切片。不足时取最后 `sample_segments` 个。
4. 并发下载采样切片，并发上限 2。每切片受 `sample_bytes_kb` 与 `sample_secs` 限制。
5. 速率 = 总字节 / 总耗时 / 1024 / 1024，单位 MiB/s。
6. 延迟 = 各切片首包时间平均值。
7. 没有切片或全部下载失败 → `probe_error`。

fMP4 限制见 §5.6。

### 5.4 DASH

1. 识别 `.mpd`、`application/dash+xml` 或正文以 `<MPD` 开头。
2. 用现有 `dash::parse` 读取轨道与 `bandwidth`、`width`、`height`、`frame_rate`。
3. 选择码率最高的视频轨道。
4. 下载初始化段与第一个媒体段，受采样上限约束。
5. 速率与延迟按下载结果计算。
6. 解析失败 → `probe_error`。DASH 探测未实现。非连通档对 `.mpd` 直接记 `probe_error`。

### 5.5 连续流与其他协议

1. 发起 GET，读取正文。
2. 达到 `sample_bytes_kb` 或 `sample_secs` 后主动断开。
3. 用已读字节与耗时计算速率。
4. 首字节时间作为延迟。
5. 正文为空 → `unreachable`。

### 5.6 `ffprobe`

`ffprobe` 只识别分辨率。它不访问网络。它读取测速阶段已下载的切片字节。这样不受切片扩展名限制，也不重复请求源站。

运行条件。全部满足时才采集并启动进程：

- 档位是 `full`。
- `quality.ffprobe=true`。
- 清单解析后，宽度或高度为空。
- 已测得速率，且不低于 `ffprobe_min_speed_mib`。没有速率数据时不运行。
- 单轮 `max_ffprobe_per_run` 未达到。`0` 表示不限。
- `ffprobe -version` 成功。服务缓存该结果。安装或删除后清空缓存。

采集规则：

- 只对第一个成功响应的采样切片采集。
- 从切片开头读取，最多 `ffprobe_sample_kb` KiB。
- 测速的字节计数不变。
- 需要采集时，先获取 `probe_semaphore` 许可。许可数为 `probe_concurrency`。许可覆盖采集与进程调用。
- 获取许可失败时，不采集。测速继续。分辨率保持未知。
- 峰值采集内存不超过 `probe_concurrency × ffprobe_sample_kb`。默认 1 × 512 KiB。最大 8 MiB。

命令。字节只经标准输入传递，不经过 shell：

```bash
ffprobe -v error -probesize 512K -analyzeduration 1000000 \
  -select_streams v:0 -show_entries stream=codec_type,width,height \
  -of json -
```

规则：

- 字节以 TS 同步字节 `0x47` 开头，且每 188 字节重复时，加 `-f mpegts`。
- 其他容器不加 `-f`。由 `ffprobe` 自动识别。
- 超时按 `ffprobe_timeout_secs`。超时后终止子进程。
- 标准输出上限 64 KiB。标准错误上限 4 KiB，只写调试日志。
- 取第一个 `codec_type=video` 且宽高大于 0 的流。
- 成功时写入宽高。日志记 `source=ffprobe`。
- 不覆盖清单已有的分辨率。

失败处理：

- 以下情况都保持分辨率未知：未安装、进程失败、超时、输出无法解析、没有视频流、达到单轮上限、许可不可用。
- 判定使用兜底速率，见 §4.1 `fallback_speed_mib`。
- 不记 `probe_error`。
- 不改变可达性、延迟与速率。

fMP4 限制：需要初始化段的 fMP4 HLS 无法用本方法识别。媒体字节缺少 `#EXT-X-MAP` 初始化段。`ffprobe` 解析失败。分辨率保持未知，使用兜底速率。不记 `probe_error`。

### 5.7 广告与占位识别

只对 HLS 生效。满足任一条件判 `ad`：

1. 媒体清单含 `#EXT-X-ENDLIST`，且切片总时长不超过 90 秒。
2. 清单地址或切片地址包含广告关键字。默认词表：
   `no_signal`、`nosignal`、`no-signal`、`signal_offline`、`no_video`、`novideo`、
   `advertisement`、`advert`、`placeholder`、`default_video`、`cctv_off`、`/ad/`、`/ads/`。
3. 关键字表可由规则覆盖。空数组表示关闭关键字判定。

### 5.8 测速算法

采用参考实现的采样方式。参数可配。

| 常量 | 默认 | 含义 |
| --- | --- | --- |
| 单流上限 | 4 MiB | 达到即停止。 |
| 单流时长 | 2.0 秒 | 达到即停止。 |
| 最小采样 | 64 KiB | 低于此值不判定稳定。 |
| 最短测量 | 1.0 秒 | 低于此值不判定稳定。 |
| 稳定窗口 | 4 个样本 | 每 64 KiB 计算一次瞬时速率。 |
| 稳定阈值 | 0.12 | 最大值与最小值差 / 平均值低于此值即提前结束。 |

计算：

```text
speed_mib = 总字节 / 总耗时 / 1024 / 1024
delay_ms  = 首字节时间 - 请求开始时间
```

补充：

- 总耗时为 0 时速率记 0。
- 重定向后的最终地址参与计算。
- 结果包含 `bytes` 与 `duration_ms`，面板可显示采样量。
- 单轮达到 `budget_mb_per_run` 后停止后续任务，保留已完成结果。
- `host_cache=true` 时，只复用同一 Host 的 `speed_mib` 与 `delay_ms`。条件如下：
  - 同一 Host、同一协议、最近一次成功测量不超过 1 个周期。
  - 当前频道仍独立发起请求，确认可达性与清单结构。
  - 当前频道失败时记 `unreachable`，不沿用缓存的通过结果。
  - 复用结果标记 `cached=true`。`resolution` 只在清单未提供时复用。

### 5.9 失败分类

| 情况 | 结果 |
| --- | --- |
| 连接失败、DNS 失败、TLS 失败 | `unreachable` |
| HTTP 4xx / 5xx | `unreachable` |
| 首字节超时 | `unreachable` |
| 正文为空 | `unreachable` |
| 清单格式错误 | `probe_error` |
| 清单没有切片 | `probe_error` |
| `ffprobe` 失败或超时 | 分辨率保持未知。使用兜底速率。不记 `probe_error` |
| 解析脚本失败 | `probe_error` |
| 达到阈值但未通过 | 对应阈值原因 |
| 达到阈值且通过 | `pass` |

## 6 结果模型与存储

### 6.1 内存快照

内存只保留订阅渲染需要的紧凑结果。完整字段留在 SQLite，面板按需查询。

```rust
#[repr(u8)]
pub enum Reason {
    Pass, Retained, Unreachable, SpeedLow, ResolutionLow,
    ResolutionHigh, DelayHigh, FpsLow, TrackMissing,
    IpMismatch, Ad, Supply, ProbeError, Unknown,
}

pub struct Entry {
    /// 配置地址的哈希。用于判断结果是否对应当前地址。
    pub url_hash: u64,
    pub checked_at: i64,
    /// 当前连续失败的第一次失败时间。成功时清空。
    pub first_fail_at: Option<i64>,
    /// 最近一次成功时间。
    pub last_ok_at: Option<i64>,
    pub next_check_at: i64,
    pub delay_ms: u32,
    /// 速率，单位 KiB/s。0 表示未测。
    pub speed_kib: u32,
    /// 分辨率。0 表示未知。
    pub width: u16,
    pub height: u16,
    /// 帧率 × 100。0 表示未知。
    pub fps_x100: u16,
    /// 位标志：ok / reachable / cached / stale。
    pub flags: u16,
    pub reason: Reason,
    pub consecutive_fails: u16,
}
```

- 键为 `gid/cid`。`QualityState` 用 `RwLock<HashMap<String, Entry>>`。
- 单条约 64 字节，另加键与哈希表开销。2 万频道约 3–5 MB。
- 分辨率在内存中存宽高，不存 `1920x1080` 字符串。
- 编码、采样字节、耗时、`skipped_rules`、完整原因文本只在 SQLite。
- 订阅渲染只读快照。不查 SQLite。持有读锁期间只做 O(1) 查询与字符串拼装。
- 写结果时只更新变更字段。不要整份克隆。
- 运行状态单独存：`run_id`、范围、总数、完成数、合格数、隐藏数、已下载字节、开始时间、取消标记。

### 6.2 SQLite 表

```sql
CREATE TABLE IF NOT EXISTS quality_runs (
    run_id       TEXT PRIMARY KEY NOT NULL,
    started_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    trigger      TEXT NOT NULL,
    scope        TEXT NOT NULL,
    scope_id     TEXT,
    probe_mode   TEXT NOT NULL,
    run_mode     TEXT NOT NULL,
    status       TEXT NOT NULL,
    tested       INTEGER NOT NULL DEFAULT 0,
    valid        INTEGER NOT NULL DEFAULT 0,
    hidden       INTEGER NOT NULL DEFAULT 0,
    bytes        INTEGER NOT NULL DEFAULT 0,
    error        TEXT
);

CREATE TABLE IF NOT EXISTS quality_results (
    gid              TEXT NOT NULL,
    cid              TEXT NOT NULL,
    url_hash         TEXT NOT NULL,
    url              TEXT NOT NULL,
    source_id        TEXT,
    checked_at       INTEGER NOT NULL,
    ok               INTEGER NOT NULL,
    reason           TEXT NOT NULL,
    http_status      INTEGER,
    delay_ms         INTEGER,
    speed_mib        REAL,
    resolution       TEXT,
    fps              REAL,
    video_codec      TEXT,
    audio_codec      TEXT,
    bytes            INTEGER NOT NULL DEFAULT 0,
    duration_ms      INTEGER NOT NULL DEFAULT 0,
    consecutive_fails INTEGER NOT NULL DEFAULT 0,
    first_fail_at    INTEGER,
    last_ok_at       INTEGER,
    skipped_rules    TEXT NOT NULL DEFAULT '[]',
    next_check_at    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (gid, cid, url_hash)
);

CREATE TABLE IF NOT EXISTS quality_history (
    gid          TEXT NOT NULL,
    cid          TEXT NOT NULL,
    url_hash     TEXT NOT NULL,
    checked_at   INTEGER NOT NULL,
    ok           INTEGER NOT NULL,
    reason       TEXT NOT NULL,
    delay_ms     INTEGER,
    speed_mib    REAL,
    resolution   TEXT,
    fps          REAL,
    bytes        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (gid, cid, url_hash, checked_at)
);

CREATE TABLE IF NOT EXISTS quality_pins (
    gid        TEXT NOT NULL,
    cid        TEXT NOT NULL,
    state      TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (gid, cid)
);
```

说明：

- `url_hash` 取配置里 `channel.url` 去掉 `$` 描述段后的 SHA-256 前 16 位。不使用解析脚本输出。
- 地址变化时保留旧记录，但旧记录不参与隐藏判定。新地址首次探测前按 `unknown` 处理，保持可见。
- `quality_results` 只保留当前地址的最新结果。旧地址结果在 7 天后清理。
- `quality_history` 的写入频率由 `history_mode` 控制：
  - `off`：不写历史。
  - `changes`（默认）：只在 `ok`、`reason` 或地址变化时写一行。同一状态不重复写。
  - `hourly`：每个频道每小时最多写一行。
  - `all`：每次探测都写。仅用于排障，不建议长期开启。
- `quality_history` 保留 `history_keep_days` 天（默认 7）。每小时清理一次。
- 结果写入按批提交。每 200 行或每 5 秒提交一次。避免每个频道一次事务。
- `quality_pins.state` 取 `keep` 或 `hide`。`keep` 强制可见，`hide` 强制隐藏。管理员操作，不被自动结果覆盖。
- 结果表不含请求头、查询令牌等敏感内容。`url` 在面板显示前做脱敏。
- 结果不写入配置文件。配置只存规则。

### 6.3 源状态扩展

在 `SourceStatus` 增加：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `quality_tested` | usize | 最近一轮校验的频道数。 |
| `quality_valid` | usize | 最近一轮合格数。 |
| `quality_hidden` | usize | 因规则隐藏的频道数。 |
| `last_probe` | u64 | 最近一轮校验完成时间。 |
| `consecutive_failures` | u32 | 连续抓取失败次数。 |
| `auto_disabled` | bool | 运行时自动停用。 |
| `auto_disable_reason` | string | 自动停用原因。 |

源自动停用：

- 只统计抓取失败。频道合格率不直接停用源。
- 连续失败达到 `auto_disable_after_fails`（默认 3）后设置 `auto_disabled=true`。
- 自动停用不修改 `source.enabled`。
- 自动停用期间，周期同步与聚合抓取都跳过。手动同步仍可执行，成功后解除。
- 面板显示原因、最近失败时间与「恢复」按钮。
- 阈值可在源 `quality` 覆盖。`0` 表示关闭。

## 7 应用点

### 7.1 订阅清单

`src/playlist.rs` 新增统一判定：

```rust
fn hidden(config: &Config, quality: Option<&QualityState>, group: &Group, channel: &Channel) -> bool
```

判定顺序：

1. `quality.enabled=false` → 不隐藏。
2. 分组为 `passthrough` 且 `probe_passthrough=false` → 不隐藏。
3. 管理员固定 `keep` → 不隐藏。
4. 管理员固定 `hide` → 隐藏。
5. `retain=true` → 不隐藏。
6. 有效 `action != hide` → 不隐藏。
7. 没有结果、结果与当前 `url_hash` 不一致、`probe_error` → 不隐藏。
8. 结果为 `pass` 或 `supply` → 不隐藏。
9. 结果为失败，且 `now - first_fail_at >= stale_keep_secs` → 隐藏。没有 `first_fail_at` 时用 `checked_at`。

`render_user`、`render_user_group`、`user_view` 共用该判定。避免两处规则漂移。

行为约定：

- 隐藏只影响新渲染的订阅清单。
- 直接播放地址不返回 404。旧链接继续可用。理由：用户可能已收藏；上游短暂失败不应造成播放中断。
- 归档组不探测，也不隐藏。
- 聚合源的条目不在第一版校验范围内。它们的上游地址没有本地 `gid/cid`，也不进配置。第一版只显示源抓取状态。P3 再评估按 URL 的探测缓存与渲染时过滤。

### 7.2 同步

默认：

- 同步流程不变。
- 同步完成后，对新增与 URL 变更的频道排队校验。不阻塞同步响应。
- 校验失败只影响清单显示，不删除频道。

可选 `sync_gate`（默认关闭）：

- 同步应用前，先校验计划中的新频道。
- 不合格频道不加入本次计划。
- 保护：本轮不合格比例超过 `sync_gate_max_fail_ratio`（默认 0.5）时，放弃本次门控，按原计划同步并标记警告。避免把源站故障当成频道全部失效。
- 已存在的频道不经门控。它们由后台周期校验处理。

清理动作：

- 面板提供「清理失效频道」。先预览，再确认。
- 清理只删除满足全部条件的同步频道：
  - `sync_follow=true`。
  - 连续 `unreachable` 次数达到 `cleanup_fail_threshold`（默认 3）。
  - 不属于 `retain`，也没有管理员 `keep` 固定。
- `probe_error`、`speed_low` 等非可达性失败**不**计入清理阈值。避免把解析器缺陷或阈值调整当成频道失效。
- 写入源 `exclude` 时复用 `subscription::exclude_channels`。键按现有优先级生成：`id:` → `idname:` → `url:`。
- 手工频道、固定频道、`retain` 频道不进入清理候选。
- 清理不可自动执行。每次都必须经过预览与确认。

### 7.3 源可用性

- 抓取失败：沿用现有 `mark_error`，增加 `consecutive_failures`。
- 抓取成功：清零，解除 `auto_disabled`。
- 源合格率：`quality_valid / quality_tested`。
  - 低于 `source_degraded_ratio`（默认 0.5）时标黄「质量下降」。
  - 等于 0 且已测数大于 0 时标红「全部不合格」。
  - 不自动停用。停用需要管理员开启。
- 聚合源：只统计抓取与解析结果。第一版不测频道。

### 7.4 同组去重与候选选择

默认关闭。开启后：

1. 在同一分组内，按 `dedupe=tvg_id` 或 `name` 分组。
   - `tvg_id` 去空白、转小写。
   - `name` 去空白、转小写，并统一全角与半角字符。
   - 没有 `tvg_id` 时回退到 `name`。
2. 每个候选组独立测速。
3. 按 `sort_by` 排序。默认 `speed`。没有测量值或并列时，保持配置顺序。
4. 保留前 `keep_candidates` 个合格项。其余隐藏。
5. `retain` 或管理员 `keep` 的频道不参与淘汰，始终保留。
6. 全部失败且 `supply=true` 时，保留可连通的最优项，标记 `supply`。
7. 没有合格项且 `supply=false` 时，全部按失败处理。

注意：

- 去重只在分组内进行。跨分组同名频道不互相淘汰。
- 去重不修改配置。隐藏项仍在配置中，可随时恢复。
- 用户频道级授权不受影响。授权仍按原频道 id 判定。

### 7.5 播放失败触发复测（可选）

- 播放链路连续失败达到阈值时，把该频道结果标记为 `unreachable`，`next_check_at` 设为当前时间。
- 调度器下一轮优先复测。
- 不修改配置。
- 复用看门狗的失败计数，避免新增计数逻辑。
- 第一阶段可不实现。周期校验已能覆盖大部分情况。

## 8 HTTP 接口

所有接口需要管理员会话。沿用 `PanelAuth`。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/panel/api/quality/status` | 全局开关、运行状态、计数、预算使用。 |
| `GET` | `/panel/api/quality/results` | 结果列表。支持 `gid`、`cid`、`source_id`、`state`、`limit`、`offset`。 |
| `GET` | `/panel/api/quality/history` | 单频道历史。参数 `gid`、`cid`、`limit`。 |
| `POST` | `/panel/api/quality/run` | 发起校验。返回 `202`。 |
| `GET` | `/panel/api/quality/runs/:run_id` | 进度与结果计数。 |
| `POST` | `/panel/api/quality/cancel` | 取消当前任务。 |
| `POST` | `/panel/api/quality/preview` | 按当前结果计算将隐藏的频道。不写状态。 |
| `POST` | `/panel/api/quality/pin` | 固定显示或隐藏。`{"gid","cid","state":"keep"|"hide"|"auto"}`。 |
| `POST` | `/panel/api/quality/cleanup` | 预览或执行清理。`{"apply":false}`。 |

规则保存：

- 全局规则：`PUT /panel/api/config` 或专用设置端点。沿用现有配置保存与 `revision` 校验。
- 源规则：`PUT /panel/api/playlist/sources/:id`。
- 分组规则：`PUT /panel/api/groups/:gid`。
- 频道规则：`PUT /panel/api/groups/:gid/channels/:cid`。

### 8.1 状态响应

```json
{
  "enabled": true,
  "probe_mode": "speed",
  "running": false,
  "run": {
    "run_id": "q-1720000000-1",
    "trigger": "schedule",
    "scope": "all",
    "run_mode": "quick",
    "started_at": 1793500000,
    "tested": 120,
    "total": 120,
    "valid": 98,
    "hidden": 22,
    "bytes": 52428800,
    "budget_bytes": 67108864,
    "status": "done"
  },
  "last_run_at": 1793500120,
  "next_run_at": 1793501020,
  "rules": {
    "min_speed_mib": 1.0,
    "max_delay_ms": 3000,
    "min_resolution": "1280x720",
    "max_resolution": "3840x2160",
    "action": "hide",
    "stale_keep_secs": 1800
  },
  "ffprobe": {
    "installed": false,
    "enabled": false,
    "ready": false,
    "note": "未安装 ffprobe；分辨率识别不可用"
  }
}
```

### 8.2 发起校验

```json
{
  "scope": "group",
  "id": "sports",
  "run_mode": "quick",
  "force": false
}
```

| 字段 | 说明 |
| --- | --- |
| `scope` | `all` / `source` / `group` / `channel`。 |
| `id` | 非 `all` 时必填。`channel` 用 `gid/cid`。 |
| `run_mode` | `quick` / `full`。省略时按 `quick`。 |
| `force` | 忽略 `next_check_at` 与退避。`quality.enabled=false` 时也允许一次干跑。 |

`force` 与总开关：

- `quality.enabled=false` 且 `force=false` → `400 quality_disabled`。
- `quality.enabled=false` 且 `force=true` → 允许探测，结果写库供预览，但订阅渲染仍不隐藏。
- 总开关关闭时，源自动停用、清理、同步门控都不执行。干跑只产生结果，不产生副作用。

返回：

```json
{ "ok": true, "run_id": "q-1720000000-1", "queued": 120 }
```

状态码：

| 情况 | 状态码 |
| --- | --- |
| 已排队 | `202` |
| 已有任务在运行 | `409 run_active` |
| 范围不存在 | `404` |
| 参数不合法 | `400` |
| `quality.enabled=false` 且非 `force` | `400 quality_disabled` |

### 8.3 结果响应

```json
{
  "items": [
    {
      "gid": "sports",
      "cid": "c1",
      "name": "体育 1",
      "source_id": "src-a",
      "checked_at": 1793500000,
      "ok": true,
      "reachable": true,
      "reason": "pass",
      "http_status": 200,
      "delay_ms": 120,
      "speed_mib": 2.45,
      "resolution": "1920x1080",
      "fps": 50.0,
      "video_codec": "h264",
      "audio_codec": "aac",
      "first_fail_at": null,
      "last_ok_at": 1793500000,
      "skipped_rules": [],
      "cached": false,
      "hidden": false,
      "pinned": "auto",
      "stale": false
    }
  ],
  "total": 1
}
```

- 地址不在列表中返回。避免令牌泄漏。需要查地址时由管理员会话单独查看。
- `hidden` 按当前规则计算。规则改动后立即反映。
- `stale` 表示结果超过 2 个周期。只影响显示。

## 9 前端

### 9.1 设置页

新增「可用性校验」卡片：

- 总开关。默认关闭。关闭时说明「不影响现有健康探测」。
- 探测档位：连通 / 清单 / 测速 / 完整。
- 周期、超时、网络并发、`ffprobe` 并发。
- 单轮流量预算、单轮频道上限。
- 失败宽限。
- 全局阈值：最低速率、最大延迟、最低/最高分辨率、分辨率速率映射、最低帧率。
- 默认动作：隐藏 / 标记 / 关闭。
- 广告过滤、`ffprobe`、同 Host 缓存、播放繁忙时延后。
- `ffprobe` 安装状态。
- 「立即校验全部」按钮与进度。

交互规则：

- 低速档位隐藏不需要的字段。例如 `reachability` 不显示速率阈值。
- 保存前做本地校验。分辨率格式错误时阻止保存。
- 修改规则后立即影响订阅渲染。渲染使用「当前规则 + 最近结果」。
- 提示文案写「已按现有结果重算；测量值将在下一轮探测更新」。不要写「下一次校验生效」。

### 9.2 分组页

分组卡：

- 新增「合格 12 / 15」徽章。
- 规则来源徽章：`全局` / `源：A` / `分组` / `频道`。点开显示有效规则。
- 分组级「质量」按钮，打开覆盖规则抽屉。

频道行：

- 保留现有状态点。
- 新增质量徽章：
  - 延迟：`120ms`。
  - 速率：`2.4 MiB/s`。
  - 分辨率：`1080p`。
  - 结果：`合格` / `速率低` / `不可达` / `探测错误` / `已保留`。
- 悬停显示完整结果：状态码、采样量、编码、最近校验时间、原因。
- 操作：单频道「立即校验」、「固定保留」、「固定隐藏」。
- 隐藏频道仍显示。加「已隐藏」徽章与原因，便于恢复。

频道编辑弹窗：

- 新增「质量规则」区。
- 每个字段有「继承」开关。显示继承值。
- 动作三选一：继承 / 隐藏 / 标记 / 关闭。

### 9.3 订阅源页

源列表：

- 新增合格率与最近校验时间。
- 失败或降级时显示徽章：`抓取失败` / `质量下降` / `自动停用`。
- 自动停用时显示原因与「恢复」按钮。

源抽屉：

- 新增「质量规则」分区：启用、动作、阈值、保留策略。
- 显示有效规则与来源层级。
- 「测试该源」按钮。只测该源的频道。
- 自动停用设置：连续失败阈值。默认关闭。

### 9.4 我的订阅页

- 与订阅清单使用同一隐藏判定。
- 被隐藏的频道不显示。页面顶部提示「N 个频道因质量规则隐藏」。
- 管理员可点提示跳到分组页查看。

### 9.5 系统页

- 显示最近一次校验任务、耗时、下载量、预算使用。
- 显示 `ffprobe` 状态。

## 10 调度与低配

### 10.1 调度

- 启动后等待 60 秒再开始第一轮。
- 每 60 秒检查一次是否到期。
- 全局 `quality.enabled=false` 时不做任何事。
- 单实例运行。同一时间只允许一个校验任务。
- 轮次：
  1. 选择 `quality.enabled` 的分组与频道。
  2. 跳过 `next_check_at > now` 的频道。
  3. 跳过 `quality.enabled=false` 的范围、归档组、`probe_passthrough=false` 的直通组。
  4. `retain` 频道照常探测，但永不隐藏。
  5. 按 `next_check_at` 升序排序，保证公平轮转。
  6. 应用 `max_channels_per_run`。
  7. 执行探测。
  8. 更新结果、源状态、下一轮时间。
- 手动任务：
  - 已有校验任务时返回 `409 run_active`。不排队等待前一个校验任务。
  - 可选择 `force` 忽略退避。
  - 可取消。取消会停止当前任务，并清空等待同步结束的队列。
- 同步与校验：
  - 同步进行时，周期校验推迟一轮。
  - 手动校验在同步期间进入队列。队列长度 1。同步结束后自动开始。
  - 同步结束后若队列中有任务，先执行手动任务，再恢复周期调度。
  - `interval_secs=0` 时不自动运行。手动任务不受影响。
- 录制与播放：
  - `defer_during_playback=true` 且在线观众大于 0 时，并发降为 1。
  - 有录制任务运行时，周期校验延后。手动校验照常，但并发降为 1。

### 10.2 退避

| 连续失败 | 下次校验 |
| --- | --- |
| 1 | 下一轮 |
| 2 | 下一轮 |
| 3 | `max(周期, 8 分钟)` |
| 4 | `max(周期, 16 分钟)` |
| 5 | `max(周期, 32 分钟)` |
| … | 上限 24 小时 |

- 退避不短于正常周期。避免失败频道比健康频道测得更勤。
- 成功后清零，下一轮按正常周期。
- 管理员手动 `force` 忽略退避。
- 结果持久化。重启后继续使用 `next_check_at`。

### 10.3 低配默认值

| 项 | 低配默认 | 说明 |
| --- | --- | --- |
| `enabled` | false | 默认不增加负载。 |
| `probe_mode` | `reachability` | 与现有健康探测等价。 |
| `interval_secs` | 900 | 15 分钟。 |
| `concurrency` | 2 | 128 MB 机器可用。 |
| `probe_concurrency` | 1 | `ffprobe` 并发与采集上限。 |
| `ffprobe` | false | 需要画质筛选时再开。 |
| `max_ffprobe_per_run` | 32 | 低配限制单轮进程数。`0` 表示不限。 |
| `ffprobe_min_speed_mib` | 0.5 | 速率低于此值不启动进程。 |
| `ffprobe_timeout_secs` | 5 | 单次进程超时。范围 1–30。 |
| `ffprobe_sample_kb` | 512 | 送给 `ffprobe` 的字节上限。范围 64–2048。 |
| `sample_segments` | 1 | 低配只采样 1 个切片。 |
| `history_mode` | `changes` | 只在状态变化时写历史。 |
| `budget_mb_per_run` | 64 | 限制单轮下载。 |
| `max_channels_per_run` | 0 | 由预算与超时自然限制。 |
| `stale_keep_secs` | 1800 | 两次失败后才下线，避免抖动。 |
| `sample_bytes_kb` | 4096 | 单流 4 MiB。 |
| `defer_during_playback` | true | 避免抢带宽。 |

建议：

- 首次启用先用 `reachability` 或 `mark` 动作观察一周。
- 需要测速时，把周期设为夜间低频，例如 6 小时一次。
- 大清单使用 `max_channels_per_run` 分批轮转。不要一次测完。
- 不把 `budget_mb_per_run` 设得过大。测速流量会进入服务器出网账单。

### 10.4 资源占用模型

目标：在 1 vCPU / 128 MB / 1 GB 磁盘上可运行；不抢播放带宽；单轮资源有上限。

#### 10.4.1 内存

| 项 | 估算 | 说明 |
| --- | --- | --- |
| 单频道快照 | 约 64 B + 键与哈希开销，合计约 130–150 B | 紧凑结构，不含编码与完整分辨率字符串。 |
| 2 万频道快照 | 约 3–5 MB | 订阅渲染只读该快照。 |
| 2 万频道健康条目 | 约 2–4 MB | 现有 `Health` 结构。 |
| 单轮播放清单缓冲 | 最多 `concurrency × max_playlist_kb`，默认约 4 MB | 解析后立即释放。 |
| 测速缓冲 | 不整段缓存。边读边计数，峰值约 `concurrency × 64 KB` | 禁止把 4 MiB 样本放进内存。 |
| 分辨率采集缓冲 | 不超过 `probe_concurrency × ffprobe_sample_kb` | 默认 512 KiB。最大 8 MiB。 |
| `ffprobe` 输出 | 小于 64 KB/次 | 只读 JSON 摘要。 |

约束：

- 完整结果按 `gid/cid` 从 SQLite 按需查询。面板列表分页，默认 100 条。
- 历史查询必须带 `gid/cid` 或时间窗。禁止一次拉全表。
- 结果表不保留请求头、完整清单或媒体样本。

#### 10.4.2 CPU

| 项 | 成本 | 控制 |
| --- | --- | --- |
| HTTP 读取与哈希 | 低 | 只计算一次 `url_hash`。 |
| M3U8/MPD 解析 | 低 | 单清单上限 2 MiB。避免逐切片正则。广告关键字用 `contains`。 |
| 判定 | 极低 | 纯内存比较。 |
| `ffprobe` | 高。单进程约 20–50 MB RSS | 默认关闭。只在速率达标后运行。并发默认 1。单轮次数可限。超时杀进程。字节经标准输入传递，不访问网络。 |
| SQLite 写入 | 中 | 批量事务。只写变更行。`history_mode=changes` 时不重复写。 |

`ffprobe` 的额外门槛：

- `ffprobe_min_speed_mib` 默认 `0.0`。启用测速筛选时建议设为最低速率，例如 `0.5`。
- 清单已有完整分辨率时，不启动进程。
- 达到 `max_ffprobe_per_run` 后停止启动新进程。已完成的结果保留。

#### 10.4.3 带宽

单频道估算：

| 档位 | 请求 | 单频道估算 | 说明 |
| --- | --- | --- | --- |
| `reachability` | 1 次 | 几 KB 到 64 KB | 只读首字节或状态。 |
| `playlist` | 2–3 次 | 约 50–300 KB | 清单 + 首个切片探测。 |
| `speed` | 2–5 次 | 约 1–8 MB | 默认 2 个切片，每片上限 4 MiB。 |
| `full` | 同 `speed` | 约 1–8 MB | 分辨率识别复用已下载字节，不增加请求。 |

单轮预算：

```text
per_channel_mb = 该档位最近一次实测平均值
limit_by_budget = budget_mb_per_run / per_channel_mb
limit_by_count  = max_channels_per_run（0 表示不限）
本轮频道数 = min(到期频道数, limit_by_budget, limit_by_count)
```

- 没有实测值时，用上表上限估算。首轮保守减半。
- 每轮记录实际 `bytes`。面板显示「本档位平均每频道 MB」。
- 达到预算 90% 后不再启动新的测速或 `full` 任务。已开始的请求允许完成。
- 预算耗尽后，本轮结束。剩余频道按 `next_check_at` 排到下一轮。

推荐周期：

| 档位 | 默认周期 | 1000 频道每天量级 | 建议 |
| --- | --- | --- | --- |
| `reachability` | 900 秒 | 几十 MB 到数 GB | 首轮观察。大清单可放宽到 30–60 分钟。 |
| `playlist` | 3600 秒 | 数 GB | 大清单建议 6 小时。 |
| `speed` | 21600 秒 | 数十 GB | 建议夜间执行，或只测重点分组。 |
| `full` | 43200 秒或手动 | 数十 GB + CPU | 默认手动。 |

说明：

- 上表按 1000 频道、每轮全量估算。实际值以面板实测为准。
- 频道超过 2000 时，不建议对全部频道开启 `speed` 或 `full`。
- 大清单优先用 `reachability` 或 `playlist` 发现不可达，再对重点分组做测速。
- 单轮预算与周期共同决定完整轮转时间。面板显示「预计 X 天覆盖全部频道」。

#### 10.4.4 磁盘

| 数据 | 估算 | 保留 |
| --- | --- | --- |
| `quality_results` | 约 300–500 B/频道 | 当前地址一条。 |
| `quality_history` | 约 150–300 B/行 | `history_mode=changes` 时约每频道每天 0–2 行。 |
| 2 万频道结果表 | 约 6–10 MB | 含索引。 |
| 2 万频道历史 7 天 | 约 20–80 MB | 取决于状态变化次数。 |
| WAL | 上限 4 MB | 沿用现有 `journal_size_limit`。 |

约束：

- 结果与历史写入同一个 SQLite 文件。沿用现有 WAL 与 `busy_timeout`。
- 批量事务：每 200 行或每 5 秒提交一次。
- 清理任务每小时运行一次。删除过期历史与孤儿结果。
- 备份不包含结果表。恢复后重新探测。

#### 10.4.5 与播放、同步、录制的协作

| 运行中任务 | 周期校验 | 手动校验 |
| --- | --- | --- |
| 有在线播放 | 并发降为 1；每任务之间暂停 200 ms | 并发降为 1 |
| 有定时录制 | 本轮延后 | 允许，并发降为 1 |
| 订阅同步 | 本轮延后 | 排队，同步结束后运行 |
| 缓存清理 | 不冲突。可并行 | 不冲突 |
| 已有校验 | 跳过 | `409 run_active` |

自适应降级：

- 每完成一个频道，检查在线观众数与录制状态。条件变化时立即调整并发。
- 播放开始后，不再启动新的 `speed` 或 `full` 任务。`reachability` 可继续。
- 连续两轮因播放繁忙被延后时，下一轮至少执行 `reachability`，避免长期不更新。
- 预算与并发变更在下一轮生效。运行中的任务不改参数。

#### 10.4.6 可观测

`/panel/api/quality/status` 增加：

| 字段 | 含义 |
| --- | --- |
| `run.bytes` | 本轮已下载字节。 |
| `run.peak_concurrency` | 本轮实际使用的最大并发。 |
| `run.ffprobe_count` | 本轮启动的 `ffprobe` 次数。 |
| `run.deferred_reason` | 本轮被延后或降级的原因。 |
| `averages.bytes_per_channel` | 最近一轮各档位每频道平均下载量。 |
| `averages.duration_ms_per_channel` | 最近一轮每频道平均耗时。 |

面板根据 `averages` 给出建议：

- 当前预算下每轮能测多少频道。
- 按当前周期覆盖全部频道需要多久。
- 是否应降低档位或延长周期。


## 11 安全与限制

| 项 | 规则 |
| --- | --- |
| 协议 | 只允许 `http` 与 `https`。其他协议直接拒绝。 |
| SSRF | 沿用现有源抓取策略。第一版不封锁内网地址，但面板警告。 |
| 重定向 | 上限 5 次。最终地址必须仍是 HTTP(S)。 |
| 正文上限 | 清单 2 MiB。连续流按采样上限。 |
| 超时 | 单频道总超时。子进程也受同一超时约束。 |
| 请求头 | 不写入结果表。日志只记数量，不记值。 |
| 地址 | 面板列表不返回完整地址。日志对查询参数与用户信息脱敏。 |
| Cookie | 不使用浏览器 Cookie。每次探测独立。 |
| 并发 | 全局信号量。同 Host 最多 2 个连接。 |
| 带宽 | 单轮预算。达到后停止发起新任务。 |
| 内存 | 内存只存紧凑快照。禁止缓存媒体样本与完整清单。 |
| 磁盘 | 结果与历史按 `history_mode` 写入。每小时清理过期行。 |
| 进程 | `ffprobe` 有并发上限、单轮次数上限与超时。超时杀进程。 |
| 播放优先 | 播放或录制繁忙时降低并发或延后。 |
| 源站压力 | 同一频道的退避。避免短时间重复打源站。 |
| `ffprobe` | 只执行固定参数。不拼接用户输入到 shell。使用参数数组。 |
| 取消 | 取消后终止网络请求与子进程。保留已完成结果。 |

## 12 测试

### 12.1 单元测试

| 模块 | 用例 |
| --- | --- |
| 规则解析 | 频道 > 分组 > 源 > 全局的逐字段覆盖。三态 `enabled`。 |
| 判定 | 每个 `reason` 的边界。分辨率未知放行。`max_resolution` 包含上限。`resolution_speed_map` 覆盖 `min_speed_mib`。 |
| 动作 | `hide` / `mark` / `off` / `retain` / `supply` / 固定保留。 |
| 宽限 | `first_fail_at` 固定。重复失败不重置。超过 `stale_keep_secs` 隐藏。成功后清零并恢复。 |
| 退避 | 连续失败后的 `next_check_at`。成功清零。 |
| 地址 | `url_hash` 不一致时旧结果按 `unknown`。新地址首次探测前可见。 |
| 健康映射 | 阈值失败不改变 `Health.Entry.ok`。只有 `unreachable` 写 `ok=false`。 |
| 测速 | 速率与延迟计算。字节上限、时长上限、稳定窗口。 |
| 广告 | `ENDLIST` 短循环。关键字命中。正常清单不命中。 |
| 去重 | `tvg_id` / `name` 同组淘汰。跨组不淘汰。`keep_candidates`。 |
| 源状态 | 自动停用阈值。成功恢复。不修改配置。 |
| 调度 | 校验运行中再次手动触发 → `409`。同步中手动触发 → 队列长度 1。 |
| 清单过滤 | `render_user`、`render_user_group`、`user_view` 三处结果一致。 |

### 12.2 集成测试

用本地 HTTP 服务提供固定样本：

| 样本 | 期望 |
| --- | --- |
| 正常 HLS 主清单 + 媒体清单 | `pass`，有速率与分辨率。 |
| 4K 主清单，规则上限 1080p，且含 1080p 变体 | 选择 1080p 变体，`pass`。 |
| 4K 主清单，规则上限 1080p，且只有 4K 变体 | `resolution_high`。 |
| 低于最低速率的切片 | `speed_low`。 |
| 404 / 连接拒绝 | `unreachable`。 |
| 重定向链 | 跟随到最终地址。超过上限 → `probe_error`。 |
| 空正文 | `unreachable`。 |
| 损坏的 m3u8 | `probe_error`，不隐藏。 |
| 短循环广告清单 | `ad`。 |
| 无清单的连续流 | 按采样测速。 |
| `ffprobe` 缺失 | 仍可判定，分辨率保持未知，使用兜底速率。 |
| `ffprobe` 关闭且规则要求帧率 | 规则记入 `skipped_rules`，结果不隐藏。 |
| `ffprobe` 可用 | 补全分辨率。不识别帧率与编码。 |
| fMP4 切片无初始化段 | 分辨率保持未知，使用兜底速率，不记 `probe_error`。 |
| 同 Host 两个频道，其中第二个 404 | 只复用测量值，第二个仍为 `unreachable`。 |
| 预算耗尽 | 停止后续任务，已完成结果保留。 |
| `ffprobe` 门槛 | 速率低于 `ffprobe_min_speed_mib` 时不启动进程。 |
| `ffprobe` 单轮上限 | 达到 `max_ffprobe_per_run` 后停止启动进程。 |
| 历史模式 | `changes` 不重复写同一状态。`hourly` 每小时最多一行。 |
| 低配降级 | 有在线播放时并发降为 1，且不再启动 `speed` 任务。 |
| 取消 | 停止网络请求与子进程。 |
| 重启 | 结果从 SQLite 恢复。 |

### 12.3 冒烟脚本

旧 `scripts/smoke-quality.sh` 已由质量准入验收脚本取代：

- `scripts/smoke-quality-admission.sh`
- 场景见 [`15-quality-admission-function.md`](15-quality-admission-function.md) §13。

## 13 兼容与迁移

| 项 | 处理 |
| --- | --- |
| 旧配置 | 新字段都有默认值。`quality.enabled=false`。 |
| 旧 `[health]` | 保持不变。质量关闭时按原逻辑运行。 |
| `offline_on_fail` | 保持不变。与质量规则叠加。 |
| `/panel/api/health` | 原字段不变。新增字段为附加项。 |
| 订阅清单 | 质量关闭时字节级不变。 |
| 备份 | 配置进备份。结果表不进备份，恢复后重新探测。 |
| 降级 | 旧二进制忽略 `quality` 段，写入 `extra`，保存时保留。 |
| 删除频道 | 配置变更后清理对应结果。避免孤儿记录。 |
| 地址变更 | 新地址使用新 `url_hash`。旧结果保留到清理周期。 |
| 时钟 | 用 Unix 秒。重启与休眠后仍可比较。 |

## 14 实施阶段

| 阶段 | 内容 | 交付判据 |
| --- | --- | --- |
| P0 | 配置模型、校验、有效规则解析、单元测试 | ✅（本计划）规则覆盖与三态行为通过测试。默认关闭。 |
| P1 | 探测器（连通、清单、测速）、内存结果、订阅过滤、状态接口、分组页徽章 | ✅（本计划）手动校验可用。不合格频道可隐藏与恢复。 |
| P2 | SQLite 结果与历史、源状态、自动停用、设置页与源抽屉 | 重启保留结果。源失败可停用与恢复。 |
| P3 | `ffprobe`、广告、去重、固定保留、清理预览、冒烟脚本 | 完整规则可用。文档与冒烟通过。 |
| P4 | 低配调度、预算、播放繁忙降级、播放失败复测 | 1 vCPU / 128 MB 下可运行。 |

## 15 决策状态

| 编号 | 决策点 | 结论 | 状态 |
| --- | --- | --- | --- |
| Q1 | 是否新建 `[quality]` | 是。`[health]` 保持轻量 | 已定（用户选择方案 B） |
| Q2 | 默认动作 | `hide` + 1800 秒宽限（两倍周期）。沿用「只影响清单」的既有下线语义 | 已定 |
| Q3 | 默认开启 | 否 | 已定 |
| Q4 | `probe_error` 是否隐藏 | 否。按未知处理 | 已定 |
| Q5 | 源自动停用是否改写配置 | 否。运行时标记，可一键恢复 | 已定 |
| Q6 | 是否第一版就做同组去重 | 否。P3 再做 | 已定 |
| Q7 | 是否第一版校验聚合源 | 否。P3 再做。第一版覆盖托管源与手工频道 | 已定（用户确认） |
| Q8 | `ffprobe` 默认值 | 关闭。关闭时是硬上限 | 已定 |
| Q9 | 直接播放是否拦截隐藏频道 | 否。隐藏只影响订阅渲染 | 已定 |
| Q10 | 清理失效频道 | 只做手动预览 + 确认。P3 交付 | 已定 |

## 16 实现落点

| 文件 | 内容 |
| --- | --- |
| `src/config/model.rs` | `QualityCfg`、`QualityRules`、`QualityOverride`、`Group.quality`、`Channel.quality`、`PlaylistSource.quality`、默认值、`normalize`。 |
| `src/config.rs` | 规则校验：分辨率格式、数值范围、枚举值、`resolution_speed_map`。 |
| `src/quality.rs` | 有效规则解析、判定、探测器、调度、内存状态。 |
| `src/quality_store.rs` | SQLite 表、结果与历史读写、清理、启动恢复。 |
| `src/health.rs` | 复用 `Entry` 结构。质量启用时让出调度。 |
| `src/playlist.rs` | `hidden` 判定。`render_user`、`render_user_group`、`user_view` 共用。 |
| `src/subscription.rs` | 同步后排队校验。可选同步门控。清理候选。 |
| `src/subs.rs` | `SourceStatus` 新字段。自动停用判定。 |
| `src/source_cache.rs` | 持久化新状态字段。 |
| `src/state.rs` | `quality` 状态、运行锁、句柄。 |
| `src/main.rs` | 启动质量调度器。 |
| `src/http.rs` | 新增 `/panel/api/quality/*` 路由。 |
| `src/http/routes/quality.rs` | 状态、结果、历史、运行、取消、预览、固定、清理。 |
| `src/http/routes/groups.rs` | 分组与频道规则保存后清理旧结果。 |
| `src/http/routes/playlist.rs` | 源规则保存后重新计算源状态。 |
| `frontend/src/settings.tsx` | 全局校验设置卡片。 |
| `frontend/src/groups.tsx` | 分组合格率、频道徽章、单频道校验、覆盖规则、固定保留。 |
| `frontend/src/playlist.tsx` | 源合格率、源规则、自动停用状态、测试源。 |
| `frontend/src/system.tsx` | 最近任务与 `ffprobe` 状态。 |
| `frontend/src/api.ts` | 质量接口类型。 |
| `scripts/smoke-quality-admission.sh` | 质量准入验收脚本。 |
| `tests/` | 单元与集成样本。 |
