//! AES-256-GCM token encryption with a machine-derived key.
//!
//! Ciphertext format stored in JSON: `enc:v1:<base64(nonce || ciphertext)>`

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::process::Command;
use std::sync::OnceLock;

const PREFIX: &str = "enc:v1:";
const APP_SALT: &[u8] = b"codex-provider-hub/cursor-v1";

fn machine_fingerprint() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            if let Ok(output) = Command::new("ioreg")
                .args(["-rd1", "-c", "IOPlatformExpertDevice"])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    if line.contains("IOPlatformUUID") {
                        if let Some(start) = line.find('"') {
                            let rest = &line[start + 1..];
                            if let Some(end) = rest.find('"') {
                                return rest[..end].to_string();
                            }
                        }
                    }
                }
            }
            format!(
                "{}|{}",
                whoami_fallback(),
                std::env::var("HOME").unwrap_or_default()
            )
        })
        .clone()
}

fn whoami_fallback() -> String {
    Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown-host".into())
}

fn cipher() -> Aes256Gcm {
    let mut hasher = Sha256::new();
    hasher.update(APP_SALT);
    hasher.update(machine_fingerprint().as_bytes());
    let key = hasher.finalize();
    Aes256Gcm::new_from_slice(&key).expect("aes key length")
}

pub fn encrypt_secret(plaintext: &str) -> Result<String, String> {
    let cipher = cipher();
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| format!("encrypt failed: {e}"))?;
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{PREFIX}{}", B64.encode(blob)))
}

pub fn decrypt_secret(stored: &str) -> Result<String, String> {
    if !stored.starts_with(PREFIX) {
        // Backward-compat: treat legacy plaintext as-is.
        return Ok(stored.to_string());
    }
    let raw = B64
        .decode(stored[PREFIX.len()..].as_bytes())
        .map_err(|e| format!("decrypt decode: {e}"))?;
    if raw.len() < 13 {
        return Err("decrypt: ciphertext too short".into());
    }
    let (nonce_bytes, ct) = raw.split_at(12);
    let cipher = cipher();
    let nonce = Nonce::from_slice(nonce_bytes);
    let plain = cipher
        .decrypt(nonce, ct)
        .map_err(|_| "decrypt failed (wrong machine key or corrupt store)".to_string())?;
    String::from_utf8(plain).map_err(|e| format!("decrypt utf8: {e}"))
}

pub fn is_encrypted(stored: &str) -> bool {
    stored.starts_with(PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let enc = encrypt_secret("hello-token").unwrap();
        assert!(is_encrypted(&enc));
        assert_eq!(decrypt_secret(&enc).unwrap(), "hello-token");
    }
}
