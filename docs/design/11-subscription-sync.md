# 11 · 定时订阅与总清单

> 设计版本 v1.0 ｜ 2026-11-02 ｜ 状态：**已实施**
> 上游：[`03-http-contract.md`](03-http-contract.md)、[`04-config-and-storage.md`](04-config-and-storage.md)、[`05-frontend.md`](05-frontend.md)
> 相关证据：F-020（订阅聚合字段与去重）
> 说明：目标 v0.8.2 没有本功能。本文描述 TV Buddy 自有能力。

## 1 需求

| 编号 | 需求 | 验收判据 |
| --- | --- | --- |
| R1 | 手工与外部订阅共同生效 | 输出一个总清单。订阅、EPG、面板、用户授权都包含两类内容 |
| R2 | 定时抓取 M3U | 每个源按计划自动抓取。管理员可立即触发 |
| R3 | 新增与移除 | 同步新增上游新条目，移除上游已消失条目 |
| R4 | 手工信息不变 | 同步不改手工分组、手工频道、手工字段 |
| R5 | 同步后可管理 | 分组与频道可改名、排序、设播放参数、设台标、删除 |
| R6 | 失败保护 | 抓取失败不清空。空清单默认拦截。移除照常应用并标红 |

非目标：

| 项 | 原因 |
| --- | --- |
| 写回上游 | 上游 M3U 是只读清单 |
| 条件请求（ETag / Last-Modified） | 第一版直接抓取 |
| 上游分组改名后的身份延续 | 第一版按旧组删除、新组创建处理 |
| 多实例分布式同步 | 单进程。配置写锁已串行化 |

## 2 方案总览

```text
调度器 / 手动触发
  → 抓取 M3U（超时、16 MiB 上限、请求头、可选代理）
  → m3u::parse
  → 选择条目（分组规则 + 全局排除 + include/exclude）
  → 生成计划（目标分组、稳定键、URL 去重）
  → 保护检查（抓取失败、超限、空清单、源已变更）
  → 一次 state.mutate 应用差异
  → config.groups（总清单）
  → 订阅、EPG、面板、用户授权、台标、健康探测
```

核心决策：同步结果物化进 `config.groups`。

- 所有消费方只读 `config.groups`。物化后自动生效。
- 备份、导出、CLI、配置快照都走同一路径。
- 代价是配置体积变大。用单源上限与总频道警告控制。

## 3 数据模型

### 3.1 源配置 `[[playlist.sources]]`

保留已有字段：`id`、`name`、`url`、`enabled`、`ttl_secs`、`group_map`、`group_mode`、`merge_group`、`unlisted_groups`、`include`、`exclude`。

新增字段：

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `mode` | string | `aggregate` | `aggregate` 渲染时拼接；`sync` 物化 |
| `schedule` | string | `inherit` | `inherit` / `interval` / `daily` |
| `sync_interval_secs` | u64 | 0 | 0 继承全局。非零范围 300–2592000 |
| `sync_daily_at` | string | 空 | `HH:MM`。空继承全局 |
| `group_prefix` | string | 空 | 新分组名前缀。空值 = 保留原名 |
| `playback_mode` | string | `auto` | `auto` / `proxy` / `remux` / `dash` / `redirect` / `rewrite`。托管源禁止 `passthrough` |
| `format` | string | `auto` | `auto` / `hls` / `mpd` |
| `position` | string | 空 | `top` / `bottom`。空继承 `playlist.merge_position` |
| `headers` | Vec\<string\> | 空 | 抓取请求头，格式 `名称: 值` |
| `proxy` | Option\<ProxyCfg\> | 空 | 抓取 M3U 的代理 |
| `max_channels` | u64 | 0 | 0 继承全局 |
| `merge_into` | BTreeMap\<string,string\> | 空 | 上游分组名 → 已有分组 id。显式并入 |
| `merge_new` | BTreeMap\<string,string\> | 空 | 上游分组名 → 新组名。显式合并到新组；同名共用一组 |

`playback_mode` 与 `format` 只用于**新建**同步分组；已有分组的工作模式与内容格式在「分组管理」里修改。

校验：

