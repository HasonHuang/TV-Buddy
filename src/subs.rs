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
    /// 上游分组名 → 已有分组 id。显式目标，不参与「映射外丢弃」。
    pub merge_into: BTreeMap<String, String>,
    /// 上游分组名 → 新组名。显式目标，不参与「映射外丢弃」。
    pub merge_new: BTreeMap<String, String>,
    /// 未出现在 `group_map` 的分组：`drop` 或 `keep`。空值按 `drop` 处理。
    pub unlisted_groups: String,
    /// 频道名白名单；支持 `group:` / `id:` / `url:` 前缀与 `*`、`?` 通配符。
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl PreviewSource {
    /// 与 `PlaylistSource::normalize` 的未列出分组规则保持一致。
    pub fn normalize(&mut self) {
        if self.unlisted_groups.trim().is_empty() {
            self.unlisted_groups = "keep".to_string();
        }
        if self.unlisted_groups == "drop"
            && self.group_map.is_empty()
            && self.group_mode.trim() != "map"
            && self.merge_into.is_empty()
            && self.merge_new.is_empty()
        {
            self.unlisted_groups = "keep".to_string();
        }
    }
}

/// 分组选择与改名规则。预检和订阅聚合共用。
pub struct GroupRules<'a> {
    pub group_map: &'a BTreeMap<String, String>,
    pub group_mode: &'a str,
    pub merge_group: &'a str,
    pub merge_into: &'a BTreeMap<String, String>,
    pub merge_new: &'a BTreeMap<String, String>,
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
        // 显式目标不参与「映射外丢弃」。`merge_into` 优先，
        // 它的目标组名要到同步阶段才解析，这里保留常规名称。
        let merge_into = self
            .merge_into
            .get(original)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let merge_new = self
            .merge_new
            .get(original)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let explicit = merge_into.is_some() || merge_new.is_some();
        if merge_into.is_none() {
            if let Some(name) = merge_new {
                return Some(name.to_string());
            }
        }
        let Some(mapped) = self.group_map.get(original).map(|value| value.trim()) else {
            if !explicit && self.unlisted_groups.trim() != "keep" {
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
    /// 已排入手动同步队列，等待同步锁。不落盘。
    #[serde(skip)]
    pub queued: bool,
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
        merge_into: &source.merge_into,
        merge_new: &source.merge_new,
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
        .filter(|item| !matches_filter(global_exclude, &item.filter_item()))
        .collect();
    let after_global_exclude = after_global.len();

    // 阶段三：源白名单。
    let after_include: Vec<Selected<'_>> = after_global
        .into_iter()
        .filter(|item| {
            source.include.is_empty() || matches_filter(&source.include, &item.filter_item())
        })
        .collect();
    let after_include_count = after_include.len();

    // 阶段四：源黑名单。
    let kept: Vec<Selected<'_>> = after_include
        .into_iter()
        .filter(|item| !matches_filter(&source.exclude, &item.filter_item()))
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

impl Selected<'_> {
    fn filter_item(&self) -> FilterItem<'_> {
        FilterItem {
            name: &self.entry.name,
            original: &self.original,
            group: &self.group,
            tvg_id: self.entry.tvg_id.as_deref().unwrap_or_default(),
            url: &self.entry.url,
        }
    }
}

/// 过滤需要的频道字段。预检、托管同步与聚合共用。
pub(crate) struct FilterItem<'a> {
    pub name: &'a str,
    pub original: &'a str,
    pub group: &'a str,
    pub tvg_id: &'a str,
    pub url: &'a str,
}

fn group_of(entry: &Entry) -> String {
    match entry.group_title.as_deref().map(str::trim) {
        Some(group) if !group.is_empty() => group.to_string(),
        _ => "(无分组)".to_string(),
    }
}

/// 过滤词命中规则。
///
/// 不含 `*` 或 `?` 时保持原语义：裸词与 `group:` 是子串，`id:` 是精确匹配。
/// 含通配符时按整串匹配。`idname:` 与 `url:` 始终精确匹配。
pub(crate) fn matches_filter(patterns: &[String], item: &FilterItem<'_>) -> bool {
    patterns
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| {
            if let Some(want) = pattern.strip_prefix("group:") {
                let want = want.trim();
                glob_or_contains(want, item.original) || glob_or_contains(want, item.group)
            } else if let Some(want) = pattern.strip_prefix("id:") {
                glob_or_equals(want.trim(), item.tvg_id)
            } else if let Some(want) = pattern.strip_prefix("idname:") {
                // `idname:<小写 tvg-id>\u{1f}<小写名称>`：同 id 多条时按名称区分。
                want.split_once('\u{1f}')
                    .is_some_and(|(want_id, want_name)| {
                        item.tvg_id.eq_ignore_ascii_case(want_id.trim())
                            && item.name.trim().to_lowercase() == want_name.trim().to_lowercase()
                    })
            } else if let Some(want) = pattern.strip_prefix("url:") {
                item.url.trim() == want.trim()
            } else {
                glob_or_contains(pattern, item.name)
            }
        })
}

