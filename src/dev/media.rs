//! The 192's card board: two NEC uPD77115A signal processors, one per
//! SD card slot, each behind a 16-bit host port of the main processor
//! (`docs/hardware.md`, "The boards" and "SD card processors").
//!
//! The processors are emulated ([`Dsp`]) and run the program the main
//! software downloads into them; this module is the wiring between
//! them, the main processor's bus and the slots:
//!
//! * the host ports at 0x15000002 (processor 0) and 0x1500004a
//!   (processor 1), with the "may read" and "may write" pins of each in
//!   a port of the main processor (0x12400001);
//! * port pin 2 of each processor, the interrupt request to the main
//!   processor;
//! * the reset lines (0x16000004);
//! * a card in each slot, on the processor's card interface.
//!
//! The processors run in step with the main processor
//! ([`Media::advance`]), and in between whenever the main processor
//! touches a port: its driver polls the pins between words, and the
//! processor has to get from one word to the next in that time, as it
//! does on the board, where it is the faster of the two at this.

use super::sdcard::SdCard;
use super::sdhi::Card;
use super::upd77115::{self, irq, Dsp};

/// System clocks a processor gets to react to an access by the host
/// before the host looks again.
const SETTLE: u64 = 400;

pub struct Media {
    pub dsps: [Dsp; 2],
    /// System clocks each processor is ahead of the main processor,
    /// from running between host accesses.
    ahead: [u64; 2],
    /// Fraction of a system clock left over by [`Media::advance`].
    rest: u64,
    /// Log the words that go through the host ports.
    pub trace: bool,
}

impl Default for Media {
    fn default() -> Self {
        Self::new()
    }
}

impl Media {
    pub fn new() -> Self {
        let mut dsps = [Dsp::new(), Dsp::new()];
        dsps[0].name = "dsp0";
        dsps[1].name = "dsp1";
        Media { dsps, ahead: [0; 2], rest: 0, trace: false }
    }

    fn settle(&mut self, channel: usize) {
        let d = &mut self.dsps[channel];
        let before = d.cycles;
        d.settle(SETTLE);
        self.ahead[channel] += d.cycles - before;
    }

    /// Low four bits of the port at 0x1240_0001: bit 2 (processor 0)
    /// and bit 0 (processor 1) are high while there is nothing to read,
    /// bit 3 and bit 1 while the processor cannot take a word.
    pub fn status(&mut self) -> u8 {
        let mut v = 0;
        for (i, shift) in [(0, 2), (1, 0)] {
            self.settle(i);
            let d = &self.dsps[i];
            v |= (u8::from(!d.host_can_read()) | u8::from(!d.host_can_write()) << 1) << shift;
        }
        v
    }

    pub fn read(&mut self, channel: usize) -> u16 {
        self.settle(channel);
        let v = self.dsps[channel].host_read();
        if self.trace {
            eprintln!("[dsp{}] host R {:04x}", channel, v);
        }
        self.settle(channel);
        v
    }

    pub fn write(&mut self, channel: usize, word: u16) {
        self.settle(channel);
        if self.trace {
            eprintln!("[dsp{}] host W {:04x}", channel, word);
        }
        self.dsps[channel].host_write(word);
        self.settle(channel);
    }

    /// The reset line of a processor, pulled low.
    pub fn reset(&mut self, channel: usize) {
        self.dsps[channel].reset();
    }

    /// Interrupt requests to the main processor since the last call:
    /// bit 2 for processor 0, bit 3 for processor 1, as in the latch at
    /// 0x07b0_0016.
    pub fn take_edges(&mut self) -> u16 {
        let mut edges = 0;
        for (i, dsp) in self.dsps.iter_mut().enumerate() {
            if std::mem::take(&mut dsp.host_irq) {
                edges |= 1 << (i + 2);
            }
        }
        edges
    }

    /// Whether a host port asks for a DMA transfer in the given
    /// direction: the "may write" pin for a transfer to the processor,
    /// the "may read" pin for one from it.
    pub fn dma_request(&mut self, address: u32, write: bool) -> bool {
        let channel = match address & 0x1fff_ffff {
            0x1500_0002 => 0,
            0x1500_004a => 1,
            _ => return false,
        };
        self.settle(channel);
        let d = &self.dsps[channel];
        if write { d.host_can_write() } else { d.host_can_read() }
    }

    /// The processors' share of `cycles` cycles of a main processor that
    /// makes `per_second` of them a second.
    pub fn advance(&mut self, cycles: u64, per_second: u64) {
        let n = cycles * upd77115::CLOCK_HZ + self.rest;
        let clocks = n / per_second;
        self.rest = n % per_second;
        for i in 0..2 {
            let owed = clocks.saturating_sub(self.ahead[i]);
            self.ahead[i] = self.ahead[i].saturating_sub(clocks);
            self.dsps[i].run(owed);
        }
    }

    /// Put a card into a slot (0 or 1).
    pub fn insert(&mut self, slot: usize, card: Card) {
        self.dsps[slot].card = Some(SdCard::new(card));
    }

    /// Take the card out of a slot.  The slot's switch is on the
    /// processor's INT4 pin, which is how the program learns that its
    /// card is gone.
    pub fn eject(&mut self, slot: usize) {
        if self.dsps[slot].card.take().is_some() {
            self.dsps[slot].request(irq::INT4);
        }
    }

