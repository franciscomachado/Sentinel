use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};

/// Encryption key for state at rest (AES-256-GCM).
///
/// Key material should come from the OS keyring via CredentialVault.
/// The nonce is prepended to the ciphertext.
pub struct StateEncryptor {
    key: LessSafeKey,
    rng: SystemRandom,
}

/// The nonce size for AES-256-GCM.
const NONCE_LEN: usize = 12;

impl StateEncryptor {
    /// Create an encryptor from a 32-byte key.
    pub fn new(key_bytes: &[u8; 32]) -> Self {
        let unbound = UnboundKey::new(&AES_256_GCM, key_bytes).expect("valid key");
        Self {
            key: LessSafeKey::new(unbound),
            rng: SystemRandom::new(),
        }
    }

    /// Generate a new random 256-bit key.
    pub fn generate_key() -> [u8; 32] {
        let rng = SystemRandom::new();
        let mut key = [0u8; 32];
        rng.fill(&mut key).expect("RNG failure");
        key
    }

    /// Encrypt plaintext. Returns nonce || ciphertext || tag.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce_bytes)
            .map_err(|_| EncryptionError::RngFailed)?;

        let nonce = Nonce::assume_unique_for_key(nonce_bytes);

        let mut in_out = plaintext.to_vec();
        self.key
            .seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| EncryptionError::EncryptFailed)?;

        // Prepend nonce
        let mut result = Vec::with_capacity(NONCE_LEN + in_out.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&in_out);
        Ok(result)
    }

    /// Decrypt ciphertext produced by `encrypt`. Input is nonce || ciphertext || tag.
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        if data.len() < NONCE_LEN + aead::AES_256_GCM.tag_len() {
            return Err(EncryptionError::TooShort);
        }

        let (nonce_bytes, ciphertext_and_tag) = data.split_at(NONCE_LEN);
        let nonce = Nonce::assume_unique_for_key(
            nonce_bytes.try_into().map_err(|_| EncryptionError::TooShort)?,
        );

        let mut in_out = ciphertext_and_tag.to_vec();
        let plaintext = self
            .key
            .open_in_place(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| EncryptionError::DecryptFailed)?;

        Ok(plaintext.to_vec())
    }

    /// Encrypt plaintext and return as base64 string.
    pub fn encrypt_to_base64(&self, plaintext: &[u8]) -> Result<String, EncryptionError> {
        use base64::Engine;
        let encrypted = self.encrypt(plaintext)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(encrypted))
    }

    /// Decrypt a base64-encoded ciphertext.
    pub fn decrypt_from_base64(&self, b64: &str) -> Result<Vec<u8>, EncryptionError> {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| EncryptionError::InvalidBase64)?;
        self.decrypt(&data)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EncryptionError {
    #[error("random number generation failed")]
    RngFailed,
    #[error("encryption failed")]
    EncryptFailed,
    #[error("decryption failed (wrong key or corrupted data)")]
    DecryptFailed,
    #[error("ciphertext too short")]
    TooShort,
    #[error("invalid base64")]
    InvalidBase64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = StateEncryptor::generate_key();
        let enc = StateEncryptor::new(&key);

        let plaintext = b"sensitive memory data: kids don't like fish stew";
        let ciphertext = enc.encrypt(plaintext).unwrap();
        let decrypted = enc.decrypt(&ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key1 = StateEncryptor::generate_key();
        let key2 = StateEncryptor::generate_key();
        let enc1 = StateEncryptor::new(&key1);
        let enc2 = StateEncryptor::new(&key2);

        let ciphertext = enc1.encrypt(b"secret").unwrap();
        assert!(enc2.decrypt(&ciphertext).is_err());
    }

    #[test]
    fn base64_roundtrip() {
        let key = StateEncryptor::generate_key();
        let enc = StateEncryptor::new(&key);

        let plaintext = b"test data for base64 encoding";
        let encoded = enc.encrypt_to_base64(plaintext).unwrap();
        let decrypted = enc.decrypt_from_base64(&encoded).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn ciphertext_differs_each_time() {
        let key = StateEncryptor::generate_key();
        let enc = StateEncryptor::new(&key);

        let plaintext = b"same input";
        let c1 = enc.encrypt(plaintext).unwrap();
        let c2 = enc.encrypt(plaintext).unwrap();

        // Different nonces → different ciphertext
        assert_ne!(c1, c2);
        // But both decrypt to the same plaintext
        assert_eq!(enc.decrypt(&c1).unwrap(), plaintext);
        assert_eq!(enc.decrypt(&c2).unwrap(), plaintext);
    }

    #[test]
    fn too_short_fails() {
        let key = StateEncryptor::generate_key();
        let enc = StateEncryptor::new(&key);

        assert!(enc.decrypt(&[0u8; 5]).is_err());
    }
}
