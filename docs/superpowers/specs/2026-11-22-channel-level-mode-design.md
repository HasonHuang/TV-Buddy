# 频道级工作模式设计（默认继承分组）

> 状态：设计待批准 ｜ 日期：2026-11-22
> 目标版本：下一次主版本。具体版本号由发布计划决定。
> 相关：[`../../design/03-http-contract.md`](../../design/03-http-contract.md)、[`../../design/04-config-and-storage.md`](../../design/04-config-and-storage.md)、[`../../design/11-subscription-sync.md`](../../design/11-subscription-sync.md)、[`../../spec/08-target-spec.md`](../../spec/08-target-spec.md)

## 1. 意图

一个分组里会同时存在两类频道：

- 手工频道：稳定或优质源。通常用「改写地址」或「服务器转发」。需要缓存、伪装、回看。
- 订阅频道：来自经常变化的源。通常用「302 跳转」。媒体流不过本机，节省带宽与 CPU。

现状只有一个 `Group.mode`。同组频道只能共用一种模式。管理员必须在两种策略里二选一。

本次改动把工作模式下移到频道：

- 频道可以覆盖工作模式。
- 频道没有覆盖时，继承分组。
- 同步源把工作模式写入新建频道，作为频道默认值。
- 302 的同步频道仍经过本机一次跳转，仍校验 Token。

## 2. 目标

1. 频道可以单独设置六种工作模式。
2. 频道未设置时，行为与现状完全一致：继承分组。
3. 同一分组可以混合不同工作模式的频道。
4. 新建同步频道自动获得源的工作模式，减少逐条维护。
5. 管理员手改的频道模式不被同步覆盖。
6. 存量配置无需迁移即可加载。
7. 直通（`passthrough`）的安全边界不放松。

## 3. 非目标

- 不改六种模式的语义。
- 不改分组级模式、访问范围、内容格式的默认值语义。
- 不改播放 Token、切片签名、IP 管控。
- 不把 `redirect_ttl`、`redirect_mode`、`remux_container`、`mpd_keep_seconds` 等模式参数下移到频道。
- 不引入第三种继承层级（例如「跟随源」）。
- 不自动改写存量同步频道。

## 4. 反目标

- 不能出现「频道设置了模式但播放仍用分组模式」。
- 不能出现「同步新增频道后，管理员必须逐条改模式」的常态。
- 不能因为升级自动改变现有频道的播放行为。
- 不能让同步频道通过 `passthrough` 绕过本机和凭证检查。
- 不能让一个分组的模式统计把混合频道的实际模式算错。

## 5. 当前状态

工作模式只有 `Group.mode` 一个字段。以下路径直接读它：

| 位置 | 用途 |
| --- | --- |
| `src/http/routes/stream.rs` | 入口分发、直通、签名门槛、DASH/HLS、变体清单、MPD 切片 |
| `src/playlist.rs` | 订阅地址生成、直通原始地址、质量隐藏 |
| `src/quality/engine.rs` | 直通组跳过探测 |
| `src/schedule.rs` | DASH 频道录制分流 |
| `src/dash_timeshift.rs` | DASH 时移会话与预热 |
| `src/http/routes/dash.rs` | 时移状态与模式回显 |
| `src/http/routes/panel.rs` | 概览的模式分布 |

频道表单没有模式字段。基线 `S-MODE-02` 与 `S-MODEL-03` 记录「模式属于分组」。

项目已有「频道级覆盖、默认继承」的先例：

- `Channel.offline_on_fail: Option<bool>`，`Config::offline_on_fail_for` 解析。
- `Channel.proxy`、`Channel.prefetch`、`Channel.quality` 使用 `Option` 覆盖。

本次沿用同一套路。

## 6. 方案比较

| 方案 | 内容 | 优点 | 缺点 | 结论 |
| --- | --- | --- | --- | --- |
| A 严格频道覆盖 | 所有频道默认继承分组；订阅频道靠手动或批量改 | 改动最小 | 每轮同步新增频道都要补设置，长期维护成本高 | 不选 |
| B 源默认落频道 | 同步创建频道时写入源模式；频道显式值优先；同步不覆盖 | 新建订阅频道自动正确；可单条覆盖；可审计 | 需要来源标记与回填入口 | **已选** |
| C 按来源动态继承 | 频道无显式值时，订阅频道取源模式，手工频道取分组模式 | 改源设置可影响存量频道 | 改源会悄悄改变已有频道；排障要跨层追源；与「源设置只影响新建」冲突 | 不选 |

