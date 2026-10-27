//! 构建脚本：保证前端产物目录存在。
//!
//! 前端未构建时，写入一个占位页。这样 `cargo build` 不会失败。

use std::{fs, path::Path};

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
    println!("cargo:rerun-if-changed=frontend/dist");
    println!("cargo:rerun-if-changed=build.rs");
}
