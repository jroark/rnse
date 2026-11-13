//! Stand-in for the faceplate controller.
//!
//! The keys, the two rotary encoders and the key illumination hang off a
//! small microcontroller in the faceplate.  It talks to the main
//! processor over serial channel 3 (0x07a3_0000).  The protocol below was
//! worked out from the `FPDRV` task of the operating system.
//!
//! # Frames
//!
//! ```text
//!   cmd  L  data[L-2]  sum
//! ```
//!
//! `L` is the index of the last byte (so a frame is `L + 1` bytes long)
//! and `sum` is the 8-bit sum of everything before it.  Every frame is
//! answered with a single `0x15` (accepted), with `0x16` followed
//! immediately by a frame of the other side's own (accepted, and here is
//! mine), or with a `0x18` frame carrying an error code.
//!
//! Faceplate to unit:
//!
//! | cmd  | L | data                                                     |
//! |------|---|----------------------------------------------------------|
//! | 0x0F | 2 | none: the controller has (re)started                     |
//! | 0x0C | 8 | six bytes: the state of every key, one bit each          |
//! | 0x0D | 6 | four bytes: detents turned; byte 0 = volume knob, byte 2 |
//! |      |   | = select knob, bits 6..0 = count.  Bit 7: for the select |
//! |      |   | knob clockwise (down a list), for the volume knob the    |
//! |      |   | direction that lowers the volume                         |
//! | 0x10 | 3 | one byte: controller software version                    |
//! | 0x04, 0x08, 0x0B, 0x11, 0x12 | | accepted, meaning not worked out      |
//!
//! Unit to faceplate: commands 0x00..0x03, 0x06, 0x07, 0x09, 0x0A and
//! 0x0E.  This model accepts them all and keeps the last of each for
//! inspection; it does not interpret them.  (After the start-up frame
//! the unit sends 0x02, 0x03, 0x0A and 0x09 once, then 0x06 several
//! times while it fades the lighting in, and 0x07.)
//!
//! # Reset
//!
//! The unit holds the controller's reset line: bit 0 of the latch at
//! 0x1600_0004 on the 192, bit 1 of the port at 0xff90_9004 on the 193,
//! low = reset.  Both stock systems start with the line low, switch
//! their serial port off, raise the line and then turn the receiver on;
//! the controller's first frame is 0x0F.  When the controller leaves a
//! command frame unanswered for two seconds the unit pulses the line low
//! ("fpdrv hard reset") and expects the whole start-up again.  How long
//! the real controller takes to start is not known; this one takes
//! [`START_MS`].
//!
//! # Keys
//!
//! Key state bits, numbered from bit 0 of the first byte (the firmware's
//! table at 0x8006b934 in the 0815 OS turns them into its event codes):
//!
//! | bit | key            | bit | key                                  |
//! |-----|----------------|-----|--------------------------------------|
//! | 7   | RADIO          | 15  | CAR                                  |
//! | 6   | MEDIA (CD/TV)  | 14  | SETUP                                |
//! | 5   | NAME           | 13  | open/close the display               |
//! | 3   | TEL            | 11  | RETURN                               |
//! | 2   | NAV            | 10  | soft key, top right                  |
//! | 1   | INFO           | 9   | soft key, top left                   |
//! | 23  | soft key, bottom left  | 21 | previous                      |
//! | 22  | soft key, bottom right | 19 | next                          |
//! | 31  | CD eject (behind the display) | 44 | select knob pressed    |
//!
//! Bits 32..39 belong to eight more keys (event codes 0x12..0x19) that
//! this faceplate does not seem to have.  The power button is not on
//! this link.

use std::collections::VecDeque;

pub const ACK: u8 = 0x15;
pub const ACK_WITH_FRAME: u8 = 0x16;
pub const ERROR: u8 = 0x18;

pub const CMD_KEYS: u8 = 0x0c;
pub const CMD_ROTARY: u8 = 0x0d;
pub const CMD_STARTED: u8 = 0x0f;
pub const CMD_VERSION: u8 = 0x10;

