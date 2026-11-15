//! An SD memory card as seen on its command line: the card's states,
//! its registers, and reads of its sectors.  This is the card of the
//! 192's slots, whose main software sends it commands one by one through
//! the slot's signal processor (`upd77115.rs`); the 193's host interface
//! (`sdhi.rs`) has its own, older handling of the same commands.
//!
//! The image behind a card ([`Card`]) is never written: a write command
//! is answered with the card's "write protect violation" status.

use super::sdhi::Card;

/// What the card puts on the command line in answer to a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// Nothing: the host's wait runs out.
    None,
    /// 48 bits: the index field and 32 bits of contents.
    Short(u8, u32),
    /// 136 bits: the 120 bits of a CID or CSD register and its CRC byte.
    Long([u8; 16]),
}

/// States of the card, as reported in bits 12-9 of its status.
const IDLE: u8 = 0;
const READY: u8 = 1;
const IDENT: u8 = 2;
const STANDBY: u8 = 3;
const TRANSFER: u8 = 4;
const DATA: u8 = 5;

pub struct SdCard {
    card: Card,
    state: u8,
    rca: u16,
    app: bool,
    /// The sector the next block of a multiple-block read comes from.
    stream: Option<u32>,
    /// Commands received, for tests and traces.
    pub commands: u64,
    pub sectors_read: u64,
    /// A card of version 1 of the specification: it does not answer
    /// CMD8 (which counts as an illegal command, and shows in the
    /// status of the next response), and like a real card it is busy
    /// for a few rounds of ACMD41 before it is ready.
    pub old: bool,
    illegal: bool,
    busy: u32,
    /// So many starts (CMD0) after which the card answers nothing: a
    /// card that is not awake yet when the host first asks.
    pub deaf: u32,
    asleep: bool,
    /// A MultiMediaCard: no application commands (CMD55 goes
    /// unanswered, and CMD8), CMD1 in ACMD41's place, and an address
    /// the host gives it with CMD3.
    pub mmc: bool,
}

fn crc7(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for b in bytes {
        for i in (0..8).rev() {
            let bit = (b >> i) & 1 ^ (crc >> 6) & 1;
            crc = (crc << 1) & 0x7f;
            if bit != 0 {
                crc ^= 0x09;
            }
        }
    }
    crc
}

impl Response {
    /// The bytes on the line after the start bits: six for a short
    /// response, sixteen (without the leading byte of a long one) for a
    /// long one.
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Response::None => Vec::new(),
            Response::Short(index, v) => {
                let mut b = vec![*index];
                b.extend(v.to_be_bytes());
                // The OCR response has no CRC, only ones.
                b.push(if *index == 0x3f { 0xff } else { crc7(&b) << 1 | 1 });
                b
            }
            Response::Long(r) => r.to_vec(),
        }
    }
}

impl SdCard {
    pub fn new(card: Card) -> Self {
        SdCard { card, state: IDLE, rca: 0, app: false, stream: None, commands: 0, sectors_read: 0, old: false, illegal: false, busy: 0, deaf: 0, asleep: false, mmc: false }
    }

    fn status(&self) -> u32 {
        u32::from(self.state) << 9 | 0x100
    }

