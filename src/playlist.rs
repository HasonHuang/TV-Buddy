//! 订阅（M3U）渲染。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §5。
//! 证据：F-004、F-009、S-URL-02/04/07。
//!
//! 只有一种订阅：`/sub/:token/playlist.m3u`。令牌即身份，范围由用户决定。
//! 全局 `/playlist.m3u` 与 `/{gid}/playlist.m3u` 已停用。

use std::collections::{BTreeMap, HashSet};

use crate::{
    config::{
        model::{Channel, Group, PlaylistSource},
        Config,
    },
    health::Health,
    subs::{matches_filter, FilterItem, GroupRules},
};

/// 渲染某个用户的订阅。
///
/// 范围按用户的可见分组与频道过滤。管理员等同全部。
/// 归档组（`access = "archived"`）一律跳过：归档表示停播。
/// 开启「探测失败下线」的频道在最近一次探测失败时也跳过。
/// `token` 是该用户自己的访问令牌，会附到播放地址上。
pub fn render_user(
    config: &Config,
    user: &crate::users::User,
    base: &str,
    token: &str,
    health: Option<&Health>,
) -> String {
    let epg_path = format!("/sub/{token}/epg.xml.gz");
    let mut body = header(config, base, &epg_path);
    for group in &config.groups {
        if group.access == "archived" || !user.can_see_group(&group.id) {
            continue;
        }
        for channel in &group.channels {
            if !user.can_see_channel(&group.id, &channel.id) {
                continue;
            }
            if offline(config, health, group, channel) {
                continue;
            }
            push_entry(&mut body, group, channel, base, Some(token));
        }
    }
    body
}

/// 频道是否因探测失败而不进订阅。
///
/// 三个条件同时成立才下线：探测已启用、有效开关为开、最近一次探测失败。
/// 没有探测记录按在线处理。
fn offline(config: &Config, health: Option<&Health>, group: &Group, channel: &Channel) -> bool {
    if !config.health.enabled {
        return false;
    }
    // 与 `Config::offline_on_fail_for` 同序：频道级优先，缺省继承分组级。
    let enabled = channel.offline_on_fail.unwrap_or(group.offline_on_fail);
    enabled && health.is_some_and(|health| health.failed(&group.id, &channel.id))
}

/// 一个用户可见的频道，供面板「我的订阅」页展示。
#[derive(Debug, Clone)]
pub struct UserChannel {
    pub id: String,
    pub name: String,
    /// 播放地址。`passthrough` 组直接给上游地址。
    pub url: String,
}

/// 一个用户可见的分组。
#[derive(Debug, Clone)]
pub struct UserGroup {
    pub id: String,
    pub name: String,
    pub channels: Vec<UserChannel>,
}

/// 面板「我的订阅」用的可见范围。
///
/// 与 [`render_user`] 共用同一套判定（归档组跳过、分组可见、频道可见、
/// 探测失败下线、非 `passthrough` 都带令牌、`passthrough` 给上游地址），
/// 避免两处规则漂移。
pub fn user_view(
    config: &Config,
    user: &crate::users::User,
    base: &str,
    token: &str,
    health: Option<&Health>,
) -> Vec<UserGroup> {
    let mut groups = Vec::new();
    for group in &config.groups {
        if group.access == "archived" || !user.can_see_group(&group.id) {
            continue;
        }
        let mut channels = Vec::new();
        for channel in &group.channels {
            if !user.can_see_channel(&group.id, &channel.id) {
                continue;
            }
            if offline(config, health, group, channel) {
                continue;
            }
            channels.push(UserChannel {
                id: channel.id.clone(),
                name: channel.name.clone(),
                url: entry_url(group, channel, base, token),
            });
        }
        if channels.is_empty() {
            continue;
        }
        groups.push(UserGroup {
            id: group.id.clone(),
            name: if group.name.is_empty() {
                group.id.clone()
            } else {
                group.name.clone()
            },
            channels,
        });
    }
    groups
}