/// Key names used by the front ends, with their key state bit.
pub const KEYS: [(&str, usize); 18] = [
    ("RADIO", 7),
    ("MEDIA", 6),
    ("NAME", 5),
    ("TEL", 3),
    ("NAV", 2),
    ("INFO", 1),
    ("CAR", 15),
    ("SETUP", 14),
    ("EJECT", 13),
    ("RETURN", 11),
    ("SOFT_TR", 10),
    ("SOFT_TL", 9),
    ("SOFT_BL", 23),
    ("SOFT_BR", 22),
    ("PREV", 21),
    ("NEXT", 19),
    ("CD_EJECT", 31),
    ("SELECT", 44),
];

/// Knob numbers for [`Panel::knob`].
pub const KNOB_VOLUME: usize = 0;
pub const KNOB_SELECT: usize = 1;

pub fn key_bit(name: &str) -> Option<usize> {
    KEYS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Build a frame around `data`.
pub fn frame(cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(data.len() + 3);
    f.push(cmd);
    f.push(data.len() as u8 + 2);
    f.extend_from_slice(data);
    let sum = f.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    f.push(sum);
    f
}

/// Something the unit said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromUnit {
    Ack,
    /// A frame, without its length byte and checksum.
    Frame { cmd: u8, data: Vec<u8> },
    /// A frame whose checksum was wrong.
    Bad(Vec<u8>),
}

/// Splits the unit's byte stream into acknowledgements and frames.
#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
}

impl Parser {
    pub fn push(&mut self, byte: u8) -> Option<FromUnit> {
        if self.buf.is_empty() && (byte == ACK || byte == ACK_WITH_FRAME) {
            // After 0x16 a frame follows; the acknowledgement itself is
            // complete either way.
            return Some(FromUnit::Ack);
        }
        self.buf.push(byte);
        if self.buf.len() < 2 {
            return None;
        }
        let last = (self.buf[1] as usize).max(2);
        if self.buf.len() <= last {
            return None;
        }
        let f = std::mem::take(&mut self.buf);
        let sum = f[..last].iter().fold(0u8, |a, b| a.wrapping_add(*b));
        Some(if sum == f[last] { FromUnit::Frame { cmd: f[0], data: f[2..last].to_vec() } } else { FromUnit::Bad(f) })
    }

    /// Drop a partly received frame (the line went quiet).
    pub fn reset(&mut self) {
        self.buf.clear();
    }
}

/// How long to wait for the unit to acknowledge a frame.
const ACK_TIMEOUT_MS: u64 = 300;
const RETRIES: u32 = 3;
/// From the release of the reset line to the start-up frame (assumed).
pub const START_MS: u64 = 20;

pub struct Panel {
    parser: Parser,
    /// Frames waiting to go out, oldest first.
    queue: VecDeque<Vec<u8>>,
    /// Frame sent and not yet acknowledged: (frame, time sent, attempts).
    pending: Option<(Vec<u8>, u64, u32)>,
    /// Current key state as reported in a key frame.
    pub keys: [u8; 6],
    /// Time of the last byte from the unit, to drop stale partial frames.
    last_rx_ms: u64,
    /// The unit has acknowledged the start-up frame.
    pub online: bool,
    /// The reset line is low: the controller does nothing.
    pub in_reset: bool,
    /// Nothing is sent before this time (start-up after a reset).
    quiet_until: u64,
    /// Times the unit has reset the controller after its first start.
    pub resets: u32,
    /// Last data of each command the unit has sent.
    pub from_unit: [Option<Vec<u8>>; 32],
    pub trace: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Panel {
        let mut p = Panel { parser: Parser::default(), queue: VecDeque::new(), pending: None, keys: [0; 6], last_rx_ms: 0, online: false, in_reset: false, quiet_until: 0, resets: 0, from_unit: Default::default(), trace: false };
        p.queue.push_back(frame(CMD_STARTED, &[]));
        p
    }

