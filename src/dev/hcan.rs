//! CAN controller with the Renesas HCAN2 register layout.
//!
//! 16-bit registers, 32 mailboxes of 0x20 bytes from offset 0x100:
//!
//! ```text
//! +0x00  STDID << 4 | RTR << 3 | IDE << 2 | EXTID[17:16]
//! +0x02  EXTID[15:0]
//! +0x04  control: bits 10:8 = MBC (0 transmit, 2/3 receive, 7 off),
//!        bit 13 = NMC (overwrite unread message), bits 3:0 = DLC
//! +0x06  time stamp
//! +0x08  data bytes 0-7
//! +0x10  acceptance mask (same layout as +0, 1 = don't care)
//! ```
//!
//! The 193 unit's on-chip controller is the later RCAN-ET variant: same
//! registers, but the mailbox has the identifier two bits further down
//! (`STDID << 2`), the acceptance mask at +0x04 and the control word at
//! +0x10.  [`Hcan::rcan_et`] builds that flavour.
//!
//! The model moves whole frames: a transmit request completes at once
//! and is handed to the host through [`Hcan::tx`]; the host injects
//! received frames with [`Hcan::receive`].  Error counters and remote
//! frames are not modelled.  Bit timing is not modelled either, but the
//! registers' contents at each transmission are kept ([`Hcan::tx_timing`])
//! so that software which talks at another rate than the stock software
//! (21 quanta a bit, prescaler 14 on the 192 and 12 on the 193: 100
//! kbit/s) can be told so.  With [`Hcan::alone`] set there is no other
//! node on the bus: nothing arrives, and a frame sent is never
//! acknowledged, so its request stays pending until it is cancelled.

use crate::iolog::IoLog;
use std::collections::VecDeque;

const MCR: usize = 0x00;
const GSR: usize = 0x02;
const IRR: usize = 0x08;
const IMR: usize = 0x0a;
const TXPR1: usize = 0x20;
const TXCR1: usize = 0x28;
const TXACK1: usize = 0x30;
const ABACK1: usize = 0x38;
const RXPR1: usize = 0x40;
const RFPR1: usize = 0x48;
const MBIMR1: usize = 0x50;
const UMSR1: usize = 0x58;
const MB: usize = 0x100;

const IRR_RESET: u16 = 1 << 0;
const IRR_RX: u16 = 1 << 1;
const IRR_RFR: u16 = 1 << 2;
const IRR_TX: u16 = 1 << 8;
const IRR_OVERRUN: u16 = 1 << 9;

/// A classic CAN data frame with an 11-bit identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u16,
    pub len: u8,
    pub data: [u8; 8],
}

impl Frame {
    pub fn new(id: u16, bytes: &[u8]) -> Frame {
        let mut data = [0u8; 8];
        let len = bytes.len().min(8);
        data[..len].copy_from_slice(&bytes[..len]);
        Frame { id: id & 0x7ff, len: len as u8, data }
    }
}

impl std::fmt::Display for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:03x} [{}]", self.id, self.len)?;
        for b in &self.data[..self.len as usize] {
            write!(f, " {:02x}", b)?;
        }
        Ok(())
    }
}

pub struct Hcan {
    /// Register file, big-endian bytes.
    regs: Vec<u8>,
    /// Frames the firmware transmitted, oldest first.
    pub tx: VecDeque<Frame>,
    pub tx_count: u64,
    pub rx_count: u64,
    /// Received frames no mailbox accepted.
    pub rx_dropped: u64,
    pub log: IoLog,
    pub trace: bool,
    /// No other node on the bus (a unit on the bench).
    pub alone: bool,
    /// The bit timing registers (BCR0, BCR1) when a frame was last sent.
    pub tx_timing: Option<(u16, u16)>,
    /// Mailbox layout: bit position of the standard identifier in the
    /// first word, offset of the control word, offset of the mask.
    id_shift: u32,
    ctl: usize,
    mask: usize,
}

impl Default for Hcan {
    fn default() -> Self {
        Self::new()
    }
}

impl Hcan {
    pub fn new() -> Self {
        let mut h = Hcan { regs: vec![0; 0x500], tx: VecDeque::new(), tx_count: 0, rx_count: 0, rx_dropped: 0, log: IoLog::new(false, 4), trace: false, alone: false, tx_timing: None, id_shift: 4, ctl: 4, mask: 0x10 };
        h.reset();
        h
    }

