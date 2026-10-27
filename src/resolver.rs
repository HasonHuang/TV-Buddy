//! 上游解析脚本：`python3 <cache>/resolvers/<script> <arg>`，stdout 即上游 URL。
//!
//! 证据：F-009 §5（S-RES-01）。
//! 频道字段从 `[groups.channels].extra` 读取：
//! `resolver_script`、`resolver_arg`、`resolver_ttl`（秒，默认 30）。
//!
//! 缓存为 SWR：TTL 内直接返回；过期但仍有旧值时先返回旧值，并在后台刷新。
//! 冷启动失败或超时（30 秒）返回错误，由调用方转向 `error_url`。

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use crate::{config::model::Channel, state::AppState};

/// 执行超时。
const TIMEOUT: Duration = Duration::from_secs(30);

/// 解析结果缓存项。
#[derive(Debug, Clone)]
struct Entry {
    url: String,
    at: Instant,
    ttl: u64,
}

/// 频道级脚本设置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub script: String,
    pub arg: String,
    pub ttl: u64,
}

impl Setting {
    /// 从频道读取设置；未配置脚本时返回 `None`。
    pub fn from_channel(channel: &Channel) -> Option<Self> {
        let script = channel
            .extra
            .get("resolver_script")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())?
            .to_string();
        let arg = channel
            .extra
            .get("resolver_arg")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let ttl = channel
            .extra
            .get("resolver_ttl")
            .and_then(|value| value.as_integer())
            .filter(|value| *value >= 0)
            .unwrap_or(30) as u64;
        Some(Self { script, arg, ttl })
    }
}

/// 缓存。
#[derive(Debug, Default)]
pub struct Cache {
    entries: Mutex<HashMap<String, Entry>>,
}

impl Cache {
    fn get(&self, key: &str) -> Option<Entry> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(key)
            .cloned()
    }

    fn put(&self, key: &str, url: &str, ttl: u64) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                key.to_string(),
                Entry {
                    url: url.to_string(),
                    at: Instant::now(),
                    ttl,
                },
            );
    }
}

impl Entry {
    fn fresh(&self) -> bool {
        self.at.elapsed() < Duration::from_secs(self.ttl)
    }
}

/// 解析频道上游地址。
///
/// 未配置脚本 → `Ok(None)`，调用方沿用 `channel.url`。
pub async fn resolve(
    state: &Arc<AppState>,
    gid: &str,
    cid: &str,
    channel: &Channel,
) -> Result<Option<String>> {
    let Some(setting) = Setting::from_channel(channel) else {
        return Ok(None);
    };
    let key = format!("{gid}/{cid}");
    if let Some(entry) = state.resolver.get(&key) {
        if entry.fresh() || entry.url.is_empty() {
            return Ok(Some(entry.url));
        }
        // 过期：先返回旧值，后台刷新。
        let state = state.clone();
        let gid = gid.to_string();
        let cid = cid.to_string();
        let setting = setting.clone();
        tokio::spawn(async move {
            if let Ok(url) = run(&state, &gid, &cid, &setting).await {
                state.resolver.put(&key_for(&gid, &cid), &url, setting.ttl);
            }
        });
        return Ok(Some(entry.url));
    }
    let url = run(state, gid, cid, &setting).await?;
    state.resolver.put(&key, &url, setting.ttl);
    Ok(Some(url))
}

fn key_for(gid: &str, cid: &str) -> String {
    format!("{gid}/{cid}")
}

async fn run(state: &AppState, gid: &str, cid: &str, setting: &Setting) -> Result<String> {
    let script = script_path(&state.snapshot().cache.disk_path, &setting.script)?;
    let mut command = tokio::process::Command::new("python3");
    command
        .arg(&script)
        .arg(&setting.arg)
        .current_dir(script.parent().unwrap_or_else(|| std::path::Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(TIMEOUT, command.output())
        .await
        .with_context(|| format!("解析脚本超时（{}s）", TIMEOUT.as_secs()))?
        .with_context(|| format!("执行 python3 失败: {}", script.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "解析脚本退出码 {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim().chars().take(200).collect::<String>()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let url = stdout
        .trim()
        .lines()
        .last()
        .unwrap_or("")
        .trim()
        .to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("解析脚本输出不是 HTTP(S) 地址（频道 {gid}/{cid}）");
    }
    Ok(url)
}

fn script_path(cache_dir: &str, name: &str) -> Result<PathBuf> {
    let name = name.trim().trim_end_matches(".py");
    let safe = crate::logo::sanitize(name).context("脚本名不合法")?;
    let path = std::path::Path::new(cache_dir)
        .join("resolvers")
        .join(format!("{safe}.py"));
    if !path.is_file() {
        bail!("脚本不存在: {}", path.display());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(extra: toml::Table) -> Channel {
        Channel {
            id: "c1".into(),
            name: "频道".into(),
            url: "http://placeholder.test/never".into(),
            extra,
            ..Channel::default()
        }
    }

    #[test]
    fn reads_setting_from_extra() {
        let mut extra = toml::Table::new();
        extra.insert(
            "resolver_script".into(),
            toml::Value::String("probe.py".into()),
        );
        extra.insert("resolver_arg".into(), toml::Value::String("abc".into()));
        extra.insert("resolver_ttl".into(), toml::Value::Integer(5));
        let setting = Setting::from_channel(&channel(extra)).expect("设置");
        assert_eq!(setting.script, "probe.py");
        assert_eq!(setting.arg, "abc");
        assert_eq!(setting.ttl, 5);
        assert!(Setting::from_channel(&channel(toml::Table::new())).is_none());
    }

    #[test]
    fn defaults_ttl_and_rejects_bad_name() {
        let mut extra = toml::Table::new();
        extra.insert(
            "resolver_script".into(),
            toml::Value::String("probe".into()),
        );
        assert_eq!(
            Setting::from_channel(&channel(extra)).expect("设置").ttl,
            30
        );
        let dir = std::env::temp_dir().join(format!("resolver-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("resolvers")).expect("建目录");
        std::fs::write(dir.join("resolvers/ok.py"), "print('x')").expect("写");
        assert!(script_path(&dir.to_string_lossy(), "ok.py").is_ok());
        assert!(script_path(&dir.to_string_lossy(), "..").is_err());
        // 目录段被剥离，仍落在 resolvers 下。
        assert_eq!(
            script_path(&dir.to_string_lossy(), "../resolvers/ok").expect("路径"),
            dir.join("resolvers/ok.py")
        );
        assert!(script_path(&dir.to_string_lossy(), "missing").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_freshness() {
        let cache = Cache::default();
        cache.put("g/c", "http://a", 60);
        assert!(cache.get("g/c").expect("命中").fresh());
        cache.put("g/c", "http://a", 0);
        assert!(!cache.get("g/c").expect("命中").fresh());
    }
}
