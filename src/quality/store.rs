//! 质量结果持久化：每个稳定键保留最近一条。

use anyhow::Result;
use rusqlite::{params, params_from_iter, types::Value, OptionalExtension, Row, Transaction};

use crate::db::SharedConn;

const COLUMNS: &str = "key, scope, source_id, source_key, gid, cid, name, target_group, url_hash, action, reason, ok, delay_ms, speed_kib, width, height, http_status, checked_at, next_check_at, first_fail_at, rule_hash, run_id, updated_at_ms";

/// 一条质量结果。前后端共享的稳定字段。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QualityResult {
    pub key: String,
    pub scope: String,
    pub source_id: String,
    pub source_key: String,
    pub gid: String,
    pub cid: String,
    pub name: String,
    pub target_group: String,
    pub url_hash: String,
    pub action: String,
    pub reason: String,
    pub ok: bool,
    pub delay_ms: Option<u32>,
    pub speed_kib: Option<u32>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub http_status: Option<u16>,
    pub checked_at: i64,
    pub next_check_at: i64,
    pub first_fail_at: Option<i64>,
    pub rule_hash: String,
    pub run_id: String,
    pub updated_at_ms: i64,
}

/// 结果查询条件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QualityFilter {
    pub source_id: Option<String>,
    pub gid: Option<String>,
    pub cid: Option<String>,
    pub action: Option<String>,
    pub reason: Option<String>,
    pub q: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

/// 状态页统计。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QualityCounts {
    pub admitted: usize,
    pub removed_7d: usize,
}

/// 结果仓库。
pub struct QualityStore {
    conn: SharedConn,
}

impl QualityStore {
    pub fn new(conn: SharedConn) -> Self {
        Self { conn }
    }

    /// 写入一条结果。同一键只保留最新值。
    pub fn upsert(&self, item: &QualityResult) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        upsert_conn(&conn, item)
    }

    /// 在一个新事务里批量写入。
    pub fn upsert_many(&self, items: &[QualityResult]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let tx = conn.transaction()?;
        self.upsert_many_tx(&tx, items)?;
        tx.commit()?;
        Ok(())
    }

    /// 调用方已持有事务时批量写入。
    pub fn upsert_many_tx(&self, tx: &Transaction<'_>, items: &[QualityResult]) -> Result<()> {
        for item in items {
            upsert_tx(tx, item)?;
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Option<QualityResult>> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM quality_results WHERE key = ?1"),
            params![key],
            row_to_result,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list(&self, filter: &QualityFilter) -> Result<(usize, Vec<QualityResult>)> {
        let (where_sql, values) = filter_sql(filter);
        let limit = if filter.limit == 0 {
            50
        } else {
            filter.limit.min(500)
        };
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM quality_results {where_sql}"),
            params_from_iter(values.iter()),
            |row| row.get(0),
        )?;
        let mut query = format!(
            "SELECT {COLUMNS} FROM quality_results {where_sql} ORDER BY checked_at DESC, key ASC LIMIT ? OFFSET ?"
        );
        let mut page_values = values;
        page_values.push(Value::Integer(limit as i64));
        page_values.push(Value::Integer(filter.offset as i64));
        let mut statement = conn.prepare(&mut query)?;
        let items = statement
            .query_map(params_from_iter(page_values.iter()), row_to_result)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((usize::try_from(total).unwrap_or(usize::MAX), items))
    }

    /// 到期结果。`next_check_at <= now`，按到期时间升序。
    pub fn due(&self, now: i64, limit: usize) -> Result<Vec<QualityResult>> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM quality_results
             WHERE next_check_at > 0 AND next_check_at <= ?1
             ORDER BY next_check_at ASC, checked_at ASC LIMIT ?2"
        ))?;
        let items = statement
            .query_map(params![now, limit as i64], row_to_result)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }

    /// 删除非准入的过期结果。返回删除行数。
    pub fn cleanup(&self, retention_days: u64, now: i64) -> Result<usize> {
        if retention_days == 0 {
            return Ok(0);
        }
        let cutoff = now.saturating_sub(
            i64::try_from(retention_days)
                .unwrap_or(i64::MAX / 86_400)
                .saturating_mul(86_400),
        );
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        Ok(conn.execute(
            "DELETE FROM quality_results WHERE action != 'admitted' AND checked_at < ?1",
            params![cutoff],
        )?)
    }

    /// 手工频道结果。启动水合使用。
    pub fn manual_entries(&self) -> Result<Vec<QualityResult>> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM quality_results WHERE scope = 'manual'"
        ))?;
        let items = statement
            .query_map([], row_to_result)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }

    pub fn counts(&self, since: i64) -> Result<QualityCounts> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let admitted: i64 = conn.query_row(
            "SELECT COUNT(*) FROM quality_results WHERE action = 'admitted'",
            [],
            |row| row.get(0),
        )?;
        let removed_7d: i64 = conn.query_row(
            "SELECT COUNT(*) FROM quality_results WHERE action = 'removed' AND checked_at >= ?1",
            params![since],
            |row| row.get(0),
        )?;
        Ok(QualityCounts {
            admitted: usize::try_from(admitted).unwrap_or(usize::MAX),
            removed_7d: usize::try_from(removed_7d).unwrap_or(usize::MAX),
        })
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        conn.execute("DELETE FROM quality_results WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        Ok(conn
            .query_row(
                "SELECT value FROM quality_meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        meta_set_conn(&conn, key, value)
    }

    pub fn meta_set_tx(&self, tx: &Transaction<'_>, key: &str, value: &str) -> Result<()> {
        meta_set_tx(tx, key, value)
    }
}

