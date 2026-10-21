//! 字幕翻译：解析 SRT/WebVTT，按行替换或并列译文，保留时间轴。
//!
//! 只翻译字幕块正文；时间行、序号、`NOTE`/`STYLE` 块与 WebVTT 设置行原样保留。
//! 输出统一使用 LF 换行。

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};

/// 支持的字幕格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Srt,
    Vtt,
}

impl Format {
    /// 输出扩展名。
    pub fn ext(self) -> &'static str {
        match self {
            Format::Srt => "srt",
            Format::Vtt => "vtt",
        }
    }

    /// 输出 Content-Type。
    pub fn mime(self) -> &'static str {
        match self {
            Format::Srt => "application/x-subrip",
            Format::Vtt => "text/vtt",
        }
    }
}

/// 按正文判定格式：以 `WEBVTT` 开头为 VTT，否则需含时间行。
pub fn detect(body: &str) -> Result<Format> {
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("WEBVTT") {
        return Ok(Format::Vtt);
    }
    if body.lines().any(|line| line.contains("-->")) {
        return Ok(Format::Srt);
    }
    bail!("不是 SRT 或 WebVTT 字幕")
}

/// 收集需要翻译的正文行（去重、保持顺序）。
pub fn text_lines(body: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();
    let mut in_cue = false;
    for line in body.lines() {
        if line.contains("-->") {
            in_cue = true;
            continue;
        }
        if line.trim().is_empty() {
            in_cue = false;
            continue;
        }
        if !in_cue || is_settings(line) {
            continue;
        }
        let text = line.trim();
        if !has_text(text) {
            continue;
        }
        if seen.insert(text.to_string()) {
            lines.push(text.to_string());
        }
    }
    lines
}

/// 用译文重建字幕。
///
/// `translation_first = true` 时先写译文；否则先写原文。找不到译文或译文与原文
/// 相同时保留原文一行。
pub fn render(
    body: &str,
    translations: &HashMap<String, String>,
    translation_first: bool,
) -> String {
    let mut out = String::with_capacity(body.len() * 2);
    let mut in_cue = false;
    for line in body.lines() {
        if line.contains("-->") {
            in_cue = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            in_cue = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !in_cue || is_settings(line) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let text = line.trim();
        match translations.get(text) {
            Some(value) if !value.trim().is_empty() && value.trim() != text => {
                if translation_first {
                    out.push_str(value.trim());
                    out.push('\n');
                    out.push_str(line);
                    out.push('\n');
                } else {
                    out.push_str(line);
                    out.push('\n');
                    out.push_str(value.trim());
                    out.push('\n');
                }
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !body.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// WebVTT 设置行：所有词均为 `键:值`。
fn is_settings(line: &str) -> bool {
    let mut any = false;
    for token in line.split_whitespace() {
        match token.split_once(':') {
            Some((key, value))
                if !key.is_empty()
                    && !value.is_empty()
                    && key.chars().all(|c| c.is_ascii_alphabetic() || c == '-') =>
            {
                any = true;
            }
            _ => return false,
        }
    }
    any
}

/// 行内是否有可译文字（过滤纯标签或符号行）。
fn has_text(line: &str) -> bool {
    line.chars().any(|c| c.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:03,000\nHello\nWorld\n\n2\n00:00:04,000 --> 00:00:05,000\nHello\n";

    const VTT: &str = "WEBVTT\n\nNOTE 备注\n\n00:00.000 --> 00:02.000 align:start position:0%\n<i>Hello</i>\n\n00:03.000 --> 00:04.000\n♪\n";

    #[test]
    fn detect_recognizes_both_formats() {
        assert_eq!(detect(SRT).expect("SRT"), Format::Srt);
        assert_eq!(detect(VTT).expect("VTT"), Format::Vtt);
        assert!(detect("not a subtitle").is_err());
    }

    #[test]
    fn text_lines_skips_timing_notes_and_symbols() {
        assert_eq!(text_lines(SRT), vec!["Hello", "World"]);
        assert_eq!(text_lines(VTT), vec!["<i>Hello</i>"]);
    }

    #[test]
    fn render_keeps_timing_and_orders_bilingual_lines() {
        let mut map = HashMap::new();
        map.insert("Hello".to_string(), "你好".to_string());
        let original_first = render(SRT, &map, false);
        assert!(
            original_first.starts_with("1\n00:00:01,000 --> 00:00:03,000\nHello\n你好\nWorld\n")
        );
        assert!(original_first.contains("2\n00:00:04,000 --> 00:00:05,000\nHello\n你好\n"));

        let translated_first = render(SRT, &map, true);
        assert!(
            translated_first.starts_with("1\n00:00:01,000 --> 00:00:03,000\n你好\nHello\nWorld\n")
        );
    }

    #[test]
    fn render_preserves_missing_final_newline() {
        let map = HashMap::new();
        assert_eq!(
            render("1\n00:00:01,000 --> 00:00:02,000\nHi", &map, false),
            "1\n00:00:01,000 --> 00:00:02,000\nHi"
        );
    }
}
