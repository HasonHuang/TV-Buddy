//! EPG 配置存储：源、频道绑定、关键字规则。
//!
//! 三张表各存一行一个 JSON 文档（`epg_sources` / `epg_bindings` / `epg_rules`）。
//! 启动时全量载入内存。写入先落库，再改内存。
//!
//! 源与绑定是节目单采集的输入。规则由 [`crate::epg_rules`] 消费。

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::SharedConn;

const SOURCES: &str = "epg_sources";
const BINDINGS: &str = "epg_bindings";
const RULES: &str = "epg_rules";

/// 一个采集源。
///
/// `kind = "xml"` 时读 `urls`；`kind = "script"` 时跑 `script`。
/// `urls` 可有多条，同一源内按 `priority` 升序合并。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// `xml` 或 `script`。
    pub kind: String,
    pub urls: Vec<SourceUrl>,
    /// 刷新间隔（秒）。0 表示沿用 `cron` 或全局采集节奏。
    pub refresh_secs: u64,
    /// 数字越小越优先。多源同频道时高优先级覆盖低优先级。
    pub priority: i64,
    /// 旧脚本源保留字段。
    pub script: String,
    pub output_files: Vec<String>,
    pub timeout_secs: u64,
    pub tz_shift_mins: i64,
    /// 旧字段。非空时按 cron 调度该源。
    pub cron: String,
    #[serde(flatten)]
    pub extra: crate::config::model::Extra,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            kind: "xml".into(),
            urls: Vec::new(),
            refresh_secs: 21_600,
            priority: 0,
            script: String::new(),
            output_files: Vec::new(),
            timeout_secs: 0,
            tz_shift_mins: 0,
            cron: String::new(),
            extra: crate::config::model::Extra::new(),
        }
    }
}

/// 源内的一条 URL。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceUrl {
    pub url: String,
    /// 数字越小越优先。
    pub priority: i64,
    pub user_agent: String,
    /// 自定义请求头。每项保存名称与值。
    pub headers: Vec<Header>,
}

/// 自定义请求头。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// 频道绑定：本地频道 → 源内频道。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Binding {
    pub gid: String,
    pub cid: String,
    pub source_id: String,
    pub raw_channel_id: String,
    /// `auto`（采集时精确匹配写入）或 `manual`（面板指定）。
    pub origin: String,
    pub upstream_display_name: String,
    pub updated_at_ms: i64,
}

impl Binding {
    /// 绑定键 `gid/cid`。
    pub fn key(&self) -> String {
        format!("{}/{}", self.gid, self.cid)
    }

    /// 是否为手工绑定。
    pub fn manual(&self) -> bool {
        self.origin == "manual"
    }
}

/// 关键字规则的条件树。
///
/// `op = "all"` 时全部命中；`"any"` 时任一命中；`"not"` 时全部不命中。
/// 关键字与子组还可以同时出现。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Condition {
    pub op: String,
    pub keywords: Vec<String>,
    pub groups: Vec<Condition>,
}

impl Default for Condition {
    fn default() -> Self {
        Self {
            op: "all".into(),
            keywords: Vec::new(),
            groups: Vec::new(),
        }
    }
}

/// 关键字规则。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub gid: String,
    pub cid: String,
    pub condition: Condition,
    /// `record`、`start` 或 `stop`。
    pub action: String,
    pub lead_secs: i64,
    pub tail_secs: i64,
    /// 录制产物子目录。空表示全局默认。
    pub output_dir: String,
    /// 录制任务名模板。
    pub name_template: String,
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            gid: String::new(),
            cid: String::new(),
            condition: Condition::default(),
            action: "record".into(),
            lead_secs: 0,
            tail_secs: 0,
            output_dir: String::new(),
            name_template: "{program.title} {datetime}".into(),
            enabled: true,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }
}

/// EPG 配置快照。备份与恢复用。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub sources: Vec<Source>,
    pub bindings: Vec<Binding>,
    pub rules: Vec<Rule>,
}