/// `#EXTM3U`、EPG 发现头与自定义头行（S-URL-07、S-EPG-01）。
///
/// `epg_path` 是该用户自己的 EPG 地址，不带主机部分。
fn header(config: &Config, base: &str, epg_path: &str) -> String {
    let mut body = if config.epg.enabled {
        format!(
            "#EXTM3U x-tvg-url=\"{}{}\"\n",
            base.trim_end_matches('/'),
            epg_path
        )
    } else {
        String::from("#EXTM3U\n")
    };
    for line in &config.server.m3u_header_lines {
        body.push_str(line);
        body.push('\n');
    }
    body
}

/// 一个频道的播放地址。
///
/// `passthrough` 直接给上游原始地址，且不套 Token（S-URL-02、S-MODE-11）。
/// 其余组用路径令牌：`/l/<token>/<gid>/<cid>.<ext>`。
pub fn entry_url(group: &Group, channel: &Channel, base: &str, token: &str) -> String {
    if group.mode == "passthrough" {
        return channel.url.clone();
    }
    let ext = if group.format == "mpd" { "mpd" } else { "m3u8" };
    let root = base.trim_end_matches('/');
    format!("{root}/l/{token}/{}/{}.{ext}", group.id, channel.id)
}

fn push_entry(
    body: &mut String,
    group: &Group,
    channel: &Channel,
    base: &str,
    token: Option<&str>,
) {
    let tvg_id = string_field(channel, "tvg_id").unwrap_or_else(|| channel.id.clone());
    let tvg_name = string_field(channel, "tvg_name").unwrap_or_else(|| channel.name.clone());
    let logo = string_field(channel, "logo")
        .or_else(|| string_field(channel, "tvg_logo"))
        .unwrap_or_default();
    let group_title = if group.name.is_empty() {
        group.id.clone()
    } else {
        group.name.clone()
    };

    body.push_str(&format!(
        "#EXTINF:-1 tvg-id=\"{}\" tvg-name=\"{}\" tvg-logo=\"{}\" group-title=\"{}\",{}\n",
        tvg_id, tvg_name, logo, group_title, channel.name
    ));

    match token {
        Some(value) => body.push_str(&entry_url(group, channel, base, value)),
        // 无令牌调用只用于内部渲染，按「不带参数」处理。
        None if group.mode == "passthrough" => body.push_str(&channel.url),
        None => body.push_str(&format!(
            "{}/{}/{}.{}",
            base.trim_end_matches('/'),
            group.id,
            channel.id,
            if group.format == "mpd" { "mpd" } else { "m3u8" }
        )),
    }
    body.push('\n');
}