选择 B。原因是它把「源默认」变成一次性的显式值，行为可预测。管理员能在频道上看到真实生效的模式。同步只负责创建时的默认值，不持续改写。

## 7. 数据模型

### 7.1 字段

```rust
pub struct Channel {
    // ...
    /// `proxy` / `remux` / `dash` / `redirect` / `rewrite` / `passthrough`。
    /// 缺省表示继承分组。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}
```

- `Group.mode` 保持必填，默认 `proxy`。
- `Channel.mode` 缺省不写盘。旧配置加载后为 `None`，行为不变。
- `None` 是唯一的继承状态。空字符串按非法值处理。
- 来源默认落盘时，`Channel.mode` 是 `Some(value)`。它不是第三层继承，而是显式值。

### 7.2 解析

```rust
impl Group {
    /// 频道有效工作模式：频道显式值优先，否则用分组默认值。
    pub fn effective_mode<'a>(&'a self, channel: &'a Channel) -> &'a str {
        channel.mode.as_deref().unwrap_or(&self.mode)
    }
}

impl Config {
    /// 按分组与频道 id 取有效模式。
    pub fn effective_mode(&self, gid: &str, cid: &str) -> Option<&str> { /* ... */ }
}
```

所有播放、订阅、探测、录制、时移、统计路径只调用这两个 helper。禁止再直接读 `group.mode` 判断单频道行为。

### 7.3 来源标记

`Channel.extra` 增加内部键：

```text
mode_source = "sync:auto"   // 创建时源是 auto
mode_source = "sync:redirect" // 创建时源是显式模式
mode_source = "manual"      // 管理员显式设置或清空过模式
```

含义：当前 `Channel.mode` 来自哪里。`sync:*` 表示同步源的默认值；`manual` 表示管理员显式操作过。

规则：

- 同步创建频道时写入 `sync:auto` 或 `sync:<模式>`。
- 管理员修改频道模式（包括选择「跟随分组」清空模式）时写入 `manual`。
- 管理员修改上游 URL，导致 `sync_follow` 变为 `false` 时写入 `manual`。
- 频道转为手工（detach）时写入 `manual`。
- 分组或源转为手工时，受影响的频道写入 `manual`。
- 导出配置时该键随 `extra` 输出。它不含敏感信息。

沿用 `Group.extra` 的 `sync_name` / `sync_target` 标记做法，不新增顶层配置字段。

## 8. 同步行为

### 8.1 新建频道

`build_channel` 写入 `mode`：

| 源 `playback_mode` | 频道模式 |
| --- | --- |
| `auto` | URL 路径去掉查询串后以 `.mpd` 结尾 → `dash`；否则 `proxy` |
| `proxy` / `remux` / `dash` / `redirect` / `rewrite` | 同源值 |
| `passthrough` | 仍然禁止。源校验与频道校验都拒绝 |

同时写入 `mode_source = "sync:auto"` 或 `"sync:<显式模式>"`。

### 8.2 后续同步

| 情况 | 行为 |
| --- | --- |
| 频道模式是管理员手改 | 不动 |
| 标记是 `sync:auto`，且当前源仍是 `auto` | URL 类型变化时重算并更新 |
| 标记是 `sync:auto`，但当前源已改为显式模式 | 不更新。源变更只影响以后新建的频道 |
| 标记是 `sync:<显式模式>` | 不更新。即使源后来改为 `auto`，旧频道也保留创建时的值 |
| 频道 `sync_follow = false` | 上游字段不更新，模式也不更新 |

源默认不是持续策略。它只在频道创建时决定一次。`auto` 的重算只用于「创建时就是 auto、现在仍是 auto」的频道，避免源设置变化静默改写已有频道。

管理员手动修改上游 URL 时，`update_channel` 会把 `sync_follow` 置为 `false`，并写入 `mode_source = "manual"`。频道模式保留为显式值。管理员接管后，模式不再自动变化。

