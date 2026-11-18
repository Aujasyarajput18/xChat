use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};
use xchat_core::envelope::{EncryptedEnvelope, RoutingHeader};
use xchat_crypto::{
    aead::{decrypt_aead, encrypt_aead},
    padding::{pad_message, unpad_message},
};

#[test]
fn test_fuzz_envelope_deserialization_mutations() {
    let mut rng = StdRng::seed_from_u64(0xDEADBEEF);
    let valid_header = RoutingHeader::new([0x42; 16], 1000, 3600);
    let valid_env = EncryptedEnvelope {
        header: valid_header,
        ratchet_header: None,
        x3dh_init: None,
        ciphertext: b"Hello Fuzzing World 1234567890".to_vec(),
    };
    let valid_bytes = valid_env.to_bytes().unwrap();

    // 1000 iterations of random bit-flips, truncations, and random bytes
    for _ in 0..1000 {
        let mut corrupted = valid_bytes.clone();
        let mutation_type = rng.gen_range(0..4);
        match mutation_type {
            0 => {
                // Truncation: slice to random length from 0 to len-1
                let trunc_len = rng.gen_range(0..corrupted.len());
                corrupted.truncate(trunc_len);
            }
            1 => {
                // Bit flips: flip between 1 and 8 random bits
                let flips = rng.gen_range(1..=8);
                for _ in 0..flips {
                    let idx = rng.gen_range(0..corrupted.len());
                    let bit = 1 << rng.gen_range(0..8);
                    corrupted[idx] ^= bit;
                }
            }
            2 => {
                // Byte insertions
                let insert_pos = rng.gen_range(0..corrupted.len());
                let random_byte = rng.gen();
                corrupted.insert(insert_pos, random_byte);
            }
            3 => {
                // Pure random noise of arbitrary length
                let noise_len = rng.gen_range(0..512);
                corrupted = vec![0u8; noise_len];
                rng.fill_bytes(&mut corrupted);
            }
            _ => unreachable!(),
        }

        // Must fail closed (Err) or succeed without panicking
        let result = EncryptedEnvelope::from_bytes(&corrupted);
        if let Ok(recovered) = result {
            // If deserialization unexpectedly succeeds on mutated bytes, header must be within sane bounds
            assert!(recovered.ciphertext.len() <= 10 * 1024 * 1024);
        }
    }
}

#[test]
fn test_fuzz_aead_tag_and_ciphertext_tampering() {
    let mut rng = StdRng::seed_from_u64(0xFEEDFACE);
    let key = [0x55u8; 32];
    let nonce = [0x77u8; 12];
    let aad = b"authenticated-metadata-v1";
    let plaintext = b"Strictly confidential payload that must not be forged!";

    let ciphertext = encrypt_aead(&key, &nonce, plaintext, aad).unwrap();
    assert!(ciphertext.len() > 16); // Tag is 16 bytes

    // 500 iterations of ciphertext tampering
    for _ in 0..500 {
        let mut tampered = ciphertext.clone();
        let flip_idx = rng.gen_range(0..tampered.len());
        let flip_bit = 1 << rng.gen_range(0..8);
        tampered[flip_idx] ^= flip_bit;

        // AEAD verification MUST fail closed
        let res = decrypt_aead(&key, &nonce, &tampered, aad);
        assert!(
            res.is_err(),
            "Decryption succeeded on tampered ciphertext at index {}",
            flip_idx
        );
    }

    // Tampered AAD must fail closed
    for _ in 0..100 {
        let mut tampered_aad = aad.to_vec();
        let flip_idx = rng.gen_range(0..tampered_aad.len());
        tampered_aad[flip_idx] ^= 0x01;

        let res = decrypt_aead(&key, &nonce, &ciphertext, &tampered_aad);
        assert!(res.is_err(), "Decryption succeeded with manipulated AAD");
    }
}

#[test]
fn test_fuzz_padding_unpad_corruptions() {
    let mut rng = StdRng::seed_from_u64(0xBAADF00D);

    for payload_len in [0, 1, 15, 63, 128, 255, 1024] {
        let mut payload = vec![0x41u8; payload_len];
        rng.fill_bytes(&mut payload);

        let padded = pad_message(&payload);
        assert!(padded.len() >= payload.len());

        // Valid unpad roundtrip
        let unpadded = unpad_message(&padded).unwrap();
        assert_eq!(unpadded, payload);

        // Fuzz corrupted padding headers and tails
        for _ in 0..100 {
            let mut corrupted = padded.clone();
            let flip_idx = rng.gen_range(0..corrupted.len());
            corrupted[flip_idx] ^= rng.gen_range(1..=255);

            let res = unpad_message(&corrupted);
            if let Ok(unpadded_corrupted) = res {
                // If it parsed, length must be strictly smaller than padded buffer
                assert!(unpadded_corrupted.len() <= corrupted.len());
            }
        }
    }
}

#[test]
fn test_ttl_timestamp_overflow_boundaries() {
    let extreme_cases = [
        (0u64, 0u64, 0u64),
        (0u64, u64::MAX, 100u64),
        (u64::MAX - 100, 200u64, u64::MAX),
        (u64::MAX, u64::MAX, u64::MAX),
        (1000u64, 3600u64, 4600u64),
        (1000u64, 3600u64, 4601u64),
    ];

    for (created, ttl, now) in extreme_cases {
        let header = RoutingHeader::new([0x01; 16], created, ttl);
        // is_expired must compute without overflow panic (uses saturating_add)
        let _ = header.is_expired(now);
    }
}
