# xChat

**A serverless, peer-to-peer encrypted messenger written in Rust, built to keep working over local Wi-Fi, LAN and Bluetooth LE when the internet is down.**

There are no accounts, phone numbers or servers. Each device is identified by a key-derived address such as `xchat_b770179928483d97`. Every message is end-to-end encrypted with X3DH key agreement and the Double Ratchet, using ChaCha20-Poly1305, HKDF-SHA-256, Ed25519 and X25519.

> **Status: v0.1.0, research prototype.** The protocol, the CLI and the browser app all work end to end and are covered by 105 automated tests. The code has **not** been independently audited, and Bluetooth, Android and real multi-device field use are not yet verified. See [Project status](#project-status).

---

## Contents

- [Features](#features)
- [Project status](#project-status)
- [Quick start](#quick-start)
- [Using the app](#using-the-app)
- [Command-line usage](#command-line-usage)
- [Security model](#security-model)
- [Architecture](#architecture)
- [Repository layout](#repository-layout)
- [Configuration, files and ports](#configuration-files-and-ports)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

---

## Features

- **End-to-end encryption with forward secrecy.** X3DH establishes each session and the Double Ratchet derives a fresh key for every message. One-time prekeys are consumed after use.
- **No servers.** Peers find each other on the local network with a UDP beacon and talk directly over TCP/HTTP.
- **Browser app.** `xchat app` serves a full chat UI with contacts, People Nearby, safety codes, QR invites, Rooms and SOS. The same UI ships as the Android WebView asset.
- **Offline delivery through blind relays.** A relay stores opaque envelopes for a recipient who is offline. Only the recipient can collect them, by proving ownership of the mailbox, and the relay never sees plaintext.
- **Burner rooms.** Password-protected group rooms that live only in RAM. They're never written to disk and disappear when you leave or restart.
- **Emergency SOS.** A signed distress beacon with coordinates, broadcast to every reachable peer. It reports how many peers actually received it.
- **Alternative carriers.** Working encoders and demos for a Chaumian mixnet, SMS framing and an AFSK audio modem.
- **Honest delivery status.** Messages show as delivered only when a peer accepted them. Otherwise you see "failed" and the reason.

---

## Project status

Tested on `main` (`e2d020f`) on Linux with Rust 1.97. I ran three app instances, a capture proxy between them, a headless browser driving the UI, the CLI daemon and relay, and hand-crafted attack requests against every API route.

### Verified working

| Area | What was checked |
| --- | --- |
| Build and CI gates | `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test --workspace` (105 tests) are all green. |
| App messaging | Two-way chat between instances. A capture of the network traffic contains no plaintext. Sessions and history survive a restart. |
| Attack resistance | Unsigned, spoofed, replayed and relabelled messages are rejected or attributed to the true sender. Cross-site requests and DNS-rebinding requests are refused. |
| Nearby discovery | Several instances, even on the same machine, discover each other. Tapping a nearby person opens a working chat without duplicates or renames. |
| Delivery status | An unreachable peer shows "failed". Ping measures real round-trip latency. A node with no peers reports 0 recipients for room sends and SOS. |
| Burner rooms | Join, two-way messaging with no echo, and RAM-only storage (gone after a restart). |
| SOS | Received by every peer with the correct coordinates. Forged beacons are rejected. |
| CLI | `send` → `daemon` with ack/nack and reuse of sessions; a reused one-time prekey is rejected. `send` → relay → `fetch` works for offline delivery, and a different identity can't fetch someone else's mailbox. `simulate-mesh` and `test-two-nodes` pass. |
| Storage | Every file in `~/.xchat_*` is created with mode `0600`. Optional passphrase protection uses Argon2id. |

### Known limitations

- **Not audited.** Don't rely on xChat where your safety depends on it.
- **The storage key sits on the same disk.** By default the at-rest key is a random file (`~/.xchat_device_key`) next to the data it protects. Set `XCHAT_PASSPHRASE` for real protection at rest. OS keychain support isn't implemented yet.
- **Relay mailboxes can be identified.** Mailbox tokens are derived from the recipient's public key and the current hour. Anyone who holds that public key, such as a contact, can recognise which mailbox at a relay belongs to that person.
- **Failed broadcasts still return HTTP 200.** Room sends and SOS with no recipients put `"status":"failed"` in the response body but keep HTTP status 200.

### Not yet verified

- Physical Bluetooth LE: `btleplug` on desktop and `BluetoothMeshManager.java` on Android.
- The Android APK build (`scripts/build-apk.sh`) and the WebView bridge.
- Two physically separate machines on a real Wi-Fi network. Testing used multiple instances on one host over its LAN interface.
- The macOS and Windows one-click launchers.

---

## Quick start

### Prerequisites

- Stable Rust via [rustup](https://rustup.rs/). Tested with Rust 1.97.
- **Linux only:** D-Bus headers for the Bluetooth library:
  ```bash
  sudo apt-get install -y libdbus-1-dev pkg-config
  ```

### Build, test and run

```bash
git clone https://github.com/EdgeRunner-runit/xChat.git
cd xChat
cargo test --workspace          # 105 tests
cargo run -- app                # opens the app at http://localhost:8080
```

The binary is called `xchat` (`target/debug/xchat`, or `target/release/xchat` with `--release`). Every `xchat …` command below can also be run as `cargo run -- …`.

### See the protocol work without any setup

```bash
cargo run -- simulate-mesh     # offline send → blind relay → BLE-to-TCP handover → decrypt → reply → attack checks
cargo run -- test-two-nodes    # two nodes over real TCP sockets with X3DH + Double Ratchet
```

---

## Using the app

```bash
xchat app                       # all interfaces, port 8080
xchat app --port 8081           # different port
xchat app --bind 127.0.0.1      # this machine only (no LAN peers)
```

Open the address it prints in your browser. Your **address** (for example `192.168.1.20:8080`) appears at the top left; share it, or your QR invite, with the other person.

| Screen | What it does |
| --- | --- |
| **Chats** | Your conversations, plus **Saved Messages** (private notes that never leave the device) and the **AI helper** test chat. |
| **People nearby** | Other xChat apps on the same network. Tap one to start chatting. |
| **New chat (+)** | Add someone by `ip:port`, invite link or QR code. |
| **Safety code** (shield icon in a chat) | Compare the emoji fingerprint in person to rule out a man-in-the-middle. |
| **Rooms** | Join a burner room by name and password. Everyone with the same name and password is in the room. Messages live only in RAM. |
| **Ways to send** | Status of each carrier: Wi-Fi/LAN, mixnet, Bluetooth, SMS and sound. |
| **SOS** (red button) | Hold to send an emergency beacon with your location to everyone nearby. |
| **My profile** | Username, display name, avatar colour and QR invite. |

**Two instances on one computer.** Give each one its own `HOME` so they don't share key files:

```bash
HOME=/tmp/alice xchat app --port 8081
HOME=/tmp/bob   xchat app --port 8082
```

They'll find each other under **People nearby**.

**Frontend development.** The UI is a single file, `android/app/src/main/assets/index.html`, compiled into the binary. `scripts/live-app-server.py` serves it from disk and proxies `/api/*` to a running node, so you can edit the HTML without rebuilding.

---

## Command-line usage

```text
xchat <COMMAND>

  app | gui        Launch the browser app
  keygen           Create an identity and print its prekey bundle
  profile          Show or set @username, display name, invite link
  contact          add | list | remove saved contacts
  send             Encrypt and send a message to a peer over TCP
  daemon           Run a peer listener, or a blind relay with --relay
  fetch            Collect and decrypt your messages from a blind relay
  discover         Listen for nearby peers
  ping             Measure reachability and latency to a contact
  sos              Broadcast a signed emergency beacon
  burner           Join a RAM-only room and chat from the terminal
  simulate-mesh    Full multi-transport simulation
  test-two-nodes   Two-node live socket test
  mixnet | sms | audio-modem   Carrier demos
```

### Identity and profile

```bash
xchat keygen                                              # creates ~/.xchat_keystore.bin, prints your bundle
xchat profile --username @alice --display-name "Alice" --invite
```

### Direct encrypted message

```bash
# Bob (receiver): start a listener. It prints Bob's prekey bundle; save it as bob.json and give it to Alice.
xchat daemon --port 7000

# Alice (sender)
xchat send --target 192.168.1.15:7000 --bundle bob.json --message "hello bob"
```

The daemon prints the decrypted message. `send` exits with `[✓]` when the daemon acknowledges the message and `[✗] … (nack)` (exit code 1) when it rejects it.

### Offline delivery through a relay

```bash
# Relay operator
xchat daemon --port 7000 --relay

# Alice sends to the relay while Bob is offline
xchat send --target relay.host:7000 --bundle bob.json --message "see you at 6"

# Bob, later
xchat fetch --relay relay.host:7000
```

The relay stores only opaque envelopes, keyed by a mailbox token that rotates every hour. A fetch has to prove it owns the mailbox, and fetched envelopes are removed from the relay.

### Rooms, SOS and ping

```bash
xchat burner --room ops --pass whisper-delta-99
xchat sos --lat 12.9716 --lon 77.5946 --message "Need help at gate 3"
xchat ping Alice
```

---

## Security model

| Property | How it's provided |
| --- | --- |
| Confidentiality | X3DH + Double Ratchet. ChaCha20-Poly1305 with the routing header bound as associated data. Bucketed padding hides message length. |
| Forward and post-compromise secrecy | A new message key per message, DH ratchet steps, one-time prekeys consumed after use, `zeroize` on secrets. |
| Sender authenticity | The sender is identified only by the identity key proven in the handshake. Unauthenticated fields such as alias and socket are never used to pick a conversation or a reply address. |
| Replay protection | A persistent replay filter keyed on message IDs. |
| Local API protection | A per-launch token is required on every UI route. Cross-origin requests and unexpected `Host` headers are refused. Peer routes (`/api/identity`, `/api/bundle`, `/api/prekey`, `/api/receive`) are protected cryptographically instead. |
| At-rest storage | Keystore, sessions, contacts, messages and the replay filter are encrypted and saved with mode `0600`. With `XCHAT_PASSPHRASE`, the key is derived with Argon2id. |
| Relay privacy | Relays see only opaque envelopes and hourly mailbox tokens. Collecting a mailbox requires a cryptographic ownership proof. |

**Primitives:** Ed25519 (`ed25519-dalek`), X25519 (`x25519-dalek`), HKDF-SHA-256, ChaCha20-Poly1305 and Argon2id.
**References:** [Signal X3DH](https://signal.org/docs/specifications/x3dh/), [Double Ratchet](https://signal.org/docs/specifications/doubleratchet/), [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439), [RFC 7748](https://www.rfc-editor.org/rfc/rfc7748), [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032), [RFC 5869](https://www.rfc-editor.org/rfc/rfc5869), [RFC 9106](https://www.rfc-editor.org/rfc/rfc9106).

Full details are in [`docs/SECURITY_ARCHITECTURE.md`](docs/SECURITY_ARCHITECTURE.md).

**Reporting vulnerabilities:** please use the repository's **Security** tab (private advisory), not a public issue.

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│ UI: CLI (xchat) · browser app · Android WebView          │
├──────────────────────────────────────────────────────────┤
│ Session: X3DH + Double Ratchet (xchat-core::session)     │
├──────────────────────────────────────────────────────────┤
│ Identity & keystore · contacts · profile · replay filter │
├──────────────────────────────────────────────────────────┤
│ Envelope · outbox · blind relay · mix pool · SOS · rooms │
├──────────────────────────────────────────────────────────┤
│ Transport trait + manager (failover)                     │
├───────────┬───────────┬──────────────┬───────────┬───────┤
│ TCP / LAN │ BLE       │ UDP discovery│ SMS frame │ AFSK  │
│           │ (btleplug)│              │           │ audio │
└───────────┴───────────┴──────────────┴───────────┴───────┘
```

**Cryptography sits above transport.** A transport only ever moves an opaque `EncryptedEnvelope`. Relays read only the routing header: destination token, TTL, message ID and hop count. Ratchet state doesn't depend on any socket, so a conversation can move between carriers without being reset.

**Life of a message in the app:**

1. On first contact, the sender fetches the peer's prekey bundle from `/api/bundle` and runs X3DH. Later messages reuse the ratchet session.
2. The plaintext is padded and encrypted with that message's ratchet key, then POSTed to the peer's `/api/receive` as a signed envelope.
3. The receiver checks the replay filter, decrypts, identifies the sender by the key proven in the handshake, and files the message under that address.
4. Delivery status goes back to the UI: delivered, or failed with a reason.

More detail: [`docs/ARCHITECTURE_MASTER.md`](docs/ARCHITECTURE_MASTER.md) · user guide: [`docs/USER_MANUAL.md`](docs/USER_MANUAL.md).

### HTTP API (served by `xchat app`)

| Route | Auth | Purpose |
| --- | --- | --- |
| `GET /api/identity`, `GET /api/bundle`, `GET /api/prekey` | none (peer) | Public identity and prekey bundle |
| `POST /api/receive` | cryptographic | Inbound envelopes from peers |
| `GET/POST /api/contacts`, `GET /api/messages?peer=` | UI token | Contacts and conversation history |
| `POST /api/send`, `POST /api/ping`, `POST /api/sos` | UI token | Send, latency check, emergency beacon |
| `POST /api/burner/join`, `POST /api/burner/leave` | UI token | Burner rooms |
| `GET /api/discovered` | UI token | Nearby peers (excludes this node) |
| `GET/POST /api/profile`, `POST /api/keygen` | UI token | Profile and identity reset |

The UI token is injected into the served page as `<meta name="xchat-csrf-token">` and sent back in the `X-XChat-Token` header.

---

## Repository layout

```
crates/
  xchat-crypto/      identity, x3dh, double_ratchet, aead, kdf, padding
  xchat-core/        envelope, session, keystore, outbox, replay, relay, mixnet,
                     contacts, profile, sos, burner, duress, chunking, cover_traffic
                     tests/  integration · fuzz_and_tamper · concurrency_race ·
                             chaos_fault · mutation_quality · stress_load
  xchat-transport/   traits, manager, tcp (incl. relay fetch), ble, ble_central,
                     mesh_sim, discovery (UDP), sms, audio_modem, mixnet_transport
  xchat-cli/         main.rs (all commands) · gui.rs (HTTP server, API, live tests)
android/             Android app: WebView + AndroidBridge + BluetoothMeshManager (minSdk 21)
docs/                Security architecture, architecture master doc, user manual
dbms/                Relational storage design (normalization, ER diagram, schema, query tuning)
sandbox/             Isolated security-assessment harness
scripts/             build-apk.sh, package-release.sh, live-app-server.py, multi-node scripts
install.sh, xchat-launcher.*, xchat-setup.command, xchat-universal.bat   One-click launchers
```

---

## Configuration, files and ports

### Environment variables

| Variable | Effect |
| --- | --- |
| `XCHAT_PASSPHRASE` | Derive the at-rest storage key from a passphrase with Argon2id (recommended). |
| `XCHAT_SALT_PATH` | Where to store the Argon2id salt (default `~/.xchat_keystore.salt`). |
| `XCHAT_STORAGE_KEY` | Supply the 32-byte storage key directly, as hex. |
| `XCHAT_DEVICE_KEY_PATH` | Location of the random device key used when no passphrase is set. |
| `HOME` | All data lives in `$HOME/.xchat_*`. Change it to run several identities side by side. |

### Data files (all mode `0600`)

| File | Contents |
| --- | --- |
| `~/.xchat_keystore.bin` | Identity, signed prekey and one-time prekeys |
| `~/.xchat_sessions.bin` | Double Ratchet sessions |
| `~/.xchat_contacts.bin` | Contacts (burner rooms excluded) |
| `~/.xchat_messages.bin` | Conversation history (burner rooms excluded) |
| `~/.xchat_replay.bin` | Replay filter |
| `~/.xchat_profile.json` | Username, display name, avatar |
| `~/.xchat_device_key` / `~/.xchat_keystore.salt` | Storage key, or Argon2id salt |

To start completely fresh, delete `~/.xchat_*`.

### Ports

| Port | Use |
| --- | --- |
| TCP `8080` | App and peer API (`xchat app --port`) |
| TCP `7000` | Daemon or relay (`xchat daemon --port`) |
| UDP `9444` | Nearby discovery beacon (port reuse enabled, so several instances can share it) |
| BLE service `a495ff10-c5b1-4b44-b512-1370f02d74de` | Bluetooth mesh |

Open TCP 8080 (or your chosen port) and UDP 9444 in your firewall so other devices on the network can reach you.

---

## Development

```bash
cargo fmt --check
cargo clippy --workspace --tests -- -D warnings
cargo test --workspace
```

The app's API tests in `crates/xchat-cli/src/gui.rs` start real servers on ephemeral ports and make real HTTP requests. CI (`.github/workflows/ci.yml`) runs fmt, clippy, the full test suite, and each stress, chaos, fuzz and concurrency suite separately on Ubuntu, then builds the Android APK.

```bash
# Android APK (needs the Android SDK)
export ANDROID_HOME=/path/to/Android/sdk
./scripts/build-apk.sh               # → dist/xchat.apk + .sha256

# macOS release tarballs (arm64, x86_64, universal)
./scripts/package-release.sh         # → dist/
```

The one-click launchers (`install.sh`, `xchat-launcher.sh`, and others) look for a local build first. Otherwise they download releases from `EdgeRunner-runit/xChat_AP_2026`.

---

## Contributing

1. Fork the repo and create a branch.
2. Make your change and add tests. API changes need a live-HTTP test in `gui.rs`.
3. Make sure fmt, clippy and the full test suite all pass.
4. Open a pull request that explains what changed and why.

Good places to start: OS keychain storage, verifying BLE on real hardware, a two-machine field test, and unlinkable relay mailbox tokens (see [Known limitations](#known-limitations)).

---

## License

The workspace manifest declares **MIT OR Apache-2.0**, but no `LICENSE` file is checked in yet. Add `LICENSE-MIT` and `LICENSE-APACHE` to make it explicit. Third-party icon notices (Lucide, Tabler) are in [`LICENSES/THIRD_PARTY.md`](LICENSES/THIRD_PARTY.md). Third-party icon notices (Lucide, Tabler) are in [`LICENSES/THIRD_PARTY.md`](LICENSES/THIRD_PARTY.md).
