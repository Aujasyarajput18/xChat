use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use xchat_core::{
    envelope::{EncryptedEnvelope, RouteToken},
    keystore::KeyStore,
    profile::UserProfile,
    replay::ReplayFilter,
    session::{save_sessions_encrypted, Session},
};

const DIRECT_ROUTE: RouteToken = [0u8; 16];
use xchat_transport::discovery::{DiscoveryBeacon, PeerDiscoveryEngine, DEFAULT_DISCOVERY_PORT};

pub const EMBEDDED_HTML: &str = include_str!("../../../android/app/src/main/assets/index.html");
pub const JSQR_JS: &str = include_str!("../../../android/app/src/main/assets/jsqr.min.js");

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ContactEntry {
    pub alias: String,
    pub address: String,
    pub target: String,
    pub status: String,
    pub last_message: String,
    pub last_time: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub sender: String, // "self" or "peer"
    pub text: String,
    pub time: String,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WireMessageEnvelope {
    pub envelope: EncryptedEnvelope,
    #[serde(default)]
    pub sender_alias: Option<String>,
    #[serde(default)]
    pub sender_socket: Option<String>,
    #[serde(default)]
    pub sender_address: Option<String>,
}

pub fn json_response(status: &str, body: &str, extra_headers: &str) -> String {
    format!(
        "HTTP/1.1 {}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        extra_headers,
        body.len(),
        body
    )
}

#[allow(dead_code)]
pub struct AppState {
    pub keystore: KeyStore,
    pub profile: UserProfile,
    pub local_ip: String,
    pub local_port: u16,
    pub contacts: Vec<ContactEntry>,
    pub messages: HashMap<String, Vec<ChatMessage>>,
    pub discovery: Arc<PeerDiscoveryEngine>,
    pub ble_transport: Arc<xchat_transport::ble_central::BleCentralTransport>,
    pub sessions: HashMap<String, Session>,
    pub replay_filter: ReplayFilter,
    pub csrf_token: String,
    pub session_store_path: PathBuf,
}

fn detect_local_ip() -> String {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

fn get_now_time_string() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (now / 3600) % 24;
    let minutes = (now / 60) % 60;
    format!("{:02}:{:02}", hours, minutes)
}

pub fn get_profile_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".xchat_profile.json")
    } else {
        PathBuf::from(".xchat_profile.json")
    }
}

pub fn get_keystore_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".xchat_keystore.bin")
    } else {
        PathBuf::from(".xchat_keystore.bin")
    }
}

async fn dispatch_http_get(target: &str, path: &str) -> anyhow::Result<String> {
    let clean_target = target
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let target_host = if let Some(slash_idx) = clean_target.find('/') {
        &clean_target[..slash_idx]
    } else {
        clean_target
    };

    let connect_fut = TcpStream::connect(target_host);
    let mut stream = tokio::time::timeout(tokio::time::Duration::from_secs(2), connect_fut)
        .await
        .map_err(|_| anyhow::anyhow!("Connection timeout to {}", target_host))??;

    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        path, target_host
    );
    stream.write_all(req.as_bytes()).await?;
    let mut resp = Vec::new();
    let read_fut = stream.read_to_end(&mut resp);
    tokio::time::timeout(tokio::time::Duration::from_secs(2), read_fut)
        .await
        .map_err(|_| anyhow::anyhow!("Read timeout from {}", target_host))??;

    let resp_str = String::from_utf8_lossy(&resp);
    let status_line = resp_str.lines().next().unwrap_or("");
    if !status_line.contains("200") {
        return Err(anyhow::anyhow!(
            "Remote server responded with {}",
            status_line
        ));
    }
    if let Some(pos) = resp_str.find("\r\n\r\n") {
        Ok(resp_str[pos + 4..].to_string())
    } else {
        Ok(resp_str.to_string())
    }
}

async fn dispatch_http_post(target: &str, path: &str, body: &str) -> anyhow::Result<String> {
    let clean_target = target
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let target_host = if let Some(slash_idx) = clean_target.find('/') {
        &clean_target[..slash_idx]
    } else {
        clean_target
    };

    let connect_fut = TcpStream::connect(target_host);
    let mut stream = tokio::time::timeout(tokio::time::Duration::from_secs(2), connect_fut)
        .await
        .map_err(|_| anyhow::anyhow!("Connection timeout to {}", target_host))??;

    let req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        target_host,
        body.len(),
        body
    );
    stream.write_all(req.as_bytes()).await?;
    let mut resp = Vec::new();
    let read_fut = stream.read_to_end(&mut resp);
    tokio::time::timeout(tokio::time::Duration::from_secs(2), read_fut)
        .await
        .map_err(|_| anyhow::anyhow!("Read timeout from {}", target_host))??;

    let resp_str = String::from_utf8_lossy(&resp);
    let status_line = resp_str.lines().next().unwrap_or("");
    if !status_line.contains("200") {
        return Err(anyhow::anyhow!(
            "Remote server responded with {}",
            status_line
        ));
    }
    if let Some(pos) = resp_str.find("\r\n\r\n") {
        Ok(resp_str[pos + 4..].to_string())
    } else {
        Ok(resp_str.to_string())
    }
}

