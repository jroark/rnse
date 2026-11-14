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
//! | 0x07d0_0000               | 1  | A/D converter                        |
//! | 0x0c00_0000 - 0x0fff_ffff | 3  | SDRAM                                |
//! | 0x1000_0000 - 0x100f_ffff | 4  | battery-backed SRAM (settings)       |
//! | 0x1230_0001               | 4  | watchdog strobe (bit 0 toggled)      |
//! | 0x12N0_0000 / 0x12N0_0001 | 4  | port N: direction (1 = output), data |
//! | 0x1500_0002 / 0x1500_004a | 5  | SD/MP3 DSP0 / DSP1 host data ports   |
//! | 0x1600_0004               | 5  | DSP reset release (bits 2 and 3)    |
//! | 0x1400_0000 - 0x17ff_ffff | 5  | peripherals (not identified yet)     |
//! | 0x1800_0000 - 0x1bff_ffff | 6  | Fujitsu Coral graphics controller    |
//!
//! Regions without a device model behave like RAM (so that register
//! read-back works) and every access can be logged.

use crate::dev::adc::Adc;
use crate::dev::atapi::Atapi;
use crate::dev::coral::{self, Coral};
use crate::dev::flash::Flash;
use crate::dev::hcan::Hcan;
use crate::dev::media::Media;
use crate::dev::uart::Uart;
use crate::iolog::IoLog;
use crate::sh4::Bus;
use std::collections::HashMap;

pub const FLASH_SIZE: usize = 0x0200_0000;
pub const RAM_BASE: u32 = 0x0c00_0000;
pub const RAM_SIZE: usize = 0x0400_0000;
pub const CORAL_BASE: u32 = 0x1800_0000;
pub const NVRAM_BASE: u32 = 0x1000_0000;
pub const NVRAM_SIZE: usize = 0x0010_0000;

/// The two generations of the unit.  They share next to nothing below
/// the firmware's module format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Model {
    /// 8P0 035 192: SH-4 (SH7750R class), Coral graphics chip, companion
    /// logic.  Firmware variant `V220`.
    Rnse192,
    /// 8P0 035 193 (2010 on): an SH-4A system-on-chip of Renesas'
    /// SH7770 "SH-Navi" family.  Firmware variants `V574` and `VB48`.
    Rnse193,
}

impl Model {
    /// The hardware a firmware variant on the disc is built for.
    pub fn for_variant(name: &str) -> Option<Model> {
        match name {
            // V220: Europe; V222: North America.
            "V220" | "V222" => Some(Model::Rnse192),
            // V574: Europe; V575: North America; VB48: not known.
            "V574" | "V575" | "VB48" => Some(Model::Rnse193),
            _ => None,
        }
    }

    pub fn flash_size(self) -> usize {
        match self {
            Model::Rnse192 => FLASH_SIZE,
            Model::Rnse193 => 0x0400_0000,
        }
    }

    pub fn ram_base(self) -> u32 {
        match self {
            Model::Rnse192 => RAM_BASE,
            Model::Rnse193 => 0x0800_0000,
        }
    }

    pub fn ram_size(self) -> usize {
        match self {
            Model::Rnse192 => RAM_SIZE,
            Model::Rnse193 => 0x0800_0000,
        }
    }

    pub fn nvram_base(self) -> u32 {
        match self {
            Model::Rnse192 => NVRAM_BASE,
            Model::Rnse193 => 0x1800_0000,
        }
    }

    /// Whether `len` bytes at physical address `phys` lie entirely in
    /// RAM or entirely in flash on this model.
    pub fn can_load(self, phys: u32, len: usize) -> bool {
        let a = phys as usize;
        let ram = self.ram_base() as usize;
        (a >= ram && a + len <= ram + self.ram_size()) || a + len <= self.flash_size()
    }
}

