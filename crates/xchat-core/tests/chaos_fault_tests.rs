use xchat_core::{
    envelope::{EncryptedEnvelope, RoutingHeader},
    errors::CoreError,
    keystore::KeyStore,
    relay::{RelayNode, MAX_HOPS},
    replay::ReplayFilter,
    session::Session,
};

#[test]
fn test_chaos_clock_jump_ttl_expiration() {
    let alice_ks = KeyStore::generate();
    let mut bob_ks = KeyStore::generate();
    let bob_bundle = bob_ks.export_bundle();

    let created_at = 1_000_000u64;
    let ttl_seconds = 300u64; // 5 minute TTL

    let (mut alice, init_env) = Session::initiate_outbound(
        &alice_ks,
        &bob_bundle,
        b"Initial",
        [0x11; 16],
        created_at,
        ttl_seconds,
    )
    .unwrap();

    let (mut bob, _) = Session::respond_inbound(&bob_ks, &init_env, created_at).unwrap();

    // 1. Delivery within TTL window succeeds
    let env_normal = alice
        .encrypt_message(
            b"Time-sensitive payload",
            [0x11; 16],
            created_at + 10,
            ttl_seconds,
        )
        .unwrap();
    let pt_normal = bob.decrypt_message(&env_normal, created_at + 100);
    assert!(pt_normal.is_ok());

    // 2. Deliver message after sudden clock jump (+1 hour forward)
    let jump_time = created_at + 3600;
    let env_delayed = alice
        .encrypt_message(b"Delayed message", [0x11; 16], created_at + 20, ttl_seconds)
        .unwrap();
    let pt_expired = bob.decrypt_message(&env_delayed, jump_time);

    // Invariant: Expired message MUST fail closed (ExpiredEnvelope error)
    assert!(
        pt_expired.is_err(),
        "Expected expired envelope error after clock jump"
    );
    match pt_expired.unwrap_err() {
        CoreError::ExpiredEnvelope { .. } => {}
        other => panic!("Expected ExpiredEnvelope, got {:?}", other),
    }
}

#[test]
fn test_chaos_relay_hop_exhaustion_fault() {
    let mut relay = RelayNode::new();
    let mut header = RoutingHeader::new([0x55; 16], 1000, 3600);
    header.hop_count = MAX_HOPS; // already at max hops limit (16)

    let env = EncryptedEnvelope {
        header,
        ratchet_header: None,
        x3dh_init: None,
        ciphertext: b"Forwarding payload".to_vec(),
    };

    // Forwarding must fail closed with RoutingError when max hops reached
    let res = relay.forward_envelope(env, 1001);
    assert!(res.is_err());
    match res.unwrap_err() {
        CoreError::RoutingError(msg) => {
            assert!(msg.contains("Maximum hop count exceeded"));
        }
        other => panic!("Expected RoutingError, got {:?}", other),
    }
}

#[test]
fn test_chaos_dropped_packets_ratchet_recovery() {
    let alice_ks = KeyStore::generate();
    let mut bob_ks = KeyStore::generate();
    let bob_bundle = bob_ks.export_bundle();

    let now = 10_000u64;
    let (mut alice, init_env) =
        Session::initiate_outbound(&alice_ks, &bob_bundle, b"Message 0", [0x22; 16], now, 3600)
            .unwrap();

    let (mut bob, pt0) = Session::respond_inbound(&bob_ks, &init_env, now).unwrap();
    assert_eq!(pt0, b"Message 0");

    // Alice generates additional messages: [M1 (dropped), M2 (dropped), M3, M4]
    let _env1_dropped = alice
        .encrypt_message(b"Message 1 (LOST IN TRANSIT)", [0x22; 16], now + 1, 3600)
        .unwrap();
    let _env2_dropped = alice
        .encrypt_message(b"Message 2 (LOST IN TRANSIT)", [0x22; 16], now + 2, 3600)
        .unwrap();
    let env3 = alice
        .encrypt_message(b"Message 3", [0x22; 16], now + 3, 3600)
        .unwrap();
    let env4 = alice
        .encrypt_message(b"Message 4", [0x22; 16], now + 4, 3600)
        .unwrap();

    // Bob skips M1 and M2, directly receives M3
    let pt3 = bob.decrypt_message(&env3, now + 3).unwrap();
    assert_eq!(pt3, b"Message 3");

    // Bob receives M4
    let pt4 = bob.decrypt_message(&env4, now + 4).unwrap();
    assert_eq!(pt4, b"Message 4");
}

#[test]
fn test_chaos_replay_window_pruning_under_timestamp_jitter() {
    let mut filter = ReplayFilter::new();
    let base_time = 50_000u64;

    // Record 5 messages
    for i in 0..5 {
        let mut id = [0u8; 16];
        id[0] = i;
        filter.check_and_record(&id, base_time + i as u64).unwrap();
    }
    assert_eq!(filter.size(), 5);

    // Jittered time advance (+100,000 seconds past 3600s window)
    let advanced_time = base_time + 100_000;
    filter.prune(advanced_time, 3600);
    assert_eq!(
        filter.size(),
        0,
        "Expected all 5 messages to be pruned outside window"
    );

    // Fresh message in current window succeeds
    let fresh_id = [0x99; 16];
    assert!(filter.check_and_record(&fresh_id, advanced_time).is_ok());
}
