//! The far end of a transport channel on the car's CAN bus.
//!
//! The unit talks to the instrument cluster (identifiers 0x6c0 out,
//! 0x6c1 back) and to the telephone module (0x6c4 out, 0x6c5 back) over
//! Volkswagen's transport protocol 2.0, on fixed identifiers.  This is
//! the other side of one such channel, as far as the transport goes:
//! it agrees to the channel, acknowledges and reassembles what the unit
//! sends, and can send blocks of its own.  What the blocks mean is the
//! business of whoever uses it.
//!
//! Frames, by their first byte:
//!
//! ```text
//! a0 bs t1 ff t3 ff   channel parameters, a request (answered with a1
//!                     and the same parameters)
//! a1 ...              the answer to it
//! a3                  is the channel still there? (answered with a1 ...)
//! a8                  disconnect (answered with a8)
//! 0n data..           data, more follows, acknowledge now
//! 1n data..           data, last of the block, acknowledge
//! 2n data..           data, more follows, no acknowledgement wanted
//! 3n data..           data, last of the block, no acknowledgement
//! bn                  acknowledged; n is the sequence number expected next
//! 9n                  not ready
//! ```
//!
//! n counts frames, modulo 16, for each direction.  A block is the data
//! of consecutive frames up to one marked as the last; there is no
//! length in front of it on these channels (the unit's first block to
//! the cluster is the single frame `10 15 01 01 02 00 00`).  The unit
//! asks for the channel with `a0 0f 8a ff 4a ff` every 100 ms or so and
//! sends `a8` when nobody answers; with a channel open it sends `a3`
//! about once a second.

use super::hcan::Frame;

pub struct Peer {
    /// Identifier the unit sends on, and the one it listens to.
    pub from_unit: u16,
    pub to_unit: u16,
    pub open: bool,
    /// Sequence number expected from the unit, and ours for the next
    /// frame we send.
    rx_seq: u8,
    tx_seq: u8,
    /// A block being received.
    partial: Vec<u8>,
    /// Blocks received whole, oldest first.
    pub received: Vec<Vec<u8>>,
    /// Blocks the unit has acknowledged, counted.
    pub acknowledged: u32,
    pub trace: bool,
}

impl Peer {
    pub fn new(from_unit: u16, to_unit: u16) -> Peer {
        Peer { from_unit, to_unit, open: false, rx_seq: 0, tx_seq: 0, partial: Vec::new(), received: Vec::new(), acknowledged: 0, trace: false }
    }

    fn reply(&self, bytes: &[u8]) -> Frame {
        Frame::new(self.to_unit, bytes)
    }

    /// A frame the unit sent.  Returns what goes back.
    pub fn from_unit(&mut self, f: &Frame) -> Vec<Frame> {
        if f.id != self.from_unit || f.len == 0 {
            return Vec::new();
        }
        let d = &f.data[..f.len as usize];
        match d[0] {
            0xa0 => {
                // A new channel: both counters start over.
                self.open = true;
                self.rx_seq = 0;
                self.tx_seq = 0;
                self.partial.clear();
                let mut answer = d.to_vec();
                answer[0] = 0xa1;
                vec![self.reply(&answer)]
            }
            0xa3 => vec![self.reply(&[0xa1, 0x0f, 0x8a, 0xff, 0x4a, 0xff])],
            0xa8 => {
                let was = std::mem::replace(&mut self.open, false);
                if was { vec![self.reply(&[0xa8])] } else { Vec::new() }
            }
            0xb0..=0xbf => {
                self.acknowledged += 1;
                Vec::new()
            }
            op @ 0x00..=0x3f if self.open => {
                let seq = op & 0x0f;
                if seq != self.rx_seq {
                    // Out of step: say which one is expected.
                    return vec![self.reply(&[0xb0 | self.rx_seq])];
                }
                self.rx_seq = (self.rx_seq + 1) & 0x0f;
                self.partial.extend_from_slice(&d[1..]);
                if op & 0x10 != 0 {
                    let block = std::mem::take(&mut self.partial);
                    if self.trace {
                        eprintln!("[tp20] {:03x} block from the unit: {}", self.from_unit, hex(&block));
                    }
                    self.received.push(block);
                }
                // 0n and 1n want an acknowledgement, 2n and 3n do not.
                if op & 0x20 == 0 { vec![self.reply(&[0xb0 | self.rx_seq])] } else { Vec::new() }
            }
            _ => Vec::new(),
        }
    }

    /// A block for the unit, as the frames to send.
    pub fn send(&mut self, block: &[u8]) -> Vec<Frame> {
        let chunks: Vec<&[u8]> = block.chunks(7).collect();
        let mut out = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            // The last frame asks for the acknowledgement.
            let op = if i + 1 == chunks.len() { 0x10 } else { 0x20 };
            let mut d = vec![op | self.tx_seq];
            d.extend_from_slice(chunk);
            self.tx_seq = (self.tx_seq + 1) & 0x0f;
            out.push(self.reply(&d));
        }
        out
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_and_blocks() {
        let mut p = Peer::new(0x6c4, 0x6c5);
        assert!(p.from_unit(&Frame::new(0x6c0, &[0xa0, 0x0f, 0x8a, 0xff, 0x4a, 0xff])).is_empty(), "another channel");
        assert!(p.from_unit(&Frame::new(0x6c4, &[0x10, 0x55])).is_empty(), "no channel yet");
        assert_eq!(p.from_unit(&Frame::new(0x6c4, &[0xa0, 0x0f, 0x8a, 0xff, 0x4a, 0xff])), [Frame::new(0x6c5, &[0xa1, 0x0f, 0x8a, 0xff, 0x4a, 0xff])]);
        assert!(p.open);
        // A block of eleven bytes in two frames, the first unacknowledged.
        assert!(p.from_unit(&Frame::new(0x6c4, &[0x20, 1, 2, 3, 4, 5, 6, 7])).is_empty());
        assert_eq!(p.from_unit(&Frame::new(0x6c4, &[0x11, 8, 9, 10, 11])), [Frame::new(0x6c5, &[0xb2])]);
        assert_eq!(p.received, [vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]]);
        // A repeated frame is answered with the number still expected.
        assert_eq!(p.from_unit(&Frame::new(0x6c4, &[0x11, 8, 9, 10, 11])), [Frame::new(0x6c5, &[0xb2])]);
        assert_eq!(p.received.len(), 1);
        // Our own block of ten bytes: two frames, the last asks.
        let out = p.send(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
        assert_eq!(out, [Frame::new(0x6c5, &[0x20, 10, 11, 12, 13, 14, 15, 16]), Frame::new(0x6c5, &[0x11, 17, 18, 19])]);
        p.from_unit(&Frame::new(0x6c4, &[0xb2]));
        assert_eq!(p.acknowledged, 1);
        assert_eq!(p.from_unit(&Frame::new(0x6c4, &[0xa3])), [Frame::new(0x6c5, &[0xa1, 0x0f, 0x8a, 0xff, 0x4a, 0xff])]);
        assert_eq!(p.from_unit(&Frame::new(0x6c4, &[0xa8])), [Frame::new(0x6c5, &[0xa8])]);
        assert!(!p.open);
    }
}
