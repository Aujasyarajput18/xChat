#!/bin/bash
set -euo pipefail

# scripts/two-device-test.sh
# End-to-end integration test for two xChat devices across isolated Linux network namespaces.

echo "=== xChat Two-Device Network Namespace Test ==="

if [ "$(uname)" != "Linux" ]; then
    echo "⚠️ This test requires Linux network namespaces (ip netns). Current OS: $(uname)"
    echo "Skipping live namespace setup on non-Linux platform."
    exit 0
fi

if [ "$(id -u)" -ne 0 ]; then
    echo "⚠️ Running with sudo privileges..."
    exec sudo "$0" "$@"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_DIR"

# Locate xchat binary
XCHAT_BIN="${REPO_DIR}/target/release/xchat"
if [ ! -f "$XCHAT_BIN" ]; then
    XCHAT_BIN="${REPO_DIR}/target/debug/xchat"
fi
if [ ! -f "$XCHAT_BIN" ]; then
    echo "Building xchat binary..."
    cargo build --release --bin xchat
    XCHAT_BIN="${REPO_DIR}/target/release/xchat"
fi

echo "Using xchat binary: $XCHAT_BIN"

# Cleanup function
cleanup() {
    echo "Cleaning up processes and network namespaces..."
    kill -9 "${PID_A:-}" "${PID_B:-}" 2>/dev/null || true
    ip netns del devA 2>/dev/null || true
    ip netns del devB 2>/dev/null || true
    ip link del br0 2>/dev/null || true
    rm -rf /tmp/dA /tmp/dB
}
trap cleanup EXIT INT TERM

# Initial cleanup
cleanup 2>/dev/null || true

# Set up bridge and namespaces
echo "Setting up bridge br0 and namespaces devA, devB..."
ip netns add devA
ip netns add devB
ip link add br0 type bridge
ip link set br0 up

for n in A B; do
    ip link add veth$n type veth peer name eth0 netns dev$n
    ip link set veth$n master br0
    ip link set veth$n up
done

ip netns exec devA ip addr add 192.168.50.10/24 brd + dev eth0
ip netns exec devB ip addr add 192.168.50.20/24 brd + dev eth0

for n in A B; do
    ip netns exec dev$n ip link set eth0 up
    ip netns exec dev$n ip link set lo up
    ip netns exec dev$n ip route add default dev eth0
done

mkdir -p /tmp/dA /tmp/dB

echo "Starting xChat instance A on devA (192.168.50.10:8080)..."
ip netns exec devA env HOME=/tmp/dA "$XCHAT_BIN" app --port 8080 --bind 0.0.0.0 --no-browser > /tmp/dA/xchat.log 2>&1 &
PID_A=$!

echo "Starting xChat instance B on devB (192.168.50.20:8080)..."
ip netns exec devB env HOME=/tmp/dB "$XCHAT_BIN" app --port 8080 --bind 0.0.0.0 --no-browser > /tmp/dB/xchat.log 2>&1 &
PID_B=$!

# Wait for both nodes to be HTTP-ready
echo "Waiting for HTTP servers to be ready..."
for i in {1..30}; do
    if ip netns exec devA curl -s http://127.0.0.1:8080/ >/dev/null 2>&1 && \
       ip netns exec devB curl -s http://127.0.0.1:8080/ >/dev/null 2>&1; then
        echo "Both nodes are HTTP ready."
        break
    fi
    sleep 0.5
done

