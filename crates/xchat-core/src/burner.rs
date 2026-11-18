use std::collections::VecDeque;

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use xchat_crypto::{
    aead::{decrypt_aead, encrypt_aead},
    errors::CryptoError,
    kdf::derive_discovery_token,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    envelope::{EncryptedEnvelope, RoutingHeader},
    errors::CoreError,
};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct BurnerMessage {
    pub sender_alias: String,
    pub content: String,
    pub timestamp: u64,
    pub expires_at: u64,
}

/// Ephemeral RAM-only self-destructing room.
/// Messages and keys are NEVER written to flash storage.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct BurnerRoom {
    #[zeroize(skip)]
    pub room_name: String,
    room_key: [u8; 32],
    #[zeroize(skip)]
    routing_token: [u8; 16],
    #[zeroize(skip)]
    ttl_seconds: u64,
    #[zeroize(skip)]
    messages: VecDeque<BurnerMessage>,
}

impl BurnerRoom {
    /// Join or create an ephemeral burner room using a room name and passphrase.
    pub fn new(room_name: &str, passphrase: &str, ttl_seconds: u64) -> Self {
        // Derive shared room key via HKDF
        let hk = hkdf::Hkdf::<Sha256>::new(Some(room_name.as_bytes()), passphrase.as_bytes());
        let mut room_key = [0u8; 32];
        hk.expand(b"xchat-burner-room-key-v1", &mut room_key)
            .expect("32 bytes is valid");

        // Derive routing token
        let routing_token = derive_discovery_token(&room_key, 0xbbbb);

        Self {
            room_name: room_name.to_string(),
            room_key,
            routing_token,
            ttl_seconds,
            messages: VecDeque::new(),
        }
    }

    pub fn routing_token(&self) -> [u8; 16] {
        self.routing_token
    }

    /// Post a message to the burner room. Returns an envelope ready for broadcast over mesh.
    /// Uses a random 12-byte nonce per message prepended to ciphertext to guarantee nonce uniqueness.
    pub fn post_message(
        &mut self,
        sender_alias: &str,
        content: &str,
        now: u64,
    ) -> Result<EncryptedEnvelope, CoreError> {
        let msg = BurnerMessage {
            sender_alias: sender_alias.to_string(),
            content: content.to_string(),
            timestamp: now,
            expires_at: now + self.ttl_seconds,
        };

        let serialized =
            bincode::serialize(&msg).map_err(|e| CoreError::Serialization(e.to_string()))?;
        let header = RoutingHeader::new(self.routing_token, now, self.ttl_seconds);
        let aad = header.to_aad();

        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext =
            encrypt_aead(&self.room_key, &nonce, &serialized, &aad).map_err(CoreError::Crypto)?;

        let mut full_payload = Vec::with_capacity(12 + ciphertext.len());
        full_payload.extend_from_slice(&nonce);
        full_payload.extend_from_slice(&ciphertext);

        self.messages.push_back(msg);

        Ok(EncryptedEnvelope {
            header,
            ratchet_header: None,
            x3dh_init: None,
            ciphertext: full_payload,
        })
    }

    /// Receive and decrypt a burner room message envelope.
    pub fn receive_envelope(
        &mut self,
        envelope: &EncryptedEnvelope,
        now: u64,
    ) -> Result<BurnerMessage, CoreError> {
        if envelope.header.destination_token != self.routing_token {
            return Err(CoreError::EnvelopeError(
                "Envelope token does not match burner room".into(),
            ));
        }

        if envelope.is_expired(now) {
            return Err(CoreError::ExpiredEnvelope {
                expires_at: envelope.header.expires_at,
                now,
            });
        }

        if envelope.ciphertext.len() < 12 {
            return Err(CoreError::Crypto(CryptoError::DecryptionFailed));
        }

        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&envelope.ciphertext[..12]);
        let ciphertext = &envelope.ciphertext[12..];

        let aad = envelope.header.to_aad();
        let decrypted =
            decrypt_aead(&self.room_key, &nonce, ciphertext, &aad).map_err(CoreError::Crypto)?;

        let msg: BurnerMessage = bincode::deserialize(&decrypted)
            .map_err(|e| CoreError::Serialization(e.to_string()))?;

        self.messages.push_back(msg.clone());
        self.prune_expired(now);

        Ok(msg)
    }

    /// Remove expired messages from memory.
    pub fn prune_expired(&mut self, now: u64) {
        self.messages.retain(|m| now < m.expires_at);
    }

    /// List active messages currently in RAM.
    pub fn get_messages(&self) -> Vec<&BurnerMessage> {
        self.messages.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burner_room_lifecycle() {
        let mut room_alice = BurnerRoom::new("recon-team-alpha", "mission-passphrase-99", 60);
        let mut room_bob = BurnerRoom::new("recon-team-alpha", "mission-passphrase-99", 60);

        assert_eq!(room_alice.routing_token(), room_bob.routing_token());

        let now = 1000;
        let env = room_alice
            .post_message("Alice", "Checkpoint secured at perimeter", now)
            .expect("post succeeds");

        let received = room_bob
            .receive_envelope(&env, now + 5)
            .expect("receive succeeds");
        assert_eq!(received.sender_alias, "Alice");
        assert_eq!(received.content, "Checkpoint secured at perimeter");

        // Expire message
        room_bob.prune_expired(now + 65);
        assert_eq!(room_bob.get_messages().len(), 0);
    }

    #[test]
    fn test_burner_same_second_nonces_unique() {
        let mut room = BurnerRoom::new("recon-team-alpha", "passphrase", 60);
        let now = 1000;

        // Post two messages in the same second
        let env1 = room.post_message("Alice", "Message 1", now).unwrap();
        let env2 = room.post_message("Alice", "Message 2", now).unwrap();

        // Nonces are in the first 12 bytes of ciphertext and must NOT match
        assert_ne!(&env1.ciphertext[..12], &env2.ciphertext[..12]);

        // Both messages must decrypt cleanly
        let mut receiver = BurnerRoom::new("recon-team-alpha", "passphrase", 60);
        let msg1 = receiver.receive_envelope(&env1, now).unwrap();
        let msg2 = receiver.receive_envelope(&env2, now).unwrap();
        assert_eq!(msg1.content, "Message 1");
        assert_eq!(msg2.content, "Message 2");
    }
}
