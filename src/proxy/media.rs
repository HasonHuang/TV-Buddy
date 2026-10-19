//! 清单改写：把上游媒体清单改成本地伪装地址。
//!
//! - 切片地址 → `/{gid}/{cid}/<P>_<S>.<ext>`（S-DIS-04）。
//! - `<S>` = `sha256(上游绝对 URL)` 前 32 位十六进制。
//! - `<P>` = `sha256("<gid>:<cid>")` 前 16 位十六进制（自研，F-017 未解）。
//! - 主清单（含 `#EXT-X-STREAM-INF`）只把变体地址绝对化。

use sha2::{Digest, Sha256};
use url::Url;

/// 改写产物。
#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    /// 改写后的清单正文。
    pub body: String,
    /// 上游是主清单（变体清单，不是切片）。
    pub is_master: bool,
    /// 新登记的切片：`(S, 上游绝对 URL, 扩展名)`。
    pub segments: Vec<(String, String, String)>,
}

/// 计算伪装前缀 `<P>`。
pub fn prefix(gid: &str, cid: &str) -> String {
    let digest = Sha256::digest(format!("{gid}:{cid}").as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// 计算伪装后缀 `<S>`。
pub fn suffix(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    hex::encode(digest)[..32].to_string()
}

/// 解析切片文件名：返回 `(S, 扩展名)`。前缀缺失时也接受 `<S>.<ext>`。
pub fn parse_segment(name: &str) -> Option<(String, String)> {
    let (stem, ext) = name.rsplit_once('.')?;
    if ext.is_empty() {
        return None;
    }
    let suffix_part = stem.rsplit('_').next()?;
    if suffix_part.len() != 32 || !suffix_part.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((suffix_part.to_ascii_lowercase(), ext.to_ascii_lowercase()))
}

/// 校验前缀。空前缀（伪装关闭）视为通过。
pub fn prefix_matches(expected: &str, name: &str) -> bool {
    match name.split_once('_') {
        Some((got, _)) => got.eq_ignore_ascii_case(expected),
        None => true,
    }
}

/// 每频道固定的扩展名轮换起点。
pub fn ext_offset(gid: &str, cid: &str) -> usize {
    Sha256::digest(format!("{gid}:{cid}").as_bytes())[0] as usize
}

/// 改写清单正文。
///
/// `base` 是清单自身的绝对 URL，用于解析相对地址。
/// `disguise` 关闭时切片地址改写为上游绝对地址。
pub fn rewrite_media(
    body: &str,
    base: &str,
    gid: &str,
    cid: &str,
    formats: &[String],
    disguise: bool,
) -> Rendered {
    let is_master = body.contains("#EXT-X-STREAM-INF");
    let prefix = prefix(gid, cid);
    let offset = ext_offset(gid, cid);
    let mut out = String::with_capacity(body.len() + 256);
    let mut segments = Vec::new();
    let mut index = 0usize;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            out.push_str(&rewrite_tags(rest, base));
            out.push('\n');
            continue;
        }
        let Ok(absolute) = absolute_url(base, trimmed) else {
            out.push_str(trimmed);
            out.push('\n');
            continue;
        };
        if is_master || !disguise {
            out.push_str(&absolute);
            out.push('\n');
            continue;
        }
        let ext = pick_ext(trimmed, formats, offset + index);
        let suffix = suffix(&absolute);
        out.push_str(&format!("/{gid}/{cid}/{prefix}_{suffix}.{ext}\n"));
        segments.push((suffix, absolute, ext));
        index += 1;
    }

    Rendered {
        body: out,
        is_master,
        segments,
    }
}

