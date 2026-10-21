//! DASH 清单（MPD）解析。
//!
//! - [`parse`] 只提取面板需要的轨道摘要（`/panel/api/dash/tracks`）。
//! - [`parse_presentation`] 解析完整演示：BaseURL、SegmentTemplate、
//!   SegmentTimeline、SegmentList 与初始化段，供 DASH→HLS 转封装使用。
//!
//! 目标 `/panel/api/dash/tracks/:gid/:cid` 的响应形状未采证（F-011），格式自定。
//! 本模块不下载媒体，只解析 XML；媒体由 `dash_hls` 按需代理。

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use roxmltree::{Document, Node};
use serde::Serialize;
use url::Url;

/// 单条轨道摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Track {
    pub id: String,
    /// `video` / `audio` / `text` / `unknown`。
    pub kind: String,
    pub codecs: Option<String>,
    pub bandwidth: Option<u64>,
    pub lang: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
}

/// 清单摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Manifest {
    /// `dynamic` 或 `static`。
    pub kind: String,
    pub duration: Option<String>,
    pub tracks: Vec<Track>,
}

/// 一个媒体段。
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// 上游段号。
    pub number: u64,
    /// SegmentTimeline 的 `t`（timescale 单位）。
    pub time: u64,
    /// 时长（秒）。
    pub duration: f64,
    /// 可用的上游绝对地址；第一个为首选，其余用于故障回退。
    pub urls: Vec<String>,
}

/// 一条完整轨道。
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    /// MPD 中的 Representation@id；可能为空。
    pub id: String,
    /// 面向 HLS 的轨道名：`video_720p`、`audio-0`、`text-0`。
    pub name: String,
    /// `video` / `audio` / `text` / `unknown`。
    pub kind: String,
    pub codecs: Option<String>,
    pub bandwidth: Option<u64>,
    pub lang: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub frame_rate: Option<String>,
    pub audio_channels: Option<String>,
    /// 初始化段候选地址。
    pub init: Vec<String>,
    /// 媒体段。
    pub segments: Vec<Segment>,
}

/// 完整 MPD 演示。
#[derive(Debug, Clone, PartialEq)]
pub struct Presentation {
    pub is_live: bool,
    pub duration: Option<f64>,
    pub streams: Vec<Stream>,
}