/// EPG 配置仓库。
pub struct Store {
    conn: SharedConn,
    sources: RwLock<Vec<Source>>,
    bindings: RwLock<Vec<Binding>>,
    rules: RwLock<Vec<Rule>>,
}

impl Store {
    /// 依附到已建表的数据库连接，并载入全部记录。
    pub fn attach(conn: SharedConn) -> Result<Self> {
        let store = Self {
            conn,
            sources: RwLock::new(Vec::new()),
            bindings: RwLock::new(Vec::new()),
            rules: RwLock::new(Vec::new()),
        };
        store.reload()?;
        Ok(store)
    }

    /// 从数据库重读全部记录。
    pub fn reload(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let sources = load::<Source>(&conn, SOURCES)?;
        let bindings = load::<Binding>(&conn, BINDINGS)?;
        let rules = load::<Rule>(&conn, RULES)?;
        drop(conn);
        *write(&self.sources) = sources;
        *write(&self.bindings) = bindings;
        *write(&self.rules) = rules;
        Ok(())
    }

    // ---- 源 ----

    pub fn sources(&self) -> Vec<Source> {
        read(&self.sources).clone()
    }

    pub fn source(&self, id: &str) -> Option<Source> {
        read(&self.sources)
            .iter()
            .find(|source| source.id == id)
            .cloned()
    }

    /// 新增或覆盖一个源。空 `id` 表示新增。
    pub fn save_source(&self, mut source: Source) -> Result<Source> {
        // 列表接口会把运行时状态拼进对象。回写时丢掉这些键。
        for key in [
            "last_fetch_started_at_ms",
            "last_fetch_finished_at_ms",
            "last_fetch_ok",
            "last_error",
            "channels",
            "programmes",
            "coverage_start_ms",
            "coverage_end_ms",
            "url_statuses",
        ] {
            source.extra.remove(key);
        }
        source.name = source.name.trim().to_string();
        if source.name.is_empty() {
            bail!("缺少源名称");
        }
        source.kind = if source.kind.trim().is_empty() {
            "xml".into()
        } else {
            source.kind.trim().to_ascii_lowercase()
        };
        match source.kind.as_str() {
            "xml" => {
                source.urls.retain(|item| !item.url.trim().is_empty());
                for url in &mut source.urls {
                    url.url = url.url.trim().to_string();
                    url.user_agent = url.user_agent.trim().to_string();
                    url.headers.retain(|header| !header.name.trim().is_empty());
                    for header in &mut url.headers {
                        header.name = header.name.trim().to_string();
                        header.value = header.value.trim().to_string();
                    }
                }
                if source.urls.is_empty() {
                    bail!("至少需要 1 个 URL");
                }
            }
            "script" => {
                if source.script.trim().is_empty() {
                    bail!("缺少脚本名");
                }
            }
            other => bail!("未知源类型 {other}"),
        }
        let now = crate::users::model::now_ms();
        if source.id.is_empty() {
            source.id = uuid::Uuid::new_v4().to_string();
        }
        let order = self
            .order_of(SOURCES, &source.id)?
            .unwrap_or(self.next_order(SOURCES)?);
        self.write_row(SOURCES, &source.id, order, &source, now)?;
        let mut list = write(&self.sources);
        match list.iter_mut().find(|item| item.id == source.id) {
            Some(existing) => *existing = source.clone(),
            None => list.push(source.clone()),
        }
        Ok(source)
    }

    /// 删除一个源及其绑定。
    pub fn delete_source(&self, id: &str) -> Result<bool> {
        let exists = read(&self.sources).iter().any(|source| source.id == id);
        if !exists {
            return Ok(false);
        }
        {
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute("DELETE FROM epg_sources WHERE id = ?1", params![id])?;
            conn.execute(
                "DELETE FROM epg_bindings
                 WHERE json_extract(config_json, '$.source_id') = ?1",
                params![id],
            )?;
        }
        write(&self.sources).retain(|source| source.id != id);
        write(&self.bindings).retain(|binding| binding.source_id != id);
        Ok(true)
    }