- 源 `id` 非空且全局唯一。
- `mode` ∈ `aggregate` / `sync`。`schedule` ∈ `inherit` / `interval` / `daily`。
- `sync_interval_secs` 为 0，或在 300–2592000 之间。
- `sync_daily_at` 为空，或合法的 `HH:MM`。
- `playback_mode` 不含 `passthrough` 时才允许托管源。
- `format` ∈ `auto` / `hls` / `mpd`。`position` ∈ 空 / `top` / `bottom`。
- `merge_into` 的键与值都不能为空。
- `merge_new` 的键与值都不能为空。
- 同一个上游分组同时出现在两张表中时，`merge_into` 优先。

### 3.2 全局配置 `[playlist]`

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `sync_enabled` | bool | true | 自动同步总开关。手动触发不受限 |
| `sync_schedule` | string | `interval` | 全局默认调度：`interval` / `daily` |
| `sync_interval_secs` | u64 | 21600 | 默认间隔 6 小时 |
| `sync_daily_at` | string | `03:30` | 每天固定时间，服务器本地时间 |
| `sync_allow_empty` | bool | false | 允许空清单清空内容 |
| `sync_warn_removal_ratio` | f64 | 0.2 | 移除比例达到该值时标红。只影响显示 |
| `sync_max_channels` | u64 | 20000 | 单源频道上限 |
| `sync_total_warn` | u64 | 5000 | 总频道数警告阈值 |
| `sync_concurrency` | u64 | 1 | 同步并发。当前串行执行 |
| `sync_fetch_timeout_secs` | u64 | 25 | 单次抓取超时。范围 1–600 |

### 3.3 分组 `[[groups]]`

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `origin` | string | `manual` | `manual` / `sync` |
| `source_id` | string | 空 | 创建该分组的源 id。只用于显示与清理 |
| `source_group` | string | 空 | 上游原分组名。同源内匹配用 |

序列化：

- `origin=manual` 时省略。
- `source_id` 与 `source_group` 为空时省略。

### 3.4 频道 `[[groups.channels]]`

| 字段 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `source_id` | string | 空 | 空 = 手工频道 |
| `source_key` | string | 空 | 源内稳定键 |
| `sync_follow` | bool | true | 跟随上游字段。用户改上游字段后置 false |

序列化：

- `source_id` 与 `source_key` 为空时省略。
- `sync_follow=true` 时省略，只写 `false`。
- 前端把缺失的 `sync_follow` 当作 true。

### 3.5 同步状态 `SourceStatus`

旧 `status.json` 继续可读。所有新字段带 `#[serde(default)]`。

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `mode` | string | `aggregate` / `sync` |
| `channels` | usize | 本源当前频道数 |
| `groups` | usize | 本源当前分组数 |
| `last_ok` | u64 | 最近成功时间 |
| `last_try` | u64 | 最近尝试时间 |
| `last_sync` | u64 | 最近应用成功时间 |
| `added` / `updated` / `removed` | usize | 最近一次差异 |
| `skipped_removals` | usize | 固定频道保留数 |
| `needs_review` | bool | 空清单等待确认 |
| `zero_blocked` | bool | 最近一次因空清单未应用 |
| `shrink_permille` | u64 | 移除比例，千分比 |
| `duration_ms` | u64 | 最近一次同步耗时 |
| `next_sync` | u64 | 下次计划时间。0 表示不自动 |
| `syncing` | bool | 正在同步。运行时字段，不落盘 |

## 4 稳定键与 ID

### 4.1 频道稳定键

按优先级取第一个可用值：

| 顺序 | 条件 | `source_key` |
| --- | --- | --- |
| 1 | `tvg-id` 非空 | `id:<小写 tvg-id>` |
| 2 | 频道名非空 | `name:<上游原分组>\u{1f}<名称>`，去空白、转小写 |
| 3 | 以上都空 | `url:<完整 URL>` |

- 用上游原分组。分组改名映射变化不换键。
- 同源重复键追加 `#<8 位 Hex>`，取自 URL。与上游顺序无关。
- 旧数据的 `#2`、`#3` 键在首次同步时按键族配对，迁移到新键；频道 ID 不变。
- 计划阶段按 URL 去重。同源先出现者优先；跨源按源配置顺序。

