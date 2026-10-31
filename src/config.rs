//! 配置模型、校验与持久化入口。
//!
//! 存：SQLite 单行 JSON 文档，见 [`store`]。
//! 运行时不再读写 TOML。旧 TOML 的解析只在 [`legacy`] 里，供迁移使用。

pub mod legacy;
pub mod model;
pub mod store;

use anyhow::bail;
use sha2::{Digest, Sha256};

pub use model::Config;
pub use store::ConfigStore;

/// 基本校验。只拦会破坏运行的值。
pub fn validate(config: &Config) -> anyhow::Result<()> {
    if config.server.bind.trim().is_empty() {
        bail!("server.bind 不能为空");
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
