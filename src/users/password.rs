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