    // ---- 绑定 ----

    pub fn bindings(&self) -> Vec<Binding> {
        read(&self.bindings).clone()
    }

    pub fn binding(&self, gid: &str, cid: &str) -> Option<Binding> {
        read(&self.bindings)
            .iter()
            .find(|binding| binding.gid == gid && binding.cid == cid)
            .cloned()
    }

    /// 写一条绑定。手工绑定优先保留；自动绑定不得覆盖手工绑定。
    pub fn set_binding(&self, mut binding: Binding) -> Result<Binding> {
        binding.gid = binding.gid.trim().to_string();
        binding.cid = binding.cid.trim().to_string();
        binding.source_id = binding.source_id.trim().to_string();
        binding.raw_channel_id = binding.raw_channel_id.trim().to_string();
        if binding.gid.is_empty() || binding.cid.is_empty() {
            bail!("缺少 gid/cid");
        }
        if binding.source_id.is_empty() || binding.raw_channel_id.is_empty() {
            bail!("缺少 source_id/raw_channel_id");
        }
        if self.source(&binding.source_id).is_none() {
            bail!("EPG 源不存在");
        }
        binding.origin = if binding.origin == "manual" {
            "manual".into()
        } else {
            "auto".into()
        };
        if let Some(existing) = self.binding(&binding.gid, &binding.cid) {
            if existing.manual() && !binding.manual() {
                return Ok(existing);
            }
        }
        binding.updated_at_ms = crate::users::model::now_ms();
        let key = binding.key();
        let order = self
            .order_of(BINDINGS, &key)?
            .unwrap_or(self.next_order(BINDINGS)?);
        self.write_row(BINDINGS, &key, order, &binding, binding.updated_at_ms)?;
        let mut list = write(&self.bindings);
        match list
            .iter_mut()
            .find(|item| item.gid == binding.gid && item.cid == binding.cid)
        {
            Some(existing) => *existing = binding.clone(),
            None => list.push(binding.clone()),
        }
        Ok(binding)
    }

    /// 删除一条绑定。
    pub fn remove_binding(&self, gid: &str, cid: &str) -> Result<bool> {
        let key = format!("{gid}/{cid}");
        let exists = read(&self.bindings)
            .iter()
            .any(|binding| binding.gid == gid && binding.cid == cid);
        if !exists {
            return Ok(false);
        }
        {
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute("DELETE FROM epg_bindings WHERE id = ?1", params![key])?;
        }
        write(&self.bindings).retain(|binding| !(binding.gid == gid && binding.cid == cid));
        Ok(true)
    }

    /// 用采集结果替换全部自动绑定。手工绑定保持不变。
    pub fn replace_auto_bindings(&self, incoming: Vec<Binding>) -> Result<()> {
        let manual: HashMap<String, Binding> = read(&self.bindings)
            .iter()
            .filter(|binding| binding.manual())
            .map(|binding| (binding.key(), binding.clone()))
            .collect();
        let now = crate::users::model::now_ms();
        let mut wanted: HashMap<String, Binding> = HashMap::new();
        for mut binding in incoming {
            binding.origin = "auto".into();
            binding.updated_at_ms = now;
            if manual.contains_key(&binding.key()) {
                continue;
            }
            if self.source(&binding.source_id).is_none() {
                continue;
            }
            wanted.insert(binding.key(), binding);
        }
        let current: HashMap<String, Binding> = read(&self.bindings)
            .iter()
            .map(|binding| (binding.key(), binding.clone()))
            .collect();
        let removed: Vec<String> = current
            .iter()
            .filter(|(key, binding)| !binding.manual() && !wanted.contains_key(*key))
            .map(|(key, _)| key.clone())
            .collect();
        let changed: Vec<Binding> = wanted
            .values()
            .filter(|binding| {
                current.get(&binding.key()).is_none_or(|existing| {
                    existing.source_id != binding.source_id
                        || existing.raw_channel_id != binding.raw_channel_id
                        || existing.upstream_display_name != binding.upstream_display_name
                })
            })
            .cloned()
            .collect();
        if removed.is_empty() && changed.is_empty() {
            return Ok(());
        }
        {
            let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            let transaction = conn.transaction()?;
            for key in &removed {
                transaction.execute("DELETE FROM epg_bindings WHERE id = ?1", params![key])?;
            }
            for binding in &changed {
                let order = self.next_order_tx(&transaction, BINDINGS)?;
                transaction.execute(
                    "INSERT INTO epg_bindings (id, sort_order, config_json, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET
                       config_json = excluded.config_json,
                       updated_at_ms = excluded.updated_at_ms",
                    params![
                        binding.key(),
                        order,
                        serde_json::to_string(binding)?,
                        binding.updated_at_ms
                    ],
                )?;
            }
            transaction.commit()?;
        }
        let mut list = write(&self.bindings);
        list.retain(|binding| binding.manual() || wanted.contains_key(&binding.key()));
        for binding in changed {
            match list
                .iter_mut()
                .find(|item| item.gid == binding.gid && item.cid == binding.cid)
            {
                Some(existing) => *existing = binding,
                None => list.push(binding),
            }
        }
        Ok(())
    }

