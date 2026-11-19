use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex as TokioMutex};
use tracing::{debug, info, warn};
use uuid::Uuid;
use xchat_core::envelope::EncryptedEnvelope;

use crate::{
    ble::{BleChunker, BleReassembler, DEFAULT_BLE_MTU},
    errors::TransportError,
    traits::{Transport, TransportType},
};

pub const BLE_SERVICE_UUID: Uuid = uuid::uuid!("a495ff10-c5b1-4b44-b512-1370f02d74de");
pub const BLE_RX_CHAR_UUID: Uuid = uuid::uuid!("a495ff11-c5b1-4b44-b512-1370f02d74de");
pub const BLE_TX_CHAR_UUID: Uuid = uuid::uuid!("a495ff12-c5b1-4b44-b512-1370f02d74de");
pub const BLE_INFO_CHAR_UUID: Uuid = uuid::uuid!("a495ff13-c5b1-4b44-b512-1370f02d74de");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveredBleDevice {
    pub id: String,
    pub name: String,
    pub rssi: i16,
    pub address: String,
    pub last_seen: u64,
}

struct ConnectedPeer {
    peripheral: Peripheral,
    rx_char: Characteristic,
    #[allow(dead_code)]
    tx_char: Option<Characteristic>,
}

/// Pure physical Bluetooth Low Energy (BLE) Central Transport for macOS & Desktop.
/// Scans for and establishes real hardware GATT connections with xChat BLE nodes (such as Android devices).
pub struct BleCentralTransport {
    is_available: Arc<AtomicBool>,
    adapter: Option<Adapter>,
    discovered_devices: Arc<Mutex<HashMap<String, DiscoveredBleDevice>>>,
    connected_peers: Arc<TokioMutex<HashMap<String, ConnectedPeer>>>,
    inbound_tx: mpsc::Sender<EncryptedEnvelope>,
    inbound_rx: TokioMutex<mpsc::Receiver<EncryptedEnvelope>>,
    inbound_json_tx: mpsc::Sender<String>,
    inbound_json_rx: Arc<TokioMutex<mpsc::Receiver<String>>>,
    msg_id_counter: Arc<AtomicU16>,
}

impl BleCentralTransport {
    pub async fn new() -> Arc<Self> {
        let (inbound_tx, inbound_rx) = mpsc::channel(128);
        let (inbound_json_tx, inbound_json_rx) = mpsc::channel(128);
        let is_available = Arc::new(AtomicBool::new(false));
        let discovered_devices = Arc::new(Mutex::new(HashMap::new()));
        let connected_peers = Arc::new(TokioMutex::new(HashMap::new()));
        let msg_id_counter = Arc::new(AtomicU16::new(1));

        let adapter = match Manager::new().await {
            Ok(manager) => match manager.adapters().await {
                Ok(adapters) => adapters.into_iter().next(),
                Err(e) => {
                    warn!("Failed to retrieve Bluetooth adapters: {}", e);
                    None
                }
            },
            Err(e) => {
                warn!("Bluetooth Manager initialization failed: {}", e);
                None
            }
        };

        if adapter.is_some() {
            is_available.store(true, Ordering::Relaxed);
            info!("Physical Bluetooth Low Energy hardware adapter initialized on host.");
        } else {
            warn!("No active Bluetooth hardware adapter found on host.");
        }

        let transport = Arc::new(Self {
            is_available,
            adapter,
            discovered_devices,
            connected_peers,
            inbound_tx,
            inbound_rx: TokioMutex::new(inbound_rx),
            inbound_json_tx,
            inbound_json_rx: Arc::new(TokioMutex::new(inbound_json_rx)),
            msg_id_counter,
        });

        if transport.is_available() {
            transport.clone().start_background_scanner();
        }

        transport
    }

    pub fn subscribe_inbound_json(&self) -> Arc<TokioMutex<mpsc::Receiver<String>>> {
        self.inbound_json_rx.clone()
    }

    pub fn get_discovered_devices(&self) -> Vec<DiscoveredBleDevice> {
        if let Ok(lock) = self.discovered_devices.lock() {
            lock.values().cloned().collect()
        } else {
            Vec::new()
        }
    }

    fn start_background_scanner(self: Arc<Self>) {
        tokio::spawn(async move {
            let Some(adapter) = &self.adapter else { return };
            if let Err(e) = adapter.start_scan(ScanFilter::default()).await {
                warn!("Failed to initiate BLE scan on adapter: {}", e);
                return;
            }

            debug!("BLE physical hardware background scanner active.");

            loop {
                tokio::time::sleep(Duration::from_millis(3000)).await;
                if !self.is_available() {
                    break;
                }

                let Ok(peripherals) = adapter.peripherals().await else {
                    continue;
                };

                for peripheral in peripherals {
                    let id_str = peripheral.id().to_string();
                    let Ok(Some(props)) = peripheral.properties().await else {
                        continue;
                    };

                    let is_xchat = props.services.iter().any(|s| *s == BLE_SERVICE_UUID);
                    if !is_xchat {
                        continue;
                    }

                    let name = props
                        .local_name
                        .clone()
                        .unwrap_or_else(|| format!("xChat-{}", &id_str[..6.min(id_str.len())]));
                    let rssi = props.rssi.unwrap_or(-70);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let device = DiscoveredBleDevice {
                        id: id_str.clone(),
                        name: name.clone(),
                        rssi,
                        address: format!("ble:{}", id_str),
                        last_seen: now,
                    };

                    if let Ok(mut lock) = self.discovered_devices.lock() {
                        lock.insert(id_str.clone(), device);
                    }

                    // Attempt connection and characteristic discovery if not already connected
                    let already_connected = {
                        let lock = self.connected_peers.lock().await;
                        lock.contains_key(&id_str)
                    };

                    if !already_connected {
                        self.clone().connect_peer(peripheral, id_str).await;
                    }
                }
            }
        });
    }

