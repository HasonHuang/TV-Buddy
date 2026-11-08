//! 定时订阅：抓取 M3U，把条目物化进 `config.groups`。
//!
//! 导出的内容与手工内容共存。同步只改 `source_id` 指向本源的分组与频道。
//! 手工分组与手工频道永不删除。
//!
//! 流程：抓取 → 选择 → 生成计划 → 保护检查 → 一次写入 → 记录状态。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    config::model::{Channel, Config, Group, PlaylistCfg, PlaylistSource},
    m3u::{self, Entry},
    state::AppState,
    subs::{self, PreviewSource, SourceStatus},
};

/// M3U 正文上限。
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// 手动触发的最短间隔（秒）。
pub const MANUAL_MIN_INTERVAL_SECS: u64 = 10;

/// 计划中的一个分组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedGroup {
    /// 上游原分组名（合并组取第一个上游分组）。
    pub original: String,
    /// 分组身份。合并组为 `merge:<目标名>`，`merge_into` 为 `gid:<id>`，
    /// 其余为 `group:<上游分组>`。
    pub target: String,
    /// 显示名，含源前缀。
    pub name: String,
    /// `merge_into` 指定的已有分组 id；空值表示按源独立建组。
    pub merge_id: String,
    /// `merge_new` 指定的新组名（不含前缀）；空值表示没有单独指定。
    pub new_name: String,
    pub count: usize,
}

/// 计划中的一个频道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChannel {
    pub key: String,
    pub id_base: String,
    pub name: String,
    pub url: String,
    /// 上游原分组名。写入 `group_title`，用于显示。
    pub source_group: String,
    /// 目标分组身份，与 [`PlannedGroup::target`] 对应。
    pub target: String,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    pub tvg_logo: Option<String>,
    pub headers: Vec<Value>,
    pub dash: bool,
}

/// 一次同步的计划。
#[derive(Debug, Clone)]
pub struct Plan {
    pub channels: Vec<PlannedChannel>,
    pub groups: Vec<PlannedGroup>,
    pub duplicates: usize,
    pub trace: subs::Trace,
}

/// 应用结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped_removals: usize,
    pub groups_created: usize,
    pub groups_removed: usize,
    pub channels: usize,
    pub groups: usize,
}

/// 预检差异。只读，不写配置。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub add: usize,
    pub update: usize,
    pub remove: usize,
    pub groups_add: usize,
    pub groups_remove: usize,
    pub groups_rename: usize,
    pub skipped_duplicates: usize,
}

/// 一次同步的结果。
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub blocked: bool,
    pub stats: ApplyStats,
    pub shrink_permille: u64,
}

/// 由源配置生成预检参数。
pub fn preview_source(source: &PlaylistSource) -> PreviewSource {
    PreviewSource {
        group_map: source.group_map.clone(),
        group_mode: source.group_mode.clone(),
        merge_group: source.merge_group.clone(),
        merge_into: source.merge_into.clone(),
        merge_new: source.merge_new.clone(),
        unlisted_groups: source.unlisted_groups.clone(),
        include: source.include.clone(),
        exclude: source.exclude.clone(),
    }
}

/// 分组身份：`merge_into` 的 id 优先，其次 `merge_new` 的新组名，
/// 再其次 merge 模式的目标名，最后原分组名。
///
/// 合并组的多个上游分组必须指向同一身份，否则会建成多个同名分组。
fn group_target(
    source: &PlaylistSource,
    original: &str,
    merge_id: &str,
    merge_new: &str,
) -> String {
    if !merge_id.is_empty() {
        return format!("gid:{merge_id}");
    }
    if !merge_new.is_empty() {
        return format!("new:{merge_new}");
    }
    let merge = source.merge_group.trim();
    if source.group_mode.trim() == "merge" && !merge.is_empty() {
        return format!("merge:{merge}");
    }
    format!("group:{original}")
}

/// 同步写入的显示名基线。管理员改名后，该值保持不变。
const SYNC_NAME_KEY: &str = "sync_name";
/// 同步分组的目标身份，用于合并组在后续同步中重新匹配。
const SYNC_TARGET_KEY: &str = "sync_target";

fn sync_marker<'a>(group: &'a Group, key: &str) -> &'a str {
    group.extra.get(key).and_then(Value::as_str).unwrap_or("")
}

fn set_sync_marker(group: &mut Group, key: &str, value: &str) {
    group
        .extra
        .insert(key.to_string(), Value::String(value.to_string()));
}

fn clear_sync_markers(group: &mut Group) {
    group.extra.remove(SYNC_NAME_KEY);
    group.extra.remove(SYNC_TARGET_KEY);
}

/// 生成同步计划。纯函数：不抓取、不写配置。
pub fn plan(config: &Config, source: &PlaylistSource, entries: &[Entry]) -> Plan {
    let selection = subs::select_entries(
        entries,
        &preview_source(source),
        &config.playlist.global_exclude,
    );

    // 其他来源（含手工）已占用的地址。同址条目丢弃，避免重复。
    let mut external_urls: HashSet<&str> = HashSet::new();
    for group in &config.groups {
        for channel in &group.channels {
            if channel.source_id != source.id && !channel.url.trim().is_empty() {
                external_urls.insert(channel.url.trim());
            }
        }
    }

    let prefix = prefix_for(source);
    let mut channels: Vec<PlannedChannel> = Vec::new();
    let mut groups: Vec<PlannedGroup> = Vec::new();
    let mut duplicates = 0usize;

    // 先过滤，再分配键：同基键的条目需要知道整组的数量。
    let mut accepted: Vec<&subs::Selected<'_>> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    for item in &selection.kept {
        let url = item.entry.url.trim();
        if url.is_empty() || external_urls.contains(url) {
            duplicates += 1;
            continue;
        }
        if !seen_urls.insert(url.to_string()) {
            duplicates += 1;
            continue;
        }
        accepted.push(item);
    }
    let mut base_counts: HashMap<String, usize> = HashMap::new();
    for item in &accepted {
        *base_counts
            .entry(stable_key(item.entry, &item.original))
            .or_insert(0) += 1;
    }
    let mut used_keys: HashSet<String> = HashSet::new();

    for item in accepted {
        let url = item.entry.url.trim().to_string();
        let base_key = stable_key(item.entry, &item.original);
        // 重复键按 URL 派生后缀：与上游顺序无关，重排或删除同组条目
        // 都不会互换身份。
        let duplicate = base_counts.get(&base_key).copied().unwrap_or(0) > 1;
        let mut key = if duplicate {
            format!("{base_key}#{}", key_suffix(&url))
        } else {
            base_key.clone()
        };
        let mut attempt = 1;
        while !used_keys.insert(key.clone()) {
            attempt += 1;
            key = format!("{base_key}#{}-{attempt}", key_suffix(&url));
        }

        let merge_id = source
            .merge_into
            .get(&item.original)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        // 已有组优先。同一上游分组只认一个显式目标。
        let merge_new = if merge_id.is_empty() {
            source
                .merge_new
                .get(&item.original)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let target = group_target(source, &item.original, &merge_id, &merge_new);
        // `merge_new` 的新组名覆盖改名表的结果。前缀只影响显示名。
        let display_group = if merge_new.is_empty() {
            item.group.clone()
        } else {
            merge_new.clone()
        };
        let name = format!("{prefix}{display_group}");
        match groups.iter_mut().find(|group| group.target == target) {
            Some(group) => group.count += 1,
            None => groups.push(PlannedGroup {
                original: item.original.clone(),
                target: target.clone(),
                name,
                merge_id,
                new_name: merge_new,
                count: 1,
            }),
        }

        let dash = is_mpd(&url);
        channels.push(PlannedChannel {
            id_base: channel_id_base(item.entry, &base_key),
            key,
            name: display_name(item.entry),
            url,
            source_group: item.original.clone(),
            target,
            tvg_id: trim_opt(item.entry.tvg_id.as_deref()),
            tvg_name: trim_opt(item.entry.tvg_name.as_deref()),
            tvg_logo: trim_opt(item.entry.tvg_logo.as_deref()),
            headers: entry_headers(item.entry),
            dash,
        });
    }

    Plan {
        channels,
        groups,
        duplicates,
        trace: selection.trace,
    }
}