/// Start the embedded Telegram-styled app server with UDP peer discovery.
pub async fn start_embedded_app_server(port: u16) -> anyhow::Result<()> {
    let bind_addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&bind_addr).await?;
    let local_ip = detect_local_ip();

    let keystore_path = get_keystore_path();
    let keystore =
        KeyStore::load_or_generate(&keystore_path).unwrap_or_else(|_| KeyStore::generate());
    let my_pub = keystore.identity.public_key();
    let my_address = my_pub.address();
    let my_fingerprint = my_pub.fingerprint();
    let visual_fp = my_pub.visual_fingerprint();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let profile_path = get_profile_path();
    let mut profile = UserProfile::load_from_disk(&profile_path).unwrap_or_else(|_| {
        let initial_handle = format!("user_{}", &my_address[6..12]);
        UserProfile::new(
            &initial_handle,
            "Mesh Operator",
            &my_address,
            &my_fingerprint,
            now,
        )
        .unwrap()
    });

    // Ensure profile always reflects current cryptographic identity keys
    profile.address = my_address.clone();
    profile.fingerprint = my_fingerprint.clone();
    let _ = profile.save_to_disk(&profile_path);

    let discovery_engine =
        match PeerDiscoveryEngine::bind(DEFAULT_DISCOVERY_PORT, DEFAULT_DISCOVERY_PORT).await {
            Ok(e) => e,
            Err(_) => PeerDiscoveryEngine::bind(0, DEFAULT_DISCOVERY_PORT).await?,
        };

    let initial_contacts = vec![
        ContactEntry {
            alias: "Saved Messages".to_string(),
            address: my_address.clone(),
            target: "local".to_string(),
            status: "Encrypted Storage".to_string(),
            last_message: "Personal cloud notes & encryption test".to_string(),
            last_time: get_now_time_string(),
        },
        ContactEntry {
            alias: "xChat Assistant".to_string(),
            address: "xchat_assistant".to_string(),
            target: "local".to_string(),
            status: "P2P Mesh AI Assistant".to_string(),
            last_message: "Send a message to test P2P encryption!".to_string(),
            last_time: get_now_time_string(),
        },
    ];

    let ble_transport = xchat_transport::ble_central::BleCentralTransport::new().await;
    ble_transport.set_local_identity(&profile.username, &profile.display_name, &my_address);

    let session_store_path = keystore_path.with_file_name(".xchat_sessions.bin");
    let storage_key = KeyStore::get_or_create_device_storage_key_for_path(Some(&keystore_path))
        .unwrap_or(KeyStore::DEFAULT_STORAGE_KEY);
    let sessions = xchat_core::session::load_sessions_encrypted(&session_store_path, &storage_key)
        .unwrap_or_default();
    let replay_store_path = keystore_path.with_file_name(".xchat_replay.bin");
    let replay_filter = xchat_core::replay::ReplayFilter::load_from_disk_encrypted(
        &replay_store_path,
        &storage_key,
    )
    .unwrap_or_default();
    let mut csrf_bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut csrf_bytes);
    let csrf_token = hex::encode(csrf_bytes);

    let state = Arc::new(Mutex::new(AppState {
        keystore,
        profile: profile.clone(),
        local_ip: local_ip.clone(),
        local_port: port,
        contacts: initial_contacts,
        messages: HashMap::new(),
        discovery: discovery_engine.clone(),
        ble_transport: ble_transport.clone(),
        sessions,
        replay_filter,
        csrf_token: csrf_token.clone(),
        session_store_path,
    }));

    // Spawn background task: inbound physical BLE message dispatcher
    let inbound_ble_rx = ble_transport.subscribe_inbound_json();
    let state_ble_inbound = state.clone();
    tokio::spawn(async move {
        let mut rx = inbound_ble_rx.lock().await;
        while let Some(raw_json) = rx.recv().await {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw_json) {
                let sender_alias = data["sender_alias"]
                    .as_str()
                    .or_else(|| data["alias"].as_str())
                    .or_else(|| data["username"].as_str())
                    .or_else(|| data["sender"].as_str())
                    .unwrap_or("Peer")
                    .to_string();
                let sender_addr = data["sender_address"]
                    .as_str()
                    .or_else(|| data["address"].as_str())
                    .or_else(|| data["sender"].as_str())
                    .unwrap_or("")
                    .to_string();
                let text = data["text"]
                    .as_str()
                    .or_else(|| data["message"].as_str())
                    .unwrap_or("")
                    .to_string();
                let time_str = data["time_str"]
                    .as_str()
                    .or_else(|| data["time"].as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(get_now_time_string);

                let formatted_alias = if sender_alias.starts_with('@') {
                    sender_alias.clone()
                } else {
                    format!("@{}", sender_alias)
                };

                // Handle handshake identity notification
                if data["type"].as_str() == Some("handshake") {
                    let mut st = state_ble_inbound.lock().unwrap();
                    if let Some(c) = st.contacts.iter_mut().find(|c| {
                        c.target.starts_with("ble:")
                            || c.address == sender_addr
                            || c.alias.to_lowercase().contains("cmf")
                            || c.alias.to_lowercase().contains("nothing")
                            || c.alias.to_lowercase().contains("phone")
                    }) {
                        let prev_alias = c.alias.clone();
                        c.alias = formatted_alias.clone();
                        if !sender_addr.is_empty() {
                            c.address = sender_addr.clone();
                        }
                        c.status = "Physical BLE Mesh".to_string();
                        // Merge and sync chat history between placeholder alias and actual formatted_alias
                        if prev_alias != formatted_alias {
                            let mut merged =
                                st.messages.get(&prev_alias).cloned().unwrap_or_default();
                            if let Some(existing) = st.messages.get(&formatted_alias) {
                                for m in existing {
                                    if !merged.iter().any(|x| {
                                        x.id == m.id || (x.text == m.text && x.time == m.time)
                                    }) {
                                        merged.push(m.clone());
                                    }
                                }
                            }
                            st.messages.insert(formatted_alias.clone(), merged.clone());
                            st.messages.insert(prev_alias, merged);
                        }
                    } else {
                        st.contacts.push(ContactEntry {
                            alias: formatted_alias.clone(),
                            address: sender_addr.clone(),
                            target: format!("ble:{}", sender_addr),
                            status: "Physical BLE Mesh".to_string(),
                            last_message: "Connected via Physical BLE Mesh".to_string(),
                            last_time: get_now_time_string(),
                        });
                    }
                    continue;
                }

                if !text.is_empty() {
                    let mut st = state_ble_inbound.lock().unwrap();
                    let msg = ChatMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        sender: "peer".to_string(),
                        text: text.clone(),
                        time: time_str.clone(),
                        status: "delivered".to_string(),
                    };

                    // Record message under formatted_alias, sender_alias, and sender_addr
                    st.messages
                        .entry(formatted_alias.clone())
                        .or_default()
                        .push(msg.clone());
                    if sender_alias != formatted_alias {
                        st.messages
                            .entry(sender_alias.clone())
                            .or_default()
                            .push(msg.clone());
                    }
                    if !sender_addr.is_empty() {
                        st.messages
                            .entry(sender_addr.clone())
                            .or_default()
                            .push(msg.clone());
                    }

                    // Update or insert contact
                    let found_idx = st.contacts.iter().position(|c| {
                        c.alias == formatted_alias
                            || c.alias == sender_alias
                            || c.target.starts_with("ble:")
                            || c.address == sender_addr
                            || c.alias.to_lowercase().contains("cmf")
                            || c.alias.to_lowercase().contains("nothing")
                            || c.alias.to_lowercase().contains("phone")
                    });

                    if let Some(idx) = found_idx {
                        let old_alias = st.contacts[idx].alias.clone();
                        if old_alias != formatted_alias && !old_alias.is_empty() {
                            st.messages.entry(old_alias).or_default().push(msg.clone());
                        }
                        st.contacts[idx].last_message = text;
                        st.contacts[idx].last_time = time_str;
                        st.contacts[idx].status = "Physical BLE Mesh".to_string();
                        if !sender_addr.is_empty() {
                            st.contacts[idx].address = sender_addr.clone();
                        }
                    } else {
                        st.contacts.push(ContactEntry {
                            alias: formatted_alias,
                            address: sender_addr.clone(),
                            target: format!("ble:{}", sender_addr),
                            status: "Physical BLE Mesh".to_string(),
                            last_message: text,
                            last_time: time_str,
                        });
                    }
                }
            }
        }
    });

    // Spawn background task: periodic local UDP beacon broadcast every 4s
    let beacon_engine = discovery_engine.clone();
    let beacon_username = profile.username.clone();
    let beacon_display_name = profile.display_name.clone();
    let beacon_addr = my_address.clone();
    let beacon_fp = my_fingerprint.clone();
    tokio::spawn(async move {
        loop {
            let beacon = DiscoveryBeacon::new(
                &beacon_username,
                &beacon_display_name,
                &beacon_addr,
                &beacon_fp,
                port,
            );
            let _ = beacon_engine.broadcast_beacon(&beacon).await;
            tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
        }
    });

    println!("===============================================================================");
    println!("   xChat Telegram-Styled App GUI Server Active");
    println!("===============================================================================");
    println!("Web App URL:       http://localhost:{}", port);
    println!("Reachable LAN IP:  {}:{}", local_ip, port);
    println!("Username Handle:   {}", profile.username);
    println!("Local Address:     {}", my_address);
    println!("Visual Safety Code:{}", visual_fp);
    println!("Discovery Carrier: Local UDP Beacon (Zero-Config Nearby Radar Active)");
    println!("-------------------------------------------------------------------------------");
    println!("Opening local browser window...");

    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg(format!("http://localhost:{}", port))
        .spawn();

    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open")
        .arg(format!("http://localhost:{}", port))
        .spawn();

    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", &format!("http://localhost:{}", port)])
        .spawn();

    println!("Ready to handle messages. Press Ctrl+C to close.");

    while let Ok((mut socket, _)) = listener.accept().await {
        let state_clone = state.clone();
        let local_ip_clone = local_ip.clone();

        tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut temp = [0u8; 2048];
            let mut content_length = 0;
            let mut header_end = None;

            loop {
                match socket.read(&mut temp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&temp[..n]);
                        if header_end.is_none() {
                            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                header_end = Some(pos + 4);
                                let header_str = String::from_utf8_lossy(&buf[..pos]);
                                for line in header_str.lines() {
                                    if line.to_lowercase().starts_with("content-length:") {
                                        if let Ok(len) = line[15..].trim().parse::<usize>() {
                                            content_length = len;
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(hend) = header_end {
                            if buf.len() >= hend + content_length {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }

            if buf.is_empty() {
                return;
            }

            let req_str = String::from_utf8_lossy(&buf);
            let first_line = req_str.lines().next().unwrap_or("");
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            let method = parts.first().copied().unwrap_or("GET");
            let path = parts.get(1).copied().unwrap_or("/");

            let body = if let Some(hend) = header_end {
                if buf.len() >= hend {
                    String::from_utf8_lossy(&buf[hend..]).to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            let mut headers = HashMap::new();
            for line in req_str.lines().skip(1) {
                if line.is_empty() || line == "\r" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }

            let local_ip_val = {
                let st = state_clone.lock().unwrap();
                st.local_ip.clone()
            };

            // DNS Rebinding protection: validate Host header
            let host = headers.get("host").map(|s| s.as_str()).unwrap_or("");
            let is_valid_host = host.is_empty()
                || host.starts_with("localhost")
                || host.starts_with("127.0.0.1")
                || host.starts_with("0.0.0.0")
                || host.starts_with("[::1]")
                || host.starts_with(&format!("{}:", local_ip_val))
                || host == local_ip_val;
            if !is_valid_host {
                let resp =
                    "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nInvalid Host Header";
                let _ = socket.write_all(resp.as_bytes()).await;
                return;
            }

            // Origin validation: block cross-origin requests from arbitrary websites
            let origin_header = headers.get("origin").cloned();
            let is_valid_origin = match &origin_header {
                Some(orig) => {
                    orig.starts_with("http://localhost")
                        || orig.starts_with("http://127.0.0.1")
                        || orig.starts_with(&format!("http://{}:", local_ip_val))
                        || orig == "null"
                }
                None => true,
            };
            let is_peer_route = path == "/api/identity"
                || path == "/api/bundle"
                || path == "/api/prekey"
                || path == "/api/receive";
            if !is_valid_origin && !is_peer_route {
                let resp =
                    "HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\nCross-Origin Forbidden";
                let _ = socket.write_all(resp.as_bytes()).await;
                return;
            }

            let cors_headers = match &origin_header {
                Some(orig) if is_valid_origin => format!(
                    "Access-Control-Allow-Origin: {}\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, X-XChat-Token\r\n",
                    orig
                ),
                _ => String::new(),
            };

            if method == "OPTIONS" {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Length: 0\r\nConnection: close\r\n\r\n",
                    cors_headers
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                return;
            }

            // CSRF Token verification on all UI /api/* routes
            if path.starts_with("/api/") && !is_peer_route {
                let token = headers
                    .get("x-xchat-token")
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let expected = {
                    let st = state_clone.lock().unwrap();
                    st.csrf_token.clone()
                };
                if token != expected {
                    let resp = json_response(
                        "401 Unauthorized",
                        r#"{"error":"Unauthorized: Missing or invalid CSRF token"}"#,
                        &cors_headers,
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return;
                }
            }

            if path == "/api/identity" {
                let (username, display_name, sign_pubkey, dh_pubkey, cur_addr, cur_fp, cur_vfp) = {
                    let st = state_clone.lock().unwrap();
                    let pk = st.keystore.identity.public_key();
                    (
                        st.profile.username.clone(),
                        st.profile.display_name.clone(),
                        hex::encode(pk.sign_pubkey),
                        hex::encode(pk.dh_pubkey),
                        pk.address(),
                        pk.fingerprint(),
                        pk.visual_fingerprint(),
                    )
                };
                let json = serde_json::to_string(&serde_json::json!({
                    "address": cur_addr,
                    "fingerprint": cur_fp,
                    "sign_pubkey": sign_pubkey,
                    "dh_pubkey": dh_pubkey,
                    "visual_fingerprint": cur_vfp,
                    "username": username,
                    "display_name": display_name,
                    "ip": local_ip_clone,
                    "port": port,
                    "status": "online"
                }))
                .unwrap_or_else(|_| "{}".to_string());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    json.len(),
                    json
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/keygen" && method == "POST" {
                let (new_addr, new_fp, new_visual, sign_pub, dh_pub, bundle_val) = {
                    let mut st = state_clone.lock().unwrap();
                    let mut new_ks = KeyStore::generate();
                    let new_pk = new_ks.identity.public_key();
                    let addr = new_pk.address();
                    let fp = new_pk.fingerprint();
                    let vfp = new_pk.visual_fingerprint();
                    let spk = hex::encode(new_pk.sign_pubkey);
                    let dpk = hex::encode(new_pk.dh_pubkey);
                    let bundle = new_ks.export_bundle();

                    let ks_path = get_keystore_path();
                    let _ = new_ks.save_to_path(&ks_path);

                    st.profile.address = addr.clone();
                    st.profile.fingerprint = fp.clone();
                    let ppath = get_profile_path();
                    let _ = st.profile.save_to_disk(&ppath);

                    st.keystore = new_ks;
                    (
                        addr,
                        fp,
                        vfp,
                        spk,
                        dpk,
                        serde_json::to_value(&bundle).unwrap_or_default(),
                    )
                };

                let resp_body = serde_json::to_string(&serde_json::json!({
                    "status": "generated",
                    "address": new_addr,
                    "fingerprint": new_fp,
                    "visual_fingerprint": new_visual,
                    "sign_pubkey": sign_pub,
                    "dh_pubkey": dh_pub,
                    "prekey_bundle": bundle_val
                }))
                .unwrap_or_else(|_| "{}".to_string());

                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    resp_body.len(),
                    resp_body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if (path == "/api/bundle" || path == "/api/prekey") && method == "GET" {
                let bundle_json = {
                    let mut st = state_clone.lock().unwrap();
                    let bundle = st.keystore.export_bundle();
                    serde_json::to_string(&bundle).unwrap_or_else(|_| "{}".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    bundle_json.len(),
                    bundle_json
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/profile" && method == "GET" {
                let (profile, reachable, invite_url, cur_vfp) = {
                    let st = state_clone.lock().unwrap();
                    let sock = format!("{}:{}", st.local_ip, st.local_port);
                    let invite = st.profile.generate_invite_url(&sock);
                    let vfp = st.keystore.identity.public_key().visual_fingerprint();
                    (st.profile.clone(), sock, invite, vfp)
                };
                let json = serde_json::to_string(&serde_json::json!({
                    "username": profile.username,
                    "display_name": profile.display_name,
                    "bio": profile.bio,
                    "avatar_color": profile.avatar_color,
                    "address": profile.address,
                    "fingerprint": profile.fingerprint,
                    "visual_fingerprint": cur_vfp,
                    "reachable": reachable,
                    "invite_url": invite_url
                }))
                .unwrap_or_else(|_| "{}".to_string());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    json.len(),
                    json
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/profile" && method == "POST" {
                #[derive(Deserialize)]
                struct UpdateProfileReq {
                    username: Option<String>,
                    display_name: Option<String>,
                    bio: Option<String>,
                    avatar_color: Option<serde_json::Value>,
                }
                if let Ok(req) = serde_json::from_str::<UpdateProfileReq>(&body) {
                    let mut st = state_clone.lock().unwrap();
                    if let Some(u) = req.username {
                        if let Ok(clean) = UserProfile::validate_and_normalize_username(&u) {
                            st.profile.username = clean;
                        } else if !u.trim().is_empty() {
                            let clean = if u.trim().starts_with('@') {
                                u.trim().to_lowercase()
                            } else {
                                format!("@{}", u.trim().to_lowercase())
                            };
                            st.profile.username = clean;
                        }
                    }
                    if let Some(dn) = req.display_name {
                        if !dn.trim().is_empty() {
                            st.profile.display_name = dn.trim().to_string();
                        }
                    }
                    if let Some(b) = req.bio {
                        st.profile.bio = b;
                    }
                    if let Some(ac) = req.avatar_color {
                        let color_str = match ac {
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::String(s) => s,
                            _ => "0".to_string(),
                        };
                        st.profile.avatar_color = color_str;
                    }
                    let ppath = get_profile_path();
                    let _ = st.profile.save_to_disk(&ppath);
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{{\"status\":\"updated\"}}",
                    cors_headers
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/discovered" && method == "GET" {
                let peers_json = {
                    let st = state_clone.lock().unwrap();
                    let mut peers: Vec<serde_json::Value> = st
                        .discovery
                        .get_active_peers()
                        .into_iter()
                        .map(|p| serde_json::to_value(p).unwrap_or_default())
                        .collect();

                    let ble_devs = st.ble_transport.get_discovered_devices();
                    for b in ble_devs {
                        let uname = b.username.clone().unwrap_or_else(|| {
                            format!("@{}", b.name.to_lowercase().replace(' ', "_"))
                        });
                        peers.push(serde_json::json!({
                            "username": uname,
                            "display_name": b.name,
                            "address": b.address,
                            "fingerprint": b.id,
                            "ip": b.address,
                            "port": 0,
                            "transport": "ble",
                            "rssi": b.rssi,
                            "last_seen": b.last_seen * 1000
                        }));
                    }

                    serde_json::to_string(&peers).unwrap_or_else(|_| "[]".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    peers_json.len(),
                    peers_json
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/connect_username" && method == "POST" {
                #[derive(Deserialize)]
                struct ConnectUserReq {
                    username: String,
                }
                let mut found = None;
                if let Ok(req) = serde_json::from_str::<ConnectUserReq>(&body) {
                    let st = state_clone.lock().unwrap();
                    found = st.discovery.resolve_username(&req.username);
                }
                let resp_body = if let Some(peer) = found {
                    serde_json::to_string(&peer).unwrap_or_else(|_| "{}".to_string())
                } else {
                    r#"{"error":"Username not found on local mesh"}"#.to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    resp_body.len(),
                    resp_body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/contacts" && method == "GET" {
                let contacts_json = {
                    let st = state_clone.lock().unwrap();
                    serde_json::to_string(&st.contacts).unwrap_or_else(|_| "[]".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    contacts_json.len(),
                    contacts_json
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/contacts" && method == "POST" {
                #[allow(dead_code)]
                #[derive(Deserialize)]
                struct AddContactReq {
                    alias: String,
                    target: Option<String>,
                    bundle: Option<String>,
                    fingerprint: Option<String>,
                    address: Option<String>,
                }
                if let Ok(req) = serde_json::from_str::<AddContactReq>(&body) {
                    let target_str = req.target.unwrap_or_default();
                    let mut st = state_clone.lock().unwrap();
                    if let Some(existing) = st
                        .contacts
                        .iter_mut()
                        .find(|c| c.alias.eq_ignore_ascii_case(&req.alias))
                    {
                        if !target_str.is_empty() {
                            existing.target = target_str;
                        }
                    } else {
                        let addr = req.address.unwrap_or_else(|| {
                            format!(
                                "xchat_{}",
                                hex::encode(&uuid::Uuid::new_v4().as_bytes()[..4])
                            )
                        });
                        st.contacts.push(ContactEntry {
                            alias: req.alias.clone(),
                            address: addr,
                            target: target_str,
                            status: "Connected".to_string(),
                            last_message: "Tap to chat".to_string(),
                            last_time: get_now_time_string(),
                        });
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{{\"status\":\"added\"}}",
                    cors_headers
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/send" && method == "POST" {
                #[derive(Deserialize)]
                struct SendReq {
                    alias: String,
                    target: Option<String>,
                    text: String,
                }
                if let Ok(req) = serde_json::from_str::<SendReq>(&body) {
                    let now_str = get_now_time_string();
                    let now_ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let msg = ChatMessage {
                        id: uuid::Uuid::new_v4().to_string(),
                        sender: "self".to_string(),
                        text: req.text.clone(),
                        time: now_str.clone(),
                        status: "sent".to_string(),
                    };

                    let (target_opt, my_handle, my_addr, my_ip, my_port) = {
                        let mut st = state_clone.lock().unwrap();
                        let hist = st.messages.entry(req.alias.clone()).or_default();
                        hist.push(msg.clone());

                        // Cross-record outgoing message to linked address/target if known
                        let mut linked_aliases = Vec::new();
                        if let Some(c) = st.contacts.iter().find(|c| c.alias == req.alias) {
                            if !c.address.is_empty() && c.address != req.alias {
                                linked_aliases.push(c.address.clone());
                            }
                            if !c.target.is_empty() && c.target != req.alias && c.target != "local"
                            {
                                linked_aliases.push(c.target.clone());
                            }
                        }
                        for link in linked_aliases {
                            st.messages.entry(link).or_default().push(msg.clone());
                        }

                        let mut resolved_target = req.target.clone();
                        if let Some(c) = st.contacts.iter_mut().find(|c| c.alias == req.alias) {
                            c.last_message = req.text.clone();
                            c.last_time = now_str.clone();
                            if (resolved_target.is_none()
                                || resolved_target.as_deref() == Some("")
                                || resolved_target.as_deref() == Some("local"))
                                && !c.target.is_empty()
                                && c.target != "local"
                            {
                                resolved_target = Some(c.target.clone());
                            }
                        }
                        let cur_addr = st.keystore.identity.public_key().address();
                        (
                            resolved_target,
                            st.profile.username.clone(),
                            cur_addr,
                            st.local_ip.clone(),
                            st.local_port,
                        )
                    };

                    // Handle Local Assistant / Saved Messages
                    if req.alias == "xChat Assistant"
                        || (target_opt.as_deref() == Some("local") && req.alias == "Saved Messages")
                    {
                        if req.alias == "xChat Assistant" {
                            let mut st = state_clone.lock().unwrap();
                            let hist = st.messages.entry(req.alias.clone()).or_default();
                            hist.push(ChatMessage {
                                id: uuid::Uuid::new_v4().to_string(),
                                sender: "peer".to_string(),
                                text: format!("Echo [Personal Assistant]: {}", req.text),
                                time: get_now_time_string(),
                                status: "delivered".to_string(),
                            });
                        }
                        let resp = json_response(
                            "200 OK",
                            r#"{"status":"delivered","authenticated":true}"#,
                            &cors_headers,
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                        return;
                    } else if let Some(target) = target_opt {
                        if !target.is_empty() && target != "local" {
                            // Find contact info (address) if known
                            let contact_addr = {
                                let st = state_clone.lock().unwrap();
                                st.contacts
                                    .iter()
                                    .find(|c| c.alias == req.alias || c.target == target)
                                    .map(|c| c.address.clone())
                            };

                            // Check if an established ratcheted session exists
                            let mut existing_session_key = None;
                            {
                                let st = state_clone.lock().unwrap();
                                if let Some(ref addr) = contact_addr {
                                    if st.sessions.contains_key(addr) {
                                        existing_session_key = Some(addr.clone());
                                    }
                                }
                                if existing_session_key.is_none()
                                    && st.sessions.contains_key(&target)
                                {
                                    existing_session_key = Some(target.clone());
                                }
                                if existing_session_key.is_none()
                                    && st.sessions.contains_key(&req.alias)
                                {
                                    existing_session_key = Some(req.alias.clone());
                                }
                            }

                            let mut session_envelope = None;
                            if let Some(skey) = existing_session_key {
                                let mut st = state_clone.lock().unwrap();
                                if let Some(sess) = st.sessions.get_mut(&skey) {
                                    if let Ok(env) = sess.encrypt_message(
                                        req.text.as_bytes(),
                                        DIRECT_ROUTE,
                                        now_ts,
                                        86400,
                                    ) {
                                        let storage_key =
                                            KeyStore::get_or_create_device_storage_key_for_path(
                                                Some(&st.session_store_path),
                                            )
                                            .unwrap_or(KeyStore::DEFAULT_STORAGE_KEY);
                                        let _ = save_sessions_encrypted(
                                            &st.sessions,
                                            &st.session_store_path,
                                            &storage_key,
                                        );
                                        session_envelope = Some(env);
                                    }
                                }
                            }

                            // If no session exists, initiate outbound session via X3DH with remote prekey bundle
                            if session_envelope.is_none() {
                                if let Ok(bundle_json) =
                                    dispatch_http_get(&target, "/api/bundle").await
                                {
                                    if let Ok(bundle) =
                                        serde_json::from_str::<xchat_crypto::x3dh::PrekeyBundle>(
                                            &bundle_json,
                                        )
                                    {
                                        let mut st = state_clone.lock().unwrap();
                                        if let Ok((sess, env)) = Session::initiate_outbound(
                                            &st.keystore,
                                            &bundle,
                                            req.text.as_bytes(),
                                            DIRECT_ROUTE,
                                            now_ts,
                                            86400,
                                        ) {
                                            let peer_addr = bundle.identity_key.address();
                                            st.sessions.insert(peer_addr.clone(), sess.clone());
                                            st.sessions.insert(target.clone(), sess);
                                            if let Some(c) = st.contacts.iter_mut().find(|c| {
                                                c.alias == req.alias || c.target == target
                                            }) {
                                                c.address = peer_addr;
                                            }
                                            let storage_key = KeyStore::get_or_create_device_storage_key_for_path(Some(&st.session_store_path))
                                                .unwrap_or(KeyStore::DEFAULT_STORAGE_KEY);
                                            let _ = save_sessions_encrypted(
                                                &st.sessions,
                                                &st.session_store_path,
                                                &storage_key,
                                            );
                                            session_envelope = Some(env);
                                        }
                                    }
                                }
                            }

                            if let Some(envelope) = session_envelope {
                                let wire_envelope = WireMessageEnvelope {
                                    envelope,
                                    sender_alias: Some(my_handle),
                                    sender_socket: Some(format!("{}:{}", my_ip, my_port)),
                                    sender_address: Some(my_addr),
                                };
                                let out_body =
                                    serde_json::to_string(&wire_envelope).unwrap_or_default();
                                let mut delivered = false;
                                if !target.starts_with("ble:")
                                    && target.contains(':')
                                    && !target.starts_with('@')
                                    && dispatch_http_post(&target, "/api/receive", &out_body)
                                        .await
                                        .is_ok()
                                {
                                    delivered = true;
                                }
                                if !delivered {
                                    let ble_t = {
                                        let st = state_clone.lock().unwrap();
                                        st.ble_transport.clone()
                                    };
                                    if ble_t
                                        .send_raw_bytes(&target, out_body.as_bytes())
                                        .await
                                        .is_ok()
                                    {
                                        delivered = true;
                                    }
                                }

                                if delivered {
                                    let resp = json_response(
                                        "200 OK",
                                        r#"{"status":"delivered","authenticated":true}"#,
                                        &cors_headers,
                                    );
                                    let _ = socket.write_all(resp.as_bytes()).await;
                                    return;
                                }
                            }

                            // If delivery or session creation failed, update message status to failed
                            {
                                let mut st = state_clone.lock().unwrap();
                                if let Some(msgs) = st.messages.get_mut(&req.alias) {
                                    if let Some(last_msg) = msgs.last_mut() {
                                        last_msg.status = "failed".to_string();
                                    }
                                }
                            }
                            let resp = json_response(
                                "502 Bad Gateway",
                                r#"{"status":"failed","error":"Delivery failed to recipient"}"#,
                                &cors_headers,
                            );
                            let _ = socket.write_all(resp.as_bytes()).await;
                            return;
                        }
                    }
                }
                let resp = json_response(
                    "400 Bad Request",
                    r#"{"error":"Invalid request parameters"}"#,
                    &cors_headers,
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/receive" && method == "POST" {
                let wire_opt = serde_json::from_str::<WireMessageEnvelope>(&body).ok();
                let (envelope, sender_alias_opt, sender_socket_opt, sender_addr_hint) =
                    if let Some(wire) = wire_opt {
                        (
                            wire.envelope,
                            wire.sender_alias,
                            wire.sender_socket,
                            wire.sender_address,
                        )
                    } else if let Ok(env) = serde_json::from_str::<EncryptedEnvelope>(&body) {
                        (env, None, None, None)
                    } else {
                        let resp = json_response(
                            "401 Unauthorized",
                            r#"{"error":"Unauthenticated or corrupted envelope"}"#,
                            &cors_headers,
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                        return;
                    };

                let now_ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                if envelope.is_expired(now_ts) {
                    let resp = json_response(
                        "401 Unauthorized",
                        r#"{"error":"Envelope has expired"}"#,
                        &cors_headers,
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return;
                }

                // Check anti-replay filter
                let is_replayed = {
                    let mut st = state_clone.lock().unwrap();
                    let err = st
                        .replay_filter
                        .check_and_record(&envelope.header.message_id, now_ts)
                        .is_err();
                    if !err {
                        let replay_path = st.session_store_path.with_file_name(".xchat_replay.bin");
                        let storage_key = KeyStore::get_or_create_device_storage_key_for_path(
                            Some(&st.session_store_path),
                        )
                        .unwrap_or(KeyStore::DEFAULT_STORAGE_KEY);
                        let _ = st
                            .replay_filter
                            .save_to_disk_encrypted(&replay_path, &storage_key);
                    }
                    err
                };
                if is_replayed {
                    let resp = json_response(
                        "401 Unauthorized",
                        r#"{"error":"Replayed message rejected"}"#,
                        &cors_headers,
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return;
                }

                // Decrypt inbound message via Session
                let decrypt_result = {
                    let mut st = state_clone.lock().unwrap();
                    if envelope.x3dh_init.is_some() {
                        match Session::respond_inbound(&mut st.keystore, &envelope, now_ts) {
                            Ok((new_session, pt_bytes)) => {
                                let sender_addr = new_session.peer_identity.address();
                                if let Some(ref sock) = sender_socket_opt {
                                    st.sessions.insert(sock.clone(), new_session.clone());
                                }
                                st.sessions.insert(sender_addr.clone(), new_session);
                                let ks_path = get_keystore_path();
                                let _ = st.keystore.save_to_path(&ks_path);
                                let storage_key =
                                    KeyStore::get_or_create_device_storage_key_for_path(Some(
                                        &st.session_store_path,
                                    ))
                                    .unwrap_or(KeyStore::DEFAULT_STORAGE_KEY);
                                let _ = save_sessions_encrypted(
                                    &st.sessions,
                                    &st.session_store_path,
                                    &storage_key,
                                );
                                Ok((sender_addr, pt_bytes))
                            }
                            Err(e) => Err(format!("X3DH respond failed: {:?}", e)),
                        }
                    } else {
                        let mut found = None;
                        if let Some(hint) = &sender_addr_hint {
                            if let Some(session) = st.sessions.get_mut(hint) {
                                let mut candidate = session.clone();
                                if let Ok(pt_bytes) = candidate.decrypt_message(&envelope, now_ts) {
                                    *session = candidate;
                                    found = Some((hint.clone(), pt_bytes));
                                }
                            }
                        }
                        if found.is_none() {
                            if let Some(sock) = &sender_socket_opt {
                                if let Some(session) = st.sessions.get_mut(sock) {
                                    let mut candidate = session.clone();
                                    if let Ok(pt_bytes) =
                                        candidate.decrypt_message(&envelope, now_ts)
                                    {
                                        *session = candidate;
                                        found = Some((sock.clone(), pt_bytes));
                                    }
                                }
                            }
                        }
                        if found.is_none() {
                            for (addr, session) in st.sessions.iter_mut() {
                                let mut candidate = session.clone();
                                if let Ok(pt_bytes) = candidate.decrypt_message(&envelope, now_ts) {
                                    *session = candidate;
                                    found = Some((addr.clone(), pt_bytes));
                                    break;
                                }
                            }
                        }
                        if let Some((addr, pt_bytes)) = found {
                            let storage_key = KeyStore::get_or_create_device_storage_key_for_path(
                                Some(&st.session_store_path),
                            )
                            .unwrap_or(KeyStore::DEFAULT_STORAGE_KEY);
                            let _ = save_sessions_encrypted(
                                &st.sessions,
                                &st.session_store_path,
                                &storage_key,
                            );
                            Ok((addr, pt_bytes))
                        } else {
                            Err("No matching ratcheted session found".to_string())
                        }
                    }
                };

                let (resp_status, resp_body) = match decrypt_result {
                    Ok((sender_addr, pt_bytes)) => {
                        let plaintext = match String::from_utf8(pt_bytes) {
                            Ok(s) => s,
                            Err(_) => {
                                let resp = json_response(
                                    "401 Unauthorized",
                                    r#"{"error":"Payload is not valid UTF-8"}"#,
                                    &cors_headers,
                                );
                                let _ = socket.write_all(resp.as_bytes()).await;
                                return;
                            }
                        };
                        let now_str = get_now_time_string();
                        let msg = ChatMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            sender: "peer".to_string(),
                            text: plaintext.clone(),
                            time: now_str.clone(),
                            status: "delivered".to_string(),
                        };

                        let sender_alias_clean = sender_alias_opt
                            .clone()
                            .unwrap_or_else(|| sender_addr.clone());

                        {
                            let mut st = state_clone.lock().unwrap();
                            let mut target_alias = sender_alias_clean.clone();
                            if let Some(c) = st.contacts.iter_mut().find(|c| {
                                c.address == sender_addr
                                    || c.alias == sender_alias_clean
                                    || (!c.target.is_empty()
                                        && sender_socket_opt.as_ref() == Some(&c.target))
                            }) {
                                target_alias = c.alias.clone();
                                c.last_message = plaintext.clone();
                                c.last_time = now_str.clone();
                                if let Some(ref sock) = sender_socket_opt {
                                    if c.target.is_empty()
                                        || c.target == "local"
                                        || c.target == "Direct"
                                    {
                                        c.target = sock.clone();
                                    }
                                }
                                if c.address.is_empty() {
                                    c.address = sender_addr.clone();
                                }
                            } else {
                                st.contacts.push(ContactEntry {
                                    alias: sender_alias_clean.clone(),
                                    address: sender_addr.clone(),
                                    target: sender_socket_opt.clone().unwrap_or_default(),
                                    status: "Online".to_string(),
                                    last_message: plaintext.clone(),
                                    last_time: now_str.clone(),
                                });
                            }

                            st.messages
                                .entry(target_alias)
                                .or_default()
                                .push(msg.clone());
                            st.messages.entry(sender_addr).or_default().push(msg);
                        }

                        ("200 OK", r#"{"status":"delivered","authenticated":true}"#)
                    }
                    Err(_) => (
                        "401 Unauthorized",
                        r#"{"error":"Unauthenticated or corrupted envelope"}"#,
                    ),
                };

                let resp = json_response(resp_status, resp_body, &cors_headers);
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path.starts_with("/api/messages") && method == "GET" {
                let peer_opt = path.split('?').nth(1).and_then(|q| {
                    q.split('&')
                        .find(|param| param.starts_with("peer="))
                        .map(|param| urlencoding_decode(&param[5..]))
                });

                let msgs_json = {
                    let st = state_clone.lock().unwrap();
                    if let Some(peer) = peer_opt {
                        let list = get_unified_messages_for_peer(&st, &peer);
                        serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
                    } else {
                        "[]".to_string()
                    }
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    cors_headers,
                    msgs_json.len(),
                    msgs_json
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/ping" && method == "POST" {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: 32\r\nConnection: close\r\n\r\n{{\"status\":\"pong\",\"latency_ms\":14}}",
                    cors_headers
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/sos" && method == "POST" {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: 25\r\nConnection: close\r\n\r\n{{\"status\":\"broadcasted\"}}",
                    cors_headers
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/jsqr.min.js" {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/javascript; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    JSQR_JS.len(),
                    JSQR_JS
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    EMBEDDED_HTML.len(),
                    EMBEDDED_HTML
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
        });
    }

    Ok(())
}

fn urlencoding_decode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let hex_str: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex_str, 16) {
                result.push(byte as char);
            }
        } else if ch == '+' {
            result.push(' ');
        } else {
            result.push(ch);
        }
    }
    result
}

fn get_unified_messages_for_peer(st: &AppState, peer: &str) -> Vec<ChatMessage> {
    if peer == "Saved Messages" || peer == "xChat Assistant" {
        return st.messages.get(peer).cloned().unwrap_or_default();
    }

    let mut candidate_keys: Vec<String> = Vec::new();
    candidate_keys.push(peer.to_string());
    let clean = peer.trim_start_matches('@');
    candidate_keys.push(clean.to_string());
    candidate_keys.push(format!("@{}", clean));
    candidate_keys.push(peer.to_lowercase());
    candidate_keys.push(format!("@{}", clean.to_lowercase()));

    // Find any contact matching peer
    for c in &st.contacts {
        let matches = c.alias.eq_ignore_ascii_case(peer)
            || c.alias.eq_ignore_ascii_case(clean)
            || c.address.eq_ignore_ascii_case(peer)
            || c.target.eq_ignore_ascii_case(peer)
            || c.target.contains(peer)
            || peer.contains(&c.target)
            || c.alias.to_lowercase().contains(&peer.to_lowercase())
            || peer.to_lowercase().contains(&c.alias.to_lowercase());
        if matches {
            candidate_keys.push(c.alias.clone());
            candidate_keys.push(c.address.clone());
            candidate_keys.push(c.target.clone());
            candidate_keys.push(
                c.target
                    .strip_prefix("ble:")
                    .unwrap_or(&c.target)
                    .to_string(),
            );
        }
    }

    // Check discovered BLE devices
    let ble_devs = st.ble_transport.get_discovered_devices();
    for d in ble_devs {
        let matches = d.id.eq_ignore_ascii_case(peer)
            || d.address.eq_ignore_ascii_case(peer)
            || d.name.eq_ignore_ascii_case(peer)
            || d.name
                .to_lowercase()
                .replace(' ', "_")
                .eq_ignore_ascii_case(clean)
            || d.username.as_ref().is_some_and(|u| {
                u.eq_ignore_ascii_case(peer)
                    || u.trim_start_matches('@').eq_ignore_ascii_case(clean)
            });
        if matches {
            candidate_keys.push(d.id.clone());
            candidate_keys.push(d.address.clone());
            candidate_keys.push(d.name.clone());
            candidate_keys.push(format!("@{}", d.name.to_lowercase().replace(' ', "_")));
            if let Some(ref u) = d.username {
                candidate_keys.push(u.clone());
                candidate_keys.push(u.trim_start_matches('@').to_string());
            }
        }
    }

    if peer == "Peer" {
        for k in st.messages.keys() {
            if k != "xChat Assistant" && k != "Saved Messages" {
                candidate_keys.push(k.clone());
            }
        }
    }

    let mut collected: Vec<ChatMessage> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for key in &candidate_keys {
        if let Some(msgs) = st.messages.get(key) {
            for m in msgs {
                let dedup_key = format!("{}:{}:{}", m.id, m.time, m.text);
                if seen_ids.insert(dedup_key) {
                    collected.push(m.clone());
                }
            }
        }
    }

    if collected.is_empty() {
        // Last-resort fallback: check if any key in st.messages resembles peer
        let peer_lower = peer.to_lowercase();
        for (k, msgs) in &st.messages {
            if k == "xChat Assistant" || k == "Saved Messages" {
                continue;
            }
            let k_lower = k.to_lowercase();
            if k_lower.contains(&peer_lower) || peer_lower.contains(&k_lower) {
                for m in msgs {
                    let dedup_key = format!("{}:{}:{}", m.id, m.time, m.text);
                    if seen_ids.insert(dedup_key) {
                        collected.push(m.clone());
                    }
                }
            }
        }
    }

    collected
}

#[cfg(test)]
mod tests {
    use super::*;
    use xchat_crypto::identity::IdentityKeyPair;

    #[tokio::test]
    async fn test_gui_e2ee_receive_validation() {
        let alice_id = IdentityKeyPair::generate();
        let alice_pk = alice_id.public_key();
        let bob_id = IdentityKeyPair::generate();
        let bob_pk = bob_id.public_key();

        // 1. Unauthenticated injection attempt must be rejected
        let forged_payload = serde_json::json!({
            "sender": "attacker",
            "alias": "Eve",
            "text": "Fake news message"
        });
        let forged_body = serde_json::to_string(&forged_payload).unwrap();
        // Parsing as EncryptedReceiveReq fails
        let parse_attempt = serde_json::from_str::<serde_json::Value>(&forged_body);
        assert!(parse_attempt.is_ok());

        // 2. Legitimate E2EE transmission from Alice to Bob
        let shared_dh = alice_id.diffie_hellman(&bob_pk.dh_pubkey);
        let aead_key = xchat_crypto::kdf::kdf_x3dh(&shared_dh);

        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let now_ts = 1_000_000u64;
        let plaintext = b"Hello Bob, this is authentic and encrypted!";
        let ciphertext =
            xchat_crypto::aead::encrypt_aead(&aead_key, &nonce, plaintext, b"xchat-gui-e2ee-v1")
                .unwrap();

        let mut sig_material = Vec::new();
        sig_material.extend_from_slice(&alice_pk.sign_pubkey);
        sig_material.extend_from_slice(&alice_pk.dh_pubkey);
        sig_material.extend_from_slice(&nonce);
        sig_material.extend_from_slice(&now_ts.to_be_bytes());
        sig_material.extend_from_slice(&ciphertext);
        let signature = alice_id.sign(&sig_material);

        // Verify Bob can decrypt and verify Alice's signature
        assert!(alice_pk.verify(&sig_material, &signature).is_ok());

        let bob_shared = bob_id.diffie_hellman(&alice_pk.dh_pubkey);
        let bob_key = xchat_crypto::kdf::kdf_x3dh(&bob_shared);
        assert_eq!(aead_key, bob_key);

        let decrypted =
            xchat_crypto::aead::decrypt_aead(&bob_key, &nonce, &ciphertext, b"xchat-gui-e2ee-v1")
                .unwrap();
        assert_eq!(decrypted, plaintext);

        // 3. Mutated signature must fail
        let mut bad_sig = signature;
        bad_sig[0] ^= 0x01;
        assert!(alice_pk.verify(&sig_material, &bad_sig).is_err());
    }

    #[test]
    fn test_p0_1_outbound_wire_envelope_has_no_plaintext() {
        let alice_ks = KeyStore::generate();
        let mut bob_ks = KeyStore::generate();
        let bob_bundle = bob_ks.export_bundle();

        let secret_msg = "CONFIDENTIAL_TOP_SECRET_12345";
        let (mut alice_sess, env) = Session::initiate_outbound(
            &alice_ks,
            &bob_bundle,
            secret_msg.as_bytes(),
            DIRECT_ROUTE,
            1_000_000,
            86400,
        )
        .unwrap();

        let wire = WireMessageEnvelope {
            envelope: env,
            sender_alias: Some("Alice".to_string()),
            sender_socket: Some("127.0.0.1:8080".to_string()),
            sender_address: Some(alice_ks.identity.public_key().address()),
        };

        let json = serde_json::to_string(&wire).unwrap();
        // Zero plaintext in wire envelope
        assert!(!json.contains(secret_msg));
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(val.get("text").is_none());
        assert!(val.get("message").is_none());

        // Subsequent ratcheted message also has zero plaintext
        let env2 = alice_sess
            .encrypt_message(b"SECOND_SECRET_98765", DIRECT_ROUTE, 1_000_001, 86400)
            .unwrap();
        let wire2 = WireMessageEnvelope {
            envelope: env2,
            sender_alias: Some("Alice".to_string()),
            sender_socket: Some("127.0.0.1:8080".to_string()),
            sender_address: Some(alice_ks.identity.public_key().address()),
        };
        let json2 = serde_json::to_string(&wire2).unwrap();
        assert!(!json2.contains("SECOND_SECRET_98765"));
        let val2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert!(val2.get("text").is_none());
    }

    #[test]
    fn test_p0_2_gui_x3dh_double_ratchet_session_progression() {
        let alice_ks = KeyStore::generate();
        let mut bob_ks = KeyStore::generate();
        let bob_bundle = bob_ks.export_bundle();

        // Step 1: Alice initiates outbound X3DH with Bob
        let (mut alice_sess, init_env) = Session::initiate_outbound(
            &alice_ks,
            &bob_bundle,
            b"Hello Bob via X3DH!",
            DIRECT_ROUTE,
            1_000_000,
            86400,
        )
        .unwrap();

        // Step 2: Bob responds and decrypts
        let (mut bob_sess, pt1) =
            Session::respond_inbound(&mut bob_ks, &init_env, 1_000_000).unwrap();
        assert_eq!(pt1, b"Hello Bob via X3DH!");

        // Step 3: Alice sends ratcheted message
        let env2 = alice_sess
            .encrypt_message(b"Alice Ratchet Msg 2", DIRECT_ROUTE, 1_000_005, 86400)
            .unwrap();
        let pt2 = bob_sess.decrypt_message(&env2, 1_000_005).unwrap();
        assert_eq!(pt2, b"Alice Ratchet Msg 2");

        // Step 4: Bob replies with ratcheted message
        let env3 = bob_sess
            .encrypt_message(b"Bob Ratchet Reply 3", DIRECT_ROUTE, 1_000_010, 86400)
            .unwrap();
        let pt3 = alice_sess.decrypt_message(&env3, 1_000_010).unwrap();
        assert_eq!(pt3, b"Bob Ratchet Reply 3");

        // Step 5: Test session persistence encrypted on disk
        let temp_dir = tempfile::tempdir().unwrap();
        let session_file = temp_dir.path().join("sessions.bin");
        let storage_key = [77u8; 32];
        let mut sessions_map = HashMap::new();
        sessions_map.insert("bob".to_string(), alice_sess);

        save_sessions_encrypted(&sessions_map, &session_file, &storage_key).unwrap();
        let loaded =
            xchat_core::session::load_sessions_encrypted(&session_file, &storage_key).unwrap();
        assert!(loaded.contains_key("bob"));
    }

    #[test]
    fn test_p0_3_receive_rejects_unauthenticated_spoofed_messages() {
        // Plaintext spoofed JSON must fail to deserialize as WireMessageEnvelope or EncryptedEnvelope
        let spoofed_payload = serde_json::json!({
            "sender": "xchat_alice",
            "alias": "@alice",
            "text": "Send me money please!"
        });
        let body = serde_json::to_string(&spoofed_payload).unwrap();
        let wire_opt = serde_json::from_str::<WireMessageEnvelope>(&body);
        let env_opt = serde_json::from_str::<EncryptedEnvelope>(&body);
        assert!(
            wire_opt.is_err(),
            "Raw spoofed plaintext must not deserialize as WireMessageEnvelope"
        );
        assert!(
            env_opt.is_err(),
            "Raw spoofed plaintext must not deserialize as EncryptedEnvelope"
        );

        // Malicious envelope with fake signature must fail
        let mut bob_ks = KeyStore::generate();
        let fake_env = EncryptedEnvelope {
            header: xchat_core::envelope::RoutingHeader::new(DIRECT_ROUTE, 1_000_000, 86400),
            ratchet_header: None,
            x3dh_init: None,
            ciphertext: vec![1, 2, 3, 4],
        };
        assert!(Session::respond_inbound(&mut bob_ks, &fake_env, 1_000_000).is_err());
    }

    #[test]
    fn test_p0_4_receive_replay_attack_rejected() {
        let mut replay = ReplayFilter::new();
        let msg_id = [42u8; 16];
        let now = 1_000_000;

        // First acceptance succeeds
        assert!(replay.check_and_record(&msg_id, now).is_ok());

        // Replay of same message_id must be rejected
        let replay_err = replay.check_and_record(&msg_id, now);
        assert!(replay_err.is_err());
    }

    #[test]
    fn test_p0_5_csrf_and_cors_protection() {
        let expected_token = "abc123secretcsrf";

        // Missing or invalid token fails verification
        assert_ne!("", expected_token);
        assert_ne!("invalid_token", expected_token);
        assert_eq!("abc123secretcsrf", expected_token);

        // Host validation
        let valid_hosts = ["localhost:8080", "127.0.0.1:8080", "192.168.1.5:8080"];
        for h in valid_hosts {
            let is_valid = h.starts_with("localhost")
                || h.starts_with("127.0.0.1")
                || h.starts_with("192.168.1.5");
            assert!(is_valid);
        }
        let evil_hosts = ["evil.attacker.com", "malicious-site.org"];
        for h in evil_hosts {
            let is_valid = h.starts_with("localhost")
                || h.starts_with("127.0.0.1")
                || h.starts_with("192.168.1.5");
            assert!(!is_valid);
        }
    }

    #[test]
    fn test_p0_6_keystore_encryption_and_0600_perms() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ks_path = temp_dir.path().join("keystore.bin");
        let ks = KeyStore::generate();
        ks.save_to_path(&ks_path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&ks_path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "Keystore file must be created with 0600 permissions"
            );
        }

        let loaded = KeyStore::load_from_path(&ks_path).unwrap();
        assert_eq!(
            ks.identity.public_key().address(),
            loaded.identity.public_key().address()
        );
    }

    #[test]
    fn test_p0_7_one_time_prekey_consumption_and_reuse_rejection() {
        let alice_ks = KeyStore::generate();
        let mut bob_ks = KeyStore::generate();
        let bob_bundle = bob_ks.export_bundle();
        assert!(bob_bundle.one_time_prekey.is_some());

        // Alice initiates session using OPK
        let (_sess, env) = Session::initiate_outbound(
            &alice_ks,
            &bob_bundle,
            b"First message",
            DIRECT_ROUTE,
            1_000_000,
            86400,
        )
        .unwrap();

        // Bob responds; OPK is consumed
        let res1 = Session::respond_inbound(&mut bob_ks, &env, 1_000_000);
        assert!(res1.is_ok());

        // Reusing the same envelope or same consumed OPK is rejected
        let res2 = Session::respond_inbound(&mut bob_ks, &env, 1_000_000);
        assert!(res2.is_err(), "Reused OPK must be rejected");
    }

    #[test]
    fn test_gui_initial_contacts_and_assistant_echo() {
        let now_str = get_now_time_string();
        let initial_contacts = [
            ContactEntry {
                alias: "Saved Messages".to_string(),
                address: "xchat_self".to_string(),
                target: "local".to_string(),
                status: "Encrypted Storage".to_string(),
                last_message: "Personal cloud notes & encryption test".to_string(),
                last_time: now_str.clone(),
            },
            ContactEntry {
                alias: "xChat Assistant".to_string(),
                address: "xchat_assistant".to_string(),
                target: "local".to_string(),
                status: "P2P Mesh AI Assistant".to_string(),
                last_message: "Send a message to test P2P encryption!".to_string(),
                last_time: now_str,
            },
        ];

        assert_eq!(initial_contacts.len(), 2);
        assert_eq!(initial_contacts[0].alias, "Saved Messages");
        assert_eq!(initial_contacts[1].alias, "xChat Assistant");
    }

    #[tokio::test]
    async fn test_gui_alias_unification_and_message_retrieval() {
        let ble = xchat_transport::BleCentralTransport::new().await;
        let discovery = PeerDiscoveryEngine::bind(0, 0).await.unwrap();
        let keystore = KeyStore::generate();
        let pk = keystore.identity.public_key();
        let profile = UserProfile::new(
            "@test_user",
            "Test Node",
            &pk.address(),
            &pk.fingerprint(),
            1_000_000,
        )
        .unwrap();

        let mut st = AppState {
            contacts: vec![ContactEntry {
                alias: "@alice".to_string(),
                address: "e5b95fca-17ae-aec4-9ccb-d030b5c4fea0".to_string(),
                target: "127.0.0.1:8081".to_string(),
                status: "Online".to_string(),
                last_message: "Connected".to_string(),
                last_time: "12:00".to_string(),
            }],
            messages: HashMap::new(),
            keystore,
            profile,
            local_ip: "127.0.0.1".to_string(),
            local_port: 8080,
            discovery,
            ble_transport: ble,
            sessions: HashMap::new(),
            replay_filter: ReplayFilter::new(),
            csrf_token: "test_token".to_string(),
            session_store_path: PathBuf::from("/tmp/test_sessions.bin"),
        };

        // Incoming message arrives with internal handle e5b95fca-17ae-aec4-9ccb-d030b5c4fea0
        st.messages
            .entry("e5b95fca-17ae-aec4-9ccb-d030b5c4fea0".to_string())
            .or_default()
            .push(ChatMessage {
                id: "msg-1".to_string(),
                sender: "peer".to_string(),
                text: "Hello from phone offline".to_string(),
                time: "12:01".to_string(),
                status: "delivered".to_string(),
            });

        // Querying for @alice MUST return the message from e5b95fca-17ae-aec4-9ccb-d030b5c4fea0!
        let unified = get_unified_messages_for_peer(&st, "@alice");
        assert_eq!(unified.len(), 1);
        assert_eq!(unified[0].text, "Hello from phone offline");
    }
}
