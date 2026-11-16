# 16 · 订阅源质量准入系统设计

> 阶段：已实施（2026-11-16）。依据 [`14-quality-admission-product.md`](14-quality-admission-product.md) 与 [`15-quality-admission-function.md`](15-quality-admission-function.md)。
> 本文定义存储、接口、调度、并发与事务。
> 基线：`5627921`（P0/P1 已实现，本设计替换其成员语义）。
> 验收：`scripts/smoke-quality-admission.sh` → `smoke-quality-admission: PASS`。

## 实施状态

- 后端：配置、结果存储、事务提交、同步门控、周期/手动引擎、安全阀、源停用/删除、质量接口。
- 前端：独立「质量校验」页；规则、结果、来源三个标签；分组页徽章跳转。
- 手工频道：默认失败保留；`offline_on_fail=true` 时超过宽限后从订阅隐藏。
- 重启：启动时从结果表水合手工频道状态。
- 容器验收：正常、慢速、404 三频道；门控准入、周期移出、安全阀、源停用/删除均通过。
- 低配内存：容器实测低于 128 MB。

## 1 总体结构

```
配置（[quality] + 源/分组/频道覆盖）
        │
        ▼
规则解析（现有 quality::rules）
        │
        ├──────────────┐
        ▼              ▼
同步门控           周期/手动校验
（sync_locked）     （quality 调度器）
        │              │
        └──────┬───────┘
               ▼
        探测引擎（现有 probe + 单轮缓存）
               │
               ▼
        判定与准入决策
        ├─ 同步源：合格准入，不合格移出
        ├─ 手工：保留或下线
        └─ 安全阀：暂停成员变更
               │
               ▼
        原子提交
        ├─ config.groups
        ├─ quality_results
        └─ source status / quality_meta
               │
               ▼
        订阅渲染与面板
```

## 2 现有代码取舍

| 模块 | 处理 | 说明 |
| --- | --- | --- |
| `quality::rules` | 保留并扩展 | 规则继承与判定复用。 |
| `quality::probe` | 保留 | 继续使用流式读取与内存上限。 |
| `quality::state` | 保留并扩展 | 运行状态、手动下线结果。 |
| `quality::schedule` | 重写 | 改为准入引擎 + cron/小时调度。 |
| `playlist::quality_hidden` | 调整 | 只处理手工频道下线；同步频道失败即移出。 |
| `health` | 保留兼容 | 质量停用时继续按健康探测下线手工频道。 |
| `subscription::sync_locked` | 扩展 | 应用前调用同步门控。 |
| `SourceStatus` | 扩展 | 增加门控阶段与统计。 |
| 前端 `quality.tsx` | 扩展 | 增加完整质量页；保留徽章组件。 |

## 3 配置模型

### 3.1 新增字段

`QualityCfg` 增加：

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `schedule` | string | `interval` | `interval` 或 `cron`。 |
| `interval_hours` | u64 | `6` | 1–8760。 |
| `cron` | string | `0 */6 * * *` | 5 段表达式。 |
| `guard_enabled` | bool | `true` | 安全阀开关。 |
| `max_remove_ratio` | f64 | `0.5` | 将移出数 / 完成判定数。 |
| `min_remove_count` | u64 | `10` | 小于该数量不触发。 |
| `result_retention_days` | u64 | `7` | 0 = 不自动清理。 |
| `gate_time_budget_secs` | u64 | `300` | 同步门控墙钟上限。0 = 不限制。 |

### 3.2 迁移

- 保留 `interval_secs` 字段，避免旧配置解析失败，但新调度不再读取。
- 新配置默认 `interval_hours=6`。旧配置首次保存后写入新字段。质量控制尚未发布，不做秒到小时的自动换算。
- `rules.action`、`rules.retain`、`rules.supply` 保留解析，但不再控制准入。
- `quick_target`、`sort_by`、`defer_during_playback`、`filter_ad` 保留解析，但新引擎不读取。
- `rules.stale_keep_secs` 只控制手工频道下线宽限。
- 手工频道失败处理使用现有 `offline_on_fail`：
  - `false` = 保留。
  - `true` = 超过宽限后下线。宽限读 `stale_keep_secs`，默认 1800 秒；0 表示立即。
- 同步源门控关闭使用现有 `source.quality.enabled=false`。

### 3.3 有效规则

