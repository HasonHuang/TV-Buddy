# 频道级工作模式 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 每个频道可以覆盖工作模式，未设置时继承分组；同步源把工作模式写入新建频道，并提供存量回填。

**Architecture:** `Channel.mode: Option<String>` 是唯一的覆盖层，`Group.mode` 是默认值。播放、订阅、探测、录制、时移统一通过 `Group::effective_mode` 取值。同步创建频道时把源模式落成显式值，并用 `mode_source` 标记来源。同步频道不能通过显式或继承得到 `passthrough`。

**Tech Stack:** Rust（axum、serde、SQLite 配置存储）、React 18 + Vite、自绘 UI 组件。

**Spec:** `docs/superpowers/specs/2026-11-22-channel-level-mode-design.md`

## Global Constraints

- 模式值只有六种：`proxy`、`remux`、`dash`、`redirect`、`rewrite`、`passthrough`。
- 频道缺省模式表示继承分组。不要写 `"inherit"` 字符串。
- `mode_source` 只有三种形状：`sync:auto`、`sync:<模式>`、`manual`。
- 同步频道（`source_id` 非空）的有效模式不能是 `passthrough`。
- `format`、`redirect_ttl`、`redirect_mode`、`remux_container`、`mpd_keep_seconds` 仍留在分组级。
- 不新增 Rust 或前端依赖。
- 注释与文档使用中文。文案短句、主动语态。
- 提交信息沿用仓库的 Conventional Commits 风格。

## Review Focus

- 混合分组订阅：直通频道输出原始 URL，其他频道输出本地入口。见 Task 2、Task 3。
- 同步频道在直通分组里：显式设置和继承都必须被拒绝，不能绕过凭证。见 Task 1、Task 6、Task 8。
- 回填：管理员显式选过「跟随分组」的频道不能被再次覆盖。见 Task 5、Task 7。
- 源 `auto`：只有创建时是 `auto`、且当前源仍是 `auto` 的频道才按 URL 重算。见 Task 5。
- 改分组模式：只影响没有显式模式的频道；显式值不变。见 Task 3、Task 5、Task 11。

---

## File Structure

| 文件 | 职责 |
| --- | --- |
| `src/config/model.rs` | 字段、模式常量、有效模式 helper、`mode_source` helper、加载修复 |
| `src/config.rs` | 配置校验 |
| `src/playlist.rs` | 订阅地址按频道模式生成 |
| `src/http/routes/stream.rs` | 播放入口按频道模式分发 |
| `src/quality/engine.rs` | 质量探测按频道跳过直通 |
| `src/schedule.rs` | 录制按频道模式分流 |
| `src/dash_timeshift.rs` | DASH 时移按频道模式启用 |
| `src/http/routes/dash.rs` | DASH 时移状态回显 |
| `src/subscription.rs` | 同步默认落盘、`auto` 重算、回填函数、detach 标记 |
| `src/http/routes/groups.rs` | 频道/分组接口校验与来源标记 |
| `src/http/routes/subs.rs` | 回填接口 |
| `src/http.rs` | 回填路由注册 |
| `src/http/routes/panel.rs` | `channel_modes` 统计 |
| `frontend/src/modes.ts` | 频道模式选项 |
| `frontend/src/groups.tsx` | 频道弹窗、列表徽标、分组提示 |
| `frontend/src/playlist.tsx` | 源抽屉回填入口与文案 |
| `frontend/src/ops.tsx` | 安全卡直通频道统计 |
| `frontend/src/overview.tsx` | 概览模式分布 |
| `tests/http_contract.rs` | 接口契约测试 |
| `docs/…`、`README.md`、`PROGRESS.md` | 文档与验收 |

---

### Task 1: 配置模型、有效模式与校验

**Files:**
- Modify: `src/config/model.rs`
- Modify: `src/config.rs`
- Test: 两个文件内的 `#[cfg(test)]` 模块

**Interfaces:**
- Produces:
  - `pub const MODE_VALUES: [&str; 6]`
  - `pub fn valid_mode(mode: &str) -> bool`
  - `pub fn is_dash_url(url: &str) -> bool`
  - `pub const MODE_SOURCE_KEY: &str` / `pub const MODE_SOURCE_MANUAL: &str`
  - `Channel.mode: Option<String>`
  - `Channel::mode_source(&self) -> &str`
  - `Channel::set_mode_source(&mut self, value: &str)`
  - `Channel::clear_mode_source(&mut self)`
  - `Group::effective_mode<'a>(&'a self, channel: &'a Channel) -> &'a str`
  - `Config::effective_mode(&self, gid: &str, cid: &str) -> Option<&str>`

- [ ] **Step 1: 写模型失败测试**

在 `src/config/model.rs` 测试模块加入这些用例：

- `channel_mode_inherits_group_and_overrides_it`：分组 `rewrite`，频道空 → `rewrite`；频道 `Some("redirect")` → `redirect`。
- `channel_mode_is_omitted_when_none`：`serde_json::to_value(Channel::default())` 的结果没有 `mode` 键。
- `is_dash_url_ignores_query_case_and_fragment`：`a.MPD?token=1` 与 `a.mpd#frag` 为真；`a.m3u8?x=.mpd` 为假。
- `mode_source_helpers_round_trip`：空 → `set("sync:auto")` → 读回 → `clear` → 空。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test config::model::tests::channel_mode -- --nocapture`
Expected: 编译失败，提示 `mode` 字段或 helper 不存在。

- [ ] **Step 3: 实现模型与 helper**

在 `src/config/model.rs` 加入常量与函数：

```rust
pub const MODE_VALUES: [&str; 6] = ["proxy", "remux", "dash", "redirect", "rewrite", "passthrough"];
pub const MODE_SOURCE_KEY: &str = "mode_source";
pub const MODE_SOURCE_MANUAL: &str = "manual";

pub fn valid_mode(mode: &str) -> bool {
    MODE_VALUES.contains(&mode)
}

