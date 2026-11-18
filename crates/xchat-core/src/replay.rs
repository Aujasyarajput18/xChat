use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use xchat_crypto::aead::{decrypt_aead, encrypt_aead};

use crate::{envelope::MessageId, errors::CoreError};

pub const DEFAULT_REPLAY_WINDOW_SECS: u64 = 86400 * 2; // 48 hours

/// Anti-replay detection filter tracking observed message identifiers.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReplayFilter {
    seen_messages: HashMap<MessageId, u64>,
}

impl ReplayFilter {
    pub fn new() -> Self {
        Self {
            seen_messages: HashMap::new(),
        }
    }

    /// Check if a message ID has already been processed. If not seen, records it.
    pub fn check_and_record(
        &mut self,
        message_id: &MessageId,
        current_time: u64,
    ) -> Result<(), CoreError> {
        if self.seen_messages.contains_key(message_id) {
            return Err(CoreError::ReplayDetected(hex::encode(message_id)));
        }

        self.seen_messages.insert(*message_id, current_time);
        Ok(())
    }

    /// Prune old message records older than the replay window.
    pub fn prune(&mut self, current_time: u64, max_age_seconds: u64) {
        let cutoff = current_time.saturating_sub(max_age_seconds);
        self.seen_messages
            .retain(|_, &mut seen_at| seen_at >= cutoff);
    }

    pub fn size(&self) -> usize {
        self.seen_messages.len()
    }

    /// Persist the replay filter encrypted with ChaCha20-Poly1305 and a CSPRNG nonce to disk.
    pub fn save_to_disk_encrypted(
        &self,
        path: &Path,
        encryption_key: &[u8; 32],
    ) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| CoreError::StorageError(e.to_string()))?;
        }
        let serialized =
            bincode::serialize(self).map_err(|e| CoreError::StorageError(e.to_string()))?;
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = encrypt_aead(
            encryption_key,
            &nonce,
            &serialized,
            b"xchat-replay-filter-v1",
        )
        .map_err(CoreError::Crypto)?;

        let mut output = Vec::with_capacity(12 + ciphertext.len());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);

        let mut file = File::create(path).map_err(|e| CoreError::StorageError(e.to_string()))?;
        file.write_all(&output)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        Ok(())
    }

    /// Restore the replay filter from an encrypted disk file using ChaCha20-Poly1305.
    pub fn load_from_disk_encrypted(
        path: &Path,
        encryption_key: &[u8; 32],
    ) -> Result<Self, CoreError> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let mut file = File::open(path).map_err(|e| CoreError::StorageError(e.to_string()))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if data.len() < 12 {
            return Err(CoreError::StorageError(
                "Corrupted replay filter file: missing nonce header".into(),
            ));
        }

        let (nonce_bytes, ciphertext) = data.split_at(12);
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(nonce_bytes);

        let decrypted = decrypt_aead(
            encryption_key,
            &nonce,
            ciphertext,
            b"xchat-replay-filter-v1",
        )
        .map_err(CoreError::Crypto)?;

        bincode::deserialize(&decrypted).map_err(|e| CoreError::StorageError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_protection() {
        let mut filter = ReplayFilter::new();
        let msg_id = [0x42; 16];

        // First arrival succeeds
        assert!(filter.check_and_record(&msg_id, 1000).is_ok());

        // Replay of same message ID must fail
        assert!(filter.check_and_record(&msg_id, 1005).is_err());

        // Different message ID succeeds
        let msg_id_2 = [0x43; 16];
        assert!(filter.check_and_record(&msg_id_2, 1010).is_ok());
    }

    #[test]
    fn test_replay_pruning() {
        let mut filter = ReplayFilter::new();
        let old_msg = [0x01; 16];
        let new_msg = [0x02; 16];

        filter.check_and_record(&old_msg, 100).unwrap();
        filter.check_and_record(&new_msg, 1000).unwrap();

        // Prune entries older than 500 seconds from t=1000 (cutoff = 500)
        filter.prune(1000, 500);

        assert_eq!(filter.size(), 1);
        // Old message was pruned, new message remains
        assert!(filter.check_and_record(&new_msg, 1010).is_err());
    }

    #[test]
    fn test_replay_filter_encrypted_persistence() {
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();
        let key = [0x77u8; 32];

        let mut filter = ReplayFilter::new();
        let msg_id = [0x99; 16];
        filter.check_and_record(&msg_id, 1000).unwrap();

        // Save to disk encrypted
        filter.save_to_disk_encrypted(path, &key).unwrap();

        // Restore from disk encrypted
        let mut loaded = ReplayFilter::load_from_disk_encrypted(path, &key).unwrap();
        assert_eq!(loaded.size(), 1);

        // Verify replayed message is still rejected after reload
        assert!(loaded.check_and_record(&msg_id, 1005).is_err());

        // Verify wrong key fails
        let wrong_key = [0x88u8; 32];
        assert!(ReplayFilter::load_from_disk_encrypted(path, &wrong_key).is_err());
    }
}
