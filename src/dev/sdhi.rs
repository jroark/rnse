//! The 193's native SD host interface (16-bit, TMIO-style registers).
//!
//! Cards are read-only sector images. The host command/response and PIO
//! paths are modelled; the decoder and audio output are separate devices.
//! Sectors are read by DMA: the driver sets bit 1 of the register at
//! +0xd8 and has a DMA channel fetch the words from the data port at
//! +0x30 (`Navi::dma_run`); the short blocks of the card's own registers
//! (ACMD13, ACMD51) it reads from the data port itself.
//!
//! The 193's driver takes a high-capacity card only if it has more than
//! 0x3f6000 sectors (and at most 0x3fd8000), and a standard-capacity one
//! only up to that, so an image is the one kind or the other by its
//! size ([`Card::high_capacity`]).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

enum Storage { File(File), Bytes(Vec<u8>) }

pub struct Card {
    storage: Storage,
    sectors: u64,
}

impl Card {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let bytes = file.metadata()?.len();
        Self::check_size(bytes)?;
        Ok(Self { storage: Storage::File(file), sectors: bytes / 512 })
    }

    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        Self::check_size(bytes.len() as u64)?;
        Ok(Self { sectors: bytes.len() as u64 / 512, storage: Storage::Bytes(bytes) })
    }

    fn check_size(bytes: u64) -> io::Result<()> {
        if bytes < 512 * 1024 || bytes % (512 * 1024) != 0 || bytes > 32 * 1024 * 1024 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "SD card image must be a multiple of 512 KiB, between 512 KiB and 32 GiB"));
        }
        Ok(())
    }

    /// More sectors than the 193's driver allows a standard-capacity
    /// card: such an image is a high-capacity card (block addresses,
    /// version 2 of the card's data register), a smaller one a
    /// standard-capacity card (byte addresses, version 1).
    pub fn high_capacity(&self) -> bool { self.sectors > 0x3f_6000 }

    pub fn sectors(&self) -> u64 { self.sectors }

    /// The card-specific data register (CSD) with the card's size in it.
    pub(crate) fn csd(&self) -> [u8; 16] {
        if self.high_capacity() {
            let size = (self.sectors / 1024 - 1) as u32;
            let mut csd = [0x40, 0x0e, 0, 0x32, 0x5b, 0x59, 0, 0, 0, 0, 0x7f, 0x80, 0x0a, 0x40, 0, 1];
            csd[7] = (size >> 16) as u8 & 0x3f;
            csd[8] = (size >> 8) as u8;
            csd[9] = size as u8;
            return csd;
        }
        // Size = (C_SIZE + 1) << (C_SIZE_MULT + 2) blocks of
        // 1 << READ_BL_LEN bytes, with a 12-bit C_SIZE: blocks of 512
        // bytes reach 1 GiB, blocks of 1024 bytes 2 GiB.
        let block: u32 = if self.sectors > 4096 * 512 { 10 } else { 9 };
        let c_size = (self.sectors >> (block - 9 + 9)) as u32 - 1;
        let mult = 7u8;
        [0x00, 0x26, 0, 0x32, 0x5b, 0x50 | block as u8,
            0x80 | (c_size >> 10) as u8, (c_size >> 2) as u8, ((c_size & 3) << 6) as u8 | 0x2d,
            0xb4 | mult >> 1, (mult & 1) << 7 | 0x7f, 0x80, 0x0a, 0x40, 0, 1]
    }

    pub(crate) fn read(&mut self, sector: u32, bytes: usize) -> io::Result<Vec<u8>> {
        let offset = u64::from(sector) * 512;
        if offset + bytes as u64 > self.sectors * 512 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "read beyond SD image"));
        }
        let mut data = vec![0; bytes];
        match &mut self.storage {
            Storage::File(file) => { file.seek(SeekFrom::Start(offset))?; file.read_exact(&mut data)?; }
            Storage::Bytes(image) => data.copy_from_slice(&image[offset as usize..offset as usize + bytes]),
        }
        Ok(data)
    }
}

pub struct Sdhi {
    pub cards: [Option<Card>; 2],
    regs: [u16; 128],
    fifo: VecDeque<u8>,
    app: bool,
    selected: bool,
    pub commands: Vec<(u8, u32)>,
    pub bytes_read: u64,
    /// Print each command and what it is answered with (`--trace-sd`).
    pub trace: bool,
}

impl Default for Sdhi {
    fn default() -> Self {
        let mut host = Self { cards: [None, None], regs: [0; 128], fifo: VecDeque::new(),
            app: false, selected: false, commands: Vec::new(), bytes_read: 0, trace: false };
        host.reset();
        host
    }
}

impl Sdhi {
    fn reset(&mut self) {
        self.regs = [0; 128];
        self.regs[0x20 / 2] = 0xffff;
        self.regs[0x22 / 2] = 0xffff;
        self.regs[0x26 / 2] = 512;
        self.regs[0xe0 / 2] = 1;
        self.fifo.clear();
        self.app = false;
        self.selected = false;
    }

    pub fn present(&self, slot: usize) -> bool { self.cards[slot].is_some() }
    /// Is there data waiting in the read FIFO?
    pub fn data_ready(&self) -> bool { !self.fifo.is_empty() }
    fn slot(&self) -> usize { (self.regs[1] & 1) as usize }

    pub fn irq(&self) -> bool {
        self.regs[0x1c / 2] & !self.regs[0x20 / 2] & 0x1d != 0
            || self.regs[0x1e / 2] & !self.regs[0x22 / 2] & 0x837f != 0
    }

