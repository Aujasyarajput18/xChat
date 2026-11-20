# xChat

**A serverless, peer-to-peer encrypted messenger written in Rust, designed to keep working over Bluetooth LE, local Wi-Fi and LAN when the internet is down.**

There are no accounts, phone numbers or servers. Each device is identified by a key-derived address such as `xchat_b770179928483d97`, and messages are end-to-end encrypted with X3DH key agreement and the Double Ratchet (ChaCha20-Poly1305, HKDF-SHA-256, Ed25519/X25519).

> **Status: research prototype (v0.1.0).** The cryptographic core and the command-line tools work and are well tested. The browser/Android app is still being hardened. Read [Project status](#project-status) before you trust it with anything real.

---

## Contents

- [Project status](#project-status)
- [Quick start](#quick-start)
- [Command-line usage](#command-line-usage)
- [The app (GUI)](#the-app-gui)
- [Architecture](#architecture)
- [Cryptography](#cryptography)
- [Repository layout](#repository-layout)
- [Development](#development)
- [Files, ports and data](#files-ports-and-data)
- [Contributing](#contributing)
- [License](#license)

---

## Project status

This section reflects the code on `main` as tested on Linux with Rust 1.97 (two app instances, a headless browser and every API route exercised by hand).

### Works today

| Area | Notes |
| --- | --- |
| Crypto library (`xchat-crypto`) | X3DH, Double Ratchet with skipped-key handling, ChaCha20-Poly1305 with bound associated data, HKDF, bucketed padding and zeroize. |
| Core protocol (`xchat-core`) | Wire envelopes, relay-blind routing header, replay filter, store-and-forward outbox and a session coordinator. |
| Test suite | 76 tests pass: unit, integration, fuzz/tamper, concurrency, chaos, mutation and stress tests. |
| `simulate-mesh`, `test-two-nodes` | End-to-end demos that encrypt, relay, decrypt and reply. Replay and tampering are rejected as expected. |
| `daemon` + `send` | Real encrypted send over TCP. The receiving daemon decrypts and prints the message. |
| `keygen`, `profile`, `contact`, `discover` | Identity and profile management, plus UDP peer discovery on port 9444. |
| `mixnet`, `sms`, `audio-modem` | Working encode/decode demonstrations of each carrier. |
| GUI basics | Loads cleanly, discovers nearby instances, sends to a contact added by `host:port`, and edits the profile. |

### Known limitations

These are tracked as open work. Until they are fixed, **the GUI does not provide the security properties described below.**

- The GUI messaging path uses a static Diffie-Hellman key rather than the Double Ratchet. It also includes a plaintext copy of every message in the envelope it sends.
- `/api/receive` accepts unsigned messages and replayed envelopes.
- The GUI server sends `Access-Control-Allow-Origin: *` and has no authentication, so any web page can call its API.
- The keystore is encrypted with a key compiled into the binary, and its file is created world-readable.
- One-time prekeys are not consumed after use.
- The GUI and `daemon` bind to `127.0.0.1`, so two different machines cannot reach each other yet.
- Tapping a "People nearby" peer saves an unusable target, and replies to inbound senders fail. In both cases the UI still shows the message as delivered.
- `ping`, `sos` and the GUI's ping, SOS and Rooms actions are placeholders that report success without sending anything.
- The relay daemon stores envelopes but never delivers them onward.
- GUI contacts and chat history are lost when the app restarts.
- `cargo fmt --check` and `cargo clippy -D warnings` currently fail. A few small fixes are needed.

**Not yet verified:** physical BLE on real radios (`btleplug` on desktop, `BluetoothMeshManager.java` on Android), the Android APK build, and the macOS and Windows launchers.

---

## Quick start

### Prerequisites

- Stable Rust via [rustup](https://rustup.rs/). The project uses edition 2021 and was tested with 1.97.
- **Linux only:** D-Bus headers for the Bluetooth library:
  ```bash
  sudo apt-get install -y libdbus-1-dev pkg-config
  ```

### Build and test

```bash
git clone https://github.com/EdgeRunner-runit/xChat.git
cd xChat
cargo build --workspace
cargo test --workspace
```

The binary is built as `target/debug/xchat-cli` (or `target/release/xchat-cli`). Every command below can also be run through Cargo:

```bash
cargo run -- <command>
```

### See it work in 30 seconds

```bash
cargo run -- simulate-mesh     # offline send → blind relay → BLE-to-TCP handover → decrypt → reply → attack checks
cargo run -- test-two-nodes    # two nodes over real TCP sockets with X3DH + Double Ratchet
```

---

## Command-line usage

```text
xchat-cli <COMMAND>

  app | gui        Launch the browser GUI (default port 8080)
  keygen           Create an identity and print its prekey bundle
  profile          Show or set @username, display name and invite link
  contact          add | list | remove saved contacts
  daemon           Run a peer listener, or a blind relay with --relay
  send             Encrypt and send one message to a peer over TCP
  discover         Listen for nearby peers on UDP 9444
  simulate-mesh    Full multi-transport simulation
  test-two-nodes   Two-node live socket test
  mixnet           Chaumian mix pool and cover-traffic demo
  sms              Split an encrypted envelope into SMS frames and reassemble it
  audio-modem      Encode an envelope as AFSK tones and decode it
  sos              Build an emergency beacon envelope (placeholder, see status)
  burner           Create a RAM-only room token (placeholder, see status)
  ping             Placeholder (see status)
```

### Create an identity

```bash
cargo run -- keygen
```

This writes `~/.xchat_keystore.bin` and prints your address, fingerprint and a JSON prekey bundle to give to peers.

### Set a username and get an invite link

```bash
cargo run -- profile --username @alice --display-name "Alice" --invite
```

### Send an encrypted message between two nodes

Run each side with its own `HOME` so the two identities stay separate:

```bash
# Terminal 1: receiver
HOME=/tmp/bob cargo run -- keygen > /tmp/bob/keygen.txt   # copy the JSON bundle into /tmp/bob/bundle.json
HOME=/tmp/bob cargo run -- daemon --port 7001

# Terminal 2: sender
HOME=/tmp/alice cargo run -- send --target 127.0.0.1:7001 \
  --bundle /tmp/bob/bundle.json --message "hello bob"
```

The daemon prints `📩 New Session! Message from xchat_…: "hello bob"`. Run `mkdir -p /tmp/bob /tmp/alice` first.

### Run a blind relay

```bash
cargo run -- daemon --port 7000 --relay
```

The relay logs each envelope's ID, destination token and hop count, but never sees any plaintext.

---

## The app (GUI)

```bash
cargo run -- app             # or: cargo run -- app --port 8081
```

This opens `http://localhost:8080`. The frontend is a single file, `android/app/src/main/assets/index.html`. It is embedded into the Rust binary at compile time and also shipped as the Android WebView asset.

What you'll find:

- **Chats:** contact list, search, messages, and a safety-code (emoji fingerprint) check.
- **People nearby:** peers found over UDP discovery.
- **Ways to send:** status of each carrier (Wi-Fi, mixnet, Bluetooth, SMS, sound).
- **Rooms:** burner-room UI.
- **My profile:** username, avatar color, QR code share and scan.
- **SOS:** hold-to-confirm emergency alert.

To try two instances on one machine:

```bash
HOME=/tmp/a cargo run -- app --port 8081
HOME=/tmp/b cargo run -- app --port 8082
```

In the first instance, open **New chat**, enter `127.0.0.1:8082`, and send a message.

Running two copies at once can corrupt them, because they share `$HOME/.xchat_*` files. Always give each instance its own `HOME`.

For frontend work, `scripts/live-app-server.py` serves `index.html` straight from disk and proxies `/api/*` to a running node, so you don't have to rebuild after every HTML change.

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│ UI: CLI (xchat-cli) · browser GUI · Android WebView      │
├──────────────────────────────────────────────────────────┤
│ Session: X3DH + Double Ratchet (xchat-core::session)     │
├──────────────────────────────────────────────────────────┤
│ Identity & keystore · contacts · profile                 │
├──────────────────────────────────────────────────────────┤
│ Envelope · replay filter · outbox · relay · mix pool     │
├──────────────────────────────────────────────────────────┤
│ Transport trait + manager (failover)                     │
├───────────┬───────────┬──────────────┬───────────┬───────┤
│ TCP / LAN │ BLE       │ BLE mesh sim │ SMS frame │ AFSK  │
│           │ (btleplug)│              │           │ audio │
└───────────┴───────────┴──────────────┴───────────┴───────┘
```

The guiding rule is that **cryptography sits above transport.** A transport only ever moves an opaque `EncryptedEnvelope`. Relays read only the routing header (an ephemeral destination token, TTL, message ID and hop count). Because ratchet state is independent of any socket, a conversation can move from BLE to TCP without being reset.

**Life of a message (CLI path):**

1. The sender fetches or holds the recipient's prekey bundle and runs X3DH.
2. The plaintext is padded to a size bucket, then encrypted with a Double Ratchet message key using ChaCha20-Poly1305, with the routing header as associated data.
3. The envelope is queued in the outbox and sent over whichever transport is available, either directly or through a blind relay.
4. The recipient checks the replay filter, decrypts, advances the ratchet and zeroizes the used key.

The design is specified in full in [`docs/SECURITY_ARCHITECTURE.md`](docs/SECURITY_ARCHITECTURE.md) and [`docs/ARCHITECTURE_MASTER.md`](docs/ARCHITECTURE_MASTER.md).

---

## Cryptography

| Purpose | Primitive |
| --- | --- |
| Identity signing | Ed25519 (`ed25519-dalek`) |
| Key agreement | X25519 (`x25519-dalek`): identity key, signed prekey, one-time prekeys, ephemeral ratchet keys |
| Initial handshake | X3DH |
| Per-message keys | Double Ratchet with bounded skipped-key cache |
| KDF | HKDF-SHA-256 with domain-separation labels |
| AEAD | ChaCha20-Poly1305 with associated data bound to the routing header |
| Length hiding | Bucketed padding |
| Memory hygiene | `zeroize` on secret types |

References: [Signal X3DH](https://signal.org/docs/specifications/x3dh/), [Signal Double Ratchet](https://signal.org/docs/specifications/doubleratchet/), [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439), [RFC 7748](https://www.rfc-editor.org/rfc/rfc7748), [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032), [RFC 5869](https://www.rfc-editor.org/rfc/rfc5869).

This code has **not** been independently audited.

---

## Repository layout

```
crates/
  xchat-crypto/      identity, x3dh, double_ratchet, aead, kdf, padding
  xchat-core/        envelope, session, keystore, outbox, replay, relay, mixnet,
                     contacts, profile, sos, burner, duress, chunking, cover_traffic
                     tests/  integration · fuzz_and_tamper · concurrency_race ·
                             chaos_fault · mutation_quality · stress_load
  xchat-transport/   traits, manager, tcp, ble (chunking), ble_central (btleplug),
                     mesh_sim, discovery (UDP), sms, audio_modem, mixnet_transport
  xchat-cli/         main.rs (all commands) · gui.rs (embedded HTTP server + API)
android/             Android app: WebView + AndroidBridge + BluetoothMeshManager (minSdk 21)
docs/                Security architecture, architecture master doc, user manual
dbms/                Relational storage design (normalization, ER diagram, schema, query tuning)
sandbox/             Isolated security-assessment harness (Makefile + run script)
scripts/             build-apk.sh, package-release.sh, live-app-server.py, multi-node test scripts
install.sh, xchat-launcher.*, xchat-setup.command, xchat-universal.bat   One-click launchers
task_plan.md, progress.md, findings.md, MASTER_EVALUATION_DOCUMENT.md      Project notes
```

---

## Development

```bash
cargo fmt --check
cargo clippy --workspace --tests -- -D warnings
cargo test --workspace

# Individual suites (as run in CI)
cargo test --test fuzz_and_tamper_tests -- --nocapture
cargo test --test concurrency_race_tests -- --nocapture
cargo test --test chaos_fault_tests -- --nocapture
cargo test --test mutation_quality_tests -- --nocapture
cargo test --test stress_load_tests --release -- --nocapture
```

CI (`.github/workflows/ci.yml`) runs all of the above on Ubuntu and also builds the Android APK.

### Android APK

```bash
export ANDROID_HOME=/path/to/Android/sdk
./scripts/build-apk.sh        # outputs dist/xchat.apk and dist/xchat.apk.sha256
```

### macOS release packages

```bash
./scripts/package-release.sh  # builds arm64 + x86_64 and lipo'd universal tarballs into dist/
```

The source-build path in `install.sh` and `xchat-launcher.sh` copies `target/release/xchat`, but Cargo produces `xchat-cli`. Until that is fixed, use `cargo run` or copy the binary yourself.

---

## Files, ports and data

| Item | Default |
| --- | --- |
| Keystore | `~/.xchat_keystore.bin` |
| Profile | `~/.xchat_profile.json` |
| CLI contacts | `~/.xchat_contacts.bin` |
| GUI | TCP `8080` (`--port`) |
| Daemon / relay | TCP `7000` (`--port`) |
| Peer discovery | UDP `9444` |
| BLE service UUID | `a495ff10-c5b1-4b44-b512-1370f02d74de` |

To start fresh, delete the three `~/.xchat_*` files.

---

## Contributing

1. Fork the repo and create a branch.
2. Make your change and add tests.
3. Make sure `cargo fmt --check`, `cargo clippy --workspace --tests -- -D warnings` and `cargo test --workspace` all pass.
4. Open a pull request that explains what changed and why.

The [known limitations](#known-limitations) are a good place to start. Please report security issues privately through the repository's **Security** tab, not as public issues.

---

## License

The workspace manifest declares **MIT OR Apache-2.0**, but no `LICENSE` file is checked in yet. Add `LICENSE-MIT` and `LICENSE-APACHE` to make this explicit. Third-party icon notices (Lucide, Tabler) are in [`LICENSES/THIRD_PARTY.md`](LICENSES/THIRD_PARTY.md).