- 同步门控先用 `source_id + source_key` 找现有频道。
- 找到时使用该频道的实际分组与频道覆盖：频道 > 分组 > 源 > 全局。
- 找不到时按新候选处理：目标分组 > 源 > 全局。目标分组覆盖由 `plan()` 预先解析并写入 `PlannedGroup.quality`。
- `plan()` 与 `apply()` 共用一个 `find_target_group`，避免两套目标分组匹配。
- 周期与手动校验使用现有完整继承：频道 > 分组 > 源 > 全局。
- 解析结果附带 `source_layer`（`global` / `source` / `group` / `channel`），供接口与页面显示。

### 3.4 校验

- `interval_hours` 范围 1–8760。
- `cron` 必须能解析。
- `max_remove_ratio` 范围 `(0, 1]`。
- `min_remove_count` 范围 1–100000。
- `result_retention_days` 范围 0–3650。
- `gate_time_budget_secs` 范围 0–86400。
- 最低分辨率不得高于最高分辨率。

## 4 存储

### 4.1 表结构

新增 `quality_results`：

```sql
CREATE TABLE IF NOT EXISTS quality_results (
    key TEXT PRIMARY KEY NOT NULL,
    scope TEXT NOT NULL,
    source_id TEXT NOT NULL DEFAULT '',
    source_key TEXT NOT NULL DEFAULT '',
    gid TEXT NOT NULL DEFAULT '',
    cid TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    target_group TEXT NOT NULL DEFAULT '',
    url_hash TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    ok INTEGER NOT NULL DEFAULT 0,
    delay_ms INTEGER,
    speed_kib INTEGER,
    width INTEGER,
    height INTEGER,
    http_status INTEGER,
    checked_at INTEGER NOT NULL,
    next_check_at INTEGER NOT NULL DEFAULT 0,
    first_fail_at INTEGER,
    rule_hash TEXT NOT NULL DEFAULT '',
    run_id TEXT NOT NULL DEFAULT '',
    updated_at_ms BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS quality_results_checked_idx
    ON quality_results (checked_at);
CREATE INDEX IF NOT EXISTS quality_results_source_idx
    ON quality_results (source_id, checked_at);
CREATE INDEX IF NOT EXISTS quality_results_action_idx
    ON quality_results (action, checked_at);
```

主键规则：

- 同步源：`sync:<source_id>:<source_key>`。一个频道只保留一行，URL 变化时更新同一行。
- 手工频道：`channel:<gid>/<cid>`。删除频道时删除对应结果行。
- `url_hash` 作为普通列，用于判断结果是否对应当前地址。

`action` 取值：

- `admitted`：已准入。
- `removed`：质量失败移出。
- `rejected`：新候选未准入。
- `hidden`：手工频道下线。
- `kept`：手工频道保留。
- `skipped`：预算或取消，未完成判定。
- `held`：判定合格，但因安全阀暂停，本轮未准入。
- `removal_paused`：判定不合格，但因安全阀暂停，本轮未移出。
- `source_removed`：源不再提供该频道，或源删除导致移除。

新增 `quality_meta`：

```sql
CREATE TABLE IF NOT EXISTS quality_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at_ms BIGINT NOT NULL
);
```

键：`last_run_at`、`last_run_id`、`last_run_summary`、`next_run_at`、`guard_tripped_at`。

### 4.2 保留与清理

- 已准入记录随每轮更新，不按保留期删除。
- 非 `admitted` 记录超过 `result_retention_days` 删除。
- `admitted` 记录在频道被移出或源删除时转为对应 `action`，之后按保留期清理。
- 删除手工频道时删除对应 `channel:<gid>/<cid>` 结果行，避免重建同 id 命中旧结果。
- 同步源不再提供频道时写 `source_removed`，不静默丢记录。
- 清理在启动时执行一次，之后每天一次。
- `result_retention_days=0` 时不自动清理。

### 4.3 事务

新增 `AppState::mutate_admission`：

1. 获取配置写锁。
2. 获取 `SharedConn` 锁并开启 SQLite 事务。
3. 读取并解析当前配置。
4. 执行成员变更闭包。
5. 规范化、校验、revision 自增、写回 `settings`。
6. 批量 upsert `quality_results` 与 `quality_meta`。
7. 提交事务。
8. `state.replace(config)` 更新内存快照与频道键集合。

任一步失败则回滚。成员变更与结果记录不会只成功一半。

## 5 判定与准入引擎

### 5.1 单轮缓存

`RunMemo`：

