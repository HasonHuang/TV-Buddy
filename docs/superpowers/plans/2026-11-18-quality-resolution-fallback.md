# Quality Resolution Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fallback speed for unknown or unmapped resolutions, rebuild the quality rules page, and add ffprobe-based resolution detection with reliable installer and rule-revalidation support.

**Architecture:** Extend `QualityRules` and `EffectiveRules` with `fallback_speed_mib`. Add a `ProbeSignature` to the stored rule hash and make stale hashes due. Capture the first sampled HLS segment bytes and pipe them to `ffprobe -f mpegts` on stdin. Extend the panel installer to place both `ffmpeg` and `ffprobe`. Rebuild the rules tab around a container-query grid and a full-width resolution table.

**Tech Stack:** Rust 2021, axum, tokio, reqwest, rusqlite, serde_json; React 18, TypeScript, Vite.

**Spec:** `docs/superpowers/specs/2026-11-18-quality-resolution-fallback-design.md`

## Global Constraints

- `fallback_speed_mib` default is `0.0`. Zero means “use `min_speed_mib`”.
- `ffprobe` default is `false`. Quality results must be unchanged when it is off.
- `ffprobe_timeout_secs`: 1–30. `probe_concurrency`: 1–4. `ffprobe_sample_kb`: 64–2048.
- `max_ffprobe_per_run`: 0 means unlimited. Count attempts, not successes.
- No new database columns. Resolution source is debug-log only.
- No new crates. Use `tokio::process`, `serde_json`, `sha2`, `flate2`.
- `ffprobe` reads captured bytes from stdin. Do not pass the playlist URL to ffprobe.
- Manual quality runs keep probing every channel in scope. Hash revalidation applies to periodic runs.
- Rule hash prefix is `tvb-quality-rules-v2`.
- Panel install is staged: both artifacts download and verify before either is renamed into place.
- Current tag baseline is `v2026.50`. Final tag is `v2026.51`.

## Review Focus

- **HLS segment URL without a standard extension:** ffprobe’s HLS demuxer rejects it. Captured bytes on stdin must still return resolution. Task 4 and Task 7 test this.
- **fMP4 HLS with `EXT-X-MAP`:** captured media bytes lack the init segment. Resolution stays unknown; the channel must keep its speed verdict and use the fallback. Task 4 and Task 7 test this.
- **ffprobe missing or PATH-only:** `state.ffprobe()` returns a relative `ffprobe`. Availability must come from `ffprobe -version`, not `Path::exists`. Task 2 and Task 3 test this.
- **Rule change while the channel is not due:** periodic runs must treat a hash mismatch as due; manual runs must still probe all. Task 2 tests this.
- **Concurrent probes:** the per-operation cap must not race, and captured buffers must stay within `probe_concurrency * ffprobe_sample_kb`. Task 5 tests this.

---

## Task 1: Fallback speed configuration and evaluation

**Files:**
- Modify: `src/config/model.rs`
- Modify: `src/config.rs`
- Modify: `src/quality/rules.rs`
- Modify: `src/quality/admission.rs`
- Modify: `src/quality/schedule.rs`

**Interfaces:**
- Consumes: existing `Config`, `QualityRules`, `QualityOverride`, `EffectiveRules`, `Measurement`, `evaluate`, `effective_rules`, `required_probe_mode`.
- Produces:
  - `QualityRules.fallback_speed_mib: f64`
  - `QualityOverride.fallback_speed_mib: Option<f64>`
  - `EffectiveRules.fallback_speed_mib: f64`
  - `QualityCfg.ffprobe: bool`, `ffprobe_timeout_secs: u64`, `probe_concurrency: u32`, `max_ffprobe_per_run: u64`, `ffprobe_min_speed_mib: f64`, `ffprobe_sample_kb: u64`

- [ ] **Step 1: Write the failing fallback tests**

Add to `src/quality/rules.rs` tests:

```rust
#[test]
fn fallback_speed_applies_when_resolution_unknown() {
    let rules = EffectiveRules {
        min_speed_mib: 1.0,
        fallback_speed_mib: 0.8,
        ..Default::default()
    };
    let measurement = Measurement {
        reachable: true,
        speed_mib: Some(0.9),
        width: None,
        height: None,
        ..Default::default()
    };
    assert!(evaluate(&rules, &measurement).ok);
}

#[test]
fn fallback_speed_applies_when_map_has_no_match() {
    let mut rules = EffectiveRules {
        min_speed_mib: 1.0,
        fallback_speed_mib: 0.8,
        ..Default::default()
    };
    rules.resolution_speed_map.insert(2560 * 1440, 0.5);
    let measurement = Measurement {
        reachable: true,
        speed_mib: Some(0.9),
        width: Some(1920),
        height: Some(1080),
        ..Default::default()
    };
    assert!(evaluate(&rules, &measurement).ok, "表无 <= px 的档时用兜底");
    rules.fallback_speed_mib = 0.0;
    assert_eq!(
        evaluate(&rules, &measurement).reason,
        Reason::SpeedLow,
        "兜底为 0 时回退到 min_speed，证明上一条不是旧行为"
    );
}

#[test]
fn fallback_zero_uses_min_speed() {
    let rules = EffectiveRules {
        min_speed_mib: 1.0,
        fallback_speed_mib: 0.0,
        ..Default::default()
    };
    let measurement = Measurement {
        reachable: true,
        speed_mib: Some(0.9),
        width: None,
        height: None,
        ..Default::default()
    };
    assert_eq!(evaluate(&rules, &measurement).reason, Reason::SpeedLow);
}

#[test]
fn fallback_does_not_bypass_known_resolution_limits() {
    let rules = EffectiveRules {
        fallback_speed_mib: 2.0,
        min_resolution_px: Some(1920 * 1080),
        ..Default::default()
    };
    let measurement = Measurement {
        reachable: true,
        speed_mib: Some(3.0),
        width: Some(1280),
        height: Some(720),
        ..Default::default()
    };
    assert_eq!(evaluate(&rules, &measurement).reason, Reason::ResolutionLow);
}
```

Add to `src/quality/schedule.rs` tests:

```rust
#[test]
fn fallback_speed_promotes_probe_mode() {
    let config = Config::default();
    let rules = EffectiveRules {
        fallback_speed_mib: 0.8,
        ..Default::default()
    };
    assert_eq!(
        required_probe_mode(&config, &rules),
        ProbeMode::Speed,
        "只设置兜底速率也必须测速"
    );
}
```

Add to `src/config.rs` tests:

```rust
#[test]
fn rejects_invalid_quality_ffprobe_fields() {
    let mut config = Config::default();
    config.quality.probe_concurrency = 0;
    assert!(validate(&config).is_err());
    config.quality.probe_concurrency = 1;
    config.quality.ffprobe_sample_kb = 4096;
    assert!(validate(&config).is_err());
    config.quality.ffprobe_sample_kb = 512;
    config.quality.ffprobe_timeout_secs = 0;
    assert!(validate(&config).is_err());
    config.quality.ffprobe_timeout_secs = 5;
    config.quality.rules.fallback_speed_mib = -0.1;
    assert!(validate(&config).is_err());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test quality::rules::tests::fallback -- --nocapture`
Expected: compile errors for missing `fallback_speed_mib`.

Run: `cargo test quality::schedule::tests::fallback_speed_promotes_probe_mode -- --nocapture`
Expected: compile error for missing field.

- [ ] **Step 3: Add the config fields and defaults**

In `src/config/model.rs`, add to `QualityCfg`:

```rust
pub ffprobe: bool,
pub ffprobe_timeout_secs: u64,
pub probe_concurrency: u32,
pub max_ffprobe_per_run: u64,
pub ffprobe_min_speed_mib: f64,
pub ffprobe_sample_kb: u64,
```

Defaults in `impl Default for QualityCfg`:

```rust
ffprobe: false,
ffprobe_timeout_secs: 5,
probe_concurrency: 1,
max_ffprobe_per_run: 0,
ffprobe_min_speed_mib: 0.0,
ffprobe_sample_kb: 512,
```

Add `pub fallback_speed_mib: f64` to `QualityRules`; default `0.0`.
Add `pub fallback_speed_mib: Option<f64>` to `QualityOverride`; `#[derive(Default)]` covers it.

- [ ] **Step 4: Add validation**

In `src/config.rs` `validate_quality`, add:

```rust
if !(1..=30).contains(&quality.ffprobe_timeout_secs) {
    bail!("quality.ffprobe_timeout_secs 必须在 1–30 之间");
}
if !(1..=4).contains(&quality.probe_concurrency) {
    bail!("quality.probe_concurrency 必须在 1–4 之间");
}
if !(64..=2048).contains(&quality.ffprobe_sample_kb) {
    bail!("quality.ffprobe_sample_kb 必须在 64–2048 之间");
}
if !quality.ffprobe_min_speed_mib.is_finite() || quality.ffprobe_min_speed_mib < 0.0 {
    bail!("quality.ffprobe_min_speed_mib 必须是非负数");
}
```

In `validate_quality_rules`, add:

```rust
if !rules.fallback_speed_mib.is_finite() || rules.fallback_speed_mib < 0.0 {
    bail!("{label}.fallback_speed_mib 必须是非负数");
}
```

In `validate_quality_override`, add:

```rust
if let Some(speed) = quality.fallback_speed_mib {
    if !speed.is_finite() || speed < 0.0 {
        bail!("{label}.fallback_speed_mib 必须是非负数");
    }
}
```

- [ ] **Step 5: Add the effective rule and evaluation**

In `src/quality/rules.rs`:

```rust
pub struct EffectiveRules {
    // existing fields
    pub fallback_speed_mib: f64,
    // existing fields
}
```

Default: `fallback_speed_mib: 0.0`.

In `effective_rules`, after `min_speed_mib`:

```rust
fallback_speed_mib: pick(channel_q, group_q, source_q, |quality| {
    quality.fallback_speed_mib
})
.unwrap_or(rules.fallback_speed_mib),
```

In `evaluate`, replace the threshold block with:

```rust
let fallback = if rules.fallback_speed_mib > 0.0 {
    rules.fallback_speed_mib
} else {
    rules.min_speed_mib
};
let threshold = match (measurement.width, measurement.height) {
    (Some(width), Some(height)) => {
        let px = u32::from(width) * u32::from(height);
        rules
            .resolution_speed_map
            .iter()
            .rev()
            .find(|(key, _)| **key <= px)
            .map(|(_, speed)| *speed)
            .unwrap_or(fallback)
    }
    _ => fallback,
};
```

- [ ] **Step 6: Add the override and probe promotion**

In `src/quality/admission.rs` `has_override`, add:

```rust
|| value.fallback_speed_mib.is_some()
```

Add a test:

```rust
#[test]
fn fallback_override_counts_as_override() {
    let value = QualityOverride {
        fallback_speed_mib: Some(0.8),
        ..Default::default()
    };
    assert!(has_override(&value));
}
```

In `src/quality/schedule.rs` `required_probe_mode`, add:

```rust
if rules.fallback_speed_mib > 0.0 {
    mode = mode_at_least(mode, ProbeMode::Speed);
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test quality::rules::tests::fallback -- --nocapture`
Expected: PASS.

Run: `cargo test quality::schedule::tests::fallback_speed_promotes_probe_mode -- --nocapture`
Expected: PASS.

Run: `cargo test config::tests::rejects_invalid_quality_ffprobe_fields -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Run the full Rust suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/config/model.rs src/config.rs src/quality/rules.rs src/quality/admission.rs src/quality/schedule.rs
git commit -m "feat(quality): add fallback speed and ffprobe config"
```

---

## Task 2: Probe signature, rule hash revalidation, and due targets

**Files:**
- Modify: `src/quality/admission.rs`
- Modify: `src/state.rs`
- Modify: `src/quality/engine.rs`
- Modify: `src/quality/gate.rs`
- Modify: `src/subscription.rs`
- Modify: `src/http/routes/quality.rs`

**Interfaces:**
- Consumes: `EffectiveRules`, `QualityResult.rule_hash`, `AppState::ffprobe`.
- Produces:
  - `pub struct ProbeSignature`
  - `ProbeSignature::from_config(config: &Config, ffprobe_ready: bool) -> ProbeSignature`
  - `pub fn rule_hash(rules: &EffectiveRules, probe: &ProbeSignature) -> String`
  - `AppState::ffprobe_ready(&self) -> bool` (async)
  - `AppState::invalidate_ffprobe_ready(&self)` (async)
  - `due_targets(..., probe: &ProbeSignature)`

- [ ] **Step 1: Write the failing hash tests**

Add to `src/quality/admission.rs` tests:

```rust
fn signature(config: &Config) -> ProbeSignature {
    ProbeSignature::from_config(config, false)
}

#[test]
fn rule_hash_changes_with_fallback_speed() {
    let config = Config::default();
    let probe = signature(&config);
    let a = EffectiveRules {
        fallback_speed_mib: 0.5,
        ..Default::default()
    };
    let b = EffectiveRules {
        fallback_speed_mib: 0.8,
        ..Default::default()
    };
    assert_ne!(rule_hash(&a, &probe), rule_hash(&b, &probe));
}

#[test]
fn rule_hash_changes_when_ffprobe_ready_changes() {
    let mut config = Config::default();
    config.quality.ffprobe = true;
    let a = ProbeSignature::from_config(&config, false);
    let b = ProbeSignature::from_config(&config, true);
    assert_ne!(rule_hash(&EffectiveRules::default(), &a), rule_hash(&EffectiveRules::default(), &b));
}

#[test]
fn rule_hash_ignores_readiness_when_ffprobe_disabled() {
    let config = Config::default();
    assert!(!config.quality.ffprobe);
    let a = ProbeSignature::from_config(&config, false);
    let b = ProbeSignature::from_config(&config, true);
    assert_eq!(rule_hash(&EffectiveRules::default(), &a), rule_hash(&EffectiveRules::default(), &b));
}
```

Add to `src/quality/engine.rs` tests:

