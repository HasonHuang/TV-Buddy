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
    /// ASS/SSA：先转为 WebVTT 再翻译。
    Ass,
}

impl Format {
    /// 输出扩展名。
    pub fn ext(self) -> &'static str {
        match self {
            Format::Srt => "srt",
            Format::Vtt => "vtt",
            Format::Ass => "ass",
        }
    }

    /// 输出 Content-Type。
    pub fn mime(self) -> &'static str {
        match self {
            Format::Srt => "application/x-subrip",
            Format::Vtt => "text/vtt",
            Format::Ass => "text/x-ssa",
        }
    }
}

/// 按正文判定格式：`WEBVTT` 开头为 VTT；`[Script Info]`/`[Events]` 为 ASS/SSA；
/// 其余需含时间行，判为 SRT。
pub fn detect(body: &str) -> Result<Format> {
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("WEBVTT") {
        return Ok(Format::Vtt);
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("[script info]") || lower.contains("[events]") {
        return Ok(Format::Ass);
    }
    if body.lines().any(|line| line.contains("-->")) {
        return Ok(Format::Srt);
    }
    bail!("不是 SRT、WebVTT 或 ASS/SSA 字幕")
}

/// 把 ASS/SSA 转为 WebVTT。
///
/// 读取 `[Events]` 的 `Format:` 行，按字段名取 `Start`、`End`、`Text`。
/// 去掉 `{\\…}` 覆盖标签，`\\N` 与 `\\n` 转为换行，`\\h` 转为空格。
pub fn convert_ass_to_vtt(body: &str) -> Result<String> {
    let mut in_events = false;
    let mut fields: Vec<String> = Vec::new();
    let mut cues: Vec<(String, String, String)> = Vec::new();
    for line in body.lines() {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_events = trimmed.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Format:") {
            fields = rest
                .split(',')
                .map(|field| field.trim().to_ascii_lowercase())
                .collect();
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("Dialogue:") else {
            continue;
        };
        if fields.is_empty() {
            bail!("ASS 缺少 Format 行");
        }
        let parts: Vec<&str> = rest.splitn(fields.len(), ',').collect();
        if parts.len() < fields.len() {
            continue;
        }
        let field = |name: &str| {
            fields
                .iter()
                .position(|item| item == name)
                .and_then(|index| parts.get(index).map(|value| value.trim()))
        };
        let (Some(start), Some(end), Some(text)) = (field("start"), field("end"), field("text"))
        else {
            bail!("ASS Format 缺少 Start/End/Text");
        };
        let text = clean_ass_text(text);
        if text.is_empty() {
            continue;
        }
        cues.push((ass_time(start)?, ass_time(end)?, text));
    }
    if cues.is_empty() {
        bail!("ASS 没有可转换的对白");
    }
    let mut out = String::from("WEBVTT\n\n");
    for (start, end, text) in cues {
        out.push_str(&start);
        out.push_str(" --> ");
        out.push_str(&end);
        out.push('\n');
        out.push_str(&text);
        out.push_str("\n\n");
    }
    Ok(out)
}

/// `H:MM:SS.cc` → `HH:MM:SS.mmm`。
fn ass_time(value: &str) -> Result<String> {
    let mut parts = value.split(':');
    let (Some(hours), Some(minutes), Some(seconds)) = (parts.next(), parts.next(), parts.next())
    else {
        bail!("ASS 时间格式错误: {value}");
    };
    let hours: u64 = hours.trim().parse().unwrap_or(0);
    let minutes: u64 = minutes.trim().parse().unwrap_or(0);
    let (secs, centis) = seconds
        .trim()
        .split_once('.')
        .unwrap_or((seconds.trim(), "0"));
    let secs: u64 = secs.parse().unwrap_or(0);
    let centis: u64 = centis.parse().unwrap_or(0);
    Ok(format!(
        "{hours:02}:{minutes:02}:{secs:02}.{:03}",
        centis * 10
    ))
}

/// 清理 ASS 正文：去覆盖标签、转义换行与空格。
fn clean_ass_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                for next in chars.by_ref() {
                    if next == '}' {
                        break;
                    }
                }
            }
            '\\' => match chars.next() {
                Some('N') | Some('n') => out.push('\n'),
                Some('h') => out.push(' '),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            _ => out.push(ch),
        }
    }
    out.trim().to_string()
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

    const ASS: &str = "[Script Info]\nScriptType: v4.00+\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,{\\i1}Hello\\NWorld\nDialogue: 0,0:00:04.00,0:00:05.00,Default,,0,0,0,,Bye\n";

    #[test]
    fn detect_recognizes_all_formats() {
        assert_eq!(detect(SRT).expect("SRT"), Format::Srt);
        assert_eq!(detect(VTT).expect("VTT"), Format::Vtt);
        assert_eq!(detect(ASS).expect("ASS"), Format::Ass);
        assert!(detect("not a subtitle").is_err());
    }

    #[test]
    fn converts_ass_to_vtt() {
        let body = convert_ass_to_vtt(ASS).expect("转换");
        assert!(body.starts_with("WEBVTT\n\n"), "{body}");
        assert!(
            body.contains("00:00:01.000 --> 00:00:03.500\nHello\nWorld\n"),
            "{body}"
        );
        assert!(
            body.contains("00:00:04.000 --> 00:00:05.000\nBye\n"),
            "{body}"
        );
        assert!(!body.contains("{\\i1}"), "{body}");
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
