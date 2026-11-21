//! 配置模型、校验与持久化入口。
//!
//! 存：SQLite 单行 JSON 文档，见 [`store`]。
//! 运行时只认数据库里的 JSON，不读写 TOML。

pub mod model;
pub mod store;

use anyhow::bail;

pub use model::Config;
pub use store::ConfigStore;

/// 全量保存时，调用方持有的版本已过期。
#[derive(Debug, thiserror::Error)]
#[error("配置已被更新，当前版本 {0}")]
pub struct StaleRevision(pub u64);

/// 基本校验。只拦会破坏运行的值。
pub fn validate(config: &Config) -> anyhow::Result<()> {
    if config.server.bind.trim().is_empty() {
        bail!("server.bind 不能为空");
    }
    if config.cache.disk_path.trim().is_empty() {
        bail!("cache.disk_path 不能为空");
    }
    if (1..60).contains(&config.cache.gc.max_age_secs) {
        bail!("cache.gc.max_age_secs 必须是 0 或至少 60");
    }
    let window = config.auth.slice_sign_window_secs;
    if !(crate::sign::WINDOW_MIN..=crate::sign::WINDOW_MAX).contains(&window) {
        bail!(
            "auth.slice_sign_window_secs 必须在 {}–{} 之间",
            crate::sign::WINDOW_MIN,
            crate::sign::WINDOW_MAX
        );
    }
    let secret = config.auth.slice_sign_secret.trim();
    if !secret.is_empty()
        && (secret.len() < 32
            || secret.len() > 128
            || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        bail!("auth.slice_sign_secret 必须是 32–128 位十六进制字符串");
    }
    let mut seen = std::collections::HashSet::new();
    for group in &config.groups {
        if group.id.trim().is_empty() {
            bail!("分组 id 不能为空");
        }
        if !seen.insert(&group.id) {
            bail!("分组 id 重复: {}", group.id);
        }
        let mut chans = std::collections::HashSet::new();
        for channel in &group.channels {
            if !chans.insert(&channel.id) {
                bail!("频道 id 重复: {}/{}", group.id, channel.id);
            }
        }
        if group.origin != "manual" && group.origin != "sync" {
            bail!("分组 {} 的 origin 必须是 manual 或 sync", group.id);
        }
    }
    validate_playlist(&config.playlist)?;
    validate_quality(&config.quality)?;
    for source in &config.playlist.sources {
        if let Some(quality) = &source.quality {
            validate_quality_override(quality, &format!("源 {} 的 quality", source.id))?;
        }
    }
    for group in &config.groups {
        if let Some(quality) = &group.quality {
            validate_quality_override(quality, &format!("分组 {} 的 quality", group.id))?;
        }
        for channel in &group.channels {
            if let Some(quality) = &channel.quality {
                validate_quality_override(
                    quality,
                    &format!("频道 {}/{} 的 quality", group.id, channel.id),
                )?;
            }
        }
    }
    for group in &config.groups {
        for channel in &group.channels {
            let rules = crate::quality::rules::effective_rules(config, group, channel);
            if let (Some(min), Some(max)) = (rules.min_resolution_px, rules.max_resolution_px) {
                if min > max {
                    bail!("频道 {}/{} 的有效分辨率下限高于上限", group.id, channel.id);
                }
            }
        }
    }
    Ok(())
}

