# 缓存配置与磁盘回收收敛实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把缓存配置收敛为「一个概念一个键」，只保留磁盘后端，默认自动计算总磁盘上限，并让回收同时约束切片正文与索引。

**Architecture:** 配置模型最终只保留 `cache.limit_mb`、`cache.auto_tune`、`cache.ttl`、`cache.segments.per_channel_max` 与 `cache.gc.max_age_secs`。自动上限由工作集、可用空间与总容量推导，返回 `cap_mb` 与 `limited_by`。GC 每 60 秒扫描 `seg_bytes` 与 `seg_url`，先压索引子预算（10%），再压总量；进程内索引按 600 秒活动窗口淘汰。

**Tech Stack:** Rust 2021 / Axum 0.7 / tokio / serde_json / rustix statvfs；React 18 / TypeScript 5 / Vite 6。

**Spec:** `docs/superpowers/specs/2026-11-21-cache-config-consolidation-design.md`

## Global Constraints

- 只保留磁盘缓存；删除 `MemoryStore` 与 `cache.backend`。切片正文不进入进程内存。
- 默认 `cache.limit_mb = 0`（自动计算）；非 0 为手动上限，优先于自动估算。
- 默认 `cache.auto_tune = true`；关闭只停止新采样，不冻结上限。
- 最终默认 `cache.ttl.segment_secs = 0`（自动，无实测用 600 秒）。
- 默认 `cache.gc.max_age_secs = 1800`；`0` 表示不按时间删除。
- 常量：`TOTAL_RATIO = 0.7`、`DISK_BUDGET_RATIO = 0.8`、`RESERVE_MB = 100`、`MIN_CAP_MB = 50`、`GC_INTERVAL_SECS = 60`、`GC_SCAN_FLOOR = 500`、`GC_SCAN_MAX = 5000`、`SEGMENT_EST_MB = 4`、`MIN_CONCURRENT_CHANNELS = 3`、`INDEX_RATIO = 0.10`、`INDEX_TTL_SECS = 600`。
- 旧键不读取、不迁移；`Config::normalize` 从各层 `extra` 删除旧键。
- 任务按顺序执行。新增字段先与旧字段并存，等所有读取方迁移后，最后一个配置任务再删除旧字段，保证每个任务结束时编译通过。
- 每个任务结束运行该任务指定的测试并提交。最后统一运行 fmt、clippy、test、前端构建。

## Review Focus

- 旧库含 `"cache": {"backend": "memory", ...}` 时：服务必须按磁盘后端启动，页面不崩溃，旧键不写回。测试：Task 9 `normalize_strips_legacy_memory_config`。
- 无实测且磁盘接近满（例如可用 120 MB）：回退 150 MB 必须被夹到下限 50 MB，不得突破可用空间。测试：Task 5 `cap_plan_fallback_participates_in_clamp`。
- `statvfs` 失败且工作集 30 MB：上限取 50 MB（下限），不进入紧急模式。测试：Task 5 `cap_plan_space_unknown_applies_floor`。
- 索引目录超过总上限 10%：先删最旧索引文件，再按总量删切片。测试：Task 6 `sweep_prunes_index_before_total`。
- 空闲时活跃频道为 0、短时换台为 10：计算用频道数分别取 3 与 10，页面显示真实活跃数。测试：Task 3 `active_channels_tracks_window`、Task 2 `working_set_scales_with_channel_count`。

---

### Task 1: 新增缓存键与校验

**Files:**
- Modify: `src/config/model.rs:288-346`
- Modify: `src/config.rs:20-89`
- Modify: `src/config/store.rs:136-166`
- Test: same files

**Interfaces:**
- Produces:
  - `CacheCfg.limit_mb: u64`（默认 `0`）
  - `CacheCfg.auto_tune: bool`（默认 `true`）
  - `Config::normalize()` 在 `disk_path` 为空白时补 `"./cache"`
  - `validate` 拒绝空 `disk_path` 与 `max_age_secs ∈ 1..=59`
- 本任务保留旧字段，供后续任务逐步迁移。

- [ ] **Step 1: 写失败测试**

在 `src/config/model.rs` 的 `tests` 模块加入：

```rust
#[test]
fn new_cache_keys_have_disk_only_defaults() {
    let cache = Config::default().cache;
    assert_eq!(cache.disk_path, "./cache");
    assert_eq!(cache.limit_mb, 0);
    assert!(cache.auto_tune);
}

#[test]
fn normalize_fills_empty_cache_disk_path() {
    let mut config = Config::default();
    config.cache.disk_path = "  ".into();
    config.normalize();
    assert_eq!(config.cache.disk_path, "./cache");
}
```

在 `src/config.rs` 的 `tests` 模块加入：

```rust
#[test]
fn validate_rejects_empty_cache_disk_path() {
    let mut config = Config::default();
    config.cache.disk_path = "  ".into();
    let error = validate(&config).expect_err("空白路径应拒绝");
    assert!(error.to_string().contains("disk_path"), "{error}");
}

#[test]
fn validate_rejects_short_gc_max_age() {
    let mut config = Config::default();
    config.cache.gc.max_age_secs = 59;
    assert!(validate(&config).is_err());
    config.cache.gc.max_age_secs = 60;
    validate(&config).expect("60 秒起合法");
    config.cache.gc.max_age_secs = 0;
    validate(&config).expect("0 表示关闭");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test config::model::tests::new_cache_keys_have_disk_only_defaults config::tests::validate_rejects_empty_cache_disk_path`

Expected: 编译失败（字段不存在）。

- [ ] **Step 3: 加字段与默认值**

`CacheCfg` 在 `disk_path` 后加：

```rust
/// 磁盘缓存总上限（MB），包含正文与索引；`0` = 自动。
pub limit_mb: u64,
/// 被动采样开关；关闭后沿用上次实测，不冻结上限。
pub auto_tune: bool,
```

`Default` 加 `limit_mb: 0`、`auto_tune: true`。

- [ ] **Step 4: normalize 与校验**

`Config::normalize` 开头加：

```rust
if self.cache.disk_path.trim().is_empty() {
    self.cache.disk_path = "./cache".into();
}
```

`validate` 加：

```rust
if config.cache.disk_path.trim().is_empty() {
    bail!("cache.disk_path 不能为空");
}
if (1..60).contains(&config.cache.gc.max_age_secs) {
    bail!("cache.gc.max_age_secs 必须是 0 或至少 60");
}
```

- [ ] **Step 5: 补 JSON 往返测试**

在 `src/config/store.rs` 的 `config_round_trips_through_json` 加：

