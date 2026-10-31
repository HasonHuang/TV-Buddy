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

    // 第一个参数是数据库路径。
    let db_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tv-buddy.db".to_string())
        .into();
    // 用绝对路径。初始口令文件放在数据库所在目录，路径不能随工作目录变。
    let db_path = std::path::absolute(&db_path).unwrap_or(db_path);

    let conn = db::connect(&db_path)?;
    let store = config::ConfigStore::new(conn.clone());
    let (cfg, fresh) = match store.load()? {
        Some(config) => (config, false),
        None => {
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
    // 首次安装：建管理员，并把初始口令写进数据库所在目录的 `cache/` 下。
    //
    // 路径固定跟随数据库文件，不跟 `cache.disk_path`、也不跟工作目录。
    // `cache.disk_path` 默认是相对路径 `./cache`，用它会让文件位置随启动方式漂移。
    match tv_buddy::users::ensure_initial_admin(&state.users) {
        Ok(Some((user, password))) => {
            let path = credentials_path(&state.db_path);
            if let Err(error) = tv_buddy::users::write_credentials_file(&path, &user, &password) {
                tracing::error!(path = %path.display(), %error, "写入初始凭据失败");
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
    // 启动时清掉过期令牌，避免表越积越大。
    match state.users.purge_expired() {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "已清理过期访问令牌"),
        Err(error) => tracing::warn!(%error, "清理过期令牌失败"),
    }
    // `last_seen_at_ms` 只在内存累计，定期落盘。
    spawn_token_flush(state.clone());

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("监听失败: {bind}"))?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %bind,
        cache = %std::path::absolute(&state.cache_dir)
            .unwrap_or_else(|_| state.cache_dir.clone())
            .display(),
        "tv-buddy 启动"
    );

    axum::serve(
        listener,
        http::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// 初始口令文件路径：数据库所在目录下的 `cache/admin_initial_credentials.txt`。
///
/// 路径固定，与启动方式、工作目录、`cache.disk_path` 都无关。
/// 三个启动入口都把数据库路径传成绝对路径，因此默认安装下结果是
/// `/opt/tv-buddy/cache/admin_initial_credentials.txt`。
fn credentials_path(db_path: &std::path::Path) -> PathBuf {
    let parent = db_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    parent.join("cache").join("admin_initial_credentials.txt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_path_sits_under_database_directory() {
        let path = credentials_path(std::path::Path::new("/opt/tv-buddy/tv-buddy.db"));
        assert_eq!(
            path,
            PathBuf::from("/opt/tv-buddy/cache/admin_initial_credentials.txt")
        );
    }

    #[test]
    fn credentials_path_does_not_follow_cache_disk_path() {
        // 输入只有数据库路径，`cache.disk_path` 不参与。
        let one = credentials_path(std::path::Path::new("/srv/tv-buddy/tv-buddy.db"));
        let two = credentials_path(std::path::Path::new("/srv/tv-buddy/other.db"));
        assert_eq!(one.parent(), two.parent());
    }

    #[test]
    fn credentials_path_handles_bare_database_name() {
        let path = credentials_path(std::path::Path::new("tv-buddy.db"));
        assert_eq!(path, PathBuf::from("./cache/admin_initial_credentials.txt"));
    }
}

/// 每 60 秒把 `last_seen_at_ms` 的改动落盘。
///
/// 令牌每次被使用都只改内存并置脏位；没有这个循环，「最近使用」永远是空。
fn spawn_token_flush(state: std::sync::Arc<state::AppState>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = state.users.flush_if_dirty() {
                tracing::warn!(%error, "写入令牌使用时间失败");
            }
        }
    });
}