# Extract CSRF tokens
TOKEN_A=$(ip netns exec devA curl -s http://127.0.0.1:8080/ | sed -n 's/.*<meta name="xchat-csrf-token" content="\([^"]*\)".*/\1/p')
TOKEN_B=$(ip netns exec devB curl -s http://127.0.0.1:8080/ | sed -n 's/.*<meta name="xchat-csrf-token" content="\([^"]*\)".*/\1/p')

if [ -z "$TOKEN_A" ] || [ -z "$TOKEN_B" ]; then
    echo "❌ Failed to retrieve CSRF tokens."
    cat /tmp/dA/xchat.log
    cat /tmp/dB/xchat.log
    exit 1
fi
echo "Retrieved CSRF tokens."

# Wait for discovery beacons between devA and devB
echo "Waiting for mutual UDP discovery between 192.168.50.10 and 192.168.50.20..."
DISCOVERED_B=""
for i in {1..30}; do
    PEERS_A=$(ip netns exec devA curl -s -H "X-XChat-Token: $TOKEN_A" http://127.0.0.1:8080/api/discovered)
    if echo "$PEERS_A" | grep -q "192.168.50.20"; then
        DISCOVERED_B="$PEERS_A"
        echo "Node A successfully discovered Node B!"
        break
    fi
    sleep 1
done

if [ -z "$DISCOVERED_B" ]; then
    echo "❌ Node A failed to discover Node B over virtual LAN."
    echo "Discovered on A: $(ip netns exec devA curl -s -H "X-XChat-Token: $TOKEN_A" http://127.0.0.1:8080/api/discovered)"
    cat /tmp/dA/xchat.log
    cat /tmp/dB/xchat.log
    exit 1
fi

ADDR_B=$(echo "$DISCOVERED_B" | jq -r '.[0].address // empty')
ALIAS_B=$(echo "$DISCOVERED_B" | jq -r '.[0].username // empty')
echo "Node B discovered by A: Alias=$ALIAS_B Address=$ADDR_B"

# Test 1: Send message A -> B
echo "--- Test 1: Node A sends direct message to Node B ---"
SEND_RESP=$(ip netns exec devA curl -s -X POST \
    -H "Content-Type: application/json" \
    -H "X-XChat-Token: $TOKEN_A" \
    -d "{\"alias\":\"$ALIAS_B\",\"address\":\"$ADDR_B\",\"target\":\"192.168.50.20:8080\",\"text\":\"Hello from DevA!\"}" \
    http://127.0.0.1:8080/api/send)
echo "Send response from A: $SEND_RESP"
echo "$SEND_RESP" | grep -q '"status":"delivered"' || { echo "❌ Direct send A->B failed"; exit 1; }

# Test 2: Node B replies via alias only
echo "--- Test 2: Node B replies to Node A via alias only ---"
sleep 1
PEERS_B=$(ip netns exec devB curl -s -H "X-XChat-Token: $TOKEN_B" http://127.0.0.1:8080/api/discovered)
ALIAS_A=$(echo "$PEERS_B" | jq -r '.[0].username // empty')
REPLY_RESP=$(ip netns exec devB curl -s -X POST \
    -H "Content-Type: application/json" \
    -H "X-XChat-Token: $TOKEN_B" \
    -d "{\"alias\":\"$ALIAS_A\",\"text\":\"Reply from DevB!\"}" \
    http://127.0.0.1:8080/api/send)
echo "Reply response from B: $REPLY_RESP"
echo "$REPLY_RESP" | grep -q '"status":"delivered"' || { echo "❌ Reply send B->A failed"; exit 1; }

# Test 3: Burner Room message both ways
echo "--- Test 3: Burner Room messaging ---"
ip netns exec devA curl -s -X POST -H "Content-Type: application/json" -H "X-XChat-Token: $TOKEN_A" \
    -d '{"alias":"#testroom"}' http://127.0.0.1:8080/api/room/join >/dev/null
ip netns exec devB curl -s -X POST -H "Content-Type: application/json" -H "X-XChat-Token: $TOKEN_B" \
    -d '{"alias":"#testroom"}' http://127.0.0.1:8080/api/room/join >/dev/null

ROOM_SEND=$(ip netns exec devA curl -s -X POST -H "Content-Type: application/json" -H "X-XChat-Token: $TOKEN_A" \
    -d '{"alias":"#testroom","text":"Room broadcast from DevA"}' http://127.0.0.1:8080/api/send)
echo "Room send response: $ROOM_SEND"
echo "$ROOM_SEND" | grep -q '"status":"delivered"' || { echo "❌ Room send failed"; exit 1; }

# Test 4: SOS broadcast reaches Node B
echo "--- Test 4: Emergency SOS broadcast ---"
SOS_RESP=$(ip netns exec devA curl -s -X POST -H "Content-Type: application/json" -H "X-XChat-Token: $TOKEN_A" \
    -d '{"message":"Wildfire nearby, evacuating","lat":37.7749,"lon":-122.4194}' http://127.0.0.1:8080/api/sos)
echo "SOS send response: $SOS_RESP"
echo "$SOS_RESP" | grep -q '"status":"delivered"' || { echo "❌ SOS broadcast failed"; exit 1; }

echo "✅ ALL TWO-DEVICE TESTS PASSED SUCCESSFULLY!"
exit 0
