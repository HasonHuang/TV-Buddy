//! 切片索引与字节缓存（M2）。
//!
//! 索引把 `<S>` 映射回上游绝对 URL。索引放内存，并落盘到
//! `<disk_path>/seg_url/<hash>.json`，重启后仍可命中。
//! 切片正文落盘到 `<disk_path>/seg_bytes/`。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::RwLock,
    time::{Duration, SystemTime},
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
    /// 登记时间（Unix 秒）。用于磁盘索引按新旧截断；缺失时为 0。
    #[serde(default)]
    pub at: u64,
}

/// 磁盘索引的默认条目上限。
///
/// `[cache.segments].per_channel_max` 为 `0`（不限）时用它兜底：客户端只会请求清单
/// 窗口内的切片（约 7–20 片），4096 条足够覆盖窗口与重试重叠；不设上限时单个频道的
/// JSON 每天可长到几十 MB，1 GB 盘上无法接受。
pub const DEFAULT_INDEX_MAX: usize = 4096;

/// 内存索引条目的存活时间（秒）。
///
/// GC 每轮调用 [`SegmentIndex::active_channels`]；超过该时长未被重新登记的条目会被
/// 清掉，空闲进程的内存索引算术上收敛到 0。
pub const INDEX_TTL_SECS: u64 = 600;

/// 活动表记录的频道数上限。
///
/// 未知路径与扫描请求不能无限撑大活动表。达到上限后只刷新已有键，不插入新键。
pub const MAX_ACTIVITY_CHANNELS: usize = 1024;

/// 磁盘索引命中的 mtime 刷新节流（秒）。
///
/// 磁盘命中会刷新索引文件的 mtime，避免回收按年龄删除仍在使用的索引。
/// 只有 mtime 超过该时长才写一次。
pub const INDEX_TOUCH_SECS: u64 = 60;

/// 内存索引容量限制；`0` 表示不限制。
#[derive(Default, Clone, Copy)]
struct Limits {
    per_channel: usize,
}

/// 内存索引。
#[derive(Default)]
pub struct SegmentIndex {
    inner: RwLock<HashMap<String, Entry>>,
    /// `"{gid}/{cid}" -> 最近一次登记切片的 Unix 秒。
    activity: RwLock<HashMap<String, u64>>,
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
    pub fn set_limits(&self, per_channel: u64) {
        *self.limits.write().unwrap_or_else(|e| e.into_inner()) = Limits {
            per_channel: per_channel as usize,
        };
    }

    /// 记下频道最近的活动时间，供 [`Self::active_channels`] 统计活跃频道数。
    ///
    /// 活动表最多记 [`MAX_ACTIVITY_CHANNELS`] 个频道。达到上限后只刷新已有键；
    /// 新键丢弃，避免未知地址撑大内存。
    pub fn note_channel(&self, gid: &str, cid: &str) {
        let key = format!("{gid}/{cid}");
        let now = now_epoch();
        let mut activity = self.activity.write().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = activity.get_mut(&key) {
            *at = now;
            return;
        }
        if activity.len() < MAX_ACTIVITY_CHANNELS {
            activity.insert(key, now);
        }
    }

    /// 清掉窗口外的活动记录与超过 [`INDEX_TTL_SECS`] 的索引条目，返回剩余活跃频道数。
    pub fn active_channels(&self, window: Duration) -> usize {
        let now = now_epoch();
        let window_secs = window.as_secs();
        let active = {
            let mut activity = self.activity.write().unwrap_or_else(|e| e.into_inner());
            activity.retain(|_, at| now.saturating_sub(*at) <= window_secs);
            let len = activity.len();
            // 扫描或短期换台可能把表撑大；窗口清理后容量远超长度时归还内存。
            if activity.capacity() > len.saturating_mul(2).saturating_add(64) {
                activity.shrink_to_fit();
            }
            len
        };
        self.write()
            .retain(|_, entry| now.saturating_sub(entry.at) <= INDEX_TTL_SECS);
        active
    }