    /// A controller with the RCAN-ET mailbox layout (see the module
    /// documentation).
    pub fn rcan_et() -> Self {
        let mut h = Hcan { id_shift: 2, ctl: 0x10, mask: 4, ..Self::new() };
        // The top bit of the control register is set on a unit (0x8000
        // under its stock software); see `shift`.
        h.w16(MCR, 0x8001);
        h
    }

    /// Hardware reset state: reset mode, everything masked.
    pub fn reset(&mut self) {
        self.regs.iter_mut().for_each(|b| *b = 0);
        self.w16(MCR, 0x0001);
        self.w16(GSR, 0x000c);
        self.w16(IRR, IRR_RESET);
        self.w16(IMR, 0xffff);
        self.w32(MBIMR1, 0xffff_ffff);
    }

    /// Put the controller in the state the boot ROM leaves it in: out of
    /// reset, with the receive, transmit and bus-off interrupts enabled
    /// (the update loader, built on the same hardware layer, programs
    /// IMR = 0xfebd).  The OS never starts the controller itself.
    pub fn start(&mut self) {
        let (bcr0, bcr1) = self.stock_timing();
        self.w16(0x06, bcr0);
        self.w16(0x04, bcr1);
        self.w16(MCR, if self.ctl == 0x10 { 0x8000 } else { 0 });
        self.w16(GSR, 0x0004);
        self.w16(IRR, 0);
        self.w16(IMR, 0xfebd);
    }

    /// BCR0 and BCR1 as the stock software of the unit with this
    /// flavour of the controller sets them.
    pub fn stock_timing(&self) -> (u16, u16) {
        if self.id_shift == 4 { (0x000d, 0xf300) } else { (0x000b, 0xb730) }
    }

    #[inline]
    fn r16(&self, off: usize) -> u16 {
        u16::from_be_bytes([self.regs[off], self.regs[off + 1]])
    }
    #[inline]
    fn w16(&mut self, off: usize, v: u16) {
        self.regs[off..off + 2].copy_from_slice(&v.to_be_bytes());
    }
    /// Mailbox bit registers come in pairs: mailboxes 31-16, then 15-0.
    fn r32(&self, off: usize) -> u32 {
        (self.r16(off) as u32) << 16 | self.r16(off + 2) as u32
    }
    fn w32(&mut self, off: usize, v: u32) {
        self.w16(off, (v >> 16) as u16);
        self.w16(off + 2, v as u16);
    }

    fn running(&self) -> bool {
        self.r16(MCR) & 0x0003 == 0
    }

    /// Recompute the summary bits of IRR from the mailbox registers.
    fn update_irr(&mut self) {
        let mut irr = self.r16(IRR) & !(IRR_RX | IRR_RFR | IRR_TX | IRR_OVERRUN);
        if self.r32(RXPR1) != 0 {
            irr |= IRR_RX;
        }
        if self.r32(RFPR1) != 0 {
            irr |= IRR_RFR;
        }
        if self.r32(TXACK1) | self.r32(ABACK1) != 0 {
            irr |= IRR_TX;
        }
        if self.r32(UMSR1) != 0 {
            irr |= IRR_OVERRUN;
        }
        self.w16(IRR, irr);
    }

    /// Interrupt output.
    pub fn irq(&self) -> bool {
        let mask = !self.r32(MBIMR1);
        let mut irr = self.r16(IRR) & !(IRR_RX | IRR_RFR | IRR_TX | IRR_OVERRUN);
        if self.r32(RXPR1) & mask != 0 {
            irr |= IRR_RX;
        }
        if self.r32(RFPR1) & mask != 0 {
            irr |= IRR_RFR;
        }
        if (self.r32(TXACK1) | self.r32(ABACK1)) & mask != 0 {
            irr |= IRR_TX;
        }
        if self.r32(UMSR1) & mask != 0 {
            irr |= IRR_OVERRUN;
        }
        irr & !self.r16(IMR) != 0
    }

    fn mb_id(&self, n: usize) -> u16 {
        (self.r16(MB + n * 0x20) >> self.shift()) & 0x7ff
    }

    /// Where a mailbox's first word has the identifier.  The 193's
    /// controller has it two bits up while the top bit of its control
    /// register is set, as its stock software has it, and four bits up
    /// when software clears that bit: a bench unit whose controller the
    /// kit had set up from reset with 0 there sent what was written for
    /// 0x7f8 (0x1fe0) as 0x1fe, and took no frame for what it was
    /// (rnse-sdk's mon193, 2026-11-21).
    fn shift(&self) -> u32 {
        if self.ctl == 0x10 && self.r16(MCR) & 0x8000 == 0 { 4 } else { self.id_shift }
    }