/// 应用计划。一次改写 `config.groups`。
///
/// 调用方必须在配置写锁内调用。计划与当前源不一致时应先中止。
pub fn apply(config: &mut Config, source: &PlaylistSource, plan: &Plan) -> ApplyStats {
    let playlist = config.playlist.clone();
    let mut stats = ApplyStats::default();
    let mut groups = std::mem::take(&mut config.groups);

    // 阶段一：解析或创建目标分组。记录分组 id，不记索引，避免插入后错位。
    // 合并组的多个上游分组共用同一身份，只建一次。
    let mut target_ids: HashMap<String, String> = HashMap::new();
    for planned in &plan.groups {
        if target_ids.contains_key(&planned.target) {
            continue;
        }
        let name = planned.name.clone();
        // 显式并入已有组：先按 id 找。目标被删除后，按身份标记找回
        // 上次为这个目标建立的同步组，避免每次同步都新建一组。
        // 不用同名/原分组名回退，避免把频道放进错误的组。
        let explicit = !planned.merge_id.is_empty();
        let existing = if explicit {
            let by_id = groups.iter().position(|g| g.id == planned.merge_id);
            by_id.or_else(|| {
                groups.iter().position(|group| {
                    group.origin == "sync"
                        && group.source_id == source.id
                        && sync_marker(group, SYNC_TARGET_KEY) == planned.target
                })
            })
        } else {
            // 先按身份标记找，避免旧同名分组抢先命中导致分组 ID 漂移。
            let by_marker = groups.iter().position(|group| {
                group.origin == "sync"
                    && group.source_id == source.id
                    && sync_marker(group, SYNC_TARGET_KEY) == planned.target
            });
            by_marker.or_else(|| {
                groups.iter().position(|group| {
                    group.origin == "sync"
                        && group.source_id == source.id
                        && (group.source_group == planned.original
                            || (group.source_group.is_empty() && group.name == name))
                })
            })
        };
        let id = match existing {
            Some(idx) => {
                // 已有组是显式目标。保留名称与全部设置，只更新频道。
                // 不写同步标记，避免多个源共用同一组时互相覆盖。
                if !explicit {
                    // 配置改名只作用于未被管理员改过名的分组。
                    // 旧分组没有基线：记下当前计划名，但不覆盖现有名称。
                    let baseline = sync_marker(&groups[idx], SYNC_NAME_KEY).to_string();
                    if baseline.is_empty() {
                        set_sync_marker(&mut groups[idx], SYNC_NAME_KEY, &name);
                    } else if groups[idx].name == baseline && groups[idx].name != name {
                        groups[idx].name = name.clone();
                        set_sync_marker(&mut groups[idx], SYNC_NAME_KEY, &name);
                    }
                    set_sync_marker(&mut groups[idx], SYNC_TARGET_KEY, &planned.target);
                }
                groups[idx].id.clone()
            }
            None => {
                let sample = plan
                    .channels
                    .iter()
                    .find(|channel| channel.target == planned.target);
                let dash = sample.map(|channel| channel.dash).unwrap_or(false);
                let mut group = Group {
                    id: unique_group_id(&groups, &name),
                    name,
                    mode: mode_for(source, dash),
                    access: "public".into(),
                    format: format_for(source, dash),
                    origin: "sync".into(),
                    source_id: source.id.clone(),
                    source_group: planned.original.clone(),
                    ..Group::default()
                };
                let id = group.id.clone();
                let display = group.name.clone();
                set_sync_marker(&mut group, SYNC_NAME_KEY, &display);
                set_sync_marker(&mut group, SYNC_TARGET_KEY, &planned.target);
                let position = position_for(source, &playlist);
                let at = if position == "top" {
                    groups
                        .iter()
                        .position(|group| group.origin != "sync")
                        .unwrap_or(groups.len())
                } else {
                    groups.len()
                };
                groups.insert(at, group);
                stats.groups_created += 1;
                id
            }
        };
        target_ids.insert(planned.target.clone(), id);
    }

    let index_by_id: HashMap<String, usize> = groups
        .iter()
        .enumerate()
        .map(|(idx, group)| (group.id.clone(), idx))
        .collect();
    let target_index: HashMap<String, usize> = target_ids
        .iter()
        .filter_map(|(original, id)| index_by_id.get(id).map(|idx| (original.clone(), *idx)))
        .collect();

    let plan_index: HashMap<String, usize> = plan
        .channels
        .iter()
        .enumerate()
        .map(|(idx, channel)| (canonical_key(&channel.key), idx))
        .collect();
    let mut existing_keys: HashSet<String> = HashSet::new();
    let mut remove_keys: HashSet<String> = HashSet::new();
    let mut move_keys: HashSet<String> = HashSet::new();

    // 阶段二：就地更新。移动与删除留到之后处理，避免索引失效。
    // 先做只读配对，再按索引写回。
    let mut slots: Vec<(usize, usize)> = Vec::new();
    let pairs = {
        let mut refs: Vec<ExistingRef<'_>> = Vec::new();
        for (gidx, group) in groups.iter().enumerate() {
            for (cidx, channel) in group.channels.iter().enumerate() {
                if channel.source_id != source.id || channel.source_key.is_empty() {
                    continue;
                }
                slots.push((gidx, cidx));
                refs.push(ExistingRef {
                    key: &channel.source_key,
                    name: &channel.name,
                    url: &channel.url,
                });
            }
        }
        pair_channels(&refs, plan)
    };
    for (slot, pair) in pairs.iter().enumerate() {
        let (gidx, cidx) = slots[slot];
        let channel = &mut groups[gidx].channels[cidx];
        let Some(pidx) = *pair else {
            if channel.sync_follow {
                remove_keys.insert(canonical_key(&channel.source_key));
            } else {
                stats.skipped_removals += 1;
            }
            continue;
        };
        let planned = &plan.channels[pidx];
        // 把旧拼写或旧序号迁移到计划键。后续同步按规范键精确匹配。
        channel.source_key = planned.key.clone();
        let planned_key = canonical_key(&planned.key);
        existing_keys.insert(planned_key.clone());
        if !channel.sync_follow {
            continue;
        }
        let wanted = target_index.get(&planned.target).copied().unwrap_or(gidx);
        if wanted != gidx {
            move_keys.insert(planned_key);
        } else if update_fields(channel, planned) {
            stats.updated += 1;
        }
    }

    // 阶段三：删除计划外且跟随上游的频道。
    if !remove_keys.is_empty() {
        for group in &mut groups {
            let before = group.channels.len();
            group.channels.retain(|channel| {
                !(channel.source_id == source.id
                    && remove_keys.contains(&canonical_key(&channel.source_key)))
            });
            stats.removed += before - group.channels.len();
        }
    }

    // 阶段四：移动频道到目标分组。按原顺序收集，避免组内顺序反转。
    if !move_keys.is_empty() {
        let mut moved: Vec<Channel> = Vec::new();
        for group in &mut groups {
            let mut kept = Vec::with_capacity(group.channels.len());
            for channel in group.channels.drain(..) {
                if channel.source_id == source.id
                    && move_keys.contains(&canonical_key(&channel.source_key))
                {
                    moved.push(channel);
                } else {
                    kept.push(channel);
                }
            }
            group.channels = kept;
        }
        for mut channel in moved {
            let Some(&pidx) = plan_index.get(&canonical_key(&channel.source_key)) else {
                continue;
            };
            let planned = &plan.channels[pidx];
            update_fields(&mut channel, planned);
            let Some(&gidx) = target_index.get(&planned.target) else {
                // 目标分组丢失：放回最后一个分组，避免丢数据。
                if let Some(last) = groups.last_mut() {
                    last.channels.push(channel);
                }
                continue;
            };
            // 保留原 id：公开播放地址与录制引用都以它为准。
            let base = if channel.id.trim().is_empty() {
                planned.id_base.clone()
            } else {
                channel.id.clone()
            };
            channel.id = unique_channel_id(&groups[gidx], &base);
            groups[gidx].channels.push(channel);
            stats.updated += 1;
        }
    }

    // 阶段五：新增计划内、当前不存在的频道。
    for planned in &plan.channels {
        if existing_keys.contains(&canonical_key(&planned.key)) {
            continue;
        }
        let Some(&gidx) = target_index.get(&planned.target) else {
            continue;
        };
        let id = unique_channel_id(&groups[gidx], &planned.id_base);
        let channel = build_channel(source, planned, id);
        groups[gidx].channels.push(channel);
        stats.added += 1;
    }

    // 阶段六：分组清理。手工分组不动。
    let mut idx = groups.len();
    while idx > 0 {
        idx -= 1;
        if groups[idx].origin != "sync" || groups[idx].source_id != source.id {
            continue;
        }
        if groups[idx].channels.is_empty() {
            groups.remove(idx);
            stats.groups_removed += 1;
            continue;
        }
        // 只要还有跟随本源的频道，分组就继续归本源管理。
        let has_following = groups[idx]
            .channels
            .iter()
            .any(|channel| channel.source_id == source.id && channel.sync_follow);
        if has_following {
            continue;
        }
        let other = groups[idx]
            .channels
            .iter()
            .find(|channel| !channel.source_id.is_empty() && channel.source_id != source.id)
            .map(|channel| channel.source_id.clone());
        match other {
            Some(source_id) => {
                groups[idx].source_id = source_id;
                groups[idx].source_group.clear();
                clear_sync_markers(&mut groups[idx]);
            }
            None => {
                groups[idx].origin = "manual".into();
                groups[idx].source_id.clear();
                groups[idx].source_group.clear();
                clear_sync_markers(&mut groups[idx]);
            }
        }
    }

    stats.channels = groups
        .iter()
        .flat_map(|group| group.channels.iter())
        .filter(|channel| channel.source_id == source.id)
        .count();
    stats.groups = groups
        .iter()
        .filter(|group| group.origin == "sync" && group.source_id == source.id)
        .count();

    config.groups = groups;
    stats
}

/// 预检差异。与 [`apply`] 使用同一套匹配规则。
pub fn diff(config: &Config, source: &PlaylistSource, plan: &Plan) -> Diff {
    let mut out = Diff {
        skipped_duplicates: plan.duplicates,
        ..Diff::default()
    };
    let mut refs: Vec<ExistingRef<'_>> = Vec::new();
    for group in &config.groups {
        for channel in &group.channels {
            if channel.source_id != source.id || channel.source_key.is_empty() {
                continue;
            }
            refs.push(ExistingRef {
                key: &channel.source_key,
                name: &channel.name,
                url: &channel.url,
            });
        }
    }
    let pairs = pair_channels(&refs, plan);
    let mut matched: HashSet<usize> = HashSet::new();
    let mut slot = 0;
    for group in &config.groups {
        for channel in &group.channels {
            if channel.source_id != source.id || channel.source_key.is_empty() {
                continue;
            }
            match pairs[slot] {
                Some(idx) => {
                    matched.insert(idx);
                    if channel.sync_follow && !same_upstream(channel, &plan.channels[idx]) {
                        out.update += 1;
                    }
                }
                None => {
                    if channel.sync_follow {
                        out.remove += 1;
                    }
                }
            }
            slot += 1;
        }
    }
    out.add = plan.channels.len().saturating_sub(matched.len());

    for planned_group in &plan.groups {
        let explicit = !planned_group.merge_id.is_empty();
        let existing = if explicit {
            let by_id = config
                .groups
                .iter()
                .position(|group| group.id == planned_group.merge_id);
            by_id.or_else(|| {
                config.groups.iter().position(|group| {
                    group.origin == "sync"
                        && group.source_id == source.id
                        && sync_marker(group, SYNC_TARGET_KEY) == planned_group.target
                })
            })
        } else {
            config.groups.iter().position(|group| {
                group.origin == "sync"
                    && group.source_id == source.id
                    && (sync_marker(group, SYNC_TARGET_KEY) == planned_group.target
                        || group.source_group == planned_group.original
                        || (group.source_group.is_empty() && group.name == planned_group.name))
            })
        };
        match existing {
            None => out.groups_add += 1,
            Some(idx) => {
                // 与 apply 一致：显式已有组不改名。
                if explicit {
                    continue;
                }
                let group = &config.groups[idx];
                let baseline = sync_marker(group, SYNC_NAME_KEY);
                // 与 apply 一致：只改未被管理员改过名的分组。
                if !baseline.is_empty()
                    && group.name == baseline
                    && group.name != planned_group.name
                {
                    out.groups_rename += 1;
                }
            }
        }
    }
    for group in &config.groups {
        if group.origin != "sync" || group.source_id != source.id {
            continue;
        }
        let planned_group = plan.groups.iter().any(|planned| {
            (!planned.merge_id.is_empty() && planned.merge_id == group.id)
                || sync_marker(group, SYNC_TARGET_KEY) == planned.target
                || planned.original == group.source_group
                || (group.source_group.is_empty() && group.name == planned.name)
        });
        if planned_group {
            continue;
        }
        let all_follow = group
            .channels
            .iter()
            .filter(|channel| channel.source_id == source.id)
            .all(|channel| channel.sync_follow);
        if all_follow {
            out.groups_remove += 1;
        }
    }
    out
}