- `url -> Measurement`，键为 URL、请求头、代理与分辨率上限的指纹。
- `rules.host_cache=true` 时，另加 host 键。命中后仍逐频道检查可达性，只复用速率与延迟。
- 只保存测量值，不保存判定。不同规则可复用同一测量。
- 生命周期为一次任务。任务结束即释放。
- 数量上限为本次目标数。不跨轮复用。

### 5.2 同步门控

入口：`quality::gate::filter_plan(state, config, source, plan) -> GateOutcome`。

门控只处理两类频道：

- **新候选**：计划中存在，但配置中按 `source_id + source_key` 找不到。
- **URL 变化**：配置中已存在，且计划 URL 的哈希与当前不同。

`sync_follow=false` 的固定频道不探测。`apply` 不更新或删除这类频道，门控探测只会产生无法执行的结果，并污染安全阀统计。

已准入且未变化的频道不参与同步门控。它们由周期校验负责移出。这样避免同步与周期重复探测，也避免大源每轮都全量测速。

步骤：

1. 判断门控是否启用：全局 `quality.enabled` 且源有效规则 `enabled`。
2. 把计划频道分为三类：新候选、URL 变化、未变化。
3. 先处理 URL 变化频道：
   - 按现有频道的分组、频道与源覆盖解析有效规则。
   - 探测并判定。DASH `.mpd` 不做清单或测速，只做 HTTP 连通检查。
   - 合格：保留在计划，更新地址。
   - 不合格：从计划移除。
   - `probe_error`：暂停本轮同步，不应用任何成员变更。源状态标记 `gate_paused=true`。
   - 时间或预算不足以处理全部 URL 变化频道：暂停本轮同步，不应用任何成员变更。
4. 再处理新候选：
   - 按稳定键排序。
   - 从 `gate_cursor` 之后开始，处理到预算或时间上限。
   - 合格：保留在计划。
   - 不合格或 `probe_error`：从计划移除，记录结果。
   - 截断：记录最后处理的稳定键到 `gate_cursor`，源状态标记 `gate_truncated=true`。已处理结果照常应用。
   - 处理到末尾：清空 `gate_cursor`。
5. 未变化频道原样保留在计划。
6. 重建 `plan.groups`，删除没有频道的计划分组。
7. 计算安全阀：
   - `tested` = 本轮完成判定的 URL 变化频道与新候选数。`probe_error` 与未知不计入。
   - `removals` = URL 变化且不合格的频道数。
   - 数量阈值 = 全部可移出频道都失败且数量达到 3 个时，取 `min_remove_count` 与「源内可移出频道数 × 30%，向上取整」的较小值；否则保持 `min_remove_count`。
   - 条件：`guard_enabled && tested > 0 && removals >= 数量阈值 && removals / tested >= max_remove_ratio`。
8. 安全阀触发：返回 `Paused`，不应用任何成员变更。
9. 未触发：返回过滤后的计划与结果，由 `sync_locked` 原子应用。

截断与恢复：

- 被截断的只有新候选。它们尚未进入配置，不会造成已有频道误删。
- 下一轮同步从 `gate_cursor` 继续。游标不存在的键从开头重新开始。
- 页面显示「本轮准入 N，剩余 M 个候选下一轮继续」。
- `gate_cursor` 是进度状态，不是淘汰池。它不重试已移出频道，也不自动恢复。

定向规则：

- 同步门控不执行解析脚本。候选频道尚未进入配置。
- 使用计划中的频道 headers 与源代理。
- 新候选 `probe_error`：不准入。
- URL 变化 `probe_error`：暂停同步，保留原频道与原地址。

### 5.3 周期与手动校验

入口：`quality::engine::run(state, scope, trigger, measure_only)`。

目标：

- 已准入同步频道。
- 手工频道按范围包含。
- 跳过：全局关闭（measure_only 时不跳过探测，但不改成员）、源停用、源门控关闭、归档组。
- 地址变化被门控暂停且新地址尚未写入时，周期校验不按旧地址重测。
- 固定频道（`sync_follow=false`）照常探测，但失败只记 `kept`，不移出，也不计入安全阀。

步骤：

1. 选目标，按 `next_check_at` 与预算截断。
2. 探测并判定。频道配置了解析脚本时，先执行脚本；脚本失败记 `probe_error`。
3. 同步频道不合格：收集为移出候选。固定频道除外。
4. 手工频道不合格：收集为下线候选；合格则清除下线状态。
5. 安全阀只统计可移出的同步频道。`probe_error`、未知、固定频道不计入分母或移出数。
6. 触发安全阀：不执行成员变更。结果动作写 `removal_paused`（失败未移出）或 `held`（合格但未准入）。
7. 未触发：`mutate_admission` 执行：
   - 从 `config.groups` 删除移出候选。
   - 更新 `quality_results`。
   - 更新手工频道 `QualityState`。
   - 更新 `quality_meta`。