pub fn is_dash_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".mpd")
}
```

给 `Channel` 增加：

```rust
/// 频道级工作模式；`None` 表示继承分组。
#[serde(skip_serializing_if = "Option::is_none")]
pub mode: Option<String>,
```

`Channel::default()` 补 `mode: None`。增加方法：

```rust
impl Channel {
    pub fn mode_source(&self) -> &str;
    pub fn set_mode_source(&mut self, value: &str);
    pub fn clear_mode_source(&mut self);
}
```

`mode_source` 读 `extra[MODE_SOURCE_KEY]`，空值返回 `""`；set 写入字符串；clear 删除键。

给 `Group` 和 `Config` 增加：

```rust
impl Group {
    pub fn effective_mode<'a>(&'a self, channel: &'a Channel) -> &'a str {
        channel.mode.as_deref().unwrap_or(self.mode.as_str())
    }
}

impl Config {
    pub fn effective_mode(&self, gid: &str, cid: &str) -> Option<&str>;
}
```

`Config::effective_mode` 找分组与频道，返回 `Some(group.effective_mode(channel))`。同步模块的 `is_mpd` 改为调用 `is_dash_url` 并删除重复实现。

- [ ] **Step 4: 运行模型测试确认通过**

Run: `cargo test config::model::tests -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 写校验失败测试**

在 `src/config.rs` 测试模块加入用例：

- `validate_rejects_unknown_group_and_channel_modes`：频道 `magic` → 错误包含 `magic`；分组 `magic` → 同样。
- `validate_rejects_sync_channel_passthrough`：`source_id = "s1"`、频道 `passthrough` → 错误包含 `passthrough`。
- `validate_rejects_passthrough_group_with_unset_sync_channel`：分组 `passthrough`、同步频道 `mode = None` → 错误包含 `passthrough`。

测试夹具：`Config::default()`，推入一个 `Group`，组内一个 `Channel`。

- [ ] **Step 6: 运行校验测试确认失败**

Run: `cargo test config::tests -- --nocapture`
Expected: FAIL，非法值未被拒绝。

- [ ] **Step 7: 实现校验**

在 `src/config.rs::validate` 的分组循环中：

```rust
if !crate::config::model::valid_mode(&group.mode) {
    bail!("分组 {} 的 mode 不支持 {}", group.id, group.mode);
}
for channel in &group.channels {
    if let Some(mode) = channel.mode.as_deref() {
        if !crate::config::model::valid_mode(mode) {
            bail!("频道 {}/{} 的 mode 不支持 {mode}", group.id, channel.id);
        }
        if mode == "passthrough" && !channel.source_id.is_empty() {
            bail!("同步频道 {}/{} 不能使用 passthrough", group.id, channel.id);
        }
    }
    if !channel.source_id.is_empty()
        && channel.mode.is_none()
        && group.mode == "passthrough"
    {
        bail!(
            "分组 {} 使用 passthrough，但同步频道 {} 未设置工作模式",
            group.id,
            channel.id
        );
    }
}
```

- [ ] **Step 8: 运行测试确认通过**

Run: `cargo test config:: -- --nocapture`
Expected: PASS。

- [ ] **Step 9: 提交**

```bash
git add src/config/model.rs src/config.rs
git commit -m "feat(config): add channel-level work mode"
```

---

### Task 2: 订阅地址按频道模式生成

**Files:**
- Modify: `src/playlist.rs`
- Test: `src/playlist.rs` 测试模块

**Interfaces:**
- Consumes: `Group::effective_mode`（Task 1）。

- [ ] **Step 1: 写失败测试**

测试 `mixed_group_uses_channel_mode_for_entry_url`：

- 分组 `g1`，`mode = "rewrite"`。
- 频道 `inherit`（无 mode）与 `direct`（`mode = Some("passthrough")`）。
- `render_user(&config, &admin(), "http://host", "tok", None, None)` 的结果：
  - 包含 `http://host/api/play/token/tok/g1/inherit.m3u8`
  - 包含 `http://upstream/direct.m3u8`
  - 不包含 `/g1/direct.m3u8`

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test playlist::tests::mixed_group -- --nocapture`
Expected: FAIL，直通频道仍输出本地入口。

- [ ] **Step 3: 实现**

在 `src/playlist.rs` 把三处判断改为 `group.effective_mode(channel)`：

- `entry_url` 的 `if group.mode == "passthrough"`。
- `push_entry` 无令牌分支的 `None if group.mode == "passthrough"`。
- `manual_hidden` 的 `if group.mode == "passthrough" && !config.quality.probe_passthrough`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test playlist::tests -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/playlist.rs
git commit -m "feat(playlist): use channel-level mode for subscription URLs"
```

---

### Task 3: 播放入口按频道模式分发

**Files:**
- Modify: `src/http/routes/stream.rs`
- Modify: `tests/http_contract.rs`（在上游 router 加 `/live2.m3u8`）

**Interfaces:**
- Consumes: `Group::effective_mode`、`Config::effective_mode`（Task 1）。

- [ ] **Step 1: 写契约失败测试**

测试 `mixed_group_routes_each_channel_by_its_mode`：

1. 用 `fixture()` 建夹具，`login` 取 token。
2. 用 `POST /panel/api/groups/import` 导入分组 `mix`：
   - 分组 `mode = "redirect"`。
   - `inherit`：无 mode，URL 为 `fixture.upstream + "/live.m3u8"`。
   - `direct`：`mode = "passthrough"`，URL 为 `fixture.upstream + "/live2.m3u8"`。
   - `proxied`：`mode = "proxy"`，URL 为 `fixture.upstream + "/live.m3u8"`。
3. 断言：
   - `GET /mix/inherit.m3u8?token=...` → `302`，`location` 是上游 `/live.m3u8`。
   - `GET /mix/direct.m3u8?token=...` → `302`，`location` 是上游 `/live2.m3u8`。
   - `GET /mix/proxied.m3u8?token=...` → `200`，正文包含 `#EXTM3U`。