    fn mb_mode(&self, n: usize) -> u8 {
        self.regs[MB + n * 0x20 + self.ctl] & 7
    }

    // ---- bus interface ----------------------------------------------------

    pub fn read(&mut self, off: u32, size: u32, pc: u32) -> u32 {
        let o = off as usize;
        let mut v = 0u32;
        for i in 0..size as usize {
            v = (v << 8) | self.regs.get(o + i).copied().unwrap_or(0) as u32;
        }
        self.log.log("can", false, off, size, v, pc);
        v
    }

    pub fn write(&mut self, off: u32, size: u32, v: u32, pc: u32) {
        self.log.log("can", true, off, size, v, pc);
        let o = off as usize;
        if o + size as usize > self.regs.len() {
            return;
        }
        if o >= MB {
            for i in 0..size as usize {
                self.regs[o + i] = (v >> ((size as usize - 1 - i) * 8)) as u8;
            }
            return;
        }
        // Control registers are 16-bit words; a 32-bit access covers two.
        if size == 4 {
            self.write(off, 2, v >> 16, pc);
            self.write(off + 2, 2, v & 0xffff, pc);
            return;
        }
        let reg = o & !1;
        let old = self.r16(reg);
        let new = if size >= 2 {
            (v >> ((size - 2) * 8)) as u16
        } else if o & 1 == 0 {
            (old & 0x00ff) | ((v as u16) << 8)
        } else {
            (old & 0xff00) | (v as u16 & 0xff)
        };
        match reg {
            MCR => {
                self.w16(MCR, new);
                if new & 1 != 0 {
                    // Reset request: enter reset mode.
                    let mcr = new;
                    self.reset();
                    self.w16(MCR, mcr);
                } else {
                    let mut gsr = self.r16(GSR) & !0x0018;
                    if new & 2 != 0 {
                        gsr |= 0x0010; // halt acknowledged
                    }
                    self.w16(GSR, gsr | 0x0004);
                }
            }
            GSR => {}
            IRR => {
                // Flags are cleared by writing 1; the mailbox summary
                // bits follow their registers.
                self.w16(IRR, old & !new);
                self.update_irr();
            }
            TXPR1 | 0x22 => {
                self.w16(reg, old | new);
                self.transmit();
            }
            TXCR1 | 0x2a => {
                // Only a frame nobody acknowledges is ever pending.
                let pending = self.r16(reg - 8);
                let cancelled = pending & new;
                if cancelled != 0 {
                    self.w16(reg - 8, pending & !cancelled);
                    self.w16(reg + 0x10, self.r16(reg + 0x10) | cancelled);
                    self.update_irr();
                }
            }
            TXACK1 | 0x32 | ABACK1 | 0x3a | RXPR1 | 0x42 | RFPR1 | 0x4a | UMSR1 | 0x5a => {
                self.w16(reg, old & !new);
                self.update_irr();
            }
            _ => self.w16(reg, new),
        }
    }

    /// A transmission has been asked for.  The frame is not out yet:
    /// it takes its time on the bus ([`tx_next`](Self::tx_next) and
    /// [`tx_complete`](Self::tx_complete), driven by the machine).
    fn transmit(&mut self) {
        if self.r32(TXPR1) == 0 || !self.running() {
            return;
        }
        self.tx_timing = Some((self.r16(0x06), self.r16(0x04)));
        if self.alone && self.trace {
            // Nobody acknowledges, so the frame never counts as sent,
            // but it is on the wires (again and again) from now until
            // it is cancelled: what a logic analyser would record.
            let pending = self.r32(TXPR1);
            if let Some(n) = (1..32).find(|n| pending & (1 << n) != 0) {
                let base = MB + n * 0x20;
                let len = (self.regs[base + self.ctl + 1] & 0xf).min(8);
                let mut f = Frame { id: self.mb_id(n), len, data: [0; 8] };
                f.data[..len as usize].copy_from_slice(&self.regs[base + 8..base + 8 + len as usize]);
                eprintln!("[can] tx {} (not acknowledged)", f);
            }
        }
    }

    /// The mailbox whose frame goes onto the bus next (the lowest of
    /// those waiting) with the frame's number of data bytes; none when
    /// nothing waits, the controller is stopped, or no other node is
    /// there to acknowledge (the frame is then repeated for ever).
    pub fn tx_next(&self) -> Option<(usize, u8)> {
        let pending = self.r32(TXPR1);
        if pending == 0 || !self.running() || self.alone {
            return None;
        }
        let n = (1..32).find(|n| pending & (1 << n) != 0)?;
        Some((n, (self.regs[MB + n * 0x20 + self.ctl + 1] & 0xf).min(8)))
    }

