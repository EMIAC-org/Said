//! BYOK provider-credential encryption (Wave 1).
//!
//! AES-256-GCM at rest; plaintext exists only in memory for the provider call.
//! The master key comes from `RUNTIME_SECRET_KEY` (env). No plaintext secret is
//! ever stored or logged; APIs return only status + a SHA-256 digest.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use sha2::{Digest, Sha256};

/// Derive a 32-byte master key from the configured secret. Accepts a 64-char
/// hex key, a base64-encoded 32-byte key, or any passphrase (SHA-256 → 32 bytes).
/// Empty input ⇒ empty key ⇒ encryption disabled (BYOK off, server keys only).
pub fn parse_master_key(raw: &str) -> Vec<u8> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    if raw.len() == 64 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Some(bytes) = (0..32)
            .map(|i| u8::from_str_radix(&raw[i * 2..i * 2 + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()
        {
            return bytes;
        }
    }
    if let Ok(b) = B64.decode(raw) {
        if b.len() == 32 {
            return b;
        }
    }
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    h.finalize().to_vec()
}

fn cipher(master_key: &[u8]) -> Option<Aes256Gcm> {
    if master_key.len() != 32 {
        return None;
    }
    Some(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(master_key)))
}

/// Encrypt → `base64(nonce(12) || ciphertext+tag)`. `None` if no master key.
pub fn encrypt(plaintext: &str, master_key: &[u8]) -> Option<String> {
    let c = cipher(master_key)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = c.encrypt(&nonce, plaintext.as_bytes()).ok()?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ct);
    Some(B64.encode(blob))
}

pub fn decrypt(blob_b64: &str, master_key: &[u8]) -> Option<String> {
    let c = cipher(master_key)?;
    let blob = B64.decode(blob_b64).ok()?;
    if blob.len() < 12 {
        return None;
    }
    let (nonce_bytes, ct) = blob.split_at(12);
    let pt = c.decrypt(Nonce::from_slice(nonce_bytes), ct).ok()?;
    String::from_utf8(pt).ok()
}

/// SHA-256 digest (hex) — identify/dedupe a secret without storing plaintext.
pub fn digest(plaintext: &str) -> String {
    let mut h = Sha256::new();
    h.update(plaintext.as_bytes());
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = parse_master_key("test-master-passphrase");
        assert_eq!(key.len(), 32);
        let enc = encrypt("sk-secret-123", &key).unwrap();
        assert_ne!(enc, "sk-secret-123");
        assert_eq!(decrypt(&enc, &key).as_deref(), Some("sk-secret-123"));
    }

    #[test]
    fn wrong_key_fails() {
        let k1 = parse_master_key("key-one");
        let k2 = parse_master_key("key-two");
        let enc = encrypt("secret", &k1).unwrap();
        assert!(decrypt(&enc, &k2).is_none());
    }

    #[test]
    fn no_key_disables_encryption() {
        assert!(encrypt("x", &[]).is_none());
        assert!(parse_master_key("").is_empty());
    }
}
