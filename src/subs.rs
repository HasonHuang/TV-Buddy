//! 订阅源预检：`/subs/{groups,preview}` 的纯逻辑。
//!
//! 证据：S-GAP-07、F-020。分组模式、全局排除、白/黑名单的先后顺序见 `trace`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::m3u::Entry;

/// 预检请求里的源参数。
#[derive(Debug, Clone, Default)]
pub struct PreviewSource {
    /// 源分组名 → 目标分组名；空值表示保留原名。
    pub group_map: BTreeMap<String, String>,
    /// `keep`（默认）、`map` 或 `merge`。
    pub group_mode: String,
    /// `merge` 模式的目标分组名；空值表示保留原名。
    pub merge_group: String,
    /// 未出现在 `group_map` 的分组：`drop` 或 `keep`（默认）。
    /// 映射为空时，`keep` / `merge` 模式保留全部分组。
    pub unlisted_groups: String,
    /// 频道名白名单；支持 `group:` 与 `id:` 前缀。
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

/// 分组选择与改名规则。预检和订阅聚合共用。
pub struct GroupRules<'a> {
    pub group_map: &'a BTreeMap<String, String>,
    pub group_mode: &'a str,
    pub merge_group: &'a str,
    pub unlisted_groups: &'a str,
}

impl GroupRules<'_> {
    /// 返回目标分组名；`None` 表示丢弃该分组。
    pub fn target(&self, original: &str) -> Option<String> {
        let mode = match self.group_mode.trim() {
            "map" => "map",
            "merge" => "merge",
            _ => "keep",
        };
        let merge = self.merge_group.trim();
        let Some(mapped) = self.group_map.get(original).map(|value| value.trim()) else {
            // 空映射没有选择集。保留原名与合并模式应导入全部分组；
            // 只有 map 模式把空映射当作「未选择任何分组」。
            let keep_all = self.group_map.is_empty() && mode != "map";
            if !keep_all && self.unlisted_groups.trim() != "keep" {
                return None;
            }
            return Some(if mode == "merge" && !merge.is_empty() {
                merge.to_string()
            } else {
                original.to_string()
            });
        };

        Some(match mode {
            "merge" if !merge.is_empty() => merge.to_string(),
            "map" if !mapped.is_empty() => mapped.to_string(),
            _ => original.to_string(),
        })
    }
}

/// 聚合源最近一次抓取状态。
///
/// 旧 `subs/status.json` 没有新字段，靠 `#[serde(default)]` 兼容。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceStatus {
    pub channels: usize,
    pub error: Option<String>,
    pub last_ok: u64,
    pub last_try: u64,
    /// `aggregate` 或 `sync`。
    pub mode: String,
    /// 该源当前物化分组数。
    pub groups: usize,
    /// 最近一次成功同步时间。
    pub last_sync: u64,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// 因 `sync_follow=false` 保留的数量。
    pub skipped_removals: usize,
    /// 空清单拦截等待确认。
    pub needs_review: bool,
    /// 最近一次因空清单未应用。
    pub zero_blocked: bool,
    /// 移除比例，千分比。
    pub shrink_permille: u64,
    pub duration_ms: u64,
    /// 下次计划时间。0 表示不自动。
    pub next_sync: u64,
    /// 正在同步。不落盘。
    #[serde(skip)]
    pub syncing: bool,
}

/// `ttl_secs = 0` 时目标按 1 小时展示与缓存。
pub fn effective_ttl_secs(ttl_secs: u64) -> u64 {
    if ttl_secs == 0 {
        3600
    } else {
        ttl_secs
    }
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
    pub after_exclude: usize,
}

/// 抽样条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub name: String,
    pub group: String,
    pub tvg_id: String,
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

/// 按分组统计条目；分组名空时记为 `(无分组)`。
pub fn group_counts(entries: &[Entry]) -> Vec<Count> {
    let mut map: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        *map.entry(group_of(entry)).or_insert(0) += 1;
    }
    map.into_iter()
        .map(|(name, count)| Count { name, count })
        .collect()
}