    async fn connect_peer(self: Arc<Self>, peripheral: Peripheral, peer_id: String) {
        if let Err(e) = peripheral.connect().await {
            debug!("Unable to connect to BLE peer {}: {}", peer_id, e);
            return;
        }

        if let Err(e) = peripheral.discover_services().await {
            warn!("Failed to discover services for BLE peer {}: {}", peer_id, e);
            let _ = peripheral.disconnect().await;
            return;
        }

        let chars = peripheral.characteristics();
        let rx_char = chars.iter().find(|c| c.uuid == BLE_RX_CHAR_UUID).cloned();
        let tx_char = chars.iter().find(|c| c.uuid == BLE_TX_CHAR_UUID).cloned();

        let Some(rx_char) = rx_char else {
            debug!("Peer {} does not have xChat RX characteristic", peer_id);
            let _ = peripheral.disconnect().await;
            return;
        };

        if let Some(ref tx) = tx_char {
            if let Err(e) = peripheral.subscribe(tx).await {
                warn!("Failed to subscribe to TX notifications on {}: {}", peer_id, e);
            } else {
                // Spawn notification listener for incoming envelopes
                let p_clone = peripheral.clone();
                let inbound_tx = self.inbound_tx.clone();
                let inbound_json_tx = self.inbound_json_tx.clone();
                let peer_id_clone = peer_id.clone();

                tokio::spawn(async move {
                    if let Ok(mut stream) = p_clone.notifications().await {
                        let mut reassembler = BleReassembler::new();
                        while let Some(notif) = stream.next().await {
                            match reassembler.process_packet(&notif.value) {
                                Ok(Some(assembled_bytes)) => {
                                    info!(
                                        "Received full reassembled BLE envelope from {} ({} bytes)",
                                        peer_id_clone,
                                        assembled_bytes.len()
                                    );
                                    if let Ok(json_str) = String::from_utf8(assembled_bytes.clone()) {
                                        let _ = inbound_json_tx.send(json_str).await;
                                    }
                                    if let Ok(env) = bincode::deserialize::<EncryptedEnvelope>(&assembled_bytes) {
                                        let _ = inbound_tx.send(env).await;
                                    } else if let Ok(env) = serde_json::from_slice::<EncryptedEnvelope>(&assembled_bytes) {
                                        let _ = inbound_tx.send(env).await;
                                    }
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    warn!("BLE chunk reassembly error: {:?}", err);
                                }
                            }
                        }
                    }
                });
            }
        }

        info!("Successfully connected to physical BLE peer: {}", peer_id);
        let mut lock = self.connected_peers.lock().await;
        lock.insert(peer_id, ConnectedPeer {
            peripheral,
            rx_char,
            tx_char,
        });
    }

    pub async fn send_raw_bytes(&self, target_id: &str, data: &[u8]) -> Result<(), TransportError> {
        let clean_id = target_id.strip_prefix("ble:").unwrap_or(target_id);
        let peers = self.connected_peers.lock().await;

        let peer = if let Some(p) = peers.get(clean_id) {
            p
        } else if let Some((_, p)) = peers.iter().next() {
            p
        } else {
            return Err(TransportError::NoRouteToHost(format!(
                "No connected BLE peer found for '{}'",
                target_id
            )));
        };

        let msg_id = self.msg_id_counter.fetch_add(1, Ordering::Relaxed);
        let chunks = BleChunker::chunk_payload(data, DEFAULT_BLE_MTU, msg_id);

        for chunk in chunks {
            peer.peripheral
                .write(&peer.rx_char, &chunk, WriteType::WithoutResponse)
                .await
                .map_err(|e| TransportError::Serialization(format!("BLE write error: {}", e)))?;
        }

        Ok(())
    }
}

#[async_trait]
impl Transport for BleCentralTransport {
    fn transport_type(&self) -> TransportType {
        TransportType::BluetoothMesh
    }

    async fn send_envelope(
        &self,
        target_address: &str,
        envelope: &EncryptedEnvelope,
    ) -> Result<(), TransportError> {
        let serialized = serde_json::to_vec(envelope)
            .map_err(|e| TransportError::Serialization(e.to_string()))?;
        self.send_raw_bytes(target_address, &serialized).await
    }

    async fn broadcast_envelope(
        &self,
        envelope: &EncryptedEnvelope,
    ) -> Result<usize, TransportError> {
        let serialized = serde_json::to_vec(envelope)
            .map_err(|e| TransportError::Serialization(e.to_string()))?;
        let msg_id = self.msg_id_counter.fetch_add(1, Ordering::Relaxed);
        let chunks = BleChunker::chunk_payload(&serialized, DEFAULT_BLE_MTU, msg_id);

        let peers = self.connected_peers.lock().await;
        let mut count = 0;

        for peer in peers.values() {
            let mut ok = true;
            for chunk in &chunks {
                if peer
                    .peripheral
                    .write(&peer.rx_char, chunk, WriteType::WithoutResponse)
                    .await
                    .is_err()
                {
                    ok = false;
                    break;
                }
            }
            if ok {
                count += 1;
            }
        }

        Ok(count)
    }

    async fn receive_envelope(&self) -> Result<EncryptedEnvelope, TransportError> {
        let mut rx = self.inbound_rx.lock().await;
        rx.recv()
            .await
            .ok_or(TransportError::ConnectionClosed)
    }

    fn is_available(&self) -> bool {
        self.is_available.load(Ordering::Relaxed)
    }
}
