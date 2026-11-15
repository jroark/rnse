//! The radio/audio processor on the other end of serial channel 1.
//!
//! The real one is a separate microcontroller with its own firmware (the
//! `A*.MOT` files on an update disc): on the 192 an NEC V850
//! (`D70F3079AY`) on a board made by Grundig, with a Philips SAA7709H
//! signal processor doing the sound.  Only its side of the serial
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
//! The amplifier's part (192, software 0815):
//!
//! * `19 [01 v]` sets the volume, v from 0 to 30 (7 after a first
//!   start).  The main unit keeps the level itself and sends the new
//!   one for every movement of the volume knob.
//! * `81 [08 s v ..]` selects the source, 3 the tuner and 1 the CD
//!   drive, and gives the volume for it.
//! * The answer to request 0x88 is a status block, which the main unit
//!   also accepts unasked at any time.  It takes four fields from the
//!   first eight bytes (not understood; zero here) and sixteen bytes of
//!   amplifier state from offset 33: the source, the volume (it logs
//!   these two: `AMP SOURCE = %d VOL = %d`), and fourteen more of which
//!   only this is known: while the eighth is not zero the volume knob
//!   does nothing.  The main unit lets its own volume rise at most five
//!   steps above the volume reported here, so an amplifier that never
//!   reports keeps the unit quiet.  This stand-in reports source and
//!   volume as it was last told them, at once.
//!
//! The main unit knows two kinds of command.  A request is answered by a
//! frame with the same command number carrying the data asked for.  A
//! setting is answered by an acknowledgement: command 0x20 with one
//! status byte, zero for "done".  The main unit sends one command at a
//! time and repeats it five times, 600 ms apart, while the answer it
//! expects does not come; everything else queued for the radio waits
//! behind it.

/// A status block as a unit's radio processor sent it; see
/// [`Radio::status`].  (It had source 3 and volume 7 at 33 and 34, and
/// "#1 for  " where the blanks are.)
const STATUS: [u8; 57] = [
    0x20, 0x10, 0x0c, 0x14, 0x07, 0xfa, 0x3d, 0x0e, 0x00, 0x00, 0x00, 0x00, b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', 0x00, 0x00, 0x00, 0x0a, 0x0a, 0x20, 0x01, 0x65, 0x00,
    0x00, 0x00, 0x01, 0xff, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

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
    /// The amplifier as the main unit has set it: the source it plays
    /// (1 the CD drive, 3 the tuner) and the volume, 0 to 30.
    pub source: u8,
    pub volume: u8,
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
        Radio { parser: Parser::new(0x01), version: *b"A0000000", trace: false, source: 0, volume: 0, seen: Default::default() }
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

    /// Source number of the CD drive's audio.
    pub const SOURCE_CD: u8 = 1;

    /// What the amplifier does to the CD drive's signal: nothing gets
    /// through unless the drive is the selected source, and the volume
    /// scales it.  The steps' size is not known; 2 dB a step is assumed,
    /// full scale at 30, silence at 0.
    pub fn cd_gain(&self) -> f32 {
        self.gain(Self::SOURCE_CD)
    }

    /// The source number the 193 selects for what its own software
    /// decodes (MP3 from an SD card), which leaves the main processor
    /// through its serial sound interface.
    pub const SOURCE_DECODER: u8 = 4;

    /// The same for any source.
    pub fn gain(&self, source: u8) -> f32 {
        if self.source != source || self.volume == 0 {
            return 0.0;
        }
        10f32.powf(-((30 - self.volume.min(30)) as f32) * 2.0 / 20.0)
    }

    /// The status block (see the module's notes): 57 bytes, of which
    /// the first eight are four fields and sixteen from offset 33 the
    /// amplifier's state.  The rest is as a real radio processor sent
    /// it to a program that asked (`sdk/examples/suite` on a unit: a
    /// North American one, tuned to an FM station, the unit's software
    /// not running): what the bytes mean is not known beyond source
    /// and volume, and the eight characters of the station's text are
    /// left blank here.
    fn status(&self) -> Frame {
        let mut d = STATUS;
        d[33] = self.source;
        d[34] = self.volume;
        Frame::new(0x88, &d)
    }

    fn answer(&mut self, f: &Frame) -> Vec<Frame> {
        match f.cmd {
            // Version request: 16 bytes, the first eight are the version
            // string the main unit checks against the installed software.
            // A unit's answered `A2220141-080807` and a zero: the rest
            // is taken to be a date, and is that one's here.
            0x0b => {
                let mut id = *b"        -080807\0";
                id[..8].copy_from_slice(&self.version);
                vec![Frame::new(0x0b, &id)]
            }
            0x88 => vec![self.status()],
            // Volume and source: acknowledged, and reported back as
            // taken.
            0x19 if f.data.len() == 2 && f.data[0] == 1 => {
                self.volume = f.data[1];
                vec![Frame::new(ACK, &[0]), self.status()]
            }
            0x81 if f.data.len() >= 3 && f.data[0] == 8 => {
                // The source, and the volume to play it at.
                self.source = f.data[1];
                self.volume = f.data[2];
                vec![Frame::new(ACK, &[0]), self.status()]
            }
            // Other settings the main software has been seen to send:
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
    fn amplifier_state_is_reported_back() {
        let mut r = Radio::new();
        let say = |r: &mut Radio, f: Frame| -> Vec<Frame> {
            let mut p = Parser::new(0x00);
            r.from_unit(&f.encode(0x01)).into_iter().filter_map(|b| p.push(b)).collect()
        };
        assert_eq!(r.cd_gain(), 0.0);
        let got = say(&mut r, Frame::new(0x19, &[1, 12]));
        assert_eq!(got[0], Frame::new(ACK, &[0]));
        assert_eq!((got[1].cmd, got[1].data.len(), got[1].data[33], got[1].data[34]), (0x88, 57, 0, 12));
        assert_eq!(got[1].data[42], 0, "the byte that would make the main unit ignore its volume knob");
        assert_eq!(r.cd_gain(), 0.0, "the tuner is not the CD drive");
        let got = say(&mut r, Frame::new(0x81, &[8, 1, 12, 0, 0, 0, 0, 1, 3]));
        assert_eq!((got[1].data[33], got[1].data[34]), (1, 12));
        say(&mut r, Frame::new(0x81, &[8, 1, 9, 0, 0, 0, 0, 1, 3]));
        assert_eq!(r.volume, 9, "the source message carries the volume");
        say(&mut r, Frame::new(0x19, &[1, 12]));
        assert!((r.cd_gain() - 10f32.powf(-36.0 / 20.0)).abs() < 1e-6);
        say(&mut r, Frame::new(0x19, &[1, 30]));
        assert_eq!(r.cd_gain(), 1.0);
        // Asked for, the same block.
        let got = say(&mut r, Frame::new(0x88, &[]));
        assert_eq!((got.len(), got[0].data[33], got[0].data[34]), (1, 1, 30));
        // Other settings are acknowledged and nothing more.
        assert_eq!(say(&mut r, Frame::new(0x19, &[7, 0, 0x0c])), [Frame::new(ACK, &[0])]);
    }

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
