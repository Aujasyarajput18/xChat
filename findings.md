# Findings & Architectural Decisions: xChat

## Architectural Decisions
1. **Language & Ecosystem**: Rust 2021 edition. Provides memory safety, strict zeroization (`zeroize`), performant async (`tokio`), and direct portable compilation.
2. **Cryptographic Suite**:
   - Identity: Ed25519 for signatures, X25519 for Diffie-Hellman (RFC 7748 / RFC 8032)
   - Session KEX: X3DH prekey exchange (Identity Key, Signed Prekey, One-Time Prekey)
   - 1:1 Ratchet: Double Ratchet with symmetric KDF chains and DH key exchange
   - AEAD: ChaCha20-Poly1305 (RFC 8439) with 96-bit nonces derived deterministically from ratchet counter
   - KDF: HKDF-SHA-256 (RFC 5869) with explicit domain separation strings
   - Padding: Bucketed power-of-two/bucket tiers (64, 256, 1024, 4096 bytes)
3. **Transport Decoupling**:
   - `Transport` trait handles sending/receiving binary opaque `EncryptedEnvelope`s.
   - The cryptographic session does not maintain any socket references; network disconnects do not reset crypto states.
   - Outbox queue buffers messages and dispatches them across whichever transport is currently active.
4. **Relay Blindness**:
   - Relay nodes inspect only `RoutingHeader` (destination token, TTL, message ID) and never access message payload or identity keys.
5. **Physical Bluetooth LE Mesh Transport Architecture (Zero Simulation)**:
   - **MTU Fragmentation**: Standard BLE ATT MTU is 23 bytes (20 payload bytes). High-speed negotiated MTU reaches 512 bytes. A framing protocol header (`[0x58, 0x43, msg_id: u16, chunk_idx: u16, total_chunks: u16]`) chunks arbitrary encrypted envelopes into physical BLE packets.
   - **Dual Role Operation**: Devices simultaneously operate as a BLE Peripheral (advertising service UUID `a495ff10-c5b1-4b44-b512-1370f02d74de` and hosting GATT server for ingress) and a BLE Central (scanning for nearby mesh beacons and connecting via GATT client for egress).
   - **Zero Internet Requirement**: Completely bypasses cellular towers, internet routers, and Wi-Fi access points. Transmits raw encrypted envelopes directly device-to-device via 2.4 GHz Bluetooth hardware.
