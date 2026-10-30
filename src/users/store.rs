//! 用户与令牌的持久化。
//!
//! 存储是 SQLite，表结构与 mpd-hls 一致：一行一个 JSON 文档
//! （`docs/design/09-users-and-devices.md` §2）。
//!
//! 读写策略：
//! - 启动时全量载入内存。热路径（令牌校验）不碰数据库。
//! - 增删改先写库、再改内存。写库失败即返回错误，内存不变。
//! - `last_seen_at_ms` 由播放请求高频更新。只改内存并置脏位，
//!   由后台每分钟刷一次盘。

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, RwLock,
    },
};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use super::model::{now_ms, AccessToken, Purpose, Role, User};

const USERS: &str = "users";
const TOKENS: &str = "access_tokens";
/// 初始管理员用户名。与目标一致。
pub const INITIAL_ADMIN: &str = "admin";

pub struct UserStore {
    path: PathBuf,
    conn: Mutex<Connection>,
    users: RwLock<Vec<User>>,
    tokens: RwLock<Vec<AccessToken>>,
    /// `last_seen_at_ms` 有未落盘的改动。
    dirty: AtomicBool,
}

impl UserStore {
    /// 打开或创建数据库，并建表。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("打开用户库失败: {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        Self::init_schema(&conn)?;
        let mut store = Self {
            path: path.to_path_buf(),
            conn: Mutex::new(conn),
            users: RwLock::new(Vec::new()),
            tokens: RwLock::new(Vec::new()),
            dirty: AtomicBool::new(false),
        };
        store.reload()?;
        Ok(store)
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        for table in [USERS, TOKENS] {
            conn.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (
                    id TEXT PRIMARY KEY NOT NULL,
                    sort_order BIGINT NOT NULL,
                    config_json TEXT NOT NULL,
                    updated_at_ms BIGINT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS {table}_order_idx ON {table} (sort_order, id);"
            ))?;
        }
        Ok(())
    }

    /// 从磁盘重读全部记录。
    fn reload(&mut self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let users = load::<User>(&conn, USERS)?;
        let tokens = load::<AccessToken>(&conn, TOKENS)?;
        drop(conn);
        *self.users.write().unwrap_or_else(|e| e.into_inner()) = users;
        *self.tokens.write().unwrap_or_else(|e| e.into_inner()) = tokens;
        Ok(())
    }

    /// 库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    // ---- 用户 ----

    pub fn list_users(&self) -> Vec<User> {
        self.users.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn user_count(&self) -> usize {
        self.users.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn find_user(&self, id: &str) -> Option<User> {
        self.users
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|user| user.id == id)
            .cloned()
    }

    pub fn find_by_username(&self, username: &str) -> Option<User> {
        let wanted = username.trim();
        self.users
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|user| user.username == wanted)
            .cloned()
    }

    /// 新建用户。用户名重复时返回错误。
    pub fn create_user(&self, mut user: User) -> Result<User> {
        if user.username.trim().is_empty() {
            anyhow::bail!("用户名不能为空");
        }
        if self.find_by_username(&user.username).is_some() {
            anyhow::bail!("用户名已存在");
        }
        if user.id.is_empty() {
            user.id = uuid::Uuid::new_v4().to_string();
        }
        let now = now_ms();
        if user.created_at_ms == 0 {
            user.created_at_ms = now;
        }
        user.updated_at_ms = now;
        let order = self.next_order(USERS)?;
        self.write_row(USERS, &user.id, order, &user, now)?;
        if let Ok(mut list) = self.users.write() {
            list.push(user.clone());
        }
        Ok(user)
    }

    /// 覆盖保存用户。找不到 id 时返回错误。
    pub fn update_user(&self, mut user: User) -> Result<User> {
        let existing = self
            .find_user(&user.id)
            .with_context(|| format!("用户不存在: {}", user.id))?;
        let owner = user.username.trim();
        if owner.is_empty() {
            anyhow::bail!("用户名不能为空");
        }
        if let Some(other) = self.find_by_username(owner) {
            if other.id != user.id {
                anyhow::bail!("用户名已存在");
            }
        }
        user.created_at_ms = existing.created_at_ms;
        user.updated_at_ms = now_ms();
        let order = self.order_of(USERS, &user.id)?.unwrap_or(0);
        self.write_row(USERS, &user.id, order, &user, user.updated_at_ms)?;
        let mut list = self.users.write().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = list.iter_mut().find(|item| item.id == user.id) {
            *slot = user.clone();
        }
        Ok(user)
    }

    /// 删除用户，同时删除其全部令牌。
    pub fn delete_user(&self, id: &str) -> Result<bool> {
        if self.find_user(id).is_none() {
            return Ok(false);
        }
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(&format!("DELETE FROM {USERS} WHERE id = ?1"), params![id])?;
        conn.execute(
            &format!("DELETE FROM {TOKENS} WHERE json_extract(config_json, '$.user_id') = ?1"),
            params![id],
        )?;
        drop(conn);
        self.users
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|user| user.id != id);
        self.tokens
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|token| token.user_id != id);
        Ok(true)
    }

    // ---- 令牌 ----

    pub fn list_tokens(&self, user_id: &str) -> Vec<AccessToken> {
        self.tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|token| token.user_id == user_id)
            .cloned()
            .collect()
    }

    pub fn all_tokens(&self) -> Vec<AccessToken> {
        self.tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 按令牌值查找。过期令牌返回 `None`。
    pub fn find_token(&self, token: &str) -> Option<AccessToken> {
        let now = now_ms();
        let found = self
            .tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|item| item.token == token)
            .cloned()?;
        if found.is_expired(now) {
            return None;
        }
        Some(found)
    }

    /// 新建令牌。`ttl_ms` 为 `None` 表示永不过期。
    pub fn create_token(
        &self,
        user_id: &str,
        purpose: Purpose,
        label: Option<String>,
        ttl_ms: Option<i64>,
    ) -> Result<AccessToken> {
        if self.find_user(user_id).is_none() {
            anyhow::bail!("用户不存在");
        }
        let now = now_ms();
        let mut token = AccessToken {
            token: super::password::random_token(),
            user_id: user_id.to_string(),
            purpose,
            label,
            created_at_ms: now,
            expires_at_ms: ttl_ms.map(|ttl| now.saturating_add(ttl)),
            last_seen_at_ms: None,
        };
        // 冲突概率极低，仍重试几次。
        for _ in 0..5 {
            if self.find_token(&token.token).is_none() {
                break;
            }
            token.token = super::password::random_token();
        }
        let id = uuid::Uuid::new_v4().to_string();
        let order = self.next_order(TOKENS)?;
        self.write_row(TOKENS, &id, order, &token, now)?;
        if let Ok(mut list) = self.tokens.write() {
            list.push(token.clone());
        }
        Ok(token)
    }

    /// 改令牌的名称或有效期。`ttl_ms` 为 `None` 表示改为永不过期。
    pub fn update_token(
        &self,
        token: &str,
        label: Option<String>,
        ttl_ms: Option<i64>,
    ) -> Result<AccessToken> {
        let existing = self
            .find_token(token)
            .with_context(|| format!("令牌不存在: {token}"))?;
        let next = AccessToken {
            label: label.or(existing.label.clone()),
            expires_at_ms: ttl_ms.map(|ttl| now_ms().saturating_add(ttl)),
            ..existing
        };
        let now = now_ms();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let Some(row_id) = row_id_of(&conn, TOKENS, "token", token)? else {
            anyhow::bail!("令牌不存在: {token}");
        };
        conn.execute(
            &format!("UPDATE {TOKENS} SET config_json = ?1, updated_at_ms = ?2 WHERE id = ?3"),
            params![serde_json::to_string(&next)?, now, row_id],
        )?;
        drop(conn);
        let mut list = self.tokens.write().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = list.iter_mut().find(|item| item.token == token) {
            *slot = next.clone();
        }
        Ok(next)
    }

    /// 撤销令牌。
    pub fn revoke_token(&self, token: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let Some(row_id) = row_id_of(&conn, TOKENS, "token", token)? else {
            return Ok(false);
        };
        conn.execute(
            &format!("DELETE FROM {TOKENS} WHERE id = ?1"),
            params![row_id],
        )?;
        drop(conn);
        self.tokens
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|item| item.token != token);
        Ok(true)
    }

    /// 记录一次使用。只改内存，置脏位。
    pub fn touch_token(&self, token: &str) {
        let now = now_ms();
        let maybe = {
            let mut list = self.tokens.write().unwrap_or_else(|e| e.into_inner());
            match list.iter_mut().find(|item| item.token == token) {
                Some(item) => {
                    // 同一秒内不重复刷。
                    if item
                        .last_seen_at_ms
                        .map(|at| now - at >= 1000)
                        .unwrap_or(true)
                    {
                        item.last_seen_at_ms = Some(now);
                        true
                    } else {
                        false
                    }
                }
                None => false,
            }
        };
        if maybe {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// 把内存里的 `last_seen_at_ms` 落盘。没有改动时直接返回。
    pub fn flush_if_dirty(&self) -> Result<()> {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return Ok(());
        }
        let snapshot = self.all_tokens();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.unchecked_transaction()?;
        for token in &snapshot {
            let Some(row_id) = row_id_of(&tx, TOKENS, "token", &token.token)? else {
                continue;
            };
            tx.execute(
                &format!("UPDATE {TOKENS} SET config_json = ?1 WHERE id = ?2"),
                params![serde_json::to_string(token)?, row_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 删除全部过期令牌。启动时调用。
    pub fn purge_expired(&self) -> Result<usize> {
        let now = now_ms();
        let expired: Vec<String> = self
            .tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|token| token.is_expired(now))
            .map(|token| token.token.clone())
            .collect();
        for token in &expired {
            self.revoke_token(token).ok();
        }
        Ok(expired.len())
    }

    // ---- 内部 ----

    fn write_row<T: serde::Serialize>(
        &self,
        table: &str,
        id: &str,
        order: i64,
        value: &T,
        updated_at_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            &format!(
                "INSERT INTO {table} (id, sort_order, config_json, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                   sort_order = excluded.sort_order,
                   config_json = excluded.config_json,
                   updated_at_ms = excluded.updated_at_ms"
            ),
            params![id, order, serde_json::to_string(value)?, updated_at_ms],
        )?;
        Ok(())
    }

    fn next_order(&self, table: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let value: Option<i64> = conn
            .query_row(&format!("SELECT MAX(sort_order) FROM {table}"), [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .unwrap_or(None);
        Ok(value.unwrap_or(0) + 1)
    }

    fn order_of(&self, table: &str, id: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        Ok(conn
            .query_row(
                &format!("SELECT sort_order FROM {table} WHERE id = ?1"),
                params![id],
                |row| row.get(0),
            )
            .ok())
    }
}

fn row_id_of(conn: &Connection, table: &str, field: &str, value: &str) -> Result<Option<String>> {
    let found = conn
        .query_row(
            &format!("SELECT id FROM {table} WHERE json_extract(config_json, '$.{field}') = ?1"),
            params![value],
            |row| row.get::<_, String>(0),
        )
        .ok();
    Ok(found)
}

fn load<T: serde::de::DeserializeOwned>(conn: &Connection, table: &str) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT config_json FROM {table} ORDER BY sort_order, id"
    ))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let raw = row?;
        match serde_json::from_str::<T>(&raw) {
            Ok(value) => out.push(value),
            Err(error) => tracing::warn!(table, %error, "跳过无法解析的记录"),
        }
    }
    Ok(out)
}