管理员要批量套用新默认，使用回填入口。

### 8.3 转为手工

`detach_channel`、`detach_group`、`detach_source`：

- 清空 `source_id`、`source_key`。
- 恢复 `sync_follow = true`。
- 写入 `mode_source = "manual"`。
- 保留 `mode` 值。转为手工后，模式变成普通频道显式值。

## 9. 存量回填

升级后，已有同步频道的 `mode` 为空，仍继承分组。提供一次性回填：

- 入口：订阅源抽屉里的「按源默认回填」按钮。
- 范围：`source_id = 当前源`、`mode` 为空、`sync_follow = true`、且 `mode_source != "manual"` 的频道。
- 动作：按 8.1 的规则计算模式，写入 `mode` 与 `mode_source`。
- 不覆盖已有 `mode` 的频道。
- 不覆盖管理员显式选过「跟随分组」的频道（`mode_source = "manual"`）。
- 跳过 `sync_follow = false` 的频道。固定频道已由管理员接管。
- 可重复执行。第二次没有可回填频道。
- 不自动执行。升级不会改变现有行为。

接口：

```text
POST /panel/api/subs/{id}/backfill-mode
```

响应：

```json
{
  "ok": true,
  "updated": 12,
  "modes": { "redirect": 12 }
}
```

前端预览：

- 按钮旁显示可回填条数。
- 条数为 0 时禁用按钮。
- 点击后用确认框列出条数与目标模式。
- 成功后用 toast 报告条数。

## 10. 播放与订阅行为

### 10.1 入口分发

`play_entry` 使用 `group.effective_mode(channel)`：

| 有效模式 | 行为 |
| --- | --- |
| `passthrough` | 302 到上游原始地址 |
| `redirect` | 302；按分组 `redirect_mode` / `redirect_ttl` 处理 |
| `rewrite` | 取一次清单并绝对化 |
| `remux` | 连续流转封装 |
| `dash` | MPD→HLS |
| `proxy` | 代理 |

`serve_rest` 的签名门槛、DASH 分支、非代理占位分支、`variant_playlist`、`mpd_segment` 全部使用有效模式。

### 10.2 订阅地址

`entry_url` 与 `push_entry` 按频道判断：

- 有效模式是 `passthrough`：输出上游原始地址，不套 Token。
- 其他模式：输出本地入口 `/l/<token>/<gid>/<cid>.<ext>`。

同一分组可以同时出现原始地址和本地入口。`group.access = "archived"` 仍然整组停播。

### 10.3 Token 与安全

- 302 同步频道仍带 Token。入口校验通过后 302 到上游。媒体流不过本机。
- `passthrough` 仍绕过本机与凭证检查。
- 同步频道的**有效模式**禁止 `passthrough`。不只是显式字段：
  - 同步频道显式设置 `passthrough`：创建与更新接口返回 `400`，配置校验拒绝，前端隐藏该选项。
  - 分组 `mode = "passthrough"` 且组内存在 `mode = null` 的同步频道：分组更新返回 `400`，配置校验拒绝。这样的频道会继承直通，同样绕过凭证。
  - 同步频道已有显式非直通模式时，分组可以设为 `passthrough`。组内手工频道仍可直通。
- 存量配置的修复：`Config::normalize` 在加载时检查上述组合。发现「分组直通 + 同步频道未设置模式」时，给这些同步频道写入显式安全模式，并记录警告日志：
  - 源 `playback_mode` 是显式非 `passthrough` 模式时，用该模式。
  - 否则用 `redirect`。
  - 写入 `mode_source = "sync:auto"` 或 `"sync:<模式>"`，按当前源模式决定。修复后配置可正常保存。
- 分组级 `passthrough` 的存量行为不变。安全卡按有效模式统计频道，不漏报混合分组里的直通频道。

### 10.4 质量探测与失败下线

- 去掉「分组是 `passthrough` 就整组跳过」。
- 改为逐频道判断：有效模式是 `passthrough` 且 `quality.probe_passthrough = false` 时跳过。
- 同一分组里的 `proxy` / `rewrite` 频道照常探测。
- `manual_hidden` 使用同样的逐频道判断。