fn validate_quality(quality: &crate::config::model::QualityCfg) -> anyhow::Result<()> {
    if !matches!(
        quality.probe_mode.as_str(),
        "reachability" | "playlist" | "speed" | "full"
    ) {
        bail!("quality.probe_mode 不支持 {}", quality.probe_mode);
    }
    if !matches!(quality.schedule.as_str(), "interval" | "cron") {
        bail!("quality.schedule 只能是 interval 或 cron");
    }
    if !(1..=8760).contains(&quality.interval_hours) {
        bail!("quality.interval_hours 必须在 1–8760 之间");
    }
    if crate::cron::parse_cron(&quality.cron).is_err() {
        bail!("quality.cron 不是有效的 5 段表达式");
    }
    if !quality.max_remove_ratio.is_finite()
        || !(0.0..=1.0).contains(&quality.max_remove_ratio)
        || quality.max_remove_ratio == 0.0
    {
        bail!("quality.max_remove_ratio 必须在 (0,1] 之间");
    }
    if !(1..=100_000).contains(&quality.min_remove_count) {
        bail!("quality.min_remove_count 必须在 1–100000 之间");
    }
    if quality.result_retention_days > 3650 {
        bail!("quality.result_retention_days 不能超过 3650");
    }
    if quality.gate_time_budget_secs > 86_400 {
        bail!("quality.gate_time_budget_secs 不能超过 86400");
    }
    if !(1..=32).contains(&quality.concurrency) {
        bail!("quality.concurrency 必须在 1–32 之间");
    }
    if !(1..=120).contains(&quality.timeout_secs) {
        bail!("quality.timeout_secs 必须在 1–120 之间");
    }
    if !(1..=120).contains(&quality.speed_timeout_secs) {
        bail!("quality.speed_timeout_secs 必须在 1–120 之间");
    }
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
    if !(64..=65_536).contains(&quality.sample_bytes_kb) {
        bail!("quality.sample_bytes_kb 必须在 64–65536 之间");
    }
    if !(0.5..=30.0).contains(&quality.sample_secs) {
        bail!("quality.sample_secs 必须在 0.5–30 之间");
    }
    if !(1..=8).contains(&quality.sample_segments) {
        bail!("quality.sample_segments 必须在 1–8 之间");
    }
    if !(64..=16_384).contains(&quality.max_playlist_kb) {
        bail!("quality.max_playlist_kb 必须在 64–16384 之间");
    }
    if quality.stale_keep_secs > 86_400 {
        bail!("quality.stale_keep_secs 不能超过 86400");
    }
    validate_quality_rules(&quality.rules, "quality.rules")
}

fn validate_quality_rules(
    rules: &crate::config::model::QualityRules,
    label: &str,
) -> anyhow::Result<()> {
    if !rules.min_speed_mib.is_finite() || rules.min_speed_mib < 0.0 {
        bail!("{label}.min_speed_mib 必须是非负数");
    }
    if !rules.fallback_speed_mib.is_finite() || rules.fallback_speed_mib < 0.0 {
        bail!("{label}.fallback_speed_mib 必须是非负数");
    }
    let min = parse_resolution_px(&rules.min_resolution, label)?;
    let max = parse_resolution_px(&rules.max_resolution, label)?;
    if let (Some(min), Some(max)) = (min, max) {
        if min > max {
            bail!("{label}.min_resolution 不能大于 max_resolution");
        }
    }
    for (key, value) in &rules.resolution_speed_map {
        let Some(_) = parse_resolution_px(key, label)? else {
            bail!("{label}.resolution_speed_map 含空分辨率");
        };
        if !value.is_finite() || *value < 0.0 {
            bail!("{label}.resolution_speed_map 的速率必须是非负数");
        }
    }
    Ok(())
}

fn validate_quality_override(
    quality: &crate::config::model::QualityOverride,
    label: &str,
) -> anyhow::Result<()> {
    if let Some(speed) = quality.min_speed_mib {
        if !speed.is_finite() || speed < 0.0 {
            bail!("{label}.min_speed_mib 必须是非负数");
        }
    }
    if let Some(speed) = quality.fallback_speed_mib {
        if !speed.is_finite() || speed < 0.0 {
            bail!("{label}.fallback_speed_mib 必须是非负数");
        }
    }
    let min = match &quality.min_resolution {
        Some(value) => parse_resolution_px(value, label)?,
        None => None,
    };
    let max = match &quality.max_resolution {
        Some(value) => parse_resolution_px(value, label)?,
        None => None,
    };
    if let (Some(min), Some(max)) = (min, max) {
        if min > max {
            bail!("{label}.min_resolution 不能大于 max_resolution");
        }
    }
    if let Some(map) = &quality.resolution_speed_map {
        for (key, value) in map {
            let Some(_) = parse_resolution_px(key, label)? else {
                bail!("{label}.resolution_speed_map 含空分辨率");
            };
            if !value.is_finite() || *value < 0.0 {
                bail!("{label}.resolution_speed_map 的速率必须是非负数");
            }
        }
    }
    if let Some(secs) = quality.stale_keep_secs {
        if secs > 86_400 {
            bail!("{label}.stale_keep_secs 不能超过 86400");
        }
    }
    Ok(())
}

