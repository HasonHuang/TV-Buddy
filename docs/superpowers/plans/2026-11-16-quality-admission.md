# 订阅源质量准入 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把订阅源同步改为逐频道质量准入：合格频道进入分组，不合格立即移出；提供独立质量页、结果记录、cron/小时调度与安全阀。

**Architecture:** 同步门控只处理新候选与 URL 变化频道，周期校验负责已准入频道。配置与结果在同一 SQLite 事务提交。新候选用每源游标分轮准入。结果表按稳定键保留最近一条，不做淘汰池与自动复检。

**Tech Stack:** Rust 2021、axum 0.7、reqwest 0.12、tokio、serde、rusqlite、sha2、React + TypeScript、Vite、Docker、Python 3。

**Spec:** `docs/design/16-quality-admission-system.md`（产品 `14-quality-admission-product.md`，功能 `15-quality-admission-function.md`）

## Global Constraints

- 默认 `quality.enabled=false`。关闭时不做成员变更；手动校验只测量。
- 同步源不合格立即移出分组。手工频道只支持保留或下线，不移出。
- 只对 `mode=sync` 源执行准入。聚合源、归档组不参与。
- 同步门控只处理新候选与 URL 变化频道。未变化的已准入频道由周期校验处理。
- 未判定、预算截断或取消不得移出已准入频道。
- `probe_error` 不移出，也不改变手工频道的下线状态。URL 变化遇 `probe_error` 暂停本轮同步，保留原地址。
- 安全阀默认开启：`max_remove_ratio=0.5`，`min_remove_count=10`。
  - 触发：本轮不改变任何成员，包括新候选准入。
  - 关闭：不触发安全阀；移出照常执行。
- 结果记录默认保留 7 天。`result_retention_days=0` 表示不自动清理。
- 同步门控时限 `gate_time_budget_secs=300`，0 表示不限制。
- 周期支持 `interval`（小时）与 5 段 cron。旧 `interval_secs` 保留解析但不生效。
- 单轮内 URL 去重。按 Host 共享默认关闭。不跨轮复用测量结果。
- 配置与结果必须在同一个 SQLite 事务提交。闭包返回要写入的结果。
- 锁顺序：`sync_lock → admission_lock → config.write_lock → SharedConn`。
- 所有质量接口仅管理员。地址不得完整写入结果或日志。
- 中文注释与文档。按 80% ASD-STE100：短句、主动语态、一句一义。
- 每任务 TDD：先写失败测试，再实现，再跑目标测试与 `cargo test`，最后提交。

## Review Focus

- 门控截断或预算耗尽时，已准入频道数量不变。未处理的新候选不得准入，游标前进。
- URL 变化遇到 `probe_error` 时，保留原地址，不删除频道。
- 同步门控不重复探测未变化的已准入频道；周期校验负责移出。
- 配置写入失败时，结果记录也不落库；结果写入失败时，配置也不改变。
- 安全阀触发时，准入与移出都不发生。安全阀关闭时，移出照常执行。
- 源停用/删除与运行中任务竞争时，不误删频道。
- 手工频道下线状态在重启后仍生效。
- 全局关闭时手动校验只测量，不改成员。

---

### Task 1: 配置字段与共享 cron

**Files:**
- Modify: `src/config/model.rs:900-950`（`QualityCfg`）
- Modify: `src/config.rs:81`（`validate_quality`）
- Create: `src/cron.rs`
- Modify: `src/lib.rs`（`pub mod cron;`）
- Modify: `src/epg.rs:2669-2830`（改为调用共享 cron）
- Test: `src/config/model.rs`、`src/config.rs`、`src/cron.rs`、`src/epg.rs`

**Interfaces:**
- Produces:
  - `QualityCfg { schedule: String, interval_hours: u64, cron: String, guard_enabled: bool, max_remove_ratio: f64, min_remove_count: u64, result_retention_days: u64, gate_time_budget_secs: u64, .. }`
  - `pub fn parse_cron(expr: &str) -> anyhow::Result<CronSpec>`
  - `pub fn cron_matches(spec: &CronSpec, epoch: i64, offset_hours: i64) -> bool`
  - `pub fn cron_prev(expr: &str, now: i64, offset_hours: i64) -> anyhow::Result<i64>`
  - `pub fn cron_preview(expr: &str, now: i64, offset_hours: i64, count: usize) -> anyhow::Result<Vec<i64>>`
  - `pub fn cron_due(expr: &str, now: i64, offset_hours: i64, last_run: i64) -> bool`

- [ ] **Step 1: Write failing config and cron tests**

```rust
// src/config/model.rs
#[test]
fn quality_admission_defaults_are_bounded() {
    let quality = QualityCfg::default();
    assert_eq!(quality.schedule, "interval");
    assert_eq!(quality.interval_hours, 6);
    assert_eq!(quality.cron, "0 */6 * * *");
    assert!(quality.guard_enabled);
    assert_eq!(quality.max_remove_ratio, 0.5);
    assert_eq!(quality.min_remove_count, 10);
    assert_eq!(quality.result_retention_days, 7);
    assert_eq!(quality.gate_time_budget_secs, 300);
}

// src/config.rs
#[test]
fn quality_admission_rejects_bad_schedule_and_guard() {
    let mut config = Config::default();
    config.quality.interval_hours = 0;
    assert!(validate(&config).is_err());
    config.quality.interval_hours = 6;
    config.quality.schedule = "cron".into();
    config.quality.cron = "bad".into();
    assert!(validate(&config).is_err());
    config.quality.cron = "0 */6 * * *".into();
    config.quality.max_remove_ratio = 0.0;
    assert!(validate(&config).is_err());
    config.quality.max_remove_ratio = 0.5;
    config.quality.min_remove_count = 0;
    assert!(validate(&config).is_err());
    config.quality.min_remove_count = 10;
    config.quality.gate_time_budget_secs = 86_401;
    assert!(validate(&config).is_err());
}

// src/cron.rs
#[test]
fn cron_due_uses_last_run_and_rejects_bad_expression() {
    let next = cron_preview("0 * * * *", 1_700_000_000, 0, 1).expect("cron")[0];
    assert!(cron_due("0 * * * *", next, 0, next - 1));
    assert!(!cron_due("0 * * * *", next, 0, next));
    assert!(!cron_due("bad", next, 0, 0), "非法表达式不触发");
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test quality_admission_defaults_are_bounded quality_admission_rejects_bad_schedule_and_guard cron_due_uses_last_run_and_rejects_bad_expression -v`
Expected: FAIL。字段、`cron_due` 不存在。

- [ ] **Step 3: Implement config fields and shared cron**

