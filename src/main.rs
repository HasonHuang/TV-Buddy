//! IPTV Proxy——与目标 v0.8.2 兼容的重实现。
//!
//! 本文件负责启动：解析参数、读配置、建状态、起服务、启热重载。
//! 模块划分见 `docs/design/02-architecture.md`。

mod auth;
mod config;
mod http;
mod logging;
mod playlist;
mod proxy;
mod reload;
mod state;
mod stats;
mod watchdog;

use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// 日志文件名（`design/04` §5）。
const LOG_FILE: &str = "iptv-proxy.log";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 配置路径来自第一个参数；与目标一致（S-ENV-06）。
    let config_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string())
        .into();

    let cfg = config::load(&config_path)
        .with_context(|| format!("读取配置失败: {}", config_path.display()))?;
    let log_path = log_path_for(&config_path);
    let logs = Arc::new(logging::RingBuffer::default());
    init_tracing(&cfg.server.log_level, log_path.as_deref(), logs.clone());

    let bind = cfg.server.bind.clone();
    let state = Arc::new(state::AppState::new(config_path, cfg, logs));
    reload::spawn(state.clone());

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

/// 日志文件与配置同目录。
fn log_path_for(config_path: &std::path::Path) -> Option<PathBuf> {
    let dir = config_path.parent()?;
    if dir.as_os_str().is_empty() {
        Some(PathBuf::from(LOG_FILE))
    } else {
        Some(dir.join(LOG_FILE))
    }
}

/// 按 `[server].log_level` 初始化日志：标准输出 + 文件 + 环形缓冲。
fn init_tracing(level: &str, log_path: Option<&std::path::Path>, ring: Arc<logging::RingBuffer>) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("iptv_proxy={},tower_http={}", level, level)));
    let writer = logging::LogWriter::new(ring, log_path.map(|path| path.to_path_buf()));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(writer))
        .init();
}