/// 预检：分组选择/改名 → 全局排除 → 源白名单 → 源黑名单。
pub fn preview(
    entries: &[Entry],
    source: &PreviewSource,
    global_exclude: &[String],
    sample_limit: usize,
) -> PreviewResult {
    let selected = select_entries(entries, source, global_exclude);

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for item in &selected.kept {
        *counts.entry(item.group.clone()).or_insert(0) += 1;
    }
    let out_groups = counts
        .into_iter()
        .map(|(name, count)| Count { name, count })
        .collect();

    let sample = selected
        .kept
        .iter()
        .take(sample_limit)
        .map(|item| Sample {
            name: item.entry.name.clone(),
            group: item.group.clone(),
            tvg_id: item.entry.tvg_id.clone().unwrap_or_default(),
        })
        .collect();

    PreviewResult {
        kept: selected.kept.len(),
        sample,
        trace: selected.trace,
        dropped_groups: selected.dropped_groups,
        out_groups,
    }
}

/// 选择结果。预检与同步共用。
#[derive(Debug, Clone)]
pub struct Selection<'a> {
    pub kept: Vec<Selected<'a>>,
    pub trace: Trace,
    pub dropped_groups: Vec<String>,
}

/// 四阶段过滤：分组选择/改名 → 全局排除 → 源白名单 → 源黑名单。
///
/// 预检与同步共用这一份规则，避免两边结论不同。
pub fn select_entries<'a>(
    entries: &'a [Entry],
    source: &PreviewSource,
    global_exclude: &[String],
) -> Selection<'a> {
    let total = entries.len();
    let rules = GroupRules {
        group_map: &source.group_map,
        group_mode: &source.group_mode,
        merge_group: &source.merge_group,
        unlisted_groups: &source.unlisted_groups,
    };

    // 阶段一：分组选择与改名。
    let mut selected = Vec::new();
    let mut dropped_groups: Vec<String> = Vec::new();
    for entry in entries {
        let original = group_of(entry);
        let Some(group) = rules.target(&original) else {
            if !dropped_groups.contains(&original) {
                dropped_groups.push(original);
            }
            continue;
        };
        selected.push(Selected {
            entry,
            original,
            group,
        });
    }
    let after_group = selected.len();

    // 阶段二：全局排除。
    let after_global: Vec<Selected<'_>> = selected
        .into_iter()
        .filter(|item| !matches_filter(global_exclude, item))
        .collect();
    let after_global_exclude = after_global.len();

    // 阶段三：源白名单。
    let after_include: Vec<Selected<'_>> = after_global
        .into_iter()
        .filter(|item| source.include.is_empty() || matches_filter(&source.include, item))
        .collect();
    let after_include_count = after_include.len();

    // 阶段四：源黑名单。
    let kept: Vec<Selected<'_>> = after_include
        .into_iter()
        .filter(|item| !matches_filter(&source.exclude, item))
        .collect();
    let kept_count = kept.len();

    Selection {
        kept,
        trace: Trace {
            total,
            after_group,
            after_global_exclude,
            after_include: after_include_count,
            after_exclude: kept_count,
        },
        dropped_groups,
    }
}

/// 已通过分组选择的条目。保留原分组以便 `group:` 过滤同时匹配新旧名称。
#[derive(Debug, Clone)]
pub struct Selected<'a> {
    pub entry: &'a Entry,
    pub original: String,
    pub group: String,
}

fn group_of(entry: &Entry) -> String {
    match entry.group_title.as_deref().map(str::trim) {
        Some(group) if !group.is_empty() => group.to_string(),
        _ => "(无分组)".to_string(),
    }
}

