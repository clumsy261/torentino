use std::collections::{HashMap, HashSet};

use sha1::{Digest, Sha1};

use crate::peer::BLOCK_SIZE;

pub struct PieceManager {
    pub total_pieces: u32,
    pub piece_length: u64,
    pub last_piece_length: u64,
    pub total_length: u64,
    have: Vec<bool>,
    piece_hashes: Vec<[u8; 20]>,
    pending_blocks: HashMap<(u32, u32), u32>,
    active_requests: HashMap<u32, HashSet<u32>>,
    received_blocks: HashMap<u32, HashSet<u32>>,
    buffers: Vec<Vec<u8>>,
    pub downloaded: u64,
    pub uploaded: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRequest {
    pub piece_index: u32,
    pub begin: u32,
    pub length: u32,
}

impl PieceManager {
    pub fn new(piece_length: u64, total_length: u64, piece_hashes: Vec<[u8; 20]>) -> Self {
        let total_pieces = piece_hashes.len() as u32;
        let last_piece_length = if total_pieces == 0 {
            0
        } else if total_length.is_multiple_of(piece_length) {
            piece_length
        } else {
            total_length % piece_length
        };

        PieceManager {
            total_pieces,
            piece_length,
            last_piece_length,
            total_length,
            have: vec![false; total_pieces as usize],
            piece_hashes,
            pending_blocks: HashMap::new(),
            active_requests: HashMap::new(),
            received_blocks: HashMap::new(),
            buffers: Vec::new(),
            downloaded: 0,
            uploaded: 0,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.have.iter().all(|&h| h)
    }

    pub fn completed_pieces(&self) -> usize {
        self.have.iter().filter(|&&h| h).count()
    }

    pub fn progress(&self) -> f64 {
        if self.total_pieces == 0 {
            return 1.0;
        }
        self.completed_pieces() as f64 / self.total_pieces as f64
    }

    pub fn piece_length(&self, piece_idx: u32) -> u64 {
        if piece_idx == self.total_pieces - 1
            && !self.total_length.is_multiple_of(self.piece_length)
        {
            self.last_piece_length
        } else {
            self.piece_length
        }
    }

    pub fn has_piece(&self, piece_idx: u32) -> bool {
        self.have.get(piece_idx as usize).copied().unwrap_or(true)
    }

    pub fn need_piece(&self, peer_bitfield: &[u8]) -> bool {
        for i in 0..self.total_pieces {
            if !self.has_piece(i) && Self::peer_has_piece(peer_bitfield, i) {
                return true;
            }
        }
        false
    }

    pub fn next_request(&mut self, peer_id: u32, peer_bitfield: &[u8]) -> Option<BlockRequest> {
        let mut best_piece: Option<u32> = None;
        let mut best_count = usize::MAX;

        for i in 0..self.total_pieces {
            if self.has_piece(i) || !Self::peer_has_piece(peer_bitfield, i) {
                continue;
            }
            let pending = self.active_requests.get(&i).map_or(0, |s| s.len());
            let piece_len = self.piece_length(i);
            let blocks_needed = (piece_len as usize).div_ceil(BLOCK_SIZE as usize);
            if pending >= blocks_needed {
                continue;
            }
            if pending < best_count {
                best_count = pending;
                best_piece = Some(i);
            }
        }

        let piece_idx = best_piece?;
        let piece_len = self.piece_length(piece_idx);
        let blocks_needed = (piece_len as usize).div_ceil(BLOCK_SIZE as usize);
        let received = self.received_blocks.get(&piece_idx);

        for block in 0..blocks_needed {
            let begin = block as u32 * BLOCK_SIZE;
            let key = (piece_idx, begin);
            if self.pending_blocks.contains_key(&key) {
                continue;
            }
            if received.is_some_and(|r| r.contains(&begin)) {
                continue;
            }

            let length = std::cmp::min(BLOCK_SIZE, piece_len as u32 - begin);
            self.pending_blocks.insert(key, peer_id);
            self.active_requests
                .entry(piece_idx)
                .or_default()
                .insert(begin);

            return Some(BlockRequest {
                piece_index: piece_idx,
                begin,
                length,
            });
        }

        None
    }

    pub fn block_received(&mut self, piece_idx: u32, begin: u32, data: &[u8]) -> Option<Vec<u8>> {
        if self.have[piece_idx as usize] {
            return None;
        }

        let key = (piece_idx, begin);
        self.pending_blocks.remove(&key);
        if let Some(sets) = self.active_requests.get_mut(&piece_idx) {
            sets.remove(&begin);
        }

        let buf_idx = piece_idx as usize;
        while self.buffers.len() <= buf_idx {
            self.buffers.push(Vec::new());
        }
        let buf = &mut self.buffers[buf_idx];
        let end = (begin as usize) + data.len();
        if buf.len() < end {
            buf.resize(end, 0);
        }
        buf[begin as usize..end].copy_from_slice(data);

        let piece_len = self.piece_length(piece_idx);
        let blocks_needed = (piece_len as usize).div_ceil(BLOCK_SIZE as usize);

        let received_len = {
            let received = self.received_blocks.entry(piece_idx).or_default();
            received.insert(begin);
            received.len()
        };
        if received_len >= blocks_needed {
            let piece_data = std::mem::take(&mut self.buffers[buf_idx]);
            return Some(piece_data);
        }

        None
    }

    pub fn verify_piece(&self, piece_idx: u32, data: &[u8]) -> bool {
        let expected = &self.piece_hashes[piece_idx as usize];
        let mut hasher = Sha1::new();
        hasher.update(data);
        let result: [u8; 20] = hasher.finalize().into();
        result == *expected
    }

    pub fn mark_complete(&mut self, piece_idx: u32, length: u64) {
        self.have[piece_idx as usize] = true;
        self.downloaded += length;
        self.received_blocks.remove(&piece_idx);
        self.active_requests.remove(&piece_idx);
    }

    pub fn resume_from(&mut self, read: impl Fn(u32, u64) -> Option<Vec<u8>>) -> u32 {
        let mut restored = 0u32;
        for idx in 0..self.total_pieces {
            let idx_u = idx as usize;
            if self.have[idx_u] {
                continue;
            }
            let len = self.piece_length(idx);
            let Some(data) = read(idx, len) else { continue };
            if data.len() as u64 != len {
                continue;
            }
            if self.verify_piece(idx, &data) {
                self.have[idx_u] = true;
                self.downloaded += len;
                restored += 1;
            }
        }
        restored
    }

    pub fn discard_piece(&mut self, piece_idx: u32) {
        self.received_blocks.remove(&piece_idx);
        self.active_requests.remove(&piece_idx);
        self.pending_blocks.retain(|(p, _), _| *p != piece_idx);
        let buf_idx = piece_idx as usize;
        if buf_idx < self.buffers.len() {
            self.buffers[buf_idx].clear();
        }
    }

    pub fn peer_has_piece(bitfield: &[u8], piece_idx: u32) -> bool {
        let byte_idx = (piece_idx / 8) as usize;
        let bit_offset = 7 - (piece_idx % 8);
        if byte_idx >= bitfield.len() {
            return false;
        }
        (bitfield[byte_idx] >> bit_offset) & 1 == 1
    }
}
