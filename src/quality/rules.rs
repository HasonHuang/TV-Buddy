//! 质量规则：有效规则解析与判定。

use std::collections::BTreeMap;

use crate::config::model::{Channel, Config, Group, QualityOverride};

/// 判定后的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Hide,
    Mark,
    Off,
}

impl Action {
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "mark" => Self::Mark,
            "off" => Self::Off,
            _ => Self::Hide,
        }
    }
}

impl Default for Action {
    fn default() -> Self {
        Self::Hide
    }
}

/// 判定原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reason {
    Pass,
    Retained,
    Unreachable,
    SpeedLow,
    ResolutionLow,
    ResolutionHigh,
    DelayHigh,
    Supply,
    ProbeError,
    #[default]
    Unknown,
}

impl Reason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Retained => "retained",
            Self::Unreachable => "unreachable",
            Self::SpeedLow => "speed_low",
            Self::ResolutionLow => "resolution_low",
            Self::ResolutionHigh => "resolution_high",
            Self::DelayHigh => "delay_high",
            Self::Supply => "supply",
            Self::ProbeError => "probe_error",
            Self::Unknown => "unknown",
        }
    }

    /// 从结果记录恢复原因。未知值按 `Unknown`。
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "pass" => Self::Pass,
            "retained" => Self::Retained,
            "unreachable" => Self::Unreachable,
            "speed_low" => Self::SpeedLow,
            "resolution_low" => Self::ResolutionLow,
            "resolution_high" => Self::ResolutionHigh,
            "delay_high" => Self::DelayHigh,
            "supply" => Self::Supply,
            "probe_error" => Self::ProbeError,
            _ => Self::Unknown,
        }
    }
}

/// 一次测量的可用数据。没有测到的项为 `None`。
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub reachable: bool,
    pub probe_error: bool,
    pub http_status: Option<u16>,
    pub delay_ms: Option<u32>,
    pub speed_mib: Option<f64>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub error: Option<String>,
}

impl Default for Measurement {
    fn default() -> Self {
        Self {
            reachable: false,
            probe_error: false,
            http_status: None,
            delay_ms: None,
            speed_mib: None,
            width: None,
            height: None,
            error: None,
        }
    }
}

/// 逐字段继承后的有效规则。
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveRules {
    pub enabled: bool,
    pub action: Action,
    pub retain: bool,
    pub min_speed_mib: f64,
    /// 分辨率未知或映射未命中时的速率。0 表示使用 `min_speed_mib`。
    pub fallback_speed_mib: f64,
    pub max_delay_ms: u64,
    pub min_resolution_px: Option<u32>,
    pub max_resolution_px: Option<u32>,
    pub resolution_speed_map: BTreeMap<u32, f64>,
    pub supply: bool,
    pub host_cache: bool,
    pub stale_keep_secs: u64,
}

impl Default for EffectiveRules {
    fn default() -> Self {
        Self {
            enabled: false,
            action: Action::Hide,
            retain: false,
            min_speed_mib: 0.0,
            fallback_speed_mib: 0.0,
            max_delay_ms: 0,
            min_resolution_px: None,
            max_resolution_px: None,
            resolution_speed_map: BTreeMap::new(),
            supply: false,
            host_cache: false,
            stale_keep_secs: 0,
        }
    }
}

/// 一条频道的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub ok: bool,
    pub reachable: bool,
    pub reason: Reason,
}

/// 解析 `宽x高` 为像素数。空或格式错返回 `None`。
pub fn parse_resolution(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (width, height) = value.split_once('x')?;
    let width: u64 = width.parse().ok()?;
    let height: u64 = height.parse().ok()?;
    if !(1..=99_999).contains(&width) || !(1..=99_999).contains(&height) {
        return None;
    }
    u32::try_from(width * height).ok()
}

fn pick<T: Clone>(
    channel: Option<&QualityOverride>,
    group: Option<&QualityOverride>,
    source: Option<&QualityOverride>,
    field: impl Fn(&QualityOverride) -> Option<T>,
) -> Option<T> {
    channel
        .and_then(&field)
        .or_else(|| group.and_then(&field))
        .or_else(|| source.and_then(&field))
}

fn map_to_px(map: &BTreeMap<String, f64>) -> BTreeMap<u32, f64> {
    map.iter()
        .filter_map(|(key, value)| parse_resolution(key).map(|px| (px, *value)))
        .collect()
}