pub struct Board {
    pub model: Model,
    /// Physical address of the first byte of `ram`.
    pub ram_base: u32,
    /// Where the Coral's video memory starts, or an address nothing
    /// decodes to on a board without one.
    coral_base: u32,
    pub flash: Vec<u8>,
    pub flash_ctl: Flash,
    /// Set while the flash is not in read-array mode.
    flash_special: bool,
    pub ram: Vec<u8>,
    /// Battery-backed SRAM holding settings; survives power-off on the
    /// real unit, so front ends may want to save and restore it.
    pub nvram: Vec<u8>,
    /// Address bits the battery-backed memory decodes: its size less one.
    nvram_mask: usize,
    /// The highest offset into it the software has used (192).
    pub nvram_top: usize,
    /// Backing store for address space without a device model.
    pub unmapped: HashMap<u32, Box<[u8; 4096]>>,
    pub coral: Coral,
    pub atapi: Atapi,
    pub can: Hcan,
    /// The 192's two SD/MP3 coprocessor host interfaces.
    pub media: Media,
    /// The companion chip's serial channels, indexed by the channel
    /// digit of the address (0x07aN_0000).  Channel 5 carries the
    /// loader's debug text, channel 1 the link to the radio processor.
    pub uart: [Uart; 16],
    /// Levels on the byte-wide input ports at 0x1250_0001, 0x1260_0001
    /// and 0x1270_0001 (indices 2, 3, 4 of the firmware's port table).
    /// Known bits: port 2 bit 4 is a "supply good" signal; when it stays
    /// low for about six seconds the firmware masks interrupts and
    /// spins, waiting for the watchdog to reset the unit.
    pub gpio_in: [u8; 8],
    /// Output latches and direction registers (1 = output) of the same
    /// ports.
    pub gpio_out: [u8; 8],
    pub gpio_dir: [u8; 8],
    pub adc: Adc,
    /// Position of the motorised display, 0 (folded down, the drive and
    /// card slots accessible) to 1023 (closed).
    pub display_pos: u16,
    pub log: IoLog,
    /// Accesses to hardware without a model.
    pub unmodelled: crate::iolog::Unmodelled,
    /// Edges seen on the 192's watchdog strobe (port 3, bit 0).
    pub watchdog_edges: u64,
    /// Some device log is on and wants the return address of each
    /// instruction (see [`Board::refresh_logging`]).
    log_pr: bool,
    pub pc: u32,
    pub irl: u8,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    /// A 192 board.
    pub fn new() -> Self {
        Self::for_model(Model::Rnse192)
    }

    pub fn for_model(model: Model) -> Self {
        Board {
            model,
            ram_base: model.ram_base(),
            coral_base: if model == Model::Rnse192 { CORAL_BASE } else { 0xf000_0000 },
            flash: vec![0xff; model.flash_size()],
            flash_ctl: if model == Model::Rnse192 { Flash::chips(0x0100_0000) } else { Flash::uniform(0x2_0000) },
            flash_special: false,
            ram: vec![0; model.ram_size()],
            nvram: vec![0; NVRAM_SIZE],
            // The 192's is one SRAM of 256 KiB (NEC uPD442000A on the
            // board), and its software uses offsets up to 0x3f630.
            nvram_mask: if model == Model::Rnse192 { 0x3_ffff } else { NVRAM_SIZE - 1 },
            nvram_top: 0,
            unmapped: HashMap::new(),
            coral: Coral::new(),
            atapi: Atapi::new(),
            can: Hcan::new(),
            media: Media::new(),
            uart: Default::default(),
            gpio_in: [0, 0, 0x10, 0x04, 0, 0, 0, 0],
            gpio_out: [0; 8],
            gpio_dir: [0; 8],
            adc: Adc::with_inputs([0, 0, 0, 1023, 0, 0, 0, 0]),
            display_pos: 1023,
            log: IoLog::new(false, 4),
            unmodelled: Default::default(),
            watchdog_edges: 0,
            log_pr: false,
            pc: 0,
            irl: 0,
        }
    }