### 10.5 录制、时移、回看

- 定时录制：有效模式是 `dash` 时走 DASH 录制，否则走原路径。
- DASH 时移：有效模式是 `dash` 且 `dash_timeshift` 点亮时启用。
- 本地回看：沿用频道 `livestream` 开关。模式变化不改变现有回看开关语义。

### 10.6 内容格式

`Group.format` 保持在分组级，不下移。

- 频道模式 `dash` 落在 HLS 分组里可以工作：DASH 模式输出 HLS 清单。
- 频道模式 `proxy` / `rewrite` 落在 MPD 分组里沿用现有行为。
- 「改写 + MPD」的已知限制（`D-005`）不变。

## 11. API 与校验

### 11.1 频道接口

`POST /panel/api/groups/{gid}/channels` 与 `PUT /panel/api/groups/{gid}/channels/{cid}`：

- 接受 `mode`：六种模式或 `null`。
- `null` 表示继承分组。
- 非法值返回 `400`，响应说明可用模式。
- 同步频道（`source_id` 非空）设置 `passthrough` 返回 `400`。
- 同步频道把 `mode` 清成 `null`，且所在分组是 `passthrough` 时返回 `400`。这会形成有效的直通。
- 请求体带 `mode` 键时，视为管理员显式操作，写入 `mode_source = "manual"`。前端在值未变时不发送该键。
- `PUT` 中上游字段变化导致 `sync_follow` 变为 `false` 时，写入 `mode_source = "manual"`。模式保留。

### 11.2 配置校验

`config::validate` 增加：

- 频道 `mode` 只允许六种值。
- 分组 `mode` 只允许六种值。这是现有的校验空白，一并补上。
- 同步频道的显式模式不能是 `passthrough`。
- 分组 `mode = "passthrough"` 时，组内不能有 `mode = null` 的同步频道。
- 存量配置中该字段缺失时不报错。

接口层要在 `state.mutate` 之前做同样的检查并返回 `400`。`state.mutate` 内部的 `normalize` 是安全网，不能代替接口层校验；否则管理员清空同步频道模式会被 `normalize` 静默改回显式值。

`Config::normalize` 在加载时修复第 3 条，见 10.3。

`ConfigStore::save_conn` 已调用 `validate`。频道接口在写入前再做一次校验，保证返回 `400` 而不是 `500`。

### 11.3 状态接口

`GET /panel/api/status` 增加：

```json
"channel_modes": { "proxy": 120, "redirect": 80, "dash": 6 }
```

- 按每条频道的有效模式计数。
- 保留 `group_modes`，避免破坏已有消费者。
- 概览页优先使用 `channel_modes`。

## 12. 前端设计

前端沿用现有 React + 自绘 `Select` / `Field` / `Badge` / `Confirm` / `useToast`。不新增设计系统，不改全局样式。

### 12.1 频道弹窗

在「上游 URL / 解析脚本」区块之后、「回看配置」之前增加一组播放字段：

```text
工作模式
[跟随分组（当前：服务器转发）      v]
频道可单独覆盖。未设置时使用分组的工作模式。
```

- `Select` 宽度 200，与现有字段一致。
- 第一项 `value = ""`，标签是「跟随分组（当前：<modeText(group.mode)>）」，说明是「使用分组的工作模式」。
- 其余项用 `modeOptions()`。
- 同步频道用 `syncModeOptions()`，不出现 `passthrough`。
- 同步频道且分组模式是 `passthrough` 时，不显示「跟随分组」选项。该频道必须有显式非直通模式。
- 来源默认值：如果 `mode_source` 以 `sync:` 开头，字段下方显示提示「当前值来自订阅源默认。手动修改后以频道为准。」管理员改动选择后提示立即消失。
- 表单只保存 `mode: null` 或模式字符串。未改动时不发 `mode`，避免误清来源标记。
- 字段具有可见标签与 `ariaLabel`。键盘、触屏、焦点样式沿用现有 `Select`。

### 12.2 频道列表