    // ---- 规则 ----

    pub fn rules(&self) -> Vec<Rule> {
        read(&self.rules).clone()
    }

    pub fn rule(&self, id: &str) -> Option<Rule> {
        read(&self.rules).iter().find(|rule| rule.id == id).cloned()
    }

    /// 新增或覆盖一条规则。空 `id` 表示新增。
    pub fn save_rule(&self, mut rule: Rule) -> Result<Rule> {
        rule.name = rule.name.trim().to_string();
        if rule.name.is_empty() {
            bail!("缺少规则名称");
        }
        rule.gid = rule.gid.trim().to_string();
        rule.cid = rule.cid.trim().to_string();
        if rule.gid.is_empty() || rule.cid.is_empty() {
            bail!("请选择频道");
        }
        rule.action = match rule.action.trim() {
            "start" => "start".into(),
            "stop" => "stop".into(),
            "record" | "" => "record".into(),
            other => bail!("未知动作 {other}"),
        };
        normalize_condition(&mut rule.condition);
        if rule.condition.keywords.is_empty() && rule.condition.groups.is_empty() {
            bail!("匹配条件不能为空");
        }
        if rule.name_template.trim().is_empty() {
            rule.name_template = "{program.title} {datetime}".into();
        }
        let now = crate::users::model::now_ms();
        if rule.id.is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
            rule.created_at_ms = now;
        } else if rule.created_at_ms == 0 {
            // 面板更新可能不带创建时间。保留库里原值。
            if let Some(existing) = self.rule(&rule.id) {
                rule.created_at_ms = existing.created_at_ms;
            }
        }
        rule.updated_at_ms = now;
        let order = self
            .order_of(RULES, &rule.id)?
            .unwrap_or(self.next_order(RULES)?);
        self.write_row(RULES, &rule.id, order, &rule, now)?;
        let mut list = write(&self.rules);
        match list.iter_mut().find(|item| item.id == rule.id) {
            Some(existing) => *existing = rule.clone(),
            None => list.push(rule.clone()),
        }
        Ok(rule)
    }

    /// 删除一条规则。
    pub fn delete_rule(&self, id: &str) -> Result<bool> {
        let exists = read(&self.rules).iter().any(|rule| rule.id == id);
        if !exists {
            return Ok(false);
        }
        {
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute("DELETE FROM epg_rules WHERE id = ?1", params![id])?;
        }
        write(&self.rules).retain(|rule| rule.id != id);
        Ok(true)
    }

    // ---- 迁移 ----

    /// 源表为空时，把配置文档里的旧源导入表。
    /// 把配置文档里的旧源导入表。返回成功导入的下标。
    ///
    /// 表里已有的源直接算成功。失败的源保留在配置里，下次启动再试。
    pub fn import_config_sources(
        &self,
        sources: &[crate::config::model::EpgSource],
    ) -> Result<Vec<usize>> {
        let mut imported = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            let id = if source.id.trim().is_empty() {
                legacy_source_id(source)
            } else {
                source.id.trim().to_string()
            };
            if self.source(&id).is_some() {
                imported.push(index);
                continue;
            }
            let urls = if source.url.trim().is_empty() {
                Vec::new()
            } else {
                vec![SourceUrl {
                    url: source.url.trim().to_string(),
                    priority: 0,
                    ..SourceUrl::default()
                }]
            };
            let converted = Source {
                id,
                name: source.name.clone(),
                enabled: source.enabled,
                kind: source.kind.clone(),
                urls,
                refresh_secs: cron_interval_secs(&source.cron).unwrap_or(0),
                priority: 0,
                script: source.script.clone(),
                output_files: source.output_files.clone(),
                timeout_secs: source.timeout_secs,
                tz_shift_mins: source.tz_shift_mins,
                cron: source.cron.clone(),
                extra: source.extra.clone(),
            };
            match self.save_source(converted) {
                Ok(_) => imported.push(index),
                Err(error) => {
                    tracing::warn!(source = %source.name, %error, "导入旧 EPG 源失败");
                }
            }
        }
        Ok(imported)
    }

    /// 绑定表为空时，把旧的 `mappings.json` 导入表。
    pub fn import_legacy_mappings(&self, cache_dir: &Path) -> Result<usize> {
        let path = cache_dir.join("epg").join("mappings.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(0);
        };
        let Ok(map) = serde_json::from_str::<HashMap<String, crate::epg::Mapping>>(&text) else {
            return Ok(0);
        };
        let mut count = 0usize;
        let mut failed = 0usize;
        for (key, mapping) in map {
            let Some((gid, cid)) = key.split_once('/') else {
                failed += 1;
                continue;
            };
            if self.binding(gid, cid).is_some() {
                continue;
            }
            let binding = Binding {
                gid: gid.to_string(),
                cid: cid.to_string(),
                source_id: mapping.source_id,
                raw_channel_id: mapping.source_channel_id,
                origin: "manual".into(),
                upstream_display_name: mapping.source_display_name,
                updated_at_ms: 0,
            };
            match self.set_binding(binding) {
                Ok(_) => count += 1,
                Err(error) => {
                    tracing::warn!(key = %key, %error, "导入旧 EPG 映射失败");
                    failed += 1;
                }
            }
        }
        if failed == 0 {
            // 改名留底。否则用户清空绑定后重启会再次导入旧映射。
            let _ = std::fs::rename(&path, path.with_extension("json.migrated"));
        }
        Ok(count)
    }

    /// 手工绑定对应的源名称，便于 API 展示。
    pub fn source_names(&self) -> HashMap<String, String> {
        read(&self.sources)
            .iter()
            .map(|source| (source.id.clone(), source.name.clone()))
            .collect()
    }

    /// 绑定键集合，供采集时判断手工绑定。
    pub fn manual_keys(&self) -> HashSet<String> {
        read(&self.bindings)
            .iter()
            .filter(|binding| binding.manual())
            .map(Binding::key)
            .collect()
    }

    /// 导出全部配置。
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            sources: self.sources(),
            bindings: self.bindings(),
            rules: self.rules(),
        }
    }

    /// 用备份内容替换全部配置。
    ///
    /// 先清空三张表，再逐条写入。写入失败时返回错误，已写入的记录保留。
    pub fn replace_all(&self, snapshot: Snapshot) -> Result<()> {
        {
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute("DELETE FROM epg_sources", [])?;
            conn.execute("DELETE FROM epg_bindings", [])?;
            conn.execute("DELETE FROM epg_rules", [])?;
        }
        *write(&self.sources) = Vec::new();
        *write(&self.bindings) = Vec::new();
        *write(&self.rules) = Vec::new();
        for source in snapshot.sources {
            self.save_source(source)?;
        }
        for binding in snapshot.bindings {
            if self.source(&binding.source_id).is_some() {
                let _ = self.set_binding(binding);
            }
        }
        for rule in snapshot.rules {
            self.save_rule(rule)?;
        }
        Ok(())
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
        next_order(&conn, table)
    }

    fn next_order_tx(&self, conn: &Connection, table: &str) -> Result<i64> {
        next_order(conn, table)
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

fn next_order(conn: &Connection, table: &str) -> Result<i64> {
    let value: Option<i64> = conn
        .query_row(&format!("SELECT MAX(sort_order) FROM {table}"), [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .unwrap_or(None);
    Ok(value.unwrap_or(0) + 1)
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
            Err(error) => tracing::warn!(table, %error, "跳过无法解析的 EPG 记录"),
        }
    }
    Ok(out)
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|error| error.into_inner())
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|error| error.into_inner())
}

