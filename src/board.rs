//! The RNS-E main board as seen from the SH-4 bus.
//!
//! Address map (physical, 29 bit), as far as it is understood:
//!
//! | range                     | CS | device                               |
//! |---------------------------|----|--------------------------------------|
//! | 0x0000_0000 - 0x01ff_ffff | 0  | NOR flash, 32 MB (code runs in place)|
//! | 0x0780_0000               | 1  | ATAPI drive, command block           |
//! | 0x0788_0000               | 1  | ATAPI drive, DMA data (DMAC ch. 1)   |
//! | 0x0790_0000               | 1  | ATAPI drive, control block           |
//! | 0x07aN_0000               | 1  | serial channels (N = 5: debug text)  |
//! | 0x07b0_0000               | 1  | interrupt priority registers         |
//! | 0x07c0_0000               | 1  | CAN controller (HCAN2 register map)  |
//! | 0x0c00_0000 - 0x0fff_ffff | 3  | SDRAM                                |
//! | 0x1000_0000 - 0x100f_ffff | 4  | battery-backed SRAM (settings)       |
//! | 0x1230_0001               | 4  | watchdog strobe (bit 0 toggled)      |
//! | 0x12xx_xxxx               | 4  | further byte-wide control latches    |
//! | 0x1400_0000 - 0x17ff_ffff | 5  | peripherals (not identified yet)     |
//! | 0x1800_0000 - 0x1bff_ffff | 6  | Fujitsu Coral graphics controller    |
//!
//! Regions without a device model behave like RAM (so that register
//! read-back works) and every access can be logged.

use crate::dev::atapi::Atapi;
use crate::dev::coral::{self, Coral};
use crate::dev::flash::Flash;
use crate::iolog::IoLog;
use crate::sh4::Bus;
use std::collections::HashMap;

pub const FLASH_SIZE: usize = 0x0200_0000;
pub const RAM_BASE: u32 = 0x0c00_0000;
pub const RAM_SIZE: usize = 0x0400_0000;
pub const CORAL_BASE: u32 = 0x1800_0000;
pub const NVRAM_BASE: u32 = 0x1000_0000;
pub const NVRAM_SIZE: usize = 0x0010_0000;

pub struct Board {
    pub flash: Vec<u8>,
    pub flash_ctl: Flash,
    /// Set while the flash is not in read-array mode.
    flash_special: bool,
    pub ram: Vec<u8>,
    /// Battery-backed SRAM holding settings; survives power-off on the
    /// real unit, so front ends may want to save and restore it.
    pub nvram: Vec<u8>,
    /// Backing store for address space without a device model.
    pub unmapped: HashMap<u32, Box<[u8; 4096]>>,
    pub coral: Coral,
    pub atapi: Atapi,
    /// Bytes transmitted on the companion chip's serial channels,
    /// indexed by the channel digit of its address (0x07aN_0000).
    pub uart_tx: [Vec<u8>; 16],
    /// Levels on the byte-wide input ports at 0x1250_0001, 0x1260_0001
    /// and 0x1270_0001 (indices 2, 3, 4 of the firmware's port table).
    /// What the individual bits mean is not known yet.
    pub gpio_in: [u8; 8],
    pub log: IoLog,
    pub pc: u32,
    pub irl: u8,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    pub fn new() -> Self {
        Board {
            flash: vec![0xff; FLASH_SIZE],
            flash_ctl: Flash::new(),
            flash_special: false,
            ram: vec![0; RAM_SIZE],
            nvram: vec![0; NVRAM_SIZE],
            unmapped: HashMap::new(),
            coral: Coral::new(),
            atapi: Atapi::new(),
            uart_tx: Default::default(),
            gpio_in: [0; 8],
            log: IoLog::new(false, 4),
            pc: 0,
            irl: 0,
        }
    }

    /// Whether `len` bytes at physical address `phys` lie entirely in
    /// RAM or entirely in flash on this board.
    pub fn can_load(phys: u32, len: usize) -> bool {
        let a = phys as usize;
        let ram = RAM_BASE as usize;
        (a >= ram && a + len <= ram + RAM_SIZE) || a + len <= FLASH_SIZE
    }

    /// Copy bytes into RAM or flash (used by loaders).  The range must
    /// satisfy [`Board::can_load`].
    pub fn load(&mut self, phys: u32, data: &[u8]) {
        let a = phys as usize;
        if phys >= RAM_BASE {
            let o = a - RAM_BASE as usize;
            self.ram[o..o + data.len()].copy_from_slice(data);
        } else {
            self.flash[a..a + data.len()].copy_from_slice(data);
        }
    }

