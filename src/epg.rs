//! 节目单（EPG）：拉取 XMLTV、按时间窗裁剪、匹配本服务频道、生成 `/epg.xml`。
//!
//! 契约见 [`docs/design/03-http-contract.md`](../docs/design/03-http-contract.md) §6.5。
//! 证据：F-008（S-EP-05、S-EP-12）。
//!
//! 输出只保留能匹配到本服务频道的频道与节目。生成文件放在
//! `<cache>/epg/epg.xml` 与 `epg.xml.gz`，由公开路由直接下发。

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use roxmltree::Document;

use crate::{config::Config, state::AppState};

/// EPG 运行状态（指标与最近一次结果）。
#[derive(Default)]
pub struct EpgState {
    /// 采集进行中。
    pub running: AtomicBool,
    /// 最近一次采集的频道数与节目数。
    pub channels: AtomicUsize,
    pub programmes: AtomicUsize,
    pub kept: AtomicUsize,
    /// 最近一次成功时间（Unix 秒）。
    pub last_success: Mutex<Option<u64>>,
    /// 最近一次错误。
    pub last_error: Mutex<Option<String>>,
}

/// 一次采集的摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub channels: usize,
    pub programmes: usize,
    pub kept: usize,
}

/// 采集中标记：离开作用域自动清除。
struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 节目单输出目录。
pub fn output_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("epg")
}

/// 读取已生成的节目单文件并解析。EPG 关闭或文件缺失时返回错误。
pub fn load_generated(config: &Config) -> Result<DocumentData> {
    let path = output_file(config);
    let body = fs::read_to_string(&path).with_context(|| format!("读 {}", path.display()))?;
    parse(&body)
}

/// 生成文件路径。
pub fn output_file(config: &Config) -> PathBuf {
    output_dir(Path::new(&config.cache.disk_path)).join("epg.xml")
}

/// 触发一次采集。未启用或已在采集时返回错误。
pub async fn refresh(state: &Arc<AppState>) -> Result<Summary> {
    let config = state.snapshot();
    if !config.epg.enabled {
        bail!("EPG 未启用");
    }
    if state.epg.running.swap(true, Ordering::SeqCst) {
        bail!("EPG 采集进行中");
    }
    let _guard = RunningGuard(&state.epg.running);

    let result = collect(state, &config).await;
    match &result {
        Ok(summary) => {
            state.epg.channels.store(summary.channels, Ordering::SeqCst);
            state
                .epg
                .programmes
                .store(summary.programmes, Ordering::SeqCst);
            state.epg.kept.store(summary.kept, Ordering::SeqCst);
            *state
                .epg
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            *state
                .epg
                .last_success
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(now_secs());
        }
        Err(error) => {
            *state
                .epg
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(format!("{error:#}"));
        }
    }
    result
}

async fn collect(state: &Arc<AppState>, config: &Config) -> Result<Summary> {
    let keys = channel_keys(config);
    let now = now_secs() as i64;
    let past = past_limit(config, now);
    let future = now + config.epg.future_days.max(0) * 86_400;

    let mut channels: Vec<Channel> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut programmes: Vec<Programme> = Vec::new();
    let mut sources = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for source in config.epg.sources.iter().filter(|source| source.enabled) {
        if source.kind != "xml" {
            // 脚本源需要外部解释器；按设计不实现（见 design/03 §6.5）。
            errors.push(format!("{}：仅支持 xml 源", source.name));
            continue;
        }
        if source.url.trim().is_empty() {
            errors.push(format!("{}：缺少 url", source.name));
            continue;
        }
        sources += 1;
        let body = match state
            .http
            .get(source.url.trim())
            .send()
            .await
            .and_then(|response| response.error_for_status())
        {
            Ok(response) => match response.text().await {
                Ok(text) => text,
                Err(error) => {
                    errors.push(format!("{}：读取正文失败 {error}", source.name));
                    continue;
                }
            },
            Err(error) => {
                errors.push(format!("{}：抓取失败 {error}", source.name));
                continue;
            }
        };
        match parse(&body) {
            Ok(document) => {
                let matched = document.matched(&keys);
                for channel in matched.channels {
                    if seen.insert(channel.id.clone()) {
                        channels.push(channel);
                    }
                }
                programmes.extend(
                    matched
                        .programmes
                        .into_iter()
                        .filter(|programme| in_window(programme, past, future)),
                );
            }
            Err(error) => errors.push(format!("{}：解析失败 {error:#}", source.name)),
        }
    }

    if sources == 0 {
        bail!("没有可用的 EPG 源");
    }
    if !errors.is_empty() && channels.is_empty() {
        bail!("{}", errors.join("；"));
    }

    let directory = output_dir(Path::new(&config.cache.disk_path));
    fs::create_dir_all(&directory).with_context(|| format!("建目录 {}", directory.display()))?;
    let xml = render(&channels, &programmes);
    let file = directory.join("epg.xml");
    fs::write(&file, xml.as_bytes()).with_context(|| format!("写 {}", file.display()))?;
    let gz = directory.join("epg.xml.gz");
    write_gzip(&gz, xml.as_bytes()).with_context(|| format!("写 {}", gz.display()))?;

    Ok(Summary {
        channels: channels.len(),
        programmes: programmes.len(),
        kept: programmes.len(),
    })
}