/// 读取频道上的未建模字符串字段（`tvg_id`、`tvg_name`、`logo`）。
fn string_field(channel: &Channel, key: &str) -> Option<String> {
    channel
        .extra
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Config;
    use crate::users::{Role, User};

    fn config_with(groups: Vec<Group>) -> Config {
        Config {
            groups,
            ..Config::default()
        }
    }

    fn group(id: &str, access: &str, mode: &str) -> Group {
        Group {
            id: id.into(),
            name: id.into(),
            access: access.into(),
            mode: mode.into(),
            channels: vec![Channel {
                id: "c1".into(),
                name: "C1".into(),
                url: "http://upstream/x.m3u8".into(),
                ..Channel::default()
            }],
            ..Group::default()
        }
    }

    /// 能看见全部分组的管理员。
    fn admin() -> User {
        User {
            id: "u1".into(),
            username: "admin".into(),
            role: Role::Admin,
            password_hash: String::new(),
            allowed_group_ids: Vec::new(),
            channel_filter_enabled: false,
            allowed_channel_ids: Vec::new(),
            web_ui_access: true,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    /// 只看得见给定分组的普通用户。
    fn plain(groups: &[&str]) -> User {
        User {
            role: Role::User,
            allowed_group_ids: groups.iter().map(|id| id.to_string()).collect(),
            ..admin()
        }
    }

    #[test]
    fn archived_group_is_skipped() {
        let config = config_with(vec![
            group("g1", "public", "proxy"),
            group("g4", "archived", "proxy"),
        ]);
        let body = render_user(&config, &admin(), "http://host", "tok", None);
        assert!(body.contains("http://host/l/tok/g1/c1.m3u8"));
        assert!(!body.contains("?token="), "订阅条目不用查询参数");
        assert!(!body.contains("/g4/"), "归档组停播，不进订阅");
    }

    #[test]
    fn user_only_sees_allowed_groups() {
        let config = config_with(vec![
            group("g1", "public", "proxy"),
            group("g2", "public", "proxy"),
        ]);
        let body = render_user(&config, &plain(&["g1"]), "http://host", "tok", None);
        assert!(body.contains("/g1/"));
        assert!(!body.contains("/g2/"));
    }

    #[test]
    fn user_with_empty_allowed_groups_sees_all_groups() {
        let config = config_with(vec![
            group("g1", "public", "proxy"),
            group("g2", "public", "proxy"),
        ]);
        let body = render_user(&config, &plain(&[]), "http://host", "tok", None);
        assert!(body.contains("/g1/"));
        assert!(body.contains("/g2/"));
    }

    #[test]
    fn public_group_entry_has_token() {
        let config = config_with(vec![group("g3", "public", "proxy")]);
        let body = render_user(&config, &admin(), "http://host", "tok", None);
        assert!(body.contains("http://host/l/tok/g3/c1.m3u8\n"));
    }

    #[test]
    fn passthrough_uses_upstream_url() {
        let config = config_with(vec![group("g5", "public", "passthrough")]);
        let body = render_user(&config, &admin(), "http://host", "tok", None);
        assert!(body.contains("http://upstream/x.m3u8"));
        assert!(!body.contains("/g5/c1"));
    }

    #[test]
    fn epg_header_points_at_own_subscription() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.epg.enabled = true;
        let body = render_user(&config, &admin(), "http://host/", "tok", None);
        let first = body.lines().next().unwrap();
        assert_eq!(
            first,
            "#EXTM3U x-tvg-url=\"http://host/sub/tok/epg.xml.gz\""
        );
    }

    #[test]
    fn epg_header_is_absent_when_disabled() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.epg.enabled = false;
        let body = render_user(&config, &admin(), "http://host", "tok", None);
        let first = body.lines().next().unwrap();
        assert_eq!(first, "#EXTM3U");
    }

    #[test]
    fn header_lines_are_emitted() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.server.m3u_header_lines = vec!["#EXTVLCOPT:http-user-agent=Rea/1.0".into()];
        let body = render_user(&config, &admin(), "http://host", "tok", None);
        let first = body.lines().nth(1).unwrap();
        assert_eq!(first, "#EXTVLCOPT:http-user-agent=Rea/1.0");
    }

    #[test]
    fn mpd_format_uses_mpd_ext() {
        let mut config = config_with(vec![group("g6", "public", "dash")]);
        config.groups[0].format = "mpd".into();
        let body = render_user(&config, &admin(), "http://host", "tok", None);
        assert!(body.contains("http://host/l/tok/g6/c1.mpd"));
    }

    #[test]
    fn user_view_matches_playlist_scope() {
        let config = config_with(vec![
            group("g1", "public", "proxy"),
            group("g2", "public", "proxy"),
            group("g4", "archived", "proxy"),
        ]);
        let groups = user_view(&config, &plain(&["g1", "g2"]), "http://host", "tok", None);
        assert_eq!(groups.len(), 2, "归档组不进视图");
        assert_eq!(groups[0].id, "g1");
        assert_eq!(groups[0].channels[0].url, "http://host/l/tok/g1/c1.m3u8");
    }

    #[test]
    fn user_view_skips_empty_groups() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.groups[0].channels.clear();
        assert!(user_view(&config, &admin(), "http://host", "tok", None).is_empty());
    }

    #[test]
    fn failed_probe_hides_channel_when_group_switch_is_on() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.groups[0].offline_on_fail = true;
        let health = Health::new();

        let body = render_user(&config, &admin(), "http://host", "tok", Some(&health));
        assert!(body.contains("/g1/"), "没有探测记录时按在线处理");

        health.record_for_test("g1", "c1", false);
        let body = render_user(&config, &admin(), "http://host", "tok", Some(&health));
        assert!(!body.contains("/g1/"), "探测失败后不进订阅");

        health.record_for_test("g1", "c1", true);
        let body = render_user(&config, &admin(), "http://host", "tok", Some(&health));
        assert!(body.contains("/g1/"), "探测成功后恢复");
    }

    #[test]
    fn channel_switch_overrides_group_switch() {
        let mut config = config_with(vec![
            group("g1", "public", "proxy"),
            group("g2", "public", "proxy"),
        ]);
        config.groups[0].offline_on_fail = true;
        config.groups[0].channels[0].offline_on_fail = Some(false);
        config.groups[1].channels[0].offline_on_fail = Some(true);

        let health = Health::new();
        health.record_for_test("g1", "c1", false);
        health.record_for_test("g2", "c1", false);

        let body = render_user(&config, &admin(), "http://host", "tok", Some(&health));
        assert!(body.contains("/g1/"), "频道级关闭覆盖分组级开启");
        assert!(!body.contains("/g2/"), "频道级开启覆盖分组级关闭");
    }

    #[test]
    fn offline_rule_requires_health_enabled() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.groups[0].offline_on_fail = true;
        config.health.enabled = false;
        let health = Health::new();
        health.record_for_test("g1", "c1", false);

        let body = render_user(&config, &admin(), "http://host", "tok", Some(&health));
        assert!(body.contains("/g1/"), "探测关闭时不下线");
    }

    #[test]
    fn user_view_applies_offline_rule() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.groups[0].offline_on_fail = true;
        let health = Health::new();
        health.record_for_test("g1", "c1", false);

        let groups = user_view(&config, &admin(), "http://host", "tok", Some(&health));
        assert!(groups.is_empty(), "频道全部下线时分组也不显示");
    }
}

