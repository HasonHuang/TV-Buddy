//! 配置加载、校验与保存。
//!
//! 读：严格 TOML。重复键或类型错都会报错（ADR-08）。
//! 写：全量规范化 + 原子替换（临时文件 → `rename`，ADR-06）。

pub mod model;

use std::path::Path;

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};

pub use model::Config;

/// 读取配置。缺省字段用默认值补齐。
pub fn load(path: &Path) -> anyhow::Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("无法读取 {}", path.display()))?;
    parse(&text)
}

/// 解析并校验配置文本。
pub fn parse(text: &str) -> anyhow::Result<Config> {
    let config: Config = toml::from_str(text).context("TOML 解析失败")?;
    validate(&config)?;
    Ok(config)
}

/// 保存配置。先写临时文件，再原子替换。
pub fn save(path: &Path, config: &Config) -> anyhow::Result<()> {
    validate(config)?;
    let text = toml::to_string_pretty(config).context("序列化配置失败")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("无法写入 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("无法替换 {}", path.display()))?;
    Ok(())
}

/// 基本校验。只拦会破坏运行的值。
pub fn validate(config: &Config) -> anyhow::Result<()> {
    if config.server.bind.trim().is_empty() {
        bail!("server.bind 不能为空");
    }
    if config.panel.enabled && config.panel.password_sha256.trim().is_empty() {
        bail!("panel.password_sha256 不能为空");
    }
    let mut seen = std::collections::HashSet::new();
    for group in &config.groups {
        if group.id.trim().is_empty() {
            bail!("分组 id 不能为空");
        }
        if !seen.insert(&group.id) {
            bail!("分组 id 重复: {}", group.id);
        }
        let mut chans = std::collections::HashSet::new();
        for channel in &group.channels {
            if !chans.insert(&channel.id) {
                bail!("频道 id 重复: {}/{}", group.id, channel.id);
            }
        }
    }
    Ok(())
}

/// 计算口令摘要。目标用 SHA-256 十六进制小写（S-AUTH-01）。
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}