8. 移出前复核：频道仍存在、URL 哈希未变、源仍启用、源门控仍开启、频道仍跟随上游。否则跳过并记录 `skipped` / `source_changed`。

取消：

- 每个分块前检查取消标志。
- 已完成判定照常处理并提交。
- 未开始频道记录 `skipped`。
- 取消不回滚已提交的移出。

启动水合：

- 启动时读取 `quality_results` 中 `scope=manual` 的最近结果。
- 把 `hidden` / `kept` 动作转换为 `QualityState` 条目。
- 保证重启后手工频道下线状态不丢失。

### 5.4 安全阀

- 关闭时不触发安全阀；移出照常执行。
- 触发时不改变成员，包括新候选准入。
- 结果记录照常写入。
- 数量阈值按可移出频道数自适应：3 个及以上且全部失败时取 `min_remove_count` 与 30% 的较小值；部分失败仍按原阈值。
- `quality_meta.guard_tripped_at` 与 `last_run_summary` 记录。
- 管理员关闭安全阀或调整阈值后重新运行。

### 5.5 手工频道

- 使用 `offline_on_fail_for(gid, cid)`：
  - `true`：失败超过 `stale_keep_secs` 后写入 `QualityState`，订阅渲染下线。
  - `false`：失败也保留。
- 首次失败只记录 `first_fail_at`；宽限内恢复则清除。
- `stale_keep_secs=0` 时立即下线。
- 不提供移出。
- 启动时从结果表水合，重启不丢失。

### 5.6 订阅渲染与优先级

| 状态 | 同步频道 | 手工频道 |
| --- | --- | --- |
| 质量启用 + 源门控开 | 准入/移出 | 质量结果决定保留或下线 |
| 质量启用 + 源门控关 | 同步直接管理 | 质量结果决定保留或下线 |
| 质量停用 | 同步直接管理 | 现有健康探测决定下线 |

- 同步频道失败时已由任务提交移出，分组中只应存在合格频道。保留防御性检查：结果与 URL 哈希不匹配时不隐藏。
- 手工频道质量启用时读 `QualityState`；质量停用时读 `Health`。
- 两条路径不得同时生效，避免重复隐藏。
- `retain`、`rules.action` 不再参与新判定。

## 6 调度

### 6.1 cron 模块

从 `epg.rs` 抽取共享 `src/cron.rs`：

- `parse(expr)`。
- `matches(spec, epoch, offset_hours)`。
- `prev(expr, now, offset_hours)`。
- `preview(expr, now, offset_hours, count)`。

EPG 保持现有公开函数行为，内部改为调用共享模块。质量调度使用同一套解析，避免两种 cron 行为。

### 6.2 周期模式

`interval`：

- 到期条件：`last_run_at + interval_hours*3600 <= now`。
- 没有 `last_run_at` 时，启动后等待 60 秒执行首轮。

`cron`：

- 到期条件：`last_run_at < cron_prev(now)`。
- 启动时若错过触发，执行一次，然后计算下一次。
- `next_run_at = cron_preview(..., 1)`。

### 6.3 调度循环

- 启动后等待 60 秒。
- 每 30 秒检查一次。
- 到期且无任务运行：启动周期任务。
- 有任务运行：跳过本轮，不排队。
- 任务结束后写 `last_run_at` 与 `next_run_at`。
- 全局关闭或 `schedule` 非法：不启动，状态页显示原因。

### 6.4 单轮范围

- 每轮按 `next_check_at` 选择到期频道。
- `budget_mb_per_run`、`max_channels_per_run`、`concurrency` 沿用现有上限。
- 一轮未覆盖的频道保持原 `next_check_at`，下一轮优先。

## 7 并发与锁

锁顺序固定：

```
sync_lock → admission_lock → config.write_lock → SharedConn
```

- `sync_lock`：现有全局同步锁。
- `admission_lock`：新增，保护门控与质量任务。
- `config.write_lock`：现有配置写锁。
- `SharedConn`：数据库连接锁。

规则：