/// 聚合源过滤 + 提取条目（F-013 §5）。
///
/// 保留 `#EXTINF` 及其附属 `#` 行和原始 URL；不改写地址，也不加 Token。
/// 过滤词：`group:<文本>` 匹配 `group-title`，`id:<文本>` 匹配 `tvg-id`，
/// `url:<文本>` 精确匹配地址，其余按频道名匹配。含 `*` 或 `?` 的通配符
/// 按整串匹配。名称、分组与 id 不区分大小写；`url:` 区分大小写。
/// 空列表表示不过滤。
pub fn aggregate(source: &PlaylistSource, global_exclude: &[String], body: &str) -> String {
    let mut seen = HashSet::new();
    aggregate_seen(source, global_exclude, body, &mut seen).0
}

/// 聚合单个源，并把已并入的 URL 记入 `seen`。
///
/// 目标按 URL 去重，范围覆盖同一源和多个源（F-020）。返回正文与实际并入条数。
pub fn aggregate_seen(
    source: &PlaylistSource,
    global_exclude: &[String],
    body: &str,
    seen: &mut HashSet<String>,
) -> (String, usize) {
    // 显式目标只用于托管同步；聚合模式保持原有分组规则。
    let no_merge_into = BTreeMap::new();
    let no_merge_new = BTreeMap::new();
    let rules = GroupRules {
        group_map: &source.group_map,
        group_mode: &source.group_mode,
        merge_group: &source.merge_group,
        merge_into: &no_merge_into,
        merge_new: &no_merge_new,
        unlisted_groups: &source.unlisted_groups,
    };
    let mut out = String::new();
    let mut added = 0usize;
    let mut pending: Vec<&str> = Vec::new();
    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            if !trimmed.starts_with("#EXTM3U") {
                pending.push(line);
            }
            continue;
        }
        let entry = pending.clone();
        pending.clear();
        let meta = entry
            .iter()
            .find(|line| line.starts_with("#EXTINF"))
            .map(|line| entry_meta(line))
            .unwrap_or_default();
        let original = if meta.1.trim().is_empty() {
            "(无分组)".to_string()
        } else {
            meta.1.trim().to_string()
        };
        let Some(group) = rules.target(&original) else {
            continue;
        };
        let item = FilterItem {
            name: &meta.0,
            original: &original,
            group: &group,
            tvg_id: &meta.2,
            url: trimmed,
        };
        if (!source.include.is_empty() && !matches_filter(&source.include, &item))
            || matches_filter(&source.exclude, &item)
            || matches_filter(global_exclude, &item)
        {
            continue;
        }
        if !seen.insert(trimmed.to_string()) {
            continue;
        }
        for line in &entry {
            if line.starts_with("#EXTINF") && group != original {
                out.push_str(&set_group_title(line, &group));
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        out.push_str(trimmed);
        out.push('\n');
        added += 1;
    }
    (out, added)
}

