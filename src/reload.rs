//! 配置热重载（M4）。
//!
//! 只监视**配置文件本身**；文件变化后重读。解析失败只记日志、保留旧快照（`design/04` §2）。
//! 注意：不要监视整个目录——日志文件也在同目录，会不断触发事件。

use std::{path::PathBuf, sync::Arc, time::Duration};

use notify::{RecursiveMode, Watcher};

use crate::state::AppState;

/// 去抖窗口：合并同一批文件事件。
const DEBOUNCE: Duration = Duration::from_millis(500);

/// 启动热重载线程。失败只记日志（不影响启动）。
pub fn spawn(state: Arc<AppState>) {
    let path = state.config_path.clone();
    let dir = path
        .parent()
        .map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        })
        .unwrap_or_else(|| PathBuf::from("."));

    // 目标：优先监视文件本身；文件不存在时退回监视目录。
    let (target, recursive) = if path.exists() {
        (path.clone(), RecursiveMode::NonRecursive)
    } else {
        (dir, RecursiveMode::Recursive)
    };

    let (tx, rx) = std::sync::mpsc::channel();
    let watched = path.clone();
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        // 只看配置文件的事件，避免日志文件写盘时误触发。
        if event.paths.iter().any(|changed| same_file(changed, &watched)) {
            let _ = tx.send(());
        }
    });
    let mut watcher = match watcher {
        Ok(watcher) => watcher,
        Err(err) => {
            tracing::warn!(error = %err, "启动配置文件监视失败");
            return;
        }
    };
    if let Err(err) = watcher.watch(&target, recursive) {
        tracing::warn!(target = %target.display(), error = %err, "监视配置文件失败");
        return;
    }

    std::thread::spawn(move || {
        // watcher 移入线程，保证存活到进程结束。
        let _watcher = watcher;
        while rx.recv().is_ok() {
            // 去抖：丢弃紧随其后的事件。
            while rx.recv_timeout(DEBOUNCE).is_ok() {}
            reload(&state);
        }
    });
}

/// 重读配置并替换快照。
pub fn reload(state: &AppState) {
    match crate::config::load(&state.config_path) {
        Ok(config) => {
            state.replace(config);
            tracing::info!("配置已热重载");
        }
        Err(err) => tracing::error!(error = %err, "配置解析失败，保留旧配置"),
    }
}

/// 路径是否为同一文件。用规范化结果比较，处理临时文件与软链接。
fn same_file(candidate: &std::path::Path, watched: &std::path::Path) -> bool {
    if candidate == watched {
        return true;
    }
    match (candidate.canonicalize(), watched.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
