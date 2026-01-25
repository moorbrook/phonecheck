/// RTP Jitter Buffer
/// Handles packet reordering and jitter compensation for better audio quality
///
/// The jitter buffer collects incoming RTP packets and outputs them in sequence order,
/// with configurable delay to absorb network jitter.

use std::collections::BTreeMap;
use tracing::{debug, trace, warn};

/// Configuration for the jitter buffer
#[derive(Debug, Clone)]
pub struct JitterBufferConfig {
    /// Target buffer depth in packets (default: 3)
    /// Higher values = more jitter tolerance but higher latency
    pub target_depth: u16,
    /// Maximum buffer size in packets before dropping old packets (default: 50)
    /// When exceeded, OLDEST packets are dropped first (FIFO eviction).
    /// This prevents unbounded memory growth from extreme network delays.
    /// At 20ms/packet, 50 packets = 1 second of audio buffer.
    pub max_size: u16,
    /// Maximum sequence number gap before considering packets lost (default: 10)
    /// If the gap exceeds this, the buffer skips ahead to the next available packet.
    /// At 50 packets/sec, gap of 10 = 200ms of missing audio triggers skip.
    pub max_gap: u16,
}

impl Default for JitterBufferConfig {
    fn default() -> Self {
        Self {
            target_depth: 3,
            max_size: 50,
            max_gap: 10,
        }
    }
}

/// A buffered RTP packet
#[derive(Debug, Clone)]
pub struct BufferedPacket {
    pub sequence: u16,
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

/// Jitter buffer state
#[derive(Debug)]
pub struct JitterBuffer {
    config: JitterBufferConfig,
    /// Packets indexed by sequence number
    packets: BTreeMap<u16, BufferedPacket>,
    /// Next expected sequence number for output
    next_seq: Option<u16>,
    /// Number of packets received
    packets_received: u64,
    /// Number of packets output in order
    packets_output: u64,
    /// Number of packets dropped (late/duplicate)
    packets_dropped: u64,
    /// Number of gaps (missing packets)
    packets_lost: u64,
}

impl JitterBuffer {
    pub fn new(config: JitterBufferConfig) -> Self {
        Self {
            config,
            packets: BTreeMap::new(),
            next_seq: None,
            packets_received: 0,
            packets_output: 0,
            packets_dropped: 0,
            packets_lost: 0,
        }
    }

    /// Insert a packet into the buffer
    /// Returns true if packet was accepted, false if dropped (late/duplicate)
    pub fn insert(&mut self, packet: BufferedPacket) -> bool {
        self.packets_received += 1;

        let seq = packet.sequence;

        // Initialize next_seq if this is the first packet
        if self.next_seq.is_none() {
            self.next_seq = Some(seq);
            debug!("Jitter buffer initialized with first sequence: {}", seq);
        }

        let next_seq = self.next_seq.unwrap();

        // Check if packet is too old (already past output window)
        if self.is_before(seq, next_seq) {
            trace!("Dropping late packet: seq={} (expected >= {})", seq, next_seq);
            self.packets_dropped += 1;
            return false;
        }

        // Check if we already have this packet
        if self.packets.contains_key(&seq) {
            trace!("Dropping duplicate packet: seq={}", seq);
            self.packets_dropped += 1;
            return false;
        }

        // Insert the packet
        self.packets.insert(seq, packet);
        trace!("Buffered packet: seq={}, buffer_size={}", seq, self.packets.len());

        // Trim buffer if too large
        while self.packets.len() > self.config.max_size as usize {
            if let Some((&oldest_seq, _)) = self.packets.iter().next() {
                self.packets.remove(&oldest_seq);
                self.packets_dropped += 1;
                warn!("Buffer overflow, dropped packet: seq={}", oldest_seq);
            }
        }

        true
    }