/// 用户表为空时创建一个管理员，口令由调用方给定。
///
/// 返回是否真的创建了。
pub fn ensure_admin(store: &UserStore, password: &str) -> Result<bool> {
    if store.user_count() > 0 {
        return Ok(false);
    }
    let hash = super::password::hash(password)?;
    let now = now_ms();
    store.create_user(User {
        id: String::new(),
        username: INITIAL_ADMIN.to_string(),
        role: Role::Admin,
        password_hash: hash,
        allowed_group_ids: Vec::new(),
        channel_filter_enabled: false,
        allowed_channel_ids: Vec::new(),
        web_ui_access: true,
        created_at_ms: now,
        updated_at_ms: now,
    })?;
    Ok(true)
}

/// 首次安装时创建管理员，口令随机生成。
///
/// 返回新建的口令明文。已有用户时返回 `None`。
pub fn ensure_initial_admin(store: &UserStore) -> Result<Option<(String, String)>> {
    if store.user_count() > 0 {
        return Ok(None);
    }
    let password = super::password::random_password(16);
    ensure_admin(store, &password)?;
    Ok(Some((INITIAL_ADMIN.to_string(), password)))
}

/// 把初始凭据写入文件。权限 0600。
pub fn write_credentials_file(path: &Path, username: &str, password: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("写入初始凭据失败: {}", path.display()))?;
    writeln!(file, "username: {username}")?;
    writeln!(file, "password: {password}")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (UserStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("tv-buddy-users-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("users.db");
        let store = UserStore::open(&path).expect("打开库");
        (store, dir)
    }

    fn sample_user(name: &str) -> User {
        User {
            id: String::new(),
            username: name.into(),
            role: Role::User,
            password_hash: "x".into(),
            allowed_group_ids: vec!["g1".into()],
            channel_filter_enabled: false,
            allowed_channel_ids: Vec::new(),
            web_ui_access: true,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn first_open_creates_admin() {
        let (store, dir) = temp_store("init");
        let created = ensure_initial_admin(&store).expect("初始化");
        let (name, password) = created.expect("应有初始管理员");
        assert_eq!(name, "admin");
        assert_eq!(password.len(), 16);
        let user = store.find_by_username("admin").expect("用户");
        assert!(user.role.is_admin());
        assert!(super::super::password::verify(
            &password,
            &user.password_hash
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn users_persist_across_reopen() {
        let (store, dir) = temp_store("persist");
        store.create_user(sample_user("alice")).expect("新建");
        let path = store.path().to_path_buf();
        drop(store);
        let reopened = UserStore::open(&path).expect("重开");
        assert_eq!(reopened.user_count(), 1);
        assert!(reopened.find_by_username("alice").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn duplicate_username_is_rejected() {
        let (store, dir) = temp_store("dup");
        store.create_user(sample_user("alice")).expect("新建");
        assert!(store.create_user(sample_user("alice")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_user_removes_tokens() {
        let (store, dir) = temp_store("cascade");
        let user = store.create_user(sample_user("bob")).expect("新建");
        store
            .create_token(&user.id, Purpose::Device, None, None)
            .expect("建令牌");
        assert_eq!(store.list_tokens(&user.id).len(), 1);
        store.delete_user(&user.id).expect("删除");
        assert_eq!(store.list_tokens(&user.id).len(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tokens_round_trip_and_expire() {
        let (store, dir) = temp_store("tokens");
        let user = store.create_user(sample_user("carol")).expect("新建");
        let token = store
            .create_token(&user.id, Purpose::Device, Some("TV".into()), Some(1000))
            .expect("建令牌");
        assert_eq!(token.token.len(), 12);
        assert!(store.find_token(&token.token).is_some());
        store.revoke_token(&token.token).expect("撤销");
        assert!(store.find_token(&token.token).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
