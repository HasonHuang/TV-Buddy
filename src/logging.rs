//! 日志：环形缓冲 + 文件输出（5 MB 截断）。
//!
//! 契约见 [`docs/design/04-config-and-storage.md`](../docs/design/04-config-and-storage.md) §5。

use std::{
    collections::VecDeque,
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
};

/// 保留的最大行数。
const RING_CAPACITY: usize = 500;
/// 日志文件上限 5 MB。
const FILE_LIMIT: u64 = 5 * 1024 * 1024;

/// 最近日志行。
#[derive(Debug, Default)]
pub struct RingBuffer {
    lines: Mutex<VecDeque<String>>,
}

impl RingBuffer {
    pub fn push(&self, line: &str) {
        let mut lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        if lines.len() >= RING_CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line.to_string());
    }

    /// 取最近 `limit` 行。
    pub fn tail(&self, limit: usize) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let skip = lines.len().saturating_sub(limit);
        lines.iter().skip(skip).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// 写日志：同时进环形缓冲与文件。
pub struct LogSink {
    ring: Arc<RingBuffer>,
    file: Mutex<Option<std::fs::File>>,
}

impl LogSink {
    /// 打开日志文件。失败时只保留环形缓冲。
    pub fn new(ring: Arc<RingBuffer>, path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            ring,
            file: Mutex::new(Some(file)),
        })
    }

    /// 不需要文件输出时使用。
    pub fn memory_only(ring: Arc<RingBuffer>) -> Self {
        Self {
            ring,
            file: Mutex::new(None),
        }
    }
}

impl Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 出口一：标准输出（`design/04` §5）。
        let _ = io::stdout().write_all(buf);
        let text = String::from_utf8_lossy(buf);
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            self.ring.push(line);
        }
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = guard.as_mut() {
            file.write_all(buf)?;
            if file
                .metadata()
                .map(|meta| meta.len() >= FILE_LIMIT)
                .unwrap_or(false)
            {
                // 目标行为：超过 5 MB 截断（S-CACHE-05）。
                let _ = file.set_len(0);
                let _ = file.write_all(b"[log truncated]\n");
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = guard.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

/// `MakeWriter` 工厂。
#[derive(Clone)]
pub struct LogWriter {
    ring: Arc<RingBuffer>,
    path: Option<std::path::PathBuf>,
}

impl LogWriter {
    pub fn new(ring: Arc<RingBuffer>, path: Option<std::path::PathBuf>) -> Self {
        Self { ring, path }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = LogSink;

    fn make_writer(&'a self) -> Self::Writer {
        match &self.path {
            Some(path) => LogSink::new(self.ring.clone(), path)
                .unwrap_or_else(|_| LogSink::memory_only(self.ring.clone())),
            None => LogSink::memory_only(self.ring.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_recent_lines() {
        let ring = RingBuffer::default();
        for index in 0..RING_CAPACITY + 10 {
            ring.push(&format!("line {index}"));
        }
        let tail = ring.tail(3);
        assert_eq!(tail.len(), 3);
        assert!(tail[2].ends_with(&format!("{}", RING_CAPACITY + 9)));
    }

    #[test]
    fn sink_writes_to_file() {
        let dir = std::env::temp_dir().join(format!("tv-buddy-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tv-buddy.log");
        let ring = Arc::new(RingBuffer::default());
        let mut sink = LogSink::new(ring.clone(), &path).unwrap();
        sink.write_all(b"hello\n").unwrap();
        sink.flush().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("hello"));
        assert_eq!(ring.tail(1), vec!["hello".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 日志文件名（`design/04` §5）。
pub const LOG_FILE: &str = "tv-buddy.log";

/// 日志文件与配置同目录。
pub fn log_path_for(config_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = config_path.parent()?;
    if dir.as_os_str().is_empty() {
        Some(std::path::PathBuf::from(LOG_FILE))
    } else {
        Some(dir.join(LOG_FILE))
    }
}

/// 按 `[server].log_level` 初始化日志：标准输出 + 文件 + 环形缓冲。
pub fn init(level: &str, log_path: Option<&std::path::Path>, ring: std::sync::Arc<RingBuffer>) {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("tv_buddy={level},tower_http={level}")));
    let writer = LogWriter::new(ring, log_path.map(|path| path.to_path_buf()));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(writer))
        .init();
}
