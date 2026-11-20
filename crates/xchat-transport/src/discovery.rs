use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use xchat_core::profile::DiscoveredPeer;

use crate::errors::TransportError;

pub const DISCOVERY_MAGIC: &str = "XCHAT_DISCOVER_V1";
pub const DEFAULT_DISCOVERY_PORT: u16 = 9444;
pub const BEACON_TTL_SECS: u64 = 60;

/// Network broadcast payload for zero-configuration local mesh peer discovery.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct DiscoveryBeacon {
    pub magic: String,
    pub username: String,
    pub display_name: String,
    pub address: String,
    pub fingerprint: String,
    pub port: u16,
    pub timestamp: u64,
}

impl DiscoveryBeacon {
    pub fn new(
        username: &str,
        display_name: &str,
        address: &str,
        fingerprint: &str,
        port: u16,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            magic: DISCOVERY_MAGIC.to_string(),
            username: username.to_string(),
            display_name: display_name.to_string(),
            address: address.to_string(),
            fingerprint: fingerprint.to_string(),
            port,
            timestamp: now,
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, TransportError> {
        bincode::serialize(self).map_err(|e| TransportError::Serialization(e.to_string()))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TransportError> {
        let beacon: Self = bincode::deserialize(bytes)
            .map_err(|e| TransportError::Serialization(e.to_string()))?;
        if beacon.magic != DISCOVERY_MAGIC {
            return Err(TransportError::Serialization(
                "Invalid discovery magic header".into(),
            ));
        }
        Ok(beacon)
    }
}

/// Zero-configuration local peer discovery engine running over UDP broadcasts.
pub struct PeerDiscoveryEngine {
    socket: Arc<UdpSocket>,
    broadcast_port: u16,
    discovered_peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
}

impl PeerDiscoveryEngine {
    /// Bind discovery listener on the local port (e.g. 9444 or OS ephemeral for testing).
    /// Enables SO_REUSEADDR and SO_REUSEPORT (on Unix) to permit multiple local instances
    /// on the same machine to co-bind the discovery port without address collisions.
    pub async fn bind(listen_port: u16, broadcast_port: u16) -> Result<Arc<Self>, TransportError> {
        let bind_addr: std::net::SocketAddr = format!("0.0.0.0:{}", listen_port)
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        let sock = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        let _ = sock.set_reuse_address(true);
        #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
        let _ = sock.set_reuse_port(true);
        let _ = sock.set_broadcast(true);
        let _ = sock.set_nonblocking(true);
        sock.bind(&bind_addr.into())?;

        let std_socket: std::net::UdpSocket = sock.into();
        let socket = UdpSocket::from_std(std_socket)?;

        let discovered_peers = Arc::new(Mutex::new(HashMap::new()));
        let is_running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let socket_arc = Arc::new(socket);

        let engine = Arc::new(Self {
            socket: socket_arc.clone(),
            broadcast_port,
            discovered_peers: discovered_peers.clone(),
            is_running: is_running.clone(),
        });

        // Background listener task
        let peers_map = discovered_peers.clone();
        let run_flag = is_running.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while run_flag.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((len, src_addr)) = socket_arc.recv_from(&mut buf).await {
                    if let Ok(beacon) = DiscoveryBeacon::from_bytes(&buf[..len]) {
                        let peer_ip = src_addr.ip().to_string();
                        let peer_socket = format!("{}:{}", peer_ip, beacon.port);

                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();

                        let peer = DiscoveredPeer {
                            username: beacon.username.clone(),
                            display_name: beacon.display_name,
                            address: beacon.address.clone(),
                            fingerprint: beacon.fingerprint,
                            socket_addr: peer_socket,
                            discovered_at: now,
                            transport_type: "Wi-Fi Beacon".to_string(),
                        };

                        let mut map = peers_map.lock().unwrap();
                        map.insert(beacon.username.to_lowercase(), peer);
                    }
                }
            }
        });

        Ok(engine)
    }

    /// Broadcast our own discovery beacon across the local network subnet.
    pub async fn broadcast_beacon(&self, beacon: &DiscoveryBeacon) -> Result<(), TransportError> {
        let bytes = beacon.to_bytes()?;
        let targets = [
            format!("255.255.255.255:{}", self.broadcast_port),
            format!("127.0.0.1:{}", self.broadcast_port),
        ];

        for target in &targets {
            if let Ok(addr) = target.parse::<SocketAddr>() {
                let _ = self.socket.send_to(&bytes, addr).await;
            }
        }

        Ok(())
    }

    /// Retrieve all currently active discovered peers (prunes stale entries older than TTL).
    pub fn get_active_peers(&self) -> Vec<DiscoveredPeer> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut map = self.discovered_peers.lock().unwrap();
        map.retain(|_, p| now.saturating_sub(p.discovered_at) <= BEACON_TTL_SECS);

        map.values().cloned().collect()
    }

    /// Resolve a `@username` to a discovered peer entry.
    pub fn resolve_username(&self, username: &str) -> Option<DiscoveredPeer> {
        let key = username.to_lowercase();
        let map = self.discovered_peers.lock().unwrap();
        map.get(&key).cloned()
    }

    /// Manually register a discovered peer (useful for unit testing and direct pairings).
    pub fn register_peer(&self, peer: DiscoveredPeer) {
        let mut map = self.discovered_peers.lock().unwrap();
        map.insert(peer.username.to_lowercase(), peer);
    }

    /// Stop the discovery engine listener.
    pub fn shutdown(&self) {
        self.is_running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discovery_beacon_serialization() {
        let beacon =
            DiscoveryBeacon::new("@alice", "Alice Smith", "xchat_abcdef", "fp_12345678", 8080);
        let bytes = beacon.to_bytes().unwrap();
        let decoded = DiscoveryBeacon::from_bytes(&bytes).unwrap();
        assert_eq!(beacon, decoded);
    }

    #[tokio::test]
    async fn test_peer_discovery_engine_registration_and_lookup() {
        let engine = PeerDiscoveryEngine::bind(0, 0).await.unwrap();

        let peer = DiscoveredPeer {
            username: "@bob".to_string(),
            display_name: "Bob Operator".to_string(),
            address: "xchat_bob".to_string(),
            fingerprint: "fp_bob".to_string(),
            socket_addr: "127.0.0.1:8081".to_string(),
            discovered_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            transport_type: "Local Test".to_string(),
        };

        engine.register_peer(peer.clone());
        let resolved = engine.resolve_username("@bob");
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().socket_addr, "127.0.0.1:8081");

        let active = engine.get_active_peers();
        assert_eq!(active.len(), 1);
        engine.shutdown();
    }
}