/// 改写 `#EXTINF` 的 `group-title`；缺属性时插入到名称逗号之前。
fn set_group_title(line: &str, group: &str) -> String {
    if let Some(start) = line.find("group-title=\"") {
        let value_start = start + "group-title=\"".len();
        if let Some(end) = line[value_start..].find('"') {
            let mut out = String::with_capacity(line.len() + group.len());
            out.push_str(&line[..value_start]);
            out.push_str(group);
            out.push_str(&line[value_start + end..]);
            return out;
        }
    }
    match line.rfind(',') {
        Some(index) => format!(
            "{} group-title=\"{}\"{}",
            &line[..index],
            group,
            &line[index..]
        ),
        None => format!("{line} group-title=\"{group}\""),
    }
}

/// 把聚合条目按位置拼到本地订阅上。
pub fn compose(local: String, extra: String, position: &str) -> String {
    if extra.is_empty() {
        return local;
    }
    let (head, rest) = match local.split_once('\n') {
        Some((first, rest)) => (format!("{first}\n"), rest.to_string()),
        None => (String::new(), local),
    };
    if position == "top" {
        format!("{head}{extra}{rest}")
    } else {
        format!("{head}{rest}{extra}")
    }
}

/// `(频道名, group-title, tvg-id)`。
fn entry_meta(line: &str) -> (String, String, String) {
    let name = line
        .rsplit_once(',')
        .map(|(_, name)| name.trim().to_string())
        .unwrap_or_default();
    (
        name,
        attr(line, "group-title").unwrap_or_default(),
        attr(line, "tvg-id").unwrap_or_default(),
    )
}