- 在 `ch-badges` 行增加模式徽标，只在**有效模式与分组模式不同**时显示。这样源码默认与分组相同的频道不会增加噪音。
- 徽标文本用 `modeCode(mode)`，如 `302`、`REWRITE`、`PROXY`。
- 悬停与键盘焦点显示 `modeTip(mode)`。
- 直通徽标使用 `warn` 色，提示「绕过本机与凭证检查」；其他模式用 `muted`。
- 与分组相同的模式不显示徽标。频道弹窗仍显示完整值。
- 频道地址行按有效模式显示：直通显示原始 URL 并带「直通」徽标；其他显示本地播放地址。
- `allTracks` 判断改为有效模式是 `dash`。

### 12.3 分组弹窗

- 「工作模式」字段保留原名，提示改为「分组默认值。频道可以单独覆盖。302 与 PASSTHROUGH 由播放器直连源。」
- 增加一行 hint：「修改后，未单独设置模式的频道立即跟随。」
- 组内有显式模式的频道时，再显示一行：「N 个频道使用单独设置的模式，不受分组修改影响。」
- 组内存在 `mode = null` 的同步频道时，选中 `PASSTHROUGH` 后在字段下方显示警示：「组内有 N 个同步频道未设置工作模式，直通会绕过凭证。请先为它们设置模式或按源默认回填。」保存时后端仍返回 `400`。
- 内容格式字段不变。

### 12.4 订阅源抽屉

- 「新分组工作模式」改为「新分组与频道默认」。
- 提示改为「用于之后新建的同步分组和新频道。已有频道不受影响。管理员改过的频道模式不会被覆盖。」
- 在字段下方增加回填行：

```text
按源默认回填现有频道（12 条）  [回填]
只回填未设置模式且仍在跟随来源的频道。
```

- 条数由前端从配置计算：`source_id` 匹配、`mode` 为空、`sync_follow !== false`、且 `mode_source !== 'manual'`。
- 订阅源页已读取 `/config` 的分组列表。把它扩展为包含 `channels` 的类型即可计数，无需新增预览接口。若后端更愿意提供预览字段，也可以放进 `/subs/status`。
- 点击后打开 `Confirm`，列出条数与目标模式；确认后调用回填接口。
- 成功后 `toast` 报告「已回填 12 个频道」。
- 失败时在抽屉里显示错误，不清空草稿。

### 12.5 安全卡

- 统计对象从分组改为频道。
- 计算方式：`(channel.mode ?? group.mode) === 'passthrough'`。
- 文案：「有 N 个频道使用「原样直通」。这些频道绕过本机和凭证检查。建议改为「改写地址」或「302 跳转」。」
- 列出前 3 个「分组名 / 频道名」。其余用省略号。
- 不使用颜色单独表达风险；同时保留文字与徽标。

### 12.6 概览分布

- 卡片标题保持「分组分布」，说明改为「频道按有效工作模式统计（含频道覆盖）」。
- 数据优先取 `channel_modes`，缺失时回退 `group_modes`。
- 模式名称与颜色沿用现有 `modeLabel` 与 `ov-dot` 样式。

### 12.7 交互与可访问性检查

- 所有新控件有可见标签，`Select` 带 `ariaLabel`。
- 徽标文本可读，不只用颜色表达状态；直通徽标同时有文字「直通」与悬停说明。
- 确认框使用现有 `Confirm`，焦点管理与 Esc 行为沿用。
- 回填按钮禁用时显示原因（条数为 0）。
- 提交有 loading、成功 toast、失败就地错误。
- 移动端沿用现有抽屉与弹窗宽度；不新增固定像素布局。

## 13. 统计与告警

- 概览：新增 `channel_modes`，按有效模式统计频道数。
- 安全卡：按有效模式统计直通频道。
- 分组列表：有显式覆盖的频道显示模式徽标。
- 不新增全局告警。直通告警沿用安全卡。

## 14. 兼容性与基线差异

- 配置无需迁移。旧 JSON 没有 `mode` 键，反序列化为 `None`。
- 保存旧配置时不写 `mode` 键。
- 现有分组级行为不变。
- 唯一的加载时修复：分组直通且组内有未设置模式的同步频道时，给这些频道写入显式安全模式（见 10.3）。这只影响本来就会绕过凭证的组合，并写警告日志。
- 这是对基线 `S-MODE-02`「模式属于分组」与 `S-MODEL-03` 的有意扩展。目标实现仍以分组为准；本实现增加频道级覆盖。
- 文档需要登记差异，避免后续验收把扩展当成回归。