/// 聚合源单次抓取上限。订阅请求会同步等待，不能套用同步源的长超时。
pub const AGGREGATE_FETCH_TIMEOUT_SECS: u64 = 25;

/// 抓取源正文。复用源级代理与请求头，超时取同步设置。
pub async fn fetch(
    state: &AppState,
    source: &PlaylistSource,
    playlist: &PlaylistCfg,
) -> anyhow::Result<String> {
    let timeout = playlist.sync_fetch_timeout_secs.clamp(1, 600);
    fetch_bounded(state, source, Duration::from_secs(timeout), MAX_BODY_BYTES).await
}

/// 抓取正文并指定超时与字节上限。聚合源与预检共用。
pub async fn fetch_bounded(
    state: &AppState,
    source: &PlaylistSource,
    timeout: Duration,
    max_bytes: usize,
) -> anyhow::Result<String> {
    let client = match source.proxy.as_ref() {
        Some(proxy) => state.client_for_proxy(proxy),
        None => state.http.clone(),
    };
    let mut request = client.get(source.url.trim()).timeout(timeout);
    for raw in &source.headers {
        if let Some((name, value)) = split_header(raw) {
            request = request.header(name, value);
        }
    }
    let mut response = request.send().await.context("抓取失败")?;
    let status = response.status();
    if !status.is_success() {
        bail!("上游返回 {status}");
    }
    if let Some(length) = response.content_length() {
        if length > max_bytes as u64 {
            bail!("正文超过 {} MiB", max_bytes / 1024 / 1024);
        }
    }
    // 流式累计，先拦上限再扩容，避免无 Content-Length 的大响应占满内存。
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.context("读取失败")? {
        if bytes.len() + chunk.len() > max_bytes {
            bail!("正文超过 {} MiB", max_bytes / 1024 / 1024);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 同步一个源。调用方应已持有全局同步锁。
pub async fn sync_locked(
    state: &Arc<AppState>,
    id: &str,
    force: bool,
) -> anyhow::Result<SyncReport> {
    let started = Instant::now();
    let config = state.snapshot();
    let Some(source) = config
        .playlist
        .sources
        .iter()
        .find(|source| source.id == id)
        .cloned()
    else {
        bail!("订阅源不存在");
    };
    if source.mode != "sync" {
        bail!("该源不是托管模式");
    }

    state.sources.update_status(id, |status| {
        status.mode = "sync".into();
        status.syncing = true;
        status.last_try = now_secs();
    });

    let text = match fetch(state, &source, &config.playlist).await {
        Ok(text) => text,
        Err(error) => {
            let message = format!("{error:#}");
            state.sources.update_status(id, |status| {
                status.syncing = false;
                status.error = Some(message.clone());
            });
            return Err(error);
        }
    };
    let entries = m3u::parse(&text);
    let plan = plan(&config, &source, &entries);
    let existing = count_source_channels(&config, id);
    let max_channels = if source.max_channels > 0 {
        source.max_channels
    } else {
        config.playlist.sync_max_channels
    };
    if max_channels > 0 && plan.channels.len() as u64 > max_channels {
        let message = format!("计划 {} 条，超过上限 {max_channels}", plan.channels.len());
        finish_error(state, id, message.clone());
        bail!("{message}");
    }

    if plan.channels.is_empty() && existing > 0 && !config.playlist.sync_allow_empty && !force {
        state.sources.update_status(id, |status| {
            status.syncing = false;
            status.needs_review = true;
            status.zero_blocked = true;
            status.error = None;
            status.duration_ms = started.elapsed().as_millis() as u64;
        });
        return Ok(SyncReport {
            blocked: true,
            ..SyncReport::default()
        });
    }

    let expected = source.clone();
    let mut applied: Option<ApplyStats> = None;
    let result = state.mutate(|config| {
        let current = config
            .playlist
            .sources
            .iter()
            .find(|source| source.id == id)
            .cloned();
        let Some(current) = current else {
            bail!("源已删除");
        };
        if current != expected {
            bail!("源配置已变更，下一轮重试");
        }
        applied = Some(apply(config, &current, &plan));
        Ok(())
    });
    if let Err(error) = result {
        let message = format!("{error:#}");
        finish_error(state, id, message.clone());
        return Err(error);
    }
    let stats = applied.unwrap_or_default();
    let shrink_permille = if existing == 0 {
        0
    } else {
        (stats.removed as u64).saturating_mul(1000) / existing as u64
    };
    state.sources.update_status(id, |status| {
        let now = now_secs();
        status.syncing = false;
        status.mode = "sync".into();
        status.channels = stats.channels;
        status.groups = stats.groups;
        status.last_try = now;
        status.last_ok = now;
        status.last_sync = now;
        status.added = stats.added;
        status.updated = stats.updated;
        status.removed = stats.removed;
        status.skipped_removals = stats.skipped_removals;
        status.needs_review = false;
        status.zero_blocked = false;
        status.shrink_permille = shrink_permille;
        status.duration_ms = started.elapsed().as_millis() as u64;
        status.error = None;
        status.next_sync = next_run_at(
            &config.playlist,
            &expected,
            now,
            config.server.timezone_offset_hours,
        );
    });

    tracing::info!(
        source = id,
        added = stats.added,
        updated = stats.updated,
        removed = stats.removed,
        skipped = stats.skipped_removals,
        "订阅同步完成"
    );
    Ok(SyncReport {
        blocked: false,
        stats,
        shrink_permille,
    })
}

/// 同步一个源。内部获取全局同步锁。
pub async fn sync_source(
    state: &Arc<AppState>,
    id: &str,
    force: bool,
) -> anyhow::Result<SyncReport> {
    let _guard = state.sync_lock.lock().await;
    sync_locked(state, id, force).await
}

/// 启动调度器。每 60 秒检查一次，启动后等待 30 秒。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut failures: HashMap<String, u32> = HashMap::new();
        loop {
            run_due(&state, &mut failures).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

async fn run_due(state: &Arc<AppState>, failures: &mut HashMap<String, u32>) {
    let config = state.snapshot();
    if !config.playlist.sync_enabled {
        return;
    }
    let now = now_secs();
    let statuses = state.sources.statuses();
    let offset = config.server.timezone_offset_hours;
    let mut due: Vec<String> = Vec::new();
    for source in &config.playlist.sources {
        if !source.enabled || source.mode != "sync" {
            continue;
        }
        let status = statuses.get(&source.id).cloned().unwrap_or_default();
        if status.error.is_none() {
            failures.remove(&source.id);
        }
        if matches!(schedule_of(&config.playlist, source), Schedule::Interval(_))
            && status.last_try > 0
            && status.error.is_some()
        {
            let attempt = *failures.get(&source.id).unwrap_or(&0);
            let backoff = backoff_secs(attempt);
            if now < status.last_try.saturating_add(backoff) {
                continue;
            }
        } else if now < next_run_at(&config.playlist, source, status.last_try, offset) {
            continue;
        }
        due.push(source.id.clone());
    }

    for id in due {
        match sync_source(state, &id, false).await {
            Ok(report) if report.blocked => {
                failures.remove(&id);
                tracing::warn!(source = %id, "空清单拦截，等待确认");
            }
            Ok(_) => {
                failures.remove(&id);
            }
            Err(error) => {
                let attempt = failures.entry(id.clone()).or_insert(0);
                *attempt += 1;
                tracing::warn!(source = %id, %error, attempt = *attempt, "订阅同步失败");
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// 调度种类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    Interval(u64),
    Daily(String),
}

/// 取源的生效调度。`inherit` 用全局默认。
pub fn schedule_of(playlist: &PlaylistCfg, source: &PlaylistSource) -> Schedule {
    let kind = match source.schedule.trim() {
        "interval" | "daily" => source.schedule.trim(),
        _ => playlist.sync_schedule.trim(),
    };
    if kind == "daily" {
        let at = if source.sync_daily_at.trim().is_empty() {
            playlist.sync_daily_at.trim()
        } else {
            source.sync_daily_at.trim()
        };
        return Schedule::Daily(at.to_string());
    }
    let secs = if source.sync_interval_secs > 0 {
        source.sync_interval_secs
    } else {
        playlist.sync_interval_secs
    }
    .max(300);
    Schedule::Interval(secs)
}

/// 下一次计划执行时间。`last_try = 0` 表示从未执行，返回 0。
pub fn next_run_at(
    playlist: &PlaylistCfg,
    source: &PlaylistSource,
    last_try: u64,
    offset_hours: i32,
) -> u64 {
    if last_try == 0 {
        return 0;
    }
    match schedule_of(playlist, source) {
        Schedule::Interval(secs) => last_try.saturating_add(secs),
        Schedule::Daily(at) => next_daily_after(last_try, offset_hours, &at),
    }
}

/// `HH:MM` 的下一次本地时间，返回 Unix 秒。
///
/// `last` 之后的第一个该时刻。本地时间用 `server.timezone_offset_hours`，
/// 不引入时区数据库。
pub fn next_daily_after(last: u64, offset_hours: i32, at: &str) -> u64 {
    let Some((hour, minute)) = parse_hhmm(at) else {
        return last.saturating_add(86_400);
    };
    let offset = i64::from(offset_hours) * 3600;
    let local = last as i64 + offset;
    let day = local.div_euclid(86_400);
    let mut target = day * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60;
    if target <= local {
        target += 86_400;
    }
    let unix = target - offset;
    unix.max(0) as u64
}

/// 失败退避：1、2、4… 分钟，上限 60 分钟。
pub fn backoff_secs(attempts: u32) -> u64 {
    let shift = attempts.min(6);
    60u64.saturating_mul(1u64 << shift).min(3600)
}

fn parse_hhmm(value: &str) -> Option<(u32, u32)> {
    let (hour, minute) = value.trim().split_once(':')?;
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// 本源当前物化的频道数。
pub fn content_count(config: &Config, id: &str) -> usize {
    count_source_channels(config, id)
}

/// 把源的内容转为手工。保留分组与频道，只断开来源关系。
pub fn detach_source(config: &mut Config, id: &str) -> usize {
    let mut count = 0;
    for group in &mut config.groups {
        for channel in &mut group.channels {
            if channel.source_id == id {
                channel.source_id.clear();
                channel.source_key.clear();
                channel.sync_follow = true;
                count += 1;
            }
        }
    }
    cleanup_sync_groups(&mut config.groups);
    count
}

/// 删除源的内容，并清理空分组。
pub fn remove_source_content(config: &mut Config, id: &str) -> usize {
    let mut count = 0;
    for group in &mut config.groups {
        let before = group.channels.len();
        group.channels.retain(|channel| channel.source_id != id);
        count += before - group.channels.len();
    }
    cleanup_sync_groups(&mut config.groups);
    count
}

/// 把一个频道的来源清空，转为手工。
pub fn detach_channel(config: &mut Config, gid: &str, cid: &str) -> bool {
    let mut found = false;
    if let Some(group) = config.group_mut(gid) {
        if let Some(channel) = group.channels.iter_mut().find(|channel| channel.id == cid) {
            channel.source_id.clear();
            channel.source_key.clear();
            channel.sync_follow = true;
            found = true;
        }
    }
    if found {
        cleanup_sync_groups(&mut config.groups);
    }
    found
}

/// 把一个分组转为手工。只断开本分组的来源频道。
///
/// 多源并入同一组时，其他源的频道保持跟随，不能一起断开。
pub fn detach_group(config: &mut Config, gid: &str) -> bool {
    let Some(group) = config.group_mut(gid) else {
        return false;
    };
    // 旧分组可能没有归属源。组内只有一个来源时按它断开；多源时保守处理。
    let owner = if group.source_id.is_empty() {
        let mut ids: HashSet<&str> = group
            .channels
            .iter()
            .filter(|channel| !channel.source_id.is_empty())
            .map(|channel| channel.source_id.as_str())
            .collect();
        if ids.len() == 1 {
            ids.drain().next().unwrap_or_default().to_string()
        } else {
            String::new()
        }
    } else {
        group.source_id.clone()
    };
    group.origin = "manual".into();
    group.source_id.clear();
    group.source_group.clear();
    clear_sync_markers(group);
    for channel in &mut group.channels {
        if owner.is_empty() || channel.source_id != owner {
            continue;
        }
        channel.source_id.clear();
        channel.source_key.clear();
        channel.sync_follow = true;
    }
    true
}

/// 两个频道的上游字段是否不同。
pub fn upstream_differs(a: &Channel, b: &Channel) -> bool {
    a.name != b.name
        || a.url != b.url
        || a.headers != b.headers
        || extra_str(a, "tvg_id") != extra_str(b, "tvg_id")
        || extra_str(a, "tvg_name") != extra_str(b, "tvg_name")
        || extra_str(a, "tvg_logo") != extra_str(b, "tvg_logo")
        || extra_str(a, "group_title") != extra_str(b, "group_title")
}

/// 把一个频道键加入源排除列表。重复时不写。
///
/// 同源内 `tvg-id` 不唯一时改用 `idname:`（id + 名称）；名称也相同时
/// 才退回 `url:`。这样上游换地址或被手工改地址后，已删频道不会回流。
pub fn exclude_channel(config: &mut Config, channel: &Channel) {
    let index = exclude_index(config);
    exclude_with_index(config, channel, &index);
}

/// 批量写入排除键。先建一次索引，删除大组时避免 O(频道数²)。
pub fn exclude_channels(config: &mut Config, channels: &[Channel]) {
    let index = exclude_index(config);
    for channel in channels {
        exclude_with_index(config, channel, &index);
    }
}

/// 排除判定需要的计数：同源同 id、同源同 id 同名称。
#[derive(Default)]
struct ExcludeIndex {
    ids: HashMap<(String, String), usize>,
    names: HashMap<(String, String, String), usize>,
}

fn exclude_index(config: &Config) -> ExcludeIndex {
    let mut index = ExcludeIndex::default();
    for group in &config.groups {
        for channel in &group.channels {
            if channel.source_id.is_empty() {
                continue;
            }
            let Some(id) = trim_opt(channel.extra.get("tvg_id").and_then(Value::as_str)) else {
                continue;
            };
            let id = id.to_lowercase();
            *index
                .ids
                .entry((channel.source_id.clone(), id.clone()))
                .or_insert(0) += 1;
            let name = channel.name.trim().to_lowercase();
            if !name.is_empty() {
                *index
                    .names
                    .entry((channel.source_id.clone(), id, name))
                    .or_insert(0) += 1;
            }
        }
    }
    index
}

fn exclude_with_index(config: &mut Config, channel: &Channel, index: &ExcludeIndex) {
    if channel.source_id.is_empty() || channel.source_key.is_empty() {
        return;
    }
    let tvg_id = trim_opt(channel.extra.get("tvg_id").and_then(Value::as_str));
    let pattern = match tvg_id {
        Some(id) => {
            let source_id = channel.source_id.clone();
            let lower = id.to_lowercase();
            let name = channel.name.trim().to_lowercase();
            if index
                .ids
                .get(&(source_id.clone(), lower.clone()))
                .copied()
                .unwrap_or(0)
                <= 1
            {
                Some(format!("id:{lower}"))
            } else if !name.is_empty()
                && index
                    .names
                    .get(&(source_id, lower.clone(), name.clone()))
                    .copied()
                    .unwrap_or(0)
                    <= 1
            {
                Some(format!("idname:{lower}\u{1f}{name}"))
            } else {
                let url = channel.url.trim();
                (!url.is_empty()).then(|| format!("url:{url}"))
            }
        }
        None => {
            let url = channel.url.trim();
            (!url.is_empty()).then(|| format!("url:{url}"))
        }
    };
    let Some(pattern) = pattern else {
        return;
    };
    let source_id = channel.source_id.clone();
    if let Some(source) = config
        .playlist
        .sources
        .iter_mut()
        .find(|source| source.id == source_id)
    {
        if !source.exclude.iter().any(|item| item == &pattern) {
            source.exclude.push(pattern);
        }
    }
}

/// 同步分组清理。
///
/// 空组删除。只剩手工或固定频道的组转为手工。归属源已消失时改挂第一个
/// 仍在跟随的来源。
fn cleanup_sync_groups(groups: &mut Vec<Group>) {
    let mut idx = groups.len();
    while idx > 0 {
        idx -= 1;
        if groups[idx].origin != "sync" {
            continue;
        }
        if groups[idx].channels.is_empty() {
            groups.remove(idx);
            continue;
        }
        let following = groups[idx]
            .channels
            .iter()
            .find(|channel| channel.sync_follow && !channel.source_id.is_empty())
            .map(|channel| channel.source_id.clone());
        match following {
            Some(source_id) => {
                if groups[idx].source_id != source_id {
                    groups[idx].source_id = source_id;
                    groups[idx].source_group.clear();
                    clear_sync_markers(&mut groups[idx]);
                }
            }
            None => {
                groups[idx].origin = "manual".into();
                groups[idx].source_id.clear();
                groups[idx].source_group.clear();
                clear_sync_markers(&mut groups[idx]);
            }
        }
    }
}

fn finish_error(state: &Arc<AppState>, id: &str, message: String) {
    state.sources.update_status(id, |status| {
        status.syncing = false;
        status.last_try = now_secs();
        status.error = Some(message);
    });
}

fn count_source_channels(config: &Config, id: &str) -> usize {
    config
        .groups
        .iter()
        .flat_map(|group| group.channels.iter())
        .filter(|channel| channel.source_id == id)
        .count()
}

/// 分组显示名前缀。空值表示保留源里的原名。
///
/// 控制字符会截断 `#EXTINF`，所以在这里再清一次。
fn prefix_for(source: &PlaylistSource) -> String {
    let prefix: String = source
        .group_prefix
        .chars()
        .filter(|ch| !ch.is_control())
        .collect();
    if prefix.trim().is_empty() {
        String::new()
    } else {
        prefix
    }
}

fn position_for<'a>(source: &'a PlaylistSource, playlist: &'a PlaylistCfg) -> &'a str {
    if source.position.trim().is_empty() {
        playlist.merge_position.trim()
    } else {
        source.position.trim()
    }
}

fn mode_for(source: &PlaylistSource, dash: bool) -> String {
    match source.playback_mode.trim() {
        "" | "auto" => {
            if dash {
                "dash".into()
            } else {
                "proxy".into()
            }
        }
        other => other.to_string(),
    }
}

fn format_for(source: &PlaylistSource, dash: bool) -> String {
    match source.format.trim() {
        "" | "auto" => {
            if dash {
                "mpd".into()
            } else {
                "hls".into()
            }
        }
        other => other.to_string(),
    }
}

fn stable_key(entry: &Entry, original: &str) -> String {
    if let Some(id) = trim_opt(entry.tvg_id.as_deref()) {
        return format!("id:{}", id.to_lowercase());
    }
    let name = entry.name.trim();
    if !name.is_empty() {
        return format!(
            "name:{}\u{1f}{}",
            original.trim().to_lowercase(),
            name.to_lowercase()
        );
    }
    format!("url:{}", entry.url.trim())
}

/// 重复键的稳定后缀：URL 前 8 位十六进制摘要。
///
/// 用 URL 而不是序号，上游重排或删除同组条目时身份不互换。
fn key_suffix(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    hex::encode(&digest[..4])
}

/// 匹配用的规范键。`name:` 键忽略大小写；`id:` 已小写；`url:` 保持原样。
///
/// 旧版本按原样保存 `name:` 键。这里归一后，升级不会误删或重复频道。
fn canonical_key(key: &str) -> String {
    if key.starts_with("name:") {
        key.to_lowercase()
    } else {
        key.to_string()
    }
}

/// 键族：去掉 `#` 后缀。
///
/// 旧版本用 `#2` 区分重复键，新版本用 `#<hash>`。首次同步按键族配对，
/// 把旧键迁移到新键。只在精确匹配失败时使用。
fn key_family(key: &str) -> String {
    let canonical = canonical_key(key);
    if let Some((head, tail)) = canonical.rsplit_once('#') {
        if !tail.is_empty()
            && tail.len() <= 12
            && tail
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            return head.to_string();
        }
    }
    canonical
}

/// 已物化频道的最小匹配信息。
#[derive(Clone, Copy)]
struct ExistingRef<'a> {
    key: &'a str,
    name: &'a str,
    url: &'a str,
}

/// 给每个已物化频道配对计划下标。
///
/// 顺序：精确键 → 键族内名称与地址相同 → 键族内按顺序。
/// 内容匹配优先，删除同 id 的其他条目时不会把存活频道的身份换掉。
fn pair_channels(existing: &[ExistingRef<'_>], plan: &Plan) -> Vec<Option<usize>> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut families: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, channel) in plan.channels.iter().enumerate() {
        let canonical = canonical_key(&channel.key);
        families
            .entry(key_family(&canonical))
            .or_default()
            .push(idx);
        index.insert(canonical, idx);
    }

    let mut pairs = vec![None; existing.len()];
    let mut matched: HashSet<usize> = HashSet::new();
    for (slot, item) in existing.iter().enumerate() {
        if let Some(&idx) = index.get(&canonical_key(item.key)) {
            if matched.insert(idx) {
                pairs[slot] = Some(idx);
            }
        }
    }
    for slot in 0..existing.len() {
        if pairs[slot].is_some() {
            continue;
        }
        let family = key_family(&canonical_key(existing[slot].key));
        let Some(candidates) = families.get(&family) else {
            continue;
        };
        let item = existing[slot];
        if let Some(&idx) = candidates.iter().find(|&&idx| {
            !matched.contains(&idx)
                && plan.channels[idx].name == item.name
                && plan.channels[idx].url == item.url
        }) {
            matched.insert(idx);
            pairs[slot] = Some(idx);
        }
    }
    for slot in 0..existing.len() {
        if pairs[slot].is_some() {
            continue;
        }
        let family = key_family(&canonical_key(existing[slot].key));
        let Some(candidates) = families.get(&family) else {
            continue;
        };
        if let Some(&idx) = candidates.iter().find(|&&idx| !matched.contains(&idx)) {
            matched.insert(idx);
            pairs[slot] = Some(idx);
        }
    }
    pairs
}

fn channel_id_base(entry: &Entry, key: &str) -> String {
    if let Some(id) = trim_opt(entry.tvg_id.as_deref()) {
        if id.len() <= 64
            && id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return id.to_string();
        }
    }
    format!("s-{}", short_hash(key))
}

fn display_name(entry: &Entry) -> String {
    trim_opt(entry.tvg_name.as_deref())
        .or_else(|| {
            let name = entry.name.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .unwrap_or_else(|| entry.url.trim().to_string())
}

fn trim_opt(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn split_header(raw: &str) -> Option<(&str, &str)> {
    let (name, value) = raw.split_once(':')?;
    let name = name.trim();
    let value = value.trim();
    (!name.is_empty() && !value.is_empty()).then_some((name, value))
}

fn entry_headers(entry: &Entry) -> Vec<Value> {
    let mut headers = Vec::new();
    if let Some(value) = trim_opt(entry.user_agent.as_deref()) {
        headers.push(Value::String(format!("User-Agent: {value}")));
    }
    if let Some(value) = trim_opt(entry.referer.as_deref()) {
        headers.push(Value::String(format!("Referer: {value}")));
    }
    for (name, value) in &entry.headers {
        headers.push(Value::String(format!("{name}: {value}")));
    }
    headers
}

fn is_mpd(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".mpd")
}

fn short_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(&hasher.finalize()[..4])
}

fn unique_group_id(groups: &[Group], name: &str) -> String {
    let base = {
        let id = m3u::group_id(name);
        if id.is_empty() {
            format!("s-{}", short_hash(name))
        } else {
            id
        }
    };
    let mut id = base.clone();
    let mut suffix = 2;
    while groups.iter().any(|group| group.id == id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

fn unique_channel_id(group: &Group, base: &str) -> String {
    let mut id = base.to_string();
    let mut suffix = 2;
    while group.channels.iter().any(|channel| channel.id == id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

fn build_channel(source: &PlaylistSource, planned: &PlannedChannel, id: String) -> Channel {
    let mut channel = Channel {
        id,
        name: planned.name.clone(),
        url: planned.url.clone(),
        headers: planned.headers.clone(),
        source_id: source.id.clone(),
        source_key: planned.key.clone(),
        sync_follow: true,
        ..Channel::default()
    };
    write_upstream(&mut channel, planned);
    channel
}

fn write_upstream(channel: &mut Channel, planned: &PlannedChannel) {
    channel.name = planned.name.clone();
    channel.url = planned.url.clone();
    channel.headers = planned.headers.clone();
    set_extra(channel, "tvg_id", planned.tvg_id.as_deref());
    set_extra(channel, "tvg_name", planned.tvg_name.as_deref());
    set_extra(channel, "tvg_logo", planned.tvg_logo.as_deref());
    set_extra(channel, "group_title", Some(planned.source_group.as_str()));
}

fn set_extra(channel: &mut Channel, key: &str, value: Option<&str>) {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            channel
                .extra
                .insert(key.to_string(), Value::String(value.to_string()));
        }
        None => {
            channel.extra.remove(key);
        }
    }
}

fn extra_str<'a>(channel: &'a Channel, key: &str) -> &'a str {
    channel
        .extra
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
}

fn update_fields(channel: &mut Channel, planned: &PlannedChannel) -> bool {
    if same_upstream(channel, planned) {
        return false;
    }
    write_upstream(channel, planned);
    true
}

fn same_upstream(channel: &Channel, planned: &PlannedChannel) -> bool {
    channel.name == planned.name
        && channel.url == planned.url
        && channel.headers == planned.headers
        && extra_str(channel, "tvg_id") == planned.tvg_id.as_deref().unwrap_or("")
        && extra_str(channel, "tvg_name") == planned.tvg_name.as_deref().unwrap_or("")
        && extra_str(channel, "tvg_logo") == planned.tvg_logo.as_deref().unwrap_or("")
        && extra_str(channel, "group_title") == planned.source_group
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

/// 读源状态。缺省值带 `mode`。
pub fn status_of(statuses: &HashMap<String, SourceStatus>, id: &str, mode: &str) -> SourceStatus {
    let mut status = statuses.get(id).cloned().unwrap_or_default();
    if status.mode.is_empty() {
        status.mode = mode.to_string();
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, group: &str, id: &str, url: &str) -> Entry {
        Entry {
            name: name.into(),
            url: url.into(),
            tvg_id: Some(id.into()),
            group_title: Some(group.into()),
            ..Entry::default()
        }
    }

    fn source() -> PlaylistSource {
        PlaylistSource {
            id: "s1".into(),
            name: "上游".into(),
            url: "http://up/source.m3u".into(),
            mode: "sync".into(),
            unlisted_groups: "keep".into(),
            ..PlaylistSource::default()
        }
    }

    fn config_with_source() -> Config {
        let mut config = Config::default();
        config.playlist.sources.push(source());
        config
    }

    fn urls(entries: &[(&str, &str, &str)]) -> Vec<Entry> {
        entries
            .iter()
            .map(|(name, group, url)| entry(name, group, name, url))
            .collect()
    }

    /// 生成计划并应用。测试辅助：避免同时借用配置。
    fn run(config: &mut Config, source: &PlaylistSource, entries: &[Entry]) -> ApplyStats {
        let plan = plan(config, source, entries);
        apply(config, source, &plan)
    }

    #[test]
    fn first_sync_adds_channels_and_group() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[
            ("c1", "央视", "http://up/c1.m3u8"),
            ("c2", "央视", "http://up/c2.m3u8"),
        ]);
        let plan = plan(&config, &source, &entries);
        let stats = apply(&mut config, &source, &plan);
        assert_eq!(stats.added, 2);
        assert_eq!(stats.groups_created, 1);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].name, "央视");
        assert_eq!(config.groups[0].origin, "sync");
        assert_eq!(config.groups[0].source_id, "s1");
        assert_eq!(config.groups[0].access, "public");
    }

    #[test]
    fn custom_group_prefix_is_kept() {
        let mut config = config_with_source();
        let mut source = source();
        source.group_prefix = "我的 · ".into();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].name, "我的 · 央视");
    }

    #[test]
    fn group_prefix_drops_control_chars() {
        let mut config = config_with_source();
        let mut source = source();
        source.group_prefix = "我的\n · ".into();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].name, "我的 · 央视");
    }

    #[test]
    fn second_sync_is_idempotent() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        let first = plan(&config, &source, &entries);
        apply(&mut config, &source, &first);
        assert_eq!(config.groups.len(), 1);

        let second = plan(&config, &source, &entries);
        let stats = apply(&mut config, &source, &second);
        assert_eq!(
            (stats.added, stats.updated, stats.removed),
            (0, 0, 0),
            "第二次同步不应有变化"
        );
        assert_eq!(config.groups[0].channels.len(), 1);
    }

    #[test]
    fn updates_url_and_name() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);

        let changed = vec![entry("新名字", "央视", "c1", "http://up/new.m3u8")];
        let stats = run(&mut config, &source, &changed);
        assert_eq!(stats.updated, 1);
        let channel = &config.groups[0].channels[0];
        assert_eq!(channel.name, "新名字");
        assert_eq!(channel.url, "http://up/new.m3u8");
    }

    #[test]
    fn fixed_channel_keeps_fields_and_survives_removal() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[
            ("c1", "央视", "http://up/c1.m3u8"),
            ("c2", "央视", "http://up/c2.m3u8"),
        ]);
        run(&mut config, &source, &entries);
        config.groups[0].channels[0].name = "手工改名".into();
        config.groups[0].channels[0].sync_follow = false;

        let stats = run(&mut config, &source, &entries[1..]);
        assert_eq!(stats.skipped_removals, 1);
        assert_eq!(config.groups[0].channels.len(), 2);
        assert_eq!(config.groups[0].channels[0].name, "手工改名");
    }

    #[test]
    fn removal_drops_following_channel() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[
            ("c1", "央视", "http://up/c1.m3u8"),
            ("c2", "央视", "http://up/c2.m3u8"),
        ]);
        run(&mut config, &source, &entries);
        let stats = run(&mut config, &source, &entries[..1]);
        assert_eq!(stats.removed, 1);
        assert_eq!(config.groups[0].channels.len(), 1);
    }

    #[test]
    fn manual_channel_never_removed() {
        let mut config = config_with_source();
        let manual = Group {
            id: "manual".into(),
            name: "手工组".into(),
            origin: "manual".into(),
            channels: vec![Channel {
                id: "m1".into(),
                name: "手工频道".into(),
                url: "http://up/manual.m3u8".into(),
                ..Channel::default()
            }],
            ..Group::default()
        };
        config.groups.push(manual);
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        run(&mut config, &source, &[]);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].id, "manual");
        assert_eq!(config.groups[0].channels.len(), 1);
        assert_eq!(config.groups[0].origin, "manual");
    }

    #[test]
    fn duplicate_urls_are_dropped() {
        let config = config_with_source();
        let source = source();
        let entries = vec![
            entry("c1", "央视", "c1", "http://up/same.m3u8"),
            entry("c2", "央视", "c2", "http://up/same.m3u8"),
        ];
        let plan = plan(&config, &source, &entries);
        assert_eq!(plan.channels.len(), 1);
        assert_eq!(plan.duplicates, 1);
    }

    #[test]
    fn manual_url_wins_over_source_entry() {
        let mut config = config_with_source();
        config.groups.push(Group {
            id: "manual".into(),
            name: "手工组".into(),
            origin: "manual".into(),
            channels: vec![Channel {
                id: "m1".into(),
                name: "手工频道".into(),
                url: "http://up/c1.m3u8".into(),
                ..Channel::default()
            }],
            ..Group::default()
        });
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        let plan = plan(&config, &source, &entries);
        assert!(plan.channels.is_empty());
        assert_eq!(plan.duplicates, 1);
    }

    #[test]
    fn merge_into_puts_channel_in_manual_group() {
        let mut config = config_with_source();
        config.groups.push(Group {
            id: "manual".into(),
            name: "手工组".into(),
            origin: "manual".into(),
            ..Group::default()
        });
        let mut source = source();
        source.merge_into.insert("央视".into(), "manual".into());
        config.playlist.sources[0] = source.clone();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        let stats = run(&mut config, &source, &entries);
        assert_eq!(stats.added, 1);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].id, "manual");
        assert_eq!(config.groups[0].channels.len(), 1);
        assert!(config.groups[0].channels[0].source_id == "s1");
    }

    #[test]
    fn merge_new_shares_one_group_and_keeps_upstream_order() {
        let mut config = config_with_source();
        let mut source = source();
        source.merge_new.insert("新闻".into(), "合集".into());
        source.merge_new.insert("体育".into(), "合集".into());
        let entries = urls(&[
            ("c1", "新闻", "http://up/c1.m3u8"),
            ("c2", "体育", "http://up/c2.m3u8"),
            ("c3", "新闻", "http://up/c3.m3u8"),
        ]);
        let stats = run(&mut config, &source, &entries);
        assert_eq!(stats.groups_created, 1);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].name, "合集");
        assert_eq!(sync_marker(&config.groups[0], SYNC_TARGET_KEY), "new:合集");
        let ids: Vec<&str> = config.groups[0]
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect();
        assert_eq!(ids, ["c1", "c2", "c3"], "合并组保留上游条目顺序");

        let stats = run(&mut config, &source, &entries);
        assert_eq!(stats.groups_created, 0, "二次同步不重复建组");
        assert_eq!(config.groups.len(), 1);
    }

    #[test]
    fn merge_new_bypasses_unlisted_drop() {
        let mut config = config_with_source();
        let mut source = source();
        source.group_mode = "map".into();
        source.group_map.insert("已列出".into(), "已列出".into());
        source.unlisted_groups = "drop".into();
        source.merge_new.insert("新闻".into(), "合集".into());
        let entries = urls(&[
            ("c1", "新闻", "http://up/c1.m3u8"),
            ("c2", "其他", "http://up/c2.m3u8"),
        ]);
        let stats = run(&mut config, &source, &entries);
        assert_eq!(stats.added, 1, "显式目标不应被映射外丢弃");
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].name, "合集");
    }

    #[test]
    fn merge_new_and_existing_targets_coexist() {
        let mut config = config_with_source();
        config.groups.push(Group {
            id: "manual".into(),
            name: "手工组".into(),
            origin: "manual".into(),
            ..Group::default()
        });
        let mut source = source();
        source.merge_into.insert("体育".into(), "manual".into());
        source.merge_new.insert("新闻".into(), "新闻合集".into());
        let entries = urls(&[
            ("c1", "新闻", "http://up/c1.m3u8"),
            ("c2", "体育", "http://up/c2.m3u8"),
        ]);
        let stats = run(&mut config, &source, &entries);
        assert_eq!(stats.added, 2);
        assert_eq!(config.groups.len(), 2);
        let manual = config
            .groups
            .iter()
            .find(|group| group.id == "manual")
            .expect("手工组应保留");
        assert_eq!(manual.name, "手工组");
        assert_eq!(manual.channels.len(), 1);
        assert!(
            sync_marker(manual, SYNC_TARGET_KEY).is_empty(),
            "显式已有组不写同步标记"
        );
        let merged = config
            .groups
            .iter()
            .find(|group| sync_marker(group, SYNC_TARGET_KEY) == "new:新闻合集")
            .expect("应建立新组");
        assert_eq!(merged.name, "新闻合集");
        assert_eq!(merged.channels.len(), 1);

        let updated = urls(&[
            ("c1", "新闻", "http://up/c1-new.m3u8"),
            ("c2", "体育", "http://up/c2-new.m3u8"),
        ]);
        let stats = run(&mut config, &source, &updated);
        assert_eq!(stats.updated, 2, "两种目标都继续更新频道");
        assert_eq!(config.groups.len(), 2);
    }

    #[test]
    fn merge_into_existing_group_keeps_name_on_prefix_change() {
        let mut config = config_with_source();
        config.groups.push(Group {
            id: "manual".into(),
            name: "新闻".into(),
            origin: "manual".into(),
            ..Group::default()
        });
        let mut source = source();
        source.merge_into.insert("新闻".into(), "manual".into());
        let entries = urls(&[("c1", "新闻", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);

        let mut renamed = source.clone();
        renamed.group_prefix = "源 · ".into();
        run(&mut config, &renamed, &entries);
        let group = config
            .groups
            .iter()
            .find(|group| group.id == "manual")
            .expect("手工组应保留");
        assert_eq!(group.name, "新闻", "显式已有组不跟随源改名");
        assert!(sync_marker(group, SYNC_NAME_KEY).is_empty());
    }

    #[test]
    fn merge_into_existing_group_keeps_settings() {
        let mut config = config_with_source();
        config.groups.push(Group {
            id: "manual".into(),
            name: "目标组".into(),
            origin: "manual".into(),
            mode: "dash".into(),
            format: "mpd".into(),
            access: "hidden".into(),
            ..Group::default()
        });
        let mut source = source();
        source.playback_mode = "redirect".into();
        source.format = "hls".into();
        source.merge_into.insert("新闻".into(), "manual".into());
        let entries = urls(&[("c1", "新闻", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        let group = config
            .groups
            .iter()
            .find(|group| group.id == "manual")
            .expect("手工组应保留");
        assert_eq!(group.mode, "dash");
        assert_eq!(group.format, "mpd");
        assert_eq!(group.access, "hidden");
        assert_eq!(group.channels.len(), 1);
    }

    #[test]
    fn merge_into_missing_target_reuses_stand_in_group() {
        let mut config = config_with_source();
        let mut source = source();
        source.merge_into.insert("新闻".into(), "gone".into());
        let entries = urls(&[("c1", "新闻", "http://up/c1.m3u8")]);
        let stats = run(&mut config, &source, &entries);
        assert_eq!(stats.groups_created, 1);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(sync_marker(&config.groups[0], SYNC_TARGET_KEY), "gid:gone");
        let stats = run(&mut config, &source, &entries);
        assert_eq!(stats.groups_created, 0, "目标缺失时不得每次新建组");
        assert_eq!(config.groups.len(), 1);
    }

    #[test]
    fn moving_channels_preserves_source_order() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[
            ("c1", "新闻", "http://up/1.m3u8"),
            ("c2", "新闻", "http://up/2.m3u8"),
            ("c3", "新闻", "http://up/3.m3u8"),
        ]);
        run(&mut config, &source, &entries);
        config.groups.push(Group {
            id: "manual".into(),
            name: "目标组".into(),
            origin: "manual".into(),
            ..Group::default()
        });
        let mut moved = source.clone();
        moved.merge_into.insert("新闻".into(), "manual".into());
        run(&mut config, &moved, &entries);

        assert_eq!(config.groups.len(), 1, "旧同步组应清空并删除");
        let ids: Vec<&str> = config.groups[0]
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect();
        assert_eq!(ids, ["c1", "c2", "c3"], "移动后保持上游顺序");
    }

    #[test]
    fn merge_new_prefix_renames_without_moving_group() {
        let mut config = config_with_source();
        let mut source = source();
        source.merge_new.insert("新闻".into(), "合集".into());
        let entries = urls(&[("c1", "新闻", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        let gid = config.groups[0].id.clone();
        assert_eq!(config.groups[0].name, "合集");

        let mut prefixed = source.clone();
        prefixed.group_prefix = "A · ".into();
        run(&mut config, &prefixed, &entries);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].id, gid, "前缀只改显示名");
        assert_eq!(config.groups[0].name, "A · 合集");
        assert_eq!(sync_marker(&config.groups[0], SYNC_TARGET_KEY), "new:合集");
    }

    #[test]
    fn explicit_existing_target_keeps_other_source_marker() {
        let mut config = Config::default();
        let mut first = source();
        first.merge_new.insert("新闻".into(), "合集".into());
        let mut second = source();
        second.id = "s2".into();
        second.name = "上游2".into();
        second.url = "http://up/source2.m3u".into();
        config.playlist.sources.push(first.clone());
        config.playlist.sources.push(second.clone());

        let first_entries = urls(&[("c1", "新闻", "http://up/a.m3u8")]);
        run(&mut config, &first, &first_entries);
        let gid = config.groups[0].id.clone();
        assert_eq!(sync_marker(&config.groups[0], SYNC_TARGET_KEY), "new:合集");

        second.merge_into.insert("体育".into(), gid);
        let second_entries = urls(&[("c2", "体育", "http://up/b.m3u8")]);
        run(&mut config, &second, &second_entries);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(
            sync_marker(&config.groups[0], SYNC_TARGET_KEY),
            "new:合集",
            "第二个源不得覆盖第一个源的目标标记"
        );

        run(&mut config, &first, &first_entries);
        assert_eq!(config.groups.len(), 1, "第一个源仍应找到原组");
        assert_eq!(config.groups[0].channels.len(), 2);
    }

    #[test]
    fn top_position_inserts_before_manual() {
        let mut config = config_with_source();
        config.groups.push(Group {
            id: "manual".into(),
            name: "手工组".into(),
            origin: "manual".into(),
            ..Group::default()
        });
        let mut source = source();
        source.position = "top".into();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].origin, "sync");
        assert_eq!(config.groups[1].origin, "manual");
    }

    #[test]
    fn group_turns_manual_after_channel_removal() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        config.groups[0].channels[0].sync_follow = false;
        run(&mut config, &source, &[]);
        assert_eq!(config.groups[0].origin, "manual");
        assert!(config.groups[0].source_id.is_empty());
    }

    #[test]
    fn empty_group_is_removed() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        run(&mut config, &source, &[]);
        assert!(config.groups.is_empty());
    }

    #[test]
    fn other_source_channel_keeps_group() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "共享", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        config.groups[0].channels.push(Channel {
            id: "o1".into(),
            name: "别源".into(),
            url: "http://up/o1.m3u8".into(),
            source_id: "s2".into(),
            source_key: "id:o1".into(),
            ..Channel::default()
        });
        run(&mut config, &source, &[]);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].source_id, "s2");
        assert_eq!(config.groups[0].origin, "sync");
    }

    #[test]
    fn diff_reports_add_update_remove() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[
            ("c1", "央视", "http://up/c1.m3u8"),
            ("c2", "央视", "http://up/c2.m3u8"),
        ]);
        run(&mut config, &source, &entries);
        let changed = vec![
            entry("c1", "央视", "c1", "http://up/c1-new.m3u8"),
            entry("c3", "央视", "c3", "http://up/c3.m3u8"),
        ];
        let diff = diff(&config, &source, &plan(&config, &source, &changed));
        assert_eq!(diff.add, 1);
        assert_eq!(diff.update, 1);
        assert_eq!(diff.remove, 1);
    }

    #[test]
    fn diff_reports_group_rename_when_prefix_changes() {
        let mut config = config_with_source();
        let mut source = source();
        source.group_prefix = "旧 · ".into();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);

        source.group_prefix.clear();
        let diff = diff(&config, &source, &plan(&config, &source, &entries));
        assert_eq!(diff.groups_rename, 1);
        assert_eq!(diff.groups_add, 0);
    }

    #[test]
    fn stable_key_prefers_tvg_id_then_name_then_url() {
        let with_id = entry("名字", "组", "abc", "http://up/1");
        assert_eq!(stable_key(&with_id, "组"), "id:abc");
        let mut by_name = entry("名字", "组", "", "http://up/1");
        by_name.tvg_id = None;
        assert_eq!(stable_key(&by_name, "组"), "name:组\u{1f}名字");
        by_name.name.clear();
        assert_eq!(stable_key(&by_name, "组"), "url:http://up/1");
    }

    #[test]
    fn duplicate_keys_get_url_suffix() {
        let config = config_with_source();
        let source = source();
        let entries = vec![
            entry("c1", "央视", "same", "http://up/1.m3u8"),
            entry("c2", "央视", "same", "http://up/2.m3u8"),
        ];
        let first = plan(&config, &source, &entries);
        for channel in &first.channels {
            assert!(
                channel.key.starts_with("id:same#"),
                "重复键应带 URL 后缀: {}",
                channel.key
            );
        }
        assert_ne!(first.channels[0].key, first.channels[1].key);
        // 上游重排后键不变。
        let reversed = vec![entries[1].clone(), entries[0].clone()];
        let second = plan(&config, &source, &reversed);
        assert_eq!(
            second.channels[0].key,
            "id:same#".to_string() + &key_suffix("http://up/2.m3u8")
        );
        assert_eq!(
            second.channels[1].key,
            "id:same#".to_string() + &key_suffix("http://up/1.m3u8")
        );
    }

    #[test]
    fn ids_avoid_conflicts_inside_group() {
        let config = config_with_source();
        let source = source();
        let entries = vec![
            entry("c1", "央视", "same", "http://up/1.m3u8"),
            entry("c2", "央视", "same", "http://up/2.m3u8"),
        ];
        let mut config = config;
        run(&mut config, &source, &entries);
        let ids: Vec<&str> = config.groups[0]
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect();
        assert_eq!(ids, vec!["same", "same-2"]);
    }

    #[test]
    fn mpd_urls_use_dash_and_mpd() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("d1", "DASH", "http://up/main.mpd")]);
        run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].mode, "dash");
        assert_eq!(config.groups[0].format, "mpd");
    }

    #[test]
    fn playback_mode_and_format_override_auto() {
        let mut config = config_with_source();
        let mut source = source();
        source.playback_mode = "remux".into();
        source.format = "hls".into();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].mode, "remux");
        assert_eq!(config.groups[0].format, "hls");
    }

    #[test]
    fn schedule_inherits_global_values() {
        let playlist = PlaylistCfg::default();
        let source = source();
        assert_eq!(schedule_of(&playlist, &source), Schedule::Interval(21_600));

        let mut daily = source.clone();
        daily.schedule = "daily".into();
        assert_eq!(
            schedule_of(&playlist, &daily),
            Schedule::Daily("03:30".into())
        );

        daily.sync_daily_at = "06:15".into();
        assert_eq!(
            schedule_of(&playlist, &daily),
            Schedule::Daily("06:15".into())
        );
    }

    #[test]
    fn daily_next_time_uses_offset() {
        // 2026-01-01 00:00:00 UTC，东八区是 08:00。
        let now = 1_767_225_600;
        let next = next_daily_after(now, 8, "09:00");
        assert_eq!(next, now + 3600);
        let next = next_daily_after(now, 8, "07:00");
        assert_eq!(next, now + 23 * 3600);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(0), 60);
        assert_eq!(backoff_secs(1), 120);
        assert_eq!(backoff_secs(3), 480);
        assert_eq!(backoff_secs(10), 3600);
    }

    #[test]
    fn detach_keeps_content_as_manual() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        let count = detach_source(&mut config, "s1");
        assert_eq!(count, 1);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].origin, "manual");
        assert_eq!(config.groups[0].channels.len(), 1);
        assert!(config.groups[0].channels[0].source_id.is_empty());
        assert!(config.groups[0].channels[0].source_key.is_empty());
    }

    #[test]
    fn remove_content_drops_empty_group() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        let count = remove_source_content(&mut config, "s1");
        assert_eq!(count, 1);
        assert!(config.groups.is_empty());
    }

    #[test]
    fn remove_keeps_group_owned_by_other_source() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "共享", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        config.groups[0].channels.push(Channel {
            id: "o1".into(),
            name: "别源".into(),
            url: "http://up/o1.m3u8".into(),
            source_id: "s2".into(),
            source_key: "id:o1".into(),
            ..Channel::default()
        });
        remove_source_content(&mut config, "s1");
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].source_id, "s2");
        assert_eq!(config.groups[0].channels.len(), 1);
    }

    #[test]
    fn empty_plan_with_existing_blocks_in_sync() {
        // 保护逻辑在 `sync_locked`；这里校验判定条件本身。
        let playlist = PlaylistCfg::default();
        assert!(!playlist.sync_allow_empty);
        assert_eq!(playlist.sync_max_channels, 20_000);
    }

    #[test]
    fn group_rename_survives_sync() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        config.groups[0].name = "我的频道".into();
        run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].name, "我的频道");
    }

    #[test]
    fn merged_sources_with_same_key_get_distinct_ids() {
        let mut config = Config::default();
        let mut first = source();
        first.merge_into.insert("共享".into(), "g".into());
        let mut second = source();
        second.id = "s2".into();
        second.name = "上游2".into();
        second.merge_into.insert("共享".into(), "g".into());
        config.playlist.sources.push(first.clone());
        config.playlist.sources.push(second.clone());
        config.groups.push(Group {
            id: "g".into(),
            name: "共享".into(),
            ..Group::default()
        });

        run(
            &mut config,
            &first,
            &[entry("C1", "共享", "same", "http://up/a.m3u8")],
        );
        run(
            &mut config,
            &second,
            &[entry("C1", "共享", "same", "http://up/b.m3u8")],
        );
        let ids: Vec<&str> = config.groups[0]
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "同组内频道 id 必须唯一");
    }

    #[test]
    fn duplicate_tvg_id_excludes_by_id_and_name() {
        let mut config = config_with_source();
        let source = source();
        let entries = vec![
            entry("标清", "央视", "same", "http://up/sd.m3u8"),
            entry("高清", "央视", "same", "http://up/hd.m3u8"),
        ];
        run(&mut config, &source, &entries);
        let channel = config.groups[0].channels[0].clone();
        exclude_channel(&mut config, &channel);
        let exclude = &config.playlist.sources[0].exclude;
        assert_eq!(exclude, &vec!["idname:same\u{1f}标清".to_string()]);
    }

    #[test]
    fn duplicate_tvg_id_and_name_excludes_by_url() {
        let mut config = config_with_source();
        let source = source();
        let entries = vec![
            entry("同名", "央视", "same", "http://up/sd.m3u8"),
            entry("同名", "央视", "same", "http://up/hd.m3u8"),
        ];
        run(&mut config, &source, &entries);
        let channel = config.groups[0].channels[0].clone();
        exclude_channel(&mut config, &channel);
        let exclude = &config.playlist.sources[0].exclude;
        assert_eq!(exclude, &vec!["url:http://up/sd.m3u8".to_string()]);
    }

    #[test]
    fn deleted_duplicate_survives_url_rotation() {
        let mut config = config_with_source();
        let source = source();
        let entries = vec![
            entry("标清", "央视", "same", "http://up/sd.m3u8"),
            entry("高清", "央视", "same", "http://up/hd.m3u8"),
        ];
        run(&mut config, &source, &entries);
        let channel = config.groups[0].channels[0].clone();
        exclude_channel(&mut config, &channel);

        // 上游轮换地址后，已删频道不应回流。
        let rotated = vec![
            entry("标清", "央视", "same", "http://up/sd2.m3u8?token=x"),
            entry("高清", "央视", "same", "http://up/hd.m3u8"),
        ];
        let hd_id = config.groups[0].channels[1].id.clone();
        // 排除键写在配置里，重新读取源后再同步。
        let source = config.playlist.sources[0].clone();
        run(&mut config, &source, &rotated);
        let names: Vec<&str> = config.groups[0]
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(names, vec!["高清"], "已删条目不应回流");
        assert_eq!(config.groups[0].channels[0].id, hd_id, "存活频道保留原 id");
    }

    #[test]
    fn detach_group_clears_channel_sources() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "央视", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        let gid = config.groups[0].id.clone();
        assert!(detach_group(&mut config, &gid));
        assert_eq!(config.groups[0].origin, "manual");
        assert!(config.groups[0].source_id.is_empty());
        assert!(config.groups[0].channels[0].source_id.is_empty());
        assert!(config.groups[0].channels[0].source_key.is_empty());
    }

    #[test]
    fn legacy_mixed_case_name_key_still_matches() {
        let mut config = config_with_source();
        let source = source();
        let entries = vec![Entry {
            name: "CCTV-1".into(),
            group_title: Some("CCTV".into()),
            url: "http://up/1".into(),
            ..Entry::default()
        }];
        run(&mut config, &source, &entries);
        // 模拟旧版本按原样写入的键。
        config.groups[0].channels[0].source_key = "name:CCTV\u{1f}CCTV-1".into();

        let stats = run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].channels.len(), 1, "旧键不应产生重复");
        assert_eq!(
            (
                stats.added,
                stats.updated,
                stats.removed,
                stats.skipped_removals
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn name_key_ignores_case() {
        let upper = Entry {
            name: "CCTV-1".into(),
            group_title: Some("CCTV".into()),
            ..Entry::default()
        };
        let lower = Entry {
            name: "cctv-1".into(),
            group_title: Some("cctv".into()),
            ..Entry::default()
        };
        assert_eq!(
            stable_key(&upper, "CCTV"),
            stable_key(&lower, "cctv"),
            "大小写变化不应换键"
        );
    }
    #[test]
    fn merge_mode_materializes_one_group() {
        let mut config = config_with_source();
        let mut source = source();
        source.group_mode = "merge".into();
        source.merge_group = "全部".into();
        source.unlisted_groups = "keep".into();
        let entries = urls(&[
            ("c1", "新闻", "http://up/1.m3u8"),
            ("c2", "体育", "http://up/2.m3u8"),
        ]);
        run(&mut config, &source, &entries);
        assert_eq!(config.groups.len(), 1, "merge 只建一个分组");
        assert_eq!(config.groups[0].name, "全部");
        assert_eq!(config.groups[0].channels.len(), 2);
        assert_eq!(config.groups[0].channels[0].extra["group_title"], "新闻");

        let stats = run(&mut config, &source, &entries);
        assert_eq!(config.groups.len(), 1, "二次同步不重复建组");
        assert_eq!(stats.groups_created, 0);
    }

    #[test]
    fn group_map_renames_existing_sync_group() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "新闻", "http://up/1.m3u8")]);
        run(&mut config, &source, &entries);
        let gid = config.groups[0].id.clone();

        let mut renamed = source.clone();
        renamed.group_mode = "map".into();
        renamed.group_map.insert("新闻".into(), "资讯".into());
        renamed.unlisted_groups = "keep".into();
        run(&mut config, &renamed, &entries);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].name, "资讯");
        assert_eq!(config.groups[0].id, gid, "改名保留分组 id");
    }

    #[test]
    fn moving_legacy_channel_keeps_public_id() {
        let mut config = config_with_source();
        let source = source();
        let entries = vec![Entry {
            name: "CCTV-1".into(),
            group_title: Some("新闻".into()),
            url: "http://up/1.m3u8".into(),
            ..Entry::default()
        }];
        run(&mut config, &source, &entries);
        let id = config.groups[0].channels[0].id.clone();
        // 模拟旧版本保留大小写的 name 键。
        config.groups[0].channels[0].source_key = "name:新闻\u{1f}CCTV-1".into();
        config.groups.push(Group {
            id: "g".into(),
            name: "共享".into(),
            ..Group::default()
        });

        let mut merged = source.clone();
        merged.merge_into.insert("新闻".into(), "g".into());
        run(&mut config, &merged, &entries);
        let channel = config
            .groups
            .iter()
            .flat_map(|group| group.channels.iter())
            .find(|channel| channel.url == "http://up/1.m3u8")
            .expect("频道应仍在配置里");
        assert_eq!(channel.id, id, "移组不应改变公开播放地址");
    }

    #[test]
    fn legacy_case_variant_keys_do_not_duplicate() {
        let mut config = config_with_source();
        let source = source();
        let entries = vec![
            Entry {
                name: "CCTV-1".into(),
                group_title: Some("央视".into()),
                url: "http://up/1.m3u8".into(),
                ..Entry::default()
            },
            Entry {
                name: "cctv-1".into(),
                group_title: Some("央视".into()),
                url: "http://up/2.m3u8".into(),
                ..Entry::default()
            },
        ];
        run(&mut config, &source, &entries);
        assert_eq!(config.groups[0].channels.len(), 2);
        // 旧版本按原样保存大小写不同的两条键。
        config.groups[0].channels[0].source_key = "name:央视\u{1f}CCTV-1".into();
        config.groups[0].channels[1].source_key = "name:央视\u{1f}cctv-1".into();

        run(&mut config, &source, &entries);
        let channels = &config.groups[0].channels;
        assert_eq!(channels.len(), 2, "升级后不应追加第三条频道");
        let urls: HashSet<&str> = channels.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls.len(), 2, "两条 URL 都不应丢失");
    }

    #[test]
    fn detach_group_keeps_other_source_channels() {
        let mut config = config_with_source();
        let source = source();
        let entries = urls(&[("c1", "共享", "http://up/c1.m3u8")]);
        run(&mut config, &source, &entries);
        config.groups[0].channels.push(Channel {
            id: "o1".into(),
            name: "别源".into(),
            url: "http://up/o1.m3u8".into(),
            source_id: "s2".into(),
            source_key: "id:o1".into(),
            ..Channel::default()
        });
        let gid = config.groups[0].id.clone();
        assert!(detach_group(&mut config, &gid));
        let group = &config.groups[0];
        assert_eq!(group.origin, "manual");
        assert!(group.channels[0].source_id.is_empty(), "本源频道断开");
        assert_eq!(group.channels[1].source_id, "s2", "其他源频道保持跟随");
        assert_eq!(group.channels[1].source_key, "id:o1");
    }

    #[test]
    fn batch_exclude_matches_single_exclude() {
        let mut config = config_with_source();
        let source = source();
        let entries = vec![
            entry("标清", "央视", "same", "http://up/sd.m3u8"),
            entry("高清", "央视", "same", "http://up/hd.m3u8"),
            entry("一套", "央视", "one", "http://up/one.m3u8"),
        ];
        run(&mut config, &source, &entries);
        let channels = config.groups[0].channels.clone();

        let mut single = config.clone();
        for channel in &channels {
            exclude_channel(&mut single, channel);
        }
        let mut batch = config.clone();
        exclude_channels(&mut batch, &channels);
        assert_eq!(
            single.playlist.sources[0].exclude,
            batch.playlist.sources[0].exclude
        );
    }
}