    /// The state of the reset line, `low` = held in reset.  The
    /// controller forgets everything while it is low and starts afresh,
    /// with its start-up frame, when it rises.
    pub fn set_reset(&mut self, low: bool, now_ms: u64) {
        if low == self.in_reset {
            return;
        }
        self.in_reset = low;
        if low {
            if self.online {
                self.resets += 1;
            }
            self.parser.reset();
            self.queue.clear();
            self.pending = None;
            self.online = false;
        } else {
            self.queue.clear();
            self.queue.push_back(frame(CMD_STARTED, &[]));
            // Keys held down through the restart are reported again.
            if self.keys != [0; 6] {
                self.queue.push_back(frame(CMD_KEYS, &self.keys));
            }
            self.quiet_until = now_ms + START_MS;
        }
    }

    /// A key went down or up.  `bit` counts from bit 0 of byte 0.
    pub fn key(&mut self, bit: usize, down: bool) {
        if bit >= 48 {
            return;
        }
        let (byte, mask) = (bit / 8, 1u8 << (bit % 8));
        let old = self.keys[byte];
        if down {
            self.keys[byte] |= mask;
        } else {
            self.keys[byte] &= !mask;
        }
        if self.keys[byte] != old && !self.in_reset {
            self.queue.push_back(frame(CMD_KEYS, &self.keys));
        }
    }

    /// A knob was turned: `knob` is [`KNOB_VOLUME`] or [`KNOB_SELECT`],
    /// `delta` in detents, positive = clockwise: down a list, or louder.
    /// (The frame's direction bit means clockwise for the select knob.
    /// For the volume knob the 192's software takes it as "quieter",
    /// subtracting the count from its volume; that this is the
    /// anticlockwise direction is an assumption about the knob, not
    /// something read from the software.)
    pub fn knob(&mut self, knob: usize, delta: i32) {
        if delta == 0 || self.in_reset {
            return;
        }
        let flagged = if knob == 0 { delta < 0 } else { delta > 0 };
        let v = (delta.unsigned_abs().min(0x7f) as u8) | if flagged { 0x80 } else { 0 };
        let mut d = [0u8; 4];
        d[if knob == 0 { 0 } else { 2 }] = v;
        self.queue.push_back(frame(CMD_ROTARY, &d));
    }

    /// Bytes arrived from the unit.  Returns what to send back at once.
    pub fn receive(&mut self, bytes: &[u8], now_ms: u64) -> Vec<u8> {
        let mut out = Vec::new();
        if self.in_reset {
            return out;
        }
        if now_ms.saturating_sub(self.last_rx_ms) > 100 {
            self.parser.reset();
        }
        self.last_rx_ms = now_ms;
        for &b in bytes {
            match self.parser.push(b) {
                None => {}
                Some(FromUnit::Ack) => {
                    if let Some((f, _, _)) = self.pending.take() {
                        if f[0] == CMD_STARTED {
                            self.online = true;
                        }
                    }
                }
                Some(FromUnit::Frame { cmd: ERROR, data }) => {
                    if self.trace {
                        eprintln!("[panel] the unit rejected a frame, code {:02x?}", data);
                    }
                    // Give up on the frame rather than repeat a mistake.
                    self.pending = None;
                }
                Some(FromUnit::Frame { cmd, data }) => {
                    if self.trace {
                        eprintln!("[panel] {:>7} ms  unit command {:02x}: {}", now_ms, cmd, hex(&data));
                    }
                    if let Some(slot) = self.from_unit.get_mut(cmd as usize) {
                        *slot = Some(data);
                    }
                    out.push(ACK);
                }
                Some(FromUnit::Bad(f)) => {
                    if self.trace {
                        eprintln!("[panel] {:>7} ms  bad frame from the unit: {}", now_ms, hex(&f));
                    }
                    out.extend(frame(ERROR, &[0x01]));
                }
            }
        }
        out
    }