/// 清空条件里的空白关键字与空组。
fn normalize_condition(condition: &mut Condition) {
    condition.op = match condition.op.trim() {
        "any" => "any".into(),
        "not" => "not".into(),
        _ => "all".into(),
    };
    condition.keywords = condition
        .keywords
        .iter()
        .map(|keyword| keyword.trim().to_string())
        .filter(|keyword| !keyword.is_empty())
        .collect();
    for group in &mut condition.groups {
        normalize_condition(group);
    }
    condition
        .groups
        .retain(|group| !group.keywords.is_empty() || !group.groups.is_empty());
}

/// 旧源没有 ID 时生成稳定 ID。重试导入不会产生重复行。
fn legacy_source_id(source: &crate::config::model::EpgSource) -> String {
    use sha2::{Digest, Sha256};
    let key = format!("{}|{}|{}", source.name, source.url, source.script);
    let digest = Sha256::digest(key.as_bytes());
    format!("legacy-{}", hex::encode(&digest[..8]))
}

/// 把旧源的 cron 换算成刷新秒数。只识别 `0 */N * * *`。
fn cron_interval_secs(cron: &str) -> Option<u64> {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }
    let hours = parts[1].strip_prefix("*/")?.parse::<u64>().ok()?;
    if parts[0] != "0" || parts[2..] != ["*", "*", "*"] {
        return None;
    }
    Some(hours.saturating_mul(3_600).max(60))
}

