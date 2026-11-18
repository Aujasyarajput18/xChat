use xchat_core::{
    duress::{DuressVaultManager, VaultMode},
    replay::ReplayFilter,
    sos::{create_emergency_beacon, decode_emergency_beacon, SosBeacon},
};
use xchat_crypto::identity::IdentityKeyPair;

#[test]
fn test_mutation_signature_verification_must_fail_on_tamper() {
    let identity = IdentityKeyPair::generate();
    let now = 1000u64;
    let mut envelope =
        create_emergency_beacon(&identity, 37.7749, -122.4194, "Distress Test", now, 3600).unwrap();

    // Verify valid beacon passes
    let valid_beacon = decode_emergency_beacon(&envelope, now).unwrap();
    assert_eq!(valid_beacon.emergency_message, "Distress Test");

    // Mutation 1: Mutated latitude in payload
    let mut beacon: SosBeacon = bincode::deserialize(&envelope.ciphertext).unwrap();
    beacon.latitude += 0.0001;
    envelope.ciphertext = bincode::serialize(&beacon).unwrap();

    let res_mutated_lat = decode_emergency_beacon(&envelope, now);
    assert!(
        res_mutated_lat.is_err(),
        "Mutation survived: Signature passed on mutated latitude!"
    );

    // Mutation 2: Mutated signature byte
    beacon.signature[0] ^= 0x01;
    envelope.ciphertext = bincode::serialize(&beacon).unwrap();
    let res_mutated_sig = decode_emergency_beacon(&envelope, now);
    assert!(
        res_mutated_sig.is_err(),
        "Mutation survived: Corrupted signature passed verification!"
    );
}

#[test]
fn test_mutation_duress_pin_verification_must_fail_on_wrong_pin() {
    let dm = DuressVaultManager::new("123456", "999999");

    // Valid authentication passes
    assert_eq!(dm.authenticate("123456"), VaultMode::Real);
    assert_eq!(dm.authenticate("999999"), VaultMode::Decoy);

    // Mutation 1: 1 digit off
    assert_eq!(
        dm.authenticate("123457"),
        VaultMode::Invalid,
        "Mutation survived: Off-by-one PIN authenticated successfully!"
    );

    // Mutation 2: Empty PIN
    assert_eq!(
        dm.authenticate(""),
        VaultMode::Invalid,
        "Mutation survived: Empty PIN authenticated!"
    );
}

#[test]
fn test_mutation_replay_filter_must_reject_duplicate() {
    let mut filter = ReplayFilter::new();
    let msg_id = [0x77u8; 16];

    assert!(filter.check_and_record(&msg_id, 1000).is_ok());
    // Duplicate MUST be rejected
    assert!(
        filter.check_and_record(&msg_id, 1001).is_err(),
        "Mutation survived: Duplicate message ID was accepted!"
    );
}