fn filter_sql(filter: &QualityFilter) -> (String, Vec<Value>) {
    let mut clauses = vec!["1 = 1".to_string()];
    let mut values = Vec::new();
    if let Some(source_id) = filter.source_id.as_deref() {
        clauses.push("source_id = ?".into());
        values.push(Value::Text(source_id.to_string()));
    }
    if let Some(gid) = filter.gid.as_deref() {
        clauses.push("gid = ?".into());
        values.push(Value::Text(gid.to_string()));
    }
    if let Some(cid) = filter.cid.as_deref() {
        clauses.push("cid = ?".into());
        values.push(Value::Text(cid.to_string()));
    }
    if let Some(action) = filter.action.as_deref() {
        clauses.push("action = ?".into());
        values.push(Value::Text(action.to_string()));
    }
    if let Some(reason) = filter.reason.as_deref() {
        clauses.push("reason = ?".into());
        values.push(Value::Text(reason.to_string()));
    }
    if let Some(q) = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        clauses.push("name LIKE ?".into());
        values.push(Value::Text(format!("%{q}%")));
    }
    (format!("WHERE {}", clauses.join(" AND ")), values)
}

fn upsert_conn(conn: &rusqlite::Connection, item: &QualityResult) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO quality_results ({COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)
             ON CONFLICT(key) DO UPDATE SET
               scope=excluded.scope, source_id=excluded.source_id, source_key=excluded.source_key,
               gid=excluded.gid, cid=excluded.cid, name=excluded.name, target_group=excluded.target_group,
               url_hash=excluded.url_hash, action=excluded.action, reason=excluded.reason, ok=excluded.ok,
               delay_ms=excluded.delay_ms, speed_kib=excluded.speed_kib, width=excluded.width,
               height=excluded.height, http_status=excluded.http_status, checked_at=excluded.checked_at,
               next_check_at=excluded.next_check_at, first_fail_at=excluded.first_fail_at,
               rule_hash=excluded.rule_hash, run_id=excluded.run_id, updated_at_ms=excluded.updated_at_ms"
        ),
        params![
            item.key,
            item.scope,
            item.source_id,
            item.source_key,
            item.gid,
            item.cid,
            item.name,
            item.target_group,
            item.url_hash,
            item.action,
            item.reason,
            i64::from(item.ok),
            item.delay_ms.map(i64::from),
            item.speed_kib.map(i64::from),
            item.width.map(i64::from),
            item.height.map(i64::from),
            item.http_status.map(i64::from),
            item.checked_at,
            item.next_check_at,
            item.first_fail_at,
            item.rule_hash,
            item.run_id,
            crate::users::model::now_ms(),
        ],
    )?;
    Ok(())
}

fn upsert_tx(tx: &Transaction<'_>, item: &QualityResult) -> Result<()> {
    tx.execute(
        &format!(
            "INSERT INTO quality_results ({COLUMNS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)
             ON CONFLICT(key) DO UPDATE SET
               scope=excluded.scope, source_id=excluded.source_id, source_key=excluded.source_key,
               gid=excluded.gid, cid=excluded.cid, name=excluded.name, target_group=excluded.target_group,
               url_hash=excluded.url_hash, action=excluded.action, reason=excluded.reason, ok=excluded.ok,
               delay_ms=excluded.delay_ms, speed_kib=excluded.speed_kib, width=excluded.width,
               height=excluded.height, http_status=excluded.http_status, checked_at=excluded.checked_at,
               next_check_at=excluded.next_check_at, first_fail_at=excluded.first_fail_at,
               rule_hash=excluded.rule_hash, run_id=excluded.run_id, updated_at_ms=excluded.updated_at_ms"
        ),
        params![
            item.key,
            item.scope,
            item.source_id,
            item.source_key,
            item.gid,
            item.cid,
            item.name,
            item.target_group,
            item.url_hash,
            item.action,
            item.reason,
            i64::from(item.ok),
            item.delay_ms.map(i64::from),
            item.speed_kib.map(i64::from),
            item.width.map(i64::from),
            item.height.map(i64::from),
            item.http_status.map(i64::from),
            item.checked_at,
            item.next_check_at,
            item.first_fail_at,
            item.rule_hash,
            item.run_id,
            crate::users::model::now_ms(),
        ],
    )?;
    Ok(())
}