在 `fixture_with_options` 的上游 router 加 `.route("/live2.m3u8", get(|| async { LIVE_BODY }))`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --test http_contract mixed_group_routes_each_channel_by_its_mode -- --nocapture`
Expected: FAIL，`direct` 返回本地入口或 `proxied` 返回 302。

- [ ] **Step 3: 实现**

在 `src/http/routes/stream.rs`：

- `play_entry` 取 `let mode = group.effective_mode(channel);`，用 `mode` 做直通判断与 `match`。
- `serve_rest` 用 `let mode = config.effective_mode(gid, cid);`：
  - 签名门槛：`matches!(mode, Some("proxy" | "remux" | "dash"))`。
  - DASH 分支：`mode == Some("dash")`。
  - 非代理占位：`mode != Some("proxy")`。
- `variant_playlist`：找到频道后判断 `group.effective_mode(channel) != "proxy"`。
- `mpd_segment`：已有 `channel`，把 `group.mode != "proxy"` 改为 `group.effective_mode(channel) != "proxy"`。

- [ ] **Step 4: 运行契约测试确认通过**

Run: `cargo test --test http_contract mixed_group_routes_each_channel_by_its_mode -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/http/routes/stream.rs tests/http_contract.rs
git commit -m "feat(stream): route playback by channel mode"
```

---

### Task 4: 质量、录制、时移按频道模式判断

**Files:**
- Modify: `src/quality/engine.rs`
- Modify: `src/schedule.rs`
- Modify: `src/dash_timeshift.rs`
- Modify: `src/http/routes/dash.rs`
- Modify: `tests/http_contract.rs`

**Interfaces:**
- Consumes: `Group::effective_mode`（Task 1）。

- [ ] **Step 1: 写质量失败测试**

测试 `guard_base_channels_uses_channel_effective_mode`：

- 分组 `mix`，`mode = "passthrough"`。
- 两个同步频道：`inherit`（无 mode）、`proxied`（`mode = "proxy"`）。
- 源 `s1` 存在且启用，`config.quality.enabled = true`，`probe_passthrough = false`。
- 断言 `guard_base_channels(&config, &RunScope::All) == 1`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test quality::engine::tests::guard_base_channels_uses_channel_effective_mode -- --nocapture`
Expected: FAIL，计数为 2 或 0。

- [ ] **Step 3: 实现质量判断**

- `due_targets`：删除分组级 `if group.mode == "passthrough" ... continue;`。在频道循环内、URL 判空之后加入：

```rust
if group.effective_mode(channel) == "passthrough" && !config.quality.probe_passthrough {
    continue;
}
```

- `guard_base_channels`：删除分组级 `.filter(|group| group.mode != "passthrough" || ...)`。在频道过滤链里加入：

```rust
.filter(|channel| {
    group.effective_mode(channel) != "passthrough" || config.quality.probe_passthrough
})
```

- [ ] **Step 4: 实现录制与时移判断**

- `src/schedule.rs::record`：`if group.effective_mode(channel) == "dash"`。
- `src/dash_timeshift.rs::settings`：先找频道，再判断 `group.effective_mode(channel) != "dash"`；然后判断 `channel.dash_timeshift`。
- `src/dash_timeshift.rs::warm`：逐频道过滤 `group.effective_mode(channel) == "dash"` 且 `channel.dash_timeshift`。
- `src/http/routes/dash.rs::timeshift_status`：`let dash = group.effective_mode(channel) == "dash" && channel.dash_timeshift;`，JSON 的 `"mode"` 改为 `group.effective_mode(channel)`。

- [ ] **Step 5: 写 DASH 时移契约测试**

测试 `dash_timeshift_uses_channel_mode`：

- 用 `fixture_with_extra` 加入分组 `mix`，`mode = "proxy"`。
- 一个频道：`mode = "dash"`、`dash_timeshift = true`、URL 用 `http://up/dash/live.mpd`。
- `login` 取 token，带 `authorization: Bearer <token>` 请求 `GET /panel/api/dash-timeshift/status`。
- 断言 `channels[0].mode == "dash"`。

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test quality::engine::tests::guard_base_channels_uses_channel_effective_mode -- --nocapture`
Run: `cargo test --test http_contract dash_timeshift_uses_channel_mode -- --nocapture`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/quality/engine.rs src/schedule.rs src/dash_timeshift.rs src/http/routes/dash.rs tests/http_contract.rs
git commit -m "feat(quality): gate probing and recording by channel mode"
```

---

### Task 5: 同步默认落盘、auto 重算与 detach 标记

**Files:**
- Modify: `src/subscription.rs`
- Test: `src/subscription.rs` 测试模块

**Interfaces:**
- Consumes: `Channel.mode`、`mode_source` helper、`is_dash_url`（Task 1）。
- Produces:
  - `fn source_mode_marker(source: &PlaylistSource) -> String`
  - `fn refresh_source_mode(source: &PlaylistSource, channel: &mut Channel, planned: &PlannedChannel)`
  - `fn update_fields(source: &PlaylistSource, channel: &mut Channel, planned: &PlannedChannel) -> bool`

- [ ] **Step 1: 写失败测试**

在 `src/subscription.rs` 测试模块加入这些用例（复用现有 `config_with_source`、`source`、`urls`、`run` helper）：

