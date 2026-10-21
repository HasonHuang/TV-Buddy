//! 订阅源预检：`/subs/{groups,preview}` 的纯逻辑。
//!
//! 证据：S-GAP-07。分组映射、全局排除、白/黑名单的先后顺序见 `trace`。

use std::collections::BTreeMap;

use crate::m3u::Entry;

/// 预检请求里的源参数。
#[derive(Debug, Clone, Default)]
pub struct PreviewSource {
    /// 源分组名 → 目标分组名；`None` 表示丢弃。
    pub group_map: Vec<(String, Option<String>)>,
    /// 未出现在 `group_map` 的分组：`drop`（默认）或 `keep`。
    pub unlisted_groups: String,
    /// 频道名白名单；支持 `group:` 与 `id:` 前缀。
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

/// 分组与条目数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Count {
    pub name: String,
    pub count: usize,
}

/// 各阶段剩余条目数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    pub total: usize,
    pub after_group: usize,
    pub after_global_exclude: usize,
    pub after_include: usize,
}

/// 抽样条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub name: String,
    pub group: String,
    pub url: String,
}

/// 预检结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewResult {
    pub kept: usize,
    pub sample: Vec<Sample>,
    pub trace: Trace,
    pub dropped_groups: Vec<String>,
    pub out_groups: Vec<Count>,
}

/// 按分组统计条目；分组名空时记为 `未分组`。
pub fn group_counts(entries: &[Entry]) -> Vec<Count> {
    let mut map: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        *map.entry(group_of(entry)).or_insert(0) += 1;
    }
    map.into_iter()
        .map(|(name, count)| Count { name, count })
        .collect()
}

/// 预检：分组映射 → 全局排除 → 源白/黑名单。
pub fn preview(
    entries: &[Entry],
    source: &PreviewSource,
    global_exclude: &[String],
    sample_limit: usize,
) -> PreviewResult {
    let total = entries.len();
    let has_map = !source.group_map.is_empty();
    let drop_unlisted = source.unlisted_groups.trim() != "keep";

    // 阶段一：分组映射。没有映射时保留全部。
    let mut mapped: Vec<(&Entry, String)> = Vec::new();
    let mut dropped_groups: Vec<String> = Vec::new();
    for entry in entries {
        let group = group_of(entry);
        if !has_map {
            mapped.push((entry, group));
            continue;
        }
        match source.group_map.iter().find(|(from, _)| from == &group) {
            Some((_, Some(target))) => mapped.push((entry, target.clone())),
            Some((_, None)) => {
                if !dropped_groups.contains(&group) {
                    dropped_groups.push(group);
                }
            }
            None => {
                if !drop_unlisted {
                    mapped.push((entry, group));
                }
            }
        }
    }
    let after_group = mapped.len();

    // 阶段二：全局排除。
    let mapped: Vec<(&Entry, String)> = mapped
        .into_iter()
        .filter(|(entry, group)| !matches_filter(global_exclude, entry, group))
        .collect();
    let after_global_exclude = mapped.len();

    // 阶段三：源白名单与黑名单。
    let kept: Vec<(&Entry, String)> = mapped
        .into_iter()
        .filter(|(entry, group)| {
            (source.include.is_empty() || matches_filter(&source.include, entry, group))
                && !matches_filter(&source.exclude, entry, group)
        })
        .collect();
    let after_include = kept.len();

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, group) in &kept {
        *counts.entry(group.clone()).or_insert(0) += 1;
    }
    let out_groups = counts
        .into_iter()
        .map(|(name, count)| Count { name, count })
        .collect();

    let sample = kept
        .iter()
        .take(sample_limit)
        .map(|(entry, group)| Sample {
            name: entry.name.clone(),
            group: group.clone(),
            url: entry.url.clone(),
        })
        .collect();

    PreviewResult {
        kept: after_include,
        sample,
        trace: Trace {
            total,
            after_group,
            after_global_exclude,
            after_include,
        },
        dropped_groups,
        out_groups,
    }
}

fn group_of(entry: &Entry) -> String {
    match entry.group_title.as_deref().map(str::trim) {
        Some(group) if !group.is_empty() => group.to_string(),
        _ => "未分组".to_string(),
    }
}

/// 过滤词命中：频道名包含、`group:` 分组包含、`id:` 精确匹配 `tvg-id`。
fn matches_filter(patterns: &[String], entry: &Entry, group: &str) -> bool {
    let id = entry.tvg_id.as_deref().unwrap_or_default();
    patterns
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| {
            if let Some(want) = pattern.strip_prefix("group:") {
                group.to_lowercase().contains(&want.trim().to_lowercase())
            } else if let Some(want) = pattern.strip_prefix("id:") {
                id.to_lowercase() == want.trim().to_lowercase()
            } else {
                entry.name.to_lowercase().contains(&pattern.to_lowercase())
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, group: &str, id: &str) -> Entry {
        Entry {
            name: name.to_string(),
            url: format!("http://up/{name}"),
            tvg_id: Some(id.to_string()),
            group_title: Some(group.to_string()),
            ..Entry::default()
        }
    }

    fn sample_entries() -> Vec<Entry> {
        vec![
            entry("CCTV-1", "远端组", "a"),
            entry("CCTV-5", "远端组", "b"),
            entry("跳过", "远端组", "skip"),
            entry("其他", "别的组", "c"),
        ]
    }

    #[test]
    fn counts_by_group() {
        assert_eq!(
            group_counts(&sample_entries()),
            vec![
                Count {
                    name: "别的组".into(),
                    count: 1
                },
                Count {
                    name: "远端组".into(),
                    count: 3
                },
            ]
        );
    }

    #[test]
    fn maps_groups_and_drops_unlisted() {
        let source = PreviewSource {
            group_map: vec![("远端组".to_string(), Some("映射组".to_string()))],
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.kept, 3);
        assert_eq!(
            result.out_groups,
            vec![Count {
                name: "映射组".into(),
                count: 3
            }]
        );
        assert!(result.dropped_groups.is_empty());
        assert_eq!(result.trace.after_group, 3);
    }

    #[test]
    fn keeps_unlisted_when_requested() {
        let source = PreviewSource {
            group_map: vec![("远端组".to_string(), None)],
            unlisted_groups: "keep".to_string(),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.kept, 1);
        assert_eq!(result.dropped_groups, vec!["远端组"]);
        assert_eq!(result.trace.total, 4);
        assert_eq!(result.trace.after_group, 1);
    }

    #[test]
    fn filters_stage_by_stage() {
        let source = PreviewSource {
            include: vec!["group:远端组".to_string()],
            exclude: vec!["id:skip".to_string()],
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &["group:别的组".to_string()], 5);
        assert_eq!(result.trace.total, 4);
        assert_eq!(result.trace.after_global_exclude, 3);
        assert_eq!(result.trace.after_include, 2);
        assert_eq!(result.kept, 2);
        assert_eq!(result.sample.len(), 2);
        assert_eq!(result.sample[0].name, "CCTV-1");
    }
}
