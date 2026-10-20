//! 素材库：视频与字幕文件。
//!
//! 目录：`<cache>/assets/`（视频）、`<cache>/subtitles/`（字幕）。
//! 只做面板内管理（上传、列表、删除、下载）；公开播放链路不引用这些文件。
//! 目标只登记了路由存在性（F-011），响应形状为本实现自定。

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{bail, Context, Result};
use serde::Serialize;

/// 视频扩展名白名单。
pub const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "ts", "mov", "webm", "m4v"];
/// 字幕扩展名白名单。
pub const SUBTITLE_EXTS: &[&str] = &["srt", "ass", "ssa", "vtt"];

/// 素材种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Subtitle,
}

impl Kind {
    /// 目录名与 URL 段。
    pub fn dir_name(self) -> &'static str {
        match self {
            Kind::Video => "assets",
            Kind::Subtitle => "subtitles",
        }
    }

    /// 扩展名白名单。
    pub fn exts(self) -> &'static [&'static str] {
        match self {
            Kind::Video => VIDEO_EXTS,
            Kind::Subtitle => SUBTITLE_EXTS,
        }
    }

    /// 从路径段解析；未知返回 `None`。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "assets" => Some(Kind::Video),
            "subtitles" => Some(Kind::Subtitle),
            _ => None,
        }
    }
}

/// 列表项。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub file: String,
    pub name: String,
    pub ext: String,
    pub bytes: u64,
    pub mtime: u64,
}

/// 素材目录。
pub fn dir(cache_dir: &str, kind: Kind) -> PathBuf {
    Path::new(cache_dir).join(kind.dir_name())
}

/// 扩展名归一化并校验白名单。
pub fn check_ext(kind: Kind, ext: &str) -> Option<String> {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if kind.exts().contains(&ext.as_str()) {
        Some(ext)
    } else {
        None
    }
}

/// 列出目录中的素材，按文件名排序。
pub fn list(cache_dir: &str, kind: Kind) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir(cache_dir, kind)) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .filter_map(|item| item.ok())
        .filter(|item| item.file_type().map(|kind| kind.is_file()).unwrap_or(false))
        .filter_map(|item| {
            let file = item.file_name().to_string_lossy().to_string();
            if file.starts_with('.') {
                return None;
            }
            let ext = file
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())?;
            if !kind.exts().contains(&ext.as_str()) {
                return None;
            }
            let meta = item.metadata().ok()?;
            Some(Entry {
                name: file
                    .rsplit_once('.')
                    .map(|(name, _)| name)
                    .unwrap_or(&file)
                    .to_string(),
                file: file.clone(),
                ext,
                bytes: meta.len(),
                mtime: meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_secs())
                    .unwrap_or(0),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.file.cmp(&b.file));
    entries
}

/// 写入素材；同名覆盖。
pub fn write(cache_dir: &str, kind: Kind, name: &str, ext: &str, data: &[u8]) -> Result<Entry> {
    let name = crate::logo::sanitize(name).context("文件名不合法")?;
    let ext = check_ext(kind, ext).context("扩展名不在白名单")?;
    if data.is_empty() {
        bail!("内容为空");
    }
    let directory = dir(cache_dir, kind);
    fs::create_dir_all(&directory).with_context(|| format!("建目录 {}", directory.display()))?;
    let file = format!("{name}.{ext}");
    let path = directory.join(&file);
    fs::write(&path, data).with_context(|| format!("写 {}", path.display()))?;
    Ok(Entry {
        file,
        name,
        ext,
        bytes: data.len() as u64,
        mtime: fs::metadata(&path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_secs())
            .unwrap_or(0),
    })
}

/// 删除素材；文件不存在时返回 `Ok(false)`。
pub fn remove(cache_dir: &str, kind: Kind, file: &str) -> Result<bool> {
    let file = crate::logo::sanitize(file).context("文件名不合法")?;
    let path = dir(cache_dir, kind).join(file);
    if !path.is_file() {
        return Ok(false);
    }
    fs::remove_file(&path).with_context(|| format!("删除 {}", path.display()))?;
    Ok(true)
}

/// 读取素材字节。
pub fn read(cache_dir: &str, kind: Kind, file: &str) -> Result<Vec<u8>> {
    let file = crate::logo::sanitize(file).context("文件名不合法")?;
    let path = dir(cache_dir, kind).join(&file);
    if !path.is_file() {
        bail!("文件不存在");
    }
    fs::read(&path).with_context(|| format!("读 {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_exts() {
        assert_eq!(Kind::parse("assets"), Some(Kind::Video));
        assert_eq!(Kind::parse("subtitles"), Some(Kind::Subtitle));
        assert_eq!(Kind::parse("logo"), None);
        assert_eq!(check_ext(Kind::Video, "MP4").as_deref(), Some("mp4"));
        assert_eq!(check_ext(Kind::Video, "srt"), None);
        assert_eq!(check_ext(Kind::Subtitle, ".vtt").as_deref(), Some("vtt"));
    }

    #[test]
    fn write_list_read_remove() {
        let dir = std::env::temp_dir().join(format!("assets-test-{}", uuid::Uuid::new_v4()));
        let cache = dir.to_string_lossy().to_string();
        let entry = write(&cache, Kind::Video, "花絮", "mp4", b"data").expect("写入");
        assert_eq!(entry.file, "花絮.mp4");
        assert_eq!(list(&cache, Kind::Video).len(), 1);
        assert!(list(&cache, Kind::Subtitle).is_empty());
        assert_eq!(
            read(&cache, Kind::Video, "花絮.mp4").expect("读取"),
            b"data"
        );
        assert!(remove(&cache, Kind::Video, "花絮.mp4").expect("删除"));
        assert!(!remove(&cache, Kind::Video, "花絮.mp4").expect("再删"));
        assert!(write(&cache, Kind::Video, "x", "exe", b"1").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