```rust
#[test]
fn stale_rule_hash_makes_channel_due() {
    let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
    let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
    store
        .upsert(&QualityResult {
            key: "sync:s1:c".into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            source_key: "c".into(),
            gid: "g".into(),
            cid: "c".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            action: "admitted".into(),
            next_check_at: 9_999_999_999,
            rule_hash: "stale".into(),
            ..QualityResult::default()
        })
        .expect("写结果");
    let config = config_with_sync_channel();
    let probe = ProbeSignature::from_config(&config, false);
    let due = due_targets(&config, &store, &RunScope::All, 100, 0, false, false, &probe)
        .expect("到期目标");
    assert_eq!(due.len(), 1, "哈希不一致必须到期");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn matching_rule_hash_keeps_channel_not_due() {
    let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
    let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
    let config = config_with_sync_channel();
    let probe = ProbeSignature::from_config(&config, false);
    let rules = effective_rules(&config, &config.groups[0], &config.groups[0].channels[0]);
    store
        .upsert(&QualityResult {
            key: "sync:s1:c".into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            source_key: "c".into(),
            gid: "g".into(),
            cid: "c".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            action: "admitted".into(),
            next_check_at: 9_999_999_999,
            rule_hash: rule_hash(&rules, &probe),
            ..QualityResult::default()
        })
        .expect("写结果");
    let due = due_targets(&config, &store, &RunScope::All, 100, 0, false, false, &probe)
        .expect("到期目标");
    assert!(due.is_empty(), "哈希一致且未到期时跳过");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn manual_run_ignores_rule_hash() {
    let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
    let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
    store
        .upsert(&QualityResult {
            key: "sync:s1:c".into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            source_key: "c".into(),
            gid: "g".into(),
            cid: "c".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            action: "admitted".into(),
            next_check_at: 9_999_999_999,
            rule_hash: "stale".into(),
            ..QualityResult::default()
        })
        .expect("写结果");
    let config = config_with_sync_channel();
    let probe = ProbeSignature::from_config(&config, false);
    let due = due_targets(&config, &store, &RunScope::All, 100, 0, false, true, &probe)
        .expect("手动目标");
    assert_eq!(due.len(), 1, "手动运行忽略到期与哈希");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn empty_rule_hash_is_stale_once() {
    let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
    let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
    store
        .upsert(&QualityResult {
            key: "sync:s1:c".into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            source_key: "c".into(),
            gid: "g".into(),
            cid: "c".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            action: "admitted".into(),
            next_check_at: 9_999_999_999,
            rule_hash: String::new(),
            ..QualityResult::default()
        })
        .expect("写结果");
    let config = config_with_sync_channel();
    let probe = ProbeSignature::from_config(&config, false);
    let due = due_targets(&config, &store, &RunScope::All, 100, 0, false, false, &probe)
        .expect("到期目标");
    assert_eq!(due.len(), 1, "空哈希必须到期一次");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn max_channels_per_run_leaves_remainder_due() {
    let dir = std::env::temp_dir().join(format!("tvb-engine-{}", uuid::Uuid::new_v4()));
    let store = QualityStore::new(crate::db::connect(&dir.join("tvb.db")).expect("db"));
    let mut config = config_with_sync_channel();
    config.groups[0].channels.push(Channel {
        id: "c2".into(),
        url: "http://up/live2.m3u8".into(),
        source_id: "s1".into(),
        source_key: "c2".into(),
        ..Channel::default()
    });
    let probe = ProbeSignature::from_config(&config, false);
    let first = due_targets(&config, &store, &RunScope::All, 100, 1, false, false, &probe)
        .expect("第一轮");
    assert_eq!(first.len(), 1, "单轮上限截断");
    let rules = effective_rules(&config, &config.groups[0], &config.groups[0].channels[0]);
    store
        .upsert(&QualityResult {
            key: "sync:s1:c".into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            source_key: "c".into(),
            gid: "g".into(),
            cid: "c".into(),
            url_hash: url_hash_hex("http://up/live.m3u8"),
            action: "admitted".into(),
            next_check_at: 9_999_999_999,
            rule_hash: rule_hash(&rules, &probe),
            ..QualityResult::default()
        })
        .expect("写结果");
    let second = due_targets(&config, &store, &RunScope::All, 100, 1, false, false, &probe)
        .expect("第二轮");
    assert_eq!(second.len(), 1, "剩余频道下一轮仍到期");
    assert_eq!(second[0].cid, "c2");
    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test quality::admission::tests::rule_hash_changes -- --nocapture`
Expected: compile error for missing `ProbeSignature`.

Run: `cargo test quality::engine::tests::stale_rule_hash_makes_channel_due -- --nocapture`
Expected: compile error for missing parameter.

- [ ] **Step 3: Add `ProbeSignature` and the v2 hash**

In `src/quality/admission.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSignature {
    pub probe_mode: String,
    pub ffprobe: bool,
    pub ffprobe_ready: bool,
    pub ffprobe_sample_kb: u64,
    pub ffprobe_min_speed_mib_bits: u64,
    pub sample_segments: u32,
    pub sample_bytes_kb: u64,
    pub sample_secs_bits: u64,
}

impl ProbeSignature {
    pub fn from_config(config: &Config, ffprobe_ready: bool) -> Self {
        Self {
            probe_mode: config.quality.probe_mode.clone(),
            ffprobe: config.quality.ffprobe,
            ffprobe_ready: config.quality.ffprobe && ffprobe_ready,
            ffprobe_sample_kb: config.quality.ffprobe_sample_kb,
            ffprobe_min_speed_mib_bits: config.quality.ffprobe_min_speed_mib.to_bits(),
            sample_segments: config.quality.sample_segments,
            sample_bytes_kb: config.quality.sample_bytes_kb,
            sample_secs_bits: config.quality.sample_secs.to_bits(),
        }
    }
}

pub fn rule_hash(rules: &EffectiveRules, probe: &ProbeSignature) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"tvb-quality-rules-v2\0");
    // existing rule fields
    hasher.update(rules.fallback_speed_mib.to_bits().to_le_bytes());
    // existing host_cache / stale_keep_secs
    hasher.update(probe.probe_mode.as_bytes());
    hasher.update([u8::from(probe.ffprobe), u8::from(probe.ffprobe_ready)]);
    hasher.update(probe.ffprobe_sample_kb.to_le_bytes());
    hasher.update(probe.ffprobe_min_speed_mib_bits.to_le_bytes());
    hasher.update(probe.sample_segments.to_le_bytes());
    hasher.update(probe.sample_bytes_kb.to_le_bytes());
    hasher.update(probe.sample_secs_bits.to_le_bytes());
    hex::encode(hasher.finalize())
}
```

Keep the existing hash inputs in their current order. Only add the new fields and change the prefix.

- [ ] **Step 4: Add the ffprobe readiness cache**

In `src/state.rs` `AppState`:

```rust
pub ffprobe_ready: tokio::sync::RwLock<Option<bool>>,
```

Initialize:

```rust
ffprobe_ready: tokio::sync::RwLock::new(None),
```

Add methods:

```rust
fn ffprobe_version_ok(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).contains("ffprobe version")
}

pub async fn ffprobe_ready(&self) -> bool {
    if let Some(value) = *self.ffprobe_ready.read().await {
        return value;
    }
    let mut guard = self.ffprobe_ready.write().await;
    if let Some(value) = *guard {
        return value;
    }
    let output = tokio::process::Command::new(self.ffprobe())
        .arg("-version")
        .output()
        .await;
    let value = output
        .map(|output| output.status.success() && ffprobe_version_ok(&output.stdout))
        .unwrap_or(false);
    *guard = Some(value);
    value
}

pub async fn invalidate_ffprobe_ready(&self) {
    *self.ffprobe_ready.write().await = None;
}
```

Add a unit test:

```rust
#[test]
fn ffprobe_version_marker_is_required() {
    assert!(ffprobe_version_ok(b"ffprobe version 6.1"));
    assert!(!ffprobe_version_ok(b"ffmpeg version 6.1"));
}
```

- [ ] **Step 5: Thread `ProbeSignature` through call sites**

Update these functions to take `probe: &ProbeSignature` and pass it to `rule_hash`:

- `src/quality/gate.rs`: `decide`, `paused_outcome`, `result_for`.
- `src/quality/engine.rs`: `decide`, `result_for`.
- `src/quality/engine.rs` `run`: compute once with `state.ffprobe_ready().await`; pass to `due_targets` and `decide`.
- `src/quality/gate.rs` `filter_plan`: compute once with `state.ffprobe_ready().await`; pass to `decide` and `paused_outcome`.
- `src/quality/engine.rs` `due_targets`: add `probe: &ProbeSignature`; compute `current_hash` for each candidate.

In `due_targets`, replace the guard with:

```rust
let current_hash = crate::quality::admission::rule_hash(&rules, probe);
let next_due = previous.as_ref().map_or(true, |result| result.next_check_at <= now);
let stale_hash = previous.as_ref().is_some_and(|result| {
    result.rule_hash.is_empty() || result.rule_hash != current_hash
});
if !include_not_due && !next_due && !stale_hash {
    continue;
}
```

- [ ] **Step 6: Expose fallback and ffprobe status**

In `src/http/routes/quality.rs`:

- `effective`: add `"fallback_speed_mib": rules.fallback_speed_mib` to the `rules` object.
- `status`: add:

```rust
"ffprobe": {
    "installed": state.ffprobe_ready().await,
    "enabled": config.quality.ffprobe,
    "ready": config.quality.ffprobe && state.ffprobe_ready().await,
    "note": if state.ffprobe_ready().await {
        ""
    } else {
        "未安装 ffprobe；分辨率识别不可用"
    },
}
```

Call `state.ffprobe_ready().await` once and store it in a local variable.

- [ ] **Step 7: Update tests and call sites for the new argument**

All `due_targets` tests add `&ProbeSignature::from_config(&config, false)`.
All `rule_hash` tests add the signature.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test quality::admission::tests::rule_hash -- --nocapture`
Expected: PASS.

Run: `cargo test quality::engine::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 9: Run the full suite and commit**

Run: `cargo test`
Expected: all tests pass.

```bash
git add src/quality/admission.rs src/state.rs src/quality/engine.rs src/quality/gate.rs src/subscription.rs src/http/routes/quality.rs
git commit -m "feat(quality): revalidate stale rule hashes"
```

---

## Task 3: Installer installs ffmpeg and ffprobe

**Files:**
- Modify: `src/tools/install.rs`
- Modify: `src/http/routes/media.rs`
- Modify: `frontend/src/ffmpeg.tsx`
- Modify: `tests/http_contract.rs`

**Interfaces:**
- Consumes: existing `Installer`, `JobSnapshot`, `ffmpeg_status`, `state.ffprobe_ready`.
- Produces:
  - `Artifact { name, file, bytes, sha256 }`
  - `artifacts_for(os, arch) -> Option<Vec<Artifact>>`
  - `remove_managed` removes both.
  - `ffmpeg-status` adds `ffprobe_installed`, `ffprobe_version`, `ffprobe_source`, `ffprobe_path`, `ffprobe_managed_installed`.

- [ ] **Step 1: Write the failing artifact tests**

Replace `artifact_table_covers_four_platforms` with:

```rust
#[test]
fn artifact_table_includes_ffprobe_for_all_platforms() {
    for (os, arch) in [
        ("linux", "x86_64"),
        ("linux", "aarch64"),
        ("macos", "x86_64"),
        ("macos", "aarch64"),
    ] {
        let items = artifacts_for(os, arch).expect("平台");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "ffprobe");
        assert_eq!(items[1].name, "ffmpeg");
    }
    assert!(artifacts_for("windows", "x86_64").is_none());
}
```

Add:

```rust
#[test]
fn remove_managed_removes_both_binaries() {
    let dir = temp_dir();
    let tools = crate::tools::dir(&dir);
    std::fs::create_dir_all(&tools).expect("建 tools");
    std::fs::write(tools.join("ffmpeg"), b"ffmpeg").expect("写 ffmpeg");
    std::fs::write(tools.join("ffprobe"), b"ffprobe").expect("写 ffprobe");
    assert!(remove_managed(&dir).expect("删除"));
    assert!(!tools.join("ffmpeg").exists());
    assert!(!tools.join("ffprobe").exists());
    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test tools::install::tests::artifact_table_includes_ffprobe -- --nocapture`
Expected: compile error for missing `artifacts_for`.

- [ ] **Step 3: Refactor the artifact model**

In `src/tools/install.rs`:

```rust
pub struct Artifact {
    pub name: String,
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}

/// 现有 ffmpeg 制品表。只增加 `name` 字段。
fn ffmpeg_artifact_for(os: &str, arch: &str) -> Option<Artifact> {
    // 复制现有 artifact_for 的 4 个平台值。
    // 返回 Artifact { name: "ffmpeg".into(), .. }。
}

fn ffprobe_artifact_for(os: &str, arch: &str) -> Option<Artifact> {
    let (file, bytes, sha256) = match (os, arch) {
        ("linux", "x86_64") => (
            "ffprobe-linux-x64.gz",
            29_276_839,
            "25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067",
        ),
        ("linux", "aarch64") => (
            "ffprobe-linux-arm64.gz",
            25_493_573,
            "2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8",
        ),
        ("macos", "x86_64") => (
            "ffprobe-darwin-x64.gz",
            25_239_438,
            "d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186",
        ),
        ("macos", "aarch64") => (
            "ffprobe-darwin-arm64.gz",
            19_207_077,
            "d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be",
        ),
        _ => return None,
    };
    Some(Artifact {
        name: "ffprobe".into(),
        file: file.into(),
        bytes,
        sha256: sha256.into(),
    })
}

pub fn artifacts_for(os: &str, arch: &str) -> Option<Vec<Artifact>> {
    Some(vec![
        ffprobe_artifact_for(os, arch)?,
        ffmpeg_artifact_for(os, arch)?,
    ])
}
```

- [ ] **Step 4: Stage both artifacts**

Change `install` to accept `artifacts: &[Artifact]`.

Download and verify each artifact to `<name>.part`. Unpack each to `<name>.unpack`. Keep the 200 MB limit per artifact. After both succeed:

1. Run `<name>.unpack -version`.
2. Check stdout contains `format!("{} version", artifact.name)`.
3. Rename `<name>.unpack` to `<name>` with mode `0755`.

Progress callback uses cumulative `downloaded` and total sum. On any failure, remove all `.part` and `.unpack` files for both artifacts.

- [ ] **Step 5: Update `Installer`**

- Field `#[cfg(test)] artifact: Option<Artifact>` becomes `artifacts: Option<Vec<Artifact>>`.
- `selected_artifact` becomes `selected_artifacts() -> Option<Vec<Artifact>>`.
- `start` passes `&artifacts`.
- Make the test fixture `fixture_artifact(name: &str, version: &str) -> (Artifact, Vec<u8>)`; the script prints `"<name> version <version>"`.
- Tests use two fixture artifacts.
- `remove_managed` loops over `["ffprobe", "ffmpeg"]` and removes `.part`/`.unpack`.
- After a successful install or remove, call `state.invalidate_ffprobe_ready().await` from the route layer, not the installer.

- [ ] **Step 6: Update `ffmpeg-status`**

In `src/http/routes/media.rs`, probe both binaries:

```rust
let ffprobe_managed_file = crate::tools::managed_file(&data_dir, "ffprobe");
let ffprobe_managed_version = match &ffprobe_managed_file {
    Some(path) => probe_version(path).await,
    None => None,
};
let ffprobe_system_version = probe_version(FsPath::new("ffprobe")).await;
let ffprobe_resolved = state.ffprobe();
let ffprobe_installed = if ffprobe_managed_file.is_some() {
    ffprobe_managed_version.is_some()
} else {
    ffprobe_system_version.is_some()
};
let ffprobe_version = if ffprobe_managed_file.is_some() {
    ffprobe_managed_version
} else {
    ffprobe_system_version.clone()
};
let ffprobe_source = if ffprobe_managed_file.is_some() {
    Some("managed")
} else if ffprobe_system_version.is_some() {
    Some("system")
} else {
    None
};
```

Add JSON fields: `ffprobe_installed`, `ffprobe_version`, `ffprobe_source`, `ffprobe_path`, `ffprobe_managed_installed`.

In `ffmpeg_install`, after `start` succeeds, spawn a watcher:

```rust
let watch = state.clone();
tokio::spawn(async move {
    loop {
        if let Some(job) = watch.ffmpeg_install.job() {
            if job.phase == "done" || job.phase == "failed" {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    watch.invalidate_ffprobe_ready().await;
});
```

In `ffmpeg_remove`, call `state.invalidate_ffprobe_ready().await` after a successful removal.

`ffmpeg_status` may also invalidate when it observes `job.phase == "done"`. That is a backup; the watcher is the primary path.

- [ ] **Step 7: Update the frontend card**

In `frontend/src/ffmpeg.tsx`:

- Extend `FfmpegStatus` with the new fields.
- Show two status rows: `ffmpeg` and `ffprobe`.
- Install button label: `安装 ffmpeg / ffprobe`.
- Delete confirm text: both binaries.
- Keep progress based on the combined job.

