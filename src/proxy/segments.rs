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
    /// 登记时间（Unix 秒）。用于磁盘索引按新旧截断；旧文件缺省为 0。
    #[serde(default)]
    pub at: u64,
}

/// 磁盘索引的默认条目上限。
///
/// `[cache.segments].per_channel_max` 为 `0`（不限）时用它兜底：客户端只会请求清单
/// 窗口内的切片（约 7–20 片），4096 条足够覆盖窗口与重试重叠；不设上限时单个频道的
/// JSON 每天可长到几十 MB，1 GB 盘上无法接受。
pub const DEFAULT_INDEX_MAX: usize = 4096;

/// 内存索引容量限制；`0` 表示不限制。
#[derive(Default, Clone, Copy)]
struct Limits {
    global: usize,
    per_channel: usize,
}

/// 内存索引。
#[derive(Default)]
pub struct SegmentIndex {
    inner: RwLock<HashMap<String, Entry>>,
    limits: RwLock<Limits>,
}

impl SegmentIndex {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(gid: &str, cid: &str, suffix: &str) -> String {
        format!("{gid}/{cid}/{suffix}")
    }

    /// 更新 `[cache.segments]` 的索引上限。
    pub fn set_limits(&self, global: u64, per_channel: u64) {
        *self.limits.write().unwrap_or_else(|e| e.into_inner()) = Limits {
            global: global as usize,
            per_channel: per_channel as usize,
        };
    }

    /// 登记一批切片。
    pub fn register(&self, gid: &str, cid: &str, entries: &[(String, String, String)]) {
        if entries.is_empty() {
            return;
        }
        let limits = *self.limits.read().unwrap_or_else(|e| e.into_inner());
        let mut map = self.write();
        for (suffix, url, ext) in entries {
            map.insert(
                Self::key(gid, cid, suffix),
                Entry {
                    url: url.clone(),
                    ext: ext.clone(),
                    at: now_epoch(),
                },
            );
        }
        if limits.per_channel > 0 {
            let prefix = format!("{gid}/{cid}/");
            let keys: Vec<String> = map
                .keys()
                .filter(|key| key.starts_with(&prefix))
                .cloned()
                .collect();
            if keys.len() > limits.per_channel {
                let remove = keys.len() - limits.per_channel;
                for key in keys.into_iter().take(remove) {
                    map.remove(&key);
                }
            }
        }
        if limits.global > 0 && map.len() > limits.global {
            let overflow = map.len() - limits.global;
            let keys: Vec<String> = map.keys().take(overflow).cloned().collect();
            for key in keys {
                map.remove(&key);
            }
        }
    }

    /// 清空内存索引（清空缓存时调用）。
    pub fn clear(&self) {
        self.write().clear();
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

/// 写索引文件。与已有内容合并；超过 `max_entries` 时按登记时间丢弃最旧的。
///
/// `max_entries = 0` 表示不限。写失败只记日志（缓存可丢）。
pub fn save_index(
    dir: &Path,
    gid: &str,
    cid: &str,
    entries: &[(String, String, String)],
    max_entries: usize,
) {
    if entries.is_empty() {
        return;
    }
    let now = now_epoch();
    let mut merged = load_index(dir, gid, cid);
    for (suffix, url, ext) in entries {
        merged.insert(
            suffix.clone(),
            Entry {
                url: url.clone(),
                ext: ext.clone(),
                at: now,
            },
        );
    }
    if max_entries > 0 && merged.len() > max_entries {
        // 最旧优先丢弃；`at` 相同（旧格式）时按 suffix 兜底，保证结果确定。
        let mut ordered: Vec<(&String, u64)> =
            merged.iter().map(|(key, entry)| (key, entry.at)).collect();
        ordered.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        let drop_count = merged.len() - max_entries;
        let doomed: Vec<String> = ordered
            .into_iter()
            .take(drop_count)
            .map(|(key, _)| key.clone())
            .collect();
        for key in doomed {
            merged.remove(&key);
        }
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

/// 当前 Unix 秒。
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

/// 切片正文缓存路径。
pub fn segment_path(dir: &Path, gid: &str, cid: &str, suffix: &str, ext: &str) -> PathBuf {
    dir.join("seg_bytes").join(format!(
        "{}_{}_{}.{}",
        slug(gid),
        slug(cid),
        suffix,
        slug(ext)
    ))
}

/// 把标识压成文件名安全的形式。
fn slug(value: &str) -> String {
    let mapped: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
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
            0,
        );
        let index = SegmentIndex::new();
        let entry = index.get(&dir, "g/1", "c 1", &suffix).expect("命中索引");
        assert_eq!(entry.url, "http://up/x.ts");
        assert_eq!(entry.ext, "jpg");
        assert!(entry.at > 0, "登记时间应被写入");
        assert!(index.get(&dir, "g/1", "c 1", "nope").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_index_truncates_at_max_entries() {
        let dir = std::env::temp_dir().join(format!("iptv-seg-trunc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // 分两批写入，第二批更新：截断后应保留较新的 20 条。
        let first: Vec<(String, String, String)> = (0..80)
            .map(|n| {
                (
                    format!("a{n:03}"),
                    format!("http://up/a{n}.ts"),
                    "jpg".into(),
                )
            })
            .collect();
        save_index(&dir, "g", "c", &first, 0);
        let second: Vec<(String, String, String)> = (0..20)
            .map(|n| {
                (
                    format!("b{n:03}"),
                    format!("http://up/b{n}.ts"),
                    "jpg".into(),
                )
            })
            .collect();
        save_index(&dir, "g", "c", &second, 20);

        let stored = load_index(&dir, "g", "c");
        assert_eq!(stored.len(), 20, "落盘条数受 max_entries 限制");
        assert!(
            stored.contains_key("b019"),
            "最新的条目必须保留（丢最旧，不能丢刚登记的）"
        );
        assert!(!stored.contains_key("a000"));
        // 不带上限时保持原行为（合并全量）。
        save_index(&dir, "g", "c", &first, 0);
        assert_eq!(load_index(&dir, "g", "c").len(), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn limits_trim_registered_entries() {
        let dir = std::env::temp_dir().join(format!("iptv-seg-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let index = SegmentIndex::new();
        index.set_limits(0, 2);
        let entries: Vec<(String, String, String)> = (0..4)
            .map(|n| (n.to_string(), format!("http://up/{n}.ts"), "jpg".into()))
            .collect();
        index.register("g", "c", &entries);
        let live = (0..4)
            .filter(|n| index.get(&dir, "g", "c", &n.to_string()).is_some())
            .count();
        assert_eq!(live, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn segment_path_is_file_name_safe() {
        let path = segment_path(
            Path::new("/tmp/cache"),
            "g/1",
            "c 1",
            "ab".repeat(16).as_str(),
            "jpg",
        );
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.contains('/'), "{name}");
    }
}