    pub fn read(&mut self, offset: u32) -> u16 {
        match offset {
            0x1c => {
                // Card present, write protected (images are read-only).
                (self.regs[0x1c / 2] & !0xa0) | if self.present(self.slot()) { 0 } else { 0x20 }
            }
            0x1e => self.regs[0x1e / 2] | 0x2000, // command bus idle
            0x30 => {
                if self.fifo.is_empty() { return 0; }
                let hi = self.fifo.pop_front().unwrap_or(0);
                let lo = self.fifo.pop_front().unwrap_or(0);
                self.bytes_read += 2;
                if self.fifo.is_empty() {
                    self.regs[0x1e / 2] &= !0x100;
                    self.regs[0x1c / 2] |= 4;
                } else if self.fifo.len() % usize::from(self.regs[0x26 / 2].max(1)) == 0 {
                    self.regs[0x1e / 2] |= 0x100;
                }
                u16::from_be_bytes([hi, lo])
            }
            0xf8 => self.slot() as u16,
            _ => self.regs[(offset as usize & 0xff) / 2],
        }
    }

    pub fn write(&mut self, offset: u32, value: u16) {
        let index = (offset as usize & 0xff) / 2;
        match offset {
            0 => { self.regs[0] = value; self.command(value); }
            0x1c | 0x1e => self.regs[index] &= value, // write zero to clear
            0xe0 if value & 1 == 0 => self.reset(),
            _ => self.regs[index] = value,
        }
    }

    fn response(&mut self, value: u32) {
        self.regs[0x0c / 2] = value as u16;
        self.regs[0x0e / 2] = (value >> 16) as u16;
    }

    fn long_response(&mut self, bytes: [u8; 16]) {
        // The controller strips the CRC/end byte. The driver reconstructs
        // its 136-bit response by shifting the register words left eight.
        let mut value = u128::from_be_bytes(bytes) >> 8;
        for index in 0x0c / 2..0x1c / 2 {
            self.regs[index] = value as u16;
            value >>= 16;
        }
    }

    fn data(&mut self, bytes: Vec<u8>) {
        self.fifo = bytes.into();
        if !self.fifo.is_empty() { self.regs[0x1e / 2] |= 0x100; }
    }

    fn command(&mut self, command: u16) {
        let cmd = (command & 0x3f) as u8;
        let arg = u32::from(self.regs[2]) | u32::from(self.regs[3]) << 16;
        self.commands.push((cmd, arg));
        let app = std::mem::take(&mut self.app);
        self.fifo.clear();
        self.regs[0x1c / 2] &= !5;
        self.regs[0x1e / 2] &= !0x837f;
        if !self.present(self.slot()) {
            self.regs[0x1e / 2] |= 0x40; // response timeout
            if self.trace { println!("[sd] slot {} CMD{} {:08x}: no card", self.slot() + 1, cmd, arg); }
            return;
        }
        let status = if self.selected { 0x900 } else { 0x700 };
        self.response(status);
        match (app, cmd) {
            (_, 0) => { self.selected = false; self.response(0); }
            (_, 8) => self.response(arg & 0xfff),
            (_, 55) => { self.app = true; self.response(status | 0x20); }
            // Ready, 3.3 V, and whether the card is a high-capacity one.
            (true, 41) => {
                let high = self.cards[self.slot()].as_ref().unwrap().high_capacity();
                self.response(if high { 0xc0ff_8000 } else { 0x80ff_8000 });
            }
            (_, 2 | 10) => self.long_response(*b"\x01RNEMUSD\x10\x00\x00\x00\x01\x01\x01\x01"),
            (_, 3) => self.response(0x0001_0700),
            (_, 7) => { self.selected = arg != 0; self.response(0x900); }
            (_, 9) => {
                let csd = self.cards[self.slot()].as_ref().unwrap().csd();
                self.long_response(csd);
            }
            (true, 51) => self.data(vec![2, 5, 0, 0, 0, 0, 0, 0]),
            (true, 13) => {
                let mut status = vec![0; 64];
                status[0] = 0x80; // four-bit bus, as selected by ACMD6
                status[8] = 2; // speed class 2
                status[10] = 0x40; // 128 KiB allocation unit
                status[12] = 1; // one allocation unit per erase group
                self.data(status);
            }
            (_, 12 | 13 | 16) | (true, 6) => {}
            (false, 6) => self.data(vec![0; 64]),
            (false, 17 | 18) => {
                let blocks = if cmd == 18 { self.regs[5].max(1) } else { 1 };
                let bytes = usize::from(blocks) * 512;
                let card = self.cards[self.slot()].as_mut().unwrap();
                // A standard-capacity card is addressed in bytes.
                let sector = if card.high_capacity() { arg } else { arg / 512 };
                match card.read(sector, bytes) {
                    Ok(bytes) => self.data(bytes),
                    Err(_) => { self.response(1 << 31); self.regs[0x1e / 2] |= 0x10; }
                }
            }
            // Images must never be modified by guest writes.
            (_, 24 | 25) => self.response((1 << 26) | status),
            _ => self.response((1 << 22) | status), // illegal command
        }
        self.regs[0x1c / 2] |= 1;
        if self.trace {
            println!("[sd] slot {} {}CMD{} {:08x} -> {:04x}{:04x}{}", self.slot() + 1,
                if app { "A" } else { "" }, cmd, arg, self.regs[0x0e / 2], self.regs[0x0c / 2],
                if self.fifo.is_empty() { String::new() } else { format!(", {} bytes", self.fifo.len()) });
        }
    }
}