/// 解析 `宽x高`。空字符串返回 `None`。
fn parse_resolution_px(value: &str, label: &str) -> anyhow::Result<Option<u64>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let Some((width, height)) = value.split_once('x') else {
        bail!("{label} 的分辨率必须是 宽x高");
    };
    let width: u64 = width
        .parse()
        .map_err(|_| anyhow::anyhow!("{label} 的分辨率宽度不合法"))?;
    let height: u64 = height
        .parse()
        .map_err(|_| anyhow::anyhow!("{label} 的分辨率高度不合法"))?;
    if !(1..=99_999).contains(&width) || !(1..=99_999).contains(&height) {
        bail!("{label} 的分辨率超出范围");
    }
    Ok(Some(width * height))
}

/// 校验订阅与同步设置。
///
/// 只拦会破坏运行的值。调度值与抓取参数可以留空，留空表示继承。
fn validate_playlist(playlist: &crate::config::model::PlaylistCfg) -> anyhow::Result<()> {
    match playlist.sync_schedule.as_str() {
        "interval" | "daily" => {}
        other => bail!("playlist.sync_schedule 不支持 {other}"),
    }
    if playlist.sync_interval_secs != 0 && !(300..=2_592_000).contains(&playlist.sync_interval_secs)
    {
        bail!("playlist.sync_interval_secs 必须在 300–2592000 之间");
    }
    if playlist.sync_schedule == "daily" {
        if !valid_daily_at(&playlist.sync_daily_at) {
            bail!("playlist.sync_daily_at 必须是 HH:MM");
        }
    } else if !playlist.sync_daily_at.is_empty() && !valid_daily_at(&playlist.sync_daily_at) {
        bail!("playlist.sync_daily_at 必须是 HH:MM");
    }
    if playlist.sync_fetch_timeout_secs == 0 || playlist.sync_fetch_timeout_secs > 600 {
        bail!("playlist.sync_fetch_timeout_secs 必须在 1–600 之间");
    }
    if playlist.sync_concurrency == 0 || playlist.sync_concurrency > 16 {
        bail!("playlist.sync_concurrency 必须在 1–16 之间");
    }
    if !(0.0..=1.0).contains(&playlist.sync_warn_removal_ratio) {
        bail!("playlist.sync_warn_removal_ratio 必须在 0–1 之间");
    }
    let mut ids = std::collections::HashSet::new();
    for source in &playlist.sources {
        if source.id.trim().is_empty() {
            bail!("订阅源 id 不能为空");
        }
        if !ids.insert(&source.id) {
            bail!("订阅源 id 重复: {}", source.id);
        }
        match source.mode.as_str() {
            "aggregate" | "sync" => {}
            other => bail!("订阅源 {} 的 mode 不支持 {other}", source.id),
        }
        match source.schedule.as_str() {
            "inherit" | "interval" | "daily" => {}
            other => bail!("订阅源 {} 的 schedule 不支持 {other}", source.id),
        }
        // 每天定时必须有可用的 HH:MM。源级留空时用全局；
        // 全局为间隔模式且时间为空时，拒绝保存，避免退化成「上次 +24h」。
        let daily = match source.schedule.as_str() {
            "daily" => true,
            "interval" => false,
            _ => playlist.sync_schedule == "daily",
        };
        if daily {
            let at = if source.sync_daily_at.trim().is_empty() {
                playlist.sync_daily_at.trim()
            } else {
                source.sync_daily_at.trim()
            };
            if !valid_daily_at(at) {
                bail!(
                    "订阅源 {} 为每天定时，但源级与全局的每天时间都为空",
                    source.id
                );
            }
        }
        if source.sync_interval_secs != 0 && !(300..=2_592_000).contains(&source.sync_interval_secs)
        {
            bail!(
                "订阅源 {} 的 sync_interval_secs 必须在 300–2592000 之间",
                source.id
            );
        }
        if !source.sync_daily_at.is_empty() && !valid_daily_at(&source.sync_daily_at) {
            bail!("订阅源 {} 的 sync_daily_at 必须是 HH:MM", source.id);
        }
        match source.playback_mode.as_str() {
            "auto" | "proxy" | "remux" | "dash" | "redirect" | "rewrite" => {}
            "passthrough" if source.mode == "sync" => {
                bail!("同步源 {} 禁止 passthrough", source.id);
            }
            "passthrough" => {}
            other => bail!("订阅源 {} 的 playback_mode 不支持 {other}", source.id),
        }
        match source.format.as_str() {
            "auto" | "hls" | "mpd" => {}
            other => bail!("订阅源 {} 的 format 不支持 {other}", source.id),
        }
        match source.position.as_str() {
            "" | "top" | "bottom" => {}
            other => bail!("订阅源 {} 的 position 不支持 {other}", source.id),
        }
        for (from, to) in &source.merge_into {
            if from.trim().is_empty() || to.trim().is_empty() {
                bail!("订阅源 {} 的 merge_into 不能有空键或空值", source.id);
            }
        }
        for (from, to) in &source.merge_new {
            if from.trim().is_empty() || to.trim().is_empty() {
                bail!("订阅源 {} 的 merge_new 不能有空键或空值", source.id);
            }
        }
    }
    Ok(())
}