### 4.2 频道 ID

| 条件 | 规则 |
| --- | --- |
| `tvg-id` 只含 `[A-Za-z0-9._~-]` | 直接使用 |
| `tvg-id` 含其他字符 | `s-<hash8>` |
| 无 `tvg-id` | `s-<sha256(source_id + "\|" + source_key) 前 8 hex>` |

目标分组内 ID 冲突时追加 `-2`、`-3`。

频道显示名取 `#EXTINF` 逗号后的名称。它为空时用 `tvg-name`，再为空用 URL。`tvg-name` 只作 EPG 元数据，不用作显示名，避免丢掉 `(字幕)`、`4K` 等后缀。

### 4.3 分组 ID

1. `merge_into` 命中已有分组时使用该分组。只按 id 查找，不按名称。
2. `merge_into` 的目标不存在时，按 `sync_target=gid:<id>` 找回上次建立的同步组。找不到才新建，避免每次同步重复建组。
3. `merge_new` 命中时使用 `new:<新组名>` 身份。同一上游分组只认一个显式目标。
   - `new:` 身份只按 `sync_target` 匹配，不复用 `source_group` 相同的旧组。首次合并会新建组并移入频道。
   - 修改新组名会形成新身份：下次同步新建组并移入频道，旧同步组清空后删除。
4. 否则按 `(source_id, source_group)` 查找。找不到时按 `(source_id, 空 source_group, 同名)` 兼容旧数据。
5. 新建分组用 `m3u::group_id(目标名)`。冲突时用 `s-<hash8>`。
6. `group_prefix` 只影响显示名，不影响 ID。
7. 分组身份：`merge_into` 为 `gid:<id>`；`merge_new` 为 `new:<新组名>`；`merge` 模式为 `merge:<目标名>`；其余为 `group:<上游分组>`。同一身份只建一个分组。
8. 同步分组在 `extra` 记 `sync_name`（显示名基线）与 `sync_target`（身份）。显式并入已有组时不写这两个标记，也不改名；转为手工或改挂其他源时清除。

## 5 同步算法

### 5.1 计划

纯函数。不抓取，不写配置。

1. `select_entries` 与预检共用。顺序：分组选择/改名 → 全局排除 → 源 `include` → 源 `exclude`。
   - 过滤词含 `*` 或 `?` 时，裸词、`group:`、`id:` 按整串通配符匹配；不含时保持子串或精确匹配。`idname:` 与 `url:` 始终精确匹配。
   - `merge_into` / `merge_new` 命中的上游分组是显式目标，不参与「映射外丢弃」。
   - `merge_new` 的新组名参与分组过滤；`merge_into` 的目标组名要到配置里解析，过滤仍用上游或映射名。
2. 收集其他来源（含手工）已占用的 URL。同址外部条目丢弃。
3. 同源重复 URL 丢弃。计数记入 `skipped_duplicates`。
4. 生成目标分组名：`group_prefix` 加映射后的分组名；`merge_new` 命中时用新组名。前缀为空时保留原名。
5. 目标优先级：`merge_into`（已有组 id）→ `merge_new`（新组名）→ `merge_group` / `group_map` / 原名。
6. 新分组属性：
   - `access=public`、`origin=sync`、`source_id`、`source_group`。
   - `playback_mode=auto`：URL 以 `.mpd` 结尾用 `dash`，否则用 `proxy`。
   - `format=auto`：`.mpd` 用 `mpd`，否则用 `hls`。
   - `position=top` 插到第一个非同步分组之前；`bottom` 追加到末尾；空值用 `merge_position`。

### 5.2 应用

在 `state.mutate` 闭包内执行。一次改写 `config.groups`。

1. 校验源仍存在、`mode=sync`、源配置未变。否则中止。
2. 解析或创建目标分组。同一身份的上游分组共用一个分组。
   - 显式并入已有组（`merge_into`）：保留目标组的名称、模式、格式、权限与代理，只更新频道。
   - 配置改名（`group_map`、`merge_group`、`group_prefix`）只作用于未手工改名的同步组。
   - 旧分组没有 `sync_name` 基线：记录基线，不覆盖现有名称。