    fn area_name(addr: u32) -> &'static str {
        match addr >> 26 {
            0 => "flash",
            1 => "cs1",
            2 => "cs2",
            3 => "ram",
            4 => "cs4",
            5 => "cs5",
            6 => "coral",
            _ => "cs7",
        }
    }

    /// 16-bit register of the interrupt priority block.
    fn ipr(&self, reg: u32) -> u32 {
        let a = 0x07b0_0000 + reg;
        match self.unmapped.get(&(a >> 12)) {
            Some(p) => ((p[(a & 0xfff) as usize] as u32) << 8) | p[(a & 0xfff) as usize + 1] as u32,
            None => 0,
        }
    }

    fn set_ipr(&mut self, reg: u32, v: u32) {
        let a = 0x07b0_0000 + reg;
        let p = self.unmapped.entry(a >> 12).or_insert_with(|| Box::new([0; 4096]));
        p[(a & 0xfff) as usize] = (v >> 8) as u8;
        p[(a & 0xfff) as usize + 1] = v as u8;
    }

    /// Recompute the level on the IRL pins.
    ///
    /// Device interrupt lines go to a priority encoder in the companion
    /// chip.  Each source has a 4-bit level in the registers at
    /// 0x07b0_0000 (level 0 masks it).  Some sources are edge-latched:
    /// their request bit is set in +0x16, gated by the same bit in +0x1e,
    /// and cleared by the handler writing 0 to it.  Known so far:
    ///
    /// | source          | level        | latch bit |
    /// |-----------------|--------------|-----------|
    /// | ATAPI INTRQ     | +0x00, 7:4   | -         |
    /// | display VSYNC   | +0x02, 11:8  | 6         |
    /// | CAN             | +0x12, 11:8  | -         |
    pub fn update_irl(&mut self) {
        let mut level = 0;
        if self.atapi.irq() {
            level = level.max((self.ipr(0x00) >> 4) & 0xf);
        }
        if self.ipr(0x16) & self.ipr(0x1e) & 0x40 != 0 {
            level = level.max((self.ipr(0x02) >> 8) & 0xf);
        }
        self.irl = level as u8;
    }

    /// Start of vertical blanking: latch the VSYNC interrupt request.
    pub fn vsync(&mut self) {
        self.coral.vsync();
        let v = self.ipr(0x16) | 0x40;
        self.set_ipr(0x16, v);
        self.update_irl();
    }

    #[cold]
    fn io_read(&mut self, addr: u32, size: u32) -> u32 {
        let v = self.io_read_inner(addr, size);
        self.update_irl();
        v
    }

    #[cold]
    fn io_write(&mut self, addr: u32, size: u32, v: u32) {
        self.io_write_inner(addr, size, v);
        self.update_irl();
    }

    fn io_read_inner(&mut self, addr: u32, size: u32) -> u32 {
        if (addr as usize) < FLASH_SIZE {
            return self.flash_ctl.read(&self.flash, addr, size);
        }
        if addr >> 26 == 6 {
            return self.coral.read(addr - CORAL_BASE, size, self.pc);
        }
        match addr & 0xfff0_0000 {
            0x1250_0000 | 0x1260_0000 | 0x1270_0000 => {
                let v = self.gpio_in[((addr >> 20) & 7) as usize - 3] as u32;
                self.log.log("gpio", false, addr, size, v, self.pc);
                return v;
            }
            NVRAM_BASE => {
                let o = (addr - NVRAM_BASE) as usize;
                let mut v = 0;
                for i in 0..size as usize {
                    v = (v << 8) | self.nvram[(o + i) & (NVRAM_SIZE - 1)] as u32;
                }
                return v;
            }
            0x0780_0000 if addr & 0x8_0000 != 0 => return self.atapi.dma_read(size),
            0x0780_0000 => return self.atapi.read((addr & 0x1f) >> 1, size, self.pc),
            0x0790_0000 => return self.atapi.read(8, size, self.pc),
            _ => {}
        }
        let mut v = 0u32;
        for i in 0..size {
            let a = addr + i;
            let b = self.unmapped.get(&(a >> 12)).map(|p| p[(a & 0xfff) as usize]).unwrap_or(0);
            v = (v << 8) | b as u32;
        }
        self.log.log(Self::area_name(addr), false, addr, size, v, self.pc);
        v
    }

    fn io_write_inner(&mut self, addr: u32, size: u32, v: u32) {
        if addr >> 26 == 6 {
            self.coral.write(addr - CORAL_BASE, size, v, self.pc);
            return;
        }
        match addr & 0xfff0_0000 {
            NVRAM_BASE => {
                let o = (addr - NVRAM_BASE) as usize;
                for i in 0..size as usize {
                    self.nvram[(o + i) & (NVRAM_SIZE - 1)] = (v >> ((size as usize - 1 - i) * 8)) as u8;
                }
                return;
            }
            0x0780_0000 => return self.atapi.write((addr & 0x1f) >> 1, size, v, self.pc),
            0x0790_0000 => return self.atapi.write(8, size, v, self.pc),
            _ => {}
        }
        if addr < FLASH_SIZE as u32 {
            self.flash_ctl.write(&mut self.flash, addr, size, v);
            self.flash_special = self.flash_ctl.special_read();
            return;
        }
        self.log.log(Self::area_name(addr), true, addr, size, v, self.pc);
        if addr & 0xfff0_ffff == 0x07a0_0006 {
            // Serial channel transmit data register.
            self.uart_tx[((addr >> 16) & 0xf) as usize].push(v as u8);
            return;
        }
        for i in 0..size {
            let a = addr + i;
            let b = (v >> ((size - 1 - i) * 8)) as u8;
            self.unmapped.entry(a >> 12).or_insert_with(|| Box::new([0; 4096]))[(a & 0xfff) as usize] = b;
        }
    }
}

