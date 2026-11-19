use serde::{Deserialize, Serialize};
use xchat_crypto::{
    errors::CryptoError,
    identity::{IdentityKeyPair, IdentityPublicKey},
};

use crate::{
    envelope::{EncryptedEnvelope, RoutingHeader},
    errors::CoreError,
};

/// Public emergency destination token for SOS mesh packets.
pub const EMERGENCY_BROADCAST_TOKEN: [u8; 16] = [0xff; 16];

mod serde_sig {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
    where
        D: Deserializer<'de>,
    {
        let v = Vec::<u8>::deserialize(deserializer)?;
        if v.len() != 64 {
            return Err(serde::de::Error::custom("expected 64 bytes signature"));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&v);
        Ok(arr)
    }
}

/// Signed emergency distress beacon broadcasted over local radio mesh.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct SosBeacon {
    pub sender_address: String,
    pub sender_pubkey: IdentityPublicKey,
    pub timestamp: u64,
    pub latitude: f64,
    pub longitude: f64,
    pub emergency_message: String,
    #[serde(with = "serde_sig")]
    pub signature: [u8; 64],
}

impl SosBeacon {
    /// Format human-readable emergency text.
    pub fn display_summary(&self) -> String {
        format!(
            "🚨 SOS DISTRESS BEACON from {}\nLocation: {:.5}, {:.5}\nTime: {}\nMessage: \"{}\"",
            self.sender_address,
            self.latitude,
            self.longitude,
            self.timestamp,
            self.emergency_message
        )
    }
}

fn beacon_signature_payload(
    sender_pubkey: &IdentityPublicKey,
    timestamp: u64,
    latitude: f64,
    longitude: f64,
    message: &str,
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"xchat-sos-beacon-v1");
    payload.extend_from_slice(&sender_pubkey.sign_pubkey);
    payload.extend_from_slice(&timestamp.to_be_bytes());
    payload.extend_from_slice(&latitude.to_bits().to_be_bytes());
    payload.extend_from_slice(&longitude.to_bits().to_be_bytes());
    payload.extend_from_slice(message.as_bytes());
    payload
}

/// Create and sign a high-priority emergency distress beacon using the sender's Ed25519 identity key.
pub fn create_emergency_beacon(
    identity: &IdentityKeyPair,
    latitude: f64,
    longitude: f64,
    message: &str,
    now: u64,
    ttl_seconds: u64,
) -> Result<EncryptedEnvelope, CoreError> {
    let sender_pubkey = identity.public_key();
    let sig_payload = beacon_signature_payload(&sender_pubkey, now, latitude, longitude, message);
    let signature = identity.sign(&sig_payload);

    let beacon = SosBeacon {
        sender_address: sender_pubkey.address(),
        sender_pubkey,
        timestamp: now,
        latitude,
        longitude,
        emergency_message: message.to_string(),
        signature,
    };

    let serialized =
        bincode::serialize(&beacon).map_err(|e| CoreError::Serialization(e.to_string()))?;
    let header = RoutingHeader::new(EMERGENCY_BROADCAST_TOKEN, now, ttl_seconds);

    Ok(EncryptedEnvelope {
        header,
        ratchet_header: None,
        x3dh_init: None,
        ciphertext: serialized,
    })
}

/// Decode and cryptographically verify an inbound emergency distress beacon.
/// Ensures the Ed25519 signature is authentic and matches the claimed sender identity.
pub fn decode_emergency_beacon(
    envelope: &EncryptedEnvelope,
    now: u64,
) -> Result<SosBeacon, CoreError> {
    if envelope.header.destination_token != EMERGENCY_BROADCAST_TOKEN {
        return Err(CoreError::EnvelopeError(
            "Not an emergency beacon packet".into(),
        ));
    }

    if envelope.is_expired(now) {
        return Err(CoreError::ExpiredEnvelope {
            expires_at: envelope.header.expires_at,
            now,
        });
    }

    let beacon: SosBeacon = bincode::deserialize(&envelope.ciphertext)
        .map_err(|e| CoreError::Serialization(e.to_string()))?;

    // Verify claimed address matches public key
    if beacon.sender_address != beacon.sender_pubkey.address() {
        return Err(CoreError::Crypto(CryptoError::InvalidSignature));
    }

    // Verify Ed25519 signature over all beacon parameters
    let sig_payload = beacon_signature_payload(
        &beacon.sender_pubkey,
        beacon.timestamp,
        beacon.latitude,
        beacon.longitude,
        &beacon.emergency_message,
    );
    beacon
        .sender_pubkey
        .verify(&sig_payload, &beacon.signature)
        .map_err(CoreError::Crypto)?;

    Ok(beacon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emergency_sos_beacon_roundtrip_and_signature_verification() {
        let identity = IdentityKeyPair::generate();
        let now = 1000;
        let env = create_emergency_beacon(
            &identity,
            37.7749,
            -122.4194,
            "Earthquake survivor, need medical aid!",
            now,
            7200,
        )
        .expect("beacon created");

        assert_eq!(env.header.destination_token, EMERGENCY_BROADCAST_TOKEN);

        let decoded = decode_emergency_beacon(&env, now + 10).expect("beacon decoded");
        assert_eq!(decoded.sender_address, identity.public_key().address());
        assert_eq!(decoded.latitude, 37.7749);
        assert_eq!(
            decoded.emergency_message,
            "Earthquake survivor, need medical aid!"
        );
    }

    #[test]
    fn test_forged_emergency_beacon_rejected() {
        let alice = IdentityKeyPair::generate();
        let eve = IdentityKeyPair::generate();
        let now = 1000;

        let env = create_emergency_beacon(
            &alice,
            37.7749,
            -122.4194,
            "Legitimate distress call",
            now,
            7200,
        )
        .expect("beacon created");

        let mut beacon: SosBeacon = bincode::deserialize(&env.ciphertext).unwrap();
        // Attacker tampers with coordinates or message
        beacon.emergency_message = "Spoofed distress call".to_string();
        let tampered_bytes = bincode::serialize(&beacon).unwrap();

        let mut tampered_env = env.clone();
        tampered_env.ciphertext = tampered_bytes;

        let result = decode_emergency_beacon(&tampered_env, now + 5);
        assert!(
            result.is_err(),
            "Tampered message must fail signature verification!"
        );

        // Attacker attempts identity substitution
        let mut impersonated_beacon: SosBeacon = bincode::deserialize(&env.ciphertext).unwrap();
        impersonated_beacon.sender_pubkey = eve.public_key();
        let impersonated_bytes = bincode::serialize(&impersonated_beacon).unwrap();
        let mut imp_env = env;
        imp_env.ciphertext = impersonated_bytes;

        let result2 = decode_emergency_beacon(&imp_env, now + 5);
        assert!(result2.is_err(), "Impersonated key must fail validation!");
    }
}
