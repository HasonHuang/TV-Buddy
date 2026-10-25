//! 清单改写：把上游媒体清单改成本地伪装地址。
//!
//! - 切片地址 → `{self_base}/{gid}/{cid}/<P>_<S>.<ext>`（绝对地址，S-DIS-04）。
//! - `<S>` = `sha256(上游绝对 URL)` 前 32 位十六进制（与目标逐字节一致）。
//! - `<P>` = 16 位十六进制，纯函数 `F(gid, cid)`（自研，F-017 未解；目标算法不需复刻，
//!   只需满足目标可观测的四条性质，见 [`channel_digest`]）。
//! - 主清单（含 `#EXT-X-STREAM-INF`）只把变体地址绝对化。

use sha2::{Digest, Sha256};
use url::Url;

/// 频道派生摘要的域分隔标签。
///
/// `<P>`、扩展名轮换起点与切片索引文件名都从这一个摘要派生：加域标签是给
/// 摘要一个用途边界，换算法时不会与其它哈希混用。
const CHANNEL_DOMAIN: &[u8] = b"rea-iptv/segment-v1";

/// 频道派生摘要：`sha256(域标签 ‖ len(gid) ‖ gid ‖ len(cid) ‖ cid)`。
///
/// 目标 `<P>` 的**具体算法未解**（F-017：已排除 2 万余种模板与主流哈希族）。
/// 但目标对 `<P>` 的可观测性质只有四条，本函数全部满足，因此不追字节一致：
///
/// 1. 16 位十六进制；
/// 2. 纯函数 `F(gid, cid)`——与 `name`、`url`、请求头、缓存与重启无关；
/// 3. 参与校验：缺 / 截短 / 错值 → `302` 占位（S-DIS-17）；
/// 4. 不同的 `(gid, cid)` 必须得到不同的 `<P>`。
///
/// 第 4 条是**长度前缀**的作用：用 `"{gid}:{cid}"` 这类分隔符拼接时，
/// `("a:b", "c")` 与 `("a", "b:c")` 会拼成同一个字符串，两个频道共用一个 `<P>`，
/// 并共用一份切片索引文件，等于把一个频道的切片地址泄漏给另一个。长度前缀
/// 让拼接无歧义，无需假设 id 不含某个字符。
fn channel_digest(gid: &str, cid: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CHANNEL_DOMAIN);
    hasher.update([0]);
    for part in [gid, cid] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

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

/// 计算伪装前缀 `<P>`：频道摘要的前 16 位十六进制。
pub fn prefix(gid: &str, cid: &str) -> String {
    hex::encode(channel_digest(gid, cid))[..16].to_string()
}

/// 计算伪装后缀 `<S>`。
pub fn suffix(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    hex::encode(digest)[..32].to_string()
}

/// 切片文件名的解析结果。
#[derive(Debug, PartialEq, Eq)]
pub struct SegmentName {
    /// `<P>`（16 位小写 hex）。
    pub prefix: String,
    /// `<S>`（32 位小写 hex）。
    pub suffix: String,
    /// 扩展名，可为空（`.ext` 整段可选）。
    pub ext: String,
}

/// 解析切片文件名 `<P>_<S>[.<ext>]`。
///
/// 文法按目标 v0.8.2 实测（2026-10-25，容器 `rea82v09`）逐条对齐：
///
/// | 输入 | 目标 |
/// | --- | --- |
/// | `<P>_<S>` / `<P>_<S>.` / `<P>_<S>.j` | `200`（`Content-Type: video/mp2t`） |
/// | `<P>_<S>.a.b` / `<P>_<S>..` | `302 error_url`（扩展名里不得再出现 `.`） |
/// | `x_<P>_<S>` / `<P>__<S>` / `<P>_<S>_x` | `302 error_url`（只允许恰好一个下划线） |
/// | 缺 / 截短 / 超长 / **大写** / 非 hex 的 `<P>` 或 `<S>` | `302 error_url` |
///
/// 所以：扩展名**整段可用**但最多一个点，`<P>`/`<S>` 必须是**定长小写** hex。
/// 目标对 `<P>` 大小写敏感（首位改成大写即 `302`），这里不做大小写折叠。
pub fn parse_segment(name: &str) -> Option<SegmentName> {
    // 扩展名可选：至多一个 `.`，且 `ext` 内不允许再有 `.`。
    let (stem, ext) = match name.split_once('.') {
        Some((stem, ext)) if !ext.contains('.') => (stem, ext),
        Some(_) => return None,
        None => (name, ""),
    };
    let (prefix, suffix) = stem.split_once('_')?;
    // `__`、`_x` 等都会落在同一处：`suffix` 长度或字符集不符即失败。
    if !is_lower_hex(prefix, 16) || !is_lower_hex(suffix, 32) {
        return None;
    }
    Some(SegmentName {
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
        ext: ext.to_ascii_lowercase(),
    })
}

