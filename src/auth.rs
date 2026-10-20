//! 鉴权：面板会话、播放 Token、封禁、限流。
//!
//! 已实现：面板会话（M0）、播放 Token（M1）、IP 统计与封禁（P1）。
//! 待实现：按 IP 的会话上限与自动限流（P2）。

pub mod ip_guard;
pub mod play_token;
pub mod session;

pub use ip_guard::IpGuard;
pub use play_token::PlayTokenStore;
pub use session::SessionStore;
