//! Aeneas-compatible Rust code for Lean verification
//!
//! This module contains simplified Rust code that can be translated to Lean 4
//! using Aeneas for formal verification.
//!
//! To verify:
//!   1. Install Aeneas: https://github.com/AeneasVerif/aeneas
//!   2. Run: aeneas -backend lean4 src/lib.rs
//!   3. Write proofs in the generated Lean files
//!
//! Note: Aeneas works best with simple, ownership-clear code.
//! Avoid: unsafe, RefCell, async, complex generics.

/// A simplified jitter buffer packet
#[derive(Clone)]
pub struct Packet {
    pub sequence: u16,
    pub payload_len: u32,
}

/// Simplified list for jitter buffer (Aeneas prefers explicit lists)
#[derive(Clone)]
pub enum PacketList {
    Nil,
    Cons(Packet, Box<PacketList>),
}

impl PacketList {
    /// Create an empty list
    pub fn new() -> Self {
        PacketList::Nil
    }

    /// Get the length of the list
    pub fn len(&self) -> u32 {
        match self {
            PacketList::Nil => 0,
            PacketList::Cons(_, tail) => 1 + tail.len(),
        }
    }

    /// Check if list is empty
    pub fn is_empty(&self) -> bool {
        matches!(self, PacketList::Nil)
    }

    /// Insert a packet (at front for simplicity)
    pub fn insert(self, packet: Packet) -> Self {
        PacketList::Cons(packet, Box::new(self))
    }

    /// Check if sequence is in list
    pub fn contains_seq(&self, seq: u16) -> bool {
        match self {
            PacketList::Nil => false,
            PacketList::Cons(p, tail) => {
                if p.sequence == seq {
                    true
                } else {
                    tail.contains_seq(seq)
                }
            }
        }
    }

    /// Remove first packet with given sequence
    pub fn remove_seq(self, seq: u16) -> (Option<Packet>, Self) {
        match self {
            PacketList::Nil => (None, PacketList::Nil),
            PacketList::Cons(p, tail) => {
                if p.sequence == seq {
                    (Some(p), *tail)
                } else {
                    let (found, new_tail) = tail.remove_seq(seq);
                    (found, PacketList::Cons(p, Box::new(new_tail)))
                }
            }
        }
    }
}

/// Check if sequence a is "before" sequence b (handles wraparound)
pub fn seq_is_before(seq_a: u16, seq_b: u16) -> bool {
    let diff = seq_b.wrapping_sub(seq_a);
    diff > 0 && diff < 0x8000
}

/// A simplified jitter buffer state
pub struct JitterBuffer {
    packets: PacketList,
    next_seq: Option<u16>,
    max_size: u32,
}

impl JitterBuffer {
    /// Create a new jitter buffer
    pub fn new(max_size: u32) -> Self {
        JitterBuffer {
            packets: PacketList::new(),
            next_seq: None,
            max_size,
        }
    }

    /// Get current buffer size
    pub fn size(&self) -> u32 {
        self.packets.len()
    }

    /// Insert a packet into the buffer
    /// Returns true if accepted, false if dropped (duplicate/late)
    pub fn insert(&mut self, packet: Packet) -> bool {
        let seq = packet.sequence;

        // Initialize next_seq if first packet
        if self.next_seq.is_none() {
            self.next_seq = Some(seq);
        }

        let next_seq = self.next_seq.unwrap();

        // Check if late (before next expected)
        if seq_is_before(seq, next_seq) {
            return false;
        }

        // Check for duplicate
        if self.packets.contains_seq(seq) {
            return false;
        }

        // Check max size (simplified: reject if full)
        if self.packets.len() >= self.max_size {
            return false;
        }

        // Accept packet
        let old_packets = std::mem::replace(&mut self.packets, PacketList::Nil);
        self.packets = old_packets.insert(packet);
        true
    }

    /// Pop the next expected packet
    pub fn pop(&mut self) -> Option<Packet> {
        let next_seq = self.next_seq?;

        let old_packets = std::mem::replace(&mut self.packets, PacketList::Nil);
        let (packet, new_packets) = old_packets.remove_seq(next_seq);
        self.packets = new_packets;

        if packet.is_some() {
            self.next_seq = Some(next_seq.wrapping_add(1));
        }

        packet
    }
}

/// Levenshtein edit distance (simplified for Aeneas)
pub fn levenshtein_simple(a: &[u8], b: &[u8]) -> u32 {
    if a.is_empty() {
        return b.len() as u32;
    }
    if b.is_empty() {
        return a.len() as u32;
    }

    // Use simple recursive definition (inefficient but clear for verification)
    let cost = if a[a.len() - 1] == b[b.len() - 1] { 0 } else { 1 };

    let delete = levenshtein_simple(&a[..a.len() - 1], b) + 1;
    let insert = levenshtein_simple(a, &b[..b.len() - 1]) + 1;
    let substitute = levenshtein_simple(&a[..a.len() - 1], &b[..b.len() - 1]) + cost;

    delete.min(insert).min(substitute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_list_len() {
        let list = PacketList::new();
        assert_eq!(list.len(), 0);

        let list = list.insert(Packet { sequence: 1, payload_len: 100 });
        assert_eq!(list.len(), 1);

        let list = list.insert(Packet { sequence: 2, payload_len: 100 });
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_seq_is_before() {
        assert!(seq_is_before(0, 1));
        assert!(seq_is_before(65535, 0)); // Wraparound
        assert!(!seq_is_before(1, 0));
        assert!(!seq_is_before(0, 0));
    }

    #[test]
    fn test_jitter_buffer_basic() {
        let mut buffer = JitterBuffer::new(10);

        assert!(buffer.insert(Packet { sequence: 0, payload_len: 100 }));
        assert!(buffer.insert(Packet { sequence: 1, payload_len: 100 }));

        let p = buffer.pop();
        assert!(p.is_some());
        assert_eq!(p.unwrap().sequence, 0);
    }
}
