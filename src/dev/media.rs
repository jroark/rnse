//! Host interface to the 192's two SD/MP3 coprocessors.
//!
//! This models the boot downloader and the runtime commands identified so
//! far, not the DSP instruction set. Cards and audio decoding are not yet
//! implemented. Unknown commands receive no fabricated success response.

use std::collections::VecDeque;

#[derive(Default)]
enum Download {
    #[default]
    Blocks,
    Length,
    Address,
    Data,
    Running,
}

#[derive(Default)]
struct Dsp {
    download: Download,
    section: u8,
    blocks: u16,
    words: u32,
    rx: VecDeque<u16>,
    command: Vec<u16>,
    edge: bool,
    uploaded: u64,
    commands: u64,
    initialized: bool,
    mode: u16,
    library_enabled: bool,
    output_enabled: bool,
}

impl Dsp {
    fn write(&mut self, word: u16) {
        match self.download {
            Download::Blocks => {
                self.blocks = word;
                self.download = Download::Length;
                if word == 0 {
                    self.next_section();
                }
            }
            Download::Length => {
                // Program sections express lengths in pairs of 16-bit
                // words. The two data sections use individual words.
                self.words = u32::from(word) * if self.section < 2 { 2 } else { 1 };
                self.download = Download::Address;
            }
            Download::Address => {
                self.download = Download::Data;
                if self.words == 0 {
                    self.finish_block();
                }
            }
            Download::Data => {
                self.uploaded += 1;
                self.words -= 1;
                if self.words == 0 {
                    self.finish_block();
                }
            }
            Download::Running => {
                self.command.push(word);
                let length = 1 + (self.command[0] & 0xff) as usize;
                if self.command.len() == length {
                    self.commands += 1;
                    match self.command.as_slice() {
                        [0x1202, 0xd762, 0x0034] => {
                            self.initialized = true;
                            self.rx.push_back(0xf000);
                            self.edge = true;
                        }
                        [0x7301, mode] => {
                            self.mode = *mode;
                            self.rx.push_back(0xf000);
                            self.edge = true;
                        }
                        [0x3301, enabled] => {
                            self.output_enabled = *enabled != 0;
                            self.rx.push_back(0xf000);
                            self.edge = true;
                        }
                        [0x3101, enabled] => {
                            self.library_enabled = *enabled != 0;
                            self.rx.push_back(0xf000);
                            self.edge = true;
                        }
                        // Library identity: this protocol stand-in reports
                        // version 0.0, build 0, not a real DSP release.
                        [0x3201, 0] => {
                            self.rx.extend([0xd503, 0, 0, 0]);
                            self.edge = true;
                        }
                        // Card status query. Both slots are empty until
                        // the card protocol and image backing are modelled.
                        [0x2203, 1, 0x5000, _] => {
                            self.rx.extend([0xd301, 0]);
                            self.edge = true;
                        }
                        _ => {}
                    }
                    self.command.clear();
                }
            }
        }
    }

    fn finish_block(&mut self) {
        self.rx.push_back(0);
        self.blocks -= 1;
        self.download = Download::Length;
        if self.blocks == 0 {
            self.next_section();
        }
    }

    fn next_section(&mut self) {
        self.section += 1;
        self.download = if self.section == 4 { Download::Running } else { Download::Blocks };
    }
}

#[derive(Default)]
pub struct Media {
    dsps: [Dsp; 2],
}

impl Media {
    pub fn new() -> Self { Self::default() }

    /// Low four bits of port 1 at 0x1240_0001. A set bit means
    /// RX empty or TX busy; this stand-in accepts writes immediately.
    pub fn status(&self) -> u8 {
        (if self.dsps[0].rx.is_empty() { 4 } else { 0 })
            | (if self.dsps[1].rx.is_empty() { 1 } else { 0 })
    }

    pub fn read(&mut self, channel: usize) -> u16 {
        self.dsps[channel].rx.pop_front().unwrap_or(0)
    }

    pub fn write(&mut self, channel: usize, word: u16) {
        self.dsps[channel].write(word);
    }

    pub fn reset(&mut self, channel: usize) {
        self.dsps[channel] = Dsp::default();
    }

    /// Companion-chip edge latch bits 2 and 3, one per receive port.
    pub fn take_edges(&mut self) -> u16 {
        let mut edges = 0;
        for (i, dsp) in self.dsps.iter_mut().enumerate() {
            if std::mem::take(&mut dsp.edge) {
                edges |= 1 << (i + 2);
            }
        }
        edges
    }

    pub fn uploaded_words(&self, channel: usize) -> u64 { self.dsps[channel].uploaded }
    pub fn command_count(&self, channel: usize) -> u64 { self.dsps[channel].commands }
    pub fn initialized(&self, channel: usize) -> bool { self.dsps[channel].initialized }
    pub fn mode(&self, channel: usize) -> u16 { self.dsps[channel].mode }

