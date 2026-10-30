//! 用户、角色与访问令牌。
//!
//! 功能对齐 mpd-hls 的 `/users`
//! （`docs/design/09-users-and-devices.md`）。

pub mod model;
pub mod password;
pub mod store;

pub use model::{AccessToken, Purpose, Role, User};
pub use store::{
    ensure_admin, ensure_initial_admin, write_credentials_file, UserStore, INITIAL_ADMIN,
};
