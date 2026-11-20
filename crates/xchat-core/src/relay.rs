use std::collections::HashMap;

use crate::{
    envelope::{EncryptedEnvelope, RouteToken},
    errors::CoreError,
};

pub const MAX_RELAY_STORE_SIZE: usize = 5000;
pub const MAX_HOPS: u8 = 16;

/// Relay node facilitating store-and-forward routing between asynchronous/offline peers.
/// Completely BLIND to message plaintext, sender identity, and conversation graph.
#[derive(Debug, Default)]
pub struct RelayNode {
    storage: HashMap<RouteToken, Vec<EncryptedEnvelope>>,
    total_stored: usize,
}

impl RelayNode {
    pub fn new() -> Self {
        Self {
            storage: HashMap::new(),
            total_stored: 0,
        }
    }

    /// Accept and buffer an opaque encrypted envelope for a destination routing token.
    pub fn forward_envelope(
        &mut self,
        mut envelope: EncryptedEnvelope,
        current_time: u64,
    ) -> Result<(), CoreError> {
        if envelope.is_expired(current_time) {
            return Err(CoreError::ExpiredEnvelope {
                expires_at: envelope.header.expires_at,
                now: current_time,
            });
        }

        if self.total_stored >= MAX_RELAY_STORE_SIZE {
            self.prune_expired(current_time);
            if self.total_stored >= MAX_RELAY_STORE_SIZE {
                return Err(CoreError::RoutingError("Relay capacity exceeded".into()));
            }
        }

        if envelope.header.hop_count >= MAX_HOPS {
            return Err(CoreError::RoutingError(format!(
                "Envelope exceeded maximum hop count ({})",
                MAX_HOPS
            )));
        }

        envelope.increment_hop();
        let token = envelope.header.destination_token;
        self.storage.entry(token).or_default().push(envelope);
        self.total_stored += 1;
        Ok(())
    }

    /// Retrieve and drain all pending envelopes addressed to a specific ephemeral routing token.
    pub fn fetch_for_token(&mut self, token: &RouteToken) -> Vec<EncryptedEnvelope> {
        if let Some(envelopes) = self.storage.remove(token) {
            self.total_stored = self.total_stored.saturating_sub(envelopes.len());
            envelopes
        } else {
            Vec::new()
        }
    }

    /// Retrieve and drain all pending envelopes addressed to a specific ephemeral routing token,
    /// cryptographically enforcing that the token is tied to the authenticated public key.
    pub fn fetch_for_token_authenticated(
        &mut self,
        token: &RouteToken,
        public_key: &[u8; 32],
        now: u64,
    ) -> Result<Vec<EncryptedEnvelope>, CoreError> {
        if !is_token_derived_from_pubkey(token, public_key, now) {
            return Err(CoreError::RoutingError(
                "Unauthorized: Requested token does not belong to the authenticated identity"
                    .into(),
            ));
        }
        Ok(self.fetch_for_token(token))
    }

    /// Retrieve and drain all pending envelopes belonging to the authenticated public key
    /// across all active rotating epochs.
    pub fn fetch_all_for_pubkey(
        &mut self,
        public_key: &[u8; 32],
        now: u64,
    ) -> Vec<EncryptedEnvelope> {
        let mut matched_tokens = Vec::new();
        for token in self.storage.keys() {
            if is_token_derived_from_pubkey(token, public_key, now) {
                matched_tokens.push(*token);
            }
        }
        let mut all_envelopes = Vec::new();
        for t in matched_tokens {
            all_envelopes.extend(self.fetch_for_token(&t));
        }
        all_envelopes
    }

    /// Prune expired packets according to packet headers.
    pub fn prune_expired(&mut self, current_time: u64) {
        let mut count = 0;
        for list in self.storage.values_mut() {
            list.retain(|env| !env.is_expired(current_time));
            count += list.len();
        }
        self.storage.retain(|_, list| !list.is_empty());
        self.total_stored = count;
    }

    pub fn total_stored(&self) -> usize {
        self.total_stored
    }
}