/// `HH:MM`，24 小时制。
pub(crate) fn valid_daily_at(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    if hour.len() != 2 || minute.len() != 2 {
        return false;
    }
    let (Ok(hour), Ok(minute)) = (hour.parse::<u32>(), minute.parse::<u32>()) else {
        return false;
    };
    hour < 24 && minute < 60
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{Channel, Group, PlaylistSource};

    fn config_with_source(source: PlaylistSource) -> Config {
        let mut config = Config::default();
        config.playlist.sources.push(source);
        config
    }

    fn source(mode: &str) -> PlaylistSource {
        PlaylistSource {
            id: "s1".into(),
            name: "源".into(),
            url: "http://up/a.m3u".into(),
            mode: mode.into(),
            ..PlaylistSource::default()
        }
    }

    #[test]
    fn default_config_is_valid() {
        validate(&Config::default()).expect("默认配置应合法");
    }

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

    #[test]
    fn sync_source_rejects_passthrough() {
        let mut source = source("sync");
        source.playback_mode = "passthrough".into();
        let error = validate(&config_with_source(source)).expect_err("应拒绝");
        assert!(error.to_string().contains("passthrough"), "{error}");
    }

    #[test]
    fn aggregate_source_allows_passthrough() {
        let mut source = source("aggregate");
        source.playback_mode = "passthrough".into();
        validate(&config_with_source(source)).expect("聚合源可保留 passthrough");
    }

    #[test]
    fn daily_source_without_time_is_rejected() {
        let mut config = config_with_source(source("sync"));
        config.playlist.sync_schedule = "interval".into();
        config.playlist.sync_daily_at.clear();
        config.playlist.sources[0].schedule = "daily".into();
        let error = validate(&config).expect_err("源级与全局都没有时间时应拒绝");
        assert!(error.to_string().contains("每天时间"), "{error}");
    }

    #[test]
    fn daily_source_inherits_global_time() {
        let mut config = config_with_source(source("sync"));
        config.playlist.sync_schedule = "daily".into();
        config.playlist.sync_daily_at = "04:15".into();
        config.playlist.sources[0].schedule = "daily".into();
        validate(&config).expect("源级留空应继承全局时间");
    }

    #[test]
    fn duplicate_source_ids_are_rejected() {
        let mut config = Config::default();
        config.playlist.sources.push(source("aggregate"));
        config.playlist.sources.push(source("sync"));
        let error = validate(&config).expect_err("应拒绝");
        assert!(error.to_string().contains("重复"), "{error}");
    }

    #[test]
    fn bad_schedule_values_are_rejected() {
        let mut source = source("sync");
        source.schedule = "hourly".into();
        assert!(validate(&config_with_source(source)).is_err());

        let mut config = Config::default();
        config.playlist.sync_daily_at = "3:30".into();
        assert!(validate(&config).is_err());

        let mut config = Config::default();
        config.playlist.sync_interval_secs = 10;
        assert!(validate(&config).is_err());
    }

    #[test]
    fn unknown_playback_mode_is_rejected() {
        let mut source = source("sync");
        source.playback_mode = "magic".into();
        assert!(validate(&config_with_source(source)).is_err());
    }

    #[test]
    fn empty_merge_into_is_rejected() {
        let mut source = source("sync");
        source.merge_into.insert("组".into(), " ".into());
        assert!(validate(&config_with_source(source)).is_err());
    }

    #[test]
    fn empty_merge_new_is_rejected() {
        let mut source = source("sync");
        source.merge_new.insert("组".into(), " ".into());
        assert!(validate(&config_with_source(source)).is_err());
    }

    #[test]
    fn unknown_group_origin_is_rejected() {
        let mut config = Config::default();
        config.groups.push(Group {
            id: "g".into(),
            origin: "other".into(),
            channels: vec![Channel::default()],
            ..Group::default()
        });
        assert!(validate(&config).is_err());
    }

    #[test]
    fn daily_time_check_accepts_full_day() {
        assert!(valid_daily_at("00:00"));
        assert!(valid_daily_at("23:59"));
        assert!(!valid_daily_at("24:00"));
        assert!(!valid_daily_at("12:60"));
        assert!(!valid_daily_at(""));
    }

    #[test]
    fn interval_schedule_allows_empty_daily_time() {
        let mut config = Config::default();
        config.playlist.sync_schedule = "interval".into();
        config.playlist.sync_daily_at.clear();
        validate(&config).expect("间隔调度不要求每天时间");
    }

    #[test]
    fn daily_schedule_requires_daily_time() {
        let mut config = Config::default();
        config.playlist.sync_schedule = "daily".into();
        config.playlist.sync_daily_at.clear();
        assert!(validate(&config).is_err());
    }

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
        config.quality.rules.fallback_speed_mib = 0.0;
        config.quality.ffprobe_min_speed_mib = -0.1;
        assert!(validate(&config).is_err());
    }

    #[test]
    fn quality_rejects_bad_values() {
        let mut config = Config::default();
        config.quality.probe_mode = "turbo".into();
        assert!(validate(&config).is_err());

        let mut config = Config::default();
        config.quality.concurrency = 0;
        assert!(validate(&config).is_err());

        let mut config = Config::default();
        config.quality.rules.min_resolution = "1080".into();
        assert!(validate(&config).is_err());

        let mut config = Config::default();
        config.quality.rules.min_resolution = "1920x1080".into();
        config.quality.rules.max_resolution = "1280x720".into();
        assert!(validate(&config).is_err());

        let mut config = Config::default();
        config.quality.rules.max_resolution = "1280x720".into();
        config.groups.push(Group {
            id: "g".into(),
            channels: vec![Channel {
                id: "c".into(),
                quality: Some(crate::config::model::QualityOverride {
                    min_resolution: Some("1920x1080".into()),
                    ..Default::default()
                }),
                ..Channel::default()
            }],
            ..Group::default()
        });
        assert!(
            validate(&config).is_err(),
            "跨层组合后 min 大于 max 必须拒绝"
        );
    }
}