3. 就地更新：计划内、`sync_follow=true` 的频道更新上游字段。
4. 删除：计划外、`sync_follow=true` 的本源频道删除。
5. 移动：目标分组变化时移动频道。保留原频道 ID，并按计划顺序追加到目标组末尾。目标组原有的频道保持原位。
6. 新增：计划内、当前不存在的频道新增。
7. 分组清理：
   - 本源同步分组且无频道：删除。
   - 仍有跟随本源的频道：保留。
   - 只剩其他源的同步频道：改挂该源。
   - 只剩手工或固定频道：转为 `manual`，清空来源字段。
8. 手工分组永不自动删除。手工频道永不自动删除。

“更新”指上游字段实际变化：`name`、`url`、`headers`、`tvg_id`、`tvg_name`、`tvg_logo`、`group_title`。值相同不计。

`sync_follow=false` 的频道保留字段与分组，计入 `skipped_removals`。

### 5.3 幂等

同一上游连续同步两次：第二次 `added=0`、`updated=0`、`removed=0`。

原因：稳定键与确定性 ID。

## 6 保护规则

按顺序检查：

| 规则 | 条件 | 结果 |
| --- | --- | --- |
| 抓取失败 | 网络错误、非 2xx、超时、读取失败 | 不改配置，记 `error` |
| 正文超限 | 超过 16 MiB | 不改配置，记 `error` |
| 条数超限 | 计划条数超过 `max_channels` | 不改配置，记 `error` |
| 空清单 | 计划 0 条、现有 > 0、`sync_allow_empty=false`、未 `force` | 不改配置，`needs_review=true`、`zero_blocked=true` |
| 缩水 | 移除数 > 0 | 照常应用。`shrink_permille` 达到阈值时标红 |
| 源已变更 | 计划所用源配置与当前不同 | 中止。下一轮重试 |

- `force=true` 只跳过空清单拦截。
- 空清单拦截时 `last_ok` 不变。
- 成功同步后清除 `needs_review` 与 `zero_blocked`。

## 7 调度

新增 `src/subscription.rs::spawn(state)`。在 `src/main.rs` 启动。

- 启动后等待 30 秒。每 60 秒检查一次。
- 全局 `sync_enabled=false` 时跳过自动同步。手动同步仍可用。
- 每个 `mode=sync`、`enabled=true` 的源独立计时。
- 间隔：`now >= last_try + interval`。间隔最小 300 秒。
- 每天：`last_try` 之后的第一个本地 `HH:MM`。
  - 本地时间用 `server.timezone_offset_hours` 计算。不引入时区数据库。
  - 时间已过则排到明天。
- 启动补跑：`last_try=0` 视为到期。
- 源或全局为每天定时时，至少一处要能解析 `HH:MM`。否则保存返回 `400`，不退化成「上次 +24 小时」。
- 到期源按配置顺序串行执行。源之间等待 1 秒。
- 失败退避：1、2、4… 分钟，上限 60 分钟。只用于间隔调度。成功或空清单拦截后清零。
- 手动同步更新 `last_try`。自动周期从手动时间重新计算。

## 8 HTTP 接口

所有接口需要管理员会话。公开路径与 `/panel/api` 别名受同一保护。

### 8.1 状态

`GET /panel/api/subs/status`：

```json
{
  "sources": [
    {
      "id": "src-a",
      "name": "上游 A",
      "url": "http://up/a.m3u",
      "enabled": true,
      "mode": "sync",
      "error": null,
      "channels": 128,
      "groups": 6,
      "last_ok": 1793500000,
      "last_try": 1793500000,
      "last_sync": 1793500000,
      "added": 2,
      "updated": 1,
      "removed": 3,
      "skipped_removals": 0,
      "needs_review": false,
      "zero_blocked": false,
      "shrink_permille": 23,
      "duration_ms": 840,
      "syncing": false,
      "next_sync": 1793521600,
      "ttl": 3600
    }
  ],
  "sync": {
    "enabled": true,
    "schedule": "interval",
    "interval_secs": 21600,
    "daily_at": "03:30"
  }
}
```