- 同步门控在 `sync_lock` 内获取 `admission_lock`，再探测，最后进入 `mutate_admission`。
- 周期/手动任务获取 `admission_lock`；拿不到时返回忙碌或等待。
- 源停用/删除只拿 `config.write_lock`，不拿 `admission_lock`。
- 质量任务提交前复核源状态与频道 URL，避免与停用/删除竞争。
- 任何路径不得先拿 `write_lock` 再拿 `admission_lock`。

## 8 HTTP 接口

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/panel/api/quality/status` | 开关、周期、下次运行、运行状态、统计、源错误、安全阀。 |
| GET | `/panel/api/quality/results` | 结果分页与筛选。 |
| GET | `/panel/api/quality/sources` | 同步源门控统计。 |
| GET | `/panel/api/quality/effective` | 有效规则与来源层级。参数 `source_id`、`gid`、`cid`。 |
| POST | `/panel/api/quality/run` | 发起校验。 |
| GET | `/panel/api/quality/runs/:run_id` | 任务进度。 |
| POST | `/panel/api/quality/cancel` | 取消当前任务。 |
| POST | `/panel/api/quality/preview` | 按最近结果预览将移出频道。不写状态。 |
| POST | `/panel/api/quality/schedule/preview` | 校验 cron，返回未来 3 次运行时间。 |
| POST | `/panel/api/playlist/sources/:id/disable` | 停用源，body `{keep_channels}`。 |
| POST | `/panel/api/playlist/sources/:id/enable` | 启用源，可触发同步。 |
| DELETE | `/panel/api/playlist/sources/:id` | 现有删除，`keep_content=0|1`。 |
| GET | `/panel/api/subs/status` | 扩展门控阶段与统计。 |

### 8.1 状态响应

```json
{
  "enabled": true,
  "schedule": {
    "mode": "cron",
    "interval_hours": 6,
    "cron": "0 */6 * * *",
    "next_run_at": 1793510000
  },
  "running": false,
  "run": {
    "run_id": "q-...",
    "trigger": "schedule",
    "phase": "done",
    "started_at": 1793500000,
    "total": 150,
    "tested": 150,
    "admitted": 12,
    "removed": 6,
    "rejected": 2,
    "skipped": 0,
    "bytes": 52428800,
    "cancelled": false,
    "paused_by_guard": false,
    "finished": true
  },
  "counts": { "admitted": 118, "removed_7d": 23, "sources": 4 },
  "guard": {
    "enabled": true,
    "max_remove_ratio": 0.5,
    "min_remove_count": 10,
    "tripped": false
  },
  "quality_enabled": true
}
```

### 8.2 结果响应

```json
{
  "total": 1,
  "items": [{
    "key": "sync:src-a:name:CCTV-5:1234",
    "scope": "sync",
    "source_id": "src-a",
    "gid": "sports",
    "cid": "cctv5",
    "name": "CCTV-5",
    "target_group": "体育",
    "action": "removed",
    "reason": "speed_low",
    "ok": false,
    "delay_ms": 120,
    "speed_mib": 0.4,
    "resolution": "1280x720",
    "checked_at": 1793500000,
    "first_fail_at": 1793400000,
    "rule_hash": "sha256:..."
  }]
}
```

### 8.3 发起校验

请求：

```json
{ "scope": "all", "id": null, "run_mode": "quick" }
```

- 全局关闭时自动只测量。响应带 `"measure_only": true`。
- 运行冲突：`409 run_active`。
- 无目标：`400 no_targets`。

### 8.4 源停用

`POST /playlist/sources/:id/disable`

```json
{ "keep_channels": true }
```

- `true`：源停用，频道保留，质量任务跳过该源。
- `false`：源停用，频道移出，结果记录保留。
- 两种情况都清空 `gate_cursor` 与门控阶段状态。
- 删除源沿用现有 `keep_content`，补充结果记录更新。

## 9 前端集成

- `SectionId` 增加 `quality`；导航增加「质量校验」。
- 新增 `frontend/src/quality-page.tsx`，保留现有徽章组件。
- `api.ts` 增加状态、结果、源、有效规则、运行、取消、停用/启用类型与函数。
- 规则保存使用 `GET /config` + `PUT /config`，带 `revision`。
- 源停用/删除使用新接口。
- 运行中每 2 秒轮询状态；结果页每 2 秒刷新。
- 分组页徽章跳转 `#/quality/results?gid=...&cid=...`。
- 质量变更后广播事件，分组页与订阅页按需刷新。
- 设置页移除质量字段，增加只读入口卡。

## 10 源停用与删除

停用：

