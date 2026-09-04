use anyhow::{Result, anyhow};
use totp_lite::{Sha1, totp_custom};

const TOTP_DIGITS: u32 = 6;
const TOTP_STEP: u64 = 30;

/// 生成一个新的 TOTP secret（base32 编码）
pub fn generate_secret() -> String {
    let secret: [u8; 20] = rand::random();
    data_encoding::BASE32_NOPAD.encode(&secret)
}

/// 验证 TOTP 代码（允许前后一个时间窗口的误差）
pub fn verify_totp(secret: &str, code: &str) -> Result<bool> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    verify_totp_at(secret, code, now)
}

fn verify_totp_at(secret: &str, code: &str, now: u64) -> Result<bool> {
    let secret_bytes = data_encoding::BASE32_NOPAD
        .decode(secret.as_bytes())
        .map_err(|e| anyhow!("Invalid base32 secret: {}", e))?;

    // 检查当前时间窗口以及前后各一个窗口（总共 3 个窗口，容忍 ±30 秒）
    for offset in [-1i64, 0, 1] {
        let time = (now as i64 + offset * TOTP_STEP as i64) as u64;
        let expected = totp_custom::<Sha1>(TOTP_STEP, TOTP_DIGITS, &secret_bytes, time);
        if code == expected {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secret() {
        let secret = generate_secret();
        assert!(!secret.is_empty());
        // Base32 字符集
        assert!(
            secret
                .chars()
                .all(|c| "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".contains(c))
        );
    }

    #[test]
    fn test_verify_totp() {
        let secret = "JBSWY3DPEHPK3PXP";
        let secret_bytes = data_encoding::BASE32_NOPAD
            .decode(secret.as_bytes())
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let code = totp_custom::<Sha1>(TOTP_STEP, TOTP_DIGITS, &secret_bytes, now);
        assert!(verify_totp(secret, &code).unwrap());

        // 错误的代码
        assert!(!verify_totp(secret, "000000").unwrap());
    }

    #[test]
    fn matches_standard_rfc_6238_time_step() {
        // RFC 6238 SHA-1 vector at Unix time 59, truncated to six digits.
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert!(verify_totp_at(secret, "287082", 59).unwrap());
    }
}