- `sync_creates_channel_with_source_mode`：源 `playback_mode = "redirect"`；跑一次同步；频道 `mode == Some("redirect")`，`mode_source == "sync:redirect"`。
- `auto_source_stamps_each_channel_by_url`：源 `auto`；HLS 与 MPD 两个频道；模式集合包含 `proxy` 与 `dash`；两条 `mode_source == "sync:auto"`。
- `auto_mode_reevaluates_when_source_stays_auto`：源 `auto`；第一次 URL 为 `.m3u8` → `proxy`；第二次同 key、URL 改 `.mpd` → `dash`。
- `source_mode_change_does_not_rewrite_existing_auto_channel`：源从 `auto` 改为 `redirect`；再次同步；频道仍为 `proxy`。
- `sync_does_not_overwrite_manual_mode`：首次同步后把频道设为 `rewrite`、`mode_source = "manual"`；再次同步；仍为 `rewrite`。
- `detach_channel_marks_mode_manual`：首次同步后 `detach_channel`；`mode_source == "manual"`，`mode` 仍为 `Some`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test subscription::tests::sync_creates_channel_with_source_mode -- --nocapture`
Expected: FAIL，`mode` 为空。

- [ ] **Step 3: 实现**

在 `src/subscription.rs` 增加：

```rust
fn source_mode_marker(source: &PlaylistSource) -> String {
    match source.playback_mode.trim() {
        "" | "auto" => "sync:auto".into(),
        other => format!("sync:{other}"),
    }
}

fn refresh_source_mode(
    source: &PlaylistSource,
    channel: &mut Channel,
    planned: &PlannedChannel,
) {
    if channel.mode_source() != "sync:auto" {
        return;
    }
    if !matches!(source.playback_mode.trim(), "" | "auto") {
        return;
    }
    channel.mode = Some(mode_for(source, planned.dash));
}
```

修改 `build_channel`：创建时写 `mode: Some(mode_for(source, planned.dash))`，随后 `channel.set_mode_source(&source_mode_marker(source))`。

修改 `update_fields` 签名与逻辑：

```rust
fn update_fields(
    source: &PlaylistSource,
    channel: &mut Channel,
    planned: &PlannedChannel,
) -> bool {
    let changed = !same_upstream(channel, planned);
    if changed {
        write_upstream(channel, planned);
    }
    refresh_source_mode(source, channel, planned);
    changed
}
```

更新两处调用：阶段二 `update_fields(source, channel, planned)`，阶段四 `update_fields(source, &mut channel, planned)`。

修改 detach：

- `detach_channel`、`detach_group`、`detach_source` 在清空 `source_id` 后调用 `channel.set_mode_source(MODE_SOURCE_MANUAL)`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test subscription::tests::sync_creates_channel_with_source_mode subscription::tests::auto_source -- --nocapture`
Run: `cargo test subscription::tests::source_mode -- --nocapture`
Run: `cargo test subscription::tests::detach -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/subscription.rs
git commit -m "feat(sync): stamp source mode onto new channels"
```

---

### Task 6: 加载时修复直通分组里的同步频道

**Files:**
- Modify: `src/config/model.rs`
- Test: `src/config/model.rs` 测试模块

**Interfaces:**
- Consumes: `MODE_SOURCE_MANUAL`、`valid_mode`（Task 1）。
- Produces: `Config::normalize` 的新行为。

- [ ] **Step 1: 写失败测试**

在 `src/config/model.rs` 测试模块加入：

- `normalize_repairs_sync_channel_in_passthrough_group`：
  - 源 `s1`，`mode = "sync"`，`playback_mode = "redirect"`。
  - 分组 `g`，`mode = "passthrough"`，频道 `c1` 有 `source_id = "s1"`、无 mode。
  - `config.normalize()` 后：`c1.mode == Some("redirect")`，`mode_source == "sync:redirect"`。
- `normalize_uses_redirect_when_source_is_auto`：
  - 同上，但源 `playback_mode = "auto"`，频道 URL 以 `.mpd` 结尾。
  - 修复后仍为 `redirect`，不是 `dash`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test config::model::tests::normalize_repairs -- --nocapture`
Expected: FAIL，模式仍为空。

- [ ] **Step 3: 实现**

在 `Config::normalize` 的源归一化之后加入：

```rust
let source_modes: HashMap<String, String> = self
    .playlist
    .sources
    .iter()
    .map(|source| (source.id.clone(), source.playback_mode.clone()))
    .collect();