fn meta_set_conn(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO quality_meta (key, value, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
        params![key, value, crate::users::model::now_ms()],
    )?;
    Ok(())
}

fn meta_set_tx(tx: &Transaction<'_>, key: &str, value: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO quality_meta (key, value, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
        params![key, value, crate::users::model::now_ms()],
    )?;
    Ok(())
}

fn row_to_result(row: &Row<'_>) -> rusqlite::Result<QualityResult> {
    Ok(QualityResult {
        key: row.get(0)?,
        scope: row.get(1)?,
        source_id: row.get(2)?,
        source_key: row.get(3)?,
        gid: row.get(4)?,
        cid: row.get(5)?,
        name: row.get(6)?,
        target_group: row.get(7)?,
        url_hash: row.get(8)?,
        action: row.get(9)?,
        reason: row.get(10)?,
        ok: row.get::<_, i64>(11)? != 0,
        delay_ms: row.get::<_, Option<i64>>(12)?.map(|value| value as u32),
        speed_kib: row.get::<_, Option<i64>>(13)?.map(|value| value as u32),
        width: row.get::<_, Option<i64>>(14)?.map(|value| value as u16),
        height: row.get::<_, Option<i64>>(15)?.map(|value| value as u16),
        http_status: row.get::<_, Option<i64>>(16)?.map(|value| value as u16),
        checked_at: row.get(17)?,
        next_check_at: row.get(18)?,
        first_fail_at: row.get(19)?,
        rule_hash: row.get(20)?,
        run_id: row.get(21)?,
        updated_at_ms: row.get(22)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (QualityStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("tvb-quality-{}", uuid::Uuid::new_v4()));
        let conn = crate::db::connect(&dir.join("tvb.db")).expect("db");
        (QualityStore::new(conn), dir)
    }

    fn result(key: &str, action: &str, checked_at: i64) -> QualityResult {
        QualityResult {
            key: key.into(),
            scope: "sync".into(),
            source_id: "s1".into(),
            source_key: "name:CCTV-5".into(),
            gid: "sports".into(),
            cid: "c5".into(),
            name: "CCTV-5".into(),
            target_group: "体育".into(),
            url_hash: "abc".into(),
            action: action.into(),
            reason: "pass".into(),
            checked_at,
            ..QualityResult::default()
        }
    }

    #[test]
    fn upsert_updates_same_stable_key() {
        let (store, dir) = store();
        store
            .upsert(&result("sync:s1:name:CCTV-5", "admitted", 1))
            .unwrap();
        store
            .upsert(&result("sync:s1:name:CCTV-5", "removed", 2))
            .unwrap();
        let (total, items) = store.list(&QualityFilter::default()).unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].action, "removed");
        assert!(store.get("sync:s1:name:CCTV-5").unwrap().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cleanup_keeps_admitted_and_recent() {
        let (store, dir) = store();
        let now = 1_000_000;
        store.upsert(&result("sync:s1:a", "admitted", 1)).unwrap();
        store
            .upsert(&result("sync:s1:b", "removed", now - 8 * 86_400))
            .unwrap();
        store
            .upsert(&result("sync:s1:c", "removed", now - 86_400))
            .unwrap();
        assert_eq!(store.cleanup(7, now).unwrap(), 1);
        assert!(
            store.get("sync:s1:a").unwrap().is_some(),
            "admitted 不按保留期删除"
        );
        assert!(store.get("sync:s1:b").unwrap().is_none());
        assert!(store.get("sync:s1:c").unwrap().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn list_filters_and_meta_roundtrip() {
        let (store, dir) = store();
        store.upsert(&result("sync:s1:a", "admitted", 1)).unwrap();
        store.upsert(&result("sync:s1:b", "removed", 2)).unwrap();
        let filter = QualityFilter {
            action: Some("removed".into()),
            ..QualityFilter::default()
        };
        assert_eq!(store.list(&filter).unwrap().0, 1);
        store.meta_set("next_run_at", "123").unwrap();
        assert_eq!(
            store.meta_get("next_run_at").unwrap().as_deref(),
            Some("123")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
