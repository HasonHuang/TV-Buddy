//! 鉴权：面板会话、播放 Token、封禁、限流。
//!
//! M0 只实现面板会话。其余模块按 `docs/design/02-architecture.md` §4 补齐。

pub mod session;

pub use session::SessionStore;
