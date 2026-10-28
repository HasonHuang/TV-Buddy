//! `[cache].backend = "memory"` 时的切片正文字典。
//!
//! 磁盘后端的行为完全不变。内存后端把切片字节留在进程内，不写 `seg_bytes`。
//! 总量受 `[cache].memory_max_mb` 限制（`0` 按 512 MB 处理），超出后按最近最少使用淘汰。
//! 进程重启即清空：这一点与磁盘后端不同，文档里写明。

use std::{collections::HashMap, sync::Mutex};

/// `memory_max_mb = 0` 时的默认上限。
const DEFAULT_LIMIT_MB: u64 = 512;

#[derive(Debug)]
struct Entry {
    bytes: Vec<u8>,
    /// 最近一次访问的序号，用于淘汰。
    tick: u64,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    used: u64,
    tick: u64,
}

/// 切片正文的内存缓存。
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 读取并刷新使用序号。
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        let entry = inner.entries.get_mut(key)?;
        entry.tick = tick;
        Some(entry.bytes.clone())
    }

    /// 写入。超出上限时按最近最少使用淘汰，直到放得下。
    ///
    /// 单条比上限还大时不写，避免把整表清空。
    pub fn put(&self, key: &str, bytes: Vec<u8>, limit: u64) {
        let size = bytes.len() as u64;
        let limit = limit.max(1);
        if size > limit {
            return;
        }
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        if let Some(old) = inner.entries.remove(key) {
            inner.used = inner.used.saturating_sub(old.bytes.len() as u64);
        }
        while inner.used + size > limit && !inner.entries.is_empty() {
            let victim = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.tick)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else {
                break;
            };
            if let Some(removed) = inner.entries.remove(&victim) {
                inner.used = inner.used.saturating_sub(removed.bytes.len() as u64);
            }
        }
        inner.used += size;
        inner.entries.insert(key.to_string(), Entry { bytes, tick });
    }

    /// 清空。
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.entries.clear();
        inner.used = 0;
    }

    /// 条目数。
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 已用字节数。
    pub fn bytes(&self) -> u64 {
        self.lock().used
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// 由 `[cache].memory_max_mb` 换算上限字节数。
pub fn limit_bytes(memory_max_mb: u64) -> u64 {
    let mb = if memory_max_mb == 0 {
        DEFAULT_LIMIT_MB
    } else {
        memory_max_mb
    };
    mb.saturating_mul(1024 * 1024)
}

/// 后端是否为内存。
pub fn is_memory(backend: &str) -> bool {
    backend.eq_ignore_ascii_case("memory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_reads() {
        let store = MemoryStore::new();
        store.put("g/c/1", b"hello".to_vec(), 1024);
        assert_eq!(store.get("g/c/1").as_deref(), Some(&b"hello"[..]));
        assert_eq!(store.len(), 1);
        assert_eq!(store.bytes(), 5);
        assert!(store.get("g/c/2").is_none());
    }

    #[test]
    fn evicts_least_recently_used() {
        let store = MemoryStore::new();
        store.put("a", vec![1; 4], 10);
        store.put("b", vec![2; 4], 10);
        // 触一下 a，让 b 成为最久未用。
        assert!(store.get("a").is_some());
        store.put("c", vec![3; 4], 10);
        assert!(store.get("b").is_none(), "b 应被淘汰");
        assert!(store.get("a").is_some());
        assert!(store.get("c").is_some());
    }

    #[test]
    fn oversized_item_is_skipped() {
        let store = MemoryStore::new();
        store.put("big", vec![0; 16], 10);
        assert!(store.is_empty());
    }

    #[test]
    fn clear_drops_everything() {
        let store = MemoryStore::new();
        store.put("a", vec![1; 4], 10);
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.bytes(), 0);
    }

    #[test]
    fn limit_falls_back_to_default() {
        assert_eq!(limit_bytes(0), DEFAULT_LIMIT_MB * 1024 * 1024);
        assert_eq!(limit_bytes(8), 8 * 1024 * 1024);
    }

    #[test]
    fn backend_name_is_case_insensitive() {
        assert!(is_memory("memory"));
        assert!(is_memory("MEMORY"));
        assert!(!is_memory("disk"));
    }
}