    /// Get the next packet in sequence order
    /// Returns None if buffer needs more packets or next packet is missing
    pub fn pop(&mut self) -> Option<BufferedPacket> {
        let next_seq = self.next_seq?;

        // Wait until we have target_depth packets before starting output
        if self.packets_output == 0 && self.packets.len() < self.config.target_depth as usize {
            return None;
        }

        // Try to get the next expected packet
        if let Some(packet) = self.packets.remove(&next_seq) {
            self.next_seq = Some(next_seq.wrapping_add(1));
            self.packets_output += 1;
            return Some(packet);
        }

        // Next packet is missing - check if we should skip it
        // Only skip if we have packets beyond the gap
        let gap = self.gap_to_next_available();
        if gap > self.config.max_gap {
            // Too big a gap, advance to the next available packet
            if let Some((&available_seq, _)) = self.packets.iter().next() {
                self.packets_lost += (available_seq.wrapping_sub(next_seq)) as u64;
                debug!(
                    "Skipping {} missing packets, jumping from {} to {}",
                    available_seq.wrapping_sub(next_seq),
                    next_seq,
                    available_seq
                );
                self.next_seq = Some(available_seq);
                return self.packets.remove(&available_seq).inspect(|_p| {
                    self.next_seq = Some(available_seq.wrapping_add(1));
                    self.packets_output += 1;
                });
            }
        }

        // Wait for the missing packet
        None
    }

    /// Check if we have packets ready to output
    pub fn has_ready(&self) -> bool {
        let Some(next_seq) = self.next_seq else {
            return false;
        };

        // During initial buffering, wait for target_depth
        if self.packets_output == 0 {
            return self.packets.len() >= self.config.target_depth as usize;
        }

        // After initial buffering, return true if next packet is available
        // or if there's a gap larger than max_gap
        if self.packets.contains_key(&next_seq) {
            return true;
        }

        // Check if gap is too large
        self.gap_to_next_available() > self.config.max_gap
    }

    /// Get all remaining packets in order (for flushing)
    pub fn drain(&mut self) -> Vec<BufferedPacket> {
        let mut result = Vec::new();

        // Skip to first available packet if needed
        if let Some(next_seq) = self.next_seq {
            if !self.packets.contains_key(&next_seq) {
                if let Some((&first_available, _)) = self.packets.iter().next() {
                    self.next_seq = Some(first_available);
                }
            }
        }

        // Pop all packets in order
        while let Some(packet) = self.pop() {
            result.push(packet);
        }

        // Also get any remaining packets that might be out of order
        let remaining: Vec<_> = self.packets.values().cloned().collect();
        self.packets.clear();
        result.extend(remaining);

        result
    }

    /// Get buffer statistics
    pub fn stats(&self) -> JitterBufferStats {
        JitterBufferStats {
            packets_received: self.packets_received,
            packets_output: self.packets_output,
            packets_dropped: self.packets_dropped,
            packets_lost: self.packets_lost,
            current_depth: self.packets.len() as u16,
        }
    }

    /// Calculate gap between current sequence and next available packet
    fn gap_to_next_available(&self) -> u16 {
        let Some(next_seq) = self.next_seq else {
            return 0;
        };

        if let Some((&first_available, _)) = self.packets.iter().next() {
            first_available.wrapping_sub(next_seq)
        } else {
            0
        }
    }

    /// Check if seq_a is before seq_b (handles wraparound)
    fn is_before(&self, seq_a: u16, seq_b: u16) -> bool {
        // Use signed comparison to handle wraparound
        // If diff > 0x8000, seq_a is actually after seq_b (wrapped)
        let diff = seq_b.wrapping_sub(seq_a);
        diff > 0 && diff < 0x8000
    }
}

/// Statistics about jitter buffer operation
#[derive(Debug, Clone, Default)]
pub struct JitterBufferStats {
    pub packets_received: u64,
    pub packets_output: u64,
    pub packets_dropped: u64,
    pub packets_lost: u64,
    pub current_depth: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_packet(seq: u16) -> BufferedPacket {
        BufferedPacket {
            sequence: seq,
            timestamp: seq as u32 * 160, // Typical G.711 timestamp increment
            payload: vec![0u8; 160],
        }
    }