    /// The frame of mailbox `n` has gone out and been acknowledged.
    pub fn tx_complete(&mut self, n: usize) {
        let pending = self.r32(TXPR1);
        if pending & (1 << n) == 0 {
            // Cancelled in the meantime.
            return;
        }
        let base = MB + n * 0x20;
        let len = (self.regs[base + self.ctl + 1] & 0xf).min(8);
        let mut f = Frame { id: self.mb_id(n), len, data: [0; 8] };
        f.data[..len as usize].copy_from_slice(&self.regs[base + 8..base + 8 + len as usize]);
        if self.trace {
            eprintln!("[can] tx {}", f);
        }
        self.tx_count += 1;
        if self.tx.len() >= 4096 {
            self.tx.pop_front();
        }
        self.tx.push_back(f);
        self.w32(TXPR1, pending & !(1 << n));
        self.w32(TXACK1, self.r32(TXACK1) | 1 << n);
        self.update_irr();
    }

    /// Everything that waits goes out at once (for tests of the
    /// controller alone, where no time passes).
    pub fn tx_flush(&mut self) {
        while let Some((n, _)) = self.tx_next() {
            self.tx_complete(n);
        }
    }

    /// A frame arrives from the bus.  Returns true if a mailbox took it.
    pub fn receive(&mut self, f: &Frame) -> bool {
        if !self.running() || self.alone {
            return false;
        }
        self.rx_count += 1;
        // Highest numbered mailbox first, as the chip searches.
        for n in (0..32).rev() {
            let mode = self.mb_mode(n);
            let receives = n == 0 || mode == 2 || mode == 3;
            if !receives {
                continue;
            }
            let base = MB + n * 0x20;
            let mask = (self.r16(base + self.mask) >> self.shift()) & 0x7ff;
            if (self.mb_id(n) ^ f.id) & !mask & 0x7ff != 0 {
                continue;
            }
            let bit = 1u32 << n;
            let unread = self.r32(RXPR1) & bit != 0;
            if unread {
                self.w32(UMSR1, self.r32(UMSR1) | bit);
                if self.regs[base + self.ctl] & 0x20 == 0 {
                    // NMC clear: keep the old message, drop the new one.
                    if self.trace {
                        eprintln!("[can] rx {} lost: mailbox {} still holds an unread one", f, n);
                    }
                    self.update_irr();
                    return true;
                }
            }
            let ctl0 = self.r16(base) & 0x0003;
            self.w16(base, (f.id << self.shift()) | ctl0);
            self.regs[base + self.ctl + 1] = (self.regs[base + self.ctl + 1] & 0xf0) | f.len;
            self.regs[base + 8..base + 16].copy_from_slice(&f.data);
            self.w32(RXPR1, self.r32(RXPR1) | bit);
            self.update_irr();
            if self.trace {
                eprintln!("[can] rx {} -> mailbox {}", f, n);
            }
            return true;
        }
        if self.trace {
            eprintln!("[can] rx {}: no mailbox takes it", f);
        }
        self.rx_dropped += 1;
        false
    }

    /// Program a mailbox the way firmware would: `mode` 0 = transmit,
    /// 2 or 3 = receive; `mask` bits set are "don't care".
    pub fn configure_mailbox(&mut self, n: usize, id: u16, mask: u16, mode: u8) {
        let base = MB + n * 0x20;
        self.w16(base, (id & 0x7ff) << self.id_shift);
        self.w16(base + self.mask, (mask & 0x7ff) << self.id_shift);
        self.regs[base + self.ctl] = (self.regs[base + self.ctl] & !7) | (mode & 7);
    }

    /// One line per mailbox that is in use: identifier, type, acceptance
    /// mask, length, data, and whether its interrupt is unmasked.
    pub fn describe(&self) -> String {
        let mbimr = self.r32(MBIMR1);
        let mut out = String::new();
        for n in 0..32 {
            let base = MB + n * 0x20;
            if self.regs[base..base + 0x14].iter().all(|b| *b == 0) {
                continue;
            }
            let mask = (self.r16(base + self.mask) >> self.shift()) & 0x7ff;
            let data: Vec<String> = self.regs[base + 8..base + 16].iter().map(|b| format!("{:02x}", b)).collect();
            out += &format!(
                "  mailbox {:2}: id {:03x} type {} mask {:03x} dlc {} data {} irq {}\n",
                n,
                self.mb_id(n),
                self.mb_mode(n),
                mask,
                self.regs[base + self.ctl + 1] & 0xf,
                data.join(" "),
                if mbimr & (1 << n) == 0 { "on" } else { "off" }
            );
        }
        out
    }

