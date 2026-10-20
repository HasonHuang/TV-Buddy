//! IPTV Proxy 库根：与目标 v0.8.2 兼容的重实现。
//!
//! 模块划分见 [`docs/design/02-architecture.md`](../docs/design/02-architecture.md)。
//! 可执行入口在 `src/main.rs`，只负责启动。

pub mod auth;
pub mod config;
pub mod epg;
pub mod http;
pub mod logging;
pub mod playlist;
pub mod proxy;
pub mod reload;
pub mod state;
pub mod stats;
pub mod watchdog;