// 供 JSON 兼容：旧文档可能带 `updated_at_ms`，新结构不建模。

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("tv-buddy-epg-store-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = crate::db::connect(&dir.join("tv-buddy.db")).expect("打开库");
        (Store::attach(conn).expect("依附"), dir)
    }

    fn xml_source(name: &str, url: &str) -> Source {
        Source {
            name: name.into(),
            kind: "xml".into(),
            enabled: true,
            urls: vec![SourceUrl {
                url: url.into(),
                ..SourceUrl::default()
            }],
            ..Source::default()
        }
    }

    #[test]
    fn source_round_trips() {
        let (store, dir) = store("source");
        let saved = store
            .save_source(xml_source("主源", "https://example.com/epg.xml"))
            .expect("保存");
        assert!(!saved.id.is_empty());
        let loaded = store.source(&saved.id).expect("读到");
        assert_eq!(loaded.name, "主源");
        assert_eq!(loaded.urls[0].url, "https://example.com/epg.xml");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_empty_url_list() {
        let (store, dir) = store("empty-url");
        let mut source = xml_source("空", "");
        source.urls.clear();
        assert!(store.save_source(source).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_binding_wins_over_auto() {
        let (store, dir) = store("binding");
        let source = store
            .save_source(xml_source("源", "https://example.com/epg.xml"))
            .expect("保存源");
        store
            .set_binding(Binding {
                gid: "g".into(),
                cid: "c".into(),
                source_id: source.id.clone(),
                raw_channel_id: "manual".into(),
                origin: "manual".into(),
                ..Binding::default()
            })
            .expect("手工绑定");
        store
            .replace_auto_bindings(vec![Binding {
                gid: "g".into(),
                cid: "c".into(),
                source_id: source.id.clone(),
                raw_channel_id: "auto".into(),
                ..Binding::default()
            }])
            .expect("自动绑定");
        assert_eq!(
            store.binding("g", "c").expect("绑定").raw_channel_id,
            "manual"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_source_removes_bindings() {
        let (store, dir) = store("delete-source");
        let source = store
            .save_source(xml_source("源", "https://example.com/epg.xml"))
            .expect("保存源");
        store
            .set_binding(Binding {
                gid: "g".into(),
                cid: "c".into(),
                source_id: source.id.clone(),
                raw_channel_id: "1".into(),
                origin: "manual".into(),
                ..Binding::default()
            })
            .expect("绑定");
        store.delete_source(&source.id).expect("删除");
        assert!(store.binding("g", "c").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rule_normalizes_condition() {
        let (store, dir) = store("rule");
        let saved = store
            .save_rule(Rule {
                name: "利物浦".into(),
                gid: "g".into(),
                cid: "c".into(),
                condition: Condition {
                    op: "all".into(),
                    keywords: vec![" 直播 ".into(), "".into()],
                    groups: vec![Condition {
                        op: "any".into(),
                        keywords: vec!["利物浦".into()],
                        groups: Vec::new(),
                    }],
                },
                action: "record".into(),
                enabled: true,
                ..Rule::default()
            })
            .expect("保存规则");
        assert_eq!(saved.condition.keywords, vec!["直播"]);
        assert_eq!(saved.condition.groups.len(), 1);
        assert_eq!(saved.name_template, "{program.title} {datetime}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn import_keeps_failed_sources_out_of_the_result() {
        let (store, dir) = store("import");
        let valid = crate::config::model::EpgSource {
            id: "s1".into(),
            name: "有效".into(),
            kind: "xml".into(),
            url: "https://example.com/epg.xml".into(),
            ..crate::config::model::EpgSource::default()
        };
        let invalid = crate::config::model::EpgSource {
            id: "s2".into(),
            name: "无地址".into(),
            kind: "xml".into(),
            ..crate::config::model::EpgSource::default()
        };
        let imported = store
            .import_config_sources(&[valid, invalid])
            .expect("导入");
        assert_eq!(imported, vec![0], "失败的源不应从配置移除");
        assert!(store.source("s1").is_some());
        assert!(store.source("s2").is_none());

        // 第二次导入：已存在的源算成功，失败项仍保留。
        let valid = crate::config::model::EpgSource {
            id: "s1".into(),
            name: "有效".into(),
            kind: "xml".into(),
            url: "https://example.com/epg.xml".into(),
            ..crate::config::model::EpgSource::default()
        };
        let invalid = crate::config::model::EpgSource {
            id: "s2".into(),
            name: "无地址".into(),
            kind: "xml".into(),
            ..crate::config::model::EpgSource::default()
        };
        let imported = store
            .import_config_sources(&[valid, invalid])
            .expect("再导入");
        assert_eq!(imported, vec![0]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn updating_rule_keeps_created_at() {
        let (store, dir) = store("rule-created");
        let saved = store
            .save_rule(Rule {
                name: "规则".into(),
                gid: "g".into(),
                cid: "c".into(),
                condition: Condition {
                    keywords: vec!["新闻".into()],
                    ..Condition::default()
                },
                action: "record".into(),
                ..Rule::default()
            })
            .expect("保存");
        assert!(saved.created_at_ms > 0);

        let updated = store
            .save_rule(Rule {
                id: saved.id.clone(),
                created_at_ms: 0,
                name: "改名".into(),
                gid: "g".into(),
                cid: "c".into(),
                condition: Condition {
                    keywords: vec!["新闻".into()],
                    ..Condition::default()
                },
                action: "record".into(),
                ..Rule::default()
            })
            .expect("更新");
        assert_eq!(updated.created_at_ms, saved.created_at_ms);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cron_interval_is_parsed() {
        assert_eq!(cron_interval_secs("0 */4 * * *"), Some(14_400));
        assert_eq!(cron_interval_secs("5 */4 * * *"), None);
        assert_eq!(cron_interval_secs("0 4 * * *"), None);
    }
}