/// 取 `key="value"` 属性。
fn attr(line: &str, key: &str) -> Option<String> {
    let start = line.find(&format!("{key}=\""))? + key.len() + 2;
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod aggregate_tests {
    use super::*;

    const BODY: &str = "#EXTM3U\n#EXTINF:-1 tvg-id=\"a\" group-title=\"新闻\",CCTV-1\nhttp://a/1.m3u8\n#EXTINF:-1 tvg-id=\"b\" group-title=\"体育\",CCTV-5\nhttp://a/5.m3u8\n";

    fn source(include: &[&str], exclude: &[&str]) -> PlaylistSource {
        PlaylistSource {
            unlisted_groups: "keep".into(),
            include: include.iter().map(|s| s.to_string()).collect(),
            exclude: exclude.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_all_when_no_filter() {
        let out = aggregate(&source(&[], &[]), &[], BODY);
        assert!(out.contains("CCTV-1") && out.contains("CCTV-5"));
        assert!(out.starts_with("#EXTINF"));
        assert!(!out.contains("#EXTM3U"));
    }

    #[test]
    fn include_by_name_group_and_id() {
        assert!(!aggregate(&source(&["cctv-1"], &[]), &[], BODY).contains("CCTV-5"));
        assert!(aggregate(&source(&["group:体育"], &[]), &[], BODY).contains("CCTV-5"));
        assert!(!aggregate(&source(&["id:b"], &[]), &[], BODY).contains("CCTV-1"));
    }

    #[test]
    fn exclude_wins_and_global_applies() {
        let out = aggregate(&source(&[], &["cctv-5"]), &[], BODY);
        assert!(out.contains("CCTV-1") && !out.contains("CCTV-5"));
        let out = aggregate(&source(&[], &[]), &["group:体育".to_string()], BODY);
        assert!(!out.contains("CCTV-5"));
    }

    #[test]
    fn group_map_selects_and_rewrites_groups() {
        let mut source = source(&[], &[]);
        source.unlisted_groups = "drop".into();
        source.group_mode = "map".into();
        source.group_map.insert("新闻".into(), "映射新闻".into());
        let out = aggregate(&source, &[], BODY);
        assert!(out.contains("group-title=\"映射新闻\""), "{out}");
        assert!(!out.contains("CCTV-5"), "{out}");
    }

    #[test]
    fn merge_mode_renames_selected_and_unlisted_groups() {
        let mut source = source(&[], &[]);
        source.group_mode = "merge".into();
        source.merge_group = "合并".into();
        source.group_map.insert("新闻".into(), String::new());
        let out = aggregate(&source, &[], BODY);
        assert_eq!(out.matches("group-title=\"合并\"").count(), 2, "{out}");
    }

    #[test]
    fn duplicate_urls_are_dropped() {
        let body = "#EXTM3U\n#EXTINF:-1 tvg-id=\"a\" group-title=\"新闻\",A\nhttp://a/1.m3u8\n#EXTINF:-1 tvg-id=\"b\" group-title=\"新闻\",B\nhttp://a/1.m3u8\n#EXTINF:-1 tvg-id=\"c\" group-title=\"新闻\",C\nhttp://a/2.m3u8\n";
        let out = aggregate(&source(&[], &[]), &[], body);
        assert_eq!(out.matches("http://a/1.m3u8").count(), 1, "{out}");
        assert!(out.contains("http://a/2.m3u8"), "{out}");
    }

    #[test]
    fn empty_map_keeps_all_in_keep_mode() {
        // 没有映射时，保留原名模式导入全部分组。
        let source = PlaylistSource::default();
        let out = aggregate(&source, &[], BODY);
        assert!(out.contains("CCTV-1") && out.contains("CCTV-5"));
    }

    #[test]
    fn empty_map_in_map_mode_drops_all() {
        let source = PlaylistSource {
            group_mode: "map".into(),
            unlisted_groups: "drop".into(),
            ..PlaylistSource::default()
        };
        assert!(aggregate(&source, &[], BODY).is_empty());
    }

    const WILD_BODY: &str = "#EXTM3U\n#EXTINF:-1 tvg-id=\"a\" group-title=\"新闻\",CCTV-1\nhttp://a/1.m3u8\n#EXTINF:-1 tvg-id=\"b\" group-title=\"体育\",CCTV-10\nhttp://a/10.m3u8\n#EXTINF:-1 tvg-id=\"c\" group-title=\"影视\",翡翠台(字幕)\nhttp://a/jade.m3u8?x=1\n";

    #[test]
    fn wildcard_filters_apply_to_aggregate() {
        let out = aggregate(&source(&["CCTV-?"], &[]), &[], WILD_BODY);
        assert!(out.contains("CCTV-1"), "{out}");
        assert!(!out.contains("CCTV-10"), "{out}");

        let out = aggregate(&source(&["*翡翠*"], &[]), &[], WILD_BODY);
        assert!(out.contains("翡翠台(字幕)"), "{out}");

        let out = aggregate(&source(&["group:体*"], &[]), &[], WILD_BODY);
        assert!(out.contains("CCTV-10"), "{out}");
        assert!(!out.lines().any(|line| line.ends_with(",CCTV-1")), "{out}");

        let out = aggregate(&source(&[], &["CCTV-*"]), &[], WILD_BODY);
        assert!(!out.contains("CCTV-1"), "{out}");
        assert!(!out.contains("CCTV-10"), "{out}");
    }

    #[test]
    fn url_and_idname_filters_apply_to_aggregate() {
        let out = aggregate(
            &source(&["url:http://a/jade.m3u8?x=1"], &[]),
            &[],
            WILD_BODY,
        );
        assert!(out.contains("翡翠台(字幕)"), "{out}");
        assert!(!out.contains("CCTV-1"), "{out}");

        let out = aggregate(&source(&["idname:a\u{1f}CCTV-1"], &[]), &[], WILD_BODY);
        assert!(out.contains("CCTV-1"), "{out}");
        assert!(!out.contains("CCTV-10"), "{out}");

        let out = aggregate(&source(&["id:a*"], &[]), &[], WILD_BODY);
        assert!(out.contains("CCTV-1"), "{out}");
        assert!(!out.contains("CCTV-10"), "{out}");
    }

    #[test]
    fn compose_places_extra() {
        let local = "#EXTM3U\n#EXTINF:-1,本地\nhttp://self/1.m3u8\n".to_string();
        let extra = "#EXTINF:-1,远端\nhttp://up/1.m3u8\n".to_string();
        let top = compose(local.clone(), extra.clone(), "top");
        assert!(top.starts_with("#EXTM3U\n#EXTINF:-1,远端"));
        let bottom = compose(local, extra, "bottom");
        assert!(bottom.ends_with("#EXTINF:-1,远端\nhttp://up/1.m3u8\n"));
        assert_eq!(
            compose("#EXTM3U\n".into(), String::new(), "top"),
            "#EXTM3U\n"
        );
    }
}
