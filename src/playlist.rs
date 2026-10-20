//! 订阅（M3U）渲染。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §5。
//! 证据：F-004、F-009、S-URL-02/04/07。

use crate::config::{
    model::{Channel, Group, PlaylistSource},
    Config,
};

/// 渲染失败的原因。
#[derive(Debug, PartialEq, Eq)]
pub enum PlaylistError {
    /// 分组已归档（`access = "archived"`）：分组订阅返回 404（F-009）。
    GroupArchived,
}

impl PlaylistError {
    /// 客户端可见的错误串。
    pub fn message(&self) -> &'static str {
        match self {
            PlaylistError::GroupArchived => "Group archived",
        }
    }
}

/// 渲染总订阅：只含 `access = "public"` 的分组（F-009）。
pub fn render_total(config: &Config, base: &str, token: Option<&str>) -> String {
    let mut body = header(config);
    for group in &config.groups {
        if group.access != "public" {
            continue;
        }
        for channel in &group.channels {
            push_entry(&mut body, group, channel, base, token);
        }
    }
    body
}

/// 渲染分组订阅。归档组报错（F-009）。
pub fn render_group(
    config: &Config,
    group: &Group,
    base: &str,
    token: Option<&str>,
) -> Result<String, PlaylistError> {
    if group.access == "archived" {
        return Err(PlaylistError::GroupArchived);
    }
    let mut body = header(config);
    for channel in &group.channels {
        push_entry(&mut body, group, channel, base, token);
    }
    Ok(body)
}

/// `#EXTM3U` 与自定义头行（S-URL-07）。
fn header(config: &Config) -> String {
    let mut body = String::from("#EXTM3U\n");
    for line in &config.server.m3u_header_lines {
        body.push_str(line);
        body.push('\n');
    }
    body
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
    let logo = string_field(channel, "logo").unwrap_or_default();
    let group_title = if group.name.is_empty() {
        group.id.clone()
    } else {
        group.name.clone()
    };

    body.push_str(&format!(
        "#EXTINF:-1 tvg-id=\"{}\" tvg-name=\"{}\" tvg-logo=\"{}\" group-title=\"{}\",{}\n",
        tvg_id, tvg_name, logo, group_title, channel.name
    ));

    // Passthrough 直接给上游原始地址，且不套 Token（S-URL-02、S-MODE-11）。
    if group.mode == "passthrough" {
        body.push_str(&channel.url);
        body.push('\n');
        return;
    }

    let ext = if group.format == "mpd" { "mpd" } else { "m3u8" };
    let url = format!(
        "{}/{}/{}.{}",
        base.trim_end_matches('/'),
        group.id,
        channel.id,
        ext
    );

    // `open` 组免 Token，条目不带参数（F-009）。
    match token {
        Some(value) if group.access != "open" => {
            body.push_str(&format!("{url}?token={value}\n"));
        }
        _ => {
            body.push_str(&url);
            body.push('\n');
        }
    }
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

    #[test]
    fn total_keeps_only_public() {
        let config = config_with(vec![
            group("g1", "public", "proxy"),
            group("g2", "hidden", "proxy"),
            group("g3", "open", "proxy"),
            group("g4", "archived", "proxy"),
        ]);
        let body = render_total(&config, "http://host", Some("tok"));
        assert!(body.contains("http://host/g1/c1.m3u8?token=tok"));
        assert!(!body.contains("/g2/"));
        assert!(!body.contains("/g3/"), "open 组不进总订阅");
        assert!(!body.contains("/g4/"), "archived 组不进总订阅");
    }

    #[test]
    fn open_group_entry_has_no_token() {
        let config = config_with(vec![group("g3", "open", "proxy")]);
        let body = render_group(&config, &config.groups[0], "http://host", Some("tok")).unwrap();
        assert!(body.contains("http://host/g3/c1.m3u8\n"));
        assert!(!body.contains("token="));
    }

    #[test]
    fn archived_group_is_rejected() {
        let config = config_with(vec![group("g4", "archived", "proxy")]);
        let error = render_group(&config, &config.groups[0], "http://host", None).unwrap_err();
        assert_eq!(error, PlaylistError::GroupArchived);
    }

    #[test]
    fn passthrough_uses_upstream_url() {
        let config = config_with(vec![group("g5", "public", "passthrough")]);
        let body = render_group(&config, &config.groups[0], "http://host", Some("tok")).unwrap();
        assert!(body.contains("http://upstream/x.m3u8"));
        assert!(!body.contains("/g5/c1"));
    }

    #[test]
    fn header_lines_are_emitted() {
        let mut config = config_with(vec![group("g1", "public", "proxy")]);
        config.server.m3u_header_lines = vec!["#EXTVLCOPT:http-user-agent=Rea/1.0".into()];
        let body = render_total(&config, "http://host", None);
        let first = body.lines().nth(1).unwrap();
        assert_eq!(first, "#EXTVLCOPT:http-user-agent=Rea/1.0");
    }

    #[test]
    fn mpd_format_uses_mpd_ext() {
        let mut config = config_with(vec![group("g6", "public", "dash")]);
        config.groups[0].format = "mpd".into();
        let body = render_total(&config, "http://host", None);
        assert!(body.contains("http://host/g6/c1.mpd"));
    }
}

/// 聚合源过滤 + 提取条目（F-013 §5）。
///
/// 保留 `#EXTINF` 及其附属 `#` 行和原始 URL；不改写地址，也不加 Token。
/// 过滤词：`group:<文本>` 匹配 `group-title`，`id:<文本>` 匹配 `tvg-id`，
/// 其余按频道名匹配。都不区分大小写，空列表表示不过滤。
pub fn aggregate(source: &PlaylistSource, global_exclude: &[String], body: &str) -> String {
    let mut out = String::new();
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
        let entry: Vec<&str> = pending.iter().copied().collect();
        pending.clear();
        let meta = entry
            .iter()
            .find(|line| line.starts_with("#EXTINF"))
            .map(|line| entry_meta(line))
            .unwrap_or_default();
        if (!source.include.is_empty() && !matches(&source.include, &meta))
            || matches(&source.exclude, &meta)
            || matches(global_exclude, &meta)
        {
            continue;
        }
        for line in &entry {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    out
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

/// 是否有任一过滤词命中。
fn matches(patterns: &[String], meta: &(String, String, String)) -> bool {
    let (name, group, id) = meta;
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
                name.to_lowercase().contains(&pattern.to_lowercase())
            }
        })
}

#[cfg(test)]
mod aggregate_tests {
    use super::*;

    const BODY: &str = "#EXTM3U\n#EXTINF:-1 tvg-id=\"a\" group-title=\"新闻\",CCTV-1\nhttp://a/1.m3u8\n#EXTINF:-1 tvg-id=\"b\" group-title=\"体育\",CCTV-5\nhttp://a/5.m3u8\n";

    fn source(include: &[&str], exclude: &[&str]) -> PlaylistSource {
        PlaylistSource {
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
