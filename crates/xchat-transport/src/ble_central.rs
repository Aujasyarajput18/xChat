use std::{
    collections::{HashMap, HashSet},
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
    pub username: Option<String>,
    pub rssi: i16,
    pub address: String,
    pub last_seen: u64,
}

#[derive(Clone)]
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
    connecting_peers: Arc<TokioMutex<HashSet<String>>>,
    inbound_tx: mpsc::Sender<EncryptedEnvelope>,
    inbound_rx: TokioMutex<mpsc::Receiver<EncryptedEnvelope>>,
    inbound_json_tx: mpsc::Sender<String>,
    inbound_json_rx: Arc<TokioMutex<mpsc::Receiver<String>>>,
    msg_id_counter: Arc<AtomicU16>,
    local_identity: Arc<Mutex<Option<serde_json::Value>>>,
}

impl BleCentralTransport {
    pub async fn new() -> Arc<Self> {
        let (inbound_tx, inbound_rx) = mpsc::channel(128);
        let (inbound_json_tx, inbound_json_rx) = mpsc::channel(128);
        let is_available = Arc::new(AtomicBool::new(false));
        let discovered_devices = Arc::new(Mutex::new(HashMap::new()));
        let connected_peers = Arc::new(TokioMutex::new(HashMap::new()));
        let connecting_peers = Arc::new(TokioMutex::new(HashSet::new()));
        let msg_id_counter = Arc::new(AtomicU16::new(1));
        let local_identity = Arc::new(Mutex::new(None));

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
            connecting_peers,
            inbound_tx,
            inbound_rx: TokioMutex::new(inbound_rx),
            inbound_json_tx,
            inbound_json_rx: Arc::new(TokioMutex::new(inbound_json_rx)),
            msg_id_counter,
            local_identity,
        });

        if transport.is_available() {
            transport.clone().start_background_scanner();
        }

        transport
    }

    pub fn set_local_identity(&self, username: &str, display_name: &str, address: &str) {
        if let Ok(mut lock) = self.local_identity.lock() {
            *lock = Some(serde_json::json!({
                "type": "handshake",
                "username": username,
                "display_name": display_name,
                "address": address,
            }));
        }
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
                tokio::time::sleep(Duration::from_millis(2500)).await;
                if !self.is_available() {
                    break;
                }

                let Ok(peripherals) = adapter.peripherals().await else {
                    continue;
                };

                let mut matching_peripherals = Vec::new();

                for peripheral in peripherals {
                    let id_str = peripheral.id().to_string();
                    let Ok(Some(props)) = peripheral.properties().await else {
                        continue;
                    };

                    let is_xchat = props.services.contains(&BLE_SERVICE_UUID);
                    let name = props.local_name.clone().unwrap_or_default();
                    if !is_xchat
                        && !name.contains("Nothing")
                        && !name.contains("Phone")
                        && !name.contains("xChat")
                    {
                        continue;
                    }

                    let rssi = props.rssi.unwrap_or(-70);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    {
                        if let Ok(mut lock) = self.discovered_devices.lock() {
                            let existing_uname = lock.get(&id_str).and_then(|d| d.username.clone());
                            let device = DiscoveredBleDevice {
                                id: id_str.clone(),
                                name: if name.is_empty() {
                                    format!("xChat-{}", &id_str[..6.min(id_str.len())])
                                } else {
                                    name.clone()
                                },
                                username: existing_uname,
                                rssi,
                                address: format!("ble:{}", id_str),
                                last_seen: now,
                            };
                            lock.insert(id_str.clone(), device);
                        }
                    }

                    matching_peripherals.push((peripheral, id_str, rssi));
                }

                matching_peripherals.sort_by_key(|a| std::cmp::Reverse(a.2));

                if let Some((freshest_p, freshest_id, _)) = matching_peripherals.first() {
                    let should_connect = {
                        let connecting = self.connecting_peers.lock().await;
                        let connected = self.connected_peers.lock().await;
                        !connecting.contains(freshest_id) && !connected.contains_key(freshest_id)
                    };

                    if should_connect {
                        self.connecting_peers.lock().await.insert(freshest_id.clone());
                        let transport_clone = self.clone();
                        let p = freshest_p.clone();
                        let id = freshest_id.clone();
                        tokio::spawn(async move {
                            transport_clone.connect_peer(p, id).await;
                        });
                    }
                }
            }
        });
    }

    async fn connect_peer(self: Arc<Self>, peripheral: Peripheral, peer_id: String) {
        // Ensure peer_id is removed from connecting_peers when connect_peer terminates
        struct ConnectingGuard {
            set: Arc<TokioMutex<HashSet<String>>>,
            id: String,
        }
        impl Drop for ConnectingGuard {
            fn drop(&mut self) {
                let set = self.set.clone();
                let id = self.id.clone();
                tokio::spawn(async move {
                    set.lock().await.remove(&id);
                });
            }
        }
        let _guard = ConnectingGuard {
            set: self.connecting_peers.clone(),
            id: peer_id.clone(),
        };

        let connect_res = tokio::time::timeout(Duration::from_secs(4), peripheral.connect()).await;
        match connect_res {
            Ok(Ok(_)) => {
                debug!("Connected to BLE peripheral: {}", peer_id);
            }
            Ok(Err(e)) => {
                debug!("Unable to connect to BLE peer {}: {}", peer_id, e);
                return;
            }
            Err(_) => {
                debug!("Connection to BLE peer {} timed out", peer_id);
                return;
            }
        }

        let disc_res =
            tokio::time::timeout(Duration::from_secs(4), peripheral.discover_services()).await;
        match disc_res {
            Ok(Ok(_)) => {}
            _ => {
                warn!("Failed to discover services for BLE peer: {}", peer_id);
                let _ = peripheral.disconnect().await;
                return;
            }
        }

        let chars = peripheral.characteristics();
        let rx_char = chars.iter().find(|c| c.uuid == BLE_RX_CHAR_UUID).cloned();
        let tx_char = chars.iter().find(|c| c.uuid == BLE_TX_CHAR_UUID).cloned();
        let info_char = chars.iter().find(|c| c.uuid == BLE_INFO_CHAR_UUID).cloned();

        let Some(rx_char) = rx_char else {
            debug!("Peer {} does not have xChat RX characteristic", peer_id);
            let _ = peripheral.disconnect().await;
            return;
        };

        // If peripheral exposes identity info characteristic, read it right away!
        if let Some(ref ic) = info_char {
            match peripheral.read(ic).await {
                Ok(info_bytes) => {
                    if let Ok(info_str) = String::from_utf8(info_bytes) {
                        debug!("Read INFO_CHAR from {}: {}", peer_id, info_str);
                        if let Ok(info_json) = serde_json::from_str::<serde_json::Value>(&info_str) {
                            let uname_opt = info_json["username"]
                                .as_str()
                                .or_else(|| info_json["sender_alias"].as_str())
                                .or_else(|| info_json["alias"].as_str());
                            if let Some(uname) = uname_opt {
                                let formatted = if uname.starts_with('@') {
                                    uname.to_string()
                                } else {
                                    format!("@{}", uname)
                                };
                                if let Ok(mut lock) = self.discovered_devices.lock() {
                                    if let Some(dev) = lock.get_mut(&peer_id) {
                                        dev.username = Some(formatted.clone());
                                        if let Some(dname) = info_json["display_name"].as_str() {
                                            dev.name = dname.to_string();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Could not read INFO_CHAR from {}: {}", peer_id, e);
                }
            }
        }

        let mut tx_subscribed = false;
        if let Some(ref tx) = tx_char {
            match peripheral.notifications().await {
                Ok(mut stream) => {
                    for attempt in 1..=4 {
                        match peripheral.subscribe(tx).await {
                            Ok(_) => {
                                tx_subscribed = true;
                                info!("Successfully subscribed to TX notifications on {} (attempt {})", peer_id, attempt);
                                break;
                            }
                            Err(e) => {
                                warn!("Subscribe attempt {} on {} failed: {}. Retrying in 200ms...", attempt, peer_id, e);
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }

                    if tx_subscribed {
                        let inbound_tx = self.inbound_tx.clone();
                        let inbound_json_tx = self.inbound_json_tx.clone();
                        let peer_id_clone = peer_id.clone();
                        let discovered_devices = self.discovered_devices.clone();

                        tokio::spawn(async move {
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
                                            info!("BLE reassembled JSON received from {}: {}", peer_id_clone, json_str);
                                            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&json_str) {
                                                let uname_opt = json_val["username"]
                                                    .as_str()
                                                    .or_else(|| json_val["sender_alias"].as_str())
                                                    .or_else(|| json_val["alias"].as_str());
                                                if let Some(uname) = uname_opt {
                                                    if let Ok(mut lock) = discovered_devices.lock() {
                                                        if let Some(dev) = lock.get_mut(&peer_id_clone) {
                                                            dev.username = Some(if uname.starts_with('@') {
                                                                uname.to_string()
                                                            } else {
                                                                format!("@{}", uname)
                                                            });
                                                            if let Some(dname) = json_val["display_name"].as_str() {
                                                                dev.name = dname.to_string();
                                                            }
                                                        }
                                                    }
                                                }
                                            }
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
                                        warn!("BLE chunk reassembly error from {}: {:?}", peer_id_clone, err);
                                    }
                                }
                            }
                            info!("BLE notification stream terminated for {}", peer_id_clone);
                        });
                    }
                }
                Err(e) => {
                    warn!("Failed to get notification stream for BLE peer {}: {}", peer_id, e);
                }
            }
        }

        if tx_char.is_some() && !tx_subscribed {
            warn!("TX subscription failed on {}. Disconnecting to allow clean reconnect.", peer_id);
            let _ = peripheral.disconnect().await;
            return;
        }

        let connected_peer = ConnectedPeer {
            peripheral: peripheral.clone(),
            rx_char: rx_char.clone(),
            tx_char,
        };

        {
            let mut lock = self.connected_peers.lock().await;
            lock.insert(peer_id.clone(), connected_peer);
        }

        // Send local identity handshake frame immediately!
        let identity_opt = {
            let lock = self.local_identity.lock().ok();
            lock.and_then(|l| l.clone())
        };

        if let Some(id_json) = identity_opt {
            if let Ok(id_bytes) = serde_json::to_vec(&id_json) {
                let msg_id = self.msg_id_counter.fetch_add(1, Ordering::Relaxed);
                let chunks = BleChunker::chunk_payload(&id_bytes, 180, msg_id);
                for chunk in chunks {
                    let _ = peripheral
                        .write(&rx_char, &chunk, WriteType::WithoutResponse)
                        .await;
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
            }
        }

        info!(
            "Successfully connected and handshaked with physical BLE peer: {}",
            peer_id
        );
    }

    pub async fn send_raw_bytes(&self, target_id: &str, data: &[u8]) -> Result<(), TransportError> {
        let clean_id = target_id.strip_prefix("ble:").unwrap_or(target_id);

        let mut peer_opt = {
            let peers = self.connected_peers.lock().await;
            peers
                .get(clean_id)
                .cloned()
                .or_else(|| peers.values().next().cloned())
        };

        if let Some(ref p) = peer_opt {
            if !p.peripheral.is_connected().await.unwrap_or(false) {
                let mut peers = self.connected_peers.lock().await;
                peers.remove(clean_id);
                peer_opt = None;
            }
        }

        if peer_opt.is_none() {
            if let Some(ref adapter) = self.adapter {
                if let Ok(peripherals) = adapter.peripherals().await {
                    for p in peripherals {
                        let pid = p.id().to_string();
                        let is_match =
                            pid == clean_id
                                || p.properties().await.ok().flatten().is_some_and(|props| {
                                    props.services.contains(&BLE_SERVICE_UUID)
                                });
                        if is_match {
                            if let Ok(Ok(_)) =
                                tokio::time::timeout(Duration::from_secs(4), p.connect()).await
                            {
                                if let Ok(Ok(_)) = tokio::time::timeout(
                                    Duration::from_secs(4),
                                    p.discover_services(),
                                )
                                .await
                                {
                                    let chars = p.characteristics();
                                    if let Some(rx) =
                                        chars.iter().find(|c| c.uuid == BLE_RX_CHAR_UUID).cloned()
                                    {
                                        let tx = chars
                                            .iter()
                                            .find(|c| c.uuid == BLE_TX_CHAR_UUID)
                                            .cloned();
                                        let connected = ConnectedPeer {
                                            peripheral: p.clone(),
                                            rx_char: rx,
                                            tx_char: tx,
                                        };
                                        let mut peers = self.connected_peers.lock().await;
                                        peers.insert(pid, connected.clone());
                                        peer_opt = Some(connected);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let Some(peer) = peer_opt else {
            return Err(TransportError::NoRouteToHost(format!(
                "No connected BLE peer found for '{}'",
                target_id
            )));
        };

        let msg_id = self.msg_id_counter.fetch_add(1, Ordering::Relaxed);
        let chunks = BleChunker::chunk_payload(data, 180, msg_id);

        for chunk in chunks {
            let res = peer
                .peripheral
                .write(&peer.rx_char, &chunk, WriteType::WithoutResponse)
                .await;
            if res.is_err() {
                peer.peripheral
                    .write(&peer.rx_char, &chunk, WriteType::WithResponse)
                    .await
                    .map_err(|e| {
                        TransportError::Serialization(format!("BLE write error: {}", e))
                    })?;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
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
        rx.recv().await.ok_or(TransportError::ConnectionClosed)
    }

    fn is_available(&self) -> bool {
        self.is_available.load(Ordering::Relaxed)
    }
}
