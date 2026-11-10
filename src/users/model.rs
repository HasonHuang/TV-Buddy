//! 用户与访问令牌的数据结构。
//!
//! 字段名与 mpd-hls 的 `struct User with 10 elements` 对齐
//! （`docs/design/09-users-and-devices.md` §2）。

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// 自定义设备凭证的最小长度。
pub const TOKEN_MIN_LEN: usize = 8;
/// 自定义设备凭证的最大长度。
pub const TOKEN_MAX_LEN: usize = 64;

/// 校验并归一化自定义设备凭证。
///
/// 凭证直接进入 URL 路径，所以只允许 ASCII 字母、数字、`_` 与 `-`。
pub fn normalize_device_token(raw: &str) -> Result<String> {
    let value = raw.trim();
    if value.len() < TOKEN_MIN_LEN || value.len() > TOKEN_MAX_LEN {
        bail!("凭证长度须为 {TOKEN_MIN_LEN}–{TOKEN_MAX_LEN} 个字符");
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        bail!("凭证只能包含字母、数字、下划线和连字符");
    }
    Ok(value.to_string())
}

/// 当前毫秒时间戳。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

/// 注册角色。管理员不受可见范围限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn is_admin(self) -> bool {
        matches!(self, Role::Admin)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
}

fn default_true() -> bool {
    true
}

/// 一个注册用户。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: Role,
    /// `argon2id` 文本格式。见 [`super::password`]。
    pub password_hash: String,
    /// 可见分组。空列表表示全部分组，包括以后新增的分组。
    #[serde(default)]
    pub allowed_group_ids: Vec<String>,
    /// 是否启用频道级过滤。
    #[serde(default)]
    pub channel_filter_enabled: bool,
    /// 可见频道。只在 [`User::channel_filter_enabled`] 为真时生效。
    #[serde(default)]
    pub allowed_channel_ids: Vec<String>,
    /// 能否登录面板。
    #[serde(default = "default_true")]
    pub web_ui_access: bool,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

impl User {
    /// 是否可见某个分组。
    ///
    /// 管理员、或 `allowed_group_ids` 为空的普通用户可见全部分组。
    pub fn can_see_group(&self, gid: &str) -> bool {
        self.role.is_admin()
            || self.allowed_group_ids.is_empty()
            || self.allowed_group_ids.iter().any(|id| id == gid)
    }

    /// 是否可见某个频道。分组可见是前置条件。
    pub fn can_see_channel(&self, gid: &str, cid: &str) -> bool {
        if !self.can_see_group(gid) {
            return false;
        }
        if self.role.is_admin() || !self.channel_filter_enabled {
            return true;
        }
        self.allowed_channel_ids
            .iter()
            .any(|id| id == &format!("{gid}/{cid}"))
    }
}

/// 令牌用途。目标是 `device` 与 `web_ui_links` 两类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// 用户自建，用于播放器订阅。
    Device,
    /// 面板生成，用于页面内的播放链接。
    WebUiLinks,
}

/// 一个访问令牌。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    /// 12 位字母数字。
    pub token: String,
    pub user_id: String,
    pub purpose: Purpose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub created_at_ms: i64,
    /// `None` 表示永不过期。
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub last_seen_at_ms: Option<i64>,
}

impl AccessToken {
    /// 按当前时间判断是否已过期。
    pub fn is_expired(&self, now: i64) -> bool {
        matches!(self.expires_at_ms, Some(at) if at <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(role: Role) -> User {
        User {
            id: "u1".into(),
            username: "u".into(),
            role,
            password_hash: String::new(),
            allowed_group_ids: vec!["g1".into()],
            channel_filter_enabled: false,
            allowed_channel_ids: Vec::new(),
            web_ui_access: true,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn admin_sees_everything() {
        let admin = user(Role::Admin);
        assert!(admin.can_see_group("anything"));
        assert!(admin.can_see_channel("g9", "c9"));
    }

    #[test]
    fn plain_user_is_limited_to_allowed_groups() {
        let plain = user(Role::User);
        assert!(plain.can_see_group("g1"));
        assert!(!plain.can_see_group("g2"));
        assert!(plain.can_see_channel("g1", "c1"));
        assert!(!plain.can_see_channel("g2", "c2"));
    }

    #[test]
    fn empty_allowed_groups_means_all() {
        let mut plain = user(Role::User);
        plain.allowed_group_ids.clear();
        assert!(plain.can_see_group("g1"));
        assert!(plain.can_see_group("new-group"));
        assert!(plain.can_see_channel("new-group", "c1"));
    }

    #[test]
    fn channel_filter_narrows_within_allowed_groups() {
        let mut plain = user(Role::User);
        plain.channel_filter_enabled = true;
        plain.allowed_channel_ids = vec!["g1/c2".into()];
        assert!(!plain.can_see_channel("g1", "c1"));
        assert!(plain.can_see_channel("g1", "c2"));
    }

    #[test]
    fn expired_token_is_detected() {
        let token = AccessToken {
            token: "abc".into(),
            user_id: "u1".into(),
            purpose: Purpose::Device,
            label: None,
            created_at_ms: 0,
            expires_at_ms: Some(100),
            last_seen_at_ms: None,
        };
        assert!(token.is_expired(100));
        assert!(!token.is_expired(99));
    }

    #[test]
    fn custom_token_accepts_safe_characters() {
        assert_eq!(
            normalize_device_token("  living-room_2  ").expect("合法"),
            "living-room_2"
        );
        assert_eq!(
            normalize_device_token("12345678").expect("合法"),
            "12345678"
        );
    }

    #[test]
    fn custom_token_rejects_bad_shape() {
        assert!(normalize_device_token("short").is_err());
        assert!(normalize_device_token(&"a".repeat(TOKEN_MAX_LEN + 1)).is_err());
        assert!(normalize_device_token("living room").is_err());
        assert!(normalize_device_token("翡翠台").is_err());
        assert!(normalize_device_token("a/b").is_err());
    }
}
