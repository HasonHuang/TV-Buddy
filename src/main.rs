//! TV Buddy——自托管 IPTV 代理服务。
//!
//! 本文件只负责启动：解析参数、读配置、建状态、起服务、启热重载。
//! 模块划分见 `docs/design/02-architecture.md`，代码在库根 `src/lib.rs`。

use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use tv_buddy::{config, epg, http, logging, reload, state};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 配置路径来自第一个参数；与目标一致（S-ENV-06）。
    let config_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string())
        .into();

    let cfg = config::load(&config_path)
        .with_context(|| format!("读取配置失败: {}", config_path.display()))?;
    let log_path = logging::log_path_for(&config_path);
    let logs = Arc::new(logging::RingBuffer::default());
    logging::init(&cfg.server.log_level, log_path.as_deref(), logs.clone());

    let bind = cfg.server.bind.clone();
    let state = Arc::new(state::AppState::new(config_path, cfg, logs)?);
    // 首次安装：建管理员，并把初始口令写进 `<cache>/admin_initial_credentials.txt`。
    match tv_buddy::users::ensure_initial_admin(&state.users) {
        Ok(Some((user, password))) => {
            let path = state.cache_dir.join("admin_initial_credentials.txt");
            if let Err(error) = tv_buddy::users::write_credentials_file(&path, &user, &password) {
                tracing::error!(%error, "写入初始凭据失败");
            }
            tracing::warn!(
                user = %user,
                path = %path.display(),
                "首次安装：已创建管理员账号，口令见该文件"
            );
        }
        Ok(None) => {}
        Err(error) => tracing::error!(%error, "初始化管理员失败"),
    }
    reload::spawn(state.clone());
    epg::spawn(state.clone());
    tv_buddy::share::spawn(state.clone());
    tv_buddy::schedule::spawn(state.clone());
    tv_buddy::cache_gc::spawn(state.clone());
    tv_buddy::timeshift::warm(&state);
    state.dash_timeshift.warm(&state);
    tv_buddy::health::spawn(state.clone());

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("监听失败: {bind}"))?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %bind,
        "tv-buddy 启动"
    );

    axum::serve(
        listener,
        http::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
