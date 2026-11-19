use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::errors::TransportError;

pub const BLE_FRAME_MAGIC: [u8; 2] = [0x58, 0x43]; // 'X', 'C'
pub const BLE_HEADER_LEN: usize = 10;
pub const DEFAULT_BLE_MTU: usize = 512;
pub const MIN_BLE_MTU: usize = 23;
pub const MAX_INCOMPLETE_AGE_SECS: u64 = 60;

/// Header prepended to every physical Bluetooth Low Energy chunk packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleFrameHeader {
    pub msg_id: u16,
    pub chunk_index: u16,
    pub total_chunks: u16,
    pub payload_len: u16,
}

impl BleFrameHeader {
    pub fn new(msg_id: u16, chunk_index: u16, total_chunks: u16, payload_len: u16) -> Self {
        Self {
            msg_id,
            chunk_index,
            total_chunks,
            payload_len,
        }
    }

    pub fn serialize(&self) -> [u8; BLE_HEADER_LEN] {
        let mut buf = [0u8; BLE_HEADER_LEN];
        buf[0..2].copy_from_slice(&BLE_FRAME_MAGIC);
        buf[2..4].copy_from_slice(&self.msg_id.to_be_bytes());
        buf[4..6].copy_from_slice(&self.chunk_index.to_be_bytes());
        buf[6..8].copy_from_slice(&self.total_chunks.to_be_bytes());
        buf[8..10].copy_from_slice(&self.payload_len.to_be_bytes());
        buf
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self, TransportError> {
        if bytes.len() < BLE_HEADER_LEN {
            return Err(TransportError::FrameTooShort);
        }
        if bytes[0..2] != BLE_FRAME_MAGIC {
            return Err(TransportError::InvalidFrameHeader);
        }
        let msg_id = u16::from_be_bytes([bytes[2], bytes[3]]);
        let chunk_index = u16::from_be_bytes([bytes[4], bytes[5]]);
        let total_chunks = u16::from_be_bytes([bytes[6], bytes[7]]);
        let payload_len = u16::from_be_bytes([bytes[8], bytes[9]]);

        if total_chunks == 0 || chunk_index >= total_chunks {
            return Err(TransportError::InvalidFrameHeader);
        }

        Ok(Self {
            msg_id,
            chunk_index,
            total_chunks,
            payload_len,
        })
    }
}

/// Slices arbitrary binary encrypted envelopes into physical MTU-sized BLE packets.
pub struct BleChunker;

impl BleChunker {
    pub fn chunk_payload(payload: &[u8], mtu: usize, msg_id: u16) -> Vec<Vec<u8>> {
        let effective_mtu = mtu.max(MIN_BLE_MTU);
        let max_chunk_payload = effective_mtu.saturating_sub(BLE_HEADER_LEN).max(1);

        if payload.is_empty() {
            let header = BleFrameHeader::new(msg_id, 0, 1, 0);
            return vec![header.serialize().to_vec()];
        }

        let total_chunks = (payload.len() + max_chunk_payload - 1) / max_chunk_payload;
        let total_chunks_u16 = total_chunks.min(u16::MAX as usize) as u16;

        let mut chunks = Vec::with_capacity(total_chunks);

        for (idx, slice) in payload.chunks(max_chunk_payload).enumerate() {
            let header = BleFrameHeader::new(
                msg_id,
                idx as u16,
                total_chunks_u16,
                slice.len() as u16,
            );
            let mut packet = Vec::with_capacity(BLE_HEADER_LEN + slice.len());
            packet.extend_from_slice(&header.serialize());
            packet.extend_from_slice(slice);
            chunks.push(packet);
        }

        chunks
    }
}

struct PendingBleMessage {
    total_chunks: u16,
    received_chunks: HashMap<u16, Vec<u8>>,
    first_seen: u64,
}

/// Reassembles streamed or out-of-order BLE chunks back into the complete envelope.
pub struct BleReassembler {
    pending: HashMap<u16, PendingBleMessage>,
}

impl BleReassembler {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    pub fn process_packet(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, TransportError> {
        let header = BleFrameHeader::deserialize(packet)?;
        let payload_slice = &packet[BLE_HEADER_LEN..];

        if payload_slice.len() != header.payload_len as usize {
            return Err(TransportError::CorruptPayload);
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Prune stale incomplete transfers older than MAX_INCOMPLETE_AGE_SECS
        self.pending.retain(|_, v| now.saturating_sub(v.first_seen) < MAX_INCOMPLETE_AGE_SECS);

        let entry = self
            .pending
            .entry(header.msg_id)
            .or_insert_with(|| PendingBleMessage {
                total_chunks: header.total_chunks,
                received_chunks: HashMap::new(),
                first_seen: now,
            });

        entry
            .received_chunks
            .insert(header.chunk_index, payload_slice.to_vec());

        if entry.received_chunks.len() == entry.total_chunks as usize {
            let mut assembled = Vec::new();
            for i in 0..entry.total_chunks {
                if let Some(chunk) = entry.received_chunks.get(&i) {
                    assembled.extend_from_slice(chunk);
                } else {
                    return Ok(None);
                }
            }
            self.pending.remove(&header.msg_id);
            Ok(Some(assembled))
        } else {
            Ok(None)
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for BleReassembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ble_frame_header_roundtrip() {
        let header = BleFrameHeader::new(1042, 3, 10, 480);
        let bytes = header.serialize();
        let decoded = BleFrameHeader::deserialize(&bytes).unwrap();
        assert_eq!(header, decoded);
    }

    #[test]
    fn test_ble_frame_header_invalid_magic() {
        let mut bytes = BleFrameHeader::new(1, 0, 1, 10).serialize();
        bytes[0] = 0x00;
        assert!(BleFrameHeader::deserialize(&bytes).is_err());
    }

    #[test]
    fn test_ble_chunker_and_reassembler_roundtrip() {
        let mut reassembler = BleReassembler::new();
        let payload = b"Hello from pure physical Bluetooth Low Energy hardware radio!";
        let chunks = BleChunker::chunk_payload(payload, 23, 77); // Tiny 23-byte MTU (forces multiple chunks)

        assert!(chunks.len() > 1);

        let mut completed = None;
        for c in chunks {
            if let Some(res) = reassembler.process_packet(&c).unwrap() {
                completed = Some(res);
            }
        }

        assert_eq!(completed.unwrap(), payload);
        assert_eq!(reassembler.pending_count(), 0);
    }

    #[test]
    fn test_ble_out_of_order_chunks_reassembly() {
        let mut reassembler = BleReassembler::new();
        let payload = b"Large encrypted wire payload over BLE GATT characteristic write.";
        let mut chunks = BleChunker::chunk_payload(payload, 30, 999);

        // Shuffle chunks to deliver out-of-order
        chunks.reverse();

        let mut completed = None;
        for c in chunks {
            if let Some(res) = reassembler.process_packet(&c).unwrap() {
                completed = Some(res);
            }
        }

        assert_eq!(completed.unwrap(), payload);
    }

    #[test]
    fn test_ble_corrupt_payload_length_rejected() {
        let mut reassembler = BleReassembler::new();
        let payload = b"Test payload";
        let mut chunks = BleChunker::chunk_payload(payload, 512, 1);
        // Truncate chunk bytes
        chunks[0].pop();
        assert!(reassembler.process_packet(&chunks[0]).is_err());
    }
}