## 14.1 已知限制

| 限制 | 影响 | 本次处理 |
| --- | --- | --- |
| `format` 仍在分组级 | 同组混合 MPD/HLS 时，入口扩展名可能与正文不一致 | 沿用现状。扩展名只做形状门槛，实际响应由模式决定。需要逐频道格式时另立设计 |
| 模式参数仍在分组级 | 频道只覆盖模式，不能单独设置 `redirect_ttl`、`redirect_mode`、`remux_container` 等 | 接受。频道共享分组参数 |
| 模式变更不立即停止旧会话 | 已存在的 remux、DASH、DASH 时移会话按空闲清理回收，可能短暂占用 CPU/磁盘 | 本次接受。若要求立即停止，另加按频道停止入口 |
| `passthrough` 与解析脚本不兼容 | 订阅直接导出静态 `channel.url`，解析脚本不会运行 | 沿用现有语义。前端在直通 + 脚本时提示 |
| 本地回看与录制的模式限制 | Rewrite / Redirect / Passthrough 的本地回看与录制支持有限 | 沿用现有行为，不按模式新增开关 |

## 15. 测试计划

### 15.1 单元测试

- `effective_mode`：频道显式值优先；缺省继承分组；未知频道返回 `None`。
- 序列化：`mode = None` 不写键；`Some("redirect")` 写键。
- 校验：六种合法值；非法值拒绝；同步频道 `passthrough` 拒绝；手工频道 `passthrough` 允许；分组直通 + 未设置模式的同步频道拒绝。
- 同步：新建频道按 `auto` / 显式模式落值；`sync:auto` 频道在源仍为 `auto` 时 URL 类型变化会重算；源改为显式模式后不重算；`sync:<显式>` 频道不重算；管理员覆盖保留；管理员改 URL 后清标记；detach 清标记。
- 加载修复：分组直通 + 未设置模式的同步频道被写成显式安全模式；修复后配置通过校验。
- 回填：只填空值且跟随中的频道；跳过 `mode_source = "manual"`；重复执行幂等；返回计数。
- 管理员显式选「跟随分组」写入 `manual`；回填不覆盖。
- 分组与源的 detach 给受影响频道写入 `manual`。

### 15.2 契约测试

- 频道创建/更新 `mode`，读回一致。
- `mode: null` 清除覆盖。
- 非法模式 `400`。
- 同步频道 `passthrough` `400`。
- 分组设为 `passthrough` 且组内有未设置模式的同步频道时 `400`；这些频道设置显式模式后可以保存。
- 同步频道在 passthrough 分组里清空模式返回 `400`。
- 混合分组订阅里同时出现原始地址和本地入口。
- 同一分组两个频道分别用 `redirect` 与 `proxy`，入口行为不同。
- 改分组模式只影响继承频道。
- 回填接口返回计数，管理员覆盖不变。
- `GET /panel/api/status` 返回 `channel_modes`。

### 15.3 前端验收

- 频道弹窗出现「工作模式」，默认选中「跟随分组」。
- 同步频道模式选项不出现 `passthrough`。
- 来源默认提示只在未手改时显示。
- 频道列表只在有效模式与分组不同时出现徽标；相同模式不出现。
- 同步频道在 passthrough 分组里看不到「跟随分组」选项。
- 订阅源抽屉显示可回填条数；0 条时按钮禁用；成功后有 toast。
- 安全卡按频道计数；混合分组里的直通频道被列出。
- 概览按频道有效模式显示分布。

## 16. 文档更新

- `README.md`：工作模式说明增加频道级覆盖。
- `docs/design/03-http-contract.md`：频道接口 `mode`、回填接口、状态 `channel_modes`。
- `docs/design/04-config-and-storage.md`：`Channel.mode` 三态、`mode_source` 标记。
- `docs/design/11-subscription-sync.md`：源 `playback_mode` 扩展为频道默认值；回填规则；与「只影响新建分组」的新表述。
- `docs/design/09-users-and-devices.md` 或安全设计：直通统计口径改为频道。
- `docs/spec/07-baseline-and-verification.md`、`docs/spec/08-target-spec.md`：登记与 `S-MODE-02` / `S-MODEL-03` 的差异。
- `docs/tests/02-panel-acceptance.md`：频道弹窗、订阅源抽屉、安全卡、概览的文案与字段。
- `PROGRESS.md`：记录本特性与版本。

