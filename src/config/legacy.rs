//! 旧 TOML 配置的解析。仅用于迁移。
//!
//! 运行时不读写 TOML。本模块只有两个用途：
//! - `tv-buddy import-config` 把旧 `config.toml` 导入数据库。
//! - 恢复旧版本生成的备份包（包内是 `config.toml`）。
//!
//! 不要在这里加新的运行时逻辑。

use anyhow::{Context, Result};

use super::model::Config;

/// 解析并校验旧配置文本。
pub fn parse(text: &str) -> Result<Config> {
    let config: Config = toml::from_str(text).context("TOML 解析失败")?;
    super::validate(&config)?;
    Ok(config)
}

/// 读取并解析旧配置文件。
pub fn load(path: &std::path::Path) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("无法读取 {}", path.display()))?;
    parse(&text)
}

/// 把配置导出成 TOML 文本。仅用于对比测试与排障。
pub fn to_toml(config: &Config) -> Result<String> {
    toml::to_string_pretty(config).context("序列化配置失败")
}