/// 模式是否含通配符。
fn has_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// 子串匹配；含通配符时改为整串 glob。
fn glob_or_contains(pattern: &str, text: &str) -> bool {
    if has_wildcard(pattern) {
        wildcard_match(pattern, text)
    } else {
        text.to_lowercase().contains(&pattern.to_lowercase())
    }
}

/// 精确匹配；含通配符时改为整串 glob。
fn glob_or_equals(pattern: &str, text: &str) -> bool {
    if has_wildcard(pattern) {
        wildcard_match(pattern, text)
    } else {
        text.to_lowercase() == pattern.to_lowercase()
    }
}

enum WildcardToken {
    Char(char),
    One,
    Many,
}

/// 简单 glob：`*` 匹配任意长度，`?` 匹配一个 Unicode 字符。
///
/// `\*`、`\?`、`\\` 表示字面字符。其它反斜杠按字面保留。
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = compile_wildcard(pattern);
    let text: Vec<char> = text.to_lowercase().chars().collect();
    wildcard_tokens(&pattern, &text)
}

fn compile_wildcard(pattern: &str) -> Vec<WildcardToken> {
    let mut tokens = Vec::new();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('*') => push_literal(&mut tokens, '*'),
                Some('?') => push_literal(&mut tokens, '?'),
                Some('\\') => push_literal(&mut tokens, '\\'),
                Some(other) => {
                    push_literal(&mut tokens, '\\');
                    push_literal(&mut tokens, other);
                }
                None => push_literal(&mut tokens, '\\'),
            },
            '*' => tokens.push(WildcardToken::Many),
            '?' => tokens.push(WildcardToken::One),
            other => push_literal(&mut tokens, other),
        }
    }
    tokens
}

fn push_literal(tokens: &mut Vec<WildcardToken>, ch: char) {
    for lower in ch.to_lowercase() {
        tokens.push(WildcardToken::Char(lower));
    }
}

fn wildcard_tokens(pattern: &[WildcardToken], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<usize> = None;
    let mut mark = 0;
    while t < text.len() {
        match pattern.get(p) {
            Some(WildcardToken::Char(ch)) if *ch == text[t] => {
                p += 1;
                t += 1;
            }
            Some(WildcardToken::One) => {
                p += 1;
                t += 1;
            }
            Some(WildcardToken::Many) => {
                star = Some(p);
                mark = t;
                p += 1;
            }
            _ => {
                let Some(star_pos) = star else {
                    return false;
                };
                p = star_pos + 1;
                mark += 1;
                t = mark;
            }
        }
    }
    while matches!(pattern.get(p), Some(WildcardToken::Many)) {
        p += 1;
    }
    p == pattern.len()
}

