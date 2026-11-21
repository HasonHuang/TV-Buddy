//! 配置持久化：SQLite 单行 JSON 文档。
//!
//! 整份配置存成一条记录（`settings.key = 'config'`），值是 `Config` 的 JSON。
//! 与用户表同一套文档模式，改动只需一次 UPSERT。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::model::Config;
use crate::{
    db::{SharedConn, CONFIG_KEY},
    users::model::now_ms,
};

/// 配置仓库。
pub struct ConfigStore {
    conn: SharedConn,
}

impl ConfigStore {
    pub fn new(conn: SharedConn) -> Self {
        Self { conn }
    }

    /// 读配置。库里没有记录时返回 `None`。
    pub fn load(&self) -> Result<Option<Config>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        load_conn(&conn)
    }

    /// 在调用方的事务里读配置。
    pub fn load_tx(&self, tx: &Transaction<'_>) -> Result<Option<Config>> {
        load_conn(tx)
    }

    /// 写配置。先补空密钥、校验，再整条替换。
    ///
    /// 每次写入自增 `revision`，并返回新值。调用方用它更新内存快照。
    pub fn save(&self, config: &Config) -> Result<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        save_conn(&conn, config)
    }

    /// 在调用方的事务里写配置。语义与 [`ConfigStore::save`] 相同。
    pub fn save_tx(&self, tx: &Transaction<'_>, config: &Config) -> Result<u64> {
        save_conn(tx, config)
    }

    /// 按 `revision` 条件保存。
    ///
    /// 在 `IMMEDIATE` 事务里读版本并写入，跨进程写者不会插在检查与写入之间。
    /// `expected` 为空或与库内值不同时返回 [`crate::config::StaleRevision`]。
    pub fn save_if_revision(&self, config: &Config, expected: Option<u64>) -> Result<u64> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_conn(&tx)?.map(|from_db| from_db.revision).unwrap_or(0);
        let Some(expected) = expected else {
            return Err(anyhow::Error::new(crate::config::StaleRevision(current)));
        };
        if expected != current {
            return Err(anyhow::Error::new(crate::config::StaleRevision(current)));
        }
        let revision = save_conn(&tx, config)?;
        tx.commit()?;
        Ok(revision)
    }

    /// 恢复默认配置。用户不受影响。
    pub fn reset(&self) -> Result<Config> {
        let mut config = Config::default();
        config.revision = self.save(&config)?;
        Ok(config)
    }
}

fn load_conn(conn: &Connection) -> Result<Option<Config>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![CONFIG_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let mut config: Config = serde_json::from_str(&raw).context("配置记录解析失败")?;
    config.normalize();
    Ok(Some(config))
}

fn save_conn(conn: &Connection, config: &Config) -> Result<u64> {
    let mut config = config.clone();
    config.normalize();
    crate::sign::ensure_secret(&mut config);
    super::validate(&config)?;
    let current: Option<Option<i64>> = conn
        .query_row(
            "SELECT json_extract(value, '$.revision') FROM settings WHERE key = ?1",
            params![CONFIG_KEY],
            |row| row.get(0),
        )
        .optional()?;
    config.revision = current.flatten().unwrap_or(0).max(0) as u64 + 1;
    let text = serde_json::to_string(&config).context("序列化配置失败")?;
    conn.execute(
        "INSERT INTO settings (key, value, updated_at_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
           value = excluded.value,
           updated_at_ms = excluded.updated_at_ms",
        params![CONFIG_KEY, text, now_ms()],
    )?;
    Ok(config.revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (ConfigStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("tv-buddy-config-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = crate::db::connect(&dir.join("tv-buddy.db")).expect("打开库");
        (ConfigStore::new(conn), dir)
    }

    #[test]
    fn empty_database_has_no_config() {
        let (store, dir) = temp_store("empty");
        assert!(store.load().expect("读取").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_round_trips_through_json() {
        let (store, dir) = temp_store("roundtrip");
        let mut config = Config::default();
        config.server.bind = "127.0.0.1:1234".into();
        config.cache.gc.max_disk_mb = 120;
        config.cache.limit_mb = 120;
        // 未建模键会落进 extra，必须跟着往返。
        config.epg.extra.insert(
            "custom_flag".into(),
            serde_json::Value::String("keep".into()),
        );
        store.save(&config).expect("保存");

        let loaded = store.load().expect("读取").expect("有配置");
        assert_eq!(loaded.server.bind, "127.0.0.1:1234");
        assert_eq!(loaded.cache.gc.max_disk_mb, 120);
        assert_eq!(loaded.cache.limit_mb, 120);
        // 未建模段跟着走。
        assert_eq!(
            loaded.epg.extra.get("custom_flag"),
            Some(&serde_json::Value::String("keep".into()))
        );

        // 覆盖保存只有一行。
        let mut next = loaded.clone();
        next.server.bind = "127.0.0.1:5678".into();
        store.save(&next).expect("再保存");
        assert_eq!(
            store.load().expect("读取").expect("有配置").server.bind,
            "127.0.0.1:5678"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_invalid_config() {
        let (store, dir) = temp_store("invalid");
        let mut config = Config::default();
        config.server.bind = "  ".into();
        assert!(store.save(&config).is_err());
        assert!(store.load().expect("读取").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_increments_revision() {
        let (store, dir) = temp_store("revision");
        let mut config = Config::default();
        config.groups.push(super::super::model::Group {
            id: "g".into(),
            access: "public".into(),
            ..Default::default()
        });
        assert_eq!(store.save(&config).expect("首次保存"), 1);

        let loaded = store.load().expect("读取").expect("有配置");
        assert_eq!(loaded.revision, 1);
        assert_eq!(loaded.groups[0].access, "public");

        assert_eq!(store.save(&loaded).expect("再次保存"), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_if_revision_rejects_stale_tokens() {
        let (store, dir) = temp_store("cas");
        let config = Config::default();
        assert_eq!(store.save(&config).expect("首次保存"), 1);

        let stale = store
            .save_if_revision(&config, Some(0))
            .expect_err("旧版本应拒绝");
        assert_eq!(
            stale
                .downcast_ref::<crate::config::StaleRevision>()
                .map(|e| e.0),
            Some(1)
        );

        assert!(store.save_if_revision(&config, None).is_err());
        assert_eq!(
            store.save_if_revision(&config, Some(1)).expect("当前版本"),
            2
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_without_revision_loads_as_zero() {
        let dir = std::env::temp_dir().join("tv-buddy-config-no-revision");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tv-buddy.db");
        let conn = crate::db::connect(&path).expect("打开库");
        let raw = serde_json::json!({
            "server": { "bind": "127.0.0.1:9000" },
            "groups": [{ "id": "g", "name": "组", "mode": "proxy", "access": "public" }]
        });
        conn.lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute(
                "INSERT INTO settings (key, value, updated_at_ms) VALUES (?1, ?2, 0)",
                params![CONFIG_KEY, raw.to_string()],
            )
            .expect("写入配置");
        let store = ConfigStore::new(conn);

        let loaded = store.load().expect("读取").expect("有配置");
        assert_eq!(loaded.revision, 0);
        assert_eq!(store.save(&loaded).expect("保存"), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