```rust
config.cache.limit_mb = 120;
// …保存后…
assert_eq!(loaded.cache.limit_mb, 120);
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test config::`

Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/config/model.rs src/config.rs src/config/store.rs
git commit -m "feat(config): 新增缓存上限与自动估算键"
```

---

### Task 2: 工作集按活跃频道数计算

**Files:**
- Modify: `src/cache_estimate.rs:1-48`, `src/cache_estimate.rs:201-213`, tests at `src/cache_estimate.rs:301-456`
- Test: same file

**Interfaces:**
- Produces: `Estimator::working_mb(&self, working_channels: u64) -> Option<u64>`
- Consumes: 无

- [ ] **Step 1: 写失败测试**

把 `segment_ema_converges` 的调用改为 `working_mb(3)`，并新增：

```rust
#[test]
fn working_set_scales_with_channel_count() {
    let dir = temp_dir("channels");
    let estimator = Estimator::load_or_default(&dir);
    for _ in 0..40 {
        estimator.observe_segment("g", 4_463_308);
    }
    estimator.observe_playlist("g", &playlist());
    assert_eq!(estimator.working_mb(3), Some(140));
    assert_eq!(estimator.working_mb(6), Some(280));
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test cache_estimate::tests::working_set_scales_with_channel_count`

Expected: 编译失败（参数不匹配）。

- [ ] **Step 3: 改签名与公式**

```rust
pub fn working_mb(&self, working_channels: u64) -> Option<u64> {
    let mut best: Option<f64> = None;
    for stat in self.read().values() {
        let (Some(window), Some(seg_bytes)) = (stat.window.as_ref(), stat.seg_bytes) else {
            continue;
        };
        let seg_mb = seg_bytes / MB as f64;
        let value = working_channels as f64 * window.count as f64 * seg_mb * WORKING_HEADROOM;
        best = Some(best.map_or(value, |prev| prev.max(value)));
    }
    best.map(round10)
}
```

删除 `CONCURRENT_CHANNELS`，把文件头与结构体注释里的「3 ×」改为「参与计算的频道数 ×」。

- [ ] **Step 4: 更新其余测试调用与临时调用点**

`no_samples_is_none`、`persist_round_trips`、并发采集相关测试中的 `working_mb()` 全部改为 `working_mb(3)`。

在 `src/cache_estimate.rs` 加 `pub const MIN_CONCURRENT_CHANNELS: u64 = 3;`，并把 `src/cache_gc.rs` 的两处调用临时改为：

```rust
state.estimate.working_mb(crate::cache_estimate::MIN_CONCURRENT_CHANNELS)
```

Task 6 会把它替换为实测活跃频道数。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test cache_estimate:: cache_gc::`

Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/cache_estimate.rs
git commit -m "feat(cache): 工作集按活跃频道数计算"
```

---

### Task 3: 切片索引活动窗口、TTL 与索引文件预算

**Files:**
- Modify: `src/proxy/segments.rs:28-163`, tests at `src/proxy/segments.rs:276-398`
- Modify: `src/state.rs:152-160`, `src/state.rs:449-453`
- Test: same files

**Interfaces:**
- Produces:
  - `pub const INDEX_TTL_SECS: u64 = 600;`
  - `SegmentIndex::note_channel(&self, gid: &str, cid: &str)`
  - `SegmentIndex::active_channels(&self, window: Duration) -> usize`
  - `SegmentIndex::set_limits(&self, per_channel: u64)`
  - `pub fn index_usage(dir: &Path) -> (u64, u64)`
  - `pub fn prune_index(dir: &Path, budget_bytes: u64, limit: usize) -> (u64, u64)`

- [ ] **Step 1: 写失败测试**

在 `src/proxy/segments.rs` 测试模块加入：

```rust
#[test]
fn active_channels_tracks_window() {
    let index = SegmentIndex::new();
    index.note_channel("g", "c1");
    index.note_channel("g", "c2");
    assert_eq!(index.active_channels(Duration::from_secs(90)), 2);
    let now = now_epoch();
    index
        .activity
        .write()
        .unwrap()
        .insert("g/c1".into(), now.saturating_sub(100));
    assert_eq!(index.active_channels(Duration::from_secs(90)), 1);
    assert_eq!(index.active_channels(Duration::from_secs(120)), 2);
}

#[test]
fn index_ttl_evicts_stale_entries() {
    let index = SegmentIndex::new();
    index.set_limits(0);
    index.write().insert(
        "g/c/old".into(),
        Entry {
            url: "http://up/old.ts".into(),
            ext: "jpg".into(),
            at: now_epoch().saturating_sub(601),
        },
    );
    // GC 每轮调用 active_channels；这里顺带清掉过期索引。
    assert_eq!(index.active_channels(Duration::from_secs(90)), 0);
    assert!(index.read().get("g/c/old").is_none(), "超过 600 秒应淘汰");
}

#[test]
fn prune_index_deletes_oldest_files() {
    let dir = std::env::temp_dir().join(format!("tv-buddy-index-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建目录");
    for index in 0..5 {
        let path = dir.join(format!("{index}.json"));
        std::fs::write(&path, vec![b'x'; 1_000]).expect("写文件");
        let when = SystemTime::now() - Duration::from_secs(10 * (5 - index));
        std::fs::File::open(&path)
            .expect("打开")
            .set_modified(when)
            .expect("mtime");
    }
    let (files, bytes) = prune_index(&dir, 2_000, 100);
    assert_eq!(files, 3);
    assert_eq!(bytes, 3_000);
    assert_eq!(std::fs::read_dir(&dir).expect("读目录").count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test proxy::segments::tests`

Expected: 编译失败（方法不存在）。

- [ ] **Step 3: 加活动表与 TTL**

```rust
activity: RwLock<HashMap<String, u64>>,
```

`Limits` 只留 `per_channel`。`set_limits` 改为 `(&self, per_channel: u64)`。

`note_channel` 写入 `format!("{gid}/{cid}") -> now_epoch()`。

`active_channels(window)` 删除 `now - ts > window.as_secs()` 的活动条目，同时清理 `inner` 中 `now - entry.at > INDEX_TTL_SECS` 的索引条目，返回剩余活跃频道数。GC 每轮调用它，空闲进程的内存索引也会收缩。

`register` 在插入前也清理一遍过期索引条目。

`State` 两处调用改为：

```rust
self.segments.set_limits(config.cache.segments.per_channel_max);
```

- [ ] **Step 4: 加索引文件统计与裁剪**

`index_usage` 遍历 `dir` 下直接子文件，返回 `(文件数, 总字节)`；读目录失败返回 `(0, 0)`。

`prune_index`：

- 收集文件与 `(len, mtime)`。
- 按 mtime 升序排序。
- 从最旧开始删除，直到总字节 ≤ `budget_bytes` 或删除数达到 `limit`。
- 返回 `(删除数, 删除字节)`。

- [ ] **Step 5: 删除全局条目上限**

删除 `ENTRY_BYTES_EST`、`memory_budget_entries`、`global_limit` 与测试 `global_limit_folds_memory_budget`。

`limits_trim_registered_entries` 改为 `index.set_limits(2)`。

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test proxy::segments:: state::`

Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/proxy/segments.rs src/state.rs
git commit -m "feat(cache): 索引活动窗口与文件预算"
```

---

### Task 4: 统一切片 TTL

**Files:**
- Modify: `src/config/model.rs:474-496`（`CacheTtlCfg::default`）
- Modify: `src/http/routes/stream.rs:45-74`
- Modify: `tests/http_contract.rs:5884-5992`
- Test: same

**Interfaces:**
- Consumes: `Estimator::ttl_secs`
- Produces: `cache.ttl.segment_secs = 0` 默认自动；手动值在 CDN 开/关下一致。

- [ ] **Step 1: 写失败测试**

在 `tests/http_contract.rs` 把 `cdn_explicit_ttl_wins` 的配置改为：

```rust
let fixture =
    fixture_with_extra(r#"{"cache":{"ttl":{"segment_secs":777}},"cdn":{"enabled":true}}"#).await;
```

新增：

```rust
#[tokio::test]
async fn manual_ttl_applies_when_cdn_disabled() {
    let fixture = fixture_with_extra(r#"{"cache":{"ttl":{"segment_secs":777}}}"#).await;
    let response = fetch(&fixture, &format!("/pg/c1.m3u8?token={}", fixture.token)).await;
    let body = response.text().await.expect("读正文");
    let path = segment_path(&body, &fixture.base, 1);
    let response = fetch(&fixture, &path).await;
    assert_eq!(header(&response, "cache-control"), "public, max-age=777");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --test http_contract cdn_explicit_ttl_wins manual_ttl_applies_when_cdn_disabled`

Expected: 失败（关闭 CDN 时仍读 `max(1)` 的自动值）。

- [ ] **Step 3: 改默认值**

`CacheTtlCfg::default()` 的 `segment_secs` 改为 `0`。

- [ ] **Step 4: 统一 segment_cache_control**

```rust
fn segment_cache_control(state: &AppState, first: bool) -> String {
    let config = state.snapshot();
    let secs = if config.cache.ttl.segment_secs != 0 {
        config.cache.ttl.segment_secs
    } else {
        state
            .estimate
            .ttl_secs()
            .unwrap_or(crate::cache_estimate::FALLBACK_TTL_SECS)
    };
    let scope = if !config.cdn.enabled {
        if first { "public" } else { "private" }
    } else if first || config.cdn.shared_cache_hits {
        "public"
    } else {
        "private"
    };
    format!("{scope}, max-age={secs}")
}
```

- [ ] **Step 5: 更新默认 TTL 断言**

Run: `cargo test --test http_contract`

按结果更新断言：已解析清单的用例仍为 300 秒；没有清单样本、直接请求切片的用例改为 600 秒。

- [ ] **Step 6: 提交**

```bash
git add src/config/model.rs src/http/routes/stream.rs tests/http_contract.rs
git commit -m "feat(cache): 统一切片 TTL 解析"
```

---

### Task 5: 上限计划与限制来源

**Files:**
- Modify: `src/cache_gc.rs:26-101`, tests at `src/cache_gc.rs:335-413`
- Test: same file

**Interfaces:**
- Produces:
  - `pub enum LimitedBy { Manual, Working, Budget, Ceiling, Floor, NoData, SpaceUnknown }`
  - `impl LimitedBy { pub fn as_str(self) -> &'static str }`
  - `pub struct CapPlan { pub cap_mb: u64, pub limited_by: LimitedBy, pub budget_mb: Option<u64>, pub ceiling_mb: Option<u64> }`
  - `pub fn cap_plan(explicit_mb: u64, space: Option<(u64, u64)>, working_mb: Option<u64>) -> CapPlan`
  - `pub fn scan_limit(cap_mb: u64) -> usize`
- 本任务保留 `plan` 与 `effective_cap_mb` 的旧签名，内部改为调用 `cap_plan`；`tick` 与 `recommendation` 仍在下一个任务切换。

- [ ] **Step 1: 写失败测试**

替换旧的上限测试：

```rust
#[test]
fn cap_plan_reports_limit_source() {
    let plan = cap_plan(0, Some((1024, 800)), Some(140));
    assert_eq!((plan.cap_mb, plan.limited_by), (140, LimitedBy::Working));
    assert_eq!(plan.budget_mb, Some(540));
    assert_eq!(plan.ceiling_mb, Some(716));

    let plan = cap_plan(0, Some((1024, 300)), Some(140));
    assert_eq!((plan.cap_mb, plan.limited_by), (140, LimitedBy::Working));

    let plan = cap_plan(0, Some((2048, 2000)), Some(3000));
    assert_eq!((plan.cap_mb, plan.limited_by), (1433, LimitedBy::Ceiling));
}

#[test]
fn cap_plan_fallback_participates_in_clamp() {
    let plan = cap_plan(0, Some((1024, 800)), None);
    assert_eq!((plan.cap_mb, plan.limited_by), (150, LimitedBy::NoData));

    let plan = cap_plan(0, Some((1024, 120)), None);
    assert_eq!((plan.cap_mb, plan.limited_by), (50, LimitedBy::Floor));
}

#[test]
fn cap_plan_space_unknown_applies_floor() {
    let plan = cap_plan(0, None, Some(140));
    assert_eq!((plan.cap_mb, plan.limited_by), (140, LimitedBy::SpaceUnknown));

    let plan = cap_plan(0, None, Some(30));
    assert_eq!((plan.cap_mb, plan.limited_by), (50, LimitedBy::SpaceUnknown));

    let plan = cap_plan(0, None, None);
    assert_eq!((plan.cap_mb, plan.limited_by), (150, LimitedBy::SpaceUnknown));
}

#[test]
fn manual_cap_wins() {
    let plan = cap_plan(200, Some((1024, 800)), Some(140));
    assert_eq!((plan.cap_mb, plan.limited_by), (200, LimitedBy::Manual));
    assert_eq!(plan.budget_mb, None);
    assert_eq!(plan.ceiling_mb, None);
}

#[test]
fn scan_limit_clamps() {
    assert_eq!(scan_limit(140), 500);
    assert_eq!(scan_limit(20_000), 5_000);
    assert_eq!(scan_limit(4_000), 1_000);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test cache_gc::tests`

Expected: 编译失败（`cap_plan` 不存在）。

- [ ] **Step 3: 定义常量与类型**

按 Global Constraints 写常量。删除 `INTERVAL_FLOOR_SECS`；保留 `STARTUP_DELAY_SECS = 30`。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitedBy {
    Manual,
    Working,
    Budget,
    Ceiling,
    Floor,
    NoData,
    SpaceUnknown,
}

impl LimitedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Working => "working",
            Self::Budget => "budget",
            Self::Ceiling => "ceiling",
            Self::Floor => "floor",
            Self::NoData => "no_data",
            Self::SpaceUnknown => "space_unknown",
        }
    }
}
```

- [ ] **Step 4: 实现 cap_plan 与 scan_limit**

`cap_plan` 规则：

- `explicit_mb != 0` → `Manual`，`budget_mb/ceiling_mb = None`。
- `space = None` → `cap = max(MIN_CAP_MB, working.unwrap_or(FALLBACK))`，`SpaceUnknown`。
- `space = Some((total, free))`：
  - `working = working_mb.unwrap_or(FALLBACK_DISK_MB)`
  - `budget = (free as f64 * DISK_BUDGET_RATIO - RESERVE_MB as f64).max(0.0) as u64`
  - `ceiling = ((total as f64 * TOTAL_RATIO) as u64).max(MIN_CAP_MB)`
  - `raw = working.min(budget)`
  - `cap = raw.clamp(MIN_CAP_MB, ceiling)`
  - `limited_by`：`raw > ceiling` → `Ceiling`；`raw < MIN_CAP_MB` → `Floor`；`budget < working` → `Budget`；`working_mb.is_none()` → `NoData`；否则 `Working`。

`scan_limit(cap_mb) = (cap_mb / SEGMENT_EST_MB).clamp(GC_SCAN_FLOOR, GC_SCAN_MAX) as usize`。

- [ ] **Step 5: 旧接口改为包装**

`effective_cap_mb(cfg, explicit, total, free, working)` 改为：

```rust
pub fn effective_cap_mb(
    _cfg: &CacheGcCfg,
    explicit_mb: u64,
    total_mb: u64,
    free_mb: u64,
    working_mb: Option<u64>,
) -> u64 {
    cap_plan(explicit_mb, Some((total_mb, free_mb)), working_mb).cap_mb
}
```

`explicit_cap_mb` 改为返回 `cache.limit_mb`：

```rust
pub fn explicit_cap_mb(cache: &CacheCfg) -> u64 {
    cache.limit_mb
}
```

删除旧测试 `explicit_cap_skips_formula`、`explicit_cap_resolves_cache_aliases`。

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test cache_gc::tests`

Expected: PASS。旧 `plan_emergency_when_below_reserve` 仍通过（旧 `plan` 仍存在）。

- [ ] **Step 7: 提交**

```bash
git add src/cache_gc.rs
git commit -m "feat(cache): 上限计划与限制来源"
```

---

### Task 6: 回收子预算、上次回收时间与推荐块

**Files:**
- Modify: `src/cache_gc.rs:79-101`, `src/cache_gc.rs:145-333`, tests at `src/cache_gc.rs:415-581`
- Modify: `src/state.rs:33-125`, `src/state.rs:160-212`
- Test: same files

**Interfaces:**
- Consumes: `segments::prune_index`、`segments::index_usage`、`Estimator::working_mb(u64)`、`SegmentIndex::active_channels`
- Produces:
  - `fn sweep(body_dir: &Path, index_dir: &Path, cap_bytes: u64, max_age: Duration, limit: usize, emergency: bool) -> (u64, u64)`
  - `pub fn plan(explicit_mb: u64, space: Option<(u64, u64)>, working_mb: Option<u64>) -> Plan`
  - `AppState.gc_last_run_epoch: AtomicU64`
  - `recommendation(&AppState) -> serde_json::Value` 新字段

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn sweep_prunes_index_before_total() {
    let body = temp_dir("idx-body");
    let index = temp_dir("idx-index");
    for n in 0..10 {
        touch(&body.join(format!("b{n}.jpg")), 1_000, Duration::from_secs(300));
    }
    for n in 0..20 {
        let age = Duration::from_secs(10 * (20 - n));
        touch(&index.join(format!("{n}.json")), 1_000, age);
    }
    // cap 10 KB → 索引子预算 1 KB，总量 10 KB。
    let (files, _) = sweep(&body, &index, 10_000, Duration::from_secs(86_400), 500, false);
    assert!(files >= 19, "至少删掉 19 个超额索引");
    let index_left = std::fs::read_dir(&index).expect("读索引目录").count();
    assert_eq!(index_left, 1, "索引保留 1 个文件");
    let total = std::fs::read_dir(&body).expect("读正文目录").count() as u64 * 1_000
        + index_left as u64 * 1_000;
    assert!(total <= 10_000, "总量应回收到上限内");
    let _ = std::fs::remove_dir_all(&body);
    let _ = std::fs::remove_dir_all(&index);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test cache_gc::tests::sweep_prunes_index_before_total`

Expected: 编译失败（`sweep` 签名不匹配）。

- [ ] **Step 3: 重写 sweep**

先在 `src/cache_gc.rs` 补 `use crate::proxy::segments;`。

`sweep` 顺序：

1. 超龄删除：同时扫描 `body_dir` 与 `index_dir`，按 mtime 从旧到新，`max_age` 内保留；`emergency` 时跳过这一步。删除数计入 `limit`。
2. 索引子预算：`let index_budget = (cap_bytes as f64 * INDEX_RATIO) as u64;`，调用 `segments::prune_index(index_dir, index_budget, limit.saturating_sub(removed))`。
3. 重新扫描两个目录，合计 `total`。
4. 目标：`emergency ? cap_bytes / 2 : cap_bytes`。按 mtime 升序删除，直到 `total <= target` 或达到 `limit`。
5. 记录日志并返回 `(files, bytes)`。

同时更新现有 `sweep` 测试的调用：第一个参数是正文目录，第二个参数是索引目录。正文专用用例建一个空索引临时目录：

```rust
let index = temp_dir("lru-index");
let (files, bytes) = sweep(&dir, &index, 3_000, Duration::from_secs(86_400), 500, false);
```

`plan_emergency_when_below_reserve` 改为新签名，并断言 `statvfs` 失败、工作集 140 MB 时上限为 140 MB（而不是固定 150 MB）。

- [ ] **Step 4: 替换 plan 与 tick**

删除 `effective_cap_mb`。`plan` 改为：

```rust
pub fn plan(explicit_mb: u64, space: Option<(u64, u64)>, working_mb: Option<u64>) -> Plan {
    let cap = cap_plan(explicit_mb, space, working_mb);
    Plan {
        cap_bytes: cap.cap_mb.saturating_mul(MB),
        emergency: space.map_or(false, |(_, free)| free < RESERVE_MB),
    }
}
```

`AppState` 加：

```rust
pub gc_last_run_epoch: std::sync::atomic::AtomicU64,
```

初始化 `0`。

`spawn` 去掉 `gc.enabled` 判断，固定 `GC_INTERVAL_SECS` 睡眠。

`tick`：

```rust
let window = Duration::from_secs(config.stats.online_window_secs.max(1));
let active = state.segments.active_channels(window) as u64;
let working_channels = active.max(MIN_CONCURRENT_CHANNELS);
let working = state.estimate.working_mb(working_channels);
let plan = plan(config.cache.limit_mb, space, working);
let limit = scan_limit(plan.cap_bytes / MB);
```

扫描后：

```rust
state
    .gc_last_run_epoch
    .store(now_epoch(), std::sync::atomic::Ordering::Relaxed);
```

- [ ] **Step 5: 重写 recommendation**

返回：

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
  "budget_mb": 540,
  "ceiling_mb": 716,
  "gc_last_run_secs_ago": 120,
  "groups": 2,
  "updated_secs_ago": 30,
  "disk_total_mb": 1024,
  "disk_free_mb": 800,
  "segment_body_bytes": 130000000,
  "segment_index_bytes": 2000000,
  "segment_index_limit_bytes": 14000000,
  "segment_cache_bytes": 132000000,
  "segment_body_files": 35,
  "auto_tune": true
}
```

- `ttl_source`：`cache.ttl.segment_secs != 0` → `manual`；否则有实测 → `measured`；无实测 → `fallback`。
- `ttl_secs`：手动值，或 `estimate.ttl_secs().unwrap_or(FALLBACK_TTL_SECS)`。
- `cap_source`：`limit_mb != 0` → `manual`，否则 `auto`。
- `channels`：`active_channels(window)`；`working_channels = max(3, channels)`。
- `segment_index_limit_bytes = ((cap_mb * MB) as f64 * INDEX_RATIO) as u64`。
- `segment_body_bytes` 取 `dir_usage(seg_bytes)`；`segment_index_bytes` 取 `segments::index_usage(seg_url)`。
- `gc_last_run_secs_ago`：`last == 0` → `null`。

同时更新 `tests/http_contract.rs` 中已存在的推荐块断言：

- `estimate_returns_recommendation`：`recommend.source` → `recommend.ttl_source`。
- `estimate_all_failed_is_not_500`：同上。
- `cache_stats_includes_recommendation`：同上；`recommend.seg_bytes` 改为 `recommend.segment_body_bytes`（若存在）。

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test cache_gc::`

Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/cache_gc.rs src/state.rs
git commit -m "feat(cache): 索引子预算与回收状态"
```

---

### Task 7: 删除内存后端

**Files:**
- Delete: `src/cache_memory.rs`
- Modify: `src/lib.rs:17`
- Modify: `src/state.rs:53-56`, `src/state.rs:152-160`, `src/state.rs:176-178`, `src/state.rs:447-457`
- Modify: `src/http/routes/stream.rs:658-768`, `src/http/routes/stream.rs:1427-1579`
- Modify: `tests/http_contract.rs:450-530`, `tests/http_contract.rs:4315-4338`
- Test: `tests/http_contract.rs`

**Interfaces:**
- Consumes: `SegmentIndex::note_channel`
- Produces: 磁盘单后端；预取、切片读取与写入只走磁盘。

- [ ] **Step 1: 状态与模块清理**

- `src/lib.rs` 删除 `pub mod cache_memory;`。
- 删除 `src/cache_memory.rs`。
- `AppState` 删除 `memory` 字段与初始化。
- `AppState::new` 与 `replace` 的 `estimate.set_auto_tune` 改为 `config.cache.auto_tune`。

- [ ] **Step 2: 切片路径只走磁盘**

`spawn_prefetch`：删除 `memory`、`memory_limit`、`key`；只保留 `path.is_file()` 检查与磁盘写入。

`segment`：删除 `memory` 分支，直接 `std::fs::read(&path)`。

`stream_segment`：删除 `key` 与 `memory` 参数；完整正文只写 `path`。

- [ ] **Step 3: 运行旧内存测试确认失败**

Run: `cargo test --test http_contract memory_backend_keeps_segments_in_process`

Expected: FAIL（切片正文现在写入 `seg_bytes`，测试仍断言 0 个文件）。

- [ ] **Step 4: 删除内存测试与夹具参数**

- 删除 `memory_backend_keeps_segments_in_process` 与 `fixture_with_backend`。
- `fixture_with_options` 去掉 `cache_backend` 参数与 JSON `"backend"` 键；3 个调用点同步。

- [ ] **Step 5: 记录活跃频道**

`media_entry` 开头加：

```rust
state.segments.note_channel(gid, cid);
```

`serve_rest` 在签名校验之后加同一行，覆盖 proxy、remux、dash、timeshift。

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test --test http_contract`

Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/lib.rs src/state.rs src/http/routes/stream.rs tests/http_contract.rs
git rm src/cache_memory.rs
git commit -m "refactor(cache): 删除内存后端"
```

---

### Task 8: 运维接口去掉后端字段并补占用字段

**Files:**
- Modify: `src/http/routes/admin.rs:17-109`
- Modify: `src/http/routes/panel.rs:157-177`
- Modify: `src/http/routes/dash.rs:161-164`
- Modify: `src/http/routes/cdn.rs:27-31`
- Modify: `tests/http_contract.rs:6274-6296`, `:6126-6128`, `:6186-6187`
- Test: same

**Interfaces:**
- Produces:
  - `GET /panel/api/cache/stats`：删除 `backend`；新增 `segment_body_bytes`、`segment_index_bytes`、`segment_cache_bytes`。
  - `/panel/api/status`（panel）：删除 `cache_backend`。
  - `/panel/api/health`（dash）：`cache` 对象删除 `backend`。

- [ ] **Step 1: 改契约测试**

`cache_stats_includes_recommendation`：

```rust
assert!(body.get("backend").is_none(), "单后端不再返回 backend");
assert!(body["segment_body_bytes"].is_u64(), "{body}");
assert!(body["segment_index_bytes"].is_u64(), "{body}");
assert_eq!(
    body["segment_cache_bytes"].as_u64(),
    Some(body["segment_body_bytes"].as_u64().unwrap() + body["segment_index_bytes"].as_u64().unwrap())
);
assert_eq!(body["recommend"]["ttl_secs"], 600, "{body}");
assert_eq!(body["recommend"]["ttl_source"], "fallback", "{body}");
assert!(body["recommend"]["disk_cap_mb"].is_u64(), "{body}");
assert!(body["recommend"]["segment_body_bytes"].is_u64(), "{body}");
```

`estimate_returns_recommendation` 与 `estimate_all_failed_is_not_500` 把 `recommend.source` 改为 `recommend.ttl_source`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --test http_contract cache_stats_includes_recommendation`

Expected: FAIL（`backend` 仍返回，新字段不存在）。

- [ ] **Step 3: 改 admin**

- `cache_flush` 删除内存清理，直接从 `removed = 0` 开始。
- `cache_stats` 删除 `"backend"`。
- 计算：

```rust
let segment_body_bytes = dir_bytes("seg_bytes");
let segment_index_bytes = dir_bytes("seg_url");
```

响应加：

```json
"segment_body_bytes": segment_body_bytes,
"segment_index_bytes": segment_index_bytes,
"segment_cache_bytes": segment_body_bytes + segment_index_bytes,
```

- [ ] **Step 4: 改 panel、dash 与 cdn 注释**

- `panel.rs` 删除 `"cache_backend": cfg.cache.backend`。
- `dash.rs` 的 `"cache"` 对象删除 `"backend"`。
- `cdn.rs` 文档注释 `recommend.source` 改为 `recommend.ttl_source`。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test --test http_contract`

Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/http/routes/admin.rs src/http/routes/panel.rs src/http/routes/dash.rs src/http/routes/cdn.rs tests/http_contract.rs
git commit -m "refactor(api): 缓存统计去掉后端字段"
```

---

### Task 9: 删除旧配置字段与旧键清理

**Files:**
- Modify: `src/config/model.rs:288-496`, `:1607-1634`, `:1845-1875`
- Modify: `src/config.rs` tests
- Modify: `src/config/store.rs:136-166`
- Modify: `src/cli.rs:260-270`
- Test: same files

**Interfaces:**
- Consumes: 此前任务已停止读取旧字段
- Produces:
  - `CacheCfg { disk_path, limit_mb, auto_tune, prefetch, segments, gc, ttl, dash, remux, disguise, extra }`
  - `CacheSegmentsCfg { per_channel_max, extra }`
  - `CacheGcCfg { max_age_secs, extra }`
  - `CacheTtlCfg { sub_url_secs, segment_secs, extra }`
  - `CdnCfg` 不再有 `auto_tune` 与 `segment_max_age_secs`
  - `Config::normalize` 从 `extra` 删除旧键

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn cache_defaults_match_disk_only_design() {
    let cache = Config::default().cache;
    assert_eq!(cache.disk_path, "./cache");
    assert_eq!(cache.limit_mb, 0);
    assert!(cache.auto_tune);
    assert_eq!(cache.ttl.segment_secs, 0);
    assert_eq!(cache.gc.max_age_secs, 1_800);
    assert_eq!(cache.segments.per_channel_max, 0);
}

#[test]
fn normalize_strips_legacy_cache_keys() {
    let mut config: Config = serde_json::from_value(serde_json::json!({
        "cache": {
            "backend": "memory",
            "disk_path": "",
            "m3u8_max_entries": 1,
            "segment_max_entries": 2,
            "disk_max_mb": 3,
            "memory_max_mb": 4,
            "segment_cache_max_mb": 5,
            "segments": { "global_max": 6, "retain_count": 7, "per_channel_max": 8 },
            "gc": {
                "enabled": false, "max_disk_mb": 9, "max_age_secs": 1800,
                "interval_secs": 300, "scan_limit": 500,
                "disk_budget_ratio": 0.8, "reserve_mb": 100, "min_mb": 50
            },
            "ttl": { "refresh_ratio": 0.4, "fallback_mpd_secs": 8 }
        },
        "cdn": { "auto_tune": false, "segment_max_age_secs": 0 }
    }))
    .expect("解析旧配置");
    config.normalize();
    assert_eq!(config.cache.disk_path, "./cache", "空路径补默认值");
    let encoded = serde_json::to_value(&config).expect("序列化");
    let cache = &encoded["cache"];
    for key in [
        "backend", "m3u8_max_entries", "segment_max_entries", "disk_max_mb",
        "memory_max_mb", "segment_cache_max_mb",
    ] {
        assert!(cache.get(key).is_none(), "旧键 {key} 应被删除: {encoded}");
    }
    assert!(cache["segments"].get("global_max").is_none());
    assert!(cache["segments"].get("retain_count").is_none());
    for key in [
        "enabled", "max_disk_mb", "interval_secs", "scan_limit",
        "disk_budget_ratio", "reserve_mb", "min_mb",
    ] {
        assert!(cache["gc"].get(key).is_none(), "旧键 gc.{key} 应被删除");
    }
    assert!(cache["ttl"].get("refresh_ratio").is_none());
    assert!(cache["ttl"].get("fallback_mpd_secs").is_none());
    assert!(encoded["cdn"].get("auto_tune").is_none());
    assert!(encoded["cdn"].get("segment_max_age_secs").is_none());
}

#[test]
fn normalize_strips_legacy_memory_config() {
    let mut config: Config = serde_json::from_value(serde_json::json!({
        "cache": { "backend": "memory", "disk_path": "./cache" }
    }))
    .expect("解析旧内存配置");
    config.normalize();
    let encoded = serde_json::to_value(&config).expect("序列化");
    assert!(encoded["cache"].get("backend").is_none());
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test config::model::tests::cache_defaults_match_disk_only_design config::model::tests::normalize_strips_legacy_cache_keys`

Expected: 编译失败或不通过（旧字段仍存在、`ttl.segment_secs` 默认仍为 300）。

- [ ] **Step 3: 删除旧字段**

`CacheCfg` 删除 `backend`、`m3u8_max_entries`、`segment_max_entries`、`disk_max_mb`、`memory_max_mb`、`segment_cache_max_mb`。

`CacheSegmentsCfg` 删除 `global_max`、`retain_count`。

`CacheGcCfg` 删除 `enabled`、`max_disk_mb`、`interval_secs`、`scan_limit`、`disk_budget_ratio`、`reserve_mb`、`min_mb`。

`CacheTtlCfg` 删除 `refresh_ratio`、`fallback_mpd_secs`；`segment_secs` 默认 `0`。

`CdnCfg` 删除 `auto_tune`、`segment_max_age_secs`。

- [ ] **Step 4: normalize 清理旧键**

在 `Config::normalize` 中调用：

```rust
fn strip_legacy(extra: &mut Extra, keys: &[&str]) {
    for key in keys {
        extra.remove(*key);
    }
}
```

键清单：

- `cache.extra`：`backend`、`m3u8_max_entries`、`segment_max_entries`、`disk_max_mb`、`memory_max_mb`、`segment_cache_max_mb`
- `cache.segments.extra`：`global_max`、`retain_count`
- `cache.gc.extra`：`enabled`、`max_disk_mb`、`interval_secs`、`scan_limit`、`disk_budget_ratio`、`reserve_mb`、`min_mb`
- `cache.ttl.extra`：`refresh_ratio`、`fallback_mpd_secs`
- `cdn.extra`：`auto_tune`、`segment_max_age_secs`

- [ ] **Step 5: 更新剩余测试与 CLI**

- `cache_subsections_round_trip`：删除 `global_max`。
- `cdn_and_gc_defaults_are_inert`：改为断言 `cache.auto_tune`、`cache.limit_mb`、`cache.gc.max_age_secs`。
- `cdn_and_gc_round_trip_through_json`：用 `cache.limit_mb = 200`。
- `src/config/store.rs`：`config.cache.gc.max_disk_mb = 120` 改为 `config.cache.limit_mb = 120`。
- `src/cli.rs`：路径 `cache.gc.max_disk_mb` 改为 `cache.limit_mb`。

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test`

Expected: PASS（前端尚未改，但 Rust 测试应全绿）。

- [ ] **Step 7: 提交**

```bash
git add src/config/model.rs src/config.rs src/config/store.rs src/cli.rs
git commit -m "refactor(config): 删除缓存旧字段并清理旧键"
```

---

### Task 10: 前端设置页与缓存状态页

**Files:**
- Modify: `frontend/src/settings.tsx:88-128`, `:766-773`, `:1070-1370`
- Modify: `frontend/src/lowcfg.tsx`
- Modify: `frontend/src/cache.tsx`
- Modify: `frontend/src/overview.tsx:712`, `:1134`
- Test: `pnpm build`

**Interfaces:**
- Consumes: 新 `recommend` 字段、`cache_stats` 顶层占用字段。
- Produces: `AutoNum`、`ToggleNum` 组件；两张主卡 + 高级折叠。

- [ ] **Step 1: 写 AutoNum 与 ToggleNum**

在 `settings.tsx` 的 `Num` 旁加入：

```tsx
function AutoNum({ label, path, unit, tip, hint, autoValue, fallback, min = 1 }: {
  label: string; path: string; unit?: string; tip?: string; hint?: string;
  autoValue: number | null | undefined; fallback: number; min?: number;
}) {
  const { draft, update } = useDraft();
  const value = asNumber(draft, path);
  const manual = value !== 0;
  const current = autoValue ?? fallback;
  return (
    <div className="field">
      <label>{label}{tip ? <Tip text={tip} /> : null}</label>
      <div className="field-control">
        <Select
          compact
          ariaLabel={`${label}取值方式`}
          value={manual ? 'manual' : 'auto'}
          options={[
            { value: 'auto', label: '自动' },
            { value: 'manual', label: '手动' },
          ]}
          onChange={(next) => update(path, next === 'manual' ? current : 0)}
        />
        <NumberInput
          min={min}
          ariaLabel={label}
          value={manual ? value : current}
          disabled={!manual}
          onChange={(next) => update(path, next)}
        />
        {unit ? <span className="field-unit">{unit}</span> : null}
      </div>
      <div className="hint">{hint ?? (manual ? '手动值，覆盖自动估算' : `自动（当前 ${current}）`)}</div>
    </div>
  );
}

function ToggleNum({ label, path, unit, tip, enabledValue = 1800, min = 60 }: {
  label: string; path: string; unit?: string; tip?: string; enabledValue?: number; min?: number;
}) {
  const { draft, update } = useDraft();
  const value = asNumber(draft, path);
  const on = value !== 0;
  return (
    <div className="field">
      <label>{label}{tip ? <Tip text={tip} /> : null}</label>
      <div className="field-control">
        <Select
          compact
          ariaLabel={`${label}开关`}
          value={on ? 'on' : 'off'}
          options={[{ value: 'on', label: '开启' }, { value: 'off', label: '关闭' }]}
          onChange={(next) => update(path, next === 'on' ? enabledValue : 0)}
        />
        <NumberInput
          min={min}
          ariaLabel={label}
          value={on ? value : enabledValue}
          disabled={!on}
          onChange={(next) => update(path, next)}
        />
        {unit ? <span className="field-unit">{unit}</span> : null}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: 重排缓存卡**

先在 `settings.tsx` 顶部补导入：

```tsx
import { formatBytes } from './format';
import { formatAgo, formatMB } from './lowcfg';
```

“缓存与回收”分区改为：

1. `Card title="缓存容量与回收"`：
   - `Text cache.disk_path`
   - `AutoNum cache.limit_mb label="缓存上限" unit="MB" autoValue={recommend?.disk_cap_mb} fallback={150}`，提示按 `recommend.limited_by` 显示：`working` 播放需要 / `budget` 可用空间 / `ceiling` 总容量 70% / `floor` 下限 / `no_data` 无数据 / `space_unknown` 空间读取失败。
   - `BoolPick cache.auto_tune label="自动估算"`
   - `ToggleNum cache.gc.max_age_secs label="过期删除" unit="秒"`
   - 状态行：`已用 {formatBytes(segment_cache_bytes)} · 索引 {formatBytes(segment_index_bytes)} / {formatMB(segment_index_limit_bytes)} · 上次回收 {formatAgo(gc_last_run_secs_ago)}`
   - 清空按钮保留。
2. `Card title="缓存有效期"`：
   - `Num cache.ttl.sub_url_secs`
   - `AutoNum cache.ttl.segment_secs label="切片有效期" unit="秒" autoValue={recommend?.ttl_secs} fallback={600}`
3. `<details className="block">` summary `高级设置 · 预载、Remux、单频道索引`：
   - `AutoNum cache.segments.per_channel_max label="单频道索引条目上限" autoValue={4096} fallback={4096}`
   - 现有预载卡、Remux 卡。

删除旧控件：`cache.backend`、`cache.disk_max_mb`、`cache.memory_max_mb`、`cache.m3u8_max_entries`、`cache.segment_max_entries`、`cache.segment_cache_max_mb`、`cache.segments.global_max`、`cache.segments.retain_count`、`cache.ttl.refresh_ratio`、`cache.ttl.fallback_mpd_secs`、`cache.gc.enabled`、`cache.gc.max_disk_mb`、`cache.gc.interval_secs`、`cache.gc.scan_limit`、`cache.gc.disk_budget_ratio`、`cache.gc.reserve_mb`、`cache.gc.min_mb`。同时删除 `ttlHint` 与 `capHint` 两个常量。

- [ ] **Step 3: 改 CDN 卡**

删除 `cdn.auto_tune` 与 `cdn.segment_max_age_secs` 两个控件；卡片注释改为「CDN 兜底只控制响应头与空体状态；缓存时长在缓存区设置」。

- [ ] **Step 4: 拆推荐卡**

`lowcfg.tsx`：

- `Recommend` 类型改为：

```ts
export type Recommend = {
  ttl_secs: number;
  ttl_source: 'manual' | 'measured' | 'fallback';
  disk_cap_mb: number;
  cap_source: 'manual' | 'auto';
  limited_by: 'manual' | 'working' | 'budget' | 'ceiling' | 'floor' | 'no_data' | 'space_unknown';
  channels: number;
  working_channels: number;
  working_mb: number | null;
  budget_mb: number | null;
  ceiling_mb: number | null;
  gc_last_run_secs_ago: number | null;
  groups: number;
  updated_secs_ago: number | null;
  disk_total_mb: number | null;
  disk_free_mb: number | null;
  segment_body_bytes: number;
  segment_index_bytes: number;
  segment_index_limit_bytes: number;
  segment_cache_bytes: number;
  segment_body_files: number;
  auto_tune: boolean;
};
```

- `RecommendCard` 只显示 CDN 侧：缓存时长、`ttl_source` 徽标、最近采样、复制按钮、立即估算按钮。
- `formatAgo` 与 `formatMB` 改为 `export function`，供缓存页复用。
- `SOURCE_LABEL` 改名为 `TTL_SOURCE_LABEL`，键为 `manual` / `measured` / `fallback`。

- [ ] **Step 5: 缓存状态页加本机缓存卡**

`cache.tsx`：

- `CacheStats` 删除 `backend`，加 `segment_body_bytes`、`segment_index_bytes`、`segment_cache_bytes`。
- 页头 `sub` 改为 `硬盘 · ${formatBytes(stats.bytes)}`。
- 在 `RecommendCard` 后加 `LocalCacheCard`，显示：
  - 缓存上限、来源（自动/手动）、限制因素中文标签。
  - `活跃 {channels} · 按 {working_channels} 个估算`。
  - 预计需要 `working_mb`、实际占用 `segment_cache_bytes`、正文、索引 / 子预算。
  - 上次回收、自动估算开关、磁盘可用 / 总。
- 删除「内存缓存只保存清单索引」提示。
- `overview.tsx` 删除 `cacheBackend` 常量与「缓存位置」行。

- [ ] **Step 6: 构建**

Run: `cd frontend && pnpm build`

Expected: `tsc --noEmit` 与 `vite build` 均通过。

- [ ] **Step 7: 提交**

```bash
git add frontend/src/settings.tsx frontend/src/lowcfg.tsx frontend/src/cache.tsx frontend/src/overview.tsx
git commit -m "feat(panel): 缓存设置与状态页收敛"
```

---

### Task 11: 文档与验收脚本

**Files:**
- Modify: `docs/design/04-config-and-storage.md:36-45`, `:194-201`, `:312-330`, `:367-383`
- Modify: `docs/design/06-low-spec-optimization.md:19`, `:37`, `:238-254`, `:300-310`, `:360-368`
- Modify: `docs/design/18-low-spec-deployment.md:81-101`, `:226`
- Modify: `docs/design/config-low-spec.json:26-37`
- Modify: `docs/tests/05-low-spec-acceptance.md:136`, `:256`, `:277-298`, `:451`, `:632`, `:651`
- Test: document review; no automated test

- [ ] **Step 1: 更新配置字段表**

`04-config-and-storage.md`：

- `[cache]` 行改为 `disk_path`、`limit_mb`、`auto_tune`、`prefetch`、`segments`、`gc`、`ttl`、`dash`、`remux`、`disguise`。
- `[cache.segments]` 只留 `per_channel_max`。
- `[cache.gc]` 只留 `max_age_secs`。
- `[cache.ttl]` 只留 `sub_url_secs`、`segment_secs`。
- `[cdn]` 删除 `auto_tune` 与 `segment_max_age_secs`。
- §11.5 删除兼容键说明，改为「旧键直接移除，不迁移」。

- [ ] **Step 2: 更新容量与回收说明**

`06-low-spec-optimization.md`：

- 上限公式改为 `clamp(min(工作集, 可用×0.8−100), 50, 总×0.7)`。
- 工作集按活跃频道数，最低 3。
- 新增磁盘索引子预算 10%、进程内索引 600 秒、扫描周期 60 秒。
- `auto_tune=false` 说明改为「停止采样，不冻结上限」。
- 删除 `memory_max_mb`、`segment_cache_max_mb`、`global_max` 的说明。

- [ ] **Step 3: 更新部署文档与示例**

`18-low-spec-deployment.md` 与 `config-low-spec.json`：

```json
"cache": {
  "disk_path": "./cache",
  "limit_mb": 0,
  "auto_tune": true,
  "segments": { "per_channel_max": 0 },
  "gc": { "max_age_secs": 1800 },
  "ttl": { "sub_url_secs": 1800, "segment_secs": 0 }
},
"cdn": {
  "enabled": false
}
```

删除 `max_disk_mb`、`interval_secs`、`auto_tune`（CDN 内）、`memory_max_mb` 等旧键。

- [ ] **Step 4: 更新验收脚本**

`docs/tests/05-low-spec-acceptance.md`：

- `config set cache.gc.max_disk_mb` → `config set cache.limit_mb`。
- `recommend.seg_bytes` → `recommend.segment_body_bytes`。
- `recommend.source` → `recommend.ttl_source`。
- 扫描周期从 `interval_secs` 改为固定 60 秒。
- 删除 `cache.gc.enabled` 检查；改为确认 GC 常开。
- 新增断言：`recommend.channels`、`recommend.working_channels`、`recommend.segment_index_limit_bytes`。

- [ ] **Step 5: 提交**

```bash
git add docs/design/04-config-and-storage.md docs/design/06-low-spec-optimization.md docs/design/18-low-spec-deployment.md docs/design/config-low-spec.json docs/tests/05-low-spec-acceptance.md
git commit -m "docs: 对齐缓存配置与磁盘回收新行为"
```

---

### Task 12: 全量验证与打 tag

**Files:**
- Test: whole repository

**Interfaces:**
- Consumes: 前 11 个任务
- Produces: 可发布的 `v2026.58`

- [ ] **Step 1: 运行格式与静态检查**

```bash
cargo fmt --check
cargo clippy --all-targets
```

Expected: 无错误、无新增 warning。

- [ ] **Step 2: 运行全部测试**

```bash
cargo test
```

Expected: PASS。

- [ ] **Step 3: 运行前端构建**

```bash
cd frontend && pnpm build
```

Expected: PASS。

- [ ] **Step 4: 检查残留字段**

```bash
grep -rn -e 'cache\.backend' -e 'memory_max_mb' -e 'cache\.disk_max_mb' \
  -e 'cache\.gc\.max_disk_mb' -e 'cache\.gc\.enabled' -e 'global_max' \
  -e 'segment_cache_max_mb' -e 'cdn\.segment_max_age_secs' \
  src/ frontend/src/ docs/design/
```

Expected: 只允许出现在旧键清理清单、测试的旧配置样例与设计文档的删除清单中；不得出现在运行路径。

- [ ] **Step 5: 提交收尾**

```bash
git add -A
git commit -m "chore: 缓存配置收敛收尾"
```

- [ ] **Step 6: 打 tag 并推送**

```bash
git tag v2026.58
git push origin main --tags
```

Expected: 远端出现 `v2026.58`。若该 tag 已被占用，顺延到下一个未占用的 `v2026.N`。