for group in &mut self.groups {
    if group.mode != "passthrough" {
        continue;
    }
    let gid = group.id.clone();
    for channel in &mut group.channels {
        if channel.source_id.is_empty() || channel.mode.is_some() {
            continue;
        }
        let source_mode = source_modes.get(&channel.source_id).map(String::as_str);
        let explicit = source_mode.filter(|mode| {
            matches!(*mode, "proxy" | "remux" | "dash" | "redirect" | "rewrite")
        });
        let mode = explicit.unwrap_or("redirect");
        channel.mode = Some(mode.to_string());
        let marker = match source_mode {
            Some("") | Some("auto") => "sync:auto".to_string(),
            Some(value) if matches!(value, "proxy" | "remux" | "dash" | "redirect" | "rewrite") => {
                format!("sync:{value}")
            }
            _ => MODE_SOURCE_MANUAL.to_string(),
        };
        channel.set_mode_source(&marker);
        tracing::warn!(
            group = %gid,
            channel = %channel.id,
            mode,
            "分组为直通，同步频道缺少工作模式；已写入安全模式"
        );
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test config::model::tests::normalize -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/config/model.rs
git commit -m "fix(config): repair sync channels under passthrough groups"
```

---

### Task 7: 回填函数与接口

**Files:**
- Modify: `src/subscription.rs`
- Modify: `src/http/routes/subs.rs`
- Modify: `src/http.rs`
- Modify: `tests/http_contract.rs`

**Interfaces:**
- Consumes: `source_mode_marker`（Task 5）、`Channel::mode_source`（Task 1）。
- Produces:
  - `pub fn backfill_modes(config: &mut Config, source_id: &str) -> (usize, BTreeMap<String, usize>)`
  - `pub async fn panel_backfill_mode(...) -> Result<Json<Value>, ApiError>`
  - 路由 `/subs/:id/backfill-mode`（挂在 `panel_api_routes`）。

- [ ] **Step 1: 写回填函数失败测试**

在 `src/subscription.rs` 测试模块加入：

- `backfill_stamps_only_pre_feature_channels`：
  - 源 `s1`，`mode = "sync"`，`playback_mode = "redirect"`。
  - 分组 `g`，`mode = "rewrite"`，三个同步频道：
    - `unset`：无 mode。
    - `manual-inherit`：无 mode，`mode_source = "manual"`。
    - `override`：`mode = Some("proxy")`。
  - `backfill_modes(&mut config, "s1")` → `updated == 1`，`modes["redirect"] == 1`。
  - `unset` 变成 `redirect`；`manual-inherit` 仍为空；`override` 仍为 `proxy`。
- `backfill_uses_auto_resolution_and_is_idempotent`：
  - 源 `auto`；两个频道 `.m3u8` 与 `.mpd`。
  - 第一次回填 → `updated == 2`，`modes` 含 `proxy` 与 `dash`。
  - 第二次回填 → `updated == 0`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test subscription::tests::backfill -- --nocapture`
Expected: 编译失败，`backfill_modes` 不存在。

- [ ] **Step 3: 实现回填函数**

在 `src/subscription.rs` 增加：

```rust
pub fn backfill_modes(
    config: &mut Config,
    source_id: &str,
) -> (usize, std::collections::BTreeMap<String, usize>) {
    let Some(source) = config
        .playlist
        .sources
        .iter()
        .find(|source| source.id == source_id && source.mode == "sync")
    else {
        return (0, std::collections::BTreeMap::new());
    };
    let marker = source_mode_marker(source);
    let mut updated = 0;
    let mut modes = std::collections::BTreeMap::new();
    for group in &mut config.groups {
        for channel in &mut group.channels {
            if channel.source_id != source_id
                || channel.mode.is_some()
                || !channel.sync_follow
                || channel.mode_source() == MODE_SOURCE_MANUAL
            {
                continue;
            }
            let mode = mode_for(source, is_dash_url(&channel.url));
            channel.mode = Some(mode.clone());
            channel.set_mode_source(&marker);
            *modes.entry(mode).or_insert(0) += 1;
            updated += 1;
        }
    }
    (updated, modes)
}
```

- [ ] **Step 4: 写接口失败测试**

测试 `backfill_mode_endpoint_stamps_unset_sync_channels`：

1. `fixture()` + `login`。
2. `POST /panel/api/playlist/sources` 创建同步源 `s1`，`playback_mode = "redirect"`。
3. `POST /panel/api/groups/import` 导入分组 `g`（`mode = "rewrite"`），频道 `c1` 有 `source_id = "s1"`、无 mode。
4. `POST /panel/api/subs/s1/backfill-mode` → `200`，`updated == 1`，`modes["redirect"] == 1`。
5. `GET /panel/api/groups/export`，找到 `id == "g"` 的分组；`c1.mode == "redirect"`，`c1.mode_source == "sync:redirect"`。

如果创建源接口要求更多字段，按 `Source` 类型补空值。

- [ ] **Step 5: 实现接口**

在 `src/http/routes/subs.rs` 增加：

```rust
/// `POST /subs/:id/backfill-mode`——按源默认回填未设置模式的同步频道。
pub async fn panel_backfill_mode(
    PanelAuth(_): PanelAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let source = state
        .snapshot()
        .playlist
        .sources
        .iter()
        .find(|source| source.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("Source not found"))?;
    if source.mode != "sync" {
        return Err(ApiError::bad_request("只有托管源可以回填频道模式"));
    }

    let mut updated = 0;
    let mut modes = std::collections::BTreeMap::new();
    state
        .mutate(|config| {
            let result = crate::subscription::backfill_modes(config, &id);
            updated = result.0;
            modes = result.1;
            Ok(())
        })
        .map_err(|error| ApiError::internal(error.to_string()))?;

    Ok(Json(json!({ "ok": true, "updated": updated, "modes": modes })))
}
```

在 `src/http.rs::panel_api_routes` 的 `/subs/:id/content` 旁加入：

```rust
.route(
    "/subs/:id/backfill-mode",
    post(routes::subs::panel_backfill_mode),
)
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test subscription::tests::backfill -- --nocapture`
Run: `cargo test --test http_contract backfill_mode_endpoint -- --nocapture`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/subscription.rs src/http/routes/subs.rs src/http.rs tests/http_contract.rs
git commit -m "feat(sync): add channel mode backfill"
```

---

### Task 8: 频道与分组接口的模式校验和来源标记

**Files:**
- Modify: `src/http/routes/groups.rs`
- Modify: `tests/http_contract.rs`

**Interfaces:**
- Consumes: `valid_mode`、`MODE_SOURCE_MANUAL`、`Channel::set_mode_source`（Task 1）。
- Produces: 无新公开接口。

- [ ] **Step 1: 写契约失败测试**

测试 `channel_mode_override_and_clear`：

1. `fixture()` + `login`。
2. `PUT /panel/api/groups/pg/channels/c1`，体 `{ "mode": "redirect" }` → `200`。
3. `GET /panel/api/groups/export`：`pg` 的 `c1.mode == "redirect"`，`mode_source == "manual"`。
4. `PUT` 同路径，体 `{ "mode": null }` → `200`。
5. 再次导出：`c1` 没有 `mode` 键，`mode_source == "manual"`。

测试 `invalid_and_sync_passthrough_modes_are_rejected`：

1. `fixture_with_extra` 写入源 `s1`（`mode = "sync"`，`playback_mode = "redirect"`）与分组 `sg`（`mode = "proxy"`），频道 `c1` 有 `source_id = "s1"`、无 mode。
2. `PUT /panel/api/groups/sg/channels/c1`，体 `{ "mode": "magic" }` → `400`。
3. 同路径，体 `{ "mode": "passthrough" }` → `400`。
4. `PUT /panel/api/groups/sg`，体 `{ "mode": "passthrough" }` → `400`。
5. 先把 `c1` 设为 `redirect`（`200`），再把分组设为 `passthrough`（`200`）。
6. 在 `passthrough` 分组里把 `c1` 设为 `null` → `400`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --test http_contract channel_mode_override_and_clear -- --nocapture`
Expected: FAIL，`mode_source` 不存在或模式未被接收。

- [ ] **Step 3: 实现**

在 `src/http/routes/groups.rs` 增加两个 helper：

```rust
fn validate_channel_mode(group: &Group, channel: &Channel) -> Result<(), ApiError>;

fn validate_group_mode(group: &Group) -> Result<(), ApiError>;
```

`validate_channel_mode`：

- `Some(mode)` 必须 `valid_mode`。
- 同步频道（`source_id` 非空）不能显式 `passthrough`。
- 同步频道 `mode = None` 且 `group.mode == "passthrough"` → `400`。

`validate_group_mode`：

- `group.mode` 必须 `valid_mode`。
- `group.mode == "passthrough"` 且组内存在 `source_id` 非空、`mode = None` 的频道 → `400`。

接入：

- `create_channel`：`merge_into` 后调用 `validate_channel_mode`。若 `body.get("mode").is_some()`，`channel.set_mode_source(MODE_SOURCE_MANUAL)`；否则 `channel.clear_mode_source()`。
- `update_channel`：`merge_into` 后调用 `validate_channel_mode`。`body.get("mode").is_some()` 时写 `manual`。上游变化导致 `sync_follow = false` 时也写 `manual`。
- `create_group` 与 `update_group`：`merge_into` 后调用 `validate_group_mode`。
- `import`：对每个导入分组调用 `validate_group_mode`，非法值返回 `400`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test --test http_contract channel_mode_override_and_clear -- --nocapture`
Run: `cargo test --test http_contract invalid_and_sync_passthrough_modes_are_rejected -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/http/routes/groups.rs tests/http_contract.rs
git commit -m "feat(api): validate channel modes and track mode source"
```

---

### Task 9: 状态接口返回有效模式分布

**Files:**
- Modify: `src/http/routes/panel.rs`
- Modify: `tests/http_contract.rs`

**Interfaces:**
- Consumes: `Group::effective_mode`（Task 1）。
- Produces: `GET /panel/api/status` 的 `channel_modes` 字段。

- [ ] **Step 1: 写失败测试**

测试 `status_counts_channels_by_effective_mode`：

- 用 `fixture_with_extra` 加入分组 `mix`，`mode = "rewrite"`，两个频道：
  - `a`：无 mode。
  - `b`：`mode = "redirect"`。
- `GET /panel/api/status`：
  - `channel_modes["redirect"] == 1`
  - `channel_modes["rewrite"] >= 1`

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --test http_contract status_counts_channels_by_effective_mode -- --nocapture`
Expected: FAIL，`channel_modes` 不存在。

- [ ] **Step 3: 实现**

在 `src/http/routes/panel.rs::status` 中按有效模式统计频道：

```rust
let mut channel_modes: serde_json::Map<String, Value> = serde_json::Map::new();
for group in &cfg.groups {
    for channel in &group.channels {
        let mode = group.effective_mode(channel);
        let count = channel_modes.entry(mode.to_string()).or_insert_with(|| json!(0));
        *count = json!(count.as_u64().unwrap_or(0) + 1);
    }
}
```

在响应 JSON 中加入 `"channel_modes": channel_modes`。保留 `group_modes`。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test --test http_contract status_counts_channels_by_effective_mode -- --nocapture`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/http/routes/panel.rs tests/http_contract.rs
git commit -m "feat(panel): report channel mode distribution"
```

---

### Task 10: 前端模式选项与频道表单状态

**Files:**
- Modify: `frontend/src/modes.ts`
- Modify: `frontend/src/groups.tsx`

**Interfaces:**
- Produces: `channelModeOptions(groupMode: string, sync: boolean, allowInherit?: boolean): ModeOption[]`
- Produces: `ChannelForm` 新字段 `mode: string`、`originalMode: string`、`modeFromSource: boolean`。

- [ ] **Step 1: 增加模式选项**

在 `frontend/src/modes.ts` 增加：

```ts
/** 频道模式选项：首项继承分组；同步频道不显示 passthrough。 */
export function channelModeOptions(
  groupMode: string,
  sync: boolean,
  allowInherit = true,
): ModeOption[] {
  const inherited: ModeOption[] = allowInherit
    ? [{ value: '', label: `跟随分组（${modeText(groupMode)}）`, desc: '使用分组的工作模式。' }]
    : [];
  return [...inherited, ...(sync ? syncModeOptions() : modeOptions())];
}
```

- [ ] **Step 2: 扩展前端类型**

在 `frontend/src/groups.tsx`：

```ts
type Channel = {
  // ...
  mode?: string;
  mode_source?: string;
  [key: string]: unknown;
};

type ChannelForm = {
  // ...
  mode: string;
  originalMode: string;
  modeFromSource: boolean;
};
```

在 `openChannel` 的已有频道分支加入：

```ts
mode: channel.mode ?? '',
originalMode: channel.mode ?? '',
modeFromSource: text(channel, 'mode_source').startsWith('sync:'),
```

新建分支加入 `mode: ''`、`originalMode: ''`、`modeFromSource: false`。

- [ ] **Step 3: 保存时只在变更后发送 mode**

在 `saveChannel` 的 payload 构造之后：

```ts
if (channelForm.isNew || form.mode !== form.originalMode) {
  payload.mode = form.mode === '' ? null : form.mode;
}
```

- [ ] **Step 4: 构建确认通过**

Run: `cd frontend && npm run build`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add frontend/src/modes.ts frontend/src/groups.tsx
git commit -m "feat(panel): add channel mode form state"
```

---

### Task 11: 频道弹窗、列表徽标与分组提示

**Files:**
- Modify: `frontend/src/groups.tsx`

**Interfaces:**
- Consumes: `channelModeOptions`（Task 10）、`modeCode`、`modeTip`。

- [ ] **Step 1: 增加导入并放频道模式字段**

在 modes 导入里加入 `channelModeOptions`：

```ts
import { channelModeOptions, modeCode, modeOptions, modeTip } from './modes';
```

在频道弹窗的 `<div className="preview">…</div>` 之后、回看 `<details>` 之前加入：

```tsx
<Field
  label="工作模式"
  tip="频道可以单独覆盖分组的工作模式。未设置时跟随分组。"
  hint={
    channelForm.form.modeFromSource
      ? '当前值来自订阅源默认。手动修改后以频道为准。'
      : undefined
  }
>
  <Select
    ariaLabel="工作模式"
    value={channelForm.form.mode}
    options={channelModeOptions(
      groups.find((group) => group.id === channelForm.gid)?.mode ?? 'proxy',
      channelForm.form.sourceId !== '',
      !(
        channelForm.form.sourceId !== '' &&
        (groups.find((group) => group.id === channelForm.gid)?.mode ?? 'proxy') ===
          'passthrough'
      ),
    )}
    onChange={(next) =>
      setChannelForm({
        ...channelForm,
        form: { ...channelForm.form, mode: next, modeFromSource: false },
      })
    }
    width={220}
  />
</Field>
```

- [ ] **Step 2: 列表按有效模式显示**

在频道行渲染中：

```tsx
const effectiveMode = channel.mode ?? group.mode;
const modeDiff = effectiveMode !== group.mode;
const local =
  effectiveMode === 'passthrough'
    ? channel.url
    : playUrl(group, channel, playToken);
const allTracks =
  effectiveMode === 'dash' && (channel.selected_tracks ?? []).length === 0;
```

替换原来的 `group.mode === 'passthrough' ? ...` 与 `group.mode === 'dash'`。

在 `ch-badges` 行、来源徽标之后加入：

```tsx
{modeDiff && (
  <HoverTip text={modeTip(effectiveMode)}>
    <Badge tone={effectiveMode === 'passthrough' ? 'warn' : 'muted'}>
      {effectiveMode === 'passthrough' ? '直通' : modeCode(effectiveMode)}
    </Badge>
  </HoverTip>
)}
```

删除原来的 `{group.mode === 'passthrough' && <Badge tone="muted">直通</Badge>}`。

- [ ] **Step 3: 分组弹窗提示**

在分组弹窗的工作模式 `Field` 下方加入：

```tsx
{(() => {
  const current = groups.find((group) => group.id === groupForm.form.id);
  const explicit = (current?.channels ?? []).filter((channel) => channel.mode).length;
  const unsafe = (current?.channels ?? []).filter(
    (channel) => channel.source_id && !channel.mode,
  ).length;
  return (
    <>
      {explicit > 0 && (
        <p className="hint">{explicit} 个频道使用单独设置的模式，不受分组修改影响。</p>
      )}
      {groupForm.form.mode === 'passthrough' && unsafe > 0 && (
        <p className="msg warn">
          组内有 {unsafe} 个同步频道未设置工作模式，直通会绕过凭证。请先为它们设置模式或按源默认回填。
        </p>
      )}
    </>
  );
})()}
```

- [ ] **Step 4: 构建确认通过**

Run: `cd frontend && npm run build`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add frontend/src/groups.tsx
git commit -m "feat(panel): show channel mode overrides"
```

---

### Task 12: 订阅源抽屉回填入口

**Files:**
- Modify: `frontend/src/playlist.tsx`

**Interfaces:**
- Consumes: `POST /subs/:id/backfill-mode`（Task 7）、`Confirm`、`useToast`。

- [ ] **Step 1: 扩展分组类型与计数**

在 `frontend/src/playlist.tsx`：

```ts
type ExistingGroup = {
  id: string;
  name: string;
  origin?: string;
  channels?: {
    id: string;
    mode?: string | null;
    source_id?: string;
    sync_follow?: boolean;
    mode_source?: string;
  }[];
};

function backfillCount(groups: ExistingGroup[], sourceId: string): number {
  return groups
    .flatMap((group) => group.channels ?? [])
    .filter(
      (channel) =>
        channel.source_id === sourceId &&
        !channel.mode &&
        channel.sync_follow !== false &&
        channel.mode_source !== 'manual',
    ).length;
}

function backfillModeText(mode: string): string {
  return mode === 'auto' || mode === ''
    ? '自动选择（.mpd → dash，其他 → proxy）'
    : modeText(mode);
}
```

- [ ] **Step 2: 增加状态与执行函数**

在组件内：

```ts
const [backfill, setBackfill] = useState<{ id: string; count: number; mode: string } | null>(
  null,
);
const [backfillBusy, setBackfillBusy] = useState(false);

const runBackfill = async () => {
  if (!backfill) return;
  setBackfillBusy(true);
  try {
    const result = await api<{ updated: number }>('POST', `/subs/${backfill.id}/backfill-mode`);
    ok(`已回填 ${result.updated} 个频道`);
    setBackfill(null);
    await load();
    await loadGroups();
  } catch (error) {
    err(error instanceof Error ? error.message : String(error));
  } finally {
    setBackfillBusy(false);
  }
};
```

- [ ] **Step 3: 修改文案并加入按钮**

把「新分组工作模式」改为「新分组与频道默认」，提示改为：

```text
用于之后新建的同步分组和新频道。已有频道不受影响。管理员改过的频道模式不会被覆盖。
```

在该字段下方加入：

```tsx
{drawer.draft.id &&
  drawer.draft.mode === 'sync' &&
  backfillCount(allGroups, drawer.draft.id) > 0 && (
    <div className="row between">
      <span className="hint">
        按源默认回填现有频道（{backfillCount(allGroups, drawer.draft.id)} 条）
      </span>
      <button
        className="ghost"
        type="button"
        onClick={() =>
          setBackfill({
            id: drawer.draft.id,
            count: backfillCount(allGroups, drawer.draft.id),
            mode: drawer.draft.playback_mode,
          })
        }
      >
        回填
      </button>
    </div>
  )}
```

- [ ] **Step 4: 增加确认框**

在页面底部与其他 `Confirm` 并列：

```tsx
{backfill && (
  <Confirm
    title="按源默认回填工作模式"
    text={`将把 ${backfill.count} 个未设置模式的同步频道改为「${backfillModeText(
      backfill.mode,
    )}」。管理员手动设置过的频道不变。`}
    confirmText="回填"
    tone="primary"
    busy={backfillBusy}
    onCancel={() => setBackfill(null)}
    onConfirm={() => void runBackfill()}
  />
)}
```

引入 `modeText`、`Confirm`。`allGroups` 已由 `loadGroups` 从 `/config` 读取。

- [ ] **Step 5: 构建确认通过**

Run: `cd frontend && npm run build`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add frontend/src/playlist.tsx
git commit -m "feat(panel): add source mode backfill action"
```

---

### Task 13: 安全卡与概览分布

**Files:**
- Modify: `frontend/src/ops.tsx`
- Modify: `frontend/src/overview.tsx`

**Interfaces:**
- Consumes: `GET /panel/api/status` 的 `channel_modes`（Task 9）。

- [ ] **Step 1: 安全卡按频道统计**

在 `frontend/src/ops.tsx` 把 `configGroups` 类型扩展为包含 `channels`，并计算：

```ts
const passthroughChannels = configGroups.flatMap((group) =>
  (group.channels ?? [])
    .filter((channel) => (channel.mode ?? group.mode) === 'passthrough')
    .map((channel) => ({
      group: group.name || group.id,
      channel: channel.name || channel.id,
    })),
);
```

把原来的 `passthroughGroups` 文案替换为：

```tsx
{passthroughChannels.length > 0 && (
  <div className="msg warn">
    有 {passthroughChannels.length} 个频道使用「原样直通」（
    {passthroughChannels
      .slice(0, 3)
      .map((item) => `${item.group} / ${item.channel}`)
      .join('、')}
    {passthroughChannels.length > 3 ? '…' : ''}
    ）。这些频道绕过本机和凭证检查。建议改为「改写地址」或「302 跳转」。
  </div>
)}
```

- [ ] **Step 2: 概览使用 channel_modes**

在 `frontend/src/overview.tsx`：

```ts
const channelModes = (status?.channel_modes ?? status?.group_modes ?? {}) as Record<
  string,
  number
>;
```

把渲染里的 `groupModes` 改为 `channelModes`，说明文字改为「频道按有效工作模式统计（含频道覆盖）」。

- [ ] **Step 3: 构建确认通过**

Run: `cd frontend && npm run build`
Expected: PASS。

- [ ] **Step 4: 提交**

```bash
git add frontend/src/ops.tsx frontend/src/overview.tsx
git commit -m "feat(panel): count passthrough channels and effective modes"
```

---

### Task 14: 文档、验收与整体验证

**Files:**
- Modify: `README.md`
- Modify: `docs/design/03-http-contract.md`
- Modify: `docs/design/04-config-and-storage.md`
- Modify: `docs/design/05-frontend.md`
- Modify: `docs/design/11-subscription-sync.md`
- Modify: `docs/spec/07-baseline-and-verification.md`
- Modify: `docs/spec/08-target-spec.md`
- Modify: `docs/tests/02-panel-acceptance.md`
- Modify: `PROGRESS.md`

- [ ] **Step 1: 更新文档**

按设计文档 §16 的清单逐项更新。至少写明：

- `Channel.mode` 的继承语义与 `mode_source` 取值。
- 源 `playback_mode` 现在同时用于新建频道。
- 回填接口与 `channel_modes` 字段。
- 与 `S-MODE-02` / `S-MODEL-03` 的差异。
- 频道弹窗、列表徽标、订阅源回填、安全卡、概览的新文案。

- [ ] **Step 2: 运行格式化与静态检查**

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Expected: 无输出或 `Finished`。

- [ ] **Step 3: 运行后端测试**

```bash
cargo test --all-targets
```

Expected: PASS。

- [ ] **Step 4: 构建前端**

```bash
cd frontend && npm run build
```

Expected: PASS。

- [ ] **Step 5: 手工验收**

按 `docs/tests/02-panel-acceptance.md` 的新增条目检查：

- 频道弹窗有「工作模式」，默认「跟随分组」。
- 同步频道看不到 `passthrough`。
- 直通分组里的同步频道看不到「跟随分组」。
- 列表只在模式与分组不同时显示徽标。
- 订阅源抽屉显示可回填条数，0 条时按钮不出现。
- 安全卡按频道显示直通数量。
- 概览按有效模式显示频道分布。
- 键盘 Tab 能到达新模式字段与回填按钮；Select 有可见标签；徽标同时有文字，不只靠颜色。

- [ ] **Step 6: 提交**

```bash
git add README.md docs PROGRESS.md
git commit -m "docs: document channel-level work modes"
```

---

## Self-Review

**Spec coverage:**

- 频道字段、继承、序列化：Task 1。
- 源默认落盘、auto 重算、来源标记、detach：Task 5。
- 存量回填：Task 7。
- 播放与订阅：Task 2、Task 3。
- 质量、录制、时移：Task 4。
- 直通安全边界：Task 1、Task 6、Task 8。
- 状态统计：Task 9。
- 前端交互：Task 10、Task 11、Task 12、Task 13。
- 兼容性与基线差异：Task 14。
- 已知限制：设计文档 §14.1，不在本次实现。

**Type consistency:**

- `Channel.mode: Option<String>` 在 Task 1 定义，后续任务统一使用。
- `mode_source` 通过 `Channel::mode_source` / `set_mode_source` 访问，不重复字符串键。
- `backfill_modes` 的返回类型在 Task 7 定义，接口直接序列化。
- 前端 `channelModeOptions` 在 Task 10 定义，Task 11 使用。

**Review Focus 测试覆盖:**

- 混合分组订阅：Task 2、Task 3。
- 同步频道直通：Task 1、Task 6、Task 8。
- 回填不覆盖手动选择：Task 5、Task 7。
- auto 重算边界：Task 5。
- 分组模式变更：Task 3、Task 5、Task 11。