    /// Identifiers the firmware has set up, as (mailbox, id, mode).
    pub fn mailboxes(&self) -> Vec<(usize, u16, u8)> {
        (0..32).map(|n| (n, self.mb_id(n), self.mb_mode(n))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Hcan {
        let mut h = Hcan::new();
        h.start();
        // Mailbox 5 receives 0x271, mailbox 9 transmits 0x661.
        h.write(0x100 + 5 * 0x20, 2, 0x2710, 0);
        h.write(0x100 + 5 * 0x20 + 4, 2, 0x0300, 0);
        h.write(0x100 + 9 * 0x20, 2, 0x6610, 0);
        h.write(0x100 + 9 * 0x20 + 4, 2, 0x2003, 0);
        h.write(0x50, 2, 0, 0);
        h.write(0x52, 2, 0, 0);
        h
    }

    #[test]
    fn received_frame_lands_in_the_matching_mailbox() {
        let mut h = setup();
        assert!(!h.irq());
        assert!(h.receive(&Frame::new(0x271, &[0x03, 0x00])));
        assert!(h.irq());
        assert_eq!(h.read(0x42, 2, 0), 1 << 5);
        assert_eq!(h.read(0x08, 2, 0) & 2, 2);
        assert_eq!(h.read(0x100 + 5 * 0x20 + 8, 1, 0), 0x03);
        assert_eq!(h.read(0x100 + 5 * 0x20 + 5, 1, 0) & 0xf, 2);
        // Acknowledge: write 1 to the pending bit.
        h.write(0x42, 2, 1 << 5, 0);
        assert!(!h.irq());
        assert_eq!(h.read(0x08, 2, 0) & 2, 0);
        // An identifier nobody listens to is dropped.
        assert!(!h.receive(&Frame::new(0x123, &[1])));
    }

    #[test]
    fn transmit_request_completes_and_reports_the_frame() {
        let mut h = setup();
        for (i, b) in [0x11u32, 0x22, 0x33].iter().enumerate() {
            h.write(0x100 + 9 * 0x20 + 8 + i as u32, 1, *b, 0);
        }
        h.write(0x22, 2, 1 << 9, 0);
        // Asked for, and on the bus for a while: not out yet.
        assert!(h.tx.is_empty());
        assert_eq!(h.read(0x22, 2, 0), 1 << 9);
        assert_eq!(h.tx_next(), Some((9, 3)));
        h.tx_complete(9);
        assert_eq!(h.tx.pop_front(), Some(Frame::new(0x661, &[0x11, 0x22, 0x33])));
        assert_eq!(h.read(0x22, 2, 0), 0);
        assert_eq!(h.read(0x32, 2, 0), 1 << 9);
        assert!(h.irq());
        h.write(0x32, 2, 1 << 9, 0);
        assert!(!h.irq());
    }

    #[test]
    fn nothing_moves_in_reset_mode() {
        let mut h = Hcan::new();
        assert!(!h.receive(&Frame::new(0x271, &[3])));
        assert_eq!(h.read(0x02, 2, 0) & 8, 8);
        h.write(0x00, 2, 0, 0);
        assert_eq!(h.read(0x02, 2, 0) & 8, 0);
    }

    #[test]
    fn rcan_et_mailbox_layout() {
        let mut h = Hcan::rcan_et();
        h.start();
        // Mailbox 1 transmits 0x436 with six data bytes, as the 193's
        // loader does; mailbox 3 receives 0x271.
        h.write(0x120, 2, 0x10d8, 0);
        h.write(0x128, 2, 0x1604, 0);
        h.write(0x130, 2, 0x0006, 0);
        h.write(0x160, 2, 0x271 << 2, 0);
        h.write(0x170, 2, 0x0200, 0);
        h.write(0x20, 4, 1 << 1, 0);
        h.tx_flush();
        let f = h.tx.pop_front().expect("frame sent");
        assert_eq!((f.id, f.len, &f.data[..2]), (0x436, 6, &[0x16u8, 0x04][..]));
        assert!(h.receive(&Frame::new(0x271, &[3])));
        assert_eq!(h.read(0x168, 1, 0), 3);
        assert_eq!(h.read(0x170, 2, 0) & 0xf, 1);
    }
}