fn number(value: Option<&str>) -> Option<u64> {
    value.and_then(|text| text.trim().parse().ok())
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

fn children<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Vec<Node<'a, 'input>> {
    node.children()
        .filter(|child| child.has_tag_name(name))
        .collect()
}

fn text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 直接子节点 `<BaseURL>` 的文本列表。
fn base_urls(node: Node<'_, '_>) -> Vec<String> {
    children(node, "BaseURL")
        .into_iter()
        .filter_map(text)
        .collect::<Vec<_>>()
}

/// 把一层 BaseURL 解析到父候选列表上。
fn resolve_bases(parents: &[String], children: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for parent in parents {
        for child in children {
            out.push(resolve_url(parent, child));
        }
    }
    dedupe(&mut out);
    out
}

fn dedupe(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn resolve_url(base: &str, reference: &str) -> String {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return reference.to_string();
    }
    let Ok(base_url) = Url::parse(base) else {
        return reference.to_string();
    };
    let base_query = base_url.query().map(str::to_string);
    match base_url.join(reference) {
        Ok(mut url) => {
            // 目录级 BaseURL 的签名/Token 查询串应带到相对切片地址。
            if !reference.contains('?') && url.query().is_none() {
                if let Some(query) = base_query {
                    url.set_query(Some(&query));
                }
            }
            url.to_string()
        }
        Err(_) => reference.to_string(),
    }
}

/// 解析 MPD 文本。
pub fn parse(xml: &str) -> Result<Manifest> {
    let document = Document::parse(xml).context("MPD 不是合法 XML")?;
    let root = document.root_element();
    if root.tag_name().name() != "MPD" {
        bail!("根元素不是 MPD");
    }
    let kind = root.attribute("type").unwrap_or("static").to_string();
    let duration = root
        .attribute("mediaPresentationDuration")
        .map(str::to_string);

    let mut tracks = Vec::new();
    for set in root
        .descendants()
        .filter(|node| node.has_tag_name("AdaptationSet"))
    {
        let set_kind = set_kind(set);
        let lang = set.attribute("lang").map(str::to_string);
        let width = number(set.attribute("maxWidth"));
        let height = number(set.attribute("maxHeight"));

        for representation in children(set, "Representation") {
            tracks.push(Track {
                id: representation.attribute("id").unwrap_or("").to_string(),
                kind: set_kind.clone(),
                codecs: representation.attribute("codecs").map(str::to_string),
                bandwidth: number(representation.attribute("bandwidth")),
                lang: lang.clone(),
                width: number(representation.attribute("width")).or(width),
                height: number(representation.attribute("height")).or(height),
            });
        }
    }
    Ok(Manifest {
        kind,
        duration,
        tracks,
    })
}

/// 解析完整 MPD。
///
/// `mpd_url` 是 MPD 自身的绝对地址，用于解析相对 BaseURL 与媒体地址。
/// 目标要求 `SegmentTemplate` 带 `SegmentTimeline`；本实现同时支持
/// `SegmentTemplate@duration`（有已知总时长时）与 `SegmentList`。
pub fn parse_presentation(xml: &str, mpd_url: &str) -> Result<Presentation> {
    let document = Document::parse(xml).context("MPD 不是合法 XML")?;
    let root = document.root_element();
    if root.tag_name().name() != "MPD" {
        bail!("根元素不是 MPD");
    }
    let is_live = root.attribute("type").unwrap_or("static") == "dynamic";
    let duration = root
        .attribute("mediaPresentationDuration")
        .and_then(parse_iso_duration);
    let time_shift_depth = root
        .attribute("timeShiftBufferDepth")
        .and_then(parse_iso_duration);
    let root_bases = resolve_bases(&[mpd_url.to_string()], &base_urls(root));
    let root_bases = if root_bases.is_empty() {
        vec![mpd_url.to_string()]
    } else {
        root_bases
    };
    let period_template = Template::default();
    let mut streams = Vec::new();

    for period in children(root, "Period") {
        let period_duration = period
            .attribute("duration")
            .and_then(parse_iso_duration)
            .or(duration);
        let period_base = resolve_bases(&root_bases, &base_urls(period));
        let period_base = if period_base.is_empty() {
            root_bases.clone()
        } else {
            period_base
        };
        let period_template = merge_template(&period_template, find_template(period));
        for set in children(period, "AdaptationSet") {
            let kind = set_kind(set);
            let set_base = resolve_bases(&period_base, &base_urls(set));
            let set_base = if set_base.is_empty() {
                period_base.clone()
            } else {
                set_base
            };
            let set_template = merge_template(&period_template, find_template(set));
            let lang = set.attribute("lang").map(str::to_string);
            let width = number(set.attribute("maxWidth"));
            let height = number(set.attribute("maxHeight"));
            let frame_rate = set.attribute("frameRate").map(str::to_string);
            let set_audio_channels = audio_channels(set);

            for representation in children(set, "Representation") {
                let rep_base = resolve_bases(&set_base, &base_urls(representation));
                let rep_base = if rep_base.is_empty() {
                    set_base.clone()
                } else {
                    rep_base
                };
                let template = merge_template(&set_template, find_template(representation));
                let rep_id = representation.attribute("id").unwrap_or("").to_string();
                let bandwidth = number(representation.attribute("bandwidth"));
                let mut segments = Vec::new();
                let mut init = Vec::new();

                if let Some(media) = template.media.as_deref() {
                    segments = build_template_segments(
                        media,
                        &rep_id,
                        bandwidth,
                        &template,
                        period_duration,
                        time_shift_depth,
                        &rep_base,
                    );
                    if let Some(initialization) = template.initialization.as_deref() {
                        let filled = fill_template(
                            initialization,
                            &rep_id,
                            bandwidth,
                            template.start_number.unwrap_or(1),
                            0,
                        );
                        init = resolve_all(&rep_base, &filled);
                    }
                } else if let Some(list) =
                    find_segment_list(representation).or_else(|| find_segment_list(set))
                {
                    let list_template = merge_template(&template, Template::from_list(list));
                    if let Some(initialization) = list_template.initialization.as_deref() {
                        let filled = fill_template(initialization, &rep_id, bandwidth, 1, 0);
                        init = resolve_all(&rep_base, &filled);
                    }
                    for (index, url) in list_template.list_urls.iter().enumerate() {
                        let duration = list_template
                            .duration
                            .map(|value| {
                                value as f64 / list_template.timescale.unwrap_or(1).max(1) as f64
                            })
                            .unwrap_or(0.0);
                        segments.push(Segment {
                            number: list_template.start_number.unwrap_or(1) + index as u64,
                            time: 0,
                            duration,
                            urls: resolve_all(&rep_base, url),
                        });
                    }
                }

                if segments.is_empty() {
                    continue;
                }
                streams.push(Stream {
                    id: rep_id,
                    name: String::new(),
                    kind: kind.clone(),
                    codecs: representation.attribute("codecs").map(str::to_string),
                    bandwidth,
                    lang: lang.clone(),
                    width: number(representation.attribute("width")).or(width),
                    height: number(representation.attribute("height")).or(height),
                    frame_rate: representation
                        .attribute("frameRate")
                        .map(str::to_string)
                        .or_else(|| frame_rate.clone()),
                    audio_channels: audio_channels(representation)
                        .or_else(|| set_audio_channels.clone()),
                    init,
                    segments,
                });
            }
        }
    }

    if streams.is_empty() {
        bail!("MPD 没有可转封装的轨道");
    }
    name_streams(&mut streams);
    Ok(Presentation {
        is_live,
        duration,
        streams,
    })
}

fn audio_channels(node: Node<'_, '_>) -> Option<String> {
    child(node, "AudioChannelConfiguration")
        .and_then(|config| config.attribute("value"))
        .map(str::to_string)
}

fn set_kind(set: Node<'_, '_>) -> String {
    set.attribute("contentType")
        .map(str::to_string)
        .or_else(|| {
            set.attribute("mimeType")
                .map(|mime| mime.split('/').next().unwrap_or("unknown").to_string())
        })
        .or_else(|| {
            children(set, "Representation")
                .into_iter()
                .find_map(|rep| rep.attribute("mimeType"))
                .map(|mime| mime.split('/').next().unwrap_or("unknown").to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// 给轨道分配稳定、可读的 HLS 名称。
fn name_streams(streams: &mut [Stream]) {
    let mut audio = 0usize;
    let mut text = 0usize;
    let mut unknown = 0usize;
    let mut used = HashSet::new();
    for stream in streams {
        let base = match stream.kind.as_str() {
            "video" => match stream.height {
                Some(height) => format!("video_{height}p"),
                None => format!("video-{}", stream.id.trim()),
            },
            "audio" => {
                let name = format!("audio-{audio}");
                audio += 1;
                name
            }
            "text" => {
                let name = format!("text-{text}");
                text += 1;
                name
            }
            _ => {
                let name = format!("track-{unknown}");
                unknown += 1;
                name
            }
        };
        let mut name = base.clone();
        let mut suffix = 2;
        while !used.insert(name.clone()) {
            name = format!("{base}-{suffix}");
            suffix += 1;
        }
        stream.name = name;
    }
}

fn resolve_all(bases: &[String], reference: &str) -> Vec<String> {
    let mut urls = bases
        .iter()
        .map(|base| resolve_url(base, reference))
        .collect::<Vec<_>>();
    dedupe(&mut urls);
    urls
}

/// 解析 ISO-8601 时长（`PT1H2M3.5S`）。
fn parse_iso_duration(value: &str) -> Option<f64> {
    let mut input = value.trim();
    if let Some(rest) = input.strip_prefix("P") {
        input = rest;
    }
    if let Some(rest) = input.strip_prefix("T") {
        input = rest;
    }
    let mut total = 0.0;
    let mut number = String::new();
    for ch in input.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
            continue;
        }
        let value: f64 = number.parse().ok()?;
        number.clear();
        total += match ch {
            'D' => value * 86_400.0,
            'H' => value * 3_600.0,
            'M' => value * 60.0,
            'S' => value,
            _ => return None,
        };
    }
    if total > 0.0 {
        Some(total)
    } else {
        None
    }
}

#[derive(Debug, Clone, Default)]
struct Template {
    media: Option<String>,
    initialization: Option<String>,
    timescale: Option<u64>,
    duration: Option<u64>,
    start_number: Option<u64>,
    timeline: Option<Vec<TimelineEntry>>,
    list_urls: Vec<String>,
}

impl Template {
    fn from_list(node: Node<'_, '_>) -> Self {
        Self {
            duration: number(node.attribute("duration")),
            timescale: number(node.attribute("timescale")),
            initialization: child(node, "Initialization")
                .and_then(|init| init.attribute("sourceURL"))
                .map(str::to_string),
            list_urls: children(node, "SegmentURL")
                .into_iter()
                .filter_map(|segment| segment.attribute("media").map(str::to_string))
                .collect(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TimelineEntry {
    t: Option<u64>,
    d: u64,
    r: i64,
}

fn merge_template(parent: &Template, child: Template) -> Template {
    Template {
        media: child.media.or_else(|| parent.media.clone()),
        initialization: child
            .initialization
            .or_else(|| parent.initialization.clone()),
        timescale: child.timescale.or(parent.timescale),
        duration: child.duration.or(parent.duration),
        start_number: child.start_number.or(parent.start_number),
        timeline: child.timeline.or_else(|| parent.timeline.clone()),
        list_urls: if child.list_urls.is_empty() {
            parent.list_urls.clone()
        } else {
            child.list_urls
        },
    }
}

fn find_template(node: Node<'_, '_>) -> Template {
    let Some(node) = child(node, "SegmentTemplate") else {
        return Template::default();
    };
    let mut template = Template {
        media: node.attribute("media").map(str::to_string),
        initialization: node.attribute("initialization").map(str::to_string),
        timescale: number(node.attribute("timescale")),
        duration: number(node.attribute("duration")),
        start_number: number(node.attribute("startNumber")),
        timeline: None,
        list_urls: Vec::new(),
    };
    if let Some(timeline) = child(node, "SegmentTimeline") {
        template.timeline = Some(
            children(timeline, "S")
                .into_iter()
                .map(|entry| TimelineEntry {
                    t: number(entry.attribute("t")),
                    d: number(entry.attribute("d")).unwrap_or(0),
                    r: entry
                        .attribute("r")
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0),
                })
                .collect(),
        );
    }
    template
}

fn find_segment_list<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    child(node, "SegmentList")
}

fn build_template_segments(
    media: &str,
    rep_id: &str,
    bandwidth: Option<u64>,
    template: &Template,
    period_duration: Option<f64>,
    time_shift_depth: Option<f64>,
    bases: &[String],
) -> Vec<Segment> {
    let timescale = template.timescale.unwrap_or(1).max(1);
    let start_number = template.start_number.unwrap_or(1);
    let mut segments = Vec::new();
    let timeline = template.timeline.as_deref().unwrap_or(&[]);

    if !timeline.is_empty() {
        let mut time = 0u64;
        let mut number = start_number;
        for (index, entry) in timeline.iter().enumerate() {
            if entry.d == 0 {
                continue;
            }
            if let Some(t) = entry.t {
                time = t;
            }
            let repeat = if entry.r < 0 {
                let next_time = timeline.get(index + 1).and_then(|next| next.t);
                let end_time = next_time
                    .or_else(|| {
                        time_shift_depth
                            .map(|depth| time.saturating_add((depth * timescale as f64) as u64))
                    })
                    .or_else(|| {
                        period_duration.map(|duration| {
                            time.saturating_add((duration * timescale as f64) as u64)
                        })
                    });
                match end_time {
                    Some(end) if end > time => (end - time).div_ceil(entry.d).max(1).min(10_000),
                    _ => 1,
                }
            } else {
                entry.r as u64 + 1
            };
            for _ in 0..repeat {
                let filled = fill_template(media, rep_id, bandwidth, number, time);
                segments.push(Segment {
                    number,
                    time,
                    duration: entry.d as f64 / timescale as f64,
                    urls: resolve_all(bases, &filled),
                });
                number += 1;
                time = time.saturating_add(entry.d);
            }
        }
        return segments;
    }

    let Some(duration) = template.duration.filter(|value| *value > 0) else {
        return segments;
    };
    let Some(total) = period_duration else {
        return segments;
    };
    let count = ((total * timescale as f64) / duration as f64).ceil() as u64;
    let mut time = 0u64;
    for index in 0..count {
        let number = start_number + index;
        let filled = fill_template(media, rep_id, bandwidth, number, time);
        segments.push(Segment {
            number,
            time,
            duration: duration as f64 / timescale as f64,
            urls: resolve_all(bases, &filled),
        });
        time = time.saturating_add(duration);
    }
    segments
}

/// 替换 `$RepresentationID$`、`$Number%05d$`、`$Time$`、`$Bandwidth$`。
fn fill_template(
    template: &str,
    rep_id: &str,
    bandwidth: Option<u64>,
    number: u64,
    time: u64,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('$') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        if token.is_empty() {
            out.push('$');
        } else {
            match token {
                "RepresentationID" => out.push_str(rep_id),
                "Bandwidth" => out.push_str(&bandwidth.unwrap_or(0).to_string()),
                _ => {
                    let (name, width) = token
                        .split_once('%')
                        .map(|(name, spec)| (name, pad_width(spec)))
                        .unwrap_or((token, None));
                    match name {
                        "Number" => out.push_str(&format_number(number, width)),
                        "Time" => out.push_str(&format_number(time, width)),
                        _ => {
                            out.push('$');
                            out.push_str(token);
                            out.push('$');
                            continue;
                        }
                    }
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn pad_width(spec: &str) -> Option<usize> {
    let digits = spec.trim_end_matches('d');
    if digits.is_empty() {
        None
    } else {
        digits.trim_start_matches('0').parse().ok()
    }
}

fn format_number(value: u64, width: Option<usize>) -> String {
    match width {
        Some(width) => format!("{value:0width$}"),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" mediaPresentationDuration="PT1H">
  <Period>
    <AdaptationSet contentType="video" maxWidth="1280" maxHeight="720">
      <Representation id="v0" codecs="avc1.4d401f" bandwidth="2500000" width="1280" height="720"/>
      <Representation id="v1" codecs="avc1.4d401e" bandwidth="800000" width="640" height="360"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="zh">
      <Representation id="a0" codecs="mp4a.40.2" bandwidth="128000"/>
    </AdaptationSet>
    <AdaptationSet contentType="text" lang="en">
      <Representation id="t0" mimeType="text/vtt"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

    const TIMELINE: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT12S">
  <BaseURL>http://origin.example/dash/</BaseURL>
  <Period>
    <AdaptationSet contentType="video" maxWidth="1280" maxHeight="720">
      <SegmentTemplate timescale="1000" initialization="init-$RepresentationID$.mp4"
        media="chunk-$RepresentationID$-$Number%03d$.m4s" startNumber="7">
        <SegmentTimeline>
          <S t="0" d="4000" r="2"/>
        </SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" codecs="avc1.4d401f" bandwidth="2500000" width="1280" height="720"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn parses_tracks() {
        let manifest = parse(SAMPLE).expect("解析");
        assert_eq!(manifest.kind, "dynamic");
        assert_eq!(manifest.duration.as_deref(), Some("PT1H"));
        assert_eq!(manifest.tracks.len(), 4);
        assert_eq!(manifest.tracks[0].kind, "video");
        assert_eq!(manifest.tracks[0].width, Some(1280));
        assert_eq!(manifest.tracks[1].height, Some(360));
        assert_eq!(manifest.tracks[2].kind, "audio");
        assert_eq!(manifest.tracks[2].lang.as_deref(), Some("zh"));
        assert_eq!(manifest.tracks[3].kind, "text");
        assert_eq!(manifest.tracks[3].bandwidth, None);
    }

    #[test]
    fn rejects_non_mpd() {
        assert!(parse("<html/>").is_err());
        assert!(parse("not xml").is_err());
    }

    #[test]
    fn parses_timeline_and_base_url() {
        let presentation = parse_presentation(TIMELINE, "http://origin.example/live/master.mpd")
            .expect("解析演示");
        assert!(!presentation.is_live);
        assert_eq!(presentation.streams.len(), 1);
        let stream = &presentation.streams[0];
        assert_eq!(stream.name, "video_720p");
        assert_eq!(stream.segments.len(), 3);
        assert_eq!(stream.segments[0].number, 7);
        assert_eq!(stream.segments[0].duration, 4.0);
        assert_eq!(
            stream.segments[0].urls[0],
            "http://origin.example/dash/chunk-v0-007.m4s"
        );
        assert_eq!(stream.init[0], "http://origin.example/dash/init-v0.mp4");
    }

    #[test]
    fn expands_negative_repeat_with_time_shift_depth() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" timeShiftBufferDepth="PT6S">
  <Period>
    <AdaptationSet contentType="video">
      <SegmentTemplate timescale="1000" media="s-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S t="1000" d="2000" r="-1"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let presentation = parse_presentation(xml, "http://origin.example/live.mpd").expect("解析");
        assert!(presentation.is_live);
        assert_eq!(presentation.streams[0].segments.len(), 3);
        assert_eq!(presentation.streams[0].segments[0].time, 1000);
    }

    #[test]
    fn carries_base_query_to_relative_segments() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S">
  <BaseURL>http://cdn.example/dash/?token=abc</BaseURL>
  <Period>
    <AdaptationSet contentType="video">
      <SegmentTemplate timescale="1000" media="s-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="0"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="v0" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let presentation = parse_presentation(xml, "http://origin.example/live.mpd").expect("解析");
        assert_eq!(
            presentation.streams[0].segments[0].urls[0],
            "http://cdn.example/dash/s-1.m4s?token=abc"
        );
    }

    #[test]
    fn keeps_multiple_base_urls_for_pool() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
  <BaseURL>http://cdn-a.example/dash/</BaseURL>
  <BaseURL>http://cdn-b.example/dash/</BaseURL>
  <Period>
    <AdaptationSet contentType="audio">
      <SegmentTemplate timescale="1000" media="a-$Number$.m4s" startNumber="1">
        <SegmentTimeline><S d="2000" r="1"/></SegmentTimeline>
      </SegmentTemplate>
      <Representation id="a0" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let presentation = parse_presentation(xml, "http://origin.example/live.mpd").expect("解析");
        let segment = &presentation.streams[0].segments[0];
        assert_eq!(segment.urls.len(), 2);
        assert_eq!(segment.urls[0], "http://cdn-a.example/dash/a-1.m4s");
        assert_eq!(segment.urls[1], "http://cdn-b.example/dash/a-1.m4s");
    }

    #[test]
    fn parses_segment_list() {
        let xml = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT8S">
  <Period>
    <AdaptationSet contentType="audio">
      <Representation id="a0" bandwidth="128000">
        <SegmentList timescale="1000" duration="2000">
          <Initialization sourceURL="init.mp4"/>
          <SegmentURL media="a-1.m4s"/>
          <SegmentURL media="a-2.m4s"/>
        </SegmentList>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let presentation = parse_presentation(xml, "http://origin.example/a.mpd").expect("解析");
        let stream = &presentation.streams[0];
        assert_eq!(stream.name, "audio-0");
        assert_eq!(stream.segments.len(), 2);
        assert_eq!(stream.segments[1].urls[0], "http://origin.example/a-2.m4s");
    }
}