/// 改写带 `URI="…"` 的标签（密钥、映射、备用音轨）。
fn rewrite_tags(tag: &str, base: &str) -> String {
    if !tag.contains("URI=\"") {
        return format!("#{tag}");
    }
    let mut out = String::from("#");
    let mut rest = tag;
    while let Some(start) = rest.find("URI=\"") {
        let (head, tail) = rest.split_at(start + 5);
        out.push_str(head);
        match tail.find('"') {
            Some(end) => {
                let (uri, after) = tail.split_at(end);
                let absolute = absolute_url(base, uri).unwrap_or_else(|_| uri.to_string());
                out.push_str(&absolute);
                rest = after;
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// 解析相对地址。
fn absolute_url(base: &str, reference: &str) -> Result<String, url::ParseError> {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return Ok(reference.to_string());
    }
    Url::parse(base).map(|base| base.join(reference).unwrap_or(base).to_string())
}

/// 选择伪装扩展名：按频道序列轮换；伪装关闭或列表为空时用原扩展名。
fn pick_ext(reference: &str, formats: &[String], index: usize) -> String {
    if !formats.is_empty() {
        return formats[index % formats.len()].trim_start_matches('.').to_lowercase();
    }
    reference
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_else(|| "bin".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats() -> Vec<String> {
        ["jpg", "png", "gif", "bmp", "webp"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn prefix_is_stable_16_hex() {
        let first = prefix("g1", "c1");
        assert_eq!(first, prefix("g1", "c1"));
        assert_eq!(first.len(), 16);
        assert_ne!(first, prefix("g1", "c2"));
    }

    #[test]
    fn suffix_matches_sha256_prefix() {
        let url = "http://up.example/a/1.ts";
        let expected = hex::encode(Sha256::digest(url.as_bytes()))[..32].to_string();
        assert_eq!(suffix(url), expected);
    }

    #[test]
    fn relative_segments_are_absolutized_and_disguised() {
        let body = "#EXTM3U\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\n../b/seg2.ts\n";
        let rendered = rewrite_media(
            body,
            "http://up.example/live/a/index.m3u8",
            "g1",
            "c1",
            &formats(),
            true,
        );
        assert!(!rendered.is_master);
        assert_eq!(rendered.segments.len(), 2);
        assert_eq!(rendered.segments[0].1, "http://up.example/live/a/seg1.ts");
        assert_eq!(rendered.segments[1].1, "http://up.example/live/b/seg2.ts");
        assert!(rendered
            .body
            .contains(&format!("/g1/c1/{}_", prefix("g1", "c1"))));
        assert!(rendered.body.contains(&format!("_{}.", rendered.segments[0].0)));
        assert!(!rendered.body.contains("seg1.ts"));
    }

    #[test]
    fn master_playlist_keeps_variants_absolute() {
        let body = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nv0/index.m3u8\n";
        let rendered = rewrite_media(
            body,
            "http://up.example/live/master.m3u8",
            "g1",
            "c1",
            &formats(),
            true,
        );
        assert!(rendered.is_master);
        assert!(rendered.segments.is_empty());
        assert!(rendered.body.contains("http://up.example/live/v0/index.m3u8"));
    }

    #[test]
    fn key_uri_is_absolutized() {
        let body = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\nseg.ts\n";
        let rendered = rewrite_media(
            body,
            "http://up.example/live/a/index.m3u8",
            "g1",
            "c1",
            &formats(),
            true,
        );
        assert!(rendered
            .body
            .contains("#EXT-X-KEY:METHOD=AES-128,URI=\"http://up.example/live/a/key.bin\""));
    }

    #[test]
    fn disguise_off_uses_absolute_urls() {
        let body = "#EXTM3U\n#EXTINF:4.0,\nseg1.ts\n";
        let rendered = rewrite_media(
            body,
            "http://up.example/live/a/index.m3u8",
            "g1",
            "c1",
            &formats(),
            false,
        );
        assert!(rendered.body.contains("http://up.example/live/a/seg1.ts"));
        assert!(rendered.segments.is_empty());
    }

    #[test]
    fn segment_name_parsing() {
        let name = format!("{}_{}.jpg", prefix("g1", "c1"), suffix("http://x/1.ts"));
        let (parsed_suffix, ext) = parse_segment(&name).unwrap();
        assert_eq!(parsed_suffix, suffix("http://x/1.ts"));
        assert_eq!(ext, "jpg");
        assert!(prefix_matches(&prefix("g1", "c1"), &name));
        assert!(!prefix_matches(&prefix("g1", "c2"), &name));
        assert!(parse_segment("plain.ts").is_none());
    }

    #[test]
    fn extension_rotates_per_channel() {
        let body = "#EXTM3U\n#EXTINF:4.0,\n1.ts\n#EXTINF:4.0,\n2.ts\n";
        let rendered = rewrite_media(
            body,
            "http://up.example/a/index.m3u8",
            "g1",
            "c1",
            &formats(),
            true,
        );
        let first = rendered.segments[0].2.clone();
        let second = rendered.segments[1].2.clone();
        assert_ne!(first, second, "同一频道内扩展名轮换");
    }
}
