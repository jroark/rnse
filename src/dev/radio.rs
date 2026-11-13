//! The radio/audio processor on the other end of serial channel 1.
//!
//! The real one is a separate microcontroller with its own firmware (the
//! `A*.MOT` files on an update disc).  Only its side of the serial
//! protocol is imitated here, and only as far as the main software has
//! been seen to need it.
//!
//! Frames, in both directions:
//!
//! ```text
//! start   0x01 from the main unit, 0x00 from the radio
//! command 1 byte
//! length  2 bytes, big-endian: number of data bytes
//! data
//! check   one's complement of the sum of all preceding bytes
//! ```
//!
//! What is known of the amplifier's part (192, software 0815), none of
//! it acted on here:
//!
//! * `19 [01 v]` sets the volume, v from 0 to 30 (7 after a first
//!   start).  The main unit keeps the level itself and sends the new
//!   one for every movement of the volume knob.
//! * `81 [08 s ..]` selects the source: 3 the tuner, 1 the CD drive.
//! * The answer to request 0x88 is a status block.  The main unit takes
//!   four fields from its first eight bytes and the amplifier's source
//!   and volume from bytes 33 and 34 (it logs them: `AMP SOURCE = %d
//!   VOL = %d`), and it lets its own volume rise at most five steps
//!   above the volume reported there.  The same block is accepted
//!   unasked, at any time.  Answered with an empty block, as here, the
//!   unit reads zeroes and volume changes above 5 are held back; with a
//!   block of 40 zero bytes (and the volume, or the source, filled in)
//!   it stops sending volume changes at all, so the first bytes carry
//!   state that has to be right.  They are not understood yet.
//!
//! The main unit knows two kinds of command.  A request is answered by a
//! frame with the same command number carrying the data asked for.  A
//! setting is answered by an acknowledgement: command 0x20 with one
//! status byte, zero for "done".  The main unit sends one command at a
//! time and repeats it five times, 600 ms apart, while the answer it
//! expects does not come; everything else queued for the radio waits
//! behind it.

/// Command number of the radio's acknowledgement of a setting.
pub const ACK: u8 = 0x20;

/// A decoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub cmd: u8,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(cmd: u8, data: &[u8]) -> Frame {
        Frame { cmd, data: data.to_vec() }
    }

    /// Wire format with the given start byte.
    pub fn encode(&self, start: u8) -> Vec<u8> {
        let mut out = vec![start, self.cmd, (self.data.len() >> 8) as u8, self.data.len() as u8];
        out.extend_from_slice(&self.data);
        let sum = out.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        out.push(!sum);
        out
    }
}

impl std::fmt::Display for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02x}", self.cmd)?;
        if !self.data.is_empty() {
            write!(f, " [")?;
            for (i, b) in self.data.iter().enumerate() {
                write!(f, "{}{:02x}", if i > 0 { " " } else { "" }, b)?;
            }
            write!(f, "]")?;
        }
        Ok(())
    }
}

/// Incremental parser for frames with a known start byte.
pub struct Parser {
    start: u8,
    buf: Vec<u8>,
}

impl Parser {
    pub fn new(start: u8) -> Self {
        Parser { start, buf: Vec::new() }
    }

    /// Feed one byte; returns a frame when one completes.  Bytes that do
    /// not fit a frame are dropped.
    pub fn push(&mut self, b: u8) -> Option<Frame> {
        if self.buf.is_empty() && b != self.start {
            return None;
        }
        self.buf.push(b);
        if self.buf.len() < 4 {
            return None;
        }
        let len = ((self.buf[2] as usize) << 8) | self.buf[3] as usize;
        if len > 0x230 {
            self.buf.clear();
            return None;
        }
        if self.buf.len() < len + 5 {
            return None;
        }
        let body = &self.buf[..len + 4];
        let sum = body.iter().fold(0u8, |a, x| a.wrapping_add(*x));
        let frame = if !sum == self.buf[len + 4] { Some(Frame { cmd: self.buf[1], data: self.buf[4..len + 4].to_vec() }) } else { None };
        self.buf.clear();
        frame
    }
}