impl Bus for Board {
    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o < RAM_SIZE {
            return self.ram[o];
        }
        if (addr as usize) < FLASH_SIZE && !self.flash_special {
            return self.flash[addr as usize];
        }
        let c = addr.wrapping_sub(CORAL_BASE) as usize;
        if c < coral::VRAM_SIZE {
            return self.coral.vram[c];
        }
        self.io_read(addr, 1) as u8
    }

    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o < RAM_SIZE - 1 {
            return u16::from_be_bytes([self.ram[o], self.ram[o + 1]]);
        }
        let a = addr as usize;
        if a < FLASH_SIZE - 1 && !self.flash_special {
            return u16::from_be_bytes([self.flash[a], self.flash[a + 1]]);
        }
        let c = addr.wrapping_sub(CORAL_BASE) as usize;
        if c < coral::VRAM_SIZE - 1 {
            let v = &self.coral.vram;
            return u16::from_be_bytes([v[c], v[c + 1]]);
        }
        self.io_read(addr, 2) as u16
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o < RAM_SIZE - 3 {
            return u32::from_be_bytes([self.ram[o], self.ram[o + 1], self.ram[o + 2], self.ram[o + 3]]);
        }
        let a = addr as usize;
        if a < FLASH_SIZE - 3 && !self.flash_special {
            return u32::from_be_bytes([self.flash[a], self.flash[a + 1], self.flash[a + 2], self.flash[a + 3]]);
        }
        let c = addr.wrapping_sub(CORAL_BASE) as usize;
        if c < coral::VRAM_SIZE - 3 {
            let v = &self.coral.vram;
            return u32::from_be_bytes([v[c], v[c + 1], v[c + 2], v[c + 3]]);
        }
        self.io_read(addr, 4)
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, v: u8) {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o < RAM_SIZE {
            self.ram[o] = v;
            return;
        }
        let c = addr.wrapping_sub(CORAL_BASE) as usize;
        if c < coral::VRAM_SIZE {
            self.coral.vram[c] = v;
            return;
        }
        self.io_write(addr, 1, v as u32);
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, v: u16) {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o < RAM_SIZE - 1 {
            self.ram[o..o + 2].copy_from_slice(&v.to_be_bytes());
            return;
        }
        let c = addr.wrapping_sub(CORAL_BASE) as usize;
        if c < coral::VRAM_SIZE - 1 {
            self.coral.vram[c..c + 2].copy_from_slice(&v.to_be_bytes());
            return;
        }
        self.io_write(addr, 2, v as u32);
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, v: u32) {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o < RAM_SIZE - 3 {
            self.ram[o..o + 4].copy_from_slice(&v.to_be_bytes());
            return;
        }
        let c = addr.wrapping_sub(CORAL_BASE) as usize;
        if c < coral::VRAM_SIZE - 3 {
            self.coral.vram[c..c + 4].copy_from_slice(&v.to_be_bytes());
            return;
        }
        self.io_write(addr, 4, v);
    }

    #[inline(always)]
    fn set_pc(&mut self, pc: u32, pr: u32) {
        self.pc = pc;
        self.log.pr = pr;
        self.coral.log.pr = pr;
        self.atapi.log.pr = pr;
    }

    #[inline(always)]
    fn irl(&self) -> u8 {
        self.irl
    }
}
