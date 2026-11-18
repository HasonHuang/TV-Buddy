//! 准入候选与有效规则。

use sha2::{Digest, Sha256};

use crate::{
    config::model::{Channel, Config, Group, PlaylistSource, ProxyCfg, QualityOverride},
    quality::rules::{effective_rules, EffectiveRules},
};

/// 候选来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateScope {
    Sync,
    Manual,
}

/// 一个待判定频道。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub scope: CandidateScope,
    pub source_id: String,
    pub source_key: String,
    pub gid: String,
    pub cid: String,
    pub name: String,
    pub target_group: String,
    pub url: String,
    pub existing_url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub proxy: Option<ProxyCfg>,
    pub rules: EffectiveRules,
    pub url_hash: String,
    pub is_url_change: bool,
    /// 跟随上游字段。固定频道只测量，不移出。
    pub sync_follow: bool,
    pub offline_on_fail: bool,
    pub interval_secs: u64,
    pub previous_first_fail_at: Option<i64>,
}

/// 解析候选的有效规则。现有频道优先；新候选使用目标分组。
pub fn candidate_rules(
    config: &Config,
    source: &PlaylistSource,
    existing: Option<(&Group, &Channel)>,
    target: Option<&Group>,
) -> EffectiveRules {
    if let Some((group, channel)) = existing {
        return effective_rules(config, group, channel);
    }
    let group = target.cloned().unwrap_or_default();
    let channel = Channel {
        source_id: source.id.clone(),
        ..Channel::default()
    };
    effective_rules(config, &group, &channel)
}

/// 探测设施的稳定签名。只包含全探针共用设置。
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

/// 有效规则与探测设置的稳定哈希。只包含新引擎读取的字段。
pub fn rule_hash(rules: &EffectiveRules, probe: &ProbeSignature) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"tvb-quality-rules-v2\0");
    hasher.update([u8::from(rules.enabled)]);
    hasher.update(rules.min_speed_mib.to_bits().to_le_bytes());
    hasher.update(rules.max_delay_ms.to_le_bytes());
    hasher.update(rules.min_resolution_px.unwrap_or(0).to_le_bytes());
    hasher.update(rules.max_resolution_px.unwrap_or(0).to_le_bytes());
    for (resolution, speed) in &rules.resolution_speed_map {
        hasher.update(resolution.to_le_bytes());
        hasher.update(speed.to_bits().to_le_bytes());
    }
    hasher.update([u8::from(rules.host_cache)]);
    hasher.update(rules.stale_keep_secs.to_le_bytes());
    hasher.update(rules.fallback_speed_mib.to_bits().to_le_bytes());
    hasher.update(probe.probe_mode.as_bytes());
    hasher.update([u8::from(probe.ffprobe), u8::from(probe.ffprobe_ready)]);
    hasher.update(probe.ffprobe_sample_kb.to_le_bytes());
    hasher.update(probe.ffprobe_min_speed_mib_bits.to_le_bytes());
    hasher.update(probe.sample_segments.to_le_bytes());
    hasher.update(probe.sample_bytes_kb.to_le_bytes());
    hasher.update(probe.sample_secs_bits.to_le_bytes());
    hex::encode(hasher.finalize())
}

/// URL 的十六进制哈希。忽略 `$描述` 后缀。
pub fn url_hash_hex(url: &str) -> String {
    let cleaned = url.split('$').next().unwrap_or(url).trim();
    hex::encode(Sha256::digest(cleaned.as_bytes()))
}

/// 有效规则的来源层级。
pub fn source_layer(config: &Config, group: &Group, channel: &Channel) -> &'static str {
    if channel.quality.as_ref().is_some_and(has_override) {
        return "channel";
    }
    if group.quality.as_ref().is_some_and(has_override) {
        return "group";
    }
    let source = config
        .playlist
        .sources
        .iter()
        .find(|source| source.id == channel.source_id);
    if source
        .and_then(|source| source.quality.as_ref())
        .is_some_and(has_override)
    {
        return "source";
    }
    "global"
}

fn has_override(value: &QualityOverride) -> bool {
    value.enabled.is_some()
        || value.min_speed_mib.is_some()
        || value.fallback_speed_mib.is_some()
        || value.max_delay_ms.is_some()
        || value.min_resolution.is_some()
        || value.max_resolution.is_some()
        || value.resolution_speed_map.is_some()
        || value.supply.is_some()
        || value.action.is_some()
        || value.retain.is_some()
        || value.host_cache.is_some()
        || value.stale_keep_secs.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::rules::EffectiveRules;

    #[test]
    fn candidate_rules_prefers_existing_channel_override() {
        let mut config = Config::default();
        config.quality.enabled = true;
        config.quality.rules.min_speed_mib = 1.0;
        let source = PlaylistSource {
            id: "s1".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(2.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let group = Group {
            id: "g".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(3.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let channel = Channel {
            id: "c".into(),
            source_id: "s1".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(4.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rules = candidate_rules(&config, &source, Some((&group, &channel)), None);
        assert_eq!(rules.min_speed_mib, 4.0);
    }

    #[test]
    fn candidate_rules_uses_target_group_for_new_channel() {
        let mut config = Config::default();
        config.quality.enabled = true;
        let source = PlaylistSource {
            id: "s1".into(),
            ..Default::default()
        };
        let target = Group {
            quality: Some(QualityOverride {
                min_speed_mib: Some(3.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rules = candidate_rules(&config, &source, None, Some(&target));
        assert_eq!(rules.min_speed_mib, 3.0);
    }

    #[test]
    fn fallback_override_counts_as_override() {
        let value = QualityOverride {
            fallback_speed_mib: Some(0.8),
            ..Default::default()
        };
        assert!(has_override(&value));
    }

    fn signature(config: &Config) -> ProbeSignature {
        ProbeSignature::from_config(config, false)
    }

    #[test]
    fn rule_hash_changes_with_threshold() {
        let mut rules = EffectiveRules {
            enabled: true,
            min_speed_mib: 1.0,
            ..EffectiveRules::default()
        };
        let probe = signature(&Config::default());
        let first = rule_hash(&rules, &probe);
        rules.min_speed_mib = 2.0;
        assert_ne!(first, rule_hash(&rules, &probe));
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
        assert_ne!(
            rule_hash(&EffectiveRules::default(), &a),
            rule_hash(&EffectiveRules::default(), &b)
        );
    }

    #[test]
    fn rule_hash_ignores_readiness_when_ffprobe_disabled() {
        let config = Config::default();
        assert!(!config.quality.ffprobe);
        let a = ProbeSignature::from_config(&config, false);
        let b = ProbeSignature::from_config(&config, true);
        assert_eq!(
            rule_hash(&EffectiveRules::default(), &a),
            rule_hash(&EffectiveRules::default(), &b)
        );
    }

    #[test]
    fn source_layer_prefers_channel_then_group_then_source() {
        let mut config = Config::default();
        let source = PlaylistSource {
            id: "s1".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(1.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        config.playlist.sources.push(source);
        let group = Group {
            quality: Some(QualityOverride {
                min_speed_mib: Some(2.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let channel = Channel {
            source_id: "s1".into(),
            quality: Some(QualityOverride {
                min_speed_mib: Some(3.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(source_layer(&config, &group, &channel), "channel");
        let no_channel = Channel {
            source_id: "s1".into(),
            ..Default::default()
        };
        assert_eq!(source_layer(&config, &group, &no_channel), "group");
        let no_group = Group::default();
        assert_eq!(source_layer(&config, &no_group, &no_channel), "source");
    }
}