- 给 `QualityCfg` 增加 8 个字段和默认值：`schedule="interval"`、`interval_hours=6`、`cron="0 */6 * * *"`、`guard_enabled=true`、`max_remove_ratio=0.5`、`min_remove_count=10`、`result_retention_days=7`、`gate_time_budget_secs=300`。
- `validate_quality` 增加范围校验：`interval_hours` 1–8760；`schedule` 为 `interval` 或 `cron`；`cron` 始终可解析；`max_remove_ratio` 在 `(0,1]`；`min_remove_count` 1–100000；`result_retention_days` 0–3650；`gate_time_budget_secs` 0–86400。
- 把 `epg.rs` 的 `CronField`、`CronSpec`、`parse_cron`、`cron_matches`、`cron_prev`、`cron_preview` 移到 `src/cron.rs` 并公开。`epg.rs` 保留 `pub use crate::cron::cron_preview;`，现有 `/epg/cron/preview` 行为不变。
- `cron_due`：表达式非法返回 `false`；否则 `last_run < cron_prev(expr, now, offset_hours)`。
- `src/lib.rs` 增加 `pub mod cron;`。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test cron -v`
Expected: PASS

Run: `cargo test quality_admission_defaults_are_bounded quality_admission_rejects_bad_schedule_and_guard -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config/model.rs src/config.rs src/cron.rs src/epg.rs src/lib.rs
git commit -m "feat(quality): 增加准入配置与共享 cron"
```

---

### Task 2: 质量结果存储

**Files:**
- Modify: `src/db.rs`（建表）
- Create: `src/quality/store.rs`
- Modify: `src/quality/mod.rs`
- Test: `src/quality/store.rs`

**Interfaces:**
- Produces:
  - `pub struct QualityResult { key, scope, source_id, source_key, gid, cid, name, target_group, url_hash, action, reason, ok, delay_ms, speed_kib, width, height, http_status, checked_at, next_check_at, first_fail_at, rule_hash, run_id, updated_at_ms }`
  - `pub struct QualityFilter { source_id, gid, cid, action, reason, q, limit, offset }`，`Default` 的 `limit=0`，`list` 将 0 归一为 50，最大 500。
  - `pub struct QualityCounts { admitted: usize, removed_7d: usize }`
  - `QualityStore::new(conn: SharedConn) -> Self`
  - `QualityStore::upsert(&self, item: &QualityResult) -> anyhow::Result<()>`
  - `QualityStore::upsert_many(&self, items: &[QualityResult]) -> anyhow::Result<()>`
  - `QualityStore::upsert_many_tx(&self, tx: &rusqlite::Transaction<'_>, items: &[QualityResult]) -> anyhow::Result<()>`
  - `QualityStore::get(&self, key: &str) -> anyhow::Result<Option<QualityResult>>`
  - `QualityStore::list(&self, filter: &QualityFilter) -> anyhow::Result<(usize, Vec<QualityResult>)>`
  - `QualityStore::due(&self, now: i64, limit: usize) -> anyhow::Result<Vec<QualityResult>>`
  - `QualityStore::cleanup(&self, retention_days: u64, now: i64) -> anyhow::Result<usize>`
  - `QualityStore::manual_entries(&self) -> anyhow::Result<Vec<QualityResult>>`
  - `QualityStore::counts(&self, since: i64) -> anyhow::Result<QualityCounts>`
  - `QualityStore::delete(&self, key: &str) -> anyhow::Result<()>`
  - `QualityStore::meta_get(&self, key: &str) -> anyhow::Result<Option<String>>`
  - `QualityStore::meta_set(&self, key: &str, value: &str) -> anyhow::Result<()>`
  - `QualityStore::meta_set_tx(&self, tx: &rusqlite::Transaction<'_>, key: &str, value: &str) -> anyhow::Result<()>`

- [ ] **Step 1: Write failing store tests**

```rust
fn store() -> (QualityStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("tvb-quality-{}", uuid::Uuid::new_v4()));
    let conn = crate::db::connect(&dir.join("tvb.db")).expect("db");
    (QualityStore::new(conn), dir)
}

fn result(key: &str, action: &str, checked_at: i64) -> QualityResult {
    QualityResult {
        key: key.into(),
        scope: "sync".into(),
        source_id: "s1".into(),
        source_key: "name:CCTV-5".into(),
        gid: "sports".into(),
        cid: "c5".into(),
        name: "CCTV-5".into(),
        target_group: "体育".into(),
        url_hash: "abc".into(),
        action: action.into(),
        reason: "pass".into(),
        checked_at,
        ..QualityResult::default()
    }
}

#[test]
fn upsert_updates_same_stable_key() {
    let (store, dir) = store();
    store.upsert(&result("sync:s1:name:CCTV-5", "admitted", 1)).unwrap();
    store.upsert(&result("sync:s1:name:CCTV-5", "removed", 2)).unwrap();
    let (total, items) = store.list(&QualityFilter::default()).unwrap();
    assert_eq!(total, 1);
    assert_eq!(items[0].action, "removed");
    assert!(store.get("sync:s1:name:CCTV-5").unwrap().is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cleanup_keeps_admitted_and_recent() {
    let (store, dir) = store();
    let now = 1_000_000;
    store.upsert(&result("sync:s1:a", "admitted", 1)).unwrap();
    store.upsert(&result("sync:s1:b", "removed", now - 8 * 86_400)).unwrap();
    store.upsert(&result("sync:s1:c", "removed", now - 86_400)).unwrap();
    assert_eq!(store.cleanup(7, now).unwrap(), 1);
    assert!(store.get("sync:s1:a").unwrap().is_some(), "admitted 不按保留期删除");
    assert!(store.get("sync:s1:b").unwrap().is_none());
    assert!(store.get("sync:s1:c").unwrap().is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn list_filters_and_meta_roundtrip() {
    let (store, dir) = store();
    store.upsert(&result("sync:s1:a", "admitted", 1)).unwrap();
    store.upsert(&result("sync:s1:b", "removed", 2)).unwrap();
    let filter = QualityFilter { action: Some("removed".into()), ..QualityFilter::default() };
    assert_eq!(store.list(&filter).unwrap().0, 1);
    store.meta_set("next_run_at", "123").unwrap();
    assert_eq!(store.meta_get("next_run_at").unwrap().as_deref(), Some("123"));
    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test quality::store -v`
Expected: FAIL。模块与类型不存在。

- [ ] **Step 3: Implement store and schema**

- `db.rs` 增加 `quality_results` 与 `quality_meta`，列按系统设计 §4.1。索引：`checked_at`、`(source_id, checked_at)`、`(action, checked_at)`。
- `store.rs` 用 `SharedConn`。`upsert` 使用 `ON CONFLICT(key) DO UPDATE`，并把 `updated_at_ms` 设为当前毫秒。
- `list` 动态拼接 `WHERE`，参数绑定；先 `COUNT(*)` 再分页。支持 `source_id`、`gid`、`cid`、`action`、`reason`、`q`（名称模糊匹配）。
- `due` 查询 `next_check_at <= now`，按 `next_check_at` 升序，限制条数。
- `cleanup` 删除 `action != 'admitted' AND checked_at < cutoff`。`cutoff = now - retention_days*86400`。
- `manual_entries` 查询 `scope='manual'`。
- `counts` 统计 `admitted` 与最近 `since` 之后的 `removed`。
- `delete` 单行删除。`meta_set`/`meta_get` 直接读写 `quality_meta`。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test quality::store -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/db.rs src/quality/store.rs src/quality/mod.rs
git commit -m "feat(quality): 增加质量结果存储"
```

---

### Task 3: 原子提交与状态扩展

**Files:**
- Modify: `src/config/store.rs`（事务内读取与保存）
- Modify: `src/state.rs`（`conn`、`quality_store`、`admission_lock`、`mutate_admission`）
- Test: `src/state.rs`

**Interfaces:**
- Produces:
  - `ConfigStore::load_tx(&self, tx: &rusqlite::Transaction<'_>) -> anyhow::Result<Option<Config>>`
  - `ConfigStore::save_tx(&self, tx: &rusqlite::Transaction<'_>, config: &Config) -> anyhow::Result<u64>`
  - `AppState::mutate_admission<F>(&self, meta: &[(String, String)], change: F) -> anyhow::Result<Vec<QualityResult>> where F: FnOnce(&mut Config) -> anyhow::Result<Vec<QualityResult>>`
  - `AppState.conn: SharedConn`
  - `AppState.quality_store: QualityStore`
  - `AppState.admission_lock: Arc<tokio::sync::Mutex<()>>`

- [ ] **Step 1: Write failing transaction tests**

```rust
fn test_state() -> (std::sync::Arc<AppState>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("tvb-state-{}", uuid::Uuid::new_v4()));
    let state = AppState::new(
        dir.join("tvb.db"),
        Config::default(),
        std::sync::Arc::new(crate::logging::RingBuffer::default()),
    )
    .expect("state");
    (std::sync::Arc::new(state), dir)
}

#[test]
fn mutate_admission_commits_config_and_results() {
    let (state, dir) = test_state();
    let item = QualityResult { key: "sync:s:c".into(), action: "admitted".into(), ..QualityResult::default() };
    let written = state
        .mutate_admission(&[("last_run_at".into(), "1".into())], |config| {
            config.groups.push(Group { id: "g".into(), ..Group::default() });
            Ok(vec![item])
        })
        .unwrap();
    assert_eq!(written.len(), 1);
    assert!(state.snapshot().group("g").is_some());
    assert!(state.quality_store.get("sync:s:c").unwrap().is_some());
    assert_eq!(state.quality_store.meta_get("last_run_at").unwrap().as_deref(), Some("1"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mutate_admission_rolls_back_on_closure_error() {
    let (state, dir) = test_state();
    let error = state
        .mutate_admission(&[], |config| {
            config.groups.push(Group { id: "g".into(), ..Group::default() });
            anyhow::bail!("stop");
        })
        .unwrap_err();
    assert!(error.to_string().contains("stop"));
    assert!(state.snapshot().group("g").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mutate_admission_rolls_back_on_invalid_config() {
    let (state, dir) = test_state();
    let error = state
        .mutate_admission(&[], |config| {
            config.server.bind = String::new();
            Ok(vec![QualityResult { key: "sync:s:c".into(), ..QualityResult::default() }])
        })
        .unwrap_err();
    assert!(!error.to_string().is_empty());
    assert!(state.quality_store.get("sync:s:c").unwrap().is_none());
    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test state::tests::mutate_admission -v`
Expected: FAIL。方法不存在。

- [ ] **Step 3: Implement transaction path**

- `AppState` 增加 `conn: SharedConn`，`AppState::new` 保留 `conn.clone()` 给 `quality_store`。
- `ConfigStore::load_tx` 复制现有 `load` 的查询与 `normalize`；`save_tx` 复制 `save` 的 normalize、`ensure_secret`、validate、revision 自增与 `INSERT ... ON CONFLICT`，但使用传入事务。
- `mutate_admission`：拿 `write_lock`，再拿 `SharedConn` 锁，开事务；`load_tx`、闭包、`save_tx`、`upsert_many_tx`、`meta_set_tx`；提交后把 `save_tx` 返回的 revision 写入 `config.revision`，再 `replace(config)`。
- 闭包在事务内执行。闭包返回的结果与实际成员变更一致。闭包失败、校验失败或结果写入失败都回滚。
- 调用方负责先拿 `admission_lock`。`mutate_admission` 不重复获取，避免死锁。
- 初始化 `quality_store` 与 `admission_lock`。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test state::tests::mutate_admission -v`
Expected: PASS

Run: `cargo test -v`
Expected: PASS。现有配置测试不受影响。

- [ ] **Step 5: Commit**

```bash
git add src/config/store.rs src/state.rs
git commit -m "feat(quality): 增加配置与结果原子提交"
```

---

### Task 4: 候选规则与计划分组覆盖

**Files:**
- Modify: `src/subscription.rs:30-60`（`PlannedGroup` 增加 `quality`，派生改为 `PartialEq`）
- Modify: `src/subscription.rs:286-380`（抽出 `find_target_group`）
- Create: `src/quality/admission.rs`
- Modify: `src/quality/mod.rs`
- Test: `src/subscription.rs`、`src/quality/admission.rs`

**Interfaces:**
- Produces:
  - `PlannedGroup.quality: Option<QualityOverride>`
  - `pub(crate) fn find_target_group(groups: &[Group], source: &PlaylistSource, planned: &PlannedGroup) -> Option<usize>`
  - `pub struct Candidate { key, scope, source_id, source_key, gid, cid, name, target_group, url, existing_url, headers, proxy, rules, url_hash, is_url_change, offline_on_fail, interval_secs, previous_first_fail_at }`
  - `pub enum CandidateScope { Sync, Manual }`
  - `pub fn candidate_rules(config: &Config, source: &PlaylistSource, existing: Option<(&Group, &Channel)>, target: Option<&Group>) -> EffectiveRules`
  - `pub fn rule_hash(rules: &EffectiveRules) -> String`
  - `pub fn url_hash_hex(url: &str) -> String`
  - `pub fn source_layer(config: &Config, group: &Group, channel: &Channel) -> &'static str`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn candidate_rules_prefers_existing_channel_override() {
    let mut config = Config::default();
    config.quality.enabled = true;
    config.quality.rules.min_speed_mib = 1.0;
    let source = PlaylistSource { id: "s1".into(), quality: Some(QualityOverride { min_speed_mib: Some(2.0), ..Default::default() }), ..Default::default() };
    let group = Group { id: "g".into(), quality: Some(QualityOverride { min_speed_mib: Some(3.0), ..Default::default() }), ..Default::default() };
    let channel = Channel { id: "c".into(), source_id: "s1".into(), quality: Some(QualityOverride { min_speed_mib: Some(4.0), ..Default::default() }), ..Default::default() };
    let rules = candidate_rules(&config, &source, Some((&group, &channel)), None);
    assert_eq!(rules.min_speed_mib, 4.0);
}

#[test]
fn candidate_rules_uses_target_group_for_new_channel() {
    let mut config = Config::default();
    config.quality.enabled = true;
    let source = PlaylistSource { id: "s1".into(), ..Default::default() };
    let target = Group { quality: Some(QualityOverride { min_speed_mib: Some(3.0), ..Default::default() }), ..Default::default() };
    let rules = candidate_rules(&config, &source, None, Some(&target));
    assert_eq!(rules.min_speed_mib, 3.0);
}

#[test]
fn rule_hash_changes_with_threshold() {
    let mut rules = EffectiveRules { enabled: true, min_speed_mib: 1.0, ..EffectiveRules::default() };
    let first = rule_hash(&rules);
    rules.min_speed_mib = 2.0;
    assert_ne!(first, rule_hash(&rules));
}

#[test]
fn source_layer_prefers_channel_then_group_then_source() {
    let mut config = Config::default();
    let source = PlaylistSource { id: "s1".into(), quality: Some(QualityOverride { min_speed_mib: Some(1.0), ..Default::default() }), ..Default::default() };
    config.playlist.sources.push(source);
    let group = Group { quality: Some(QualityOverride { min_speed_mib: Some(2.0), ..Default::default() }), ..Default::default() };
    let channel = Channel { source_id: "s1".into(), quality: Some(QualityOverride { min_speed_mib: Some(3.0), ..Default::default() }), ..Default::default() };
    assert_eq!(source_layer(&config, &group, &channel), "channel");
    let no_channel = Channel { source_id: "s1".into(), ..Default::default() };
    assert_eq!(source_layer(&config, &group, &no_channel), "group");
    let no_group = Group::default();
    assert_eq!(source_layer(&config, &no_group, &no_channel), "source");
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test candidate_rules rule_hash_changes -v`
Expected: FAIL。函数与字段不存在。

- [ ] **Step 3: Implement helper and refactor apply**

- `PlannedGroup` 增加 `quality: Option<QualityOverride>`。`QualityOverride` 没有 `Eq`，所以 `PlannedGroup` 的派生从 `PartialEq, Eq` 改为 `PartialEq`。
- 把 `apply` 中“按 `merge_id` / 同步标记 / source_group 查找现有组”的逻辑抽成 `find_target_group`，返回索引。`apply` 与 `plan` 共用，替换现有内联逻辑，保持现有同步测试通过。
- `plan()` 构建完 `groups` 后，对每个计划分组调用 `find_target_group`，把现有分组的 `quality` 复制到 `PlannedGroup.quality`。新分组为 `None`。
- `candidate_rules`：有现有频道时构造该频道与分组调用 `effective_rules`；否则构造临时频道（`source_id`、无频道覆盖）与目标分组（或空分组）调用。
- `rule_hash`：按固定字段顺序与版本前缀做 SHA-256。只哈希新引擎读取的字段，不含旧 `action`、`retain`。
- `url_hash_hex`：去掉 `$描述`，对 URL 做 SHA-256，返回十六进制。
- `source_layer`：频道覆盖有任一字段时返回 `channel`，否则分组覆盖有任一字段时 `group`，否则源覆盖有任一字段时 `source`，否则 `global`。
- 给 `EffectiveRules`、`Action`、`Measurement` 增加 `Default`，供测试与候选构造使用。前两者默认与 `QualityRules::default` 一致；`Measurement` 默认全部为 `None` / `false`。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test candidate_rules rule_hash_changes subscription::tests -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/subscription.rs src/quality/admission.rs src/quality/mod.rs
git commit -m "feat(quality): 增加候选规则与目标分组覆盖"
```

---

### Task 5: 门控纯决策与计划过滤

**Files:**
- Create: `src/quality/gate.rs`
- Modify: `src/subscription.rs`（`Plan::retain_keys`）
- Modify: `src/quality/mod.rs`
- Test: `src/quality/gate.rs`、`src/subscription.rs`

**Interfaces:**
- Produces:
  - `pub struct Guard { enabled: bool, max_remove_ratio: f64, min_remove_count: usize }`，`Default` 为关闭。
  - `pub struct GateTargets { url_changed: Vec<Candidate>, new: Vec<Candidate>, new_all: Vec<String>, kept: Vec<Candidate>, cursor_after: Option<String>, truncated: bool, remaining: usize }`，派生 `Default`。`new_all` 是本轮全部新候选键，`new` 只含被游标选中的部分；`kept` 是规则关闭、无需探测的候选。
  - `pub struct GateStats { tested, admitted, rejected, removed, source_removed }`，派生 `Default`。
  - `pub struct CandidateOutcome { candidate, measurement, verdict, bytes, duration_ms }`
  - `pub enum GateOutcome { Apply { plan, results, stats, cursor, truncated, remaining }, Paused { reason: String, results, stats } }`
  - `pub fn select_targets(config: &Config, source: &PlaylistSource, plan: &Plan, cursor: Option<&str>, max_new: usize) -> GateTargets`
  - `pub fn decide(plan: Plan, targets: &GateTargets, outcomes: &[CandidateOutcome], guard: &Guard) -> GateOutcome`
  - `Plan::retain_keys(&self, keep: &HashSet<String>) -> Plan`

- [ ] **Step 1: Write failing decision tests**

```rust
fn candidate(key: &str, url_change: bool) -> Candidate {
    Candidate {
        key: key.into(),
        scope: CandidateScope::Sync,
        source_id: "s1".into(),
        source_key: key.into(),
        gid: if url_change { "g".into() } else { String::new() },
        cid: if url_change { key.into() } else { String::new() },
        name: key.into(),
        target_group: "体育".into(),
        url: "http://up/new.m3u8".into(),
        existing_url: url_change.then(|| "http://up/old.m3u8".into()),
        headers: Vec::new(),
        proxy: None,
        rules: EffectiveRules::default(),
        url_hash: url_hash_hex("http://up/new.m3u8"),
        is_url_change: url_change,
        offline_on_fail: false,
        interval_secs: 6 * 3600,
        previous_first_fail_at: None,
    }
}

fn outcome(key: &str, ok: bool, reason: Reason, url_change: bool) -> CandidateOutcome {
    CandidateOutcome {
        candidate: candidate(key, url_change),
        measurement: Measurement {
            reachable: !matches!(reason, Reason::Unreachable | Reason::ProbeError),
            probe_error: reason == Reason::ProbeError,
            http_status: None,
            delay_ms: None,
            speed_mib: None,
            width: None,
            height: None,
            error: None,
        },
        verdict: Verdict { ok, reachable: true, reason },
        bytes: 0,
        duration_ms: 0,
    }
}

#[test]
fn new_failed_candidate_is_rejected_and_plan_keeps_passing() {
    let targets = GateTargets { new: vec![candidate("new1", false), candidate("new2", false)], ..Default::default() };
    let plan = plan_with(&["new1", "new2"]);
    let out = decide(
        plan,
        &targets,
        &[outcome("new1", true, Reason::Pass, false), outcome("new2", false, Reason::SpeedLow, false)],
        &Guard { enabled: true, max_remove_ratio: 0.5, min_remove_count: 10 },
    );
    match out {
        GateOutcome::Apply { plan, stats, .. } => {
            assert_eq!(plan.channels.len(), 1);
            assert_eq!(plan.channels[0].key, "new1");
            assert_eq!(stats.rejected, 1);
        }
        _ => panic!("应应用"),
    }
}

#[test]
fn url_changed_probe_error_pauses_sync() {
    let targets = GateTargets { url_changed: vec![candidate("c1", true)], ..Default::default() };
    let out = decide(plan_with(&["c1"]), &targets, &[outcome("c1", false, Reason::ProbeError, true)], &Guard::default());
    assert!(matches!(out, GateOutcome::Paused { .. }));
}

#[test]
fn truncation_keeps_admitted_and_advances_cursor() {
    let config = config_with_existing(&["new:1"]);
    let plan = plan_with(&["new:1", "new:2", "new:3", "new:4"]);
    let targets = select_targets(&config, &source(), &plan, Some("new:1"), 2);
    assert_eq!(targets.new.iter().map(|item| item.key.as_str()).collect::<Vec<_>>(), vec!["new:2", "new:3"]);
    assert!(targets.truncated);
    assert_eq!(targets.cursor_after.as_deref(), Some("new:3"));
    assert_eq!(targets.remaining, 1);
    assert!(!targets.new.iter().any(|item| item.key == "new:1"), "已准入且未变化的频道不参与门控");
}

#[test]
fn unprocessed_new_candidates_are_not_admitted() {
    let targets = GateTargets { new: vec![candidate("new1", false)], truncated: true, ..Default::default() };
    let plan = plan_with(&["new1", "new2"]);
    let out = decide(plan, &targets, &[outcome("new1", true, Reason::Pass, false)], &Guard::default());
    match out {
        GateOutcome::Apply { plan, .. } => {
            assert_eq!(plan.channels.len(), 1);
            assert_eq!(plan.channels[0].key, "new1", "未处理的新候选不得准入");
        }
        _ => panic!("应应用"),
    }
}

#[test]
fn guard_disabled_removes_failed_url_change() {
    let targets = GateTargets { url_changed: vec![candidate("c1", true)], ..Default::default() };
    let out = decide(plan_with(&["c1"]), &targets, &[outcome("c1", false, Reason::Unreachable, true)], &Guard::default());
    match out {
        GateOutcome::Apply { plan, stats, .. } => {
            assert!(plan.channels.is_empty());
            assert_eq!(stats.removed, 1);
        }
        _ => panic!("应应用"),
    }
}

#[test]
fn guard_trips_at_exact_threshold() {
    let targets = GateTargets { url_changed: (0..10).map(|i| candidate(&format!("c{i}"), true)).collect(), ..Default::default() };
    let outcomes: Vec<_> = (0..10).map(|i| outcome(&format!("c{i}"), false, Reason::Unreachable, true)).collect();
    let guard = Guard { enabled: true, max_remove_ratio: 0.5, min_remove_count: 10 };
    assert!(matches!(decide(plan_with(&[]), &targets, &outcomes, &guard), GateOutcome::Paused { .. }));
}

#[test]
fn guard_does_not_trip_below_min_count() {
    let targets = GateTargets { url_changed: (0..9).map(|i| candidate(&format!("c{i}"), true)).collect(), ..Default::default() };
    let outcomes: Vec<_> = (0..9).map(|i| outcome(&format!("c{i}"), false, Reason::Unreachable, true)).collect();
    let guard = Guard { enabled: true, max_remove_ratio: 0.5, min_remove_count: 10 };
    assert!(matches!(decide(plan_with(&[]), &targets, &outcomes, &guard), GateOutcome::Apply { .. }));
}
```

测试辅助函数 `plan_with(keys)` 生成 `Plan`（每个键一个频道与一个分组），`config_with_existing(keys)` 生成含已准入频道的 `Config`（频道 URL 与计划一致，避免误判为 URL 变化），`source()` 返回 `PlaylistSource { id: "s1", mode: "sync", .. }`。

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test quality::gate -v`
Expected: FAIL。模块与类型不存在。

- [ ] **Step 3: Implement gate decision**

- `select_targets`：按 `source_id + source_key` 找现有频道。计划 URL 哈希不同归入 `url_changed`；找不到归入 `new`；其余忽略。规则 `enabled=false` 的候选归入 `kept`，不探测。`candidate.interval_secs = config.quality.interval_hours * 3600`。`new` 按稳定键排序，从 `cursor` 之后切片到 `max_new`（0 表示不限）；返回 `cursor_after`、`truncated`、`remaining`。游标不存在时从开头开始。
- `decide`：
  - URL 变化结果中有 `ProbeError` → `Paused { reason: "probe_error" }`。
  - `tested = outcomes.len()`；`removals` 只计 URL 变化且失败且不是 `probe_error`。
  - 安全阀条件：`enabled && removals >= min_remove_count && tested > 0 && removals / tested >= max_remove_ratio`。
  - 触发：`Paused { reason: "guard" }`。结果动作：合格 `held`，失败 `removal_paused`。
  - 未触发或安全阀关闭：URL 变化失败从计划移除，结果动作 `removed`。新候选不合格 `rejected`，合格 `admitted`。
  - 构造保留集合时从 `plan.channels` 的全部键开始，再删除以下键：
    - 未出现在 `targets.new` 中的新候选（被截断或尚未处理），但 `targets.kept` 中的候选除外；
    - `targets.new` 中判定失败的新候选；
    - 需要移出的 URL 变化频道。
  - 未变化的已准入频道与 `targets.kept` 中的候选必须保留。未处理的新候选不得进入计划。
  - `Apply` 返回过滤后的计划、结果、统计、游标。
- `Plan::retain_keys`：保留指定键；按原 `groups` 顺序重建，`count` 为保留频道数；删除 0 频道的组；保留 `trace` 与 `duplicates`。
- 结果记录使用 `QualityResult`：同步键 `sync:<source_id>:<source_key>`；新候选 `gid`/`cid` 为空；`checked_at` 用当前时间；`rule_hash = rule_hash(&candidate.rules)`；失败时 `first_fail_at = candidate.previous_first_fail_at.or(Some(now))`，合格时为 `None`。准入结果 `next_check_at = now + candidate.interval_secs`；拒绝结果 `next_check_at=0`（不自动复检）。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test quality::gate subscription::tests::retain_keys -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/quality/gate.rs src/quality/mod.rs src/subscription.rs
git commit -m "feat(quality): 增加同步门控决策与计划过滤"
```

---

### Task 6: 门控探测编排与同步接入

**Files:**
- Modify: `src/quality/gate.rs`（`filter_plan`、`RunMemo`、`gate_expired`）
- Modify: `src/subscription.rs:759-895`（`sync_locked`）
- Modify: `src/subs.rs:109-140`（`SourceStatus` 门控字段）
- Modify: `src/state.rs`（`probe_client` 改为 `pub(crate)`）
- Modify: `tests/http_contract.rs`（伪上游增加 `/sync.m3u8`，增加门控集成测试）
- Test: `src/quality/gate.rs`、`tests/http_contract.rs`

**Interfaces:**
- Produces:
  - `pub async fn filter_plan(state: &Arc<AppState>, config: &Config, source: &PlaylistSource, plan: Plan) -> GateOutcome`
  - `pub struct RunMemo { .. }`，`get(&self, url_hash: u64) -> Option<&Measurement>`、`get_host(&self, host: &str) -> Option<&Measurement>`、`insert(&mut self, url_hash: u64, host: Option<&str>, measurement: Measurement, host_cache: bool)`
  - `pub fn gate_expired(started: std::time::Instant, budget_secs: u64) -> bool`
  - `SourceStatus { gate_phase, gate_tested, gate_admitted, gate_rejected, gate_removed, gate_paused, gate_truncated, gate_cursor, gate_remaining, gate_last_at }`
  - `SyncReport` 增加 `gate_paused: bool`、`gate_admitted: usize`、`gate_rejected: usize`、`gate_removed: usize`、`gate_truncated: bool`

- [ ] **Step 1: Write failing unit and integration tests**

```rust
#[test]
fn gate_budget_zero_never_expires() {
    assert!(!gate_expired(std::time::Instant::now() - std::time::Duration::from_secs(999), 0));
}
#[test]
fn run_memo_dedupes_url_and_host() {
    let mut memo = RunMemo::default();
    let measurement = Measurement { reachable: true, ..Measurement::default() };
    assert!(memo.get(1).is_none());
    memo.insert(1, None, measurement.clone(), false);
    assert_eq!(memo.get(1), Some(&measurement));
    memo.insert(2, Some("example.com"), measurement.clone(), true);
    assert_eq!(memo.get(2), Some(&measurement));
}
```

集成测试在 `tests/http_contract.rs`：

- 给伪上游增加 `/sync.m3u8` 路由，按请求 `Host` 拼绝对地址，返回两条频道：`/live.m3u8`（正常）与 `/missing.m3u8`（404）。
- 用 `fixture_with_extra` 增加：

```json
{
  "quality": { "enabled": true, "probe_mode": "playlist", "max_channels_per_run": 10 },
  "playlist": { "sources": [{ "id": "gate", "name": "门控源", "mode": "sync", "enabled": true, "url": "{upstream}/sync.m3u8" }] }
}
```

- 调用 `subscription::sync_source(&fixture.state, "gate", false).await`。
- 断言：配置中该源分组只有「正常频道」；`quality_store` 有一条 `rejected`，原因为 `unreachable`。
- 第二次同步，断言结果没有新增对「正常频道」的探测记录（`checked_at` 不变），证明未变化频道不重复探测。
- 再构造一个已准入频道从清单消失的场景，断言结果动作为 `source_removed`。

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --test http_contract sync_gate_admits_only_passing_channels -v`
Expected: FAIL。门控未接入。

- [ ] **Step 3: Implement probing and sync integration**

- `RunMemo`：键为 `url_hash`；`host_cache` 时另存 host 键。只保存 `Measurement`，不保存判定。生命周期为一次任务。
- `gate_expired`：`budget_secs > 0` 且已用时间达到或超过预算。
- `filter_plan`：
  - 全局或源级 `quality.enabled=false` 时直接返回原计划，统计为 0。
  - 调 `select_targets`。`max_new` 用 `run_limit(budget_mb_per_run, 8.0, max_channels_per_run)`。`kept` 中的候选不探测，直接保留。
  - 先顺序探测 URL 变化频道。探测前从 `quality_store` 填 `previous_first_fail_at`；检查预算；`probe_error` 或超时 → `Paused`，不应用任何成员变更。
  - 再按 `concurrency` 分块探测新候选。每块前检查预算与取消标志。超时后保留已测结果，未测候选截断，`cursor` 指向最后一个已处理键。
  - 使用计划中的频道 headers（`Value` 字符串或 `{name,value}` 对象解析为 `(String, String)`）与候选代理。候选代理优先级：现有频道/目标分组代理，其次源代理。给 `AppState` 增加 `pub(crate) fn probe_client_for_candidate(&self, proxy: Option<&ProxyCfg>) -> reqwest::Client`，内部调用现有私有 `probe_client`。
- 调 `decide`。`Apply` 时额外生成 `source_removed` 结果：在 `apply` 前记录现有源频道的 `canonical_key(source_key)`，`apply` 后对比剩余集合；消失的频道写 `source_removed`。按稳定键对比，避免频道移动被误判为删除。`Paused` 时不生成。
- `sync_locked`：
  - 在 `plan` 后获取 `admission_lock`，再调 `filter_plan`。
  - `Paused`：用 `quality_store.upsert_many` 写结果；更新源状态 `gate_phase="paused"`、`gate_paused=true`、`gate_last_at`；返回 `Ok(SyncReport { gate_paused: true, .. })`，不应用配置。
  - `Apply`：调 `mutate_admission`。闭包内检查源仍存在且 `current == expected`，再 `apply` 过滤后的计划；闭包返回门控结果与 `source_removed` 结果。提交后更新源状态与游标。
  - 门控关闭时照常 `apply`，不清空游标以外的现有行为。
  - 源状态阶段：`fetching|parsing|gating|applying|done|paused`。
- `SourceStatus` 新字段使用 `#[serde(default)]` 兼容旧状态文件。
- `SyncReport` 新字段用于 API 与日志。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --test http_contract sync_gate_admits_only_passing_channels -v`
Expected: PASS

Run: `cargo test quality::gate -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/quality/gate.rs src/subscription.rs src/subs.rs src/state.rs tests/http_contract.rs
git commit -m "feat(quality): 接入同步门控与单轮缓存"
```

---

### Task 7: 周期与手动准入引擎

**Files:**
- Create: `src/quality/engine.rs`
- Modify: `src/quality/mod.rs`
- Modify: `src/quality/state.rs`（`RunState` 字段与 `try_start_run` 签名）
- Modify: `src/quality/rules.rs`（`Reason::parse`）
- Modify: `tests/http_contract.rs`（引擎集成测试）
- Test: `src/quality/engine.rs`、`src/quality/state.rs`、`tests/http_contract.rs`

**Interfaces:**
- Produces:
  - `pub enum RunTrigger { Schedule, Manual }`
  - `pub struct RunSpec { run_id, trigger, scope, measure_only }`
  - `pub struct Removal { gid, cid, url_hash }`
  - `pub struct RunSummary { run_id, tested, admitted, removed, rejected, skipped, hidden, paused_by_guard }`
  - `pub struct EngineDecision { removals: Vec<Removal>, manual_entries: Vec<(String, String, Entry)>, results: Vec<QualityResult>, tested, removed, hidden, kept, paused }`
  - `pub fn due_targets(config, results: &QualityStore, scope, now, limit, measure_only) -> anyhow::Result<Vec<Candidate>>`
  - `pub fn removal_allowed(config: &Config, removal: &Removal) -> bool`
  - `pub fn decide(outcomes: &[CandidateOutcome], measure_only: bool, guard: &Guard) -> EngineDecision`
  - `pub async fn run(state: &Arc<AppState>, spec: RunSpec) -> anyhow::Result<RunSummary>`
  - `RunState` 增加 `trigger: String`、`phase: String`、`admitted: usize`、`removed: usize`、`rejected: usize`、`skipped: usize`、`paused_by_guard: bool`、`measure_only: bool`；`try_start_run(&self, run_id, trigger, total, measure_only) -> bool`

- [ ] **Step 1: Write failing engine tests**

```rust
#[test]
fn guard_pauses_sync_removals() {
    let outcomes: Vec<_> = (0..10).map(|i| outcome_sync(&format!("c{i}"), false, Reason::Unreachable)).collect();
    let guard = Guard { enabled: true, max_remove_ratio: 0.5, min_remove_count: 10 };
    let decision = decide(&outcomes, false, &guard);
    assert!(decision.paused);
    assert!(decision.removals.is_empty());
    assert_eq!(decision.results.iter().filter(|item| item.action == "removal_paused").count(), 10);
}

#[test]
fn measure_only_never_removes_or_hides() {
    let outcomes = vec![outcome_manual("c1", false, Reason::Unreachable, true)];
    let decision = decide(&outcomes, true, &Guard::default());
    assert!(decision.removals.is_empty());
    assert!(decision.manual_entries.is_empty());
    assert_eq!(decision.results[0].action, "kept");
}

#[test]
fn manual_hide_requires_offline_on_fail() {
    let decision = decide(&[outcome_manual("c1", false, Reason::Unreachable, true)], false, &Guard::default());
    assert_eq!(decision.results[0].action, "hidden");
    assert_eq!(decision.manual_entries.len(), 1);
}

#[test]
fn removal_revalidation_skips_changed_source_or_url() {
    let mut config = config_with_sync_channel();
    let removal = Removal { gid: "g".into(), cid: "c".into(), url_hash: url_hash_hex("http://up/live.m3u8") };
    assert!(removal_allowed(&config, &removal));
    config.groups[0].channels[0].url = "http://up/changed.m3u8".into();
    assert!(!removal_allowed(&config, &removal), "URL 变化时不得按旧结果移出");
    config.groups[0].channels[0].url = "http://up/live.m3u8".into();
    config.playlist.sources[0].enabled = false;
    assert!(!removal_allowed(&config, &removal), "源停用后不得移出");
}

#[test]
fn manual_probe_error_does_not_hide() {
    let decision = decide(&[outcome_manual("c1", false, Reason::ProbeError, true)], false, &Guard::default());
    assert_eq!(decision.results[0].action, "kept");
    assert!(decision.manual_entries.is_empty(), "探测错误不改手工状态");
}

#[test]
fn manual_success_clears_hidden_state() {
    let decision = decide(&[outcome_manual("c1", true, Reason::Pass, true)], false, &Guard::default());
    assert_eq!(decision.results[0].action, "kept");
    assert!(decision.manual_entries[0].2.ok());
}
```

`outcome_sync` / `outcome_manual` 构造 `CandidateOutcome`，`Candidate.scope` 分别为 `Sync` / `Manual`，`offline_on_fail` 由参数给出。`Measurement` 增加 `Default`。

集成测试在 `tests/http_contract.rs`：

- `admitted_failure_is_removed_by_engine`：sync 源频道指向 `/missing.m3u8`，插入一条 `admitted` 结果，调用 `engine::run`；断言频道被移出，结果动作 `removed`。
- `probe_error_is_not_removed_by_engine`：频道指向 `/manifest.mpd`（探测返回 `probe_error`），断言频道保留，结果动作 `skipped`。
- `disabled_quality_run_is_measure_only`：全局 `quality.enabled=false`，调用 `run_now`；断言频道数量不变，运行状态 `measure_only=true`。

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test quality::engine -v`
Expected: FAIL。模块不存在。

- [ ] **Step 3: Implement engine**

- `due_targets`：从 `quality_results` 取 `next_check_at <= now` 的记录，按到期时间排序；再补齐没有结果的目标，直到 `limit`。用结果填写 `previous_first_fail_at` 与 `interval_secs`。跳过归档组、源停用、源门控关闭、空 URL。`measure_only` 时不跳过全局关闭，但仍跳过源停用与归档。保留 `scope` 过滤。
- `decide`：
  - 手工频道合格：`kept`，写 `Entry`（清除下线）；失败且 `offline_on_fail=true`：`hidden`，写失败 `Entry`；失败且关闭：`kept`，写失败 `Entry`；`probe_error`：`kept`，不写 `Entry`，保持原状态。
  - 同步频道合格：`admitted`；失败：移出候选；`probe_error`：`skipped`，不移出。
  - `measure_only`：不改成员、不改手工状态；同步结果 `kept`，手工结果 `kept`。
  - 安全阀只统计同步移出。触发：不移出；同步结果 `removal_paused`/`held`。关闭或不触发：同步失败照常移出。手工结果仍按规则写 `hidden`/`kept`。
  - `next_check_at` 用 `interval_hours * 3600`；`first_fail_at` 从上一结果或 `Entry` 继承。
- `run`：
  - 拿 `admission_lock`；拿到后重新读取配置。`measure_only = spec.measure_only || !config.quality.enabled`。
  - 分块探测，复用 `RunMemo` 做单轮 URL 去重；支持取消与预算。
  - 调 `decide`。
  - `removal_allowed`：频道仍存在、`source_id` 匹配、URL 哈希等于记录值、源仍启用、源门控仍开启。`run` 的闭包用它复核；不满足的结果改 `skipped`、原因 `source_changed`。
  - 有同步移出时用 `mutate_admission`；无同步移出（`measure_only`、安全阀触发、仅手工）时用 `quality_store.upsert_many` 与 `meta_set` 写结果，不修改配置 revision。
  - 提交后更新 `QualityState` 手工条目；同步条目也可写入，但订阅渲染只读手工。
  - 写 `quality_meta`：`last_run_at`、`last_run_id`、`last_run_summary`、`next_run_at`、`guard_tripped_at`。
  - 更新 `RunState` 各阶段与统计；结束用 `finish_run_id`，避免覆盖新任务。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test quality::engine -v`
Expected: PASS

Run: `cargo test --test http_contract admitted_failure_is_removed_by_engine probe_error_is_not_removed_by_engine disabled_quality_run_is_measure_only -v`
Expected: PASS

Run: `cargo test quality::state -v`
Expected: PASS（更新 `try_start_run` 的现有测试）。

- [ ] **Step 5: Commit**

```bash
git add src/quality/engine.rs src/quality/mod.rs src/quality/state.rs src/quality/rules.rs
git commit -m "feat(quality): 增加周期与手动准入引擎"
```

---

### Task 8: cron/小时调度与运行状态

**Files:**
- Modify: `src/quality/schedule.rs`（重写调度）
- Modify: `src/main.rs:78`（启动入口保持）
- Test: `src/quality/schedule.rs`

**Interfaces:**
- Produces:
  - `pub fn next_run_at(config: &Config, last_run: i64, now: i64) -> Option<i64>`
  - `pub fn spawn(state: Arc<AppState>)`
  - `pub async fn run_now(state, scope, mode) -> Result<(String, usize), RunError>`
  - `pub fn measure_only_for(config: &Config) -> bool`
  - `pub fn cleanup_quality_results(state: &AppState) -> anyhow::Result<usize>`

- [ ] **Step 1: Write failing schedule tests**

```rust
#[test]
fn next_run_interval_and_cron() {
    let mut config = Config::default();
    config.quality.enabled = true;
    config.quality.schedule = "interval".into();
    config.quality.interval_hours = 6;
    assert_eq!(next_run_at(&config, 100, 200), Some(100 + 6 * 3600));

    config.quality.schedule = "cron".into();
    config.quality.cron = "0 * * * *".into();
    let next = next_run_at(&config, 0, 1_700_000_000).expect("next");
    assert!(next > 1_700_000_000);
}

#[test]
fn disabled_config_means_measure_only() {
    let mut config = Config::default();
    config.quality.enabled = false;
    assert!(measure_only_for(&config));
    config.quality.enabled = true;
    assert!(!measure_only_for(&config));
}

#[test]
fn run_lock_allows_one_task() {
    let state = QualityState::new();
    assert!(state.try_start_run("r1", "manual", 1, false));
    assert!(!state.try_start_run("r2", "manual", 1, false));
    state.finish_run_id("r1");
    assert!(state.try_start_run("r3", "manual", 1, false));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test quality::schedule -v`
Expected: FAIL。`next_run_at` 不存在或行为不符。

- [ ] **Step 3: Implement scheduler**

- `next_run_at`：`cron` 用 `cron_preview`；`interval` 用 `last_run + interval_hours*3600`；关闭或非法返回 `None`。
- `spawn`：启动后等待 60 秒，每 30 秒检查。到期且没有运行任务时 spawn `engine::run`；`engine::run` 自己获取 `admission_lock`。运行中跳过，不排队。任务结束写 `last_run_at` 与 `next_run_at`。启动时清理一次结果，之后每 24 小时清理。
- `run_now`：scope 解析、`measure_only = measure_only_for(&config)`、`try_start_run`、spawn `engine::run`、返回 `(run_id, queued)`。全局关闭不再返回 `Disabled`。
- `cleanup_quality_results`：`result_retention_days=0` 跳过；否则调 `quality_store.cleanup`。
- 任务结束写 `last_run_summary`；`guard_tripped_at` 在安全阀触发时写。
- `RunError` 复用现有枚举；`Disabled` 保留但不再由 `run_now` 返回。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test quality::schedule -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/quality/schedule.rs src/main.rs
git commit -m "feat(quality): 增加 cron 与小时调度"
```

---

### Task 9: 源停用与删除

**Files:**
- Modify: `src/http/routes/playlist.rs:155-320`
- Modify: `src/http.rs`（路由）
- Modify: `src/subscription.rs`（复用删除/断开辅助）
- Test: `tests/http_contract.rs` 或 `src/http/routes/playlist.rs`

**Interfaces:**
- Produces:
  - `POST /panel/api/playlist/sources/:id/disable` body `{ "keep_channels": bool }`
  - `POST /panel/api/playlist/sources/:id/enable`
  - 停用/删除时清空 `gate_cursor`、`gate_remaining`、`gate_phase`

- [ ] **Step 1: Write failing handler tests**

直接调用处理函数（与现有 `delete_channel` 测试同风格）：

```rust
#[tokio::test]
async fn disable_source_keep_channels_leaves_config_unchanged() {
    let fixture = fixture_with_source().await;
    let before = fixture.state.snapshot().group("pg").unwrap().channels.len();
    let response = tv_buddy::http::routes::playlist::disable_source(
        tv_buddy::http::ext::PanelAuth(String::new()),
        State(fixture.state.clone()),
        Path("src-a".into()),
        Json(json!({ "keep_channels": true })),
    )
    .await
    .expect("停用");
    let _ = response;
    assert_eq!(fixture.state.snapshot().group("pg").unwrap().channels.len(), before);
    assert!(!fixture.state.snapshot().playlist.sources.iter().find(|item| item.id == "src-a").unwrap().enabled);
}

#[tokio::test]
async fn disable_source_remove_channels_writes_removed_results() {
    let fixture = fixture_with_source().await;
    let _ = tv_buddy::http::routes::playlist::disable_source(
        tv_buddy::http::ext::PanelAuth(String::new()),
        State(fixture.state.clone()),
        Path("src-a".into()),
        Json(json!({ "keep_channels": false })),
    )
    .await
    .expect("停用并移出");
    assert!(fixture.state.snapshot().group("pg").unwrap().channels.is_empty());
    let (_, items) = fixture.state.quality_store.list(&QualityFilter::default()).unwrap();
    assert!(items.iter().any(|item| item.action == "removed"));
}
```

`fixture_with_source` 复用 `fixture_with_extra`：在 `playlist.sources` 增加 `src-a`，并把 `pg` 频道的 `source_id` 设为 `src-a`、`source_key` 设为 `name:C1`。`pg` 是手工分组，移出频道后分组本身保留。删除测试覆盖 `keep_content=1` 转手工与 `0` 删除。

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test disable_source -v`
Expected: FAIL。处理函数不存在。

- [ ] **Step 3: Implement source actions**

- 增加两个处理函数；操作在 `state.mutate_admission` 内完成，源不存在返回 404。`keep_channels` 缺失或不是布尔值返回 400。
- 停用保留：`enabled=false`，频道不动，清空门控进度，结果不变。
- 停用移出：删除该源频道与空同步分组；结果动作 `removed`，原因 `source_disabled`。
- 启用：`enabled=true`；若为 sync，`tokio::spawn(subscription::sync_source(state.clone(), id, false))`，返回 `{ ok, queued: true }`。
- 删除沿用现有 `keep_content`：
  - `keep_content=1`：`detach_source` 后，把每个频道的同步结果改为手工结果：写入 `channel:<gid>/<cid>`，删除旧 `sync:` 键。
  - `keep_content=0`：`remove_source_content` 后，把旧结果动作改为 `source_removed`。
- 两种情况都清空门控状态与正文缓存。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test disable_source -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/http/routes/playlist.rs src/http.rs src/subscription.rs tests/http_contract.rs
git commit -m "feat(quality): 增加源停用与启用选项"
```

---

### Task 10: 质量 HTTP 接口

**Files:**
- Rewrite: `src/http/routes/quality.rs`
- Modify: `src/http.rs`（质量路由）
- Test: `src/http/routes/quality.rs`、`tests/http_contract.rs`

**Interfaces:**
- Produces:
  - `GET /panel/api/quality/status`
  - `GET /panel/api/quality/results`
  - `GET /panel/api/quality/sources`
  - `GET /panel/api/quality/effective`
  - `POST /panel/api/quality/run`
  - `GET /panel/api/quality/runs/:run_id`
  - `POST /panel/api/quality/cancel`
  - `POST /panel/api/quality/preview`
  - `POST /panel/api/quality/schedule/preview`

- [ ] **Step 1: Write failing request tests**

```rust
#[test]
fn run_request_parses_scope_and_mode() {
    let body = serde_json::json!({ "scope": "channel", "id": "g1/c1", "run_mode": "quick" });
    let request = RunRequest::from_json(&body).expect("parse");
    assert_eq!(request.scope, RunScope::Channel("g1".into(), "c1".into()));
    assert_eq!(request.mode, RunMode::Quick);
}

#[test]
fn results_query_parses_filters_and_default_limit() {
    let query = ResultsQuery::from_pairs(&[("source_id", "s1"), ("action", "removed"), ("limit", "50"), ("offset", "100")]);
    assert_eq!(query.filter.source_id.as_deref(), Some("s1"));
    assert_eq!(query.filter.action.as_deref(), Some("removed"));
    assert_eq!(query.filter.limit, 50);
    assert_eq!(query.filter.offset, 100);

    let empty = ResultsQuery::from_pairs(&[]);
    assert_eq!(empty.filter.limit, 50);
}

#[test]
fn schedule_preview_rejects_bad_cron() {
    let body = serde_json::json!({ "mode": "cron", "cron": "bad" });
    assert!(SchedulePreviewRequest::from_json(&body).is_err());
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test http::routes::quality -v`
Expected: FAIL。新字段与接口不存在。

- [ ] **Step 3: Implement handlers**

- `RunRequest`：保留 `scope`、`mode`；删除 `force`。`measure_only` 由服务端按当前配置决定。
- `status` 返回开关、周期、下次运行、运行状态、统计、安全阀，字段按系统设计 §8.1。`counts` 用 `quality_store.counts`。
- `results` 用 `QualityStore::list`；返回 `{ total, items }`。`speed_mib = speed_kib/1024`，`resolution` 由宽高生成。
- `sources` 只列 `mode=sync`，汇总门控状态与剩余候选；聚合源不列或折叠。
- `effective` 返回有效规则与 `source_layer`；参数 `source_id`、`gid`、`cid`。
- `run`：全局关闭时仍可运行，`measure_only=true`；冲突 409 `run_active`；无目标 400 `no_targets`；响应带 `measure_only`。
- `schedule/preview`：校验 cron，返回未来 3 次；`interval` 返回未来 3 次间隔时间。
- `preview`：按最近结果列出将被移出的频道；不写状态。
- 所有 handler 使用 `PanelAuth`。地址不完整返回。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test http::routes::quality -v`
Expected: PASS

Run: `cargo test --test http_contract quality_ -v`
Expected: PASS（新增接口契约测试）。

- [ ] **Step 5: Commit**

```bash
git add src/http/routes/quality.rs src/http.rs tests/http_contract.rs
git commit -m "feat(quality): 增加准入状态与结果接口"
```

---

### Task 11: 订阅渲染、手工策略与水合

**Files:**
- Modify: `src/playlist.rs:99-145`（`quality_hidden`、`offline`）
- Modify: `src/state.rs`（启动水合）
- Modify: `src/quality/state.rs`（`Entry::from_result`）
- Modify: `src/http/routes/groups.rs`（删除频道/分组清理结果）
- Test: `src/playlist.rs`、`src/quality/state.rs`

**Interfaces:**
- Produces:
  - `pub(crate) fn manual_hidden(config, quality, group, channel) -> bool`
  - `AppState::hydrate_manual_quality() -> anyhow::Result<()>`
  - `Entry::from_result(result: &QualityResult, url_hash: u64) -> Entry`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn manual_keep_stays_visible_after_failure() {
    let mut config = config_with(vec![group("g1", "public", "proxy")]);
    config.quality.enabled = true;
    config.groups[0].offline_on_fail = false;
    let state = quality_with(Reason::Unreachable, now(), Some(now()));
    let body = render_user(&config, &admin(), "http://host", "tok", Some(&Health::new()), Some(&state));
    assert!(body.contains("C1"));
}

#[test]
fn manual_hide_requires_offline_on_fail() {
    let mut config = config_with(vec![group("g1", "public", "proxy")]);
    config.quality.enabled = true;
    config.groups[0].offline_on_fail = true;
    let state = quality_with(Reason::Unreachable, now(), Some(now()));
    let body = render_user(&config, &admin(), "http://host", "tok", Some(&Health::new()), Some(&state));
    assert!(!body.contains("C1"));
}

#[test]
fn sync_channel_is_not_hidden_by_quality_result() {
    let mut config = config_with(vec![group("g1", "public", "proxy")]);
    config.quality.enabled = true;
    config.groups[0].channels[0].source_id = "s1".into();
    let state = quality_with(Reason::Unreachable, now(), Some(now()));
    let body = render_user(&config, &admin(), "http://host", "tok", Some(&Health::new()), Some(&state));
    assert!(body.contains("C1"));
}

#[tokio::test]
async fn manual_entries_hydrate_after_restart() {
    let dir = std::env::temp_dir().join(format!("tvb-hydrate-{}", uuid::Uuid::new_v4()));
    let mut config = Config::default();
    config.groups.push(Group {
        id: "g1".into(),
        channels: vec![Channel { id: "c1".into(), url: "http://up/live.m3u8".into(), ..Channel::default() }],
        ..Group::default()
    });
    let state = std::sync::Arc::new(AppState::new(dir.join("tvb.db"), config, std::sync::Arc::new(crate::logging::RingBuffer::default())).unwrap());
    state.quality_store.upsert(&QualityResult {
        key: "channel:g1/c1".into(),
        scope: "manual".into(),
        action: "hidden".into(),
        gid: "g1".into(),
        cid: "c1".into(),
        url_hash: crate::quality::admission::url_hash_hex("http://up/live.m3u8"),
        checked_at: 1,
        ..QualityResult::default()
    }).unwrap();
    state.hydrate_manual_quality().unwrap();
    assert!(state.quality.get("g1", "c1").is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn entry_from_result_marks_hidden_failure() {
    let result = QualityResult { action: "hidden".into(), reason: "unreachable".into(), ok: false, ..QualityResult::default() };
    let entry = Entry::from_result(&result, 7);
    assert!(!entry.ok());
    assert_eq!(entry.reason, Reason::Unreachable);
    assert_eq!(entry.url_hash, 7);
}
```

`manual_entries_hydrate_after_restart` 放在 `src/state.rs` 的测试模块；`entry_from_result_marks_hidden_failure` 放在 `src/quality/state.rs`。测试辅助函数沿用 `src/playlist.rs` 现有 `config_with`、`group`、`admin`、`render_user`、`quality_with`，按新行为调整。

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test playlist::tests::manual_ quality::state::tests::from_result -v`
Expected: FAIL。行为仍按旧宽限与 action 判定。

- [ ] **Step 3: Implement manual policy and hydration**

- `quality_hidden` 改为：
  - 全局关闭 → false。
  - 透传组且未开启 `probe_passthrough` → false。
  - 同步频道（`source_id` 非空）→ false。失败已由任务移出；保留防御性 URL 哈希检查。
  - 手工频道 → `effective_rules(...).enabled` 为 true、`offline_on_fail_for(gid,cid)` 为 true、结果失败、`Entry.url_hash` 与当前 URL 一致 → true；否则 false。`rules.action`、`retain`、`stale_keep_secs` 不再参与判定。
- `offline`：`config.quality.enabled` 时返回 false，避免健康探测与质量结果同时隐藏。
- 质量关闭时保留现有健康探测下线路径。
- `hydrate_manual_quality` 在 `AppState::new` 末尾调用，读取 `scope=manual` 结果，按当前频道 URL 计算 `url_hash`，转成 `Entry` 写入 `QualityState`。频道不存在或 URL 哈希不匹配时跳过。
- `Entry::from_result`：`flags` 由 `result.ok` 决定；`reason` 用 `Reason::parse`；`first_fail_at`、`next_check_at`、延迟、速率、分辨率从结果复制。
- 删除手工频道时调用 `quality_store.delete("channel:<gid>/<cid>")`；同步频道同时删除 `sync:<source_id>:<source_key>`。删除分组时遍历频道清理两类键。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test playlist::tests -v`
Expected: PASS

Run: `cargo test -v`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/playlist.rs src/state.rs src/quality/state.rs src/quality/rules.rs src/http/routes/groups.rs
git commit -m "feat(quality): 手工频道下线策略与重启水合"
```

---

### Task 12: 前端接口、质量页骨架与规则页

**Files:**
- Modify: `frontend/src/api.ts`
- Create: `frontend/src/quality-page.tsx`
- Modify: `frontend/src/panel.tsx`
- Modify: `frontend/src/settings.tsx`
- Test: `npm --prefix frontend run build`

**Interfaces:**
- Produces:
  - `QualityResult`、`QualityStatus`、`QualitySource`、`QualityEffective` TypeScript 类型
  - `fetchQualityStatus()`、`fetchQualityResults()`、`fetchQualitySources()`、`fetchQualityEffective()`、`runQuality()`、`cancelQuality()`、`previewQualitySchedule()`
  - `QualityPage` with tabs `overview|rules|results|sources`
  - 侧栏 `quality` 分区

- [ ] **Step 1: Add API types and a failing page import**

- 先写 `QualityPage` 骨架并在 `panel.tsx` 引用，运行构建。Expected: FAIL，模块或类型不存在。
- 再补实现，使构建通过。

- [ ] **Step 2: Implement API types and page shell**

- `api.ts` 增加类型与函数，路径按系统设计 §8。
  - `runQuality` 删除 `force` 参数，响应增加 `measure_only`。
  - `QualityResult.reason` 扩展 `source_disabled`、`source_changed`、`budget`、`cancelled`、`guard`。
- `quality-page.tsx` 增加页头、四个标签、概览卡、加载/错误/空状态。
- 概览显示开关、周期、下次运行、最近任务、准入/移出/探测错误、立即校验与取消确认。
- 规则页显示运行参数、阈值、安全阀、周期、覆盖表；保存使用 `GET /config` 与 `putConfig`，带 `revision`。
- 周期页支持按小时与 cron；cron 输入调用 `previewQualitySchedule`。
- `panel.tsx` 增加 `quality` 分区、图标 `activity`、管理员可见、渲染 `QualityPage`。
- `settings.tsx` 删除质量字段卡片，增加只读入口卡。

- [ ] **Step 3: Build and fix types**

Run: `npm --prefix frontend run build`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add frontend/src/api.ts frontend/src/quality-page.tsx frontend/src/panel.tsx frontend/src/settings.tsx
git commit -m "feat(quality): 增加质量页骨架与规则管理"
```

---

### Task 13: 结果页、来源页与分组联动

**Files:**
- Modify: `frontend/src/quality-page.tsx`
- Modify: `frontend/src/groups.tsx`
- Modify: `frontend/src/quality.tsx`
- Test: `npm --prefix frontend run build`

**Interfaces:**
- Produces:
  - 结果表：筛选、分页、详情抽屉、已移出提示、暂停/源移除状态。
  - 来源表：门控开关、阶段、剩余候选、立即同步、停用/删除对话框。
  - 分组页徽章跳转 `#/quality/results?gid=...&cid=...`。
  - 频道编辑弹窗手工失败处理（继承 / 保留 / 下线）。

- [ ] **Step 1: Implement results and sources tabs**

- 结果表列按功能设计 §6.2；动作徽章包含 `admitted/removed/rejected/hidden/kept/held/removal_paused/source_removed`。
- 已移出行只读，提示需要重新同步源。
- 来源页只列 `mode=sync`；聚合源折叠说明。
- 停用对话框：保留/移出单选；移出二次确认；保留可记住选择。
- 删除对话框沿用现有 `keep_content`，增加二次确认。
- 分组页徽章增加跳转事件；质量页读取 URL 参数过滤。
- `quality.tsx` 更新 `QualityResult` 字段：`hidden` 改为 `action === 'hidden'`，`speed_mib` 保留，新增 `action` 徽章。
- `groups.tsx` 的单频道校验调用改为 `runQuality('channel', key)`；不再传 `force`。
- 质量变更后广播 `tv-buddy:quality-changed`，分组页与订阅页按需刷新。

- [ ] **Step 2: Build and verify**

Run: `npm --prefix frontend run build`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add frontend/src/quality-page.tsx frontend/src/groups.tsx frontend/src/quality.tsx
git commit -m "feat(quality): 增加结果与来源管理页"
```

---

### Task 14: Docker + Python 验收与文档

**Files:**
- Create: `scripts/smoke-quality-admission.sh`
- Create: `scripts/quality_mock_source.py`
- Modify: `docs/design/README.md`
- Modify: `docs/design/16-quality-admission-system.md`（实施状态）
- Test: 脚本输出 `smoke-quality-admission: PASS`

**Interfaces:**
- Consumes: Task 6-13 的接口与页面。
- Produces: 可重复的容器验收。

- [ ] **Step 1: Write the mock source**

`scripts/quality_mock_source.py`：

- 监听 `0.0.0.0:19932`。
- `/ok.m3u8`：媒体清单，切片快速返回。
- `/slow.m3u8`：媒体清单，切片每 100 ms 发送 1 KiB。
- `/dead.m3u8`：404。
- `/source.m3u8`：源清单，包含正常、慢速、不可达三个频道。
- `/control?slow=ok|slow`：切换慢速频道的地址，用于恢复测试。

- [ ] **Step 2: Write the smoke script**

`scripts/smoke-quality-admission.sh`：

1. 检查 curl、jq、python3、docker。
2. 启动 mock source。
3. `docker compose -f tests/manual/local/docker-compose.yml down --remove-orphans`，再 `up -d --build`。
4. 等待 `http://127.0.0.1:19900/health`。
5. 登录 `admin/admin`。
6. 写入 `[quality]` 与 sync 源配置：`probe_mode=speed`、`min_speed_mib=1.0`、`sample_secs=2`、安全阀开启、`min_remove_count=2`、`max_remove_ratio=0.5`。
7. 触发同步；轮询 `/panel/api/subs/status` 直到门控结束。
8. 断言：分组只有正常频道；结果页有 `rejected` 的慢速与 404。
9. 关闭门控同步三个频道，再开启门控并手动校验；断言慢速与 404 被移出。
10. 切换 `/control?slow=ok`，重新同步；断言慢速频道恢复。
11. 安全阀：制造 2 个失败，断言成员不变并出现暂停警告；关闭安全阀后重跑，断言移出。
12. 源停用保留/移出；源删除保留为本地/随源删除。
13. `docker stats --no-stream` 记录内存，断言低于 128 MB。
14. 输出 `smoke-quality-admission: PASS`。

- [ ] **Step 3: Run the script**

Run: `bash scripts/smoke-quality-admission.sh`
Expected: `smoke-quality-admission: PASS`

- [ ] **Step 4: Run full checks**

Run: `cargo test -v`
Expected: PASS

Run: `npm --prefix frontend run build`
Expected: PASS

- [ ] **Step 5: Update docs and commit**

- `docs/design/README.md` 把系统设计标为已实施。
- 系统设计顶部增加实施状态。
- 提交脚本与文档。

```bash
git add scripts/smoke-quality-admission.sh scripts/quality_mock_source.py docs/design/README.md docs/design/16-quality-admission-system.md
git commit -m "test(quality): 增加容器准入验收与文档状态"
```

## Self-Review

**Spec coverage**

| 系统设计项 | 任务 |
| --- | --- |
| 配置字段、校验、cron | Task 1 |
| 结果存储、保留清理 | Task 2、Task 8 |
| 原子提交 | Task 3 |
| 候选规则、目标分组覆盖 | Task 4 |
| 门控决策、计划过滤 | Task 5 |
| 门控探测、同步接入、游标、source_removed | Task 6 |
| 周期/手动引擎、安全阀 | Task 7 |
| cron/小时调度、运行状态 | Task 8 |
| 源停用/删除 | Task 9 |
| 质量接口 | Task 10 |
| 手工策略、重启水合 | Task 11 |
| 前端质量页 | Task 12 |
| 前端结果/来源/分组联动 | Task 13 |
| Docker 验收 | Task 14 |

**Review Focus 对应测试**

| 关注点 | 测试 |
| --- | --- |
| 截断不误删已准入、不误准入新候选 | Task 5 `truncation_keeps_admitted_and_advances_cursor`、`unprocessed_new_candidates_are_not_admitted`；Task 6 集成 |
| URL 变化 `probe_error` 保留 | Task 5 `url_changed_probe_error_pauses_sync` |
| 未变化频道不重复探测 | Task 5 `truncation...`；Task 6 二次同步断言 |
| 事务回滚 | Task 3 `mutate_admission_rolls_back_on_*` |
| 安全阀触发/关闭 | Task 5 `guard_trips_at_exact_threshold`、`guard_disabled_...`；Task 7 `guard_pauses_sync_removals` |
| 源停用竞争 | Task 7 `removal_revalidation_skips_changed_source_or_url`；Task 9 停用测试 |
| 手工下线重启保留 | Task 11 `manual_entries_hydrate_after_restart` |
| 全局关闭只测量 | Task 8 `disabled_config_means_measure_only`；Task 10 契约测试 |

**类型一致性**

- `Candidate` 在 Task 4 定义，Task 5/6/7 使用。
- `QualityResult` 在 Task 2 定义，Task 3/5/6/7/9/10/11 使用。
- `GateOutcome` 在 Task 5 定义，Task 6 使用。
- `EngineDecision` 在 Task 7 定义，Task 8 使用。
- `RunState` 在 Task 7 扩展，Task 8/10 使用。
- `mutate_admission` 的闭包返回 `Vec<QualityResult>`，Task 3/6/7/9 一致使用。

## 完成后的发布动作

实现、整支分支复核与容器验收通过后：

- 由用户选择合并或 PR。
- 按用户偏好打 tag 并推送。版本号用 `v2026.N.M`；预发布加 `-beta.N`。不攒批。