/// 定长小写十六进制判定（目标拒绝大写，见 [`parse_segment`]）。
fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// 校验切片名的 `<P>` 是否等于 `expected`。
///
/// 目标对 `<P>` **大小写敏感**（实测：首位改大写即 `302`），所以这里逐字符相等，
/// 不做大小写折叠。形如 `<S>.jpg` 的缺前缀名字没有下划线，直接判失败
/// （S-DIS-17：缺 `<P>` → `302`）。伪装关闭时不生成伪装地址，走到这里的名不是
/// 伪装形状，因此不需要「空前缀视为通过」的旁路。
pub fn prefix_matches(expected: &str, name: &str) -> bool {
    match name.split_once('_') {
        Some((got, _)) => got == expected,
        None => false,
    }
}

/// 扩展名 → 切片 `Content-Type`（S-DIS-13）。
///
/// 目标实测是一张 **5 项白名单**（外加 `jpeg` 同义）：
/// `jpg/jpeg → image/jpeg`、`png/gif/bmp/webp` 各自对应，**其余一律
/// `video/mp2t`**——包括空扩展名、`ts`、`m4s`、`txt`、`m3u8`、`svg`、`ico`、`avif`。
///
/// 不用 `mime_guess`：它会给出 `video/vnd.dlna.mpeg-tts`（`ts`）、`image/svg+xml`
/// 等与目标不同的值，而 `Content-Type` 是伪装面的一部分。
pub fn segment_content_type(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        _ => "video/mp2t",
    }
}

/// 每频道固定的扩展名轮换起点。
pub fn ext_offset(gid: &str, cid: &str) -> usize {
    channel_digest(gid, cid)[0] as usize
}

/// 清单是否为主清单（含变体）。
pub fn is_master(body: &str) -> bool {
    body.contains("#EXT-X-STREAM-INF")
}

/// 清单的 `#EXT-X-TARGETDURATION`（秒）。缺失时返回 `None`。
pub fn target_duration(body: &str) -> Option<u64> {
    body.lines().find_map(|line| {
        line.trim()
            .strip_prefix("#EXT-X-TARGETDURATION:")?
            .trim()
            .parse()
            .ok()
    })
}

/// 窗口统计：`(切片数, 时长总和秒)`。一条 `#EXTINF` 都没有时返回 `None`。
///
/// 供被动实测推算推荐 TTL 与磁盘工作集
/// （`docs/design/06-low-spec-optimization.md` §3.3）。
pub fn window_stats(body: &str) -> Option<(u64, f64)> {
    let mut count = 0u64;
    let mut sum = 0.0f64;
    for line in body.lines() {
        let Some(rest) = line.trim().strip_prefix("#EXTINF:") else {
            continue;
        };
        // 形如 `#EXTINF:8.00000000,标题`；标题可缺省。
        let duration = rest.split(',').next().unwrap_or(rest).trim();
        let Ok(secs) = duration.parse::<f64>() else {
            continue;
        };
        count += 1;
        sum += secs;
    }
    (count > 0).then_some((count, sum))
}

/// 主清单 `#EXT-X-STREAM-INF` 的属性白名单。
///
/// v0.8.2 实测只保留这 4 项；`PROGRAM-ID`、`NAME`、`AUDIO`、`SUBTITLES` 等均丢弃。
const VARIANT_ATTRS: [&str; 4] = ["BANDWIDTH", "CODECS", "RESOLUTION", "FRAME-RATE"];

