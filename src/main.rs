//! IPTV Proxy——与目标 v0.8.2 兼容的重实现。
//!
//! 本文件负责启动：解析参数、读配置、建状态、起服务。
//! 模块划分见 `docs/design/02-architecture.md`。

mod auth;
mod config;
mod http;
mod state;

use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 配置路径来自第一个参数；与目标一致（S-ENV-06）。
    let config_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string())
        .into();

    let cfg = config::load(&config_path)
        .with_context(|| format!("读取配置失败: {}", config_path.display()))?;
    init_tracing(&cfg.server.log_level);

    let bind = cfg.server.bind.clone();
    let state = Arc::new(state::AppState::new(config_path, cfg));

    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("监听失败: {bind}"))?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %bind,
        "iptv-proxy 启动"
    );

    axum::serve(listener, app).await?;
    Ok(())
}

/// 按 `[server].log_level` 初始化日志。
fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("iptv_proxy={},tower_http={}", level, level)));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
