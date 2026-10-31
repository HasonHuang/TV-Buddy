//! 配置模型、校验与持久化入口。
//!
//! 存：SQLite 单行 JSON 文档，见 [`store`]。
//! 运行时只认数据库里的 JSON，不读写 TOML。

pub mod model;
pub mod store;

use anyhow::bail;

pub use model::Config;
pub use store::ConfigStore;

/// 基本校验。只拦会破坏运行的值。
pub fn validate(config: &Config) -> anyhow::Result<()> {
    if config.server.bind.trim().is_empty() {
        bail!("server.bind 不能为空");
    }
    let window = config.auth.slice_sign_window_secs;
    if !(crate::sign::WINDOW_MIN..=crate::sign::WINDOW_MAX).contains(&window) {
        bail!(
            "auth.slice_sign_window_secs 必须在 {}–{} 之间",
            crate::sign::WINDOW_MIN,
            crate::sign::WINDOW_MAX
        );
    }
    let secret = config.auth.slice_sign_secret.trim();
    if !secret.is_empty()
        && (secret.len() < 32
            || secret.len() > 128
            || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        bail!("auth.slice_sign_secret 必须是 32–128 位十六进制字符串");
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
