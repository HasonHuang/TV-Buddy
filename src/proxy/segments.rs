//! 切片索引与字节缓存（M2）。
//!
//! 索引把 `<S>` 映射回上游绝对 URL。索引放内存，并落盘到
//! `<disk_path>/seg_url/<hash>.json`，重启后仍可命中。
//! 切片正文落盘到 `<disk_path>/seg_bytes/`。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::RwLock,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 一条切片索引。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// 上游绝对 URL。
    pub url: String,
    /// 伪装扩展名。
    pub ext: String,
}

/// 内存索引。
#[derive(Default)]
pub struct SegmentIndex {
    inner: RwLock<HashMap<String, Entry>>,
}

impl SegmentIndex {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(gid: &str, cid: &str, suffix: &str) -> String {
        format!("{gid}/{cid}/{suffix}")
    }

    /// 登记一批切片。
    pub fn register(&self, gid: &str, cid: &str, entries: &[(String, String, String)]) {
        if entries.is_empty() {
            return;
        }
        let mut map = self.write();
        for (suffix, url, ext) in entries {
            map.insert(
                Self::key(gid, cid, suffix),
                Entry {
                    url: url.clone(),
                    ext: ext.clone(),
                },
            );
        }
    }

    /// 查索引：先内存，再磁盘。
    pub fn get(&self, dir: &Path, gid: &str, cid: &str, suffix: &str) -> Option<Entry> {
        if let Some(entry) = self.read().get(&Self::key(gid, cid, suffix)) {
            return Some(entry.clone());
        }
        let from_disk = load_index(dir, gid, cid).remove(suffix)?;
        self.register(
            gid,
            cid,
            &[(
                suffix.to_string(),
                from_disk.url.clone(),
                from_disk.ext.clone(),
            )],
        );
        Some(from_disk)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Entry>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Entry>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// 索引文件路径。
fn index_path(dir: &Path, gid: &str, cid: &str) -> PathBuf {
    let digest = Sha256::digest(format!("{gid}/{cid}").as_bytes());
    dir.join("seg_url")
        .join(format!("{}.json", &hex::encode(digest)[..16]))
}

/// 读索引文件。文件不存在或损坏时返回空表。
pub fn load_index(dir: &Path, gid: &str, cid: &str) -> HashMap<String, Entry> {
    std::fs::read_to_string(index_path(dir, gid, cid))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 写索引文件。与已有内容合并。写失败只记日志（缓存可丢）。
pub fn save_index(dir: &Path, gid: &str, cid: &str, entries: &[(String, String, String)]) {
    if entries.is_empty() {
        return;
    }
    let mut merged = load_index(dir, gid, cid);
    for (suffix, url, ext) in entries {
        merged.insert(
            suffix.clone(),
            Entry {
                url: url.clone(),
                ext: ext.clone(),
            },
        );
    }
    let path = index_path(dir, gid, cid);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    match serde_json::to_string(&merged) {
        Ok(text) => {
            if let Err(err) = std::fs::write(&path, text) {
                tracing::warn!(error = %err, "写切片索引失败");
            }
        }
        Err(err) => tracing::warn!(error = %err, "序列化切片索引失败"),
    }
}

/// 切片正文缓存路径。
pub fn segment_path(dir: &Path, gid: &str, cid: &str, suffix: &str, ext: &str) -> PathBuf {
    dir.join("seg_bytes")
        .join(format!("{}_{}_{}.{}", slug(gid), slug(cid), suffix, slug(ext)))
}

/// 把标识压成文件名安全的形式。
fn slug(value: &str) -> String {
    let mapped: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if mapped.is_empty() || mapped.len() > 48 {
        format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_string()
    } else {
        mapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("iptv-seg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let suffix = "a".repeat(32);
        save_index(
            &dir,
            "g/1",
            "c 1",
            &[(
                suffix.clone(),
                "http://up/x.ts".to_string(),
                "jpg".to_string(),
            )],
        );
        let index = SegmentIndex::new();
        let entry = index.get(&dir, "g/1", "c 1", &suffix).expect("命中索引");
        assert_eq!(entry.url, "http://up/x.ts");
        assert_eq!(entry.ext, "jpg");
        assert!(index.get(&dir, "g/1", "c 1", "nope").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn segment_path_is_file_name_safe() {
        let path = segment_path(Path::new("/tmp/cache"), "g/1", "c 1", "ab".repeat(16).as_str(), "jpg");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.contains('/'), "{name}");
    }
}