    /// Called regularly: returns the next frame to transmit, if it is
    /// time for one.
    pub fn poll(&mut self, now_ms: u64) -> Option<Vec<u8>> {
        if self.in_reset || now_ms < self.quiet_until {
            return None;
        }
        if let Some((f, sent, tries)) = &mut self.pending {
            if now_ms.saturating_sub(*sent) < ACK_TIMEOUT_MS {
                return None;
            }
            // The start-up frame is repeated until the unit listens.
            if *tries < RETRIES || f[0] == CMD_STARTED {
                *sent = now_ms;
                *tries += 1;
                return Some(f.clone());
            }
            self.pending = None;
        }
        let f = self.queue.pop_front()?;
        self.pending = Some((f.clone(), now_ms, 1));
        Some(f)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        assert_eq!(frame(CMD_STARTED, &[]), [0x0f, 0x02, 0x11]);
        // The unit's own "error" frame as built by its driver.
        assert_eq!(frame(ERROR, &[0x80]), [0x18, 0x03, 0x80, 0x9b]);
        assert_eq!(frame(CMD_KEYS, &[1, 0, 0, 0, 0, 0]).len(), 9);
    }

    #[test]
    fn parser_splits_acks_and_frames() {
        let mut p = Parser::default();
        let mut got = Vec::new();
        for b in [0x15, 0x16, 0x00, 0x03, 0x2e, 0x31, 0x07, 0x03, 0x01, 0xff] {
            got.extend(p.push(b));
        }
        assert_eq!(got, [FromUnit::Ack, FromUnit::Ack, FromUnit::Frame { cmd: 0, data: vec![0x2e] }, FromUnit::Bad(vec![7, 3, 1, 0xff])]);
    }

    #[test]
    fn reset_line() {
        let mut p = Panel::new();
        p.set_reset(true, 0);
        assert_eq!(p.poll(100), None, "silent while held in reset");
        p.key(9, true);
        assert!(p.receive(&frame(0x07, &[0, 0, 0, 3]), 110).is_empty());
        p.set_reset(false, 200);
        assert_eq!(p.poll(200 + START_MS - 1), None, "still starting");
        assert_eq!(p.poll(200 + START_MS), Some(vec![0x0f, 0x02, 0x11]));
        assert!(p.receive(&[ACK], 230).is_empty());
        assert!(p.online);
        assert_eq!(p.resets, 0);
        // The key held since before the start is reported.
        assert_eq!(p.poll(240).unwrap(), frame(CMD_KEYS, &[0, 2, 0, 0, 0, 0]));
        p.receive(&[ACK], 250);
        // A pulse on the line starts it all over.
        p.set_reset(true, 300);
        p.set_reset(false, 302);
        assert!(!p.online);
        assert_eq!(p.resets, 1);
        assert_eq!(p.poll(302 + START_MS), Some(vec![0x0f, 0x02, 0x11]));
    }

    #[test]
    fn start_up_then_keys() {
        let mut p = Panel::new();
        assert_eq!(p.poll(0), Some(vec![0x0f, 0x02, 0x11]));
        assert_eq!(p.poll(100), None, "waiting for the acknowledgement");
        assert_eq!(p.poll(400), Some(vec![0x0f, 0x02, 0x11]), "repeated");
        assert!(p.receive(&[ACK], 410).is_empty());
        assert!(p.online);
        p.key(9, true);
        p.key(9, true);
        let f = p.poll(420).unwrap();
        assert_eq!(f, frame(CMD_KEYS, &[0, 2, 0, 0, 0, 0]));
        assert_eq!(p.poll(430), None);
        // The unit acknowledges and sends a frame of its own in one go.
        let mut reply = vec![ACK_WITH_FRAME];
        reply.extend(frame(0x07, &[1, 2, 3, 4, 5, 6]));
        assert_eq!(p.receive(&reply, 440), [ACK]);
        assert_eq!(p.from_unit[7].as_deref(), Some(&[1u8, 2, 3, 4, 5, 6][..]));
        p.key(9, false);
        p.knob(1, -2);
        assert_eq!(p.poll(450).unwrap(), frame(CMD_KEYS, &[0; 6]));
        p.receive(&[ACK], 451);
        assert_eq!(p.poll(452).unwrap(), frame(CMD_ROTARY, &[0, 0, 0x02, 0]));
        assert_eq!(key_bit("NAV"), Some(2));
        assert_eq!(key_bit("POWER"), None);
    }
}