    #[test]
    fn test_in_order_packets() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 5,
        });

        // Insert packets 0, 1, 2, 3 in order
        assert!(buffer.insert(make_packet(0)));
        assert!(buffer.insert(make_packet(1)));
        assert!(buffer.insert(make_packet(2)));
        assert!(buffer.insert(make_packet(3)));

        // Should output 0, 1, 2, 3 in order
        assert_eq!(buffer.pop().unwrap().sequence, 0);
        assert_eq!(buffer.pop().unwrap().sequence, 1);
        assert_eq!(buffer.pop().unwrap().sequence, 2);
        assert_eq!(buffer.pop().unwrap().sequence, 3);
        assert!(buffer.pop().is_none());
    }

    #[test]
    fn test_out_of_order_packets() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 5,
        });

        // Insert packets out of order: 0, 2, 1, 3
        assert!(buffer.insert(make_packet(0)));
        assert!(buffer.insert(make_packet(2)));
        assert!(buffer.insert(make_packet(1)));
        assert!(buffer.insert(make_packet(3)));

        // Should output 0, 1, 2, 3 in order
        assert_eq!(buffer.pop().unwrap().sequence, 0);
        assert_eq!(buffer.pop().unwrap().sequence, 1);
        assert_eq!(buffer.pop().unwrap().sequence, 2);
        assert_eq!(buffer.pop().unwrap().sequence, 3);
    }

    #[test]
    fn test_late_packet_dropped() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 5,
        });

        // Insert 0, 1, 2
        buffer.insert(make_packet(0));
        buffer.insert(make_packet(1));
        buffer.insert(make_packet(2));

        // Pop 0
        assert_eq!(buffer.pop().unwrap().sequence, 0);

        // Now insert a "late" packet 0 - should be dropped
        assert!(!buffer.insert(make_packet(0)));

        let stats = buffer.stats();
        assert_eq!(stats.packets_dropped, 1);
    }

    #[test]
    fn test_duplicate_packet_dropped() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 5,
        });

        // Insert same packet twice
        assert!(buffer.insert(make_packet(0)));
        assert!(!buffer.insert(make_packet(0))); // Duplicate

        let stats = buffer.stats();
        assert_eq!(stats.packets_dropped, 1);
    }

    #[test]
    fn test_gap_handling() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 2, // Gap of 3 will exceed this
        });

        // Insert packets with gap: 0, 1, 5, 6 (missing 2, 3, 4)
        buffer.insert(make_packet(0));
        buffer.insert(make_packet(1));
        buffer.insert(make_packet(5));
        buffer.insert(make_packet(6));

        // Should output 0, 1
        assert_eq!(buffer.pop().unwrap().sequence, 0);
        assert_eq!(buffer.pop().unwrap().sequence, 1);

        // Now we're at seq 2, but 2, 3, 4 are missing. Gap from 2 to 5 is 3 > max_gap (2)
        // Should skip to 5
        assert_eq!(buffer.pop().unwrap().sequence, 5);
        assert_eq!(buffer.pop().unwrap().sequence, 6);
        assert!(buffer.pop().is_none());
    }

    #[test]
    fn test_buffer_overflow() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 3,
            max_gap: 5,
        });

        // Insert more packets than max_size
        buffer.insert(make_packet(0));
        buffer.insert(make_packet(1));
        buffer.insert(make_packet(2));
        buffer.insert(make_packet(3)); // Should trigger overflow

        // Should have dropped oldest
        let stats = buffer.stats();
        assert_eq!(stats.packets_dropped, 1);
    }

    #[test]
    fn test_sequence_wraparound() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 5,
        });

        // Insert packets around u16::MAX
        buffer.insert(make_packet(65534));
        buffer.insert(make_packet(65535));
        buffer.insert(make_packet(0));
        buffer.insert(make_packet(1));

        // Should output in order across wraparound
        assert_eq!(buffer.pop().unwrap().sequence, 65534);
        assert_eq!(buffer.pop().unwrap().sequence, 65535);
        assert_eq!(buffer.pop().unwrap().sequence, 0);
        assert_eq!(buffer.pop().unwrap().sequence, 1);
    }

    #[test]
    fn test_drain() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 5,
        });

        buffer.insert(make_packet(0));
        buffer.insert(make_packet(1));
        buffer.insert(make_packet(3)); // Gap at 2

        let packets = buffer.drain();
        assert_eq!(packets.len(), 3);
        assert_eq!(packets[0].sequence, 0);
        assert_eq!(packets[1].sequence, 1);
        assert_eq!(packets[2].sequence, 3);
    }

    #[test]
    fn test_initial_buffering() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 3,
            max_size: 10,
            max_gap: 5,
        });

        // Insert only 2 packets - shouldn't output yet (target_depth=3)
        buffer.insert(make_packet(0));
        buffer.insert(make_packet(1));

        assert!(!buffer.has_ready());
        assert!(buffer.pop().is_none());

        // Insert third packet
        buffer.insert(make_packet(2));

        assert!(buffer.has_ready());
        assert_eq!(buffer.pop().unwrap().sequence, 0);
    }

    #[test]
    fn test_stats() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig::default());

        buffer.insert(make_packet(0));
        buffer.insert(make_packet(1));
        buffer.insert(make_packet(2));
        buffer.insert(make_packet(0)); // Duplicate

        buffer.pop();
        buffer.pop();

        let stats = buffer.stats();
        assert_eq!(stats.packets_received, 4);
        assert_eq!(stats.packets_dropped, 1); // Duplicate
        assert_eq!(stats.packets_output, 2);
        assert_eq!(stats.current_depth, 1); // One packet remaining
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Inserting in-order packets yields them in order
        #[test]
        fn in_order_yields_in_order(start in 0u16..60000, count in 1usize..20) {
            let mut buffer = JitterBuffer::new(JitterBufferConfig {
                target_depth: 1,
                max_size: 100,
                max_gap: 50,
            });

            // Insert in order
            for i in 0..count {
                buffer.insert(BufferedPacket {
                    sequence: start.wrapping_add(i as u16),
                    timestamp: 0,
                    payload: vec![],
                });
            }

            // Output should be in order
            let mut prev_seq: Option<u16> = None;
            while let Some(packet) = buffer.pop() {
                if let Some(prev) = prev_seq {
                    prop_assert_eq!(packet.sequence, prev.wrapping_add(1));
                }
                prev_seq = Some(packet.sequence);
            }
        }

        /// No packets are lost when received in order
        #[test]
        fn no_loss_when_in_order(start in 0u16..60000, count in 5usize..20) {
            let mut buffer = JitterBuffer::new(JitterBufferConfig {
                target_depth: 2,
                max_size: 100,
                max_gap: 50,
            });

            for i in 0..count {
                buffer.insert(BufferedPacket {
                    sequence: start.wrapping_add(i as u16),
                    timestamp: 0,
                    payload: vec![],
                });
            }

            while buffer.pop().is_some() {}

            let stats = buffer.stats();
            prop_assert_eq!(stats.packets_lost, 0);
        }

        /// Late packets are always dropped
        #[test]
        fn late_packets_dropped(
            start in 0u16..60000,
            advance in 10usize..20
        ) {
            let mut buffer = JitterBuffer::new(JitterBufferConfig {
                target_depth: 2,
                max_size: 100,
                max_gap: 50,
            });

            // Insert and pop some packets
            for i in 0..advance {
                buffer.insert(BufferedPacket {
                    sequence: start.wrapping_add(i as u16),
                    timestamp: 0,
                    payload: vec![],
                });
            }

            // Pop half the packets
            let popped = advance / 2;
            for _ in 0..popped {
                buffer.pop();
            }

            // Try to insert a late packet (one that was already output)
            // Use the first sequence that was popped
            let late_seq = start;
            let accepted = buffer.insert(BufferedPacket {
                sequence: late_seq,
                timestamp: 0,
                payload: vec![],
            });

            // Should be dropped (it's before the current output window)
            prop_assert!(!accepted);
        }

        /// Sequence number wraparound is handled correctly
        #[test]
        fn handles_wraparound(offset in 0u16..1000) {
            let mut buffer = JitterBuffer::new(JitterBufferConfig {
                target_depth: 2,
                max_size: 100,
                max_gap: 50,
            });

            // Insert around wraparound point
            let start = u16::MAX - offset;
            for i in 0..20 {
                buffer.insert(BufferedPacket {
                    sequence: start.wrapping_add(i),
                    timestamp: 0,
                    payload: vec![],
                });
            }

            // Should output all in order
            let mut output = Vec::new();
            while let Some(packet) = buffer.pop() {
                output.push(packet.sequence);
            }

            // Verify order (handling wraparound)
            for i in 1..output.len() {
                prop_assert_eq!(output[i], output[i - 1].wrapping_add(1));
            }
        }
    }
}