/// Verify whether an ephemeral RouteToken was derived from the given public key
/// across any recent or active epoch window.
pub fn is_token_derived_from_pubkey(token: &RouteToken, public_key: &[u8; 32], now: u64) -> bool {
    // 1. Check 1-hour epoch windows (past 24h and future 2h)
    let cur_hour = now / 3600;
    for h in 0..=24 {
        let epoch = cur_hour.saturating_sub(h);
        if xchat_crypto::kdf::derive_discovery_token(public_key, epoch) == *token {
            return true;
        }
    }
    for h in 1..=2 {
        let epoch = cur_hour + h;
        if xchat_crypto::kdf::derive_discovery_token(public_key, epoch) == *token {
            return true;
        }
    }

    // 2. Check fine-grained 60-second rotating epoch windows (past 120 minutes)
    let cur_minute = now / 60;
    for m in 0..=120 {
        let epoch = cur_minute.saturating_sub(m);
        if xchat_crypto::kdf::derive_discovery_token(public_key, epoch) == *token {
            return true;
        }
    }
    for m in 1..=5 {
        let epoch = cur_minute + m;
        if xchat_crypto::kdf::derive_discovery_token(public_key, epoch) == *token {
            return true;
        }
    }

    // 3. Check 5-minute rotating epoch windows (past 288 intervals = 24h)
    let cur_5min = now / 300;
    for interval in 0..=288 {
        let epoch = cur_5min.saturating_sub(interval);
        if xchat_crypto::kdf::derive_discovery_token(public_key, epoch) == *token {
            return true;
        }
    }

    // 4. Check sequence-based ratchet tokens (seq 0..256)
    for seq in 0..=256 {
        if xchat_crypto::kdf::derive_discovery_token(public_key, seq) == *token {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::RoutingHeader;

    #[test]
    fn test_relay_blind_forwarding() {
        let mut relay = RelayNode::new();
        let token_bob = [0x55; 16];

        let header = RoutingHeader::new(token_bob, 1000, 3600);
        let env = EncryptedEnvelope {
            header,
            ratchet_header: None,
            x3dh_init: None,
            ciphertext: vec![0xde, 0xad, 0xbe, 0xef],
        };

        // Forward to relay
        relay
            .forward_envelope(env.clone(), 1005)
            .expect("forward succeeds");
        assert_eq!(relay.total_stored(), 1);

        // Fetch using token
        let fetched = relay.fetch_for_token(&token_bob);
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].header.hop_count, 1);
        assert_eq!(relay.total_stored(), 0);
    }

    #[test]
    fn test_relay_max_hops_enforced() {
        let mut relay = RelayNode::new();
        let mut header = RoutingHeader::new([0x55; 16], 1000, 3600);
        header.hop_count = MAX_HOPS;
        let env = EncryptedEnvelope {
            header,
            ratchet_header: None,
            x3dh_init: None,
            ciphertext: vec![1, 2, 3],
        };

        let result = relay.forward_envelope(env, 1005);
        assert!(result.is_err());
    }

    #[test]
    fn test_relay_mailbox_cannot_be_drained_by_different_identity() {
        let mut relay = RelayNode::new();
        let bob = xchat_crypto::identity::IdentityKeyPair::generate();
        let mallory = xchat_crypto::identity::IdentityKeyPair::generate();
        let now = 1_000_000u64;

        let bob_pubkey = bob.public_key().sign_pubkey;
        let mallory_pubkey = mallory.public_key().sign_pubkey;

        // Bob's token derived by epoch
        let bob_token = xchat_crypto::kdf::derive_discovery_token(&bob_pubkey, now / 3600);

        let header = RoutingHeader::new(bob_token, now, 3600);
        let env = EncryptedEnvelope {
            header,
            ratchet_header: None,
            x3dh_init: None,
            ciphertext: b"Confidential message for Bob".to_vec(),
        };

        relay.forward_envelope(env, now).unwrap();
        assert_eq!(relay.total_stored(), 1);

        // 1. Mallory attempts to fetch Bob's token
        let mal_res = relay.fetch_for_token_authenticated(&bob_token, &mallory_pubkey, now);
        assert!(
            mal_res.is_err(),
            "Mallory must be rejected when requesting Bob's token"
        );
        assert_eq!(
            relay.total_stored(),
            1,
            "Bob's message must NOT be drained by Mallory!"
        );

        // Mallory attempts to fetch all envelopes for Mallory's key
        let mal_all = relay.fetch_all_for_pubkey(&mallory_pubkey, now);
        assert!(mal_all.is_empty(), "Mallory must get 0 envelopes");
        assert_eq!(
            relay.total_stored(),
            1,
            "Bob's message must NOT be drained by Mallory!"
        );

        // 2. Bob fetches for Bob's token
        let bob_fetched = relay
            .fetch_for_token_authenticated(&bob_token, &bob_pubkey, now)
            .unwrap();
        assert_eq!(bob_fetched.len(), 1);
        assert_eq!(bob_fetched[0].ciphertext, b"Confidential message for Bob");
        assert_eq!(
            relay.total_stored(),
            0,
            "Bob successfully drains his own mailbox"
        );
    }
}
