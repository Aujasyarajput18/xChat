use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use xchat_crypto::{
    aead::{decrypt_aead, encrypt_aead},
    identity::IdentityKeyPair,
    x3dh::{OneTimePrekeyPair, PrekeyBundle, SignedPrekeyPair},
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::errors::CoreError;

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct StoredKeyData {
    sign_seed: [u8; 32],
    dh_secret: [u8; 32],
    spk_secret: [u8; 32],
    spk_pubkey: [u8; 32],
    #[zeroize(skip)]
    spk_signature: Vec<u8>,
    otps: Vec<(u32, [u8; 32], [u8; 32])>,
}

/// In-memory and persistent cryptographic keystore.
pub struct KeyStore {
    pub identity: IdentityKeyPair,
    pub signed_prekey: SignedPrekeyPair,
    pub one_time_prekeys: Vec<OneTimePrekeyPair>,
    pub peer_bundles: HashMap<[u8; 32], PrekeyBundle>,
}

impl KeyStore {
    /// Generate a fresh keystore with new identity, signed prekey, and 10 one-time prekeys.
    pub fn generate() -> Self {
        let identity = IdentityKeyPair::generate();
        let signed_prekey = SignedPrekeyPair::generate(&identity);
        let mut one_time_prekeys = Vec::with_capacity(10);
        for id in 0..10 {
            one_time_prekeys.push(OneTimePrekeyPair::generate(id));
        }

        Self {
            identity,
            signed_prekey,
            one_time_prekeys,
            peer_bundles: HashMap::new(),
        }
    }

    /// Export public prekey bundle for publication or peer discovery.
    pub fn export_bundle(&mut self) -> PrekeyBundle {
        let opk = self.one_time_prekeys.last().map(|last| last.pubkey);

        PrekeyBundle {
            identity_key: self.identity.public_key(),
            signed_prekey: self.signed_prekey.pubkey,
            signed_prekey_signature: self.signed_prekey.signature,
            one_time_prekey: opk,
        }
    }

    /// Register a peer's public bundle.
    pub fn register_peer_bundle(&mut self, bundle: PrekeyBundle) {
        self.peer_bundles
            .insert(bundle.identity_key.sign_pubkey, bundle);
    }

    /// Consume a one-time prekey by public key, removing it from storage to enforce single-use forward secrecy.
    pub fn consume_one_time_prekey(&mut self, pubkey: &[u8; 32]) -> Option<OneTimePrekeyPair> {
        if let Some(pos) = self
            .one_time_prekeys
            .iter()
            .position(|opk| opk.pubkey == *pubkey)
        {
            Some(self.one_time_prekeys.remove(pos))
        } else {
            None
        }
    }

    /// Replenish one-time prekeys up to the target count if running low.
    pub fn replenish_one_time_prekeys(&mut self, target_count: usize) {
        let current = self.one_time_prekeys.len();
        if current < target_count {
            let max_id = self
                .one_time_prekeys
                .iter()
                .map(|k| k.id)
                .max()
                .unwrap_or(0);
            for i in 1..=(target_count - current) {
                self.one_time_prekeys
                    .push(OneTimePrekeyPair::generate(max_id + i as u32));
            }
        }
    }

    /// Rotate the signed prekey using the device identity key.
    pub fn rotate_signed_prekey(&mut self) {
        self.signed_prekey = SignedPrekeyPair::generate(&self.identity);
    }

    /// Save keystore to disk encrypted under a passphrase-derived key.
    /// A fresh random 12-byte nonce is generated for every save and prepended to the file.
    pub fn save_encrypted(&self, path: &Path, encryption_key: &[u8; 32]) -> Result<(), CoreError> {
        let (sign_seed, dh_secret) = self.identity.to_bytes();

        let otps = self
            .one_time_prekeys
            .iter()
            .map(|opk| (opk.id, opk.secret.to_bytes(), opk.pubkey))
            .collect();

        let stored = StoredKeyData {
            sign_seed,
            dh_secret,
            spk_secret: self.signed_prekey.secret.to_bytes(),
            spk_pubkey: self.signed_prekey.pubkey,
            spk_signature: self.signed_prekey.signature.to_vec(),
            otps,
        };

        let serialized =
            bincode::serialize(&stored).map_err(|e| CoreError::StorageError(e.to_string()))?;
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = encrypt_aead(encryption_key, &nonce, &serialized, b"xchat-keystore-v1")
            .map_err(CoreError::Crypto)?;

        let mut output = Vec::with_capacity(12 + ciphertext.len());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);

        Self::write_secure_file(path, &output)
    }

    /// Internal helper to write a file with 0600 permissions on Unix.
    pub fn write_secure_file(path: &Path, data: &[u8]) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| CoreError::StorageError(e.to_string()))?;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut opt = fs::OpenOptions::new();
            opt.write(true).create(true).truncate(true).mode(0o600);
            let mut file = opt
                .open(path)
                .map_err(|e| CoreError::StorageError(e.to_string()))?;
            file.write_all(data)
                .map_err(|e| CoreError::StorageError(e.to_string()))?;
        }
        #[cfg(not(unix))]
        {
            let mut file =
                File::create(path).map_err(|e| CoreError::StorageError(e.to_string()))?;
            file.write_all(data)
                .map_err(|e| CoreError::StorageError(e.to_string()))?;
        }
        Ok(())
    }

    /// Load encrypted keystore from disk.
    /// Extracts the 12-byte random nonce from the beginning of the file.
    pub fn load_encrypted(path: &Path, encryption_key: &[u8; 32]) -> Result<Self, CoreError> {
        let mut file = File::open(path).map_err(|e| CoreError::StorageError(e.to_string()))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if data.len() < 12 {
            return Err(CoreError::StorageError(
                "Keystore file corrupted or too short".into(),
            ));
        }

        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&data[..12]);
        let ciphertext = &data[12..];

        let decrypted = decrypt_aead(encryption_key, &nonce, ciphertext, b"xchat-keystore-v1")
            .map_err(CoreError::Crypto)?;

        let mut stored: StoredKeyData =
            bincode::deserialize(&decrypted).map_err(|e| CoreError::StorageError(e.to_string()))?;

        let identity = IdentityKeyPair::from_bytes(stored.sign_seed, stored.dh_secret);

        let mut sig_arr = [0u8; 64];
        if stored.spk_signature.len() == 64 {
            sig_arr.copy_from_slice(&stored.spk_signature);
        }
        let signed_prekey = SignedPrekeyPair {
            secret: x25519_dalek::StaticSecret::from(stored.spk_secret),
            pubkey: stored.spk_pubkey,
            signature: sig_arr,
        };

        let otps = std::mem::take(&mut stored.otps);
        let one_time_prekeys = otps
            .into_iter()
            .map(|(id, sec, pubk)| OneTimePrekeyPair {
                id,
                secret: x25519_dalek::StaticSecret::from(sec),
                pubkey: pubk,
            })
            .collect();

        Ok(Self {
            identity,
            signed_prekey,
            one_time_prekeys,
            peer_bundles: HashMap::new(),
        })
    }

    /// Legacy default device encryption key used for backwards-compatible migration of v1 keystores.
    pub const DEFAULT_STORAGE_KEY: [u8; 32] = [
        0x58, 0x43, 0x68, 0x61, 0x74, 0x2d, 0x44, 0x65, 0x76, 0x69, 0x63, 0x65, 0x2d, 0x4b, 0x65,
        0x79, 0x73, 0x74, 0x6f, 0x72, 0x65, 0x2d, 0x56, 0x61, 0x75, 0x6c, 0x74, 0x2d, 0x4b, 0x65,
        0x79, 0x21,
    ];

    /// Derive or retrieve a secure per-device encryption key.
    /// Priority:
    /// 1. `XCHAT_PASSPHRASE` environment variable (derived using HKDF-SHA256)
    /// 2. `XCHAT_STORAGE_KEY` environment variable (hex-encoded 32-byte key)
    /// 3. Secure random key saved in `path.with_file_name(".xchat_device_key")` or `~/.xchat_device_key` with 0600 mode
    pub fn get_or_create_device_storage_key_for_path(
        path: Option<&Path>,
    ) -> Result<[u8; 32], CoreError> {
        if let Ok(pass) = std::env::var("XCHAT_PASSPHRASE") {
            if !pass.trim().is_empty() {
                let hk = hkdf::Hkdf::<sha2::Sha256>::new(
                    Some(b"xchat-keystore-passphrase-salt-v1"),
                    pass.as_bytes(),
                );
                let mut key = [0u8; 32];
                hk.expand(b"xchat-device-storage-key", &mut key)
                    .map_err(|e| {
                        CoreError::Crypto(xchat_crypto::errors::CryptoError::AeadError(
                            e.to_string(),
                        ))
                    })?;
                return Ok(key);
            }
        }
        if let Ok(hex_key) = std::env::var("XCHAT_STORAGE_KEY") {
            if let Ok(bytes) = hex::decode(hex_key.trim()) {
                if bytes.len() == 32 {
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&bytes);
                    return Ok(key);
                }
            }
        }
        let key_path = if let Ok(custom) = std::env::var("XCHAT_DEVICE_KEY_PATH") {
            PathBuf::from(custom)
        } else if let Some(p) = path {
            p.with_file_name(".xchat_device_key")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".xchat_device_key")
        } else {
            PathBuf::from(".xchat_device_key")
        };
        if key_path.exists() {
            if let Ok(data) = fs::read(&key_path) {
                if data.len() == 32 {
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&data);
                    return Ok(key);
                }
            }
        }
        let mut key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
        let _ = Self::write_secure_file(&key_path, &key);
        Ok(key)
    }

    /// Default lookup using standard device key location.
    pub fn get_or_create_device_storage_key() -> Result<[u8; 32], CoreError> {
        Self::get_or_create_device_storage_key_for_path(None)
    }

    /// Save keystore to disk using the local device storage key (with 0600 permissions).
    pub fn save_to_path(&self, path: &Path) -> Result<(), CoreError> {
        let key = Self::get_or_create_device_storage_key_for_path(Some(path))?;
        self.save_encrypted(path, &key)
    }

    /// Load keystore from disk using the local device storage key.
    /// If loading with the device key fails, it automatically falls back to the legacy
    /// `DEFAULT_STORAGE_KEY` and re-encrypts with the device key (seamless migration).
    pub fn load_from_path(path: &Path) -> Result<Self, CoreError> {
        let device_key = Self::get_or_create_device_storage_key_for_path(Some(path))?;
        match Self::load_encrypted(path, &device_key) {
            Ok(ks) => Ok(ks),
            Err(_) => {
                // Migration path: attempt legacy key
                match Self::load_encrypted(path, &Self::DEFAULT_STORAGE_KEY) {
                    Ok(legacy_ks) => {
                        // Re-encrypt immediately under device key with 0600 mode
                        let _ = legacy_ks.save_encrypted(path, &device_key);
                        Ok(legacy_ks)
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// Load existing keystore or generate and persist a fresh one for communication.
    pub fn load_or_generate(path: &Path) -> Result<Self, CoreError> {
        if path.exists() {
            if let Ok(ks) = Self::load_from_path(path) {
                return Ok(ks);
            }
        }
        let ks = Self::generate();
        let _ = ks.save_to_path(path);
        Ok(ks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_keystore_encryption_and_restoration() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();
        let key = [0x77u8; 32];

        let original = KeyStore::generate();
        let orig_addr = original.identity.public_key().address();

        original.save_encrypted(path, &key).expect("save succeeds");

        let loaded = KeyStore::load_encrypted(path, &key).expect("load succeeds");
        assert_eq!(loaded.identity.public_key().address(), orig_addr);
        assert_eq!(loaded.one_time_prekeys.len(), 10);
    }

    #[test]
    fn test_keystore_random_nonce_uniqueness() {
        let temp_file1 = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        let key = [0x77u8; 32];

        let original = KeyStore::generate();
        original
            .save_encrypted(temp_file1.path(), &key)
            .expect("save 1");
        original
            .save_encrypted(temp_file2.path(), &key)
            .expect("save 2");

        let data1 = fs::read(temp_file1.path()).unwrap();
        let data2 = fs::read(temp_file2.path()).unwrap();

        // Nonce must not repeat: first 12 bytes must differ
        assert_ne!(&data1[..12], &data2[..12]);
        // Ciphertext must differ due to fresh nonce
        assert_ne!(data1, data2);
    }

    #[test]
    fn test_consume_one_time_prekey() {
        let mut ks = KeyStore::generate();
        assert_eq!(ks.one_time_prekeys.len(), 10);
        let target_pubkey = ks.one_time_prekeys[0].pubkey;

        let consumed = ks.consume_one_time_prekey(&target_pubkey);
        assert!(consumed.is_some());
        assert_eq!(consumed.unwrap().pubkey, target_pubkey);
        assert_eq!(ks.one_time_prekeys.len(), 9);

        // Cannot consume again
        assert!(ks.consume_one_time_prekey(&target_pubkey).is_none());
    }

    #[test]
    fn test_load_or_generate_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("keystore.bin");

        let ks1 = KeyStore::load_or_generate(&path).expect("generate first time");
        let addr1 = ks1.identity.public_key().address();

        let ks2 = KeyStore::load_or_generate(&path).expect("load second time");
        let addr2 = ks2.identity.public_key().address();

        assert_eq!(addr1, addr2);
        assert_eq!(ks1.signed_prekey.pubkey, ks2.signed_prekey.pubkey);
    }
}