/// 转义通配符字符。自动生成的 `id:` 排除键使用。
pub(crate) fn escape_glob(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '*' | '?' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
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
    fn normalize_keeps_drop_with_explicit_target() {
        let mut source = PreviewSource::default();
        source.unlisted_groups = "drop".into();
        source.merge_new.insert("新闻".into(), "合集".into());
        source.normalize();
        assert_eq!(source.unlisted_groups, "drop");
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
    fn merge_new_changes_preview_group_name() {
        let source = PreviewSource {
            merge_new: BTreeMap::from([("远端组".to_string(), "新组".to_string())]),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.kept, 3);
        assert_eq!(
            result.out_groups,
            vec![Count {
                name: "新组".into(),
                count: 3
            }]
        );
        assert_eq!(
            result
                .sample
                .iter()
                .filter(|sample| sample.group == "新组")
                .count(),
            3
        );
        assert_eq!(result.dropped_groups, vec!["别的组"]);
    }

    #[test]
    fn merge_into_priority_wins_in_preview() {
        let source = PreviewSource {
            merge_into: BTreeMap::from([("远端组".to_string(), "g".to_string())]),
            merge_new: BTreeMap::from([("远端组".to_string(), "新组".to_string())]),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(
            result.out_groups,
            vec![Count {
                name: "远端组".into(),
                count: 3
            }],
            "merge_into 优先，不应显示 merge_new 的组名"
        );
    }

    #[test]
    fn merge_into_bypasses_unlisted_drop() {
        let source = PreviewSource {
            merge_into: BTreeMap::from([("远端组".to_string(), "g".to_string())]),
            unlisted_groups: "drop".into(),
            ..PreviewSource::default()
        };
        let result = preview(&sample_entries(), &source, &[], 5);
        assert_eq!(result.kept, 3);
        assert_eq!(
            result.out_groups,
            vec![Count {
                name: "远端组".into(),
                count: 3
            }]
        );
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
    fn empty_map_keeps_all_when_unlisted_kept() {
        // 保留原名模式 + 保留映射外分组：导入全部分组。
        let source = PreviewSource {
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
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
    fn preview_source_normalizes_like_saved_source() {
        let mut source = PreviewSource::default();
        source.group_mode = "keep".into();
        source.unlisted_groups = "drop".into();
        source.normalize();
        assert_eq!(source.unlisted_groups, "keep");

        let mut mapped = PreviewSource::default();
        mapped.group_mode = "map".into();
        mapped.unlisted_groups = "drop".into();
        mapped.normalize();
        assert_eq!(mapped.unlisted_groups, "drop");
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

    fn wildcard_entries() -> Vec<Entry> {
        vec![
            entry("CCTV-1", "港台", "a"),
            entry("CCTV-10", "港台", "b"),
            entry("翡翠台(字幕)", "港台", "c"),
            entry("凤凰卫视", "香港", "d"),
        ]
    }

    #[test]
    fn wildcard_matches_full_string() {
        assert!(wildcard_match("CCTV-?", "cctv-1"));
        assert!(!wildcard_match("CCTV-?", "CCTV-10"));
        assert!(wildcard_match("翡翠*", "翡翠台(字幕)"));
        assert!(wildcard_match("*体育*", "CCTV-5体育"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*b*c", "axxbyyc"));
        assert!(wildcard_match("a\\?b", "a?b"));
        assert!(!wildcard_match("a\\?b", "axb"));
        assert!(wildcard_match("a\\*b", "a*b"));
        assert!(wildcard_match(&escape_glob("a?b"), "a?b"));
        assert!(!wildcard_match(&escape_glob("a?b"), "axb"));
    }

    #[test]
    fn preview_filters_with_wildcards() {
        let source = PreviewSource {
            include: vec!["CCTV-?".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        let result = preview(&wildcard_entries(), &source, &[], 5);
        assert_eq!(result.kept, 1);
        assert_eq!(result.sample[0].name, "CCTV-1");

        let source = PreviewSource {
            include: vec!["*翡翠*".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&wildcard_entries(), &source, &[], 5).kept, 1);

        let source = PreviewSource {
            include: vec!["group:港*".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&wildcard_entries(), &source, &[], 5).kept, 3);

        let source = PreviewSource {
            include: vec!["cctv*".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&wildcard_entries(), &source, &[], 5).kept, 2);

        let source = PreviewSource {
            include: vec!["*".into()],
            exclude: vec!["CCTV-*".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&wildcard_entries(), &source, &[], 5).kept, 2);

        let source = PreviewSource {
            exclude: vec!["*".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&wildcard_entries(), &source, &[], 5).kept, 0);
    }

    #[test]
    fn wildcard_id_and_exact_url_and_idname() {
        let source = PreviewSource {
            include: vec!["id:a*".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&wildcard_entries(), &source, &[], 5).kept, 1);

        let source = PreviewSource {
            include: vec!["id:a?".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&wildcard_entries(), &source, &[], 5).kept, 0);

        let entries = vec![Entry {
            name: "带参数".into(),
            url: "http://up/x?y=1".into(),
            tvg_id: Some("q".into()),
            group_title: Some("组".into()),
            ..Entry::default()
        }];
        let source = PreviewSource {
            exclude: vec!["url:http://up/x?y=1".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&entries, &source, &[], 5).kept, 0);

        let source = PreviewSource {
            include: vec!["url:http://up/x?y=2".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&entries, &source, &[], 5).kept, 0);

        let source = PreviewSource {
            exclude: vec!["idname:q\u{1f}带参数".into()],
            unlisted_groups: "keep".into(),
            ..PreviewSource::default()
        };
        assert_eq!(preview(&entries, &source, &[], 5).kept, 0);
    }
}
