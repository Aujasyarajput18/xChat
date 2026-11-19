#!/usr/bin/env bash
set -euo pipefail

echo "==============================================================================="
echo "   xChat Username System & UDP Beacon Discovery Verification Test"
echo "==============================================================================="

PORT1=9651
PORT2=9652
BASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$BASE_DIR"

echo "[*] Compiling xchat-cli binary..."
cargo build --release --bin xchat-cli

BIN="$BASE_DIR/target/release/xchat-cli"

echo "[*] Spawning Node 1 (Alice) on port $PORT1..."
$BIN app --port $PORT1 > /dev/null 2>&1 &
PID1=$!

echo "[*] Spawning Node 2 (Bob) on port $PORT2..."
$BIN app --port $PORT2 > /dev/null 2>&1 &
PID2=$!

cleanup() {
    echo "[*] Stopping test nodes (PID1=$PID1, PID2=$PID2)..."
    kill -9 $PID1 2>/dev/null || true
    kill -9 $PID2 2>/dev/null || true
}
trap cleanup EXIT

echo "[*] Waiting for both application nodes to bind..."
sleep 2

# 1. Verify Node 1 Identity & Profile API
echo "[*] Step 1: Testing Node 1 /api/profile endpoint..."
PROF1=$(curl -s "http://127.0.0.1:$PORT1/api/profile")
echo "    Node 1 Profile: $PROF1"
if [[ "$PROF1" != *"\"username\""* ]]; then
    echo "[-] FAILED: Node 1 profile does not contain username"
    exit 1
fi
echo "    [✓] Node 1 Profile API verified!"

# 2. Update Profiles with Custom @handles
echo "[*] Step 2: Customizing Node 1 (@alice_prime) and Node 2 (@bob_matrix)..."
curl -s -X POST "http://127.0.0.1:$PORT1/api/profile" \
    -H "Content-Type: application/json" \
    -d '{"username":"@alice_prime","display_name":"Alice Prime","bio":"Sovereign Mesh Operator"}'

curl -s -X POST "http://127.0.0.1:$PORT2/api/profile" \
    -H "Content-Type: application/json" \
    -d '{"username":"@bob_matrix","display_name":"Bob Matrix","bio":"Cypherpunk Node"}'

sleep 1

# 3. Verify Updated Profile Handles
echo "[*] Step 3: Verifying normalized @username handles on both nodes..."
PROF1_UPDATED=$(curl -s "http://127.0.0.1:$PORT1/api/profile")
PROF2_UPDATED=$(curl -s "http://127.0.0.1:$PORT2/api/profile")

if [[ "$PROF1_UPDATED" != *"\"username\":\"@alice_prime\""* ]]; then
    echo "[-] FAILED: Node 1 did not update to @alice_prime: $PROF1_UPDATED"
    exit 1
fi
if [[ "$PROF2_UPDATED" != *"\"username\":\"@bob_matrix\""* ]]; then
    echo "[-] FAILED: Node 2 did not update to @bob_matrix: $PROF2_UPDATED"
    exit 1
fi
echo "    [✓] Both @username handles successfully normalized and stored!"

# 4. Check Discovered Peers Endpoint
echo "[*] Step 4: Querying /api/discovered endpoint on Node 1..."
DISCOVERED1=$(curl -s "http://127.0.0.1:$PORT1/api/discovered")
echo "    Discovered Peers: $DISCOVERED1"
echo "    [✓] Nearby mesh discovery endpoint responded successfully!"

# 5. Connect and Exchange Encrypted Messages
echo "[*] Step 5: Establishing peer link from Alice to Bob (127.0.0.1:$PORT2)..."
curl -s -X POST "http://127.0.0.1:$PORT1/api/contacts" \
    -H "Content-Type: application/json" \
    -d "{\"alias\":\"@bob_matrix\",\"target\":\"127.0.0.1:$PORT2\"}"

TEST_MSG="Cypherpunk hello from @alice_prime to @bob_matrix over local mesh!"
echo "[*] Step 6: Transmitting message from Alice to Bob..."
curl -s -X POST "http://127.0.0.1:$PORT1/api/send" \
    -H "Content-Type: application/json" \
    -d "{\"alias\":\"@bob_matrix\",\"target\":\"127.0.0.1:$PORT2\",\"text\":\"$TEST_MSG\"}"

sleep 1

echo "[*] Step 7: Verifying Bob received message..."
BOB_MSGS=$(curl -s "http://127.0.0.1:$PORT2/api/messages?peer=@alice_prime")
if [[ "$BOB_MSGS" != *"$TEST_MSG"* ]]; then
    # Also check generic peer name fallback
    BOB_MSGS=$(curl -s "http://127.0.0.1:$PORT2/api/messages?peer=Peer")
fi
echo "    Bob Messages: $BOB_MSGS"
if [[ "$BOB_MSGS" != *"$TEST_MSG"* ]]; then
    echo "[-] FAILED: Bob did not receive message from Alice"
    exit 1
fi
echo "    [✓] Message received and decrypted on Bob's node!"

# 8. Bob Replies to Alice
REPLY_MSG="Ratchet step confirmed! Verified encrypted tunnel."
echo "[*] Step 8: Bob transmitting reply back to Alice..."
curl -s -X POST "http://127.0.0.1:$PORT2/api/send" \
    -H "Content-Type: application/json" \
    -d "{\"alias\":\"@alice_prime\",\"target\":\"127.0.0.1:$PORT1\",\"text\":\"$REPLY_MSG\"}"

sleep 1

echo "[*] Step 9: Verifying Alice received reply..."
ALICE_MSGS=$(curl -s "http://127.0.0.1:$PORT1/api/messages?peer=@bob_matrix")
echo "    Alice Messages: $ALICE_MSGS"
if [[ "$ALICE_MSGS" != *"$REPLY_MSG"* ]]; then
    echo "[-] FAILED: Alice did not receive reply from Bob"
    exit 1
fi
echo "    [✓] Reply verified in Alice's message log!"

echo "==============================================================================="
echo "   [SUCCESS] Full @username pairing and two-node P2P exchange verified!"
echo "==============================================================================="
