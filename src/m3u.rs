//! M3U 播放列表导入。
//!
//! 解析 `#EXTINF` 属性、`#EXTVLCOPT`、`#EXTHTTP` 与行内头。
//! 落库规则见 [`docs/findings/dynamic/08-import-backup-edge.md`](../docs/findings/dynamic/08-import-backup-edge.md) §2。

/// 一条待导入的频道。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// `#EXTINF` 逗号后的名称。
    pub name: String,
    /// 地址行。
    pub url: String,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    pub tvg_logo: Option<String>,
    pub group_title: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    /// `#EXTHTTP` 中除 `User-Agent`/`Referer` 外的头。
    pub headers: Vec<(String, String)>,
}

/// 解析 M3U 正文。没有地址行的条目丢弃。
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Option<Entry> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            current = Some(parse_extinf(rest));
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTVLCOPT:") {
            if let Some(entry) = current.as_mut() {
                apply_opt(entry, rest.trim());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTHTTP:") {
            if let Some(entry) = current.as_mut() {
                apply_http(entry, rest.trim());
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some(mut entry) = current.take() {
            entry.url = line.to_string();
            entries.push(entry);
        }
    }
    entries
}

fn parse_extinf(rest: &str) -> Entry {
    let mut entry = Entry::default();
    let (attrs, name) = match rest.rfind(',') {
        Some(index) => (&rest[..index], &rest[index + 1..]),
        None => (rest, ""),
    };
    entry.name = name.trim().to_string();
    for (key, value) in attributes(attrs) {
        match key.as_str() {
            "tvg-id" => entry.tvg_id = Some(value),
            "tvg-name" => entry.tvg_name = Some(value),
            "tvg-logo" => entry.tvg_logo = Some(value),
            "group-title" => entry.group_title = Some(value),
            "http-user-agent" => entry.user_agent = Some(value),
            "http-referer" => entry.referer = Some(value),
            _ => {}
        }
    }
    entry
}

/// 扫描 `key="value"` 与 `key=value` 属性对。
fn attributes(attrs: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = attrs.chars().collect();
    let mut index = 0;
    let mut pairs = Vec::new();
    while index < chars.len() {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        let key_start = index;
        while index < chars.len() && chars[index] != '=' && !chars[index].is_whitespace() {
            index += 1;
        }
        let key: String = chars[key_start..index].iter().collect();
        if index >= chars.len() || chars[index] != '=' {
            // 没有等号的片段（如时长）跳过。
            if key.is_empty() {
                index += 1;
            }
            continue;
        }
        index += 1;
        let quoted = index < chars.len() && chars[index] == '"';
        if quoted {
            index += 1;
        }
        let value_start = index;
        while index < chars.len() {
            if quoted && chars[index] == '"' {
                break;
            }
            if !quoted && chars[index].is_whitespace() {
                break;
            }
            index += 1;
        }
        let value: String = chars[value_start..index].iter().collect();
        if quoted {
            index += 1;
        }
        if !key.is_empty() {
            pairs.push((key.to_ascii_lowercase(), value));
        }
    }
    pairs
}

fn apply_opt(entry: &mut Entry, value: &str) {
    if let Some(agent) = value.strip_prefix("http-user-agent=") {
        entry.user_agent = Some(agent.trim().to_string());
    } else if let Some(referer) = value.strip_prefix("http-referer=") {
        entry.referer = Some(referer.trim().to_string());
    }
}

fn apply_http(entry: &mut Entry, value: &str) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(value) else {
        return;
    };
    let Some(map) = json.as_object() else {
        return;
    };
    for (key, value) in map {
        let Some(value) = value.as_str() else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "user-agent" => entry.user_agent = Some(value.to_string()),
            "referer" | "referrer" => entry.referer = Some(value.to_string()),
            _ => entry.headers.push((key.clone(), value.to_string())),
        }
    }
}

