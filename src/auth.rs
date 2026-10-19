//! 鉴权：面板会话、播放 Token、封禁、限流。
//!
//! 已实现：面板会话（M0）、播放 Token（M1）。
//! 待实现：封禁、限流（P1，见 `docs/design/02-architecture.md` §4）。

pub mod play_token;
pub mod session;

pub use play_token::PlayTokenStore;
pub use session::SessionStore;
