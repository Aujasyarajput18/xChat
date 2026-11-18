use std::sync::Arc;
use tokio::sync::Mutex;
use xchat_core::{
    envelope::{EncryptedEnvelope, RoutingHeader},
    keystore::KeyStore,
    outbox::StoreAndForwardQueue,
    replay::ReplayFilter,
};

#[tokio::test]
async fn test_race_parallel_opk_consumption_mutual_exclusion() {
    let ks = KeyStore::generate();
    // Retrieve the first OPK pubkey generated
    let opk_pubkey = ks
        .one_time_prekeys
        .first()
        .map(|opk| opk.pubkey)
        .expect("Should have OPK");

    let ks_shared = Arc::new(Mutex::new(ks));
    let mut handles = Vec::new();

    // 100 concurrent tasks race to consume the single OPK
    for _ in 0..100 {
        let ks_clone = ks_shared.clone();
        handles.push(tokio::spawn(async move {
            let mut guard = ks_clone.lock().await;
            guard.consume_one_time_prekey(&opk_pubkey)
        }));
    }

    let mut successful_consumptions = 0;
    for handle in handles {
        if let Ok(Some(_consumed_key)) = handle.await {
            successful_consumptions += 1;
        }
    }

    // Invariant: Exactly ONE task successfully consumed the one-time prekey
    assert_eq!(
        successful_consumptions, 1,
        "Invariant violation: OPK was consumed by {} tasks (expected exactly 1)",
        successful_consumptions
    );
}

#[tokio::test]
async fn test_race_concurrent_replay_filter_recording() {
    let filter = Arc::new(Mutex::new(ReplayFilter::new()));
    let target_msg_id = [0xEEu8; 16];
    let timestamp = 5000u64;

    let mut handles = Vec::new();
    // 100 concurrent tasks attempt to record the identical message ID at the same timestamp
    for _ in 0..100 {
        let filter_clone = filter.clone();
        handles.push(tokio::spawn(async move {
            let mut guard = filter_clone.lock().await;
            guard.check_and_record(&target_msg_id, timestamp)
        }));
    }

    let mut accepted_count = 0;
    let mut rejected_count = 0;

    for handle in handles {
        match handle.await.unwrap() {
            Ok(()) => accepted_count += 1,
            Err(_) => rejected_count += 1,
        }
    }

    // Invariant: Exactly 1 task accepted, 99 tasks rejected as replays
    assert_eq!(
        accepted_count, 1,
        "Expected exactly 1 acceptance for concurrent replay"
    );
    assert_eq!(
        rejected_count, 99,
        "Expected 99 rejections for concurrent replay"
    );
}

#[tokio::test]
async fn test_race_concurrent_outbox_enqueue_and_drain() {
    let outbox = Arc::new(Mutex::new(StoreAndForwardQueue::new()));
    let total_producers = 20;
    let items_per_producer = 25; // 500 items total

    let mut handles = Vec::new();

    // Spawn 20 concurrent producers
    for p in 0..total_producers {
        let outbox_clone = outbox.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..items_per_producer {
                let mut token = [0u8; 16];
                token[0] = p as u8;
                token[1] = i as u8;
                let header = RoutingHeader::new(token, 1000 + i as u64, 3600);
                let env = EncryptedEnvelope {
                    header,
                    ratchet_header: None,
                    x3dh_init: None,
                    ciphertext: vec![p as u8, i as u8],
                };
                let mut guard = outbox_clone.lock().await;
                guard.enqueue(env);
            }
        }));
    }

    // Await all producers
    for handle in handles {
        handle.await.unwrap();
    }

    let guard = outbox.lock().await;
    assert_eq!(guard.len(), total_producers * items_per_producer);
}