    /// Available 16-bit DMA transfer units for a host port. Writes are
    /// always ready; reads must stop at the end of the receive FIFO.
    pub fn dma_request(&self, address: u32, write: bool) -> u32 {
        let channel = match address & 0x1fff_ffff {
            0x1500_0002 => 0,
            0x1500_004a => 1,
            _ => return 0,
        };
        if write { u32::MAX } else { self.dsps[channel].rx.len() as u32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot(m: &mut Media, ch: usize) {
        // Two program words followed by one data word. No firmware bytes.
        for word in [1, 1, 0xc00, 0xabcd, 0x1234, 0, 1, 1, 0x800, 0x5678, 0] {
            m.write(ch, word);
        }
    }

    #[test]
    fn download_and_command_replies_use_separate_interrupt_paths() {
        let mut m = Media::new();
        assert_eq!(m.status(), 5);
        boot(&mut m, 0);
        assert_eq!(m.uploaded_words(0), 3);
        assert_eq!(m.take_edges(), 0);
        assert_eq!(m.status(), 1);
        assert_eq!(m.read(0), 0);
        assert_eq!(m.read(0), 0);
        assert_eq!(m.status(), 5);
        for word in [0x1202, 0xd762, 0x0034] { m.write(0, word); }
        assert_eq!(m.take_edges(), 4);
        assert_eq!(m.take_edges(), 0);
        assert_eq!(m.read(0), 0xf000);
        assert_eq!(m.command_count(0), 1);
    }

    #[test]
    fn channels_and_reset_are_independent() {
        let mut m = Media::new();
        boot(&mut m, 1);
        m.read(1); m.read(1);
        m.write(1, 0x7301);
        assert_eq!(m.take_edges(), 0);
        m.write(1, 2);
        assert_eq!(m.take_edges(), 8);
        assert_eq!(m.status(), 4);
        m.reset(0);
        assert_eq!(m.read(1), 0xf000);
        m.write(1, 0x9900); // Unidentified command: no success reply.
        assert_eq!(m.status(), 5);
        m.reset(1);
        assert_eq!(m.command_count(1), 0);
    }

    #[test]
    fn host_dma_and_receive_interrupt() {
        use crate::machine::Machine;
        use crate::sh4::Bus;
        let mut m = Machine::new();
        boot(&mut m.board.media, 0);
        m.board.read16(0x1500_0002);
        m.board.read16(0x1500_0002);
        m.board.write16(0x07b0_0000, 0x0600); // DSP0 priority 6
        m.board.write16(0x07b0_001e, 4); // RX masked
        m.board.ram[..4].copy_from_slice(&[0x73, 1, 0, 2]);
        let dma = &mut m.cpu.io.dma[7];
        dma.sar = 0x8c00_0000;
        dma.dar = 0xb500_0002;
        dma.tcr = 2;
        dma.chcr = 0xa1025;
        // DDT mode must retain DBL and honor master enable.
        m.cpu.io.write(&mut m.board, 0xffa0_0040, 4, 0xc300, 0, 0);
        m.run(0);
        assert_eq!(m.cpu.io.dma[7].tcr, 2);
        m.cpu.io.write(&mut m.board, 0xffa0_0040, 4, 0xc301, 0, 0);
        assert_eq!(m.cpu.io.dmaor, 0xc301);
        m.run(0);
        assert_eq!(m.cpu.io.dma[7].tcr, 0);
        assert_eq!(m.board.media.mode(0), 2);
        assert_eq!(m.board.irl, 0);
        assert_eq!(m.board.read16(0x07b0_0016) & 4, 4);
        m.board.write16(0x07b0_001e, 0);
        assert_eq!(m.board.irl, 6);
        m.board.write16(0x07b0_0016, 0);
        assert_eq!(m.board.irl, 0);

        m.cpu.io.dma[7] = Default::default();
        let dma = &mut m.cpu.io.dma[7];
        dma.sar = 0xb500_0002;
        dma.dar = 0x8c00_0010;
        dma.tcr = 2;
        dma.chcr = 0xa4025;
        m.board.ram[18..20].copy_from_slice(&[0xbe, 0xef]);
        m.run(0);
        assert_eq!(&m.board.ram[16..20], &[0xf0, 0, 0xbe, 0xef]);
        assert_eq!(m.cpu.io.dma[7].tcr, 1, "DMA must stop at an empty FIFO");
        m.run(0);
        assert_eq!(m.cpu.io.dma[7].tcr, 1);
    }

    #[test]
    fn reset_clears_pending_reply_and_193_keeps_its_own_map() {
        use crate::board::{Board, Model};
        use crate::sh4::Bus;
        let mut b = Board::new();
        boot(&mut b.media, 0);
        b.read16(0x1500_0002); b.read16(0x1500_0002);
        b.write16(0x07b0_0000, 0x0600);
        b.write16(0x1500_0002, 0x7301);
        b.write16(0x1500_0002, 2);
        assert_eq!(b.irl, 6);
        b.write32(0x1600_0004, 0);
        assert_eq!(b.irl, 0);
        assert_eq!(b.read8(0x1240_0001) & 15, 5);
        assert_eq!(b.media.mode(0), 0);
        let mut b = Board::for_model(Model::Rnse193);
        b.write16(0x1500_0002, 0x1234);
        assert_eq!(b.read16(0x1500_0002), 0x1234);
        assert_eq!(b.media.uploaded_words(0), 0);
    }
}