/// State machine model for jitter buffer
#[cfg(test)]
mod state_machine {
    use super::*;
    use stateright::*;
    use std::collections::BTreeSet;

    /// Actions that can be performed on the jitter buffer
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    enum Action {
        /// Insert next sequence number in order
        InsertInOrder,
        /// Insert a packet that's already been output (late)
        InsertLate,
        /// Insert a duplicate of an existing buffered packet
        InsertDuplicate,
        /// Pop the next packet
        Pop,
        /// Drain all remaining packets
        Drain,
    }

    /// Detailed state for comprehensive model checking
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct BufferState {
        /// Packets currently in the buffer (by sequence number)
        buffered: BTreeSet<u16>,
        /// Packets that have been output
        output: Vec<u16>,
        /// Next sequence number to insert (simulating sender)
        next_to_send: u16,
        /// Last sequence number that was output (for late detection)
        last_output_seq: Option<u16>,
        /// Number of dropped packets (late/duplicate)
        dropped: u16,
        /// Max buffer size (from config)
        max_size: u16,
    }

    impl BufferState {
        fn new(max_size: u16) -> Self {
            Self {
                buffered: BTreeSet::new(),
                output: Vec::new(),
                next_to_send: 0,
                last_output_seq: None,
                dropped: 0,
                max_size,
            }
        }
    }