## 17. 留给实现者的决定

- `mode_source` 的具体键名，只要不与现有 `extra` 键冲突。值格式按 7.3。
- 加载修复的警告日志字段与级别。
- 有效模式 helper 的命名与放置位置。
- 回填接口的路径前缀跟随当前路由整理方案。
- 前端模式徽标的具体 tone 组合，只要满足文字可读与直通警示。
- 测试文件的拆分方式。

## 附录 A：场景速查

记号：

- A：分组模式。
- B：订阅源的 `playback_mode`。
- C：管理员在频道上手动设置的模式。
- `auto`：源按每个频道的 URL 解析。

### A.1 播放时如何取值

```text
读取频道
  │
  ├─ channel.mode 非空 ──────────────► 用 channel.mode
  │
  └─ channel.mode 为空
        │
        └─► 用 group.mode
              │
              └─ 同步频道 + group.mode = passthrough ?
                    ├─ 是 → 不允许。校验拒绝，加载时修复
                    └─ 否 → 正常使用
```

### A.2 同步创建频道时如何取值

```text
源 playback_mode
   │
   ├─ auto ──► URL 去掉 ? 与 #
   │             ├─ 以 .mpd 结尾 → dash
   │             └─ 其他          → proxy
   │
   └─ B（前五种）──────────────────► B
                    │
                    ▼
        channel.mode = 结果（显式值）
        mode_source = sync:auto / sync:B
```

### A.3 频道模式状态变化

```text
[未设置 / 继承分组]
   │  新建同步频道 / 按源默认回填
   ▼
[源默认：sync:auto | sync:B]
   │  管理员改模式
   ▼
[手动显式：C]
   │  管理员选「跟随分组」
   ▼
[未设置 / 继承分组]

[源默认] --管理员改 URL / detach--> [手动显式：保留当前值]
```

### A.4 场景矩阵

| # | 场景 | 分组 A | 源 B | 频道状态 | 生效模式 |
| --- | --- | --- | --- | --- | --- |
| 1 | 源新建分组 | B | B 或 `auto` | 新建，显式 B | B。`auto` 时逐频道解析 |
| 2 | 源并入已有分组 | A | B 或 `auto` | 新建，显式 B | B。分组保持 A |
| 3 | 同组手工频道 | A | — | 无频道值 | A |
| 4 | 升级前的同步频道 | A | B | 无频道值 | A。回填后变 B |
| 5 | 管理员改频道模式 | A | B | 显式 C | C |
| 6 | 管理员选「跟随分组」 | A | B | 空 | A |
| 7 | 源模式 B 改为 B' | A | B' | 旧频道显式 B | B。只有新频道用 B' |
| 8 | 上游 URL 变化，源仍 `auto` | A | `auto` | `sync:auto` | 重新解析。`.mpd` ↔ `proxy` |
| 9 | 管理员手动改 URL | A | `auto` | `sync_follow = false` | 保留当前值，转为手动 |
| 10 | 分组模式 A 改为 A' | A' | B | 空频道值 | A'。显式 B / C 不变 |
| 11 | 频道转为手工 | A | — | 保留显式值 | 保留。来源标记清除 |
| 12 | 按源默认回填 | A | B | 空 → 显式 B | B。管理员覆盖与固定频道跳过 |

### A.5 安全场景

| 场景 | 结果 |
| --- | --- |
| 源 `playback_mode = passthrough` | 源配置拒绝 |
| 同步频道显式 `passthrough` | 频道接口 `400`，配置校验拒绝，前端隐藏 |
| 分组 `passthrough` + 同步频道模式为空 | 分组更新 `400`；加载时给该频道写入显式安全模式 |
| 手工频道 `passthrough` | 允许。订阅输出原始 URL，不套 Token |
| 同步频道 `redirect`（302） | 允许。入口校验 Token，媒体流不过本机 |