fn write_gzip(path: &Path, data: &[u8]) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    encoder.write_all(data)?;
    encoder.finish()?;
    Ok(())
}

/// 启动后台采集：启用时立即跑一次，之后按 `cron`（只识别 `0 */N * * *`）或 4 小时。
pub fn spawn(state: Arc<AppState>) {
    let enabled = state.snapshot().epg.enabled;
    if !enabled {
        return;
    }
    let hours = cron_hours(&state.snapshot().epg.cron).unwrap_or(4).max(1);
    tokio::spawn(async move {
        match refresh(&state).await {
            Ok(summary) => tracing::info!(
                channels = summary.channels,
                programmes = summary.kept,
                "EPG 采集完成"
            ),
            Err(error) => tracing::warn!(%error, "EPG 采集失败"),
        }
        let mut ticker = tokio::time::interval(Duration::from_secs(hours * 3600));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if !state.snapshot().epg.enabled {
                continue;
            }
            match refresh(&state).await {
                Ok(summary) => tracing::info!(
                    channels = summary.channels,
                    programmes = summary.kept,
                    "EPG 采集完成"
                ),
                Err(error) => tracing::warn!(%error, "EPG 采集失败"),
            }
        }
    });
}

/// `0 */N * * *` → N 小时。
fn cron_hours(cron: &str) -> Option<u64> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() < 5 || fields[0] != "0" || fields[2..] != ["*", "*", "*"] {
        return None;
    }
    let hours = fields[1].strip_prefix("*/")?;
    hours.parse().ok()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn past_limit(config: &Config, now: i64) -> i64 {
    let by_days = now - config.epg.keep_days.max(0) * 86_400;
    if config.epg.keep_yesterday {
        // 保留昨天 0 点以后的内容。
        by_days.min(now - (now % 86_400) - 86_400)
    } else {
        by_days
    }
}

/// 频道匹配键：频道名 + 显式的 EPG 标识（`tvg_id`/`tvg-id`/`epg_id`）。
fn channel_keys(config: &Config) -> HashSet<String> {
    let mut keys = HashSet::new();
    for group in &config.groups {
        for channel in &group.channels {
            if config.epg.match_by_name {
                keys.insert(channel.name.trim().to_string());
            }
            for key in ["tvg_id", "tvg-id", "epg_id"] {
                if let Some(value) = channel.extra.get(key).and_then(|value| value.as_str()) {
                    if !value.trim().is_empty() {
                        keys.insert(value.trim().to_string());
                    }
                }
            }
        }
    }
    keys.remove("");
    keys
}

/// 解析结果。
#[derive(Debug, Default)]
pub struct DocumentData {
    pub channels: Vec<Channel>,
    pub programmes: Vec<Programme>,
}

impl DocumentData {
    /// 只保留能匹配到本服务频道的内容。
    fn matched(self, keys: &HashSet<String>) -> Self {
        let channels: Vec<Channel> = self
            .channels
            .into_iter()
            .filter(|channel| {
                keys.contains(channel.id.trim())
                    || channel
                        .names
                        .iter()
                        .any(|(_, name)| keys.contains(name.trim()))
            })
            .collect();
        let ids: HashSet<String> = channels.iter().map(|c| c.id.clone()).collect();
        let programmes = self
            .programmes
            .into_iter()
            .filter(|programme| ids.contains(programme.channel.trim()))
            .collect();
        Self {
            channels,
            programmes,
        }
    }
}

/// 解析频道的重命名与显示名。
#[derive(Debug, Clone, Default)]
pub struct Channel {
    pub id: String,
    pub names: Vec<(Option<String>, String)>,
    /// 原样保留的子节点（如 `icon`）。
    pub raw_children: Vec<String>,
}

/// 解析一条节目。
#[derive(Debug, Clone, Default)]
pub struct Programme {
    pub start: String,
    pub stop: String,
    pub channel: String,
    /// 文本子节点：`(元素名, lang, 文本)`。
    pub texts: Vec<(String, Option<String>, String)>,
    /// 原样保留的子节点（`icon`、`credits` 等）。
    pub raw_children: Vec<String>,
}

