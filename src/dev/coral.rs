//! Fujitsu "Coral" graphics display controller (MB8629x family).
//!
//! The chip occupies one 64 MB chip-select area:
//!
//! | offset      | contents                         |
//! |-------------|----------------------------------|
//! | 0x000_0000  | video memory                     |
//! | 0x1fc_0000  | host interface registers         |
//! | 0x1fd_0000  | display controller registers     |
//! | 0x1ff_0000  | drawing engine registers         |
//! | 0x1ff_8000  | geometry engine registers        |
//!
//! Registers are little-endian inside the chip, but the RNS-E runs it on
//! a big-endian bus, so a 32-bit register reads naturally while 16-bit
//! and 8-bit fields appear at "swapped" addresses (documented offset XOR
//! 2 or 3).  The same holds for video memory: pixel N of an 8 bpp
//! surface is the byte at bus address N ^ 3, and a 16 bpp pixel is the
//! big-endian halfword at (2N) ^ 2.  This model stores the register file in bus byte order and
//! decodes through [`Coral::r32`]/[`Coral::r16`]/[`Coral::r8`], which take
//! the documented offsets.

use crate::iolog::IoLog;

pub const VRAM_SIZE: usize = 0x01fc_0000;
const REG_BASE: u32 = 0x01fc_0000;
const REG_SIZE: usize = 0x0004_0000;

pub const HOST: u32 = 0x0000;
pub const DISP: u32 = 0x1_0000;
pub const DRAW: u32 = 0x3_0000;
pub const GEO: u32 = 0x3_8000;

// Host interface registers
pub const IST: u32 = HOST + 0x20;
pub const IMASK: u32 = HOST + 0x24;
pub const SRST: u32 = HOST + 0x2c;
pub const CID: u32 = HOST + 0xf0;

// Interrupt status bits
pub const IST_CERR: u32 = 1 << 0;
pub const IST_CEND: u32 = 1 << 1;
pub const IST_VSYNC: u32 = 1 << 2;
pub const IST_FSYNC: u32 = 1 << 3;
pub const IST_SYNCERR: u32 = 1 << 4;

// Display controller registers
pub const DCM0: u32 = DISP;
pub const DCE: u32 = DISP + 0x02;
pub const HTP: u32 = DISP + 0x06;
pub const HDP: u32 = DISP + 0x08;
pub const VTR: u32 = DISP + 0x12;
pub const VDP: u32 = DISP + 0x16;
pub const DCM1: u32 = DISP + 0x100;

/// Display layers in the order the chip numbers them.
#[derive(Clone, Copy, Debug)]
pub struct LayerRegs {
    pub name: &'static str,
    pub mode: u32,
    pub origin: u32,
    pub disp_addr: u32,
    pub dx: u32,
    pub dy: u32,
    pub palette: u32,
    /// Enable bit in the legacy DCE register.
    pub dce_bit: u32,
}

pub const LAYERS: [LayerRegs; 6] = [
    LayerRegs { name: "L0/C", mode: DISP + 0x20, origin: DISP + 0x24, disp_addr: DISP + 0x28, dx: DISP + 0x2c, dy: DISP + 0x2e, palette: DISP + 0x400, dce_bit: 0 },
    LayerRegs { name: "L1/W", mode: DISP + 0x30, origin: DISP + 0x34, disp_addr: DISP + 0x38, dx: 0, dy: 0, palette: DISP + 0x400, dce_bit: 1 },
    LayerRegs { name: "L2/ML", mode: DISP + 0x40, origin: DISP + 0x44, disp_addr: DISP + 0x48, dx: DISP + 0x54, dy: DISP + 0x56, palette: DISP + 0x800, dce_bit: 2 },
    LayerRegs { name: "L3/MR", mode: DISP + 0x58, origin: DISP + 0x5c, disp_addr: DISP + 0x60, dx: DISP + 0x6c, dy: DISP + 0x6e, palette: DISP + 0x800, dce_bit: 2 },
    LayerRegs { name: "L4/BL", mode: DISP + 0x70, origin: DISP + 0x74, disp_addr: DISP + 0x78, dx: DISP + 0x84, dy: DISP + 0x86, palette: DISP + 0x800, dce_bit: 3 },
    LayerRegs { name: "L5/BR", mode: DISP + 0x88, origin: DISP + 0x8c, disp_addr: DISP + 0x90, dx: DISP + 0x9c, dy: DISP + 0x9e, palette: DISP + 0x800, dce_bit: 3 },
];

