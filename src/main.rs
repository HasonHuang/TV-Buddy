//! TV Buddy——自托管 IPTV 代理服务。
//!
//! 本文件只负责启动：解析参数、读配置、建状态、起服务、启热重载。
//! 模块划分见 `docs/design/02-architecture.md`，代码在库根 `src/lib.rs`。

use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use tv_buddy::{cli, config, db, epg, http, logging, state};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 子命令（导入旧配置、读写配置值、改口令）不启动服务。
    if let Some(code) = cli::run_if_subcommand()? {
        std::process::exit(code);
    }

    // 第一个参数是数据库路径。旧参数 `config.toml` 给出明确提示。
    let db_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tv-buddy.db".to_string())
        .into();
    if db_path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
    {
        anyhow::bail!(
            "配置已改为 SQLite。先运行 `tv-buddy import-config {}` 导入，再用数据库路径启动",
            db_path.display()
        );
    }

    let conn = db::connect(&db_path)?;
    let store = config::ConfigStore::new(conn.clone());
    let (cfg, fresh) = match store.load()? {
        Some(config) => (config, false),
        None => {
            // 库里没有配置，旁边却有旧配置文件：多半是升级后忘了导入。
            // 不静默生成默认配置，否则会用空分组启动，且用户库路径也找错。
            let legacy = db_path
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("config.toml");
            if legacy.is_file() {
                anyhow::bail!(
                    "检测到旧配置文件 {}。请先运行 `tv-buddy import-config {} --db {}`",
                    legacy.display(),
                    legacy.display(),
                    db_path.display()
                );
            }
            let config = config::Config::default();
            store.save(&config)?;
            (config, true)
        }
    };
    drop(store);
    drop(conn);

    let log_path = logging::log_path_for(&db_path);
    let logs = Arc::new(logging::RingBuffer::default());
    logging::init(&cfg.server.log_level, log_path.as_deref(), logs.clone());
    if fresh {
        tracing::warn!(
            path = %db_path.display(),
            "首次启动：已生成默认配置。请在面板「系统设置」里调整"
        );
    }

    let bind = cfg.server.bind.clone();
    let state = Arc::new(state::AppState::new(db_path, cfg, logs)?);
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
