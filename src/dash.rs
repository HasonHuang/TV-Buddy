//! DASH 清单（MPD）解析：只提取面板需要的轨道摘要。
//!
//! 目标 `/panel/api/dash/tracks/:gid/:cid` 的响应形状未采证（F-011），格式自定。
//! 本模块不下载媒体，只解析 XML。

use anyhow::{bail, Context, Result};
use roxmltree::Document;
use serde::Serialize;

/// 单条轨道。
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

fn number(value: Option<&str>) -> Option<u64> {
    value.and_then(|text| text.trim().parse().ok())
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
        let set_kind = set
            .attribute("contentType")
            .map(str::to_string)
            .or_else(|| {
                set.attribute("mimeType")
                    .map(|mime| mime.split('/').next().unwrap_or("unknown").to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());
        let lang = set.attribute("lang").map(str::to_string);
        let width = number(set.attribute("maxWidth"));
        let height = number(set.attribute("maxHeight"));

        for representation in set
            .children()
            .filter(|node| node.has_tag_name("Representation"))
        {
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
}