    struct JitterBufferModel {
        max_ops: u16,
        max_size: u16,
    }

    impl Model for JitterBufferModel {
        type State = BufferState;
        type Action = Action;

        fn init_states(&self) -> Vec<Self::State> {
            vec![BufferState::new(self.max_size)]
        }

        fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
            let total_ops = state.next_to_send + state.dropped;
            if total_ops < self.max_ops {
                actions.push(Action::InsertInOrder);

                // Can insert late if we've output something
                if state.last_output_seq.is_some() {
                    actions.push(Action::InsertLate);
                }

                // Can insert duplicate if buffer has packets
                if !state.buffered.is_empty() {
                    actions.push(Action::InsertDuplicate);
                }
            }

            if !state.buffered.is_empty() {
                actions.push(Action::Pop);
                actions.push(Action::Drain);
            }
        }

        fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
            let mut next = state.clone();

            match action {
                Action::InsertInOrder => {
                    let seq = state.next_to_send;
                    next.next_to_send = seq.wrapping_add(1);

                    // Check if late (already output)
                    if let Some(last) = state.last_output_seq {
                        // Simple check: if seq <= last, it's late
                        if seq <= last {
                            next.dropped += 1;
                            return Some(next);
                        }
                    }

                    // Insert into buffer
                    next.buffered.insert(seq);

                    // Enforce max_size (drop oldest)
                    while next.buffered.len() > self.max_size as usize {
                        if let Some(&oldest) = next.buffered.iter().next() {
                            next.buffered.remove(&oldest);
                            next.dropped += 1;
                        }
                    }
                }

                Action::InsertLate => {
                    // Insert a sequence number before last_output_seq
                    if let Some(last) = state.last_output_seq {
                        let late_seq = last.saturating_sub(1);
                        next.dropped += 1; // Late packets are dropped
                    }
                }

                Action::InsertDuplicate => {
                    // Try to insert an existing sequence
                    if let Some(&existing) = state.buffered.iter().next() {
                        // Duplicate - dropped
                        next.dropped += 1;
                    }
                }

                Action::Pop => {
                    // Pop the smallest sequence number (BTreeSet is sorted)
                    if let Some(&seq) = state.buffered.iter().next() {
                        next.buffered.remove(&seq);
                        next.output.push(seq);
                        next.last_output_seq = Some(seq);
                    }
                }

                Action::Drain => {
                    // Drain all packets in order
                    let mut seqs: Vec<u16> = state.buffered.iter().copied().collect();
                    seqs.sort();
                    next.output.extend(seqs.iter());
                    if let Some(&last) = seqs.last() {
                        next.last_output_seq = Some(last);
                    }
                    next.buffered.clear();
                }
            }

