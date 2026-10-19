//! 订阅（M3U）渲染。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §5。
//! 证据：F-004、F-009、S-URL-02/04/07。

use crate::config::{
    model::{Channel, Group},
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