- 更新 `source.enabled=false`。
- `keep_channels=true`：不改频道；质量任务跳过该源。
- `keep_channels=false`：删除该源频道与空同步分组；结果记录更新为 `removed`。
- 写入源状态，记录操作时间。

启用：

- 更新 `source.enabled=true`。
- 若源为 `sync`，按现有逻辑排入一次同步。
- 被移出的频道由同步门控重新准入。

删除：

- `keep_content=1`：现有逻辑把频道转为本地；结果记录改为 `scope=manual`。
- `keep_content=0`：现有逻辑删除频道；结果记录改为 `source_removed`。
- 删除源状态与正文缓存。
- 清空 `gate_cursor`、`gate_remaining` 与门控阶段状态。

## 11 可观测性

- 源状态增加：`gate_phase`、`gate_tested`、`gate_admitted`、`gate_rejected`、`gate_removed`、`gate_paused`、`gate_truncated`、`gate_cursor`、`gate_remaining`、`gate_last_at`。
- 运行状态增加 `phase`：`queued` / `fetching` / `parsing` / `probing` / `applying` / `done` / `cancelled` / `failed` / `paused_by_guard`。
- 日志字段：`run_id`、`source`、`gid`、`cid`、`reason`、`action`、`tested`、`removed`、`paused`。
- 不记录完整地址与查询令牌。
- 结果页与来源页读取持久记录，不依赖日志。

## 12 测试策略

单元：

- 配置迁移与校验。
- cron 共享模块与预览。
- 安全阀边界，含 `held` / `removal_paused`。
- 计划过滤与分组重建。
- 门控只处理新候选与 URL 变化；未变化频道不重复探测。
- 截断时已准入频道数量不变；新候选游标前进。
- URL 变化 `probe_error` 暂停同步，保留原地址。
- 结果稳定键、URL 变化更新同一行、保留清理。
- 手工频道删除清理结果；重启水合下线状态。
- 手工策略与订阅隐藏，含质量/健康优先级。

集成（`tests/http_contract.rs`）：

- 同步门控准入。
- 已准入失败立即移出。
- `probe_error` 不移出。
- 安全阀触发与关闭。
- 源停用保留/移出。
- 源删除保留为本地/随源删除。
- 全局关闭只测量。

容器验收：

- 使用 `tests/manual/local/docker-compose.yml` 与 Python 模拟源。
- 脚本：`scripts/smoke-quality-admission.sh`。
- 场景见功能设计 §13。

## 13 实施拆分

1. 配置字段、迁移、校验、cron 共享模块。
2. `quality_results` / `quality_meta` 存储与清理。
3. `mutate_admission` 事务与管理 API。
4. 探测引擎改造：单轮缓存、候选目标、规则解析。
5. 同步门控与计划过滤。
6. 周期/手动准入引擎、安全阀、移出提交。
7. 源停用/删除与源状态扩展。
8. 订阅渲染手工策略与健康兼容。
9. 前端质量页、设置页与分组页联动。
10. Docker + Python 验收脚本、文档与回归。

## 14 风险与解决方案

| 风险 | 方案 |
| --- | --- |
| 同步门控候选的分组覆盖解析 | 从 `apply` 抽出 `find_target_group`，`plan()` 为每个 `PlannedGroup` 写入已存在分组的 `quality` 覆盖。门控与 `apply` 共用，不写两套匹配。 |
| 配置与结果跨表事务 | 给 `ConfigStore` 增加事务内读取与保存方法。`mutate_admission` 在同一个 `SharedConn` 事务里写 `settings`、`quality_results`、`quality_meta`。不用降级方案。 |
| 移出后空同步分组 | 质量移出不删分组。下一次完整同步的 `apply` 阶段六会删除空同步分组。分组页暂时显示 0 个频道。 |
| 源停用/删除与运行中任务竞争 | 提交前复核：频道仍存在、`source_id` 未变、URL 哈希未变、源仍启用、源门控仍开启。不满足则跳过移出，记录 `skipped` / `source_changed`。 |
| 旧 `rules.action`、`retain`、`supply` 字段 | 保留解析，避免旧配置失效；新引擎不读取。规则页只显示新字段，旧字段留在「其余字段」JSON。 |
| 结果表增长 | 每个稳定键只保留一行。已准入行持续更新；非准入行超过保留期删除。启动时与每天各清理一次。 |
| 低配环境同步门控耗时 | 保留预算、并发与进度；增加 `gate_time_budget_secs`，默认 300 秒。超时后未判定候选不进入计划，源状态标记截断。 |
