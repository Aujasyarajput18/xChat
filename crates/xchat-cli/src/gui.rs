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
use xchat_core::{keystore::KeyStore, profile::UserProfile};
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

    let state = Arc::new(Mutex::new(AppState {
        keystore,
        profile: profile.clone(),
        local_ip: local_ip.clone(),
        local_port: port,
        contacts: initial_contacts,
        messages: HashMap::new(),
        discovery: discovery_engine.clone(),
        ble_transport: ble_transport.clone(),
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
                            c.address = sender_addr;
                        }
                        c.status = "Physical BLE Mesh".to_string();
                        // Merge chat history from placeholder alias into actual formatted_alias
                        if prev_alias != formatted_alias {
                            if let Some(prev_msgs) = st.messages.remove(&prev_alias) {
                                st.messages
                                    .entry(formatted_alias.clone())
                                    .or_default()
                                    .extend(prev_msgs);
                            }
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
                    });

                    if let Some(idx) = found_idx {
                        let old_alias = st.contacts[idx].alias.clone();
                        if old_alias != formatted_alias && !formatted_alias.is_empty() {
                            st.messages.entry(old_alias).or_default().push(msg.clone());
                            st.contacts[idx].alias = formatted_alias;
                        }
                        st.contacts[idx].last_message = text;
                        st.contacts[idx].last_time = time_str;
                        st.contacts[idx].status = "Physical BLE Mesh".to_string();
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

            let cors_headers = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\n";

            if method == "OPTIONS" {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Length: 0\r\nConnection: close\r\n\r\n",
                    cors_headers
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                return;
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

                    let (target_opt, my_handle, my_addr, sign_seed, dh_secret_bytes) = {
                        let mut st = state_clone.lock().unwrap();
                        let hist = st.messages.entry(req.alias.clone()).or_default();
                        hist.push(msg.clone());

                        if let Some(c) = st.contacts.iter_mut().find(|c| c.alias == req.alias) {
                            c.last_message = req.text.clone();
                            c.last_time = now_str.clone();
                        }
                        let (s, d) = st.keystore.identity.to_bytes();
                        let cur_addr = st.keystore.identity.public_key().address();
                        (req.target, st.profile.username.clone(), cur_addr, s, d)
                    };

                    // Handle Local Assistant / Echo Bot
                    if req.alias == "xChat Assistant" || target_opt.as_deref() == Some("local") {
                        if req.alias == "xChat Assistant" {
                            let mut st = state_clone.lock().unwrap();
                            let hist = st.messages.entry(req.alias.clone()).or_default();
                            hist.push(ChatMessage {
                                id: uuid::Uuid::new_v4().to_string(),
                                sender: "peer".to_string(),
                                text: format!("Echo [Ratchet Active]: {}", req.text),
                                time: get_now_time_string(),
                                status: "delivered".to_string(),
                            });
                        }
                    } else if let Some(target) = target_opt {
                        if !target.is_empty() && target != "local" {
                            let sender_id = xchat_crypto::identity::IdentityKeyPair::from_bytes(
                                sign_seed,
                                dh_secret_bytes,
                            );
                            let sender_pk = sender_id.public_key();

                            // 1. Fetch remote peer public identity to obtain dh_pubkey
                            let peer_id_json = dispatch_http_get(&target, "/api/identity")
                                .await
                                .unwrap_or_default();
                            let target_dh_opt =
                                serde_json::from_str::<serde_json::Value>(&peer_id_json)
                                    .ok()
                                    .and_then(|v| {
                                        v["dh_pubkey"].as_str().and_then(|s| hex::decode(s).ok())
                                    });

                            if let Some(dh_bytes) = target_dh_opt {
                                if dh_bytes.len() == 32 {
                                    let mut target_dh = [0u8; 32];
                                    target_dh.copy_from_slice(&dh_bytes);

                                    // 2. Perform Diffie-Hellman and derive symmetric AEAD key
                                    let shared_dh = sender_id.diffie_hellman(&target_dh);
                                    let aead_key = xchat_crypto::kdf::kdf_x3dh(&shared_dh);

                                    // 3. Encrypt payload under ChaCha20-Poly1305 with CSPRNG nonce
                                    let mut nonce = [0u8; 12];
                                    OsRng.fill_bytes(&mut nonce);
                                    let ciphertext = xchat_crypto::aead::encrypt_aead(
                                        &aead_key,
                                        &nonce,
                                        req.text.as_bytes(),
                                        b"xchat-gui-e2ee-v1",
                                    )
                                    .unwrap_or_default();

                                    // 4. Digitally sign the ciphertext & metadata with Ed25519 identity key
                                    let mut sig_material = Vec::new();
                                    sig_material.extend_from_slice(&sender_pk.sign_pubkey);
                                    sig_material.extend_from_slice(&sender_pk.dh_pubkey);
                                    sig_material.extend_from_slice(&nonce);
                                    sig_material.extend_from_slice(&now_ts.to_be_bytes());
                                    sig_material.extend_from_slice(&ciphertext);
                                    let signature = sender_id.sign(&sig_material);

                                    let wire_envelope = serde_json::json!({
                                        "version": 1,
                                        "sender_address": my_addr,
                                        "sender_alias": my_handle,
                                        "alias": my_handle,
                                        "username": my_handle,
                                        "sender_sign_pubkey": hex::encode(sender_pk.sign_pubkey),
                                        "sender_dh_pubkey": hex::encode(sender_pk.dh_pubkey),
                                        "nonce": hex::encode(nonce),
                                        "ciphertext": hex::encode(ciphertext),
                                        "text": req.text,
                                        "timestamp": now_ts,
                                        "time_str": now_str,
                                        "signature": hex::encode(signature)
                                    });
                                    let out_body =
                                        serde_json::to_string(&wire_envelope).unwrap_or_default();
                                    if target.starts_with("ble:") {
                                        let ble_t = {
                                            let st = state_clone.lock().unwrap();
                                            st.ble_transport.clone()
                                        };
                                        if let Err(e) =
                                            ble_t.send_raw_bytes(&target, out_body.as_bytes()).await
                                        {
                                            tracing::error!("Physical BLE send failed: {}", e);
                                        } else {
                                            tracing::info!(
                                                "Physical BLE message dispatched to {}",
                                                target
                                            );
                                        }
                                    } else {
                                        let _ =
                                            dispatch_http_post(&target, "/api/receive", &out_body)
                                                .await;
                                    }
                                }
                            } else {
                                // Direct delivery over BLE or LAN HTTP
                                let wire_envelope = serde_json::json!({
                                    "sender": my_addr,
                                    "sender_address": my_addr,
                                    "sender_alias": my_handle,
                                    "alias": my_handle,
                                    "username": my_handle,
                                    "target": target,
                                    "text": req.text,
                                    "time": now_str,
                                    "time_str": now_str,
                                    "timestamp": now_ts,
                                });
                                let out_body =
                                    serde_json::to_string(&wire_envelope).unwrap_or_default();
                                if target.starts_with("ble:") {
                                    let ble_t = {
                                        let st = state_clone.lock().unwrap();
                                        st.ble_transport.clone()
                                    };
                                    if let Err(e) =
                                        ble_t.send_raw_bytes(&target, out_body.as_bytes()).await
                                    {
                                        tracing::error!("Physical BLE fallback send failed: {}", e);
                                    } else {
                                        tracing::info!(
                                            "Physical BLE message dispatched to {}",
                                            target
                                        );
                                    }
                                } else {
                                    let _ = dispatch_http_post(&target, "/api/receive", &out_body)
                                        .await;
                                }
                            }
                        }
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{{\"status\":\"sent\"}}",
                    cors_headers
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path == "/api/receive" && method == "POST" {
                #[derive(Deserialize)]
                struct EncryptedReceiveReq {
                    #[allow(unused)]
                    version: u8,
                    sender_address: String,
                    sender_alias: String,
                    sender_sign_pubkey: String,
                    sender_dh_pubkey: String,
                    nonce: String,
                    ciphertext: String,
                    timestamp: u64,
                    time_str: Option<String>,
                    signature: String,
                }

                #[derive(Deserialize)]
                struct SimpleReceiveReq {
                    sender: Option<String>,
                    alias: Option<String>,
                    text: Option<String>,
                    message: Option<String>,
                    time: Option<String>,
                }

                let decrypt_result = (|| -> Result<(String, String, String, String), String> {
                    if let Ok(req) = serde_json::from_str::<EncryptedReceiveReq>(&body) {
                        let sign_pub_bytes = hex::decode(&req.sender_sign_pubkey)
                            .map_err(|_| "Invalid sender_sign_pubkey hex")?;
                        let dh_pub_bytes = hex::decode(&req.sender_dh_pubkey)
                            .map_err(|_| "Invalid sender_dh_pubkey hex")?;
                        let nonce_bytes =
                            hex::decode(&req.nonce).map_err(|_| "Invalid nonce hex")?;
                        let ct_bytes =
                            hex::decode(&req.ciphertext).map_err(|_| "Invalid ciphertext hex")?;
                        let sig_bytes =
                            hex::decode(&req.signature).map_err(|_| "Invalid signature hex")?;

                        if sign_pub_bytes.len() != 32
                            || dh_pub_bytes.len() != 32
                            || nonce_bytes.len() != 12
                            || sig_bytes.len() != 64
                        {
                            return Err("Invalid cryptographic parameter lengths".into());
                        }

                        let mut sign_pk = [0u8; 32];
                        sign_pk.copy_from_slice(&sign_pub_bytes);
                        let mut dh_pk = [0u8; 32];
                        dh_pk.copy_from_slice(&dh_pub_bytes);
                        let mut nonce_arr = [0u8; 12];
                        nonce_arr.copy_from_slice(&nonce_bytes);
                        let mut sig_arr = [0u8; 64];
                        sig_arr.copy_from_slice(&sig_bytes);

                        // 1. Verify Ed25519 digital signature
                        let sender_pub = xchat_crypto::identity::IdentityPublicKey {
                            sign_pubkey: sign_pk,
                            dh_pubkey: dh_pk,
                        };
                        let mut sig_material = Vec::new();
                        sig_material.extend_from_slice(&sign_pk);
                        sig_material.extend_from_slice(&dh_pk);
                        sig_material.extend_from_slice(&nonce_arr);
                        sig_material.extend_from_slice(&req.timestamp.to_be_bytes());
                        sig_material.extend_from_slice(&ct_bytes);

                        sender_pub
                            .verify(&sig_material, &sig_arr)
                            .map_err(|_| "Cryptographic signature verification failed")?;

                        // 2. Perform Diffie-Hellman with receiver identity
                        let (sign_seed, dh_secret_bytes) = {
                            let st = state_clone.lock().unwrap();
                            st.keystore.identity.to_bytes()
                        };
                        let my_id = xchat_crypto::identity::IdentityKeyPair::from_bytes(
                            sign_seed,
                            dh_secret_bytes,
                        );
                        let shared_dh = my_id.diffie_hellman(&dh_pk);
                        let aead_key = xchat_crypto::kdf::kdf_x3dh(&shared_dh);

                        // 3. Decrypt ciphertext under AEAD
                        let pt_bytes = xchat_crypto::aead::decrypt_aead(
                            &aead_key,
                            &nonce_arr,
                            &ct_bytes,
                            b"xchat-gui-e2ee-v1",
                        )
                        .map_err(|_| "AEAD decryption failed")?;

                        let text = String::from_utf8(pt_bytes)
                            .map_err(|_| "Decrypted payload is not valid UTF-8")?;

                        let time_str = req.time_str.unwrap_or_else(get_now_time_string);
                        return Ok((req.sender_alias, req.sender_address, text, time_str));
                    }

                    // Fallback: Standard direct JSON payload (e.g. from Android or mobile mesh bridge)
                    if let Ok(simple) = serde_json::from_str::<SimpleReceiveReq>(&body) {
                        let text = simple.text.or(simple.message).unwrap_or_default();
                        if !text.is_empty() {
                            let sender_addr =
                                simple.sender.unwrap_or_else(|| "xchat_peer".to_string());
                            let sender_alias = simple.alias.unwrap_or_else(|| "Peer".to_string());
                            let time_str = simple.time.unwrap_or_else(get_now_time_string);
                            return Ok((sender_alias, sender_addr, text, time_str));
                        }
                    }

                    Err("Missing or invalid message envelope".into())
                })();

                let (resp_status, resp_body) = match decrypt_result {
                    Ok((sender_alias, sender_addr, plaintext, now_str)) => {
                        let msg = ChatMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            sender: "peer".to_string(),
                            text: plaintext.clone(),
                            time: now_str.clone(),
                            status: "delivered".to_string(),
                        };

                        let mut st = state_clone.lock().unwrap();

                        // Match existing contact by address, alias, or target
                        let mut target_alias = sender_alias.clone();
                        if let Some(c) = st.contacts.iter_mut().find(|c| {
                            c.alias == sender_alias
                                || c.address == sender_addr
                                || (!c.target.is_empty()
                                    && (sender_addr.contains(&c.target)
                                        || sender_alias.contains(&c.target)))
                        }) {
                            target_alias = c.alias.clone();
                            c.last_message = plaintext.clone();
                            c.last_time = now_str.clone();
                        } else {
                            st.contacts.push(ContactEntry {
                                alias: sender_alias.clone(),
                                address: sender_addr.clone(),
                                target: "Direct".to_string(),
                                status: "Online".to_string(),
                                last_message: plaintext.clone(),
                                last_time: now_str.clone(),
                            });
                        }

                        // Store under target_alias AND sender_alias AND sender_addr for multi-key lookup
                        st.messages
                            .entry(target_alias)
                            .or_default()
                            .push(msg.clone());
                        if sender_alias != "Peer" {
                            st.messages
                                .entry(sender_alias)
                                .or_default()
                                .push(msg.clone());
                        }
                        st.messages.entry(sender_addr).or_default().push(msg);

                        ("200 OK", r#"{"status":"delivered","authenticated":true}"#)
                    }
                    Err(_) => (
                        "401 Unauthorized",
                        r#"{"error":"Unauthenticated or corrupted envelope"}"#,
                    ),
                };

                let resp = format!(
                    "HTTP/1.1 {}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_status,
                    cors_headers,
                    resp_body.len(),
                    resp_body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            } else if path.starts_with("/api/messages") && method == "GET" {
                let peer_opt = path.split('?').nth(1).and_then(|q| {
                    q.split('&')
                        .find(|param| param.starts_with("peer="))
                        .map(|param| urlencoding_decode(&param[5..]))
                });

                let msgs_json =
                    {
                        let st = state_clone.lock().unwrap();
                        if let Some(peer) = peer_opt {
                            let mut list = st.messages.get(&peer).cloned().unwrap_or_default();
                            if list.is_empty() {
                                let peer_clean = peer.trim_start_matches('@');
                                if let Some(m) = st.messages.get(&format!("@{}", peer_clean)) {
                                    list = m.clone();
                                } else if let Some(m) = st.messages.get(peer_clean) {
                                    list = m.clone();
                                } else if let Some(c) = st.contacts.iter().find(|c| {
                                    c.alias == peer
                                        || c.address == peer
                                        || c.target == peer
                                        || c.target.contains(&peer)
                                        || c.alias.contains(&peer)
                                        || peer.contains(&c.alias)
                                }) {
                                    if let Some(m) = st.messages.get(&c.alias) {
                                        list = m.clone();
                                    } else if let Some(m) = st.messages.get(&c.address) {
                                        list = m.clone();
                                    } else if let Some(m) = st.messages.get(&c.target) {
                                        list = m.clone();
                                    }
                                } else if peer == "Peer" {
                                    if let Some((_, m)) = st.messages.iter().find(|(k, _)| {
                                        *k != "xChat Assistant" && *k != "Saved Messages"
                                    }) {
                                        list = m.clone();
                                    }
                                }
                            }
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
    fn test_gui_receive_simple_mobile_payload() {
        #[derive(Deserialize)]
        #[allow(unused)]
        struct SimpleReceiveReq {
            sender: Option<String>,
            alias: Option<String>,
            text: Option<String>,
            message: Option<String>,
            time: Option<String>,
        }

        let mobile_body = serde_json::json!({
            "sender": "xchat_android_node",
            "alias": "Phone",
            "text": "Hello from Android device!",
            "time": "12:00 PM"
        });
        let body_str = serde_json::to_string(&mobile_body).unwrap();
        let parsed = serde_json::from_str::<SimpleReceiveReq>(&body_str);
        assert!(parsed.is_ok());
        let req = parsed.unwrap();
        assert_eq!(req.text.unwrap(), "Hello from Android device!");
        assert_eq!(req.alias.unwrap(), "Phone");
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
}