### 8.2 触发同步

`POST /panel/api/subs/sync`：

```json
{ "id": "src-a", "force": false }
```

- `id` 省略：排队全部启用的托管源。
- `force`：跳过空清单拦截。
- 异步执行。返回 `202`：

```json
{ "ok": true, "queued": 1 }
```

- 同步串行执行。已有任务在运行时，请求排队等待；同一源已在队列中时返回 `queued=0`。
- 源不存在：`404`。源不是托管模式：`400`。没有可同步的源：`400`。

### 8.3 预检

`POST /panel/api/subs/preview`：

- 保留原有 `kept`、`sample`、`trace`。
- 源是托管模式时，增加 `sync`：

```json
{
  "sync": {
    "add": 2,
    "update": 1,
    "remove": 3,
    "groups_add": 1,
    "groups_remove": 0,
    "groups_rename": 2,
    "skipped_duplicates": 0,
    "guard": {
      "over_limit": false,
      "empty": false,
      "max_channels": 20000,
      "allow_empty": false
    }
  }
}
```

- 托管源使用草稿源抓取。请求头、代理、超时生效。
- 聚合源忽略 `merge_into` / `merge_new`，预检结果与聚合清单一致。
- 预检不写配置，不写状态。

### 8.4 清理内容

`DELETE /panel/api/subs/:id/content?to_manual=1`：

- 默认 `to_manual=1`：保留内容，清空来源字段，转为手工。
- `to_manual=0`：删除本源频道，并清理空同步分组。
- 返回 `{ "ok": true, "channels": 128, "to_manual": true }`。

### 8.5 源增删改

| 接口 | 说明 |
| --- | --- |
| `PUT /panel/api/playlist/settings` | 只保存全局字段，保留源列表 |
| `POST /panel/api/playlist/sources` | 新建一个源。空 id 生成 UUID |
| `PUT /panel/api/playlist/sources/:id` | 保存一个源。`?keep_content=0` 删除已有内容 |
| `DELETE /panel/api/playlist/sources/:id` | 默认保留内容为手工。`?keep_content=0` 一并删除 |

- 源启用时 URL 必须是 `http(s)`。
- 已有物化内容且请求未显式选择时，修改模式或删除源返回 `409`，`code=content_exists`，附 `channels` 计数。
- 改 `sync` → `aggregate` 后，源摘要变化。正在运行的同步自动中止。
- 改 `aggregate` → `sync` 后，清空该源的 `last_try`。保存后立即同步。单源保存与全量替换两条路径一致。
- 读取分组与预检使用同一套抓取参数：请求头、代理、按模式选超时。聚合源固定 25 秒。
- 失败聚合源在 60 秒内不重试，避免订阅请求堆积。

### 8.6 分组与频道

| 接口 | 说明 |
| --- | --- |
| `PUT /panel/api/groups/:gid/channels/:cid` | 上游字段变化且未显式传 `sync_follow` 时，自动置 `false` |
| `DELETE /panel/api/groups/:gid/channels/:cid` | 同步频道删除时写入源 `exclude`。`tvg-id` 同源唯一用 `id:`；不唯一但名称唯一用 `idname:`；都相同才用 `url:` |
| `DELETE /panel/api/groups/:gid` | 同步分组删除时写入组内每个频道键。不写 `group:` |
| `POST /panel/api/groups/:gid/detach` | 分组转为手工。只断开本源频道，其他源频道保持跟随 |
| `POST /panel/api/groups/:gid/channels/:cid/detach` | 频道转为手工 |
| `POST /panel/api/groups/import[/force]` | 默认跳过 `source_id` 非空的分组 |
| `POST /panel/api/groups/import/m3u` | 同上。`replace` 遇同步分组时要求 `include_synced=true` |

防回流只用于用户主动删除。上游自己删除的频道不加 `exclude`。上游恢复时重新新增。

## 9 面板

### 9.1 订阅源页

`frontend/src/playlist.tsx` 重写为列表加侧边抽屉。

