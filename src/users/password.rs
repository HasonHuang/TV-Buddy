//! 口令散列：argon2id。
//!
//! 与 mpd-hls 同为 argon2id（`docs/design/09-users-and-devices.md` §2.1）。
//! 参数用库默认值，输出标准 PHC 文本格式。

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

/// 生成口令散列。结果形如 `$argon2id$v=19$m=...$<salt>$<hash>`。
pub fn hash(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|value| value.to_string())
        .map_err(|error| anyhow::anyhow!("口令散列失败: {error}"))
}

/// 校验口令。散列文本损坏时按不匹配处理。
pub fn verify(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// 校验结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 口令正确，散列格式已是当前算法。
    Ok,
    /// 口令正确，但散列是旧的 `sha256:` 格式，应重算。
    OkNeedsRehash,
    /// 口令错误。
    Fail,
}

/// 校验口令，兼容旧 `sha256:<hex>` 格式。
///
/// 旧格式来自 `[panel].password_sha256` 的一次性迁移。
/// 校验通过后调用方应重算 argon2 并保存。
pub fn verify_any(password: &str, stored: &str) -> Verdict {
    if let Some(hex) = stored.strip_prefix("sha256:") {
        return if crate::config::sha256_hex(password) == hex.trim() {
            Verdict::OkNeedsRehash
        } else {
            Verdict::Fail
        };
    }
    if verify(password, stored) {
        Verdict::Ok
    } else {
        Verdict::Fail
    }
}

/// 生成 16 位随机口令，用于首次安装。
///
/// 字符集去掉易混淆的 `0O1lI`。
pub fn random_password(len: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// 生成 12 位访问令牌。字母数字混合，与目标同形。
pub fn random_token() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..12)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_round_trip() {
        let stored = hash("admin").expect("散列");
        assert!(stored.starts_with("$argon2id$"));
        assert!(verify("admin", &stored));
        assert!(!verify("wrong", &stored));
    }

    #[test]
    fn legacy_sha256_hashes_are_accepted_once() {
        let legacy = format!("sha256:{}", crate::config::sha256_hex("admin"));
        assert_eq!(verify_any("admin", &legacy), Verdict::OkNeedsRehash);
        assert_eq!(verify_any("nope", &legacy), Verdict::Fail);
        let argon = hash("admin").expect("散列");
        assert_eq!(verify_any("admin", &argon), Verdict::Ok);
    }

    #[test]
    fn broken_hash_does_not_panic() {
        assert!(!verify("admin", "not-a-hash"));
        assert!(!verify("admin", ""));
    }

    #[test]
    fn random_tokens_have_expected_shape() {
        let token = random_token();
        assert_eq!(token.len(), 12);
        assert!(token.chars().all(|ch| ch.is_ascii_alphanumeric()));
        assert_ne!(token, random_token());
        assert_eq!(random_password(16).len(), 16);
    }
}
