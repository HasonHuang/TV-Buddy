//! 配置持久化：SQLite 单行 JSON 文档。
//!
//! 整份配置存成一条记录（`settings.key = 'config'`），值是 `Config` 的 JSON。
//! 与用户表同一套文档模式，改动只需一次 UPSERT。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

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
        let raw: Option<String> = {
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![CONFIG_KEY],
                |row| row.get(0),
            )
            .optional()?
        };
        let Some(raw) = raw else {
            return Ok(None);
        };
        let config: Config = serde_json::from_str(&raw).context("配置记录解析失败")?;
        Ok(Some(config))
    }

    /// 写配置。先校验，再整条替换。
    pub fn save(&self, config: &Config) -> Result<()> {
        super::validate(config)?;
        let text = serde_json::to_string(config).context("序列化配置失败")?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO settings (key, value, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
               value = excluded.value,
               updated_at_ms = excluded.updated_at_ms",
            params![CONFIG_KEY, text, now_ms()],
        )?;
        Ok(())
    }

    /// 恢复默认配置。用户不受影响。
    pub fn reset(&self) -> Result<Config> {
        let config = Config::default();
        self.save(&config)?;
        Ok(config)
    }
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
        // 未建模键会落进 extra，必须跟着往返。
        config.epg.extra.insert(
            "custom_flag".into(),
            serde_json::Value::String("keep".into()),
        );
        store.save(&config).expect("保存");

        let loaded = store.load().expect("读取").expect("有配置");
        assert_eq!(loaded.server.bind, "127.0.0.1:1234");
        assert_eq!(loaded.cache.gc.max_disk_mb, 120);
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
}