/// 解析 XMLTV 正文。
pub fn parse(body: &str) -> Result<DocumentData> {
    let document = Document::parse(body).context("XML 不合法")?;
    let root = document.root_element();
    if root.tag_name().name() != "tv" {
        bail!("根节点不是 tv");
    }
    let mut data = DocumentData::default();
    for node in root.children().filter(|node| node.is_element()) {
        match node.tag_name().name() {
            "channel" => {
                let id = node.attribute("id").unwrap_or("").trim().to_string();
                if id.is_empty() {
                    continue;
                }
                let mut channel = Channel {
                    id,
                    ..Channel::default()
                };
                for child in node.children().filter(|child| child.is_element()) {
                    if child.tag_name().name() == "display-name" {
                        channel.names.push((lang_of(child), text_of(child)));
                    } else if child.tag_name().name() == "icon" {
                        channel.raw_children.push(raw_of(body, child));
                    }
                }
                data.channels.push(channel);
            }
            "programme" => {
                let start = node.attribute("start").unwrap_or("").trim().to_string();
                let stop = node.attribute("stop").unwrap_or("").trim().to_string();
                let channel = node.attribute("channel").unwrap_or("").trim().to_string();
                if channel.is_empty() {
                    continue;
                }
                let mut programme = Programme {
                    start,
                    stop,
                    channel,
                    ..Programme::default()
                };
                for child in node.children().filter(|child| child.is_element()) {
                    let name = child.tag_name().name();
                    if is_text_element(name) {
                        programme
                            .texts
                            .push((name.to_string(), lang_of(child), text_of(child)));
                    } else {
                        programme.raw_children.push(raw_of(body, child));
                    }
                }
                data.programmes.push(programme);
            }
            _ => {}
        }
    }
    Ok(data)
}

fn is_text_element(name: &str) -> bool {
    matches!(
        name,
        "title"
            | "sub-title"
            | "desc"
            | "category"
            | "keyword"
            | "language"
            | "orig-language"
            | "date"
            | "episode-num"
            | "country"
    )
}