/// 按 v0.8.2 白名单过滤 `#EXT-X-STREAM-INF` 属性，保持原顺序。
pub fn normalize_variant_attrs(attrs: &str) -> String {
    let kept: Vec<&str> = split_attrs(attrs)
        .into_iter()
        .filter(|part| {
            part.split_once('=')
                .map(|(name, _)| VARIANT_ATTRS.contains(&name.trim()))
                .unwrap_or(false)
        })
        .collect();
    if kept.is_empty() {
        "BANDWIDTH=1000000".to_string()
    } else {
        kept.join(",")
    }
}

/// 按逗号切分属性串；忽略引号内的逗号（如 `CODECS="avc1,mp4a"`）。
fn split_attrs(attrs: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut quoted = false;
    for (index, ch) in attrs.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                let part = attrs[start..index].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = attrs[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

/// 主清单里的最佳变体：返回 `(STREAM-INF 属性串, 绝对地址)`。
///
/// 按 `BANDWIDTH` 取最大；一条变体都没有时返回 `None`。
/// 属性串保持上游原样；对外输出前应经 [`normalize_variant_attrs`]。
pub fn best_variant(body: &str, base: &str) -> Option<(String, String)> {
    let mut attrs: Option<String> = None;
    let mut best: Option<(u64, String, String)> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#EXT-X-STREAM-INF:") {
            attrs = Some(rest.to_string());
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(current) = attrs.take() else {
            continue;
        };
        let Ok(url) = absolute_url(base, trimmed) else {
            continue;
        };
        let bandwidth = bandwidth_of(&current);
        if best
            .as_ref()
            .map_or(true, |(known, _, _)| bandwidth > *known)
        {
            best = Some((bandwidth, current, url));
        }
    }
    best.map(|(_, attrs, url)| (attrs, url))
}

/// 解析 `BANDWIDTH=…`；缺失时按 0 处理。
fn bandwidth_of(attrs: &str) -> u64 {
    split_attrs(attrs)
        .into_iter()
        .find_map(|part| part.strip_prefix("BANDWIDTH="))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

/// 改写清单正文。
///
/// `base` 是清单自身的绝对 URL，用于解析相对地址。
/// `self_base` 是本服务的对外基址（`http://主机:端口`），用于拼绝对伪装地址。
/// `disguise` 关闭时切片地址改写为上游绝对地址。
pub fn rewrite_media(
    body: &str,
    base: &str,
    gid: &str,
    cid: &str,
    formats: &[String],
    disguise: bool,
    self_base: &str,
) -> Rendered {
    let is_master = is_master(body);
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
        out.push_str(&format!(
            "{self_base}/{gid}/{cid}/{prefix}_{suffix}.{ext}\n"
        ));
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

/// 选择伪装扩展名。
///
/// v0.8.2 实测：上游扩展名已在 `formats` 中时直接保留（如 `.jpg` 切片仍为 `.jpg`）；
/// 否则按频道序列轮换。伪装关闭或列表为空时用原扩展名。
fn pick_ext(reference: &str, formats: &[String], index: usize) -> String {
    let original = reference
        .split('?')
        .next()
        .and_then(|path| path.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    if formats.is_empty() {
        return original.unwrap_or_else(|| "bin".to_string());
    }
    if let Some(ext) = &original {
        if formats
            .iter()
            .any(|format| format.trim_start_matches('.').eq_ignore_ascii_case(ext))
        {
            return ext.clone();
        }
    }
    formats[index % formats.len()]
        .trim_start_matches('.')
        .to_lowercase()
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
        assert!(
            first.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{first}"
        );
        assert_ne!(first, prefix("g1", "c2"));
        // 固定值：改算法必须同时改这里，避免无意中改掉已下发的切片地址。
        assert_eq!(first, "f2653cf674a55a0e");
    }

    /// 目标 `<P>` 的第 4 条性质：不同 `(gid, cid)` 必须得到不同前缀。
    ///
    /// 分隔符拼接（`"{gid}:{cid}"`）在 id 含分隔符时会把两个频道映射到同一个
    /// `<P>`，进而共用切片索引文件。长度前缀消除了这种歧义。
    #[test]
    fn prefix_has_no_separator_collision() {
        assert_ne!(prefix("a:b", "c"), prefix("a", "b:c"));
        assert_ne!(prefix("g1/c1", "x"), prefix("g1", "c1/x"));
        assert_ne!(prefix("", "ab"), prefix("a", "b"));
        // 顺序敏感：交换 gid/cid 必须换前缀。
        assert_ne!(prefix("g1", "c2"), prefix("c2", "g1"));
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
            "http://self",
        );
        assert!(!rendered.is_master);
        assert_eq!(rendered.segments.len(), 2);
        assert_eq!(rendered.segments[0].1, "http://up.example/live/a/seg1.ts");
        assert_eq!(rendered.segments[1].1, "http://up.example/live/b/seg2.ts");
        assert!(rendered
            .body
            .contains(&format!("http://self/g1/c1/{}_", prefix("g1", "c1"))));
        assert!(rendered
            .body
            .contains(&format!("_{}.", rendered.segments[0].0)));
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
            "http://self",
        );
        assert!(rendered.is_master);
        assert!(rendered.segments.is_empty());
        assert!(rendered
            .body
            .contains("http://up.example/live/v0/index.m3u8"));
    }

    #[test]
    fn target_duration_is_read() {
        assert_eq!(
            target_duration("#EXTM3U\n#EXT-X-TARGETDURATION:12\n"),
            Some(12)
        );
        assert_eq!(target_duration("#EXTM3U\n"), None);
    }

    #[test]
    fn window_stats_counts_extinf() {
        let body = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:8\n\
                    #EXTINF:8.00000000,\n1.jpg\n#EXTINF:8.00000000,\n2.jpg\n\
                    #EXTINF:8.00000000,\n3.jpg\n#EXTINF:8.00000000,\n4.jpg\n\
                    #EXTINF:8.00000000,\n5.jpg\n#EXTINF:8.00000000,\n6.jpg\n\
                    #EXTINF:8.00000000,\n7.jpg\n#EXT-X-ENDLIST\n";
        let (count, sum) = window_stats(body).expect("有 EXTINF");
        assert_eq!(count, 7);
        assert!((sum - 56.0).abs() < 1e-9, "{sum}");
        assert_eq!(target_duration(body), Some(8));
    }

    #[test]
    fn window_stats_ignores_master_and_bad_lines() {
        assert!(window_stats("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nv/index.m3u8\n").is_none());
        assert!(window_stats("#EXTM3U\n").is_none());
        // 时长非数字的行被跳过，不计入条数。
        assert_eq!(
            window_stats("#EXTINF:,\na.ts\n#EXTINF:4.5,\nb.ts\n"),
            Some((1, 4.5))
        );
    }

    #[test]
    fn variant_attrs_follow_target_allowlist() {
        let attrs = r#"PROGRAM-ID=1,BANDWIDTH=123456,AVERAGE-BANDWIDTH=100000,CODECS="avc1.64001f,mp4a.40.2",RESOLUTION=640x360,FRAME-RATE=25.000,NAME="hi",AUDIO="aud",SUBTITLES="sub""#;
        assert_eq!(
            normalize_variant_attrs(attrs),
            r#"BANDWIDTH=123456,CODECS="avc1.64001f,mp4a.40.2",RESOLUTION=640x360,FRAME-RATE=25.000"#
        );
        assert_eq!(normalize_variant_attrs("PROGRAM-ID=1"), "BANDWIDTH=1000000");
    }

    #[test]
    fn bandwidth_ignores_quoted_commas() {
        let attrs = r#"CODECS="avc1.64001f,mp4a.40.2",BANDWIDTH=800000"#;
        assert_eq!(bandwidth_of(attrs), 800000);
    }

    #[test]
    fn best_variant_takes_highest_bandwidth() {
        let body = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=640x360\n360/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1920x1080\n1080/index.m3u8\n";
        let (attrs, url) = best_variant(body, "http://up.example/live/master.m3u8").unwrap();
        assert_eq!(attrs, "BANDWIDTH=2000000,RESOLUTION=1920x1080");
        assert_eq!(url, "http://up.example/live/1080/index.m3u8");
        assert!(best_variant("#EXTM3U\nseg.ts\n", "http://up.example/a.m3u8").is_none());
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
            "http://self",
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
            "http://self",
        );
        assert!(rendered.body.contains("http://up.example/live/a/seg1.ts"));
        assert!(rendered.segments.is_empty());
    }

    #[test]
    fn segment_name_parsing() {
        let name = format!("{}_{}.jpg", prefix("g1", "c1"), suffix("http://x/1.ts"));
        let parsed = parse_segment(&name).expect("形状合法");
        assert_eq!(parsed.suffix, suffix("http://x/1.ts"));
        assert_eq!(parsed.ext, "jpg");
        assert_eq!(parsed.prefix, prefix("g1", "c1"));
        assert!(prefix_matches(&prefix("g1", "c1"), &name));
        assert!(!prefix_matches(&prefix("g1", "c2"), &name));
        assert!(parse_segment("plain.ts").is_none());
    }

    /// S-DIS-17：`<P>` 缺失 / 截短 / 非十六进制都必须被判为无效。
    ///
    /// 目标实测（F-017 §3）：`/{gid}/{cid}/<S>.jpg` → `302`，
    /// `/{gid}/{cid}/ea86_<S>.jpg` → `302`。早先的实现对「没有下划线」的名字
    /// 直接放行，等于让缺 `<P>` 的地址拿到切片。
    #[test]
    fn segment_name_requires_prefix() {
        let s = suffix("http://x/1.ts");
        assert!(
            parse_segment(&format!("{s}.jpg")).is_none(),
            "缺 <P> 应无效"
        );
        assert!(!prefix_matches(&prefix("g1", "c1"), &format!("{s}.jpg")));
        assert!(
            parse_segment(&format!("ea86_{s}.jpg")).is_none(),
            "截短 <P> 应无效"
        );
        assert!(parse_segment(&format!("zzzzzzzzzzzzzzzz_{s}.jpg")).is_none());
        assert!(parse_segment(&format!("{}_ea86.jpg", prefix("g1", "c1"))).is_none());
    }

    /// 扩展名不参与校验：`_<S>.png` 与 `.jpg` 都解析成功（S-DIS-05）。
    #[test]
    fn extension_is_not_validated() {
        let p = prefix("g1", "c1");
        let s = suffix("http://x/1.ts");
        for ext in ["jpg", "png", "ts", "txt", "m3u8", "webp"] {
            let name = format!("{p}_{s}.{ext}");
            let parsed = parse_segment(&name).unwrap_or_else(|| panic!("{ext} 应可解析"));
            assert_eq!(parsed.ext, ext);
            assert!(prefix_matches(&p, &name));
        }
        // 整段扩展名可以不写（目标实测：`<P>_<S>` 与 `<P>_<S>.` 都 200）。
        for name in [format!("{p}_{s}"), format!("{p}_{s}.")] {
            let parsed = parse_segment(&name).expect("扩展名可选");
            assert_eq!(parsed.ext, "");
        }
    }

    /// `Content-Type` 是 5 项白名单，其余一律 `video/mp2t`（目标实测矩阵）。
    #[test]
    fn segment_content_type_matches_target_allowlist() {
        assert_eq!(segment_content_type("jpg"), "image/jpeg");
        assert_eq!(segment_content_type("jpeg"), "image/jpeg");
        assert_eq!(segment_content_type("png"), "image/png");
        assert_eq!(segment_content_type("gif"), "image/gif");
        assert_eq!(segment_content_type("bmp"), "image/bmp");
        assert_eq!(segment_content_type("webp"), "image/webp");
        for ext in [
            "", "ts", "m4s", "mp4", "txt", "m3u8", "svg", "ico", "avif", "tiff",
        ] {
            assert_eq!(
                segment_content_type(ext),
                "video/mp2t",
                "{ext} 应回落到 video/mp2t"
            );
        }
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
            "http://self",
        );
        let first = rendered.segments[0].2.clone();
        let second = rendered.segments[1].2.clone();
        assert_ne!(first, second, "同一频道内扩展名轮换");
    }

    #[test]
    fn known_upstream_extension_is_preserved() {
        let body = "#EXTM3U\n#EXTINF:4.0,\nseg1.jpg?token=x\n#EXTINF:4.0,\nseg2.jpg?token=y\n";
        let rendered = rewrite_media(
            body,
            "http://up.example/a/index.m3u8",
            "g1",
            "c1",
            &formats(),
            true,
            "http://self",
        );
        assert_eq!(rendered.segments[0].2, "jpg");
        assert_eq!(rendered.segments[1].2, "jpg");
        assert_eq!(
            rendered.segments[0].1,
            "http://up.example/a/seg1.jpg?token=x"
        );
    }
}