pub struct Radio {
    parser: Parser,
    /// Version the radio reports: eight characters, the `AUDVER` of the
    /// installed software set (the main unit compares the two).
    pub version: [u8; 8],
    pub trace: bool,
    /// Commands seen, for statistics: (command, first data byte or 0xff).
    pub seen: std::collections::BTreeMap<(u8, u8), u32>,
}

impl Default for Radio {
    fn default() -> Self {
        Self::new()
    }
}

impl Radio {
    pub fn new() -> Self {
        Radio { parser: Parser::new(0x01), version: *b"A0000000", trace: false, seen: Default::default() }
    }

    /// Bytes from the main unit; returns the bytes the radio sends back.
    pub fn from_unit(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in bytes {
            if let Some(f) = self.parser.push(b) {
                *self.seen.entry((f.cmd, f.data.first().copied().unwrap_or(0xff))).or_insert(0) += 1;
                for reply in self.answer(&f) {
                    if self.trace {
                        eprintln!("[radio] unit: {}   radio: {}", f, reply);
                    }
                    out.extend_from_slice(&reply.encode(0x00));
                }
            }
        }
        out
    }

    fn answer(&mut self, f: &Frame) -> Vec<Frame> {
        match f.cmd {
            // Version request: 16 bytes, the first eight are the version
            // string the main unit checks against the installed software.
            0x0b => {
                let mut id = [b' '; 16];
                id[..8].copy_from_slice(&self.version);
                vec![Frame::new(0x0b, &id)]
            }
            // Settings the main software has been seen to send:
            // acknowledged.
            0x17 | 0x19 | 0x80 | 0x81 => vec![Frame::new(ACK, &[0])],
            // Everything else is taken for a request and answered with
            // an empty frame of the same command, which satisfies the
            // ones seen so far (0x88, 0x8a, 0xd0, 0xd1, 0xd9, 0xdd, 0xde).
            _ => vec![Frame::new(f.cmd, &[])],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_as_seen_on_the_wire() {
        // Captured from the firmware.
        assert_eq!(Frame::new(0x0b, &[]).encode(0x01), [0x01, 0x0b, 0x00, 0x00, 0xf3]);
        assert_eq!(Frame::new(0x81, &[0x0d, 0x04, 0x05, 0xa0]).encode(0x01), [0x01, 0x81, 0x00, 0x04, 0x0d, 0x04, 0x05, 0xa0, 0xc3]);
        let mut p = Parser::new(0x01);
        let mut got = Vec::new();
        for b in [0xff, 0x01, 0x80, 0x00, 0x02, 0x00, 0x02, 0x7a, 0x01, 0xdd, 0x00, 0x00, 0x21] {
            got.extend(p.push(b));
        }
        assert_eq!(got, [Frame::new(0x80, &[0x00, 0x02]), Frame::new(0xdd, &[])]);
        // A bad checksum drops the frame.
        let mut p = Parser::new(0x01);
        assert!([0x01, 0x0b, 0x00, 0x00, 0xf4].iter().all(|b| p.push(*b).is_none()));
    }

    #[test]
    fn settings_are_acknowledged() {
        let mut r = Radio::new();
        let reply = r.from_unit(&Frame::new(0x81, &[0x0d, 0x04, 0x05, 0xa0]).encode(0x01));
        assert_eq!(reply, [0x00, 0x20, 0x00, 0x01, 0x00, 0xde]);
        let reply = r.from_unit(&Frame::new(0xd1, &[]).encode(0x01));
        assert_eq!(reply, Frame::new(0xd1, &[]).encode(0x00));
    }

    #[test]
    fn version_request_is_answered() {
        let mut r = Radio::new();
        r.version = *b"A2210141";
        let reply = r.from_unit(&Frame::new(0x0b, &[]).encode(0x01));
        let mut p = Parser::new(0x00);
        let f = reply.iter().filter_map(|b| p.push(*b)).next().unwrap();
        assert_eq!(f.cmd, 0x0b);
        assert_eq!(&f.data[..8], b"A2210141");
        assert_eq!(f.data.len(), 16);
    }
}