    /// 登记一批切片。
    pub fn register(&self, gid: &str, cid: &str, entries: &[(String, String, String)]) {
        if entries.is_empty() {
            return;
        }
        let limits = *self.limits.read().unwrap_or_else(|e| e.into_inner());
        let now = now_epoch();
        let mut map = self.write();
        map.retain(|_, entry| now.saturating_sub(entry.at) <= INDEX_TTL_SECS);
        for (suffix, url, ext) in entries {
            map.insert(
                Self::key(gid, cid, suffix),
                Entry {
                    url: url.clone(),
                    ext: ext.clone(),
                    at: now,
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
    }

    /// 清空内存索引与活动记录（清空缓存时调用）。
    pub fn clear(&self) {
        self.write().clear();
        self.activity
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// 查索引：先内存，再磁盘。
    ///
    /// 磁盘命中后回填内存，并刷新索引文件 mtime（有节流）：VOD 或不再刷新的客户端
    /// 即使内存条目过了 [`INDEX_TTL_SECS`]，磁盘索引也不会被按年龄或索引子预算删掉。
    pub fn get(&self, dir: &Path, gid: &str, cid: &str, suffix: &str) -> Option<Entry> {
        if let Some(entry) = self.read().get(&Self::key(gid, cid, suffix)) {
            return Some(entry.clone());
        }
        let path = index_path(dir, gid, cid);
        let from_disk = load_index(dir, gid, cid).remove(suffix)?;
        touch_index(&path);
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
///
/// 文件名取自 [`super::media::prefix`] 的同一份频道摘要（`<P>`）：`(gid, cid)` 不同则
/// `<P>` 不同，两个频道不会共用一份索引文件。不用 `"{gid}/{cid}"` 拼键——那样
/// `("a/b", "c")` 与 `("a", "b/c")` 会落到同一个文件名。
fn index_path(dir: &Path, gid: &str, cid: &str) -> PathBuf {
    dir.join("seg_url")
        .join(format!("{}.json", super::media::prefix(gid, cid)))
}

/// 读索引文件。文件不存在或损坏时返回空表。
pub fn load_index(dir: &Path, gid: &str, cid: &str) -> HashMap<String, Entry> {
    std::fs::read_to_string(index_path(dir, gid, cid))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 磁盘命中后刷新索引文件 mtime。只有超过 [`INDEX_TOUCH_SECS`] 才写；错误忽略。
fn touch_index(path: &Path) {
    let Some(age) = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
    else {
        return;
    };
    if age.as_secs() < INDEX_TOUCH_SECS {
        return;
    }
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// 统计索引目录下直接子文件的数量与总字节；读目录失败时返回 `(0, 0)`。
pub fn index_usage(dir: &Path) -> (u64, u64) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in read_dir.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            files += 1;
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    (files, bytes)
}

/// 按 mtime 从旧到新删除索引文件，直到总量不超过 `budget_bytes` 或删满 `limit` 个。
///
/// 返回 `(删除数, 删除字节)`。读目录失败时返回 `(0, 0)`。
pub fn prune_index(dir: &Path, budget_bytes: u64, limit: usize) -> (u64, u64) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in read_dir.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((mtime, metadata.len(), entry.path()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
    let mut total = files
        .iter()
        .fold(0u64, |sum, (_, len, _)| sum.saturating_add(*len));
    let mut deleted = 0u64;
    let mut freed = 0u64;
    for (_, len, path) in files {
        if total <= budget_bytes || deleted as usize >= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            freed = freed.saturating_add(len);
            deleted += 1;
        }
    }
    (deleted, freed)
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
        // 最旧优先丢弃；`at` 相同时按 suffix 兜底，保证结果确定。
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
///
/// **不含扩展名**：目标实测（2026-10-25）同一 `(P, S)` 依次请求 `.jpg`/`.png`/`.ts`/`.bmp`
/// 时上游只被拉 **1 次**，`seg_bytes/` 里也只有一份文件（`<P>_<S 前 3 位>__<尾部哈希>`，
/// 同样没有扩展名）。扩展名只影响响应头里的 `Content-Type`（S-DIS-05、S-DIS-13），
/// 不该生成多份副本——否则伪装后缀一换就要重新回源。
pub fn segment_path(dir: &Path, gid: &str, cid: &str, suffix: &str) -> PathBuf {
    dir.join("seg_bytes")
        .join(format!("{}_{}_{}", slug(gid), slug(cid), suffix))
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
        let dir = std::env::temp_dir().join(format!("tv-buddy-seg-{}", std::process::id()));
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
    fn disk_hit_refreshes_index_mtime() {
        let dir = std::env::temp_dir().join(format!("tv-buddy-seg-touch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let suffix = "c".repeat(32);
        save_index(
            &dir,
            "g",
            "c",
            &[(
                suffix.clone(),
                "http://up/x.ts".to_string(),
                "jpg".to_string(),
            )],
            0,
        );
        let path = index_path(&dir, "g", "c");
        let old = SystemTime::now() - Duration::from_secs(INDEX_TOUCH_SECS * 3);
        std::fs::File::open(&path)
            .expect("打开索引")
            .set_modified(old)
            .expect("设置旧 mtime");

        let index = SegmentIndex::new();
        assert!(index.get(&dir, "g", "c", &suffix).is_some(), "磁盘回退命中");
        let age = SystemTime::now()
            .duration_since(
                std::fs::metadata(&path)
                    .expect("读元数据")
                    .modified()
                    .expect("读 mtime"),
            )
            .expect("mtime 不应在未来");
        assert!(age.as_secs() < INDEX_TOUCH_SECS, "命中应刷新索引 mtime");
        assert!(
            index
                .read()
                .contains_key(&SegmentIndex::key("g", "c", &suffix)),
            "磁盘命中应回填内存索引"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_index_truncates_at_max_entries() {
        let dir = std::env::temp_dir().join(format!("tv-buddy-seg-trunc-{}", std::process::id()));
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
        // 不带上限时合并全量。
        save_index(&dir, "g", "c", &first, 0);
        assert_eq!(load_index(&dir, "g", "c").len(), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn limits_trim_registered_entries() {
        let dir = std::env::temp_dir().join(format!("tv-buddy-seg-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let index = SegmentIndex::new();
        index.set_limits(2);
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
    fn active_channels_tracks_window() {
        let index = SegmentIndex::new();
        index.note_channel("g", "c1");
        index.note_channel("g", "c2");
        assert_eq!(index.active_channels(Duration::from_secs(90)), 2);
        let now = now_epoch();
        index
            .activity
            .write()
            .unwrap()
            .insert("g/c1".into(), now.saturating_sub(100));
        assert_eq!(index.active_channels(Duration::from_secs(90)), 1);
        // 上一轮把 c1 淘汰了；重新放进活动表再验证更宽的窗口。
        index
            .activity
            .write()
            .unwrap()
            .insert("g/c1".into(), now.saturating_sub(100));
        assert_eq!(index.active_channels(Duration::from_secs(120)), 2);
    }

    #[test]
    fn active_channels_caps_map_size() {
        let index = SegmentIndex::new();
        for n in 0..MAX_ACTIVITY_CHANNELS + 10 {
            index.note_channel("g", &format!("c{n}"));
        }
        assert_eq!(
            index.activity.read().unwrap().len(),
            MAX_ACTIVITY_CHANNELS,
            "活动表不能超过上限"
        );
        // 已有键仍可刷新，不占新位置。
        index
            .activity
            .write()
            .unwrap()
            .insert("g/c0".into(), now_epoch().saturating_sub(100));
        index.note_channel("g", "c0");
        assert!(index.activity.read().unwrap()["g/c0"] > now_epoch().saturating_sub(100));
        assert_eq!(index.activity.read().unwrap().len(), MAX_ACTIVITY_CHANNELS);
        // 达到上限后新键被丢弃。
        index.note_channel("g", "overflow");
        assert!(!index.activity.read().unwrap().contains_key("g/overflow"));
    }

    #[test]
    fn clear_resets_active_channels() {
        let index = SegmentIndex::new();
        index.note_channel("g", "c1");
        assert_eq!(index.active_channels(Duration::from_secs(90)), 1);
        index.clear();
        assert_eq!(index.active_channels(Duration::from_secs(90)), 0);
    }

    #[test]
    fn index_ttl_evicts_stale_entries() {
        let index = SegmentIndex::new();
        index.set_limits(0);
        index.write().insert(
            "g/c/old".into(),
            Entry {
                url: "http://up/old.ts".into(),
                ext: "jpg".into(),
                at: now_epoch().saturating_sub(601),
            },
        );
        // GC 每轮调用 active_channels；这里顺带清掉过期索引。
        assert_eq!(index.active_channels(Duration::from_secs(90)), 0);
        assert!(index.read().get("g/c/old").is_none(), "超过 600 秒应淘汰");
    }

    #[test]
    fn index_usage_sums_files() {
        let dir = std::env::temp_dir().join(format!("tv-buddy-index-usage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(index_usage(&dir), (0, 0), "目录不存在时返回零");
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(dir.join("a.json"), vec![b'x'; 10]).expect("写文件");
        std::fs::write(dir.join("b.json"), vec![b'x'; 20]).expect("写文件");
        assert_eq!(index_usage(&dir), (2, 30));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_index_deletes_oldest_files() {
        let dir = std::env::temp_dir().join(format!("tv-buddy-index-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        for index in 0..5 {
            let path = dir.join(format!("{index}.json"));
            std::fs::write(&path, vec![b'x'; 1_000]).expect("写文件");
            let when = SystemTime::now() - Duration::from_secs(10 * (5 - index));
            std::fs::File::open(&path)
                .expect("打开")
                .set_modified(when)
                .expect("mtime");
        }
        let (files, bytes) = prune_index(&dir, 2_000, 100);
        assert_eq!(files, 3);
        assert_eq!(bytes, 3_000);
        assert_eq!(std::fs::read_dir(&dir).expect("读目录").count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn segment_path_is_file_name_safe() {
        let path = segment_path(
            Path::new("/tmp/cache"),
            "g/1",
            "c 1",
            "ab".repeat(16).as_str(),
        );
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.contains('/'), "{name}");
    }

    /// 缓存路径与扩展名无关：伪装后缀换了不该多写一份文件（目标同此行为）。
    #[test]
    fn segment_path_ignores_extension() {
        let first = segment_path(Path::new("/tmp/cache"), "g", "c", "ab".repeat(16).as_str());
        assert!(!first.to_string_lossy().contains(".jpg"));
        // 索引文件按 `<P>` 命名，同一 `(gid,cid)` 只有一个文件。
        let a = index_path(Path::new("/tmp/cache"), "a:b", "c");
        let b = index_path(Path::new("/tmp/cache"), "a", "b:c");
        assert_ne!(a, b, "不同 (gid,cid) 不能共用索引文件");
    }
}
