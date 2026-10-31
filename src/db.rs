//! 数据库：单文件 SQLite 存储。
//!
//! 一个库文件装配置、用户与令牌。运行时只用这一个文件。
//! 建表是幂等的，启动时执行一次。
//! 连接设 WAL 与 `synchronous = NORMAL`，进程内共用一条。

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// 进程内共用的数据库连接。
///
/// 写入串行化。本项目写入量小，一条连接足够，且事务边界清楚。
pub type SharedConn = Arc<Mutex<Connection>>;

/// 配置文档在 `settings` 表里的键。
pub const CONFIG_KEY: &str = "config";

/// 打开或创建数据库，并建表。
pub fn connect(path: &Path) -> Result<SharedConn> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        }
    }
    let conn =
        Connection::open(path).with_context(|| format!("打开数据库失败: {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    conn.pragma_update(None, "synchronous", "NORMAL").ok();
    // 管理 CLI 与服务可能同时写。等待而不是立刻报错。
    conn.busy_timeout(Duration::from_millis(5000)).ok();
    // WAL 长期运行会膨胀。限制单个 WAL 文件。
    conn.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)
        .ok();
    init_schema(&conn)?;
    restrict_permissions(path);
    Ok(Arc::new(Mutex::new(conn)))
}

/// 库文件只给属主读写。里面存口令散列与访问令牌。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// 建表。三张表都用同一套 JSON 文档结构。
fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
             key TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL,
             updated_at_ms BIGINT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS users (
             id TEXT PRIMARY KEY NOT NULL,
             sort_order BIGINT NOT NULL,
             config_json TEXT NOT NULL,
             updated_at_ms BIGINT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS users_order_idx ON users (sort_order, id);
         CREATE TABLE IF NOT EXISTS access_tokens (
             id TEXT PRIMARY KEY NOT NULL,
             sort_order BIGINT NOT NULL,
             config_json TEXT NOT NULL,
             updated_at_ms BIGINT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS access_tokens_order_idx
             ON access_tokens (sort_order, id);",
    )
    .context("建表失败")?;
    Ok(())
}