/// 解析频道的有效规则。频道 > 分组 > 源 > 全局。
pub fn effective_rules(config: &Config, group: &Group, channel: &Channel) -> EffectiveRules {
    let source = if channel.source_id.is_empty() {
        None
    } else {
        config
            .playlist
            .sources
            .iter()
            .find(|source| source.id == channel.source_id)
    };
    let channel_q = channel.quality.as_ref();
    let group_q = group.quality.as_ref();
    let source_q = source.and_then(|source| source.quality.as_ref());
    let rules = &config.quality.rules;

    let enabled = config.quality.enabled
        && pick(channel_q, group_q, source_q, |quality| quality.enabled).unwrap_or(true);

    EffectiveRules {
        enabled,
        action: Action::parse(
            &pick(channel_q, group_q, source_q, |quality| {
                quality.action.clone()
            })
            .unwrap_or_else(|| rules.action.clone()),
        ),
        retain: pick(channel_q, group_q, source_q, |quality| quality.retain)
            .unwrap_or(rules.retain),
        min_speed_mib: pick(channel_q, group_q, source_q, |quality| {
            quality.min_speed_mib
        })
        .unwrap_or(rules.min_speed_mib),
        fallback_speed_mib: pick(channel_q, group_q, source_q, |quality| {
            quality.fallback_speed_mib
        })
        .unwrap_or(rules.fallback_speed_mib),
        max_delay_ms: pick(channel_q, group_q, source_q, |quality| quality.max_delay_ms)
            .unwrap_or(rules.max_delay_ms),
        min_resolution_px: pick(channel_q, group_q, source_q, |quality| {
            quality.min_resolution.clone()
        })
        .or_else(|| (!rules.min_resolution.is_empty()).then(|| rules.min_resolution.clone()))
        .and_then(|value| parse_resolution(&value)),
        max_resolution_px: pick(channel_q, group_q, source_q, |quality| {
            quality.max_resolution.clone()
        })
        .or_else(|| (!rules.max_resolution.is_empty()).then(|| rules.max_resolution.clone()))
        .and_then(|value| parse_resolution(&value)),
        resolution_speed_map: pick(channel_q, group_q, source_q, |quality| {
            quality.resolution_speed_map.clone()
        })
        .map(|map| map_to_px(&map))
        .unwrap_or_else(|| map_to_px(&rules.resolution_speed_map)),
        supply: pick(channel_q, group_q, source_q, |quality| quality.supply)
            .unwrap_or(rules.supply),
        host_cache: pick(channel_q, group_q, source_q, |quality| quality.host_cache)
            .unwrap_or(rules.host_cache),
        stale_keep_secs: pick(channel_q, group_q, source_q, |quality| {
            quality.stale_keep_secs
        })
        .unwrap_or(config.quality.stale_keep_secs),
    }
}

