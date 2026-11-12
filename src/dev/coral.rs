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
/// Display list transfer: source address in video memory, word count,
/// request.
pub const LSA: u32 = HOST + 0x40;
pub const LCO: u32 = HOST + 0x44;
pub const LREQ: u32 = HOST + 0x48;

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
    /// Display lists executed.
    pub lists: u64,
    pub trace: bool,
    /// Display list commands executed, by type byte.
    pub cmd_counts: Vec<u64>,
    unknown: Vec<u32>,
}

/// Where drawing commands write.
struct Target {
    base: usize,
    /// Pixels per line.
    stride: usize,
    bpp16: bool,
    cx0: i32,
    cx1: i32,
    cy0: i32,
    cy1: i32,
}

impl Default for Coral {
    fn default() -> Self {
        Self::new()
    }
}

impl Coral {
    pub fn new() -> Self {
        let mut c = Coral { vram: vec![0; VRAM_SIZE], regs: vec![0; REG_SIZE], log: IoLog::new(false, 4), frame: 0, fifo_words: 0, lists: 0, trace: false, cmd_counts: vec![0; 256], unknown: Vec::new() };
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
            LREQ => {
                self.w32(word, new);
                if new & 1 != 0 {
                    self.run_display_list();
                    self.w32(LREQ, 0);
                }
            }
            _ => {
                if (DRAW..DRAW + 0x1_0000).contains(&word) {
                    self.fifo_words += 1;
                }
                self.w32(word, new);
            }
        }
    }

    #[inline]
    fn vram32(&self, off: usize) -> u32 {
        if off + 4 > VRAM_SIZE {
            return 0;
        }
        u32::from_be_bytes([self.vram[off], self.vram[off + 1], self.vram[off + 2], self.vram[off + 3]])
    }

    // ---- drawing engine --------------------------------------------------
    //
    // Display lists are sequences of 32-bit words.  The first word of a
    // command is `type << 24 | command << 16 | parameter`; how many words
    // follow depends on the type.  Only the 2D subset the firmware uses is
    // implemented; an unknown type ends the list (its length is unknown).

    /// Drawing register by its documented offset from the draw base.
    #[inline]
    fn dreg(&self, off: u32) -> u32 {
        self.r32(DRAW + off)
    }

    /// Frame buffer addresses are given either as offsets into video
    /// memory or as the CPU's address of the same byte.
    #[inline]
    fn vaddr(a: u32) -> usize {
        (a & 0x03ff_ffff) as usize
    }

    #[inline]
    fn px_read(&self, base: usize, stride: usize, x: i32, y: i32, bpp16: bool) -> u32 {
        if x < 0 || y < 0 {
            return 0;
        }
        if bpp16 {
            let p = (base + (y as usize * stride + x as usize) * 2) ^ 2;
            if p + 1 >= VRAM_SIZE {
                return 0;
            }
            u16::from_be_bytes([self.vram[p], self.vram[p + 1]]) as u32
        } else {
            let p = (base + y as usize * stride + x as usize) ^ 3;
            if p >= VRAM_SIZE {
                return 0;
            }
            self.vram[p] as u32
        }
    }

    #[inline]
    fn px_write(&mut self, base: usize, stride: usize, x: i32, y: i32, bpp16: bool, v: u32) {
        if x < 0 || y < 0 {
            return;
        }
        if bpp16 {
            let p = (base + (y as usize * stride + x as usize) * 2) ^ 2;
            if p + 1 < VRAM_SIZE {
                self.vram[p..p + 2].copy_from_slice(&(v as u16).to_be_bytes());
            }
        } else {
            let p = (base + y as usize * stride + x as usize) ^ 3;
            if p < VRAM_SIZE {
                self.vram[p] = v as u8;
            }
        }
    }

    /// Raster operation selected by a mode register's LOG field.
    #[inline]
    fn rop(log: u32, src: u32, dst: u32) -> u32 {
        match log & 0xf {
            0x0 => 0,
            0x1 => src & dst,
            0x2 => src & !dst,
            0x3 => src,
            0x4 => !src & dst,
            0x5 => dst,
            0x6 => src ^ dst,
            0x7 => src | dst,
            0x8 => !(src | dst),
            0x9 => !(src ^ dst),
            0xa => !dst,
            0xb => src | !dst,
            0xc => !src,
            0xd => !src | dst,
            0xe => !(src & dst),
            _ => !0,
        }
    }

    /// Destination surface and clip rectangle from the drawing registers.
    fn target(&self) -> Target {
        Target {
            base: Self::vaddr(self.dreg(0x440)),
            stride: (self.dreg(0x444) & 0xfff) as usize,
            bpp16: self.dreg(0x420) & 0x8000 != 0,
            cx0: (self.dreg(0x454) & 0xfff) as i32,
            cx1: (self.dreg(0x458) & 0xfff) as i32,
            cy0: (self.dreg(0x45c) & 0xfff) as i32,
            cy1: (self.dreg(0x460) & 0xfff) as i32,
        }
    }

    fn plot(&mut self, t: &Target, x: i32, y: i32, color: u32, log: u32) {
        if x < t.cx0 || x > t.cx1 || y < t.cy0 || y > t.cy1 {
            return;
        }
        let v = if log & 0xf == 3 {
            color
        } else {
            let d = self.px_read(t.base, t.stride, x, y, t.bpp16);
            Self::rop(log, color, d)
        };
        self.px_write(t.base, t.stride, x, y, t.bpp16, v);
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let t = self.target();
        let color = self.dreg(0x480);
        let log = (self.dreg(0x430) >> 9) & 0xf;
        for yy in y..y + h {
            for xx in x..x + w {
                self.plot(&t, xx, yy, color, log);
            }
        }
    }

    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let t = self.target();
        let color = self.dreg(0x480);
        let log = (self.dreg(0x424) >> 9) & 0xf;
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        for _ in 0..8192 {
            self.plot(&t, x, y, color, log);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Copy a rectangle between two surfaces (which may be the same).
    #[allow(clippy::too_many_arguments)]
    fn blit(&mut self, sbase: usize, sstride: usize, sx: i32, sy: i32, dbase: usize, dstride: usize, dx: i32, dy: i32, w: i32, h: i32, clip: bool) {
        let t = self.target();
        let bpp16 = t.bpp16;
        let mode = self.dreg(0x430);
        let log = (mode >> 9) & 0xf;
        let transparent = mode & 2 != 0;
        let key = self.dreg(0x280) & if bpp16 { 0x7fff } else { 0xff };
        // Choose the copy direction so that overlapping moves work.
        let backwards = sbase == dbase && (dy > sy || (dy == sy && dx > sx));
        for j in 0..h {
            let j = if backwards { h - 1 - j } else { j };
            for i in 0..w {
                let i = if backwards { w - 1 - i } else { i };
                let src = self.px_read(sbase, sstride, sx + i, sy + j, bpp16);
                if transparent && (src & if bpp16 { 0x7fff } else { 0xff }) == key {
                    continue;
                }
                let (x, y) = (dx + i, dy + j);
                if clip && (x < t.cx0 || x > t.cx1 || y < t.cy0 || y > t.cy1) {
                    continue;
                }
                let v = if log == 3 { src } else { Self::rop(log, src, self.px_read(dbase, dstride, x, y, bpp16)) };
                self.px_write(dbase, dstride, x, y, bpp16, v);
            }
        }
    }

    fn run_display_list(&mut self) {
        let addr = (self.r32(LSA) & 0x03ff_fffc) as usize;
        let count = (self.r32(LCO) & 0x00ff_ffff) as usize;
        let words: Vec<u32> = (0..count).map(|i| self.vram32(addr + i * 4)).collect();
        self.lists += 1;
        if self.trace {
            eprintln!("[coral] display list #{} at {:07x}, {} words (frame {}):", self.lists, addr, count, self.frame);
            for chunk in words.chunks(8) {
                eprintln!("[coral]   {}", chunk.iter().map(|w| format!("{:08x}", w)).collect::<Vec<_>>().join(" "));
            }
        }
        let mut ist = self.execute(&words);
        ist |= IST_CEND;
        let v = self.r32(IST) | ist;
        self.w32(IST, v);
    }

    /// Run a display list; returns interrupt status bits to raise.
    fn execute(&mut self, words: &[u32]) -> u32 {
        let mut i = 0;
        let mut ist = 0;
        let xy = |w: u32| -> (i32, i32) { ((w & 0xffff) as i16 as i32, (w >> 16) as i16 as i32) };
        while i < words.len() {
            let head = words[i];
            let (ty, cmd, param) = (head >> 24, (head >> 16) & 0xff, head & 0xffff);
            let arg = |k: usize| words.get(i + k).copied().unwrap_or(0);
            self.cmd_counts[ty as usize] += 1;
            let len = match ty {
                0xff | 0xfc | 0xf0 | 0x20 | 0x23 | 0x40 => 1, // Nop, Sync, Draw (flush), G_Nop, G_End, G_Init
                0xfd => {
                    // Interrupt: the list asks for the "command end" flag.
                    ist |= IST_CEND;
                    1
                }
                0xf1 => {
                    // SetRegister: `cmd` values from register `param * 4`.
                    for k in 0..cmd {
                        let off = (param + k) * 4;
                        if off < 0x1_0000 {
                            self.w32(DRAW + off, arg(1 + k as usize));
                        }
                    }
                    1 + cmd as usize
                }
                0xc0 => {
                    // SetModeRegister: MDR0-MDR4 selected by `cmd`.
                    if cmd <= 4 {
                        self.w32(DRAW + 0x420 + cmd * 4, arg(1));
                    }
                    2
                }
                0xce => {
                    // SetColorRegister: 0 = foreground, 1 = background.
                    self.w32(DRAW + if cmd == 0 { 0x480 } else { 0x484 }, arg(1));
                    2
                }
                0xc1 | 0xc8 | 0xc9 | 0xcc => 2,
                0x41 | 0x44 => 5, // G_Viewport, G_ViewVolumeXYClip
                0x42 | 0x45 => 3, // G_DepthRange, G_ViewVolumeZClip
                0x46 => 2,
                0x43 => 17, // G_LoadMatrix
                0x09 => {
                    // DrawRectP: 0x41 = fill with the foreground colour.
                    let (x, y) = xy(arg(1));
                    let (w, h) = xy(arg(2));
                    if cmd == 0x41 {
                        self.fill_rect(x, y, w, h);
                    } else {
                        self.unknown_cmd(head);
                    }
                    3
                }
                0x04 | 0x05 => {
                    // Line from the start point registers to each vertex.
                    let (mut x0, mut y0) = ((self.dreg(0x540) >> 16) as i16 as i32, (self.dreg(0x544) >> 16) as i16 as i32);
                    if self.dreg(0x540) & 0xffff_0000 == 0 && self.dreg(0x540) != 0 {
                        // Integer rather than 16.16 fixed point.
                        x0 = self.dreg(0x540) as i32;
                        y0 = self.dreg(0x544) as i32;
                    }
                    for k in 0..param as usize {
                        let (x1, y1) = xy(arg(1 + k));
                        self.line(x0, y0, x1, y1);
                        x0 = x1;
                        y0 = y1;
                    }
                    1 + param as usize
                }
                0x0d => {
                    // BltCopyP within the current frame buffer.
                    let (sx, sy) = xy(arg(1));
                    let (dx, dy) = xy(arg(2));
                    let (w, h) = xy(arg(3));
                    let t = self.target();
                    self.blit(t.base, t.stride, sx, sy, t.base, t.stride, dx, dy, w, h, true);
                    4
                }
                0x0f | 0x1f => {
                    // BltCopyAlternateP: source and destination surfaces
                    // given explicitly.
                    let (sx, sy) = xy(arg(3));
                    let (dx, dy) = xy(arg(6));
                    let (w, h) = xy(arg(7));
                    self.blit(Self::vaddr(arg(1)), (arg(2) & 0xfff) as usize, sx, sy, Self::vaddr(arg(4)), (arg(5) & 0xfff) as usize, dx, dy, w, h, false);
                    8
                }
                0x0b => {
                    // DrawBitmapP: `param` words follow (position, size,
                    // pattern).  0x42 = colour pattern, 0x43 = 1 bpp
                    // pattern expanded with the fore/background colours.
                    let (x, y) = xy(arg(1));
                    let (w, h) = xy(arg(2));
                    let t = self.target();
                    let log = (self.dreg(0x430) >> 9) & 0xf;
                    if cmd == 0x43 {
                        let (fg, bg) = (self.dreg(0x480), self.dreg(0x484));
                        let opaque_bg = self.dreg(0x430) & 2 == 0;
                        let row_words = ((w + 31) / 32).max(0) as usize;
                        for j in 0..h {
                            for c in 0..w {
                                let word = arg(3 + j as usize * row_words + (c / 32) as usize);
                                let on = word & (0x8000_0000 >> (c % 32)) != 0;
                                if on {
                                    self.plot(&t, x + c, y + j, fg, log);
                                } else if opaque_bg {
                                    self.plot(&t, x + c, y + j, bg, log);
                                }
                            }
                        }
                    } else if cmd == 0x42 {
                        let per_word = if t.bpp16 { 2 } else { 4 };
                        let row_words = ((w + per_word - 1) / per_word).max(0) as usize;
                        for j in 0..h {
                            for c in 0..w {
                                let word = arg(3 + j as usize * row_words + (c / per_word) as usize);
                                let v = if t.bpp16 { (word >> ((c % 2) * 16)) & 0xffff } else { (word >> ((c % 4) * 8)) & 0xff };
                                self.plot(&t, x + c, y + j, v, log);
                            }
                        }
                    } else {
                        self.unknown_cmd(head);
                    }
                    1 + param as usize
                }
                _ => {
                    self.unknown_cmd(head);
                    ist |= IST_CERR & 0; // real hardware flags a command error; keep going quietly
                    break;
                }
            };
            i += len;
        }
        ist
    }

    #[cold]
    fn unknown_cmd(&mut self, head: u32) {
        let key = head >> 16;
        if !self.unknown.contains(&key) {
            self.unknown.push(key);
            eprintln!("[coral] display list command {:08x} (type {:02x}, command {:02x}) is not implemented", head, head >> 24, (head >> 16) & 0xff);
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

    /// Display enable: the legacy bit in DCE or the extended one in DCM1.
    pub fn display_enabled(&self) -> bool {
        self.r16(DCE) & 0x8000 != 0 || self.r32(DCM1) & 0x8000_0000 != 0
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

    pub fn layer_enabled(&self, i: usize) -> bool {
        let dce = self.r16(DCE);
        let ext = self.r32(DCM1) >> 16;
        dce & (1 << LAYERS[i].dce_bit) != 0 || ext & (1 << i) != 0
    }

    fn palette(&self, base: u32, idx: u32) -> (u8, u8, u8) {
        let e = self.r32(base + idx * 4);
        (((e >> 16) & 0xff) as u8, ((e >> 8) & 0xff) as u8, (e & 0xff) as u8)
    }

    /// Layers from back to front.  DLS holds the order as six nibbles,
    /// front-most first, 7 meaning "none"; it reads 0 until the firmware
    /// programs it, in which case the fixed legacy order applies.
    fn layer_order(&self) -> Vec<usize> {
        let dls = self.r32(DISP + 0x180);
        if dls == 0 {
            return vec![5, 4, 3, 2, 1, 0];
        }
        let mut order = Vec::new();
        for k in (0..6).rev() {
            let l = ((dls >> (k * 4)) & 0xf) as usize;
            if l < 6 && !order.contains(&l) {
                order.push(l);
            }
        }
        order
    }

    /// Screen rectangle of a layer: the whole screen, or its window when
    /// window mode is on (always for L1).
    fn layer_window(&self, i: usize, w: usize, h: usize) -> (usize, usize, usize, usize) {
        let ext = DISP + 0x110 + i as u32 * 0x10;
        let (wx, wy, ww, wh) = if i == 1 && self.r32(ext) & 2 == 0 {
            (self.r16(DISP + 0x18), self.r16(DISP + 0x1a), self.r16(DISP + 0x1c), self.r16(DISP + 0x1e) + 1)
        } else if self.r32(ext) & 2 != 0 {
            (self.r16(ext + 4), self.r16(ext + 6), self.r16(ext + 8), self.r16(ext + 10) + 1)
        } else {
            (0, 0, w as u32, h as u32)
        };
        let x0 = (wx as usize).min(w);
        let y0 = (wy as usize).min(h);
        (x0, y0, (x0 + ww as usize).min(w), (y0 + wh as usize).min(h))
    }

    /// Compose the enabled layers into an RGB888 image.
    pub fn render(&self) -> (usize, usize, Vec<u8>) {
        let (w, h) = self.resolution();
        let mut out = vec![0u8; w * h * 3];
        if !self.display_enabled() {
            return (w, h, out);
        }
        let bg = self.r32(DISP + 0x184);
        for px in out.chunks_exact_mut(3) {
            px[0] = (bg >> 16) as u8;
            px[1] = (bg >> 8) as u8;
            px[2] = bg as u8;
        }
        for i in self.layer_order() {
            if !self.layer_enabled(i) {
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
            // Transparency: a pixel equal to the layer's transparent
            // colour shows what is behind it.  Colour code 0 only counts
            // when the "zero transparent" bit is clear.
            let etc = self.r32(DISP + 0x1a0 + i as u32 * 4);
            let legacy_zt = match i {
                0 => self.r16(DISP + 0xbc) & 0x8000 != 0,
                2 => self.r16(DISP + 0xc2) & 0x8000 != 0,
                3 => self.r16(DISP + 0xc0) & 0x8000 != 0,
                _ => true,
            };
            let key = etc & 0x7fff;
            let zero_opaque = etc & 0x8000_0000 != 0 || (etc == 0 && legacy_zt);
            let transparent = |v: u32| v == key && (v != 0 || !zero_opaque);
            let (x0, y0, x1, y1) = self.layer_window(i, w, h);
            for y in y0..y1 {
                let row = da + (y - y0) * stride;
                for x in x0..x1 {
                    let o = (y * w + x) * 3;
                    if direct {
                        let p = (row + (x - x0) * 2) ^ 2;
                        if p + 1 >= VRAM_SIZE {
                            break;
                        }
                        let v = u16::from_be_bytes([self.vram[p], self.vram[p + 1]]) as u32 & 0x7fff;
                        if transparent(v) {
                            continue;
                        }
                        let (r, g, b) = ((v >> 10) as u8 & 0x1f, (v >> 5) as u8 & 0x1f, v as u8 & 0x1f);
                        out[o] = (r << 3) | (r >> 2);
                        out[o + 1] = (g << 3) | (g >> 2);
                        out[o + 2] = (b << 3) | (b >> 2);
                    } else {
                        let p = (row + (x - x0)) ^ 3;
                        if p >= VRAM_SIZE {
                            break;
                        }
                        let idx = self.vram[p] as u32;
                        if transparent(idx) {
                            continue;
                        }
                        let c = self.palette(l.palette, idx);
                        out[o] = c.0;
                        out[o + 1] = c.1;
                        out[o + 2] = c.2;
                    }
                }
            }
        }
        (w, h, out)
    }

    /// Render an arbitrary 16 bpp surface in video memory (for looking at
    /// off-screen buffers).  `stride` is in pixels.
    pub fn render_surface(&self, addr: u32, stride: usize, w: usize, h: usize) -> Vec<u8> {
        let base = Self::vaddr(addr);
        let mut out = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let v = self.px_read(base, stride, x as i32, y as i32, true);
                let (r, g, b) = ((v >> 10) as u8 & 0x1f, (v >> 5) as u8 & 0x1f, v as u8 & 0x1f);
                let o = (y * w + x) * 3;
                out[o] = (r << 3) | (r >> 2);
                out[o + 1] = (g << 3) | (g >> 2);
                out[o + 2] = (b << 3) | (b >> 2);
            }
        }
        out
    }

    /// Human-readable summary of the display configuration.
    pub fn describe(&self) -> String {
        let (w, h) = self.resolution();
        let mut s = format!(
"coral: display {} {}x{} DCM0={:04x} DCE={:04x} DCM1={:08x} DLS={:08x} IST={:02x} IMASK={:02x} frames={} display-lists={}\n",
            if self.display_enabled() { "on" } else { "off" },
            w,
            h,
            self.r16(DCM0),
            self.r16(DCE),
            self.r32(DCM1),
            self.r32(DISP + 0x180),
            self.r32(IST),
            self.r32(IMASK),
            self.frame,
            self.lists
        );
        let cmds: Vec<String> = self.cmd_counts.iter().enumerate().filter(|(_, &n)| n > 0).map(|(t, n)| format!("{:02x}:{}", t, n)).collect();
        if !cmds.is_empty() {
            s += &format!("  display list commands by type: {}\n", cmds.join(" "));
        }
        for (i, l) in LAYERS.iter().enumerate() {
            let mode = self.r32(l.mode);
            s += &format!(
                "  {:<6} {} mode={:08x} ({}bpp stride {} height {}) origin={:07x} disp={:07x} ext={:x} etc={:08x}\n",
                l.name,
                if self.layer_enabled(i) { "on " } else { "off" },
                mode,
                if mode >> 31 != 0 { 16 } else { 8 },
                ((mode >> 16) & 0xff) * 64,
                (mode & 0xfff) + 1,
                self.r32(l.origin),
                self.r32(l.disp_addr),
                self.r32(DISP + 0x110 + i as u32 * 0x10),
                self.r32(DISP + 0x1a0 + i as u32 * 4)
            );
        }
        s
    }
}