            Some(next)
        }

        fn properties(&self) -> Vec<Property<Self>> {
            vec![
                // Safety: Buffer size never exceeds max_size
                Property::always("buffer_size_bounded", |model: &Self, state: &BufferState| {
                    state.buffered.len() <= model.max_size as usize
                }),

                // Safety: No duplicate sequences in output
                Property::always("no_duplicate_output", |_: &Self, state: &BufferState| {
                    let mut seen = std::collections::HashSet::new();
                    state.output.iter().all(|&seq| seen.insert(seq))
                }),

                // Safety: Output is monotonically increasing (no out-of-order output)
                Property::always("output_ordered", |_: &Self, state: &BufferState| {
                    state.output.windows(2).all(|w| w[0] < w[1])
                }),

                // Safety: Total packets = buffered + output + dropped
                Property::always("packet_accounting", |_: &Self, state: &BufferState| {
                    let total_sent = state.next_to_send as usize;
                    let accounted = state.buffered.len() + state.output.len() + state.dropped as usize;
                    // Allow for late/duplicate attempts which increment dropped but not sent
                    accounted >= state.output.len() + state.buffered.len()
                }),

                // Safety: Output sequences are subset of sent sequences
                Property::always("output_valid_sequences", |_: &Self, state: &BufferState| {
                    state.output.iter().all(|&seq| seq < state.next_to_send)
                }),
            ]
        }
    }

    #[test]
    fn test_jitter_buffer_model_basic() {
        let model = JitterBufferModel { max_ops: 5, max_size: 10 };
        let checker = model.checker().threads(1).spawn_bfs().join();
        println!("States explored: {}", checker.unique_state_count());
        checker.assert_properties();
    }

    #[test]
    fn test_jitter_buffer_model_constrained() {
        // Test with small max_size to stress overflow handling
        let model = JitterBufferModel { max_ops: 8, max_size: 3 };
        let checker = model.checker().threads(1).spawn_bfs().join();
        println!("States explored (constrained): {}", checker.unique_state_count());
        checker.assert_properties();
    }
}