/// 分组 id：只保留 ASCII 字母数字与 `_`/`-`；为空时用 `g-<6hex>`。
pub fn group_id(title: &str) -> String {
    let id: String = title
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if id.is_empty() {
        format!("g-{}", short_hex())
    } else {
        id
    }
}

/// 频道 id：优先 `tvg-id`；否则随机 `g-<6hex>`（与目标一致：无 `tvg-id` 不幂等）。
pub fn channel_id(tvg_id: Option<&str>) -> String {
    match tvg_id.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value.to_string(),
        None => format!("g-{}", short_hex()),
    }
}

fn short_hex() -> String {
    hex::encode(&uuid::Uuid::new_v4().as_bytes()[..3])
}

/// 随机频道 id：`r-<8hex>`。
///
/// 用于 `[auth].random_id_enabled = true` 时的导入：忽略 `tvg-id`，
/// 每个频道都拿一个新随机 id。目标开启该键后已有频道与切片前缀不变
/// （F-010 §4），所以它只影响新导入的频道。
pub fn random_channel_id() -> String {
    let bytes = uuid::Uuid::new_v4();
    format!("r-{}", hex::encode(&bytes.as_bytes()[..4]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"#EXTM3U
#EXTINF:-1 tvg-id="cctv1" tvg-name="CCTV-1" tvg-logo="http://logo/1.png" group-title="央视频道",CCTV-1 综合
#EXTVLCOPT:http-user-agent=Mozilla/5.0
http://up/1.m3u8
#EXTINF:-1 group-title="卫视频道",湖南卫视
#EXTHTTP:{"Referer":"http://ref/","X-Token":"abc"}
http://up/2.m3u8
#EXTINF:-1,无分组频道
http://up/3.m3u8
"#;

    fn entry_list() -> Vec<Entry> {
        parse(SAMPLE)
    }

    #[test]
    fn parse_extinf_attributes() {
        let entries = entry_list();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].tvg_id.as_deref(), Some("cctv1"));
        assert_eq!(entries[0].tvg_name.as_deref(), Some("CCTV-1"));
        assert_eq!(entries[0].tvg_logo.as_deref(), Some("http://logo/1.png"));
        assert_eq!(entries[0].group_title.as_deref(), Some("央视频道"));
        assert_eq!(entries[0].url, "http://up/1.m3u8");
        assert_eq!(entries[2].group_title, None);
    }

    #[test]
    fn parse_header_directives() {
        let entries = entry_list();
        assert_eq!(entries[0].user_agent.as_deref(), Some("Mozilla/5.0"));
        assert_eq!(entries[1].referer.as_deref(), Some("http://ref/"));
        assert_eq!(
            entries[1].headers,
            vec![("X-Token".to_string(), "abc".to_string())]
        );
    }

    #[test]
    fn group_id_drops_non_ascii() {
        assert_eq!(group_id("导入组A"), "A");
        assert_eq!(group_id("News-1"), "News-1");
        assert!(group_id("电视剧").starts_with("g-"));
        assert_eq!(group_id("电视剧").len(), 8);
    }

    #[test]
    fn random_channel_id_has_fixed_shape_and_varies() {
        let first = random_channel_id();
        assert!(first.starts_with("r-"), "{first}");
        assert_eq!(first.len(), 10);
        assert!(first[2..].bytes().all(|b| b.is_ascii_hexdigit()), "{first}");
        assert_ne!(first, random_channel_id());
    }

    #[test]
    fn channel_id_prefers_tvg_id() {
        assert_eq!(channel_id(Some(" cctv1 ")), "cctv1");
        assert!(channel_id(Some("  ")).starts_with("g-"));
        assert!(channel_id(None).starts_with("g-"));
    }

    #[test]
    fn entries_without_url_are_dropped() {
        let entries = parse("#EXTM3U\n#EXTINF:-1,只有标题\n");
        assert!(entries.is_empty());
    }
}
