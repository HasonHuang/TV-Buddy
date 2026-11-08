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
    Ok(())
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
}