/// 过滤词命中：频道名包含、`group:` 新旧分组名包含、`id:` 精确匹配 `tvg-id`、
/// `url:` 精确匹配地址。
fn matches_filter(patterns: &[String], item: &Selected<'_>) -> bool {
    let id = item.entry.tvg_id.as_deref().unwrap_or_default();
    patterns
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| {
            if let Some(want) = pattern.strip_prefix("group:") {
                let want = want.trim().to_lowercase();
                item.original.to_lowercase().contains(&want)
                    || item.group.to_lowercase().contains(&want)
            } else if let Some(want) = pattern.strip_prefix("id:") {
                id.to_lowercase() == want.trim().to_lowercase()
            } else if let Some(want) = pattern.strip_prefix("idname:") {
                // `idname:<小写 tvg-id>\u{1f}<小写名称>`：同 id 多条时按名称区分。
                want.split_once('\u{1f}')
                    .is_some_and(|(want_id, want_name)| {
                        id.eq_ignore_ascii_case(want_id.trim())
                            && item.entry.name.trim().to_lowercase()
                                == want_name.trim().to_lowercase()
                    })
            } else if let Some(want) = pattern.strip_prefix("url:") {
                item.entry.url.trim() == want.trim()
            } else {
                item.entry
                    .name
                    .to_lowercase()
                    .contains(&pattern.to_lowercase())
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
            group_map: BTreeMap::from([("远端组".to_string(), "映射组".to_string())]),
            group_mode: "map".into(),
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
        assert_eq!(result.dropped_groups, vec!["别的组"]);
        assert_eq!(result.trace.after_group, 3);
    }

    #[test]
    fn keep_mode_ignores_mapped_names() {
        let source = PreviewSource {
            group_map: BTreeMap::from([("远端组".to_string(), "映射组".to_string())]),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.out_groups[0].name, "远端组");
        assert_eq!(result.dropped_groups, vec!["别的组"]);
    }

    #[test]
    fn merge_mode_unifies_selected_and_unlisted_groups() {
        let source = PreviewSource {
            group_map: BTreeMap::from([("远端组".to_string(), String::new())]),
            group_mode: "merge".into(),
            merge_group: "合并组".into(),
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.kept, 4);
        assert_eq!(
            result.out_groups,
            vec![Count {
                name: "合并组".into(),
                count: 4
            }]
        );
    }

    #[test]
    fn empty_map_keeps_all_in_keep_mode() {
        // 保留原名模式没有选择集，应导入全部分组，不再依赖 unlisted_groups。
        let source = PreviewSource::default();
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.kept, 4);
        assert!(result.dropped_groups.is_empty());
    }

    #[test]
    fn empty_map_in_map_mode_still_drops_unlisted() {
        // map 模式把空映射当作「未选择任何分组」。
        let source = PreviewSource {
            group_mode: "map".into(),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.kept, 0);
        assert_eq!(result.dropped_groups, vec!["远端组", "别的组"]);
    }

    #[test]
    fn filters_stage_by_stage() {
        let source = PreviewSource {
            include: vec!["group:远端组".to_string()],
            exclude: vec!["id:skip".to_string()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &["group:别的组".to_string()], 5);
        assert_eq!(result.trace.total, 4);
        assert_eq!(result.trace.after_group, 4);
        assert_eq!(result.trace.after_global_exclude, 3);
        assert_eq!(result.trace.after_include, 3);
        assert_eq!(result.trace.after_exclude, 2);
        assert_eq!(result.kept, 2);
        assert_eq!(result.sample.len(), 2);
        assert_eq!(result.sample[0].name, "CCTV-1");
        assert_eq!(result.sample[0].tvg_id, "a");
    }

    #[test]
    fn group_filter_matches_original_and_mapped_names() {
        let mut source = PreviewSource {
            group_map: BTreeMap::from([("远端组".to_string(), "映射组".to_string())]),
            group_mode: "map".into(),
            include: vec!["group:远端组".to_string()],
            ..PreviewSource::default()
        };
        assert_eq!(preview(&sample_entries(), &source, &[], 5).kept, 3);

        source.include = vec!["group:映射组".to_string()];
        assert_eq!(preview(&sample_entries(), &source, &[], 5).kept, 3);
    }

    #[test]
    fn ungrouped_entries_use_target_label() {
        let entries = vec![Entry {
            name: "无分组频道".into(),
            url: "http://up/u".into(),
            ..Entry::default()
        }];
        let source = PreviewSource {
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        let result = preview(&entries, &source, &[], 5);
        assert_eq!(result.sample[0].group, "(无分组)");
        assert_eq!(result.sample[0].tvg_id, "");
    }
}