Run: `cd frontend && pnpm build`
Expected: PASS.

- [ ] **Step 8: Run Rust tests**

Add these tests before running:

1. In `src/tools/install.rs` tests, one install test collects progress callbacks into a `Mutex<Vec<(String, u64, Option<u64>)>>`. Assert: every `total` is the sum of both fixture artifact sizes; `downloaded` never decreases; the final `downloaded` equals `total`; observed phases include downloading, verifying, and installing.
2. In `tests/http_contract.rs`, add an ffprobe status test in the style of `ffmpeg_status_does_not_fall_back_from_broken_managed_binary` and `ffmpeg_status_reports_non_executable_managed_file`. Put an executable `ffprobe` script into the managed tools directory and assert `ffprobe_installed: true`, `ffprobe_version` from `-version`, `ffprobe_source: "managed"`, `ffprobe_path` ends with `ffprobe`, `ffprobe_managed_installed: true`. Put a non-executable file at the same path and assert `ffprobe_installed: false` with `ffprobe_managed_installed: true` (the field is file presence). Do not add a second unsupported PATH-only fixture; the `-version` behavior of the helper is exercised by the managed case.
3. Extract a shared `binary_status` helper so the ffmpeg and ffprobe status blocks are not duplicated. The helper takes the managed file (already resolved with the caller's own semantics; ffmpeg requires the executable bit, ffprobe uses file presence) and the system binary name, and returns `(installed, version, source)`. Both callers keep their existing field values.

Run: `cargo test tools::install -- --nocapture`
Expected: PASS.

Run: `cargo test http::routes::media -- --nocapture`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/tools/install.rs src/http/routes/media.rs frontend/src/ffmpeg.tsx tests/http_contract.rs
git commit -m "feat(tools): install ffmpeg and ffprobe together"
```

---

## Task 4: Full probe mode, segment capture, and resolution detection

**Files:**
- Modify: `src/quality/probe.rs`
- Modify: `src/quality/schedule.rs`
- Modify: `src/quality/gate.rs` (temporary `ProbeGates::unlimited` call site)

**Interfaces:**
- Consumes: `Artifact` no; `ProbeLimits`, `Measurement`, ffprobe config fields.
- Produces:
  - `ProbeMode::Full`
  - `ProbeLimits` fields: `ffprobe`, `ffprobe_ready`, `ffprobe_path`, `ffprobe_timeout`, `ffprobe_sample_bytes`, `ffprobe_min_speed_mib`
  - `ProbeGates { probe_semaphore: Arc<Semaphore>, ffprobe_counter: Arc<AtomicU64>, max_ffprobe_per_run: u64 }`
  - `ProbeGates::unlimited()`
  - `fn is_mpegts(bytes: &[u8]) -> bool`
  - `fn parse_ffprobe_json(stdout: &[u8]) -> Option<(u16, u16)>`
  - `fn reserve_ffprobe_attempt(counter: &AtomicU64, max: u64) -> bool`
  - `async fn detect_resolution(path: &Path, bytes: &[u8], timeout: Duration) -> Result<(u16, u16), String>`

- [ ] **Step 1: Write the failing detector tests**

Add to `src/quality/probe.rs` tests:

```rust
#[test]
fn is_mpegts_detects_sync_bytes() {
    let mut bytes = vec![0u8; 400];
    bytes[0] = 0x47;
    bytes[188] = 0x47;
    bytes[376] = 0x47;
    assert!(is_mpegts(&bytes));
    bytes[188] = 0;
    assert!(!is_mpegts(&bytes));
}

#[test]
fn parse_ffprobe_json_picks_first_video() {
    let json = br#"{"streams":[{"codec_type":"audio"},{"codec_type":"video","width":1920,"height":1080}]}"#;
    assert_eq!(parse_ffprobe_json(json), Some((1920, 1080)));
    assert_eq!(parse_ffprobe_json(br#"{"streams":[]}"#), None);
    assert_eq!(parse_ffprobe_json(b"not json"), None);
}

#[test]
fn ffprobe_cap_counts_attempts() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let counter = AtomicU64::new(0);
    assert!(reserve_ffprobe_attempt(&counter, 1));
    assert!(!reserve_ffprobe_attempt(&counter, 1));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert!(reserve_ffprobe_attempt(&counter, 0), "0 表示不限制");
}

#[test]
fn probe_semaphore_bounds_capture_permits() {
    use std::sync::Arc;
    let gates = ProbeGates {
        probe_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
        ffprobe_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        max_ffprobe_per_run: 0,
    };
    let first = gates
        .probe_semaphore
        .clone()
        .try_acquire_owned()
        .expect("第一个许可");
    assert!(gates.probe_semaphore.clone().try_acquire_owned().is_err());
    drop(first);
    assert!(gates.probe_semaphore.clone().try_acquire_owned().is_ok());
}
```

Add a temp-script test:

```rust
#[tokio::test]
async fn detect_resolution_reads_stdin() {
    let dir = std::env::temp_dir().join(format!("tvb-ffprobe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("建目录");
    let script = dir.join("ffprobe");
    std::fs::write(
        &script,
        b"#!/bin/sh\ncat > /dev/null\nprintf '{\"streams\":[{\"codec_type\":\"video\",\"width\":1280,\"height\":720}]}'\n",
    )
    .expect("写脚本");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("权限");

    let result = detect_resolution(&script, &[0x47; 400], Duration::from_secs(2)).await;
    assert_eq!(result, Ok((1280, 720)));
    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test quality::probe::tests::is_mpegts -- --nocapture`
Expected: compile error for missing function.

- [ ] **Step 3: Add `ProbeMode::Full`, limits, and gates**

In `src/quality/probe.rs`:

```rust
pub enum ProbeMode {
    Reachability,
    Playlist,
    Speed,
    Full,
}
```

Add to `ProbeLimits`:

```rust
pub ffprobe: bool,
pub ffprobe_ready: bool,
pub ffprobe_path: std::path::PathBuf,
pub ffprobe_timeout: Duration,
pub ffprobe_sample_bytes: usize,
pub ffprobe_min_speed_mib: f64,
```

Add:

```rust
pub struct ProbeGates {
    pub probe_semaphore: Arc<tokio::sync::Semaphore>,
    pub ffprobe_counter: Arc<std::sync::atomic::AtomicU64>,
    pub max_ffprobe_per_run: u64,
}

impl ProbeGates {
    pub fn unlimited() -> Self {
        Self {
            probe_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
            ffprobe_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            max_ffprobe_per_run: 0,
        }
    }
}
```

In `src/quality/schedule.rs`:

- `probe_mode("full")` returns `ProbeMode::Full`.
- `probe_rank(Full) = 3`.
- `probe_limits` takes `ffprobe_path: PathBuf` and `ffprobe_ready: bool`; fills the new fields from config.
- `required_probe_mode` already handles fallback; keep `full` at `Full`.

- [ ] **Step 4: Add capture to `read_counted`**

Change the signature to:

```rust
async fn read_counted(
    mut response: Response,
    max_bytes: usize,
    max_secs: f64,
    capture: Option<&mut Vec<u8>>,
    capture_limit: usize,
) -> Result<(u64, Duration), String>
```

When `capture` is `Some` and `capture_limit > 0`, append `chunk[..take]` until the buffer reaches `capture_limit`.

Update all call sites with `None, 0` except the speed loop.

- [ ] **Step 5: Add the detector**

Implement:

```rust
fn is_mpegts(bytes: &[u8]) -> bool { ... }

fn parse_ffprobe_json(stdout: &[u8]) -> Option<(u16, u16)> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    value.get("streams")?.as_array()?.iter().find_map(|stream| {
        if stream.get("codec_type")?.as_str()? != "video" {
            return None;
        }
        let width = stream.get("width")?.as_u64()?;
        let height = stream.get("height")?.as_u64()?;
        (width > 0 && height > 0 && width <= u16::MAX as u64 && height <= u16::MAX as u64)
            .then(|| (width as u16, height as u16))
    })
}

async fn detect_resolution(
    path: &std::path::Path,
    bytes: &[u8],
    timeout: Duration,
) -> Result<(u16, u16), String> { ... }
```

Add the cap helper:

```rust
fn reserve_ffprobe_attempt(
    counter: &std::sync::atomic::AtomicU64,
    max: u64,
) -> bool {
    use std::sync::atomic::Ordering;
    if max == 0 {
        return true;
    }
    let previous = counter.fetch_add(1, Ordering::SeqCst);
    if previous >= max {
        counter.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}
```

`detect_resolution`:

- Build `tokio::process::Command::new(path)`.
- Args: `-v error`, `-probesize 512K`, `-analyzeduration 1000000`, `-select_streams v:0`, `-show_entries stream=codec_type,width,height`, `-of json`, `-`.
- Add `-f mpegts` only when `is_mpegts(bytes)`.
- `stdin(Stdio::piped())`, `stdout(Stdio::piped())`, `stderr(Stdio::piped())`, `kill_on_drop(true)`.
- Spawn. Take stdin, stdout, and stderr.
- Drain stdout and stderr concurrently with the stdin write. Keep at most 64 KiB of stdout and 4 KiB of stderr in memory; discard the rest so the child never blocks.
- Write bytes to stdin and drop it.
- Wrap the whole interaction (stdin write + drains + `wait()`) in `tokio::time::timeout`.
- On timeout, return `Err("ffprobe 超时")`.
- On non-success, return `Err` with the retained stderr.
- On success, `parse_ffprobe_json(&stdout).ok_or_else(|| "ffprobe 无视频流".into())`.

- [ ] **Step 6: Integrate into `probe`**

Change `probe` to take `gates: &ProbeGates`.

At the start:

```rust
let effective_mode = if mode == ProbeMode::Full && !limits.ffprobe {
    ProbeMode::Speed
} else {
    mode
};
```

In the speed sampling loop:

- If `effective_mode == Full && limits.ffprobe && limits.ffprobe_ready && (width.is_none() || height.is_none())`:
  - Check `reserve_ffprobe_attempt(&gates.ffprobe_counter, gates.max_ffprobe_per_run)`. On `false`, skip capture and ffprobe.
  - Try `gates.probe_semaphore.clone().try_acquire_owned()`.
  - If acquired, keep the permit and a `captured: Option<Vec<u8>>`. Create the buffer with `Vec::with_capacity(limits.ffprobe_sample_bytes)` so it never reallocates above the design bound.
- On each segment, if `captured.is_none()` and the permit is held, pass `Some(&mut capture)` and `limits.ffprobe_sample_bytes` to `read_counted`. After a successful read with bytes, set `captured = Some(capture)`.
- After the loop, if `captured` is `Some` and speed meets `ffprobe_min_speed_mib`:
  - Call `detect_resolution(&limits.ffprobe_path, &bytes, limits.ffprobe_timeout).await`.
  - On success, set `measurement.width` and `height`.
  - On failure, log `source=unknown`.
- Drop the permit after detection. If the speed is below the threshold, drop the permit without running ffprobe. The attempt stays counted.

The counter is reserved before the semaphore. This is the race-free order from the spec. An attempt can be consumed without a process if the speed gate fails or the semaphore is unavailable. That is acceptable: the cap limits attempts.

If `effective_mode == Speed`, skip capture and ffprobe.

- [ ] **Step 7: Update call sites**

In `src/quality/gate.rs`, pass `&ProbeGates::unlimited()` to `probe` and update `probe_limits(config, state.ffprobe(), state.ffprobe_ready().await)`.
This is temporary; Task 5 replaces it with per-run handles.

Also add `ProbeMode::Full => 3u8` to `memo_fingerprint`. Without it, adding the enum variant breaks the exhaustive match in this task.

- [ ] **Step 8: Run the tests**

Run: `cargo test quality::probe -- --nocapture`
Expected: PASS.

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/quality/probe.rs src/quality/schedule.rs src/quality/gate.rs
git commit -m "feat(quality): detect resolution from captured segments"
```

---

## Task 5: Wire per-run handles, host cache, and caps

**Files:**
- Modify: `src/quality/gate.rs`
- Modify: `src/quality/engine.rs`
- Modify: `src/quality/probe.rs` (tests and permit/buffer drop order; `subscription.rs` needs no change)

**Interfaces:**
- Consumes: `ProbeGates`, `ProbeSignature`, `state.ffprobe_ready`.
- Produces:
  - `probe_candidate(..., gates: &ProbeGates)`
  - `fn should_use_host_cache(candidate: &Candidate, mode: ProbeMode, ffprobe: bool, dash_fallback: bool) -> bool`
  - `filter_plan` creates `ProbeGates` per operation.
  - `engine::run` creates `ProbeGates` per run.
  - Host cache is skipped when `mode == Full && config.quality.ffprobe`.

- [ ] **Step 1: Write the failing host-cache test**

Add to `src/quality/gate.rs` tests:

```rust
#[test]
fn full_ffprobe_skips_host_cache() {
    let mut candidate = candidate("sync:s1:c", false);
    candidate.rules.host_cache = true;
    assert!(!should_use_host_cache(&candidate, ProbeMode::Full, true, false));
    assert!(should_use_host_cache(&candidate, ProbeMode::Full, false, false));
    assert!(should_use_host_cache(&candidate, ProbeMode::Speed, true, false));
    assert!(!should_use_host_cache(&candidate, ProbeMode::Full, true, true), "DASH 回退优先");
    candidate.rules.host_cache = false;
    assert!(!should_use_host_cache(&candidate, ProbeMode::Speed, false, false));
}
```

Add to `src/quality/probe.rs` tests:

```rust
#[tokio::test]
async fn probe_skips_capture_when_semaphore_is_held() {
    let dir = std::env::temp_dir().join(format!("tvb-ffprobe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("建目录");
    let marker = dir.join("ran");
    let body = format!(
        "#!/bin/sh\necho x >> {}\ncat > /dev/null\nprintf '{{\"streams\":[{{\"codec_type\":\"video\",\"width\":1280,\"height\":720}}]}}'\n",
        marker.display()
    );
    let script = write_script(&dir, body.as_bytes());
    let limits = ProbeLimits {
        ffprobe: true,
        ffprobe_ready: true,
        ffprobe_path: script,
        ..ProbeLimits::for_test()
    };
    let gates = ProbeGates {
        probe_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        ffprobe_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        max_ffprobe_per_run: 0,
    };
    let held = gates.probe_semaphore.clone().acquire_owned().await.expect("许可");
    let url = serve_hls_segment().await;
    let out = probe(
        &reqwest::Client::new(),
        &url,
        &[],
        ProbeMode::Full,
        None,
        &limits,
        &gates,
    )
    .await;
    assert!(out.measurement.width.is_none(), "无许可时不采集");
    assert!(out.measurement.speed_mib.is_some(), "测速继续");
    assert!(!marker.exists(), "不得运行 ffprobe");
    drop(held);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn shared_gates_run_ffprobe_once_when_capped() {
    let dir = std::env::temp_dir().join(format!("tvb-ffprobe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("建目录");
    let marker = dir.join("runs");
    let body = format!(
        "#!/bin/sh\necho x >> {}\ncat > /dev/null\nprintf '{{\"streams\":[{{\"codec_type\":\"video\",\"width\":1280,\"height\":720}}]}}'\n",
        marker.display()
    );
    let script = write_script(&dir, body.as_bytes());
    let limits = ProbeLimits {
        ffprobe: true,
        ffprobe_ready: true,
        ffprobe_path: script,
        ..ProbeLimits::for_test()
    };
    let gates = ProbeGates {
        probe_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        ffprobe_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        max_ffprobe_per_run: 1,
    };
    let first = serve_hls_segment().await;
    let second = serve_hls_segment().await;
    let client = reqwest::Client::new();
    let _ = probe(&client, &first, &[], ProbeMode::Full, None, &limits, &gates).await;
    let _ = probe(&client, &second, &[], ProbeMode::Full, None, &limits, &gates).await;
    let runs = std::fs::read_to_string(&marker).unwrap_or_default();
    assert_eq!(runs.lines().count(), 1, "共享计数器只允许一次 ffprobe");
    let _ = std::fs::remove_dir_all(dir);
}
```

In `src/quality/probe.rs`, drop the capture buffer before dropping the semaphore permit on every exit path of the capture block, so peak memory never exceeds the permit count.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test quality::gate::tests::full_ffprobe_skips_host_cache -- --nocapture`
Expected: compile error for missing `should_use_host_cache`.

- [ ] **Step 3: Create per-operation handles**

In `engine::run`:

```rust
let probe = ProbeSignature::from_config(&config, state.ffprobe_ready().await);
let gates = ProbeGates {
    probe_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(
        config.quality.probe_concurrency as usize,
    )),
    ffprobe_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    max_ffprobe_per_run: config.quality.max_ffprobe_per_run,
};
```

Pass `&probe` to `due_targets` and `decide`. Pass `&gates` to `probe_candidate`.

In `gate::filter_plan`, create the same handles before probing.

Update `probe_candidate` signature:

```rust
pub(crate) async fn probe_candidate(
    state: &Arc<AppState>,
    config: &Config,
    candidate: &Candidate,
    memo: &std::sync::Mutex<RunMemo>,
    resolve: bool,
    gates: &ProbeGates,
) -> CandidateOutcome
```

- [ ] **Step 4: Skip the host cache**

In `probe_candidate`, add:

```rust
fn should_use_host_cache(
    candidate: &Candidate,
    mode: ProbeMode,
    ffprobe: bool,
    dash_fallback: bool,
) -> bool {
    candidate.rules.host_cache
        && !dash_fallback
        && !(mode == ProbeMode::Full && ffprobe)
}
```

Use it:

```rust
let host_cached = should_use_host_cache(
    candidate,
    probe_mode,
    config.quality.ffprobe,
    dash_fallback,
)
.then(|| ...)
.flatten();
```

Pass `gates` to `probe`.

- [ ] **Step 5: Verify memo rank**

Confirm `memo_fingerprint` contains `ProbeMode::Full => 3u8` from Task 4. No code change here.

- [ ] **Step 6: Run the tests**

Run: `cargo test quality::gate -- --nocapture`
Expected: PASS.

Run: `cargo test quality::engine -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Run the full suite and commit**

Run: `cargo test`
Expected: all tests pass.

```bash
git add src/quality/gate.rs src/quality/engine.rs src/quality/probe.rs
git commit -m "feat(quality): wire probe gates and host-cache rules"
```

---

## Task 6: Rebuild the quality rules page

**Files:**
- Modify: `frontend/src/api.ts`
- Modify: `frontend/src/quality-page.tsx`
- Modify: `frontend/src/style.css`
- Modify: `frontend/src/ui.tsx`

**Interfaces:**
- Consumes: `GET /panel/api/config`, `GET /panel/api/quality/status`, `PUT /panel/api/config`.
- Produces:
  - `QualityEffective.rules.fallback_speed_mib`
  - `QualityStatus.ffprobe`
  - `RulesTab({ status, onSaved })`, where `status: QualityStatus | null`
  - Rules tab: 3-column grid, fallback field, full-width resolution table, ffprobe card.

- [ ] **Step 1: Update the frontend types**

In `frontend/src/api.ts`:

```ts
export type QualityEffective = {
  rules: {
    // existing
    fallback_speed_mib: number;
  };
  // ...
};

export type QualityStatus = {
  // existing
  ffprobe: {
    installed: boolean;
    enabled: boolean;
    ready: boolean;
    note: string;
  };
};
```

- [ ] **Step 2: Pass quality status into `RulesTab`**

Change:

```tsx
function RulesTab({ onSaved }: { onSaved: () => void }) {
```

to:

```tsx
function RulesTab({
  status,
  onSaved,
}: {
  status: QualityStatus | null;
  onSaved: () => void;
}) {
```

Change the call site:

```tsx
{tab === 'rules' && <RulesTab status={status} onSaved={() => void load()} />}
```

- [ ] **Step 3: Add the fallback field**

In `quality-page.tsx`, place it next to “最低速率”:

```tsx
<Field label="兜底速率" hint="MiB/s，无法识别分辨率，或速率表未命中时使用；0 = 用最低速率">
  <NumberInput
    min={0}
    step={0.1}
    value={asNumber(draft, 'quality.rules.fallback_speed_mib')}
    onChange={(value) => update('quality.rules.fallback_speed_mib', value)}
  />
</Field>
```

- [ ] **Step 4: Replace the threshold card layout**

Wrap the threshold card in `className="card quality-rules-card"`.

Replace `<div className="fields">` with `<div className="quality-rules-grid">`.

Add section headings:

```tsx
<h3 className="quality-section-title">速率与延迟</h3>
<h3 className="quality-section-title">分辨率</h3>
<h3 className="quality-section-title">其他</h3>
```

Field mapping:

- 速率与延迟: 最低速率、兜底速率、最大延迟。
- 分辨率: 最低分辨率、最高分辨率、Host 缓存、分辨率速率表。
- 其他: 手工下线宽限。

The resolution table Field gets `className="quality-field-wide"`. The `Field` component appends it to its own `field` class.

- [ ] **Step 5: Rewrite the resolution table**

Use a native table:

```tsx
<Field
  label="分辨率速率要求"
  hint="按实际分辨率取不超过它的最高档；未命中时用兜底速率；空行不保存"
  className="quality-field-wide"
>
  <table className="quality-speed-table">
    <thead>
      <tr>
        <th>分辨率</th>
        <th>最低速率 (MiB/s)</th>
        <th>操作</th>
      </tr>
    </thead>
    <tbody>
      {speedRows.map((row, index) => (
        <tr key={index}>
          <td data-label="分辨率">
            <input
              aria-label={`第 ${index + 1} 行分辨率`}
              value={row.resolution}
              placeholder="1280x720"
              onChange={...}
            />
          </td>
          <td data-label="最低速率 (MiB/s)">
            <input
              aria-label={`第 ${index + 1} 行最低速率`}
              type="number"
              min={0}
              step={0.1}
              value={row.speed}
              placeholder="MiB/s"
              onChange={...}
            />
          </td>
          <td>
            <button type="button" className="ghost" onClick={...}>删除</button>
          </td>
        </tr>
      ))}
    </tbody>
  </table>
  <button type="button" className="ghost" onClick={...}>添加分辨率</button>
</Field>
```

The `Field` component currently does not accept `className`. Add an optional `className?: string` prop and append it to `className={`field ${className ?? ''}`}` in `frontend/src/ui.tsx`.

- [ ] **Step 6: Add the ffprobe card**

Place it between “判定阈值” and “安全阀”:

```tsx
<div className="card">
  <div className="card-head">
    <div className="card-head-text">
      <h2>分辨率探测</h2>
      <span className="muted">完整档读取切片开头，用 ffprobe 识别分辨率</span>
    </div>
  </div>
  <div className="fields">
    <Field label="启用分辨率识别" tip="只在完整档生效。未安装 ffprobe 时使用兜底速率。">
      <Switch
        checked={asBool(draft, 'quality.ffprobe')}
        onChange={(next) => update('quality.ffprobe', next)}
        label="启用分辨率识别"
      />
    </Field>
    <Field label="单次超时" hint="秒，1–30">
      <NumberInput min={1} max={30} value={asNumber(draft, 'quality.ffprobe_timeout_secs')} onChange={...} />
    </Field>
    <Field label="并发" hint="1–4">
      <NumberInput min={1} max={4} value={asNumber(draft, 'quality.probe_concurrency')} onChange={...} />
    </Field>
    <Field label="单轮上限" hint="0 = 不限制">
      <NumberInput min={0} value={asNumber(draft, 'quality.max_ffprobe_per_run')} onChange={...} />
    </Field>
    <Field label="最低测速门槛" hint="MiB/s，0 = 不限制">
      <NumberInput min={0} step={0.1} value={asNumber(draft, 'quality.ffprobe_min_speed_mib')} onChange={...} />
    </Field>
    <Field label="采样大小" hint="KiB，读取切片开头。64–2048">
      <NumberInput min={64} max={2048} value={asNumber(draft, 'quality.ffprobe_sample_kb')} onChange={...} />
    </Field>
  </div>
  {status && !status.ffprobe.installed ? (
    <div className="hint">未安装 ffprobe。分辨率识别不可用，规则会使用兜底速率。</div>
  ) : null}
  {status && status.ffprobe.installed && !status.ffprobe.enabled ? (
    <div className="hint">开启后，完整档会读取切片开头识别分辨率。</div>
  ) : null}
</div>
```

- [ ] **Step 7: Update labels**

- `PROBE_LABEL.full` → `'完整'`.
- Full option description → `'测速，并按需识别分辨率。'`.
- `OVERRIDE_LABEL.fallback_speed_mib` → `'兜底速率'`.
- The actual-probe hint: when `probe_mode=full` and `ffprobe=false`, show `实际档位：测速（ffprobe 未启用）`.

- [ ] **Step 8: Add the CSS**

In `frontend/src/style.css`:

```css
.quality-rules-card {
  container-type: inline-size;
}

.quality-rules-grid {
  display: grid;
  grid-template-columns: 1fr;
  gap: 12px 16px;
}

@container (min-width: 640px) {
  .quality-rules-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@container (min-width: 1000px) {
  .quality-rules-grid {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}

.quality-field-wide {
  grid-column: 1 / -1;
}

.quality-section-title {
  grid-column: 1 / -1;
  margin: 6px 0 0;
  font-size: 12px;
  font-weight: 600;
  color: var(--muted);
}

.quality-speed-table {
  width: 100%;
  border-collapse: collapse;
}

.quality-speed-table th,
.quality-speed-table td {
  padding: 6px 8px 6px 0;
  text-align: left;
  vertical-align: middle;
}

.quality-speed-table th {
  font-size: 11px;
  font-weight: 600;
  color: var(--muted);
}

.quality-speed-table input {
  width: 100%;
  min-width: 0;
}

@container (max-width: 560px) {
  .quality-speed-table thead {
    display: none;
  }
  .quality-speed-table tr {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: 6px;
    margin-bottom: 6px;
  }
  .quality-speed-table td {
    padding: 0;
  }
  .quality-speed-table td::before {
    content: attr(data-label);
    display: block;
    margin-bottom: 3px;
    font-size: 11px;
    color: var(--muted);
  }
  .quality-speed-table td:not([data-label])::before {
    display: none;
  }
}
```

Remove `.quality-speed-map` and `.quality-speed-row` fixed widths. Keep them if still referenced elsewhere; the plan may replace them entirely.

- [ ] **Step 9: Build and check**

Run: `cd frontend && pnpm build`
Expected: PASS.

Run the app and check widths 1440, 1024, 720, 460. Expected: no overlap, no horizontal scroll, table headers visible above 560 px, labels visible below.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/api.ts frontend/src/quality-page.tsx frontend/src/style.css frontend/src/ui.tsx
git commit -m "feat(panel): rebuild quality rules layout"
```

---

## Task 7: Documentation and end-to-end verification

**Files:**
- Modify: `docs/design/13-availability-and-speedtest.md`
- Modify: `docs/design/15-quality-admission-function.md`
- Modify: `docs/tests/07-python-m3u8-simulation.md`
- Modify: `docs/superpowers/specs/2026-11-18-panel-ffmpeg-install-design.md`

- [ ] **Step 1: Update design docs**

- `13-availability-and-speedtest.md`: replace the ffprobe-on-URL description with the captured-bytes-on-stdin approach. Change ffprobe failure from `probe_error` to “unknown resolution, fallback”. Document the fMP4 limitation and new config fields.
- `15-quality-admission-function.md`: add the fallback speed field, resolution detection card, and table layout.
- `07-python-m3u8-simulation.md`: add acceptance steps for fallback and ffprobe.
- `2026-11-18-panel-ffmpeg-install-design.md`: update to two artifacts and two status rows.

- [ ] **Step 2: Run formatting and tests**

Run: `cargo fmt --check`
Expected: PASS. If it fails, run `cargo fmt` and re-run.

Run: `cargo test`
Expected: all tests pass.

Run: `cargo clippy --all-targets`
Expected: PASS. CI runs this without `-D warnings` (`.github/workflows/ci.yml`). Record any warning whose location is in a file this feature changed; the repo has pre-existing warnings elsewhere. Do not add `-D warnings`.

Run: `cd frontend && pnpm build`
Expected: PASS.

- [ ] **Step 3: Manual container test**

1. Build the Linux binary: `docker build --target build -t tvb-build:test .`
2. Generate a TS HLS fixture with ffmpeg. Use a media playlist whose segment URLs have no `.ts` extension, and a master playlist without `RESOLUTION`.
3. Run the simulator and a local tv-buddy container. Set `probe_mode=full`, `ffprobe=true`, `min_speed_mib=0`, `fallback_speed_mib=0.8`, `ffprobe_sample_kb=512`.
4. Run the quality check. Expected: resolution is detected; result shows `WxH`. If resolution is unknown, the reason follows the fallback speed, not `probe_error`.
5. Disable ffprobe. Expected: resolution `—`, fallback used.
6. Serve an fMP4 HLS fixture with `EXT-X-MAP`. Expected: resolution `—`, fallback used, no `probe_error`.
7. Install from the panel. Expected: both binaries downloaded and usable; `ffprobe -version` succeeds.
8. Check the rules page at 1440, 1024, 720, 460 px. Expected: no overlap. If no browser is available, record this row as 未验证 with that reason in `docs/tests/runs/`; do not claim it passed.

- [ ] **Step 4: Commit and tag**

```bash
git add docs/design/13-availability-and-speedtest.md docs/design/15-quality-admission-function.md docs/tests/07-python-m3u8-simulation.md docs/tests/README.md docs/tests/runs/ docs/superpowers/specs/2026-11-18-quality-resolution-fallback-design.md docs/superpowers/specs/2026-11-18-panel-ffmpeg-install-design.md docs/superpowers/plans/2026-11-18-quality-resolution-fallback.md
git commit -m "docs: document quality fallback and ffprobe detection"
git tag -a v2026.51 -m "v2026.51 quality fallback and ffprobe resolution"
git push origin main --tags
```

Do not stage `docs/research/`; it is unrelated to this feature. The push follows the project's per-feature tag convention. If the push is not desired in the current environment, stop after `git tag`.

---

## Plan Self-Review

- **Spec coverage:** fallback field/evaluation (Task 1), rule hash revalidation (Task 2), installer + status + UI (Task 3), ffprobe detection (Task 4), run handles and host cache (Task 5), rules page rebuild (Task 6), docs and verification (Task 7).
- **Type consistency:** `ProbeSignature`, `ProbeGates`, `ProbeMode::Full`, `fallback_speed_mib`, `ffprobe_ready` are used with the same names across tasks.
- **Review Focus:** each listed input has a task test or manual step.
- **Proportion:** the plan defines interfaces and tests. Function bodies are left to the implementer except for the ffprobe parser, hash field list, and the fixed artifact values.
