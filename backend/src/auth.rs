use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use pbkdf2::pbkdf2_hmac;
use sha2::{Digest, Sha256};

pub const CLIENT_PASSWORD_ROUNDS: u32 = 600_000;

pub fn hash_password(password_derived: &str) -> anyhow::Result<String> {
    Ok(Argon2::default()
        .hash_password(password_derived.as_bytes())
        .map_err(|error| anyhow::anyhow!(error.to_string()))?
        .to_string())
}

pub fn verify_password(password_derived: &str, hash: &str) -> bool {
    PasswordHash::new(hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(password_derived.as_bytes(), &parsed)
            .is_ok()
    })
}

pub fn derive_client_password(password: &str, deployment_salt: &str) -> String {
    let mut digest = [0_u8; 32];
    pbkdf2_hmac::<Sha256>(
        password.as_bytes(),
        format!("nodeflare:{deployment_salt}").as_bytes(),
        CLIENT_PASSWORD_ROUNDS,
        &mut digest,
    );
    hex::encode(digest)
}

pub fn random_token(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::fill(&mut value);
    hex::encode(value)
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn valid_password_derived(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_verifies_derived_passwords() {
        let derived = derive_client_password("correct horse battery staple", "deployment-salt");
        let hash = hash_password(&derived).unwrap();
        assert!(verify_password(&derived, &hash));
        assert!(!verify_password(
            &derive_client_password("wrong", "deployment-salt"),
            &hash
        ));
    }

    #[test]
    fn creates_random_tokens() {
        let left = random_token(32);
        let right = random_token(32);
        assert_eq!(left.len(), 64);
        assert_ne!(left, right);
        assert_eq!(token_hash(&left).len(), 64);
    }
}