/// 按有效规则判定一条测量结果。
pub fn evaluate(rules: &EffectiveRules, measurement: &Measurement) -> Verdict {
    if measurement.probe_error {
        return Verdict {
            ok: false,
            reachable: measurement.reachable,
            reason: Reason::ProbeError,
        };
    }
    if !measurement.reachable {
        return Verdict {
            ok: false,
            reachable: false,
            reason: Reason::Unreachable,
        };
    }

    let mut reason = None;
    if let (Some(width), Some(height)) = (measurement.width, measurement.height) {
        let px = width as u32 * height as u32;
        if rules.min_resolution_px.is_some_and(|min| px < min) {
            reason = Some(Reason::ResolutionLow);
        } else if rules.max_resolution_px.is_some_and(|max| px > max) {
            reason = Some(Reason::ResolutionHigh);
        }
    }

    if reason.is_none() {
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
        if threshold > 0.0 && measurement.speed_mib.unwrap_or(0.0) < threshold {
            reason = Some(Reason::SpeedLow);
        }
    }

    if reason.is_none() {
        if rules.max_delay_ms > 0 {
            if let Some(delay) = measurement.delay_ms {
                if u64::from(delay) > rules.max_delay_ms {
                    reason = Some(Reason::DelayHigh);
                }
            }
        }
    }

    match reason {
        Some(reason) => Verdict {
            ok: false,
            reachable: true,
            reason,
        },
        None => Verdict {
            ok: true,
            reachable: true,
            reason: Reason::Pass,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{Channel, Config, Group, PlaylistSource, QualityOverride};

    fn base() -> Config {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.playlist.sources.push(PlaylistSource {
            id: "src-a".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(1.0),
                ..QualityOverride::default()
            }),
            ..PlaylistSource::default()
        });
        config
    }

    #[test]
    fn channel_overrides_group_overrides_source_overrides_global() {
        let mut config = base();
        config.quality.rules.min_speed_mib = 0.5;
        let mut group = Group {
            id: "g".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(2.0),
                ..QualityOverride::default()
            }),
            ..Group::default()
        };
        group.channels.push(Channel {
            id: "c".into(),
            source_id: "src-a".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(3.0),
                ..QualityOverride::default()
            }),
            ..Channel::default()
        });
        config.groups.push(group);
        let g = &config.groups[0];
        let c = &g.channels[0];
        assert_eq!(effective_rules(&config, g, c).min_speed_mib, 3.0);

        let mut c2 = c.clone();
        c2.quality = None;
        assert_eq!(effective_rules(&config, g, &c2).min_speed_mib, 2.0);

        let mut g2 = g.clone();
        g2.quality = None;
        let c3 = g2.channels[0].clone();
        assert_eq!(effective_rules(&config, &g2, &c3).min_speed_mib, 3.0);
    }

    #[test]
    fn fallback_speed_inherits_channel_then_group_then_source_then_global() {
        let mut config = Config::default();
        config.quality.rules.fallback_speed_mib = 0.1;
        config.playlist.sources.push(PlaylistSource {
            id: "src-a".into(),
            quality: Some(QualityOverride {
                fallback_speed_mib: Some(0.2),
                ..QualityOverride::default()
            }),
            ..PlaylistSource::default()
        });
        let mut group = Group {
            id: "g".into(),
            quality: Some(QualityOverride {
                fallback_speed_mib: Some(0.4),
                ..QualityOverride::default()
            }),
            ..Group::default()
        };
        group.channels.push(Channel {
            id: "c".into(),
            source_id: "src-a".into(),
            quality: Some(QualityOverride {
                fallback_speed_mib: Some(0.8),
                ..QualityOverride::default()
            }),
            ..Channel::default()
        });
        config.groups.push(group);
        let g = &config.groups[0];
        let c = &g.channels[0];
        assert_eq!(
            effective_rules(&config, g, c).fallback_speed_mib,
            0.8,
            "频道层最高"
        );

        let mut c2 = c.clone();
        c2.quality = None;
        assert_eq!(
            effective_rules(&config, g, &c2).fallback_speed_mib,
            0.4,
            "频道未写时用分组层"
        );

        let mut g2 = g.clone();
        g2.quality = None;
        let mut c3 = g2.channels[0].clone();
        c3.quality = None;
        assert_eq!(
            effective_rules(&config, &g2, &c3).fallback_speed_mib,
            0.2,
            "分组与频道都未写时用来源层"
        );

        let mut c4 = c3.clone();
        c4.source_id.clear();
        assert_eq!(
            effective_rules(&config, &g2, &c4).fallback_speed_mib,
            0.1,
            "来源不匹配时用全局"
        );
    }

    #[test]
    fn source_rules_only_apply_to_matching_source_id() {
        let mut config = base();
        config.quality.rules.min_speed_mib = 0.5;
        let group = Group {
            id: "g".into(),
            channels: vec![
                Channel {
                    id: "a".into(),
                    source_id: "src-a".into(),
                    ..Channel::default()
                },
                Channel {
                    id: "manual".into(),
                    ..Channel::default()
                },
            ],
            ..Group::default()
        };
        assert_eq!(
            effective_rules(&config, &group, &group.channels[0]).min_speed_mib,
            1.0
        );
        assert_eq!(
            effective_rules(&config, &group, &group.channels[1]).min_speed_mib,
            0.5
        );
    }

    #[test]
    fn global_switch_is_a_hard_gate() {
        let mut config = base();
        config.quality.enabled = false;
        let group = Group {
            id: "g".into(),
            quality: Some(QualityOverride {
                enabled: Some(true),
                ..QualityOverride::default()
            }),
            ..Group::default()
        };
        let channel = Channel {
            id: "c".into(),
            ..Channel::default()
        };
        assert!(!effective_rules(&config, &group, &channel).enabled);
    }

    #[test]
    fn verdict_covers_thresholds_and_unknown_resolution() {
        let rules = EffectiveRules {
            enabled: true,
            action: Action::Hide,
            retain: false,
            min_speed_mib: 1.0,
            fallback_speed_mib: 0.0,
            max_delay_ms: 3000,
            min_resolution_px: Some(1280 * 720),
            max_resolution_px: Some(1920 * 1080),
            resolution_speed_map: BTreeMap::new(),
            supply: false,
            host_cache: true,
            stale_keep_secs: 1800,
        };
        let ok = Measurement {
            reachable: true,
            probe_error: false,
            http_status: Some(200),
            delay_ms: Some(100),
            speed_mib: Some(2.0),
            width: Some(1920),
            height: Some(1080),
            error: None,
        };
        assert!(evaluate(&rules, &ok).ok);

        let mut low = ok.clone();
        low.speed_mib = Some(0.5);
        assert_eq!(evaluate(&rules, &low).reason, Reason::SpeedLow);

        let mut slow = ok.clone();
        slow.delay_ms = Some(5000);
        assert_eq!(evaluate(&rules, &slow).reason, Reason::DelayHigh);

        let mut small = ok.clone();
        small.width = Some(1280);
        small.height = Some(720);
        assert_eq!(evaluate(&rules, &small).reason, Reason::Pass);

        let mut unknown = ok.clone();
        unknown.width = None;
        unknown.height = None;
        assert_eq!(
            evaluate(&rules, &unknown).reason,
            Reason::Pass,
            "分辨率未知应放行"
        );

        let mut parse = ok.clone();
        parse.probe_error = true;
        assert_eq!(evaluate(&rules, &parse).reason, Reason::ProbeError);
        assert!(!evaluate(&rules, &parse).ok);

        let mut high = ok.clone();
        high.width = Some(3840);
        high.height = Some(2160);
        assert_eq!(evaluate(&rules, &high).reason, Reason::ResolutionHigh);

        let down = Measurement {
            reachable: false,
            probe_error: false,
            http_status: None,
            delay_ms: None,
            speed_mib: None,
            width: None,
            height: None,
            error: Some("timeout".into()),
        };
        assert_eq!(evaluate(&rules, &down).reason, Reason::Unreachable);
    }

    #[test]
    fn supply_does_not_bypass_thresholds() {
        let rules = EffectiveRules {
            enabled: true,
            action: Action::Hide,
            retain: false,
            min_speed_mib: 1.0,
            fallback_speed_mib: 0.0,
            max_delay_ms: 0,
            min_resolution_px: None,
            max_resolution_px: None,
            resolution_speed_map: BTreeMap::new(),
            supply: true,
            host_cache: false,
            stale_keep_secs: 1800,
        };
        let slow = Measurement {
            reachable: true,
            probe_error: false,
            http_status: Some(200),
            delay_ms: Some(10),
            speed_mib: Some(0.1),
            width: None,
            height: None,
            error: None,
        };
        assert_eq!(
            evaluate(&rules, &slow).reason,
            Reason::SpeedLow,
            "supply 不再绕过阈值"
        );
    }

    #[test]
    fn resolution_speed_map_overrides_min_speed() {
        let mut map = BTreeMap::new();
        map.insert(1280 * 720, 1.0);
        map.insert(1920 * 1080, 3.0);
        let rules = EffectiveRules {
            enabled: true,
            min_speed_mib: 0.5,
            resolution_speed_map: map,
            ..EffectiveRules::default()
        };
        let measurement = |width: u16, height: u16, speed: f64| Measurement {
            reachable: true,
            width: Some(width),
            height: Some(height),
            speed_mib: Some(speed),
            ..Measurement::default()
        };
        assert_eq!(
            evaluate(&rules, &measurement(1920, 1080, 2.0)).reason,
            Reason::SpeedLow,
            "1080p 要求 3 MiB/s"
        );
        assert!(
            evaluate(&rules, &measurement(1280, 720, 2.0)).ok,
            "720p 只要求 1 MiB/s"
        );
        assert_eq!(
            evaluate(&rules, &measurement(640, 360, 0.4)).reason,
            Reason::SpeedLow,
            "未命中档位时使用最低速率"
        );
    }

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

    #[test]
    fn retain_does_not_bypass_thresholds() {
        let rules = EffectiveRules {
            enabled: true,
            action: Action::Hide,
            retain: true,
            min_speed_mib: 1.0,
            fallback_speed_mib: 0.0,
            max_delay_ms: 0,
            min_resolution_px: None,
            max_resolution_px: None,
            resolution_speed_map: BTreeMap::new(),
            supply: false,
            host_cache: true,
            stale_keep_secs: 1800,
        };
        let slow = Measurement {
            reachable: true,
            probe_error: false,
            http_status: Some(200),
            delay_ms: Some(10),
            speed_mib: Some(0.1),
            width: None,
            height: None,
            error: None,
        };
        assert_eq!(evaluate(&rules, &slow).reason, Reason::SpeedLow);
        let down = Measurement {
            reachable: false,
            probe_error: false,
            http_status: None,
            delay_ms: None,
            speed_mib: None,
            width: None,
            height: None,
            error: Some("x".into()),
        };
        assert_eq!(evaluate(&rules, &down).reason, Reason::Unreachable);
    }
}
