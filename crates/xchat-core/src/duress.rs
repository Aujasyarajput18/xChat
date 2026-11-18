use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

type HmacSha256 = Hmac<Sha256>;

const PIN_ITERATIONS: u32 = 25_000;
const STORAGE_KEY_ITERATIONS: u32 = 10_000;

/// Mode unlocked by user authentication.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum VaultMode {
    Real,
    Decoy,
    Invalid,
}

/// Duress and Decoy PIN authenticator with constant-time verification and PBKDF2-style key stretching.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct DuressVaultManager {
    salt: [u8; 16],
    primary_pin_hash: [u8; 32],
    decoy_pin_hash: [u8; 32],
}

impl DuressVaultManager {
    /// Create a new DuressVaultManager with a freshly generated random salt.
    pub fn new(primary_pin: &str, decoy_pin: &str) -> Self {
        assert_ne!(
            primary_pin, decoy_pin,
            "Primary and Decoy PIN must be different"
        );
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        Self::with_salt(primary_pin, decoy_pin, salt)
    }

    /// Create or restore a DuressVaultManager with an existing salt.
    pub fn with_salt(primary_pin: &str, decoy_pin: &str, salt: [u8; 16]) -> Self {
        assert_ne!(
            primary_pin, decoy_pin,
            "Primary and Decoy PIN must be different"
        );
        Self {
            salt,
            primary_pin_hash: Self::hash_pin(primary_pin, &salt),
            decoy_pin_hash: Self::hash_pin(decoy_pin, &salt),
        }
    }

    pub fn salt(&self) -> &[u8; 16] {
        &self.salt
    }

    fn hash_pin(pin: &str, salt: &[u8; 16]) -> [u8; 32] {
        let mut mac =
            HmacSha256::new_from_slice(pin.as_bytes()).expect("HMAC key length infallible");
        mac.update(b"xchat-pin-auth-v2");
        mac.update(salt);
        let mut block: [u8; 32] = mac.finalize().into_bytes().into();

        for _ in 0..PIN_ITERATIONS {
            let mut m =
                HmacSha256::new_from_slice(pin.as_bytes()).expect("HMAC key length infallible");
            m.update(&block);
            block = m.finalize().into_bytes().into();
        }
        block
    }

    /// Authenticate a user-provided PIN using constant-time comparison.
    pub fn authenticate(&self, input_pin: &str) -> VaultMode {
        let input_hash = Self::hash_pin(input_pin, &self.salt);
        if bool::from(input_hash.ct_eq(&self.primary_pin_hash)) {
            VaultMode::Real
        } else if bool::from(input_hash.ct_eq(&self.decoy_pin_hash)) {
            VaultMode::Decoy
        } else {
            VaultMode::Invalid
        }
    }

    /// Derive local storage encryption key for whichever mode was authenticated.
    pub fn derive_storage_key(&self, input_pin: &str, mode: VaultMode) -> Option<[u8; 32]> {
        if mode == VaultMode::Invalid {
            return None;
        }

        let mut mac =
            HmacSha256::new_from_slice(input_pin.as_bytes()).expect("HMAC key length infallible");
        mac.update(&self.salt);
        match mode {
            VaultMode::Real => mac.update(b"xchat-real-storage-key-v2"),
            VaultMode::Decoy => mac.update(b"xchat-decoy-storage-key-v2"),
            VaultMode::Invalid => unreachable!(),
        }
        let mut key: [u8; 32] = mac.finalize().into_bytes().into();

        for _ in 0..STORAGE_KEY_ITERATIONS {
            let mut m = HmacSha256::new_from_slice(input_pin.as_bytes())
                .expect("HMAC key length infallible");
            m.update(&key);
            key = m.finalize().into_bytes().into();
        }

        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duress_pin_authentication() {
        let vault = DuressVaultManager::new("123456", "999999");

        assert_eq!(vault.authenticate("123456"), VaultMode::Real);
        assert_eq!(vault.authenticate("999999"), VaultMode::Decoy);
        assert_eq!(vault.authenticate("000000"), VaultMode::Invalid);

        let real_key = vault.derive_storage_key("123456", VaultMode::Real).unwrap();
        let decoy_key = vault
            .derive_storage_key("999999", VaultMode::Decoy)
            .unwrap();
        assert_ne!(real_key, decoy_key);
    }
}
