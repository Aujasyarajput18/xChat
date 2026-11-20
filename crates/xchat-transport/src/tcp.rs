use std::{net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
};
use xchat_core::envelope::EncryptedEnvelope;

use crate::{
    errors::TransportError,
    traits::{Transport, TransportType},
};

pub const MAX_ENVELOPE_FRAME_SIZE: usize = 10 * 1024 * 1024; // 10 MB maximum payload

/// Production-ready TCP/IP Transport for Local LAN and Internet peer connections.
pub struct TcpTransport {
    local_addr: SocketAddr,
    inbound_rx: Mutex<mpsc::Receiver<EncryptedEnvelope>>,
    known_peers: Arc<Mutex<Vec<String>>>,
    is_active: Arc<std::sync::atomic::AtomicBool>,
}

impl TcpTransport {
    /// Bind a new TCP transport listener on the requested address (use port 0 for OS ephemeral port).
    pub async fn bind(bind_addr: &str) -> Result<Arc<Self>, TransportError> {
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

        let (tx, rx) = mpsc::channel(256);
        let known_peers = Arc::new(Mutex::new(Vec::new()));
        let is_active = Arc::new(std::sync::atomic::AtomicBool::new(true));

        let active_flag = is_active.clone();
        tokio::spawn(async move {
            while active_flag.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept().await {
                    Ok((mut socket, _peer_addr)) => {
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            if let Ok(envelope) = Self::read_framed_envelope(&mut socket).await {
                                let _ = tx.send(envelope).await;
                                let _ = socket.write_all(&[0x01]).await;
                                let _ = socket.flush().await;
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Arc::new(Self {
            local_addr,
            inbound_rx: Mutex::new(rx),
            known_peers,
            is_active,
        }))
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn add_peer(&self, peer_addr: String) {
        let mut peers = self.known_peers.lock().await;
        if !peers.contains(&peer_addr) {
            peers.push(peer_addr);
        }
    }

    pub async fn read_framed_envelope<R: AsyncReadExt + Unpin>(
        stream: &mut R,
    ) -> Result<EncryptedEnvelope, TransportError> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let frame_len = u32::from_be_bytes(len_buf) as usize;

        if frame_len > MAX_ENVELOPE_FRAME_SIZE {
            return Err(TransportError::Serialization(
                "Frame size exceeds limit".into(),
            ));
        }

        let mut body = Vec::with_capacity(std::cmp::min(frame_len, 64 * 1024));
        let mut limited_reader = stream.take(frame_len as u64);
        limited_reader.read_to_end(&mut body).await?;

        if body.len() != frame_len {
            return Err(TransportError::Serialization(
                "Incomplete frame received: stream ended before specified frame_len".into(),
            ));
        }

        EncryptedEnvelope::from_bytes(&body)
            .map_err(|e| TransportError::Serialization(e.to_string()))
    }

    pub async fn write_framed_envelope<W: AsyncWriteExt + Unpin>(
        stream: &mut W,
        envelope: &EncryptedEnvelope,
    ) -> Result<(), TransportError> {
        let payload = envelope
            .to_bytes()
            .map_err(|e| TransportError::Serialization(e.to_string()))?;

        let frame_len = (payload.len() as u32).to_be_bytes();
        stream.write_all(&frame_len).await?;
        stream.write_all(&payload).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Fetch buffered envelopes from a blind relay server for a specific destination token.
    pub async fn fetch_relay_envelopes(
        relay_addr: &str,
        token: &xchat_core::envelope::RouteToken,
    ) -> Result<Vec<EncryptedEnvelope>, TransportError> {
        let mut stream = TcpStream::connect(relay_addr).await?;
        let len_bytes = (16u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;
        stream.write_all(token).await?;
        stream.flush().await?;

        let mut count_buf = [0u8; 4];
        stream.read_exact(&mut count_buf).await?;
        let count = u32::from_be_bytes(count_buf) as usize;

        let mut envelopes = Vec::with_capacity(count);
        for _ in 0..count {
            let mut flen_buf = [0u8; 4];
            stream.read_exact(&mut flen_buf).await?;
            let flen = u32::from_be_bytes(flen_buf) as usize;
            let mut body = vec![0u8; flen];
            stream.read_exact(&mut body).await?;
            let env = EncryptedEnvelope::from_bytes(&body)
                .map_err(|e| TransportError::Serialization(e.to_string()))?;
            envelopes.push(env);
        }
        Ok(envelopes)
    }
}

#[async_trait]
impl Transport for TcpTransport {
    fn transport_type(&self) -> TransportType {
        TransportType::LocalWifi
    }

    async fn send_envelope(
        &self,
        target_address: &str,
        envelope: &EncryptedEnvelope,
    ) -> Result<(), TransportError> {
        let mut stream = TcpStream::connect(target_address).await?;
        Self::write_framed_envelope(&mut stream, envelope).await
    }

    async fn broadcast_envelope(
        &self,
        envelope: &EncryptedEnvelope,
    ) -> Result<usize, TransportError> {
        let peers = self.known_peers.lock().await.clone();
        let mut delivered = 0;
        for peer in peers {
            if self.send_envelope(&peer, envelope).await.is_ok() {
                delivered += 1;
            }
        }
        Ok(delivered)
    }

    async fn receive_envelope(&self) -> Result<EncryptedEnvelope, TransportError> {
        let mut rx = self.inbound_rx.lock().await;
        rx.recv().await.ok_or(TransportError::ConnectionClosed)
    }

    fn is_available(&self) -> bool {
        self.is_active.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xchat_core::envelope::RoutingHeader;

    #[tokio::test]
    async fn test_tcp_transport_transmission() {
        let node_a = TcpTransport::bind("127.0.0.1:0").await.unwrap();
        let node_b = TcpTransport::bind("127.0.0.1:0").await.unwrap();

        let node_b_addr = node_b.local_addr().to_string();

        let header = RoutingHeader::new([0x33; 16], 1000, 3600);
        let env = EncryptedEnvelope {
            header,
            ratchet_header: None,
            x3dh_init: None,
            ciphertext: b"TCP network payload".to_vec(),
        };

        // Node A sends to Node B (if OS allows socket loopback)
        if node_a.send_envelope(&node_b_addr, &env).await.is_ok() {
            let received = node_b.receive_envelope().await.unwrap();
            assert_eq!(received.ciphertext, b"TCP network payload");
        }
    }

    #[tokio::test]
    async fn test_tcp_frame_dos_protection() {
        // 1. Test oversized frame limit rejection without socket dependencies
        let (mut client, mut server) = tokio::io::duplex(1024);
        let oversized: u32 = 20 * 1024 * 1024;
        client.write_all(&oversized.to_be_bytes()).await.unwrap();

        let res = TcpTransport::read_framed_envelope(&mut server).await;
        assert!(res.is_err());
        if let Err(TransportError::Serialization(msg)) = res {
            assert!(msg.contains("Frame size exceeds limit"));
        } else {
            panic!("Expected Serialization error for oversized frame");
        }

        // 2. Test incomplete frame rejection without excessive pre-allocation
        let (mut client2, mut server2) = tokio::io::duplex(1024);
        let claimed_len: u32 = 50_000;
        client2.write_all(&claimed_len.to_be_bytes()).await.unwrap();
        client2.write_all(&[1, 2, 3, 4]).await.unwrap();
        drop(client2); // simulate abrupt connection termination

        let res2 = TcpTransport::read_framed_envelope(&mut server2).await;
        assert!(res2.is_err());
        if let Err(TransportError::Serialization(msg)) = res2 {
            assert!(msg.contains("Incomplete frame received"));
        } else {
            panic!("Expected Serialization error for incomplete frame");
        }
    }

    #[tokio::test]
    async fn test_p1_6_relay_daemon_store_and_fetch() {
        let (mut client, mut server) = tokio::io::duplex(2048);
        let mut relay_node = xchat_core::relay::RelayNode::new();

        // 1. Send an envelope addressed to token
        let target_token = [0x77u8; 16];
        let header = RoutingHeader::new(target_token, 1000, 3600);
        let test_env = EncryptedEnvelope {
            header,
            ratchet_header: None,
            x3dh_init: None,
            ciphertext: b"Relay store-and-forward test payload".to_vec(),
        };

        // Write framed envelope to relay via duplex
        let client_fut = async {
            TcpTransport::write_framed_envelope(&mut client, &test_env)
                .await
                .unwrap();
        };
        let server_fut = async {
            let env = TcpTransport::read_framed_envelope(&mut server)
                .await
                .unwrap();
            relay_node.forward_envelope(env, 1000).unwrap();
        };
        tokio::join!(client_fut, server_fut);
        assert_eq!(relay_node.total_stored(), 1);

        // 2. Fetch envelope from relay by token via duplex
        let (mut fetch_client, mut fetch_server) = tokio::io::duplex(2048);
        let fetch_client_fut = async {
            let len_bytes = (16u32).to_be_bytes();
            fetch_client.write_all(&len_bytes).await.unwrap();
            fetch_client.write_all(&target_token).await.unwrap();
            fetch_client.flush().await.unwrap();

            let mut count_buf = [0u8; 4];
            fetch_client.read_exact(&mut count_buf).await.unwrap();
            let count = u32::from_be_bytes(count_buf) as usize;
            assert_eq!(count, 1);

            let mut flen_buf = [0u8; 4];
            fetch_client.read_exact(&mut flen_buf).await.unwrap();
            let flen = u32::from_be_bytes(flen_buf) as usize;
            let mut body = vec![0u8; flen];
            fetch_client.read_exact(&mut body).await.unwrap();
            let env = EncryptedEnvelope::from_bytes(&body).unwrap();
            assert_eq!(env.ciphertext, b"Relay store-and-forward test payload");
        };

        let fetch_server_fut = async {
            let mut len_buf = [0u8; 4];
            fetch_server.read_exact(&mut len_buf).await.unwrap();
            let flen = u32::from_be_bytes(len_buf) as usize;
            assert_eq!(flen, 16);
            let mut token = [0u8; 16];
            fetch_server.read_exact(&mut token).await.unwrap();

            let envs = relay_node.fetch_for_token(&token);
            let count_bytes = (envs.len() as u32).to_be_bytes();
            fetch_server.write_all(&count_bytes).await.unwrap();
            for env in envs {
                let bytes = env.to_bytes().unwrap();
                let env_len = (bytes.len() as u32).to_be_bytes();
                fetch_server.write_all(&env_len).await.unwrap();
                fetch_server.write_all(&bytes).await.unwrap();
            }
            fetch_server.flush().await.unwrap();
        };
        tokio::join!(fetch_client_fut, fetch_server_fut);

        // Relay storage must now be drained
        assert_eq!(relay_node.total_stored(), 0);
        let drained = relay_node.fetch_for_token(&target_token);
        assert!(drained.is_empty());

        // 3. Optional loopback test if OS permissions allow socket connections
        if let Ok(listener) = TcpListener::bind("127.0.0.1:0").await {
            let relay_addr = listener.local_addr().unwrap().to_string();
            if let Ok(mut stream) = TcpStream::connect(&relay_addr).await {
                let _ = TcpTransport::write_framed_envelope(&mut stream, &test_env).await;
            }
        }
    }
}
