use std::time::Instant;
use xchat_core::{keystore::KeyStore, session::Session};
use xchat_crypto::{
    aead::{decrypt_aead, encrypt_aead},
    x3dh::{x3dh_initiate, x3dh_respond},
};

#[test]
fn test_stress_symmetric_crypto_throughput_and_latencies() {
    let key = [0x42u8; 32];
    let nonce = [0x11u8; 12];
    let aad = b"xchat-stress-aad";
    let payload_sizes = [64, 1024, 16384];

    for &size in &payload_sizes {
        let plaintext = vec![0x61u8; size];
        let iterations = 2000;
        let mut latencies_us = Vec::with_capacity(iterations);

        let start_total = Instant::now();
        for _ in 0..iterations {
            let iter_start = Instant::now();
            let ct = encrypt_aead(&key, &nonce, &plaintext, aad).unwrap();
            let pt = decrypt_aead(&key, &nonce, &ct, aad).unwrap();
            assert_eq!(pt.len(), size);
            latencies_us.push(iter_start.elapsed().as_micros());
        }
        let total_duration = start_total.elapsed();
        latencies_us.sort_unstable();

        let p50 = latencies_us[iterations * 50 / 100];
        let p95 = latencies_us[iterations * 95 / 100];
        let p99 = latencies_us[iterations * 99 / 100];
        let ops_per_sec = (iterations as f64 / total_duration.as_secs_f64()) * 2.0; // encrypt + decrypt

        println!(
            "[STRESS] Payload size: {:>5} B | Total: {:.2?} | {:.0} ops/sec | p50: {} µs | p95: {} µs | p99: {} µs",
            size, total_duration, ops_per_sec, p50, p95, p99
        );

        // Assert SLO: symmetric encrypt+decrypt roundtrip p95 < 500 µs (release) or < 15,000 µs (unoptimized debug)
        let max_allowed_p95 = if cfg!(debug_assertions) { 15_000 } else { 500 };
        assert!(
            p95 < max_allowed_p95,
            "SLO breached: p95 latency {} µs exceeds limit {} µs",
            p95,
            max_allowed_p95
        );
    }
}

#[test]
fn test_stress_x3dh_handshake_throughput() {
    let iterations = 200;
    let mut latencies_ms = Vec::with_capacity(iterations);

    let start_total = Instant::now();
    for _ in 0..iterations {
        let iter_start = Instant::now();

        let alice_ks = KeyStore::generate();
        let mut bob_ks = KeyStore::generate();
        let bob_bundle = bob_ks.export_bundle();

        let (init_msg, shared_alice) = x3dh_initiate(&alice_ks.identity, &bob_bundle).unwrap();
        let shared_bob = x3dh_respond(
            &bob_ks.identity,
            &bob_ks.signed_prekey.secret,
            &bob_ks.one_time_prekeys,
            &init_msg,
        )
        .unwrap();

        assert_eq!(shared_alice, shared_bob);
        latencies_ms.push(iter_start.elapsed().as_secs_f64() * 1000.0);
    }
    let total_duration = start_total.elapsed();
    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let p50 = latencies_ms[iterations * 50 / 100];
    let p95 = latencies_ms[iterations * 95 / 100];
    let handshakes_per_sec = iterations as f64 / total_duration.as_secs_f64();

    println!(
        "[STRESS] X3DH Handshake | Total: {:.2?} | {:.1} handshakes/sec | p50: {:.2} ms | p95: {:.2} ms",
        total_duration, handshakes_per_sec, p50, p95
    );

    // Assert SLO: X3DH full exchange p95 < 50 ms
    assert!(p95 < 50.0, "SLO breached: X3DH p95 latency exceeds 50ms");
}

#[test]
fn test_stress_double_ratchet_conversation_soak() {
    let alice_ks = KeyStore::generate();
    let mut bob_ks = KeyStore::generate();
    let bob_bundle = bob_ks.export_bundle();

    let now = 1000u64;
    let (mut alice_session, init_env) = Session::initiate_outbound(
        &alice_ks,
        &bob_bundle,
        b"Initial Handshake",
        [0xAA; 16],
        now,
        3600,
    )
    .unwrap();

    let (mut bob_session, pt_init) = Session::respond_inbound(&mut bob_ks, &init_env, now).unwrap();
    assert_eq!(pt_init, b"Initial Handshake");

    let rounds = 500;
    let start = Instant::now();

    for r in 0..rounds {
        let msg_time = now + 1 + (r as u64 * 2);
        // Alice -> Bob
        let msg_a = format!("Ping from Alice round {}", r).into_bytes();
        let env_a = alice_session
            .encrypt_message(&msg_a, [0xAA; 16], msg_time, 3600)
            .unwrap();
        let pt_b = bob_session.decrypt_message(&env_a, msg_time).unwrap();
        assert_eq!(pt_b, msg_a);

        // Bob -> Alice
        let msg_b = format!("Pong from Bob round {}", r).into_bytes();
        let env_b = bob_session
            .encrypt_message(&msg_b, [0xAA; 16], msg_time + 1, 3600)
            .unwrap();
        let pt_a = alice_session.decrypt_message(&env_b, msg_time + 1).unwrap();
        assert_eq!(pt_a, msg_b);
    }

    let duration = start.elapsed();
    let msgs_per_sec = (rounds * 2) as f64 / duration.as_secs_f64();
    println!(
        "[STRESS] Double Ratchet Soak: {} messages in {:.2?} ({:.0} msg/sec)",
        rounds * 2,
        duration,
        msgs_per_sec
    );
}