列表列：启用、名称、地址、模式、频道/分组、最近同步、状态、操作。

- 启用列是按钮，按钮文字显示当前状态（`已启用` / `已禁用`），点一下切换。抽屉里用开关。
- 模式徽章：`聚合` / `托管`。
- 状态：同步中、排队中、同步完成、同步失败、缩水、待确认、未同步。徽章下一行显示相对时间与增删摘要；失败时显示错误文字。
- 最近同步：托管源显示最近成功同步，聚合源显示最近成功抓取。
- 操作：立即同步、编辑、删除。同步中或排队中时按钮显示对应文字并禁用。
- 每 5 秒轮询状态。
- 添加源默认处理方式是 `托管`。

抽屉分区：

1. 基本：名称、URL、启用。
2. 模式：聚合 / 托管。附差异说明。
3. 分组：读取分组、映射、未列出分组、前缀、并入已有分组。
4. 同步：调度、间隔、每天时间、单源上限、播放模式、格式。
5. 高级：请求头、代理、插入位置。
6. 预检：新增/更新/移除、分组新增/改名/移除计数、保护提示、保存并开始同步。

其他行为：

- 全局设置单独保存。
- 删除源二选一：保留为手工、一并删除。
- 首次切换为托管时提示：按映射改名、映射为空且丢弃映射外分组时，不会导入任何频道。旧配置的空映射 `drop` 在没有显式目标时归一为 `keep`。
- 有未授权的同步分组时显示提示。提示只统计受限用户，不自动授权。

### 9.2 分组页

- 分组徽章：`手工` / `订阅：<源名>`。
- 频道徽章：`手工` / `订阅：<源名> · 跟随` / `订阅：<源名> · 固定`。
- 编辑上游字段后自动固定。可“恢复跟随”。
- 删除同步频道或分组时提供“删除并排除”与“转为手工”。
- 全量保存带 `revision`。`409` 时重新加载并提示。

### 9.3 用户页

- 分组授权列表显示来源徽章。
- 新同步分组不自动授权。

### 9.4 安全卡

- `token_enabled=false`：红色警告 + 一键开启。
- 手工 `passthrough` 分组：黄色警告。
- 免令牌直链已停用。

## 10 管理规则

| 用户操作 | 同步后的结果 |
| --- | --- |
| 改名称、URL、台标、请求头 | `sync_follow=false`。以后不覆盖上游字段 |
| 改播放参数、代理、预取、回看 | 手工字段。同步不动 |
| 改分组名称 | 保留。同步不覆盖手工改名；配置映射改名只作用于未手工改名的分组 |
| 改分组模式、访问级别、格式 | 保留。同步只在新建时写默认值 |
| 删除同步频道 | 写入源 `exclude`。下次同步不加回 |
| 删除同步分组 | 写入组内频道键。下次同步不重建 |
| 手工新增频道到同步分组 | 保留 |
| 关闭源 | 自动同步停止。内容保留 |
| 删除源 | 询问：保留为手工或删除 |
| `sync` 切到 `aggregate` | 询问：保留为手工或删除 |
| `aggregate` 切到 `sync` | 保存后立即同步 |
| 上游改 URL、改名、移组 | `sync_follow=true` 时更新；否则保留 |
| 上游改分组名 | 旧组删除、新组创建。旧组有手工内容时保留 |
| 上游清空清单 | 默认拦截。`sync_allow_empty=true` 时清空 |

## 11 限制

| 项 | 默认 | 行为 |
| --- | --- | --- |
| 正文上限 | 16 MiB | 超限报错，不改配置 |
| 单源频道上限 | 20000 | 超限报错，不改配置 |
| 抓取超时 | 25 秒 | 超时保留现状 |
| 空清单 | 拦截 | 标记待确认 |
| 总频道警告 | 5000 | 面板提示。大清单建议用聚合模式 |

补充：

