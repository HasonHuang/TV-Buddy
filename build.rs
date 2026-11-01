//! 构建脚本：保证前端产物目录存在，并确定版本号。
//!
//! 前端未构建时，写入一个占位页。这样 `cargo build` 不会失败。
//!
//! 版本号按以下顺序确定：
//!
//! 1. 环境变量 `TV_BUDDY_VERSION`（发布脚本从 git 标签传入）。
//! 2. `git describe` 的最近标签。
//! 3. `Cargo.toml` 的 `version`。
//!
//! 结果写入 `TV_BUDDY_VERSION`，源码用 `env!` 读取。

use std::{
    env, fs,
    path::Path,
    process::Command,
};

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>TV Buddy</title>
<style>body{font-family:system-ui,sans-serif;background:#111;color:#ddd;display:grid;place-items:center;height:100vh;margin:0}
code{background:#222;padding:2px 6px;border-radius:4px}</style></head>
<body><div><h1>前端未构建</h1><p>请运行 <code>scripts/build-frontend.sh</code>，然后重新编译。</p></div></body></html>
"#;

fn main() {
    let dir = Path::new("frontend/dist");
    if !dir.join("index.html").exists() {
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(dir.join("index.html"), PLACEHOLDER);
    }
    println!("cargo:rustc-env=TV_BUDDY_VERSION={}", version());
    println!("cargo:rerun-if-changed=frontend/dist");
    println!("cargo:rerun-if-changed=build.rs");
    // 切换分支或提交时重新取版本号。`.git` 不存在（源码包构建）时忽略。
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=TV_BUDDY_VERSION");
}

/// 版本号不含前导 `v`。面板自己加 `v` 前缀。
fn version() -> String {
    if let Some(value) = env::var("TV_BUDDY_VERSION").ok().and_then(|v| clean(&v)) {
        return value;
    }
    if let Some(value) = git_describe().and_then(|v| clean(&v)) {
        return value;
    }
    env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string())
}

/// 去掉空白和前导 `v`。空串返回 `None`。
fn clean(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('v').trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 取最近的 `v*` 标签。没有标签、没有 git、命令失败都返回 `None`。
fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--dirty", "--match", "v[0-9]*"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}
