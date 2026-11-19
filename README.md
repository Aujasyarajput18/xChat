<div align="center">

# xChat

### Serverless, offline-first, end-to-end encrypted messaging over Bluetooth Low Energy mesh

[![Release](https://img.shields.io/github/v/release/EdgeRunner-runit/xChat_AP_2026?color=2ea44f&logo=github)](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/EdgeRunner-runit/xChat/blob/main/LICENSE)
[![Build & Test](https://img.shields.io/badge/Build-Passing-brightgreen.svg)](https://github.com/EdgeRunner-runit/xChat/blob/main)
[![Language: Rust](https://img.shields.io/badge/Core-Rust-orange.svg)](https://www.rust-lang.org/)

**[Download](#-download-releases)** · **[Project Website](https://edgerunner-runit.github.io/xChat/)** · **[User Manual](https://github.com/EdgeRunner-runit/xChat/blob/main/USER_MANUAL.md)** · **[Security Architecture](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/SECURITY_ARCHITECTURE.md)**

</div>

---

## Table of Contents

1. [What is xChat?](#what-is-xchat)
2. [Key Features](#key-features)
3. [Download Releases](#-download-releases)
4. [The App (Telegram-Styled GUI)](#️-the-app-telegram-styled-gui)
5. [How It Works: Architecture](#how-it-works-architecture)
6. [Bluetooth Low Energy Mesh Networking](#bluetooth-low-energy-mesh-networking)
7. [Cryptography in Depth](#cryptography-in-depth)
8. [Security Guarantees](#security-guarantees)
9. [Repository Structure](#repository-structure)
10. [Getting Started (Build from Source)](#getting-started-build-from-source)
11. [Using the CLI](#using-the-cli)
12. [Code Quality and Verification](#code-quality-and-verification)
13. [Documentation and External Links](#documentation-and-external-links)
14. [Contributing](#contributing)
15. [License](#license)

---

## What is xChat?

xChat is a **decentralized, peer-to-peer messaging system** that delivers secure, end-to-end encrypted communication across different physical transports **without relying on any central server**.

There are no phone numbers, no email addresses and no accounts. Every user is identified by a **cryptographic address** of the form:

```
xchat_<hex>        e.g.  xchat_b770179928483d97
```

Because the network is serverless and multi-transport, it is highly resistant to censorship, infrastructure outages and surveillance. When the internet is down, xChat keeps working over **Bluetooth Low Energy (BLE) mesh** and local wireless links, with each nearby phone acting as a relay for encrypted traffic it cannot read.

> **Design principle:** *Cryptography sits strictly above the transport layer.*
> Transports only ever carry opaque, encrypted envelopes. Neither relays nor eavesdroppers can see plaintext or long-term user identities.

---

## Key Features

| Feature | Description |
| --- | --- |
| **Offline BLE mesh messaging** | Messages hop from device to device over Bluetooth Low Energy, with no internet or cell tower needed. |
| **End-to-end encryption** | ChaCha20-Poly1305 AEAD protects every message, with associated data bound to the packet. |
| **Double Ratchet** | A fresh key for every message gives forward secrecy and post-compromise (break-in) recovery. |
| **X3DH key agreement** | Start a secure session with a peer who is offline, using published prekey bundles. |
| **Anonymous identities** | Identities are key-derived addresses (`xchat_<hex>`), with no personal data. |
| **Store-and-forward** | Messages are queued in an encrypted outbox and flushed when the recipient is reachable. |
| **Blind relays** | Relay nodes only see ephemeral routing tokens, never plaintext or long-term identity. |
| **Multi-transport failover** | Move between BLE, local Wi-Fi/LAN and TCP/IP without resetting the cryptographic session. |
| **Metadata minimization** | Bucketed padding and rotating discovery tokens resist passive traffic analysis. |
| **Anti-replay protection** | Sliding-window and persistent message-ID tracking reject retransmitted packets. |
| **Emergency SOS beacon** | Instantly broadcast distress coordinates to nearby peers. |
| **Ephemeral burner rooms** | Transient, RAM-only group chats that leave no trace. |
| **Telegram-styled GUI** | A lightweight embedded Web GUI for Android and desktop (`xchat app`). |

---

## 📦 Download Releases

Ready-to-use binaries, the Android APK and 1-click launchers are published on **[GitHub Releases (v0.1.0)](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/tag/v0.1.0)**.

### 📱 Android Standalone App (APK, no Termux required)

- **[xchat-v0.1.0.apk](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-v0.1.0.apk)** (about 25 KB). Installs on Android 5.0 and later (through Android 15/16). Download, tap to install and launch like any normal app. No command line or Termux needed.

### ⚡ 1-Click Universal Auto-Launchers (Desktop and Termux)

Download and run. The launcher detects your device (macOS, Windows, Linux or Android), configures keys, starts the service and opens the app GUI.

| Platform | Launcher | How to start |
| --- | --- | --- |
| **macOS** | [`xchat-setup.command`](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-setup.command) | Double-click in Finder |
| **Windows** | [`xchat-launcher.bat`](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-launcher.bat) or [`xchat-universal.bat`](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-universal.bat) | Double-click |
| **Linux / Android (Termux)** | [`xchat-launcher.sh`](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-launcher.sh) | `chmod +x xchat-launcher.sh && ./xchat-launcher.sh` |

**Universal one-liner (macOS / Linux / Android):**

```bash
curl -fsSL https://raw.githubusercontent.com/EdgeRunner-runit/xChat_AP_2026/main/install.sh | bash
```

> 🔐 Piping a script into `bash` runs it with your privileges. If you prefer, open [`install.sh`](https://github.com/EdgeRunner-runit/xChat/blob/main/install.sh) first and read it before running.

### Standalone Binary Packages

| Package | Target |
| --- | --- |
| [xchat-v0.1.0.apk](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-v0.1.0.apk) | Android APK |
| [xchat-v0.1.0-macos-universal.tar.gz](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-v0.1.0-macos-universal.tar.gz) | macOS universal (Apple Silicon M1–M4 and Intel x86_64) |
| [xchat-v0.1.0-macos-arm64.tar.gz](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-v0.1.0-macos-arm64.tar.gz) | macOS, Apple Silicon only |
| [xchat-v0.1.0-macos-x86_64.tar.gz](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/download/v0.1.0/xchat-v0.1.0-macos-x86_64.tar.gz) | macOS, Intel only |

For **Linux, Windows, Android (Termux) and iOS** instructions, see the [**Complete User Manual**](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/USER_MANUAL.md) and the [**Hardware Field-Testing Playbook**](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/FIELD_TEST_PLAYBOOK.md).

---

## 🖥️ The App (Telegram-Styled GUI)

Launch the lightweight embedded local Web GUI with one command:

```bash
xchat app          # or: xchat gui
```

It opens automatically in your browser at `http://localhost:8080`.

- **🎨 Telegram design language:** dark theme (`#0e1621`), sidebar (`#17212b`), Telegram blue (`#5288c1`), dynamic coloured avatars and double-checkmark read receipts (`✓✓`).
- **🌐 Cross-device interoperability:** connect phone to laptop, laptop to laptop or phone to phone over local Wi-Fi, hotspot or LAN by entering the peer's socket address (for example `192.168.1.45:9400`).
- **Contact book and zero-metadata reachability ping:** test whether a peer answers over local wireless before you chat.
- **Direct messaging:** live ratcheted conversations with offline queuing and Double Ratchet forward secrecy.
- **Emergency SOS beacon:** broadcast distress coordinates in one tap.
- **Ephemeral burner rooms:** untraceable, transient group chats held only in RAM.
- **Relay and crypto diagnostics:** live view of ratchet state, hop counts and memory zeroization.

---

## How It Works: Architecture

The application is built as layers. Each layer talks only to the one below it, so any transport can be swapped without touching the cryptography.

```
┌─────────────────────────────────────────────────────────────┐
│                            UI                               │
├─────────────────────────────────────────────────────────────┤
│                     Messaging Protocol                      │
├─────────────────────────────────────────────────────────────┤
│                 E2EE Session / Group Crypto                 │
├─────────────────────────────────────────────────────────────┤
│                Identity + Key Management                    │
├─────────────────────────────────────────────────────────────┤
│               Message Queue / Store Forward                 │
├─────────────────────────────────────────────────────────────┤
│           Chaumian Mixnet Pool (Shuffle + Cover)            │
├─────────────────────────────────────────────────────────────┤
│                 Transport Abstraction Layer                 │
├──────────────┬──────────────┬──────────────┬────────────────┤
│ Internet/LTE │ Cellular SMS │ Voice Audio  │ Bluetooth Mesh │
│ Mixnet Nodes │ GSM-Safe 140B│ Call Modem   │ & Local Wi-Fi  │
└──────────────┴──────────────┴──────────────┴────────────────┘
```

**Life of a message**

1. You type a message in the UI.
2. The messaging protocol wraps it in a wire envelope.
3. The session layer encrypts it with the Double Ratchet (ChaCha20-Poly1305) and pads it to a fixed-size bucket.
4. The envelope goes into the store-and-forward outbox if the recipient is not reachable yet.
5. The transport layer sends the opaque ciphertext over BLE mesh, local Wi-Fi/LAN or TCP/IP.
6. Blind relay nodes forward it using only an ephemeral routing token.
7. The recipient decrypts it, advances the ratchet and zeroizes the used key material.

The complete technical specification is the formal 50-section document at [`docs/SECURITY_ARCHITECTURE.md`](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/SECURITY_ARCHITECTURE.md).

---

## Bluetooth Low Energy Mesh Networking

BLE is xChat's primary offline transport. It is designed for situations where infrastructure is missing or untrusted: disasters, protests, remote areas and censored networks.

- **Low energy:** BLE keeps battery drain small compared with continuous Wi-Fi, so a phone can stay in the mesh for long periods.
- **Multi-hop relay:** every node can forward packets for peers outside the sender's direct radio range, so the mesh grows with each phone that joins.
- **Blind forwarding:** relays only inspect ephemeral destination routing tokens. They cannot read content or learn long-term identities.
- **Store-and-forward:** if the destination is out of reach, the encrypted envelope waits in a queue and is delivered when a path appears.
- **Transport handover:** a conversation that starts over BLE can continue over TCP on a LAN (or the reverse) while keeping its ratchet state intact. The `simulate-mesh` command demonstrates exactly this.
- **Rotating discovery tokens:** peers advertise short-lived tokens instead of stable identifiers, limiting passive tracking.

> **Note:** the `xchat-transport` crate currently includes a **BLE mesh simulator** for development and testing, alongside the async TCP framing and the failover manager. See the [Field-Testing Playbook](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/FIELD_TEST_PLAYBOOK.md) for real-hardware testing.

---

## Cryptography in Depth

All primitives live in the `xchat-crypto` crate and are used together as one protocol.

| Layer | Algorithm | Purpose |
| --- | --- | --- |
| **Identity signing** | **Ed25519** | Long-term signing key. The public key and its fingerprint form your `xchat_<hex>` address. |
| **Key exchange** | **X25519** (Diffie-Hellman) | Long-term DH key, signed prekey, one-time prekeys and ephemeral ratchet keys. |
| **Initial agreement** | **X3DH** | Asynchronous handshake using a prekey bundle (identity key + signed prekey + one-time prekeys), so you can message someone who is offline. |
| **Session ratchet** | **Double Ratchet** | Derives a new key per message. Gives forward secrecy and break-in recovery. |
| **Key derivation** | **HKDF** | Expands shared secrets into root, chain and message keys. |
| **Message encryption** | **ChaCha20-Poly1305** | AEAD cipher. Encrypts and authenticates each message and binds the associated data (AAD). |
| **Traffic hardening** | **Bucketed padding** | Pads messages to 64, 128, 256, 512 or 1024 bytes to hide their true length. |
| **Memory safety** | **Zeroize** | Wipes key material from memory as soon as it is no longer needed. |

### Key generation

Running `xchat keygen` (or `cargo run -- keygen`) creates:

- an **Ed25519** signing key,
- an **X25519** DH key,
- a **signed prekey**,
- a batch of **one-time prekeys** (10 by default),
- a shareable **prekey bundle** in JSON, plus your device address and identity fingerprint.

Private keys are kept in an **encrypted keystore** (part of `xchat-core`).

### How the ratchet protects you

- **Forward secrecy:** each message uses a new key, and old keys are zeroized right after decryption. Stealing a phone later does not expose past messages.
- **Post-compromise security:** a fresh ephemeral Diffie-Hellman exchange is mixed in as the conversation continues, so a leaked key stops being useful after the next ratchet step.

---

## Security Guarantees

| Area | Guarantee |
| --- | --- |
| **Confidentiality and integrity** | ChaCha20-Poly1305 AEAD with bound Associated Authenticated Data (AAD). |
| **Forward secrecy (FS)** | Per-message Double Ratchet. Old keys are zeroized immediately after decryption. |
| **Break-in recovery (PCS)** | Ephemeral Diffie-Hellman ratcheting restores security after a key compromise. |
| **Offline messaging** | X3DH initial key agreement through published prekey bundles (identity, signed prekey and one-time prekeys). |
| **Transport independence** | Changing physical transport (BLE to TCP) preserves the ratchet state without a reset. |
| **Relay blindness** | Intermediate nodes see only ephemeral destination routing tokens, never plaintext or long-term identities. |
| **Anti-replay** | Sliding window and persistent message-ID tracking reject retransmitted packets. |
| **Metadata minimization** | Bucketed padding (64, 128, 256, 512, 1024 bytes) and rotating ephemeral discovery tokens resist passive traffic analysis. |

The threat model and the reasoning behind each decision are in [`findings.md`](https://github.com/EdgeRunner-runit/xChat/blob/main/findings.md) and the full specification.

---

## Repository Structure

The project is a modular **Rust Cargo workspace** with an Android wrapper, launcher scripts and documentation.

```
xChat/
├── .github/
│   └── workflows/        # CI automation (build, test and lint pipelines)
├── android/
│   └── app/src/main/     # Android app source for the standalone APK build
├── crates/
│   ├── xchat-crypto/     # Cryptographic primitives: Ed25519/X25519 identity, X3DH,
│   │                     # Double Ratchet, ChaCha20-Poly1305 AEAD, HKDF, padding, zeroize
│   ├── xchat-core/       # Wire envelopes, store-and-forward outbox, replay filters,
│   │                     # encrypted keystore, blind relay nodes, session coordinator
│   ├── xchat-transport/  # Transport trait, async TCP framing, BLE mesh simulator,
│   │                     # multi-transport failover manager
│   └── xchat-cli/        # CLI application, node daemon, interactive mesh simulation
├── dbms/                 # Local data storage / database layer
├── docs/
│   ├── SECURITY_ARCHITECTURE.md   # Formal 50-section security architecture spec
│   ├── USER_MANUAL.md             # Complete user manual (all platforms)
│   └── FIELD_TEST_PLAYBOOK.md     # Hardware field-testing playbook
├── scripts/              # Helper and build scripts
├── Cargo.toml            # Workspace manifest
├── Cargo.lock            # Pinned dependency versions
├── install.sh            # Universal one-line installer (macOS / Linux / Android)
├── xchat-launcher.sh     # 1-click launcher: Linux and Android (Termux)
├── xchat-launcher.bat    # 1-click launcher: Windows
├── xchat-universal.bat   # Universal Windows launcher
├── xchat-setup.command   # 1-click launcher: macOS (double-click)
├── USER_MANUAL.md        # User manual (repository root copy)
├── task_plan.md          # Implementation plan and tracking
├── findings.md           # Architectural decisions and threat-model analysis
├── progress.md           # Progress and test-execution log
└── README.md             # You are here
```

### What each part is for

| Path | Contents | Read it if you want to… |
| --- | --- | --- |
| [`crates/xchat-crypto`](https://github.com/EdgeRunner-runit/xChat/tree/main/crates) | All cryptography | Audit the encryption, ratchet and key handling |
| [`crates/xchat-core`](https://github.com/EdgeRunner-runit/xChat/tree/main/crates) | Protocol logic, outbox, keystore, relays | Understand how messages are queued, filtered and relayed |
| [`crates/xchat-transport`](https://github.com/EdgeRunner-runit/xChat/tree/main/crates) | BLE mesh simulator, TCP, failover | Add or test a transport |
| [`crates/xchat-cli`](https://github.com/EdgeRunner-runit/xChat/tree/main/crates) | The `xchat` command and daemon | Run nodes, simulations or the GUI |
| [`android/`](https://github.com/EdgeRunner-runit/xChat/tree/main/android/app/src/main) | Android app source | Build or modify the APK |
| [`dbms/`](https://github.com/EdgeRunner-runit/xChat/tree/main/dbms) | Data storage layer | See how local data is organised |
| [`scripts/`](https://github.com/EdgeRunner-runit/xChat/tree/main/scripts) | Helper scripts | Automate builds and setup |
| [`docs/`](https://github.com/EdgeRunner-runit/xChat/tree/main/docs) | Specification, manual, field-test playbook | Learn the design or test on real hardware |
| [`.github/workflows/`](https://github.com/EdgeRunner-runit/xChat/tree/main/.github/workflows) | CI configuration | Check how builds and tests run |
| [`task_plan.md`](https://github.com/EdgeRunner-runit/xChat/blob/main/task_plan.md) · [`findings.md`](https://github.com/EdgeRunner-runit/xChat/blob/main/findings.md) · [`progress.md`](https://github.com/EdgeRunner-runit/xChat/blob/main/progress.md) | Planning, decisions and test log | Follow the project's history and reasoning |

---

## Getting Started (Build from Source)

### Prerequisites

- [Rust and Cargo](https://rustup.rs/) (edition 2021, Rust 1.80+ recommended)

### Clone and build

```bash
git clone https://github.com/EdgeRunner-runit/xChat.git
cd xChat
cargo build --workspace
```

### Run all unit and integration tests

```bash
cargo test --workspace
```

---

## Using the CLI

### 1. Generate a cryptographic identity

Creates a fresh Ed25519 signing key, X25519 DH key, signed prekey and one-time prekeys:

```bash
cargo run -- keygen
```

Example output:

```
=======================================================
   xChat Cryptographic Identity Key Generation
=======================================================
Device Address:       xchat_b770179928483d97
Identity Fingerprint: b770179928483d97fff0871acb785264f93a7a0730cadd99d6d239777a7a9f52
Ed25519 Sign Pubkey:  b770179928483d97fff0871acb785264f93a7a0730cadd99d6d239777a7a9f52
X25519 DH Pubkey:     fb7e38b246bbbe345c94ea7fe3671d5bd2dc79d4a5d49af10be13f5003c87d1a
Signed Prekey:        5fea0c4c6c2c18b8d4bfef70597ca0602b370ae4f57232f6649808e7815cc71d
Active One-Time Keys: 10
Prekey Bundle (JSON): ...
```

### 2. Run the multi-transport mesh simulation

An end-to-end, multi-node simulation that demonstrates:

- **Offline store-and-forward:** the sender encrypts with X3DH and Double Ratchet while the recipient is offline.
- **Relay blindness:** a blind relay buffers opaque ciphertext without knowing user identities.
- **Transport handover:** the recipient comes online on a TCP LAN and the relay flushes envelopes from BLE to TCP.
- **Bi-directional ratcheting:** the recipient decrypts and replies, advancing the Diffie-Hellman ratchet.
- **Active adversarial defences:** automated checks confirm that replay attacks, ciphertext bit-flips and routing-header tampering are blocked.

```bash
cargo run -- simulate-mesh
```

### 3. Start a blind relay node

Runs an autonomous relay that buffers and routes opaque packets for offline peers:

```bash
cargo run -- daemon --port 7000 --relay
```

### 4. Start a standard peer daemon

Runs a peer listener ready to receive framed encrypted envelopes:

```bash
cargo run -- daemon --port 7001
```

### 5. Launch the GUI

```bash
cargo run -- app
```

---

## Code Quality and Verification

The codebase follows strict Rust quality and safety standards:

```bash
# Check formatting
cargo fmt --check

# Strict linting (zero warnings permitted)
cargo clippy --workspace -- -D warnings

# Run the entire test suite
cargo test --workspace
```

---

## Documentation and External Links

| Resource | Link |
| --- | --- |
| 🌐 Project website | [edgerunner-runit.github.io/xChat](https://edgerunner-runit.github.io/xChat/) |
| 📖 Complete User Manual | [docs/USER_MANUAL.md](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/USER_MANUAL.md) · [root copy](https://github.com/EdgeRunner-runit/xChat/blob/main/USER_MANUAL.md) |
| 🛡️ Security Architecture (50 sections) | [docs/SECURITY_ARCHITECTURE.md](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/SECURITY_ARCHITECTURE.md) |
| 📡 Hardware Field-Testing Playbook | [docs/FIELD_TEST_PLAYBOOK.md](https://github.com/EdgeRunner-runit/xChat/blob/main/docs/FIELD_TEST_PLAYBOOK.md) |
| 📦 Releases and downloads (v0.1.0) | [xChat_AP_2026 Releases](https://github.com/EdgeRunner-runit/xChat_AP_2026/releases/tag/v0.1.0) |
| 🧭 Implementation plan | [task_plan.md](https://github.com/EdgeRunner-runit/xChat/blob/main/task_plan.md) |
| 🔍 Findings and threat model | [findings.md](https://github.com/EdgeRunner-runit/xChat/blob/main/findings.md) |
| 📈 Progress and test log | [progress.md](https://github.com/EdgeRunner-runit/xChat/blob/main/progress.md) |
| 🐛 Report a bug / request a feature | [Issues](https://github.com/EdgeRunner-runit/xChat/issues) |
| 🔀 Propose changes | [Pull requests](https://github.com/EdgeRunner-runit/xChat/pulls) |
| ⚙️ CI status | [Actions](https://github.com/EdgeRunner-runit/xChat/actions) |
| 🔒 Security reports | [Security tab](https://github.com/EdgeRunner-runit/xChat/security) |
| 🦀 Install Rust | [rustup.rs](https://rustup.rs/) |

### Background reading on the protocols

- [Signal: Double Ratchet specification](https://signal.org/docs/specifications/doubleratchet/)
- [Signal: X3DH key agreement specification](https://signal.org/docs/specifications/x3dh/)
- [RFC 8439: ChaCha20 and Poly1305 for IETF protocols](https://www.rfc-editor.org/rfc/rfc8439)
- [RFC 7748: Elliptic curves for security (X25519)](https://www.rfc-editor.org/rfc/rfc7748)
- [RFC 8032: Edwards-curve Digital Signature Algorithm (Ed25519)](https://www.rfc-editor.org/rfc/rfc8032)
- [RFC 5869: HKDF](https://www.rfc-editor.org/rfc/rfc5869)

---

## Contributing

Contributions are welcome.

1. Fork the repository and create a feature branch.
2. Make your change and add tests where it makes sense.
3. Run `cargo fmt --check`, `cargo clippy --workspace -- -D warnings` and `cargo test --workspace`. All three must pass.
4. Open a pull request describing what changed and why.

For bugs and ideas, open an [issue](https://github.com/EdgeRunner-runit/xChat/issues). For anything security-sensitive, please use the [Security tab](https://github.com/EdgeRunner-runit/xChat/security) rather than a public issue.

---

## License

xChat is released under the **MIT License**. See [`LICENSE`](https://github.com/EdgeRunner-runit/xChat/blob/main/LICENSE) for details.

---

<div align="center">

**xChat**: no servers, no phone numbers, no permission needed.

</div>space
```