    pub fn present(&self, slot: usize) -> bool {
        self.dsps[slot].card.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boot ROM's download: a dummy word, the number of
    /// instructions, the handshake setting, the instructions.  This
    /// program sends the host the words it gets, each plus one, with the
    /// interrupt pin pulled low for each.
    fn boot(m: &mut Media, ch: usize) {
        let program: [u32; 12] = [
            0x3801_0004, // r0l = 4
            0x4810_3804, // PDT = r0l: pin 2 high, once it is an output
            0x3801_3004, // r0l = 0x3004
            0x4810_3805, // PCD = r0l: pin 2 is an output
            0x3801_0004, // r0l = 4
            0x4810_3804, // PDT = r0l: pin 2 high
            0x4811_3806, // r0l = HDT
            0x5800_0001, // r0 = r0 + 1
            0x4810_3806, // HDT = r0l
            0x6100_0000, // clr(r0)
            0x4810_3804, // PDT = r0l: pin 2 low
            0x2c7f_fc80, // jmp back to "r0l = 4"
        ];
        for word in [0, program.len() as u16, 0x0c00] {
            m.write(ch, word);
        }
        for w in program {
            m.write(ch, w as u16);
            m.write(ch, (w >> 16) as u16);
        }
    }

    #[test]
    fn host_words_pins_and_interrupts() {
        let mut m = Media::new();
        // Nothing to read, both processors ready for a word.
        assert_eq!(m.status(), 5);
        boot(&mut m, 0);
        assert!(m.dsps[0].fault.is_none(), "{:?}", m.dsps[0].fault);
        assert_eq!(m.status(), 5);
        assert_eq!(m.take_edges(), 0);
        m.write(0, 0x1233);
        assert_eq!(m.status(), 1, "a word to read");
        assert_eq!(m.take_edges(), 4);
        assert_eq!(m.take_edges(), 0);
        assert_eq!(m.read(0), 0x1234);
        assert_eq!(m.status(), 5);
        // The other processor still waits for its program.
        assert!(m.dsps[1].idle());
        boot(&mut m, 1);
        m.write(1, 7);
        assert_eq!(m.take_edges(), 8);
        assert_eq!(m.status(), 4);
        m.reset(1);
        assert_eq!(m.status(), 5);
        assert_eq!(m.read(0), 0x1234, "the word stays in the register");
    }

    #[test]
    fn time_is_shared_out() {
        let mut m = Media::new();
        boot(&mut m, 0);
        let before = m.dsps[0].cycles;
        // A second of a 235.2 MHz main processor.
        for _ in 0..1000 {
            m.advance(235_200, 235_200_000);
        }
        let got = m.dsps[0].cycles - before;
        assert!(got <= upd77115::CLOCK_HZ && got > upd77115::CLOCK_HZ - 10_000, "{got}");
    }

    #[test]
    fn host_dma_and_interrupt_latch() {
        use crate::machine::Machine;
        use crate::sh4::Bus;
        let mut m = Machine::new();
        boot(&mut m.board.media, 0);
        m.board.write16(0x07b0_0000, 0x0600); // processor 0 at priority 6
        m.board.write16(0x07b0_001e, 4); // masked
        m.board.ram[..4].copy_from_slice(&[0x11, 0x10, 0x22, 0x20]);
        let dma = &mut m.cpu.io.dma[7];
        dma.sar = 0x8c00_0000;
        dma.dar = 0xb500_0002;
        dma.tcr = 1;
        dma.chcr = 0xa1025;
        // The channel runs only with the master enable.
        m.cpu.io.write(&mut m.board, 0xffa0_0040, 4, 0xc300, 0, 0);
        m.run(0);
        assert_eq!(m.cpu.io.dma[7].tcr, 1);
        m.cpu.io.write(&mut m.board, 0xffa0_0040, 4, 0xc301, 0, 0);
        m.run(0);
        assert_eq!(m.cpu.io.dma[7].tcr, 0);
        // The processor has answered: its request is latched, and
        // reaches the main processor once unmasked.
        assert_eq!(m.board.irl, 0);
        assert_eq!(m.board.read16(0x07b0_0016) & 4, 4);
        m.board.write16(0x07b0_001e, 0);
        assert_eq!(m.board.irl, 6);
        m.board.write16(0x07b0_0016, 0);
        assert_eq!(m.board.irl, 0);
        // The answer by DMA; a second word is not there to be had.
        m.cpu.io.dma[7] = Default::default();
        let dma = &mut m.cpu.io.dma[7];
        dma.sar = 0xb500_0002;
        dma.dar = 0x8c00_0010;
        dma.tcr = 2;
        dma.chcr = 0xa4025;
        m.run(0);
        assert_eq!(&m.board.ram[16..18], &[0x11, 0x11]);
        assert_eq!(m.cpu.io.dma[7].tcr, 1, "a transfer waits for the processor's word");
    }

    #[test]
    fn the_193_has_no_card_board() {
        use crate::board::{Board, Model};
        use crate::sh4::Bus;
        let mut b = Board::for_model(Model::Rnse193);
        b.write16(0x1500_0002, 0x1234);
        assert_eq!(b.read16(0x1500_0002), 0x1234);
        assert!(b.media.dsps[0].idle() && b.media.dsps[0].cycles == 0);
    }
}
