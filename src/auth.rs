//! 鉴权：面板会话、播放 Token、封禁、登录锁、播放会话限制。
//!
//! 已实现：面板会话（M0）、播放 Token（M1）、IP 统计与封禁（P1）、登录失败锁（P3）、
//! 播放会话上限与空闲超时（P2-1）。

pub mod ip_guard;
pub mod login_guard;
pub mod play_token;
pub mod rate_limit;
pub mod session;
pub mod session_guard;
pub mod unban;

pub use ip_guard::IpGuard;
pub use login_guard::LoginGuard;
pub use play_token::PlayTokenStore;
pub use rate_limit::{Decision, Limits, RateLimiter};
pub use session::SessionStore;
pub use session_guard::{whitelisted, SessionGuard, Verdict};
pub use unban::UnbanQuota;