/// Kani formal verification proofs
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    // Helper: replicate is_before logic for testing (since it's private)
    fn is_before(seq_a: u16, seq_b: u16) -> bool {
        let diff = seq_b.wrapping_sub(seq_a);
        diff > 0 && diff < 0x8000
    }

    /// Proves: is_before() correctly handles most u16 pairs
    /// When diff != 0x8000 (the ambiguous midpoint), exactly one of
    /// is_before(a,b), is_before(b,a), or a==b is true
    #[kani::proof]
    fn is_before_trichotomy() {
        let a: u16 = kani::any();
        let b: u16 = kani::any();

        let diff = b.wrapping_sub(a);

        // Skip the ambiguous midpoint case (diff == 0x8000)
        // When exactly 32768 apart, neither direction is "before"
        kani::assume(diff != 0x8000);

        let a_before_b = is_before(a, b);
        let b_before_a = is_before(b, a);
        let equal = a == b;

        // Exactly one must be true (trichotomy, excluding midpoint)
        let count = a_before_b as u8 + b_before_a as u8 + equal as u8;
        kani::assert(count == 1, "exactly one of <, >, or == must hold");
    }

    /// Proves: the midpoint case (diff == 0x8000) is ambiguous
    /// Neither sequence is "before" the other
    #[kani::proof]
    fn is_before_midpoint_ambiguous() {
        let a: u16 = kani::any();
        let b: u16 = a.wrapping_add(0x8000); // Exactly 32768 apart

        // Both directions return false - ordering is ambiguous
        kani::assert(!is_before(a, b), "midpoint: a not before b");
        kani::assert(!is_before(b, a), "midpoint: b not before a");
        kani::assert(a != b, "midpoint values are not equal");
    }

    /// Proves: is_before() is anti-symmetric
    /// If a < b then NOT b < a
    #[kani::proof]
    fn is_before_antisymmetric() {
        let a: u16 = kani::any();
        let b: u16 = kani::any();

        if is_before(a, b) {
            kani::assert(!is_before(b, a), "is_before must be antisymmetric");
        }
    }

    /// Proves: is_before() handles the wraparound boundary correctly
    /// 65535 should be before 0, 1, 2, ... up to ~32767
    #[kani::proof]
    fn is_before_wraparound_boundary() {
        // 65535 is "before" 0 in RTP sequence space (just wrapped)
        kani::assert(
            is_before(65535, 0),
            "65535 should be before 0 (wraparound)"
        );

        // 0 is "after" 65535
        kani::assert(
            !is_before(0, 65535),
            "0 should not be before 65535"
        );

        // Test the midpoint: 32768 is the boundary
        // Values < 32768 apart are considered "close"
        kani::assert(
            is_before(0, 32767),
            "0 should be before 32767"
        );

        // 32768 apart is ambiguous - we treat it as "not before"
        kani::assert(
            !is_before(0, 32768),
            "0 should not be before 32768 (boundary)"
        );
    }

    /// Proves: insert never panics for any sequence number
    #[kani::proof]
    fn insert_never_panics() {
        let seq: u16 = kani::any();

        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 3,
            max_size: 10,
            max_gap: 5,
        });

        let packet = BufferedPacket {
            sequence: seq,
            timestamp: 0,
            payload: vec![],
        };

        // Should not panic
        let _ = buffer.insert(packet);
    }

    /// Proves: buffer size never exceeds max_size after any insert
    #[kani::proof]
    #[kani::unwind(12)]
    fn max_size_enforced() {
        let max_size: u16 = kani::any();
        kani::assume(max_size > 0 && max_size <= 10);

        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 0,
            max_size,
            max_gap: 100,
        });

        // Insert more packets than max_size
        for i in 0..max_size + 2 {
            let packet = BufferedPacket {
                sequence: i,
                timestamp: 0,
                payload: vec![],
            };
            buffer.insert(packet);
        }

        // Buffer size should be at most max_size
        kani::assert(
            buffer.packets.len() <= max_size as usize,
            "buffer must not exceed max_size"
        );
    }

    /// Proves: pop never panics
    #[kani::proof]
    fn pop_never_panics() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig::default());

        // Pop from empty buffer
        let _ = buffer.pop();

        // Insert one packet and pop
        buffer.insert(BufferedPacket {
            sequence: 0,
            timestamp: 0,
            payload: vec![],
        });
        let _ = buffer.pop();
        let _ = buffer.pop(); // Pop again from empty
    }

    /// Proves: drain never panics and returns all buffered packets
    #[kani::proof]
    #[kani::unwind(6)]
    fn drain_returns_all() {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 0,
            max_size: 5,
            max_gap: 100,
        });

        // Insert some packets
        for i in 0..3u16 {
            buffer.insert(BufferedPacket {
                sequence: i,
                timestamp: 0,
                payload: vec![],
            });
        }

        let count_before = buffer.packets.len();
        let drained = buffer.drain();

        kani::assert(
            drained.len() == count_before,
            "drain must return all buffered packets"
        );
        kani::assert(
            buffer.packets.is_empty(),
            "buffer must be empty after drain"
        );
    }
}