    /// Copy bytes into RAM or flash (used by loaders).  The range must
    /// satisfy [`Model::can_load`].
    pub fn load(&mut self, phys: u32, data: &[u8]) {
        let a = phys as usize;
        if phys >= self.ram_base {
            let o = a - self.ram_base as usize;
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
    /// Registers of the 192 whose model is simply that they hold what
    /// is written: the companion chip's interrupt priorities, masks and
    /// flags (which `update_irl` reads), the byte-wide ports' direction
    /// and data registers, and the output latches.  What the port lines
    /// are wired to is mostly unknown, but not how the registers behave
    /// for outputs.
    fn stored_192(addr: u32) -> bool {
        matches!(addr, 0x07b0_0000..=0x07b0_001f | 0x1230_0000..=0x12a0_0001 | 0x12d0_0000..=0x12d0_0007 | 0x1600_0004..=0x1600_0007) && (addr & 0xffff < 0x20)
    }

    /// The latch at 0x1600_0004 (32 bits).  Bit 0 is the reset line of
    /// the faceplate controller, low = reset; the other bits the OS
    /// sets (0x04, 0x08, 0x10, 0x20, 0x40) are unidentified.
    pub fn latch16(&self) -> u32 {
        let a = 0x1600_0004u32;
        match self.unmapped.get(&(a >> 12)) {
            Some(p) => u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
            None => 0,
        }
    }

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

    /// The 32-bit interrupt status register at 0x07b0_0018.  Bits known
    /// from the OS's handlers: 14 = CAN, 24 = serial channel 1.  (2, 13 and 29 are tested too;
    /// their sources are not identified.)
    fn irq_status(&self) -> u32 {
        let mut s = 0;
        if self.can.irq() {
            s |= 1 << 14;
        }
        if self.uart[1].irq() {
            s |= 1 << 24;
        }
        s
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
    /// | SD/MP3 DSP0 RX  | +0x00, 11:8  | 2         |
    /// | SD/MP3 DSP1 RX  | +0x00, 15:12 | 3         |
    /// | display VSYNC   | +0x02, 11:8  | 6         |
    /// | Coral INT       | +0x02, 7:4   | -         |
    /// | serial ch. 1    | +0x08, 3:0   | -         |
    /// | serial ch. 3    | +0x08, 11:8  | -         |
    /// | A/D converter   | +0x12, 3:0   | -         |
    /// | CAN             | +0x12, 11:8  | -         |
    ///
    /// Level-triggered sources also show in the status register at
    /// +0x18 (see `irq_status`).
    pub fn update_irl(&mut self) {
        if self.model == Model::Rnse193 {
            // Everything on this board interrupts through the on-chip
            // controller.
            self.irl = 0;
            return;
        }
        let mut level = 0;
        if self.atapi.irq() {
            level = level.max((self.ipr(0x00) >> 4) & 0xf);
        }
        if self.can.irq() {
            level = level.max((self.ipr(0x12) >> 8) & 0xf);
        }
        if self.coral.irq() {
            level = level.max((self.ipr(0x02) >> 4) & 0xf);
        }
        if self.uart[1].irq() {
            level = level.max(self.ipr(0x08) & 0xf);
        }
        if self.uart[3].irq() {
            level = level.max((self.ipr(0x08) >> 8) & 0xf);
        }
        if self.adc.irq() {
            level = level.max(self.ipr(0x12) & 0xf);
        }
        if self.ipr(0x16) & self.ipr(0x1e) & 0x40 != 0 {
            level = level.max((self.ipr(0x02) >> 8) & 0xf);
        }
        // The SD driver clears these mask bits when enabling RX.
        let dsp_edges = self.ipr(0x16) & !self.ipr(0x1e);
        for i in 0..2 {
            if dsp_edges & (4 << i) != 0 {
                level = level.max((self.ipr(0x00) >> (8 + 4 * i)) & 0xf);
            }
        }
        self.irl = level as u8;
    }

    /// Advance the fold-down display by `ms` milliseconds.
    ///
    /// The display tilts down on a motor to uncover the CD drive and the
    /// card slots.  Port 3 (0x1260_0001) drives it:
    ///
    /// | port 3 bit | direction | meaning                              |
    /// |------------|-----------|--------------------------------------|
    /// | 4          | out       | run the motor towards high readings  |
    /// | 3          | out       | run the motor towards low readings   |
    /// | 2          | in        | 0 only at the low end stop           |
    ///
    /// A/D input 3 follows the position (at most 94 at the low stop, at
    /// least 894 at the high stop as far as the firmware is concerned).
    /// A full sweep takes about 1.5 s here.
    pub fn step_display(&mut self, ms: u32) {
        let drive = self.gpio_out[3] & self.gpio_dir[3];
        let step = (ms * 1023 / 1500).max(1) as i32;
        let mut pos = self.display_pos as i32;
        if drive & 0x10 != 0 && drive & 0x08 == 0 {
            pos += step;
        } else if drive & 0x08 != 0 && drive & 0x10 == 0 {
            pos -= step;
        }
        self.display_pos = pos.clamp(0, 1023) as u16;
        self.adc.input[3] = self.display_pos;
        if self.display_pos > 94 {
            self.gpio_in[3] |= 0x04;
        } else {
            self.gpio_in[3] &= !0x04;
        }
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
        if (addr as usize) < self.flash.len() {
            return self.flash_ctl.read(&self.flash, addr, size);
        }
        if self.model == Model::Rnse193 {
            return self.io_read_193(addr, size);
        }
        if addr >> 26 == 6 {
            return self.coral.read(addr - CORAL_BASE, size, self.pc);
        }
        if size == 2 && matches!(addr, 0x1500_0002 | 0x1500_004a) {
            let v = self.media.read(usize::from(addr == 0x1500_004a)) as u32;
            self.log.log("media", false, addr, size, v, self.pc);
            return v;
        }
        if addr == 0x1240_0001 && size == 1 {
            let v = self.unmapped.get(&(addr >> 12)).map_or(0, |p| p[1]);
            let v = u32::from((v & !0x0f) | self.media.status());
            self.log.log("media", false, addr, size, v, self.pc);
            return v;
        }
        if addr & 0xffff_fffc == 0x07b0_0018 {
            // Interrupt status: which sources are requesting right now.
            // Handlers that share a level test their bit here.
            let status = self.irq_status();
            let v = match size {
                4 => status,
                2 => (status >> ((2 - (addr & 2)) * 8)) & 0xffff,
                _ => (status >> ((3 - (addr & 3)) * 8)) & 0xff,
            };
            self.log.log("intc", false, addr, size, v, self.pc);
            return v;
        }
        if addr & 0xfff0_0000 == 0x07a0_0000 {
            let v = self.uart[((addr >> 16) & 0xf) as usize].read(addr & 0xffff) as u32;
            self.log.log("uart", false, addr, size, v, self.pc);
            return v;
        }
        match addr & 0xfff0_0000 {
            0x1250_0000 | 0x1260_0000 | 0x1270_0000 if size == 1 => {
                let i = ((addr >> 20) & 7) as usize - 3;
                let v = if addr & 1 == 0 { self.gpio_dir[i] } else { (self.gpio_in[i] & !self.gpio_dir[i]) | (self.gpio_out[i] & self.gpio_dir[i]) } as u32;
                self.log.log("gpio", false, addr, size, v, self.pc);
                return v;
            }
            0x07d0_0000 => {
                let v = self.adc.read(addr) as u32;
                self.log.log("adc", false, addr, size, v, self.pc);
                return v;
            }
            NVRAM_BASE => {
                let o = (addr - NVRAM_BASE) as usize;
                self.nvram_top = self.nvram_top.max(o);
                let mut v = 0;
                for i in 0..size as usize {
                    v = (v << 8) | self.nvram[(o + i) & self.nvram_mask] as u32;
                }
                return v;
            }
            0x07c0_0000 => return self.can.read(addr & 0xf_ffff, size, self.pc),
            // The data register's second address: the DMA controller
            // reads disc data here, and the OS's driver takes the answers
            // to other commands from it word by word.
            0x0780_0000 if addr & 0x8_0000 != 0 => {
                return if self.atapi.dma_request() { self.atapi.dma_read(size) } else { self.atapi.read(0, size, self.pc) };
            }
            0x0780_0000 => return self.atapi.read((addr & 0x1f) >> 1, size, self.pc),
            0x0790_0000 => return self.atapi.read(8, size, self.pc),
            _ => {}
        }
        if !Self::stored_192(addr) {
            self.unmodelled.note(addr, false, self.pc);
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
        if self.model == Model::Rnse193 {
            return self.io_write_193(addr, size, v);
        }
        if addr >> 26 == 6 {
            self.coral.write(addr - CORAL_BASE, size, v, self.pc);
            return;
        }
        if size == 2 && matches!(addr, 0x1500_0002 | 0x1500_004a) {
            self.media.write(usize::from(addr == 0x1500_004a), v as u16);
            let edges = self.media.take_edges();
            self.set_ipr(0x16, self.ipr(0x16) | u32::from(edges));
            self.log.log("media", true, addr, size, v, self.pc);
            return;
        }
        if addr == 0x1600_0004 && size == 4 {
            for i in 0..2 {
                if v & (4 << i) == 0 {
                    self.media.reset(i);
                    self.set_ipr(0x16, self.ipr(0x16) & !(4 << i));
                }
            }
        }
        match addr & 0xfff0_0000 {
            NVRAM_BASE => {
                let o = (addr - NVRAM_BASE) as usize;
                self.nvram_top = self.nvram_top.max(o);
                for i in 0..size as usize {
                    self.nvram[(o + i) & self.nvram_mask] = (v >> ((size as usize - 1 - i) * 8)) as u8;
                }
                return;
            }
            0x07c0_0000 => return self.can.write(addr & 0xf_ffff, size, v, self.pc),
            0x0780_0000 => return self.atapi.write((addr & 0x1f) >> 1, size, v, self.pc),
            0x0790_0000 => return self.atapi.write(8, size, v, self.pc),
            _ => {}
        }
        if (addr as usize) < self.flash.len() {
            self.flash_ctl.write(&mut self.flash, addr, size, v);
            self.flash_special = self.flash_ctl.special_read();
            return;
        }
        self.log.log(Self::area_name(addr), true, addr, size, v, self.pc);
        match addr & 0xfff0_0000 {
            0x1250_0000 | 0x1260_0000 | 0x1270_0000 if size == 1 => {
                let i = ((addr >> 20) & 7) as usize - 3;
                if addr & 1 == 0 {
                    self.gpio_dir[i] = v as u8;
                } else {
                    self.gpio_out[i] = v as u8;
                }
                return;
            }
            0x07d0_0000 => return self.adc.write(addr, v as u16),
            _ => {}
        }
        if addr & 0xfff0_0000 == 0x07a0_0000 {
            self.uart[((addr >> 16) & 0xf) as usize].write(addr & 0xffff, v as u16);
            return;
        }
        if !Self::stored_192(addr) {
            self.unmodelled.note(addr, true, self.pc);
        }
        if addr == 0x1230_0001 && size == 1 {
            let old = self.unmapped.get(&(addr >> 12)).map_or(0, |p| p[(addr & 0xfff) as usize]);
            if (old ^ v as u8) & 1 != 0 {
                self.watchdog_edges += 1;
            }
        }
        for i in 0..size {
            let a = addr + i;
            let b = (v >> ((size - 1 - i) * 8)) as u8;
            self.unmapped.entry(a >> 12).or_insert_with(|| Box::new([0; 4096]))[(a & 0xfff) as usize] = b;
        }
    }

    // ---- 193 ---------------------------------------------------------------

    fn nvram_offset(&self, addr: u32) -> Option<usize> {
        let o = addr.wrapping_sub(self.model.nvram_base()) as usize;
        (o < NVRAM_SIZE).then_some(o)
    }

    fn io_read_193(&mut self, addr: u32, size: u32) -> u32 {
        if let Some(o) = self.nvram_offset(addr) {
            let mut v = 0;
            for i in 0..size as usize {
                v = (v << 8) | self.nvram[(o + i) & self.nvram_mask] as u32;
            }
            return v;
        }
        self.unmodelled.note(addr, false, self.pc);
        let mut v = 0u32;
        for i in 0..size {
            let a = addr + i;
            let b = self.unmapped.get(&(a >> 12)).map_or(0, |p| p[(a & 0xfff) as usize]);
            v = (v << 8) | b as u32;
        }
        self.log.log(Self::area_name_193(addr), false, addr, size, v, self.pc);
        v
    }

    fn io_write_193(&mut self, addr: u32, size: u32, v: u32) {
        if (addr as usize) < self.flash.len() {
            self.flash_ctl.write(&mut self.flash, addr, size, v);
            self.flash_special = self.flash_ctl.special_read();
            return;
        }
        if let Some(o) = self.nvram_offset(addr) {
            for i in 0..size as usize {
                self.nvram[(o + i) & self.nvram_mask] = (v >> ((size as usize - 1 - i) * 8)) as u8;
            }
            return;
        }
        self.log.log(Self::area_name_193(addr), true, addr, size, v, self.pc);
        self.unmodelled.note(addr, true, self.pc);
        for i in 0..size {
            let a = addr + i;
            let b = (v >> ((size - 1 - i) * 8)) as u8;
            self.unmapped.entry(a >> 12).or_insert_with(|| Box::new([0; 4096]))[(a & 0xfff) as usize] = b;
        }
    }

    fn area_name_193(addr: u32) -> &'static str {
        match addr >> 26 {
            0 => "flash",
            1 => "cs1",
            2 | 3 => "ram",
            4 => "cs4",
            5 => "cs5",
            6 => "cs6",
            _ => "cs7",
        }
    }
}

impl Bus for Board {
    #[inline(always)]
    fn is_ram(&self, addr: u32) -> bool {
        (addr.wrapping_sub(self.ram_base) as usize) < self.ram.len()
    }

    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        let o = addr.wrapping_sub(self.ram_base) as usize;
        if o < self.ram.len() {
            return self.ram[o];
        }
        if (addr as usize) < self.flash.len() && !self.flash_special {
            return self.flash[addr as usize];
        }
        let c = addr.wrapping_sub(self.coral_base) as usize;
        if c < coral::VRAM_SIZE {
            return self.coral.vram[c];
        }
        self.io_read(addr, 1) as u8
    }

    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        let o = addr.wrapping_sub(self.ram_base) as usize;
        if o < self.ram.len() - 1 {
            return u16::from_be_bytes([self.ram[o], self.ram[o + 1]]);
        }
        let a = addr as usize;
        if a < self.flash.len() - 1 && !self.flash_special {
            return u16::from_be_bytes([self.flash[a], self.flash[a + 1]]);
        }
        let c = addr.wrapping_sub(self.coral_base) as usize;
        if c < coral::VRAM_SIZE - 1 {
            let v = &self.coral.vram;
            return u16::from_be_bytes([v[c], v[c + 1]]);
        }
        self.io_read(addr, 2) as u16
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        let o = addr.wrapping_sub(self.ram_base) as usize;
        if o < self.ram.len() - 3 {
            return u32::from_be_bytes([self.ram[o], self.ram[o + 1], self.ram[o + 2], self.ram[o + 3]]);
        }
        let a = addr as usize;
        if a < self.flash.len() - 3 && !self.flash_special {
            return u32::from_be_bytes([self.flash[a], self.flash[a + 1], self.flash[a + 2], self.flash[a + 3]]);
        }
        let c = addr.wrapping_sub(self.coral_base) as usize;
        if c < coral::VRAM_SIZE - 3 {
            let v = &self.coral.vram;
            return u32::from_be_bytes([v[c], v[c + 1], v[c + 2], v[c + 3]]);
        }
        self.io_read(addr, 4)
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, v: u8) {
        let o = addr.wrapping_sub(self.ram_base) as usize;
        if o < self.ram.len() {
            self.ram[o] = v;
            return;
        }
        let c = addr.wrapping_sub(self.coral_base) as usize;
        if c < coral::VRAM_SIZE {
            self.coral.vram[c] = v;
            return;
        }
        self.io_write(addr, 1, v as u32);
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, v: u16) {
        let o = addr.wrapping_sub(self.ram_base) as usize;
        if o < self.ram.len() - 1 {
            self.ram[o..o + 2].copy_from_slice(&v.to_be_bytes());
            return;
        }
        let c = addr.wrapping_sub(self.coral_base) as usize;
        if c < coral::VRAM_SIZE - 1 {
            self.coral.vram[c..c + 2].copy_from_slice(&v.to_be_bytes());
            return;
        }
        self.io_write(addr, 2, v as u32);
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, v: u32) {
        let o = addr.wrapping_sub(self.ram_base) as usize;
        if o < self.ram.len() - 3 {
            self.ram[o..o + 4].copy_from_slice(&v.to_be_bytes());
            return;
        }
        let c = addr.wrapping_sub(self.coral_base) as usize;
        if c < coral::VRAM_SIZE - 3 {
            self.coral.vram[c..c + 4].copy_from_slice(&v.to_be_bytes());
            return;
        }
        self.io_write(addr, 4, v);
    }

    #[inline(always)]
    fn set_pc(&mut self, pc: u32, pr: u32) {
        self.pc = pc;
        if self.log_pr {
            self.log.pr = pr;
            self.coral.log.pr = pr;
            self.atapi.log.pr = pr;
            self.can.log.pr = pr;
        }
    }

    #[inline(always)]
    fn irl(&self) -> u8 {
        self.irl
    }
}

impl Board {
    /// Note whether any device log is switched on; called before the CPU
    /// runs, so that the logs' fields can be set directly.
    pub fn refresh_logging(&mut self) {
        self.log_pr = self.log.enabled || self.coral.log.enabled || self.atapi.log.enabled || self.can.log.enabled;
    }
}