pub struct Coral {
    pub vram: Vec<u8>,
    /// Register file, bus byte order.
    pub regs: Vec<u8>,
    pub log: IoLog,
    /// Frames completed since power-on.
    pub frame: u64,
    /// Display list / drawing words written to the FIFO but not yet
    /// interpreted (drawing engine is not modelled yet).
    pub fifo_words: u64,
}

impl Default for Coral {
    fn default() -> Self {
        Self::new()
    }
}

impl Coral {
    pub fn new() -> Self {
        let mut c = Coral { vram: vec![0; VRAM_SIZE], regs: vec![0; REG_SIZE], log: IoLog::new(false, 4), frame: 0, fifo_words: 0 };
        c.reset();
        c
    }

    fn reset(&mut self) {
        self.regs.iter_mut().for_each(|b| *b = 0);
        // Chip ID: Coral-P (MB86295), as reported in the CID register.
        self.w32(CID, 0x0000_0303);
    }

    // ---- register file, by documented (little-endian) offset -------------

    #[inline]
    pub fn r32(&self, off: u32) -> u32 {
        let o = (off & !3) as usize;
        u32::from_be_bytes([self.regs[o], self.regs[o + 1], self.regs[o + 2], self.regs[o + 3]])
    }
    #[inline]
    pub fn w32(&mut self, off: u32, v: u32) {
        let o = (off & !3) as usize;
        self.regs[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    #[inline]
    pub fn r16(&self, off: u32) -> u32 {
        (self.r32(off) >> ((off & 2) * 8)) & 0xffff
    }
    #[inline]
    pub fn r8(&self, off: u32) -> u32 {
        (self.r32(off) >> ((off & 3) * 8)) & 0xff
    }

    // ---- bus interface ----------------------------------------------------

    /// `addr` is the offset inside the chip-select area.
    pub fn read(&mut self, addr: u32, size: u32, pc: u32) -> u32 {
        let o = (addr - REG_BASE) as usize;
        let mut v = 0u32;
        for i in 0..size as usize {
            v = (v << 8) | self.regs[(o + i) & (REG_SIZE - 1)] as u32;
        }
        self.log.log("coral", false, addr, size, v, pc);
        v
    }

    pub fn write(&mut self, addr: u32, size: u32, v: u32, pc: u32) {
        self.log.log("coral", true, addr, size, v, pc);
        let off = addr - REG_BASE;
        let word = off & !3;
        let old = self.r32(word);
        // Merge the bytes written into the containing 32-bit word.
        let mut new = old;
        for i in 0..size {
            let byte = (v >> ((size - 1 - i) * 8)) & 0xff;
            let sh = (3 - ((off + i) & 3)) * 8;
            new = (new & !(0xff << sh)) | (byte << sh);
        }
        match word {
            IST => {
                // Status bits are cleared by writing 0 to them.
                self.w32(IST, old & new);
            }
            SRST => {
                if new & 1 != 0 {
                    let (ist, imask) = (0, self.r32(IMASK));
                    let mmr = self.r32(HOST + 0xfffc);
                    self.reset();
                    self.w32(IST, ist);
                    self.w32(IMASK, imask);
                    self.w32(HOST + 0xfffc, mmr);
                }
            }
            CID => {}
            _ => {
                if (DRAW..DRAW + 0x1_0000).contains(&word) {
                    self.fifo_words += 1;
                }
                self.w32(word, new);
            }
        }
    }

    /// True while the chip drives its interrupt output.
    pub fn irq(&self) -> bool {
        self.r32(IST) & self.r32(IMASK) != 0
    }

    /// Called once per display frame.
    pub fn vsync(&mut self) {
        self.frame += 1;
        let ist = self.r32(IST) | IST_VSYNC | IST_FSYNC;
        self.w32(IST, ist);
    }

    // ---- display ----------------------------------------------------------

    pub fn display_enabled(&self) -> bool {
        self.r16(DCE) & 0x8000 != 0
    }

    /// Visible size in pixels from the timing registers.
    pub fn resolution(&self) -> (usize, usize) {
        let w = self.r16(HDP) as usize + 1;
        let h = self.r16(VDP) as usize + 1;
        if w < 16 || h < 16 || w > 1024 || h > 768 {
            (400, 240)
        } else {
            (w, h)
        }
    }

    fn layer_enabled(&self, i: usize) -> bool {
        let dce = self.r16(DCE);
        let ext = self.r32(DCM1) >> 16;
        dce & (1 << LAYERS[i].dce_bit) != 0 || ext & (1 << i) != 0
    }

    fn palette(&self, base: u32, idx: u32) -> (u8, u8, u8, bool) {
        let e = self.r32(base + idx * 4);
        (((e >> 16) & 0xff) as u8, ((e >> 8) & 0xff) as u8, (e & 0xff) as u8, e >> 31 != 0)
    }

    /// Compose the enabled layers into an RGB888 image.
    pub fn render(&self) -> (usize, usize, Vec<u8>) {
        let (w, h) = self.resolution();
        let mut out = vec![0u8; w * h * 3];
        if !self.display_enabled() {
            return (w, h, out);
        }
        // Back to front.  The real ordering is programmable (DLS); the
        // default is L5 at the back and L0 in front.
        for i in (0..6).rev() {
            if !self.layer_enabled(i) || i == 1 {
                continue;
            }
            let l = &LAYERS[i];
            let mode = self.r32(l.mode);
            let direct = mode >> 31 != 0;
            let stride = (((mode >> 16) & 0xff) as usize) * 64;
            if stride == 0 {
                continue;
            }
            let da = (self.r32(l.disp_addr) & 0x03ff_ffff) as usize;
            // Colour 0 of an indirect layer in front of another one is
            // transparent; the bottom layer is opaque.
            let transparent0 = i != 5 && i != 4;
            for y in 0..h {
                let row = da + y * stride;
                for x in 0..w {
                    let (r, g, b);
                    if direct {
                        let p = (row + x * 2) ^ 2;
                        if p + 1 >= VRAM_SIZE {
                            break;
                        }
                        let v = u16::from_be_bytes([self.vram[p], self.vram[p + 1]]);
                        if v == 0 && transparent0 {
                            continue;
                        }
                        r = ((v >> 10) & 0x1f) as u8;
                        g = ((v >> 5) & 0x1f) as u8;
                        b = (v & 0x1f) as u8;
                        let o = (y * w + x) * 3;
                        out[o] = (r << 3) | (r >> 2);
                        out[o + 1] = (g << 3) | (g >> 2);
                        out[o + 2] = (b << 3) | (b >> 2);
                    } else {
                        let p = (row + x) ^ 3;
                        if p >= VRAM_SIZE {
                            break;
                        }
                        let idx = self.vram[p] as u32;
                        if idx == 0 && transparent0 {
                            continue;
                        }
                        let c = self.palette(l.palette, idx);
                        let o = (y * w + x) * 3;
                        out[o] = c.0;
                        out[o + 1] = c.1;
                        out[o + 2] = c.2;
                    }
                }
            }
        }
        (w, h, out)
    }

    /// Human-readable summary of the display configuration.
    pub fn describe(&self) -> String {
        let (w, h) = self.resolution();
        let mut s = format!(
            "coral: display {} {}x{} DCM0={:04x} DCE={:04x} DCM1={:08x} IST={:02x} IMASK={:02x} frames={} draw-reg-writes={}\n",
            if self.display_enabled() { "on" } else { "off" },
            w,
            h,
            self.r16(DCM0),
            self.r16(DCE),
            self.r32(DCM1),
            self.r32(IST),
            self.r32(IMASK),
            self.frame,
            self.fifo_words
        );
        for (i, l) in LAYERS.iter().enumerate() {
            let mode = self.r32(l.mode);
            s += &format!(
                "  {:<6} {} mode={:08x} ({}bpp stride {} height {}) origin={:07x} disp={:07x}\n",
                l.name,
                if self.layer_enabled(i) { "on " } else { "off" },
                mode,
                if mode >> 31 != 0 { 16 } else { 8 },
                ((mode >> 16) & 0xff) * 64,
                (mode & 0xfff) + 1,
                self.r32(l.origin),
                self.r32(l.disp_addr)
            );
        }
        s
    }
}