- 配置是 SQLite 单行 JSON。每次同步写整份配置。低配机器建议把超过 2 万的清单改用聚合模式。
- 源地址由管理员配置。服务端会主动抓取。内网部署时注意 SSRF 风险。第一版不封锁内网地址。
- 源 URL 与备份可能包含账号令牌。与现有聚合源行为一致。
- `sync_concurrency` 当前只登记，不并行执行。
- 手动同步不设冷却。同步串行执行；已有任务运行时，新请求排队，同一源不重复排队。
- 聚合输出与本地频道按 URL 去重。本地已存在的地址不再追加。

## 12 兼容与迁移

| 项 | 处理 |
| --- | --- |
| 旧配置 | 新字段有默认值。`mode` 空按 `aggregate` |
| 旧同步字段 | 旧版本把新字段放进 `extra`，保存时原样写回 |
| 备份与恢复 | `config.json` 含来源字段。恢复后继续同步 |
| 公开订阅预检 | 已改为管理员会话。见 `03` §3 |
| 孤儿分组 | `source_id` 指向已删除源。面板可转为手工或删除 |
| 降级 | 旧二进制把新字段放进 `extra`，保存时写回。数据不丢 |

## 13 测试

### 13.1 单元测试

`src/subscription.rs` 与 `src/config.rs` 覆盖：

- 首次同步、第二次零变更。
- 新增、删除、改 URL、改名、移组。
- 固定频道不覆盖、不删除。
- 删除后防回流（`id:` / `idname:` / `url:`）。
- 每源独立分组、`merge_into`、`merge_new`、不自动合并。
- 新建组与已有组混用、同名新组共用一组、目标切换与移组顺序。
- 空组删除、只剩手工转手工、多源并组保留。
- 空清单拦截、`force`、`sync_allow_empty`。
- 缩水比例。
- 跨源/手工 URL 去重、ID 冲突、稳定键后缀。
- 分组改名保留。多源并入同组时频道 ID 不冲突。
- 同源重复 `tvg-id` 删除时改用 `url:` 排除。
- 旧大小写键继续匹配。
- 调度：间隔、每天、退避。
- 源配置校验。

### 13.2 冒烟

`scripts/smoke-subsync.sh` 覆盖：

1. 起可变 M3U 源。登录管理员。
2. 建托管源、预检、保存、同步。
3. 检查分组、频道、状态、幂等。
4. 检查订阅清单含同步频道，地址带 `/l/<token>/`。
5. 用户授权过滤。
6. 上游新增、删除、改 URL。
7. 手工改名并固定。同步后保留。
8. 删除同步频道。同步后不回流。
9. 空清单拦截。开启 `sync_allow_empty` 后清空。
10. 抓取失败。内容不变。
11. 删除源默认保留为手工；再测一并删除。
12. 聚合源行为不变。

## 14 实施状态

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| P0 | 鉴权加固 | ✅ `v2026.16-beta.1` |
| P1 | 数据模型、计划、应用、保护、单元测试 | ✅ `v2026.16-beta.2` |
| P2 | 同步接口、状态、预检、调度 | ✅ `v2026.16-beta.2` |
| P3 | 订阅源抽屉、分组徽章、固定、detach、revision | ✅ `v2026.16-beta.2` |
| P4 | 文档、冒烟、发布 | ✅ `v2026.16` |

## 15 实现落点

| 文件 | 内容 |
| --- | --- |
| `src/config/model.rs` | 新字段、默认值、`Config::normalize` |
| `src/config.rs` | `validate_playlist` 与时间校验 |
| `src/subscription.rs` | 抓取、计划、应用、清理、保护、调度、状态 |
| `src/subs.rs` | `select_entries`、`SourceStatus`、`url:` 过滤 |
| `src/state.rs` | 同步锁、触发节流、`client_for_proxy` |
| `src/main.rs` | 启动调度器 |
| `src/http/routes/subs.rs` | 状态、同步、预检、清理 |
| `src/http/routes/playlist.rs` | 全局设置、单源增删改 |
| `src/http/routes/groups.rs` | 固定、detach、删除排除、导入保护 |
| `frontend/src/playlist.tsx` | 列表与抽屉 |
| `frontend/src/groups.tsx` | 来源徽章、固定、detach |
| `frontend/src/api.ts` | `putConfig`、`revision`、`409` |
| `scripts/smoke-subsync.sh` | 冒烟脚本 |