    /// A command and its argument.  Returns the response and, for a
    /// command that reads, the first block of data.
    pub fn command(&mut self, cmd: u8, arg: u32) -> (Response, Option<Vec<u8>>) {
        self.commands += 1;
        let app = std::mem::take(&mut self.app);
        // An illegal command is reported once, in the next response.
        let status = self.status() | if std::mem::take(&mut self.illegal) { 0x0040_0000 } else { 0 };
        let r1 = |v: u32| Response::Short(cmd, v);
        if cmd == 0 {
            self.asleep = self.deaf > 0;
            self.deaf = self.deaf.saturating_sub(1);
        } else if self.asleep {
            return (Response::None, None);
        }
        match (app, cmd) {
            (_, 0) => {
                self.state = IDLE;
                self.stream = None;
                self.busy = if self.old || self.mmc { 3 } else { 0 };
                (Response::None, None)
            }
            (false, 8) if self.old || self.mmc => {
                self.illegal = true;
                (Response::None, None)
            }
            (_, 55) if self.mmc => {
                self.illegal = true;
                (Response::None, None)
            }
            (false, 1) if self.mmc => {
                let waited = self.busy == 0;
                self.busy = self.busy.saturating_sub(1);
                if waited && arg & 0x00ff_8000 != 0 {
                    self.state = READY;
                }
                (Response::Short(0x3f, 0x00ff_8000 | if waited { 0x8000_0000 } else { 0 }), None)
            }
            (false, 3) if self.mmc && (self.state == IDENT || self.state == STANDBY) => {
                self.rca = (arg >> 16) as u16;
                self.state = STANDBY;
                (r1(status), None)
            }
            // A card of version 2 of the specification answers this one.
            (false, 8) if self.state == IDLE => (r1(arg & 0xfff), None),
            (_, 55) => {
                self.app = true;
                (r1(status | 0x20), None)
            }
            (true, 41) => {
                // Ready, 2.7-3.6 V.  A high-capacity card comes up only
                // for a host that says it knows about such cards.
                let high = self.card.high_capacity();
                let waited = self.busy == 0;
                self.busy = self.busy.saturating_sub(1);
                let ready = (!high || arg & 0x4000_0000 != 0) && waited;
                if ready && arg & 0x00ff_8000 != 0 {
                    self.state = READY;
                }
                let ocr = 0x00ff_8000 | if ready { 0x8000_0000 } else { 0 } | if high && ready { 0x4000_0000 } else { 0 };
                (Response::Short(0x3f, ocr), None)
            }
            (false, 2) if self.state == READY => {
                self.state = IDENT;
                (Response::Long(self.cid()), None)
            }
            (false, 3) if self.state == IDENT || self.state == STANDBY => {
                self.rca = self.rca.wrapping_add(1).max(1);
                let r = u32::from(self.rca) << 16 | status & 0x1fff;
                self.state = STANDBY;
                (r1(r), None)
            }
            (false, 7) => {
                if (arg >> 16) as u16 == self.rca && (self.rca != 0 || self.mmc) && self.state >= STANDBY {
                    self.state = TRANSFER;
                    (r1(status), None)
                } else {
                    if self.state >= STANDBY {
                        self.state = STANDBY;
                    }
                    (Response::None, None)
                }
            }
            (false, 9) if self.state == STANDBY => (Response::Long(self.card.csd()), None),
            (false, 10) if self.state == STANDBY => (Response::Long(self.cid()), None),
            (false, 12) => {
                self.stream = None;
                if self.state == DATA {
                    self.state = TRANSFER;
                }
                (r1(status), None)
            }
            (_, 13) if !app => (r1(status), None),
            (true, 13) if self.state == TRANSFER => {
                let mut s = vec![0; 64];
                s[8] = 2; // speed class 2
                s[10] = 0x40; // 128 KiB allocation unit
                s[12] = 1;
                (r1(status), Some(s))
            }
            (false, 16) | (true, 6) | (true, 42) if self.state == TRANSFER => (r1(status), None),
            (true, 51) if self.state == TRANSFER => {
                // SCR: version 2.00 of the specification for a
                // high-capacity card, 1.01 otherwise; one data line.
                let version = if self.card.high_capacity() { 2 } else { 1 };
                (r1(status), Some(vec![version, 0x25, 0, 0, 0, 0, 0, 0]))
            }
            (false, 6) if self.state == TRANSFER => (r1(status), Some(vec![0; 64])),
            (false, 17 | 18) if self.state == TRANSFER => {
                // A standard-capacity card is addressed in bytes.
                let sector = if self.card.high_capacity() { arg } else { arg / 512 };
                match self.card.read(sector, 512) {
                    Ok(data) => {
                        self.sectors_read += 1;
                        if cmd == 18 {
                            self.stream = Some(sector + 1);
                            self.state = DATA;
                        }
                        (r1(status), Some(data))
                    }
                    Err(_) => (r1(status | 0x8000_0000), None),
                }
            }
            // The image is not written.
            (false, 24 | 25) if self.state == TRANSFER => (r1(status | 0x0400_0000), None),
            // Anything else is an illegal command, which a card leaves
            // unanswered.
            _ => (Response::None, None),
        }
    }

    /// The next block of a multiple-block read, until it is stopped or
    /// the card ends.
    pub fn next_block(&mut self) -> Option<Vec<u8>> {
        let sector = self.stream?;
        match self.card.read(sector, 512) {
            Ok(data) => {
                self.sectors_read += 1;
                self.stream = Some(sector + 1);
                Some(data)
            }
            Err(_) => {
                self.stream = None;
                None
            }
        }
    }

    fn cid(&self) -> [u8; 16] {
        let mut cid = *b"\x01RNEMUSD\x10\x00\x00\x00\x01\x01\x01\x00";
        cid[15] = crc7(&cid[..15]) << 1 | 1;
        cid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identification_and_a_read() {
        let mut image = vec![0u8; 1024 * 1024];
        image[512 * 3..512 * 3 + 4].copy_from_slice(b"here");
        let mut c = SdCard::new(Card::from_bytes(image).unwrap());
        assert_eq!(c.command(0, 0).0, Response::None);
        assert_eq!(c.command(17, 0).0, Response::None, "not in the transfer state yet");
        c.command(55, 0);
        assert_eq!(c.command(41, 0x00ff_8000).0, Response::Short(0x3f, 0x80ff_8000));
        assert!(matches!(c.command(2, 0).0, Response::Long(_)));
        let Response::Short(3, r) = c.command(3, 0).0 else { panic!() };
        assert_eq!(r >> 16, 1);
        assert!(matches!(c.command(9, r & 0xffff_0000).0, Response::Long(_)));
        assert_eq!(c.command(7, r & 0xffff_0000).0, Response::Short(7, 0x700));
        assert_eq!(c.command(13, r & 0xffff_0000).0, Response::Short(13, 0x900));
        let (r, data) = c.command(18, 3 * 512);
        assert_eq!(r, Response::Short(18, 0x900));
        assert_eq!(&data.unwrap()[..4], b"here");
        assert!(c.next_block().is_some());
        assert_eq!(c.command(12, 0).0, Response::Short(12, 0xb00));
        assert!(c.next_block().is_none());
        // Writes are refused, and reads past the end.
        assert_eq!(c.command(24, 0).0, Response::Short(24, 0x0400_0900));
        assert_eq!(c.command(17, 1024 * 1024).0, Response::Short(17, 0x8000_0900));
        assert_eq!(Response::Short(13, 0x900).bytes().len(), 6);
    }
}