fn lang_of(node: roxmltree::Node) -> Option<String> {
    node.attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
        .or_else(|| node.attribute("lang"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn text_of(node: roxmltree::Node) -> String {
    node.text().unwrap_or("").trim().to_string()
}

fn raw_of(source: &str, node: roxmltree::Node) -> String {
    let range = node.range();
    source[range.start..range.end].to_string()
}

fn in_window(programme: &Programme, past: i64, future: i64) -> bool {
    let start = parse_time(&programme.start);
    let stop = parse_time(&programme.stop).or(start);
    match (start, stop) {
        (Some(start), Some(stop)) => stop >= past && start <= future,
        _ => true,
    }
}

/// 生成 XMLTV 正文。
pub fn render(channels: &[Channel], programmes: &[Programme]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<tv generator-info-name=\"iptv-proxy\">\n");
    for channel in channels {
        out.push_str(&format!("  <channel id=\"{}\">\n", escape(&channel.id)));
        if channel.names.is_empty() {
            out.push_str(&format!(
                "    <display-name lang=\"zh\">{}</display-name>\n",
                escape(&channel.id)
            ));
        }
        for (lang, name) in &channel.names {
            out.push_str(&format!(
                "    <display-name{}>{}</display-name>\n",
                lang_attr(lang),
                escape(name)
            ));
        }
        for raw in &channel.raw_children {
            out.push_str(&format!("    {raw}\n"));
        }
        out.push_str("  </channel>\n");
    }
    for programme in programmes {
        out.push_str(&format!(
            "  <programme start=\"{}\" stop=\"{}\" channel=\"{}\">\n",
            escape(&programme.start),
            escape(&programme.stop),
            escape(&programme.channel)
        ));
        for (name, lang, text) in &programme.texts {
            out.push_str(&format!(
                "    <{name}{}>{}</{name}>\n",
                lang_attr(lang),
                escape(text)
            ));
        }
        for raw in &programme.raw_children {
            out.push_str(&format!("    {raw}\n"));
        }
        out.push_str("  </programme>\n");
    }
    out.push_str("</tv>\n");
    out
}

fn lang_attr(lang: &Option<String>) -> String {
    match lang {
        Some(value) => format!(" lang=\"{}\"", escape(value)),
        None => " lang=\"zh\"".to_string(),
    }
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// 解析 XMLTV 时间：`YYYYMMDDHHMMSS[ +HHMM]` → Unix 秒。
pub fn parse_time(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.len() < 14 {
        return None;
    }
    let digits = value.get(..14)?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let year: i64 = digits[0..4].parse().ok()?;
    let month: i64 = digits[4..6].parse().ok()?;
    let day: i64 = digits[6..8].parse().ok()?;
    let hour: i64 = digits[8..10].parse().ok()?;
    let minute: i64 = digits[10..12].parse().ok()?;
    let second: i64 = digits[12..14].parse().ok()?;

    let rest = value[14..].trim();
    let offset = if rest.is_empty() {
        0
    } else {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let body: String = rest
            .trim_start_matches(['+', '-'])
            .chars()
            .filter(|c| *c != ':')
            .collect();
        if body.len() < 4 {
            return None;
        }
        let hours: i64 = body[0..2].parse().ok()?;
        let minutes: i64 = body[2..4].parse().ok()?;
        sign * (hours * 3600 + minutes * 60)
    };

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// 公历日期 → 1970-01-01 起的天数。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv generator-info-name="src">
  <channel id="cctv1"><display-name lang="zh">CCTV-1</display-name><icon src="http://logo/1.png"/></channel>
  <channel id="other"><display-name>Other</display-name></channel>
  <programme start="20261020080000 +0800" stop="20261020090000 +0800" channel="cctv1">
    <title lang="zh">新闻</title>
    <desc>早间新闻</desc>
    <category lang="en">News</category>
    <icon src="http://img/1.jpg"/>
  </programme>
  <programme start="20261020090000 +0800" stop="20261020100000 +0800" channel="other">
    <title>别的台</title>
  </programme>
</tv>"#;

    #[test]
    fn parse_keeps_structure() {
        let data = parse(SAMPLE).expect("应解析成功");
        assert_eq!(data.channels.len(), 2);
        assert_eq!(data.programmes.len(), 2);
        assert_eq!(data.channels[0].names[0].1, "CCTV-1");
        assert!(data.channels[0].raw_children[0].contains("icon"));
        assert_eq!(data.programmes[0].texts[0].0, "title");
        assert!(data.programmes[0].texts[1].1.is_none());
        assert!(data.programmes[0].raw_children[0].contains("icon"));
    }

    #[test]
    fn match_filters_unknown_channels() {
        let data = parse(SAMPLE).expect("应解析成功");
        let mut keys = HashSet::new();
        keys.insert("CCTV-1".to_string());
        let matched = data.matched(&keys);
        assert_eq!(matched.channels.len(), 1);
        assert_eq!(matched.channels[0].id, "cctv1");
        assert_eq!(matched.programmes.len(), 1);
    }

    #[test]
    fn render_adds_lang_and_escapes() {
        let channels = vec![Channel {
            id: "cctv1".into(),
            names: vec![(None, "CCTV & 1".into())],
            raw_children: vec![],
        }];
        let programmes = vec![Programme {
            start: "20261020080000 +0800".into(),
            stop: "20261020090000 +0800".into(),
            channel: "cctv1".into(),
            texts: vec![("title".into(), None, "新闻".into())],
            raw_children: vec![],
        }];
        let xml = render(&channels, &programmes);
        assert!(xml.contains("generator-info-name=\"iptv-proxy\""));
        assert!(xml.contains("<display-name lang=\"zh\">CCTV &amp; 1</display-name>"));
        assert!(xml.contains("<title lang=\"zh\">新闻</title>"));
        // 结构可再解析。
        parse(&xml).expect("输出应可再解析");
    }

    #[test]
    fn parse_time_handles_offsets() {
        // 2026-10-20T08:00:00+08:00 == 2026-10-20T00:00:00Z
        assert_eq!(parse_time("20261020080000 +0800"), Some(1_792_454_400));
        assert_eq!(parse_time("20261020000000 +0000"), Some(1_792_454_400));
        assert_eq!(parse_time("短"), None);
    }

    #[test]
    fn window_filters_old_and_far_future() {
        let now = 1_792_454_400;
        let old = Programme {
            start: "20260101000000 +0000".into(),
            stop: "20260101010000 +0000".into(),
            ..Programme::default()
        };
        let fresh = Programme {
            start: "20261020080000 +0800".into(),
            stop: "20261020090000 +0800".into(),
            ..Programme::default()
        };
        let far = Programme {
            start: "20300101000000 +0000".into(),
            stop: "20300101010000 +0000".into(),
            ..Programme::default()
        };
        assert!(!in_window(&old, now - 3 * 86_400, now + 86_400));
        assert!(in_window(&fresh, now - 3 * 86_400, now + 86_400));
        assert!(!in_window(&far, now - 3 * 86_400, now + 86_400));
    }

    #[test]
    fn cron_hours_parses_simple_pattern() {
        assert_eq!(cron_hours("0 */4 * * *"), Some(4));
        assert_eq!(cron_hours("0 3 * * *"), None);
        assert_eq!(cron_hours(""), None);
    }
}
