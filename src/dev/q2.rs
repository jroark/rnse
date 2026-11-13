//! The 193's 2D graphics engine ("Q2" in the firmware's messages) at
//! 0xffe8_0000.
//!
//! The firmware builds display lists in RAM and has the engine run them;
//! everything on the screen is drawn this way.  The register layout is
//! the one of the drawing engine Renesas later called M2DG:
//!
//! | offset | register                                                    |
//! |--------|-------------------------------------------------------------|
//! | +0x00  | SCLR: bit 31 = reset, bit 0 = start running the list        |
//! | +0x04  | SR, status: bit 2 = list ended (TRAP), bit 1 = INT command, |
//! |        | bit 0 = command error                                       |
//! | +0x08  | SRCR: ones clear status bits                                |
//! | +0x0c  | IER: interrupt enable for the status bits                   |
//! | +0x48  | DLSAR: address of the display list                          |
//! | +0x4c  | SSAR: source surface                                        |
//! | +0x50  | RSAR: surface drawn to                                      |
//! | +0x54  | WSAR: work area                                             |
//! | +0x58  | SSTRR, +0x5c DSTRR: widths of the two surfaces in pixels    |
//! | +0x80  | STCR: source colour treated as transparent                  |
//! | +0x88  | ALPHR: alpha value                                          |
//! | +0xc0  | RCLR: rendering control                                     |
//! | +0xcc  | LCOR: offset added to coordinates (x << 16 | y)             |
//! | +0xd0  | SCLMAR: system clip, maximum (x << 16 | y)                  |
//! | +0xd4  | UCLMIR, +0xd8 UCLMAR: user clip, minimum and maximum        |
//!
//! Display list commands are 32-bit words, the operation in the top
//! byte and drawing mode bits below it (0x2000 = clip to the user
//! rectangle, 0x0800 = skip source pixels of the transparent colour):
//!
//! | word 0      | operands                                              |
//! |-------------|-------------------------------------------------------|
//! | 0x00000000  | TRAP: end of the list                                 |
//! | 0x10000000  | no operand; meaning unknown, skipped                  |
//! | 0x18000000  | WPR: (count - 1) << 16 | register, then the values    |
//! | 0x40000000  | LCOFS: new coordinate offset                          |
//! | 0xa0......  | BITBLTC: raster operation, colour, width - 1,         |
//! |             | height - 1, destination (x << 16 | y): fill           |
//! | 0xa2......  | BITBLTA: raster operation, byte offset into the       |
//! |             | source surface, width - 1, height - 1, destination:   |
//! |             | copy                                                  |
//! | 0xa8......  | colour, address of a coverage map (one byte per      |
//! |             | pixel), width - 1, height - 1, destination: blend the  |
//! |             | colour in by the map's values (antialiased text)       |
//! | 0xab......  | address and width of the 8-bit surface that holds the |
//! |             | alpha values of the surface drawn to, a zero word,     |
//! |             | width - 1, height - 1, position: precedes drawing      |
//! |             | that should respect and update those alpha values     |
//! | 0xaa......  | two zero words, width - 1, height - 1, position: ends |
//! |             | what 0xab began                                        |
//!
//! The firmware keeps an 8-bit surface of alpha values directly behind
//! each 16-bit surface of a blended display plane.  How exactly the
//! engine uses it between 0xab and 0xaa is a guess: text is blended
//! weighing the old pixel by its alpha and the alpha is raised to match;
//! copies in between are done as plain copies.
//!
//! Only what the firmware has been seen to use is implemented, for 16
//! bits per pixel; anything else ends the list and is counted in
//! [`Q2::unknown`].

use std::collections::BTreeMap;

const SCLR: usize = 0x00;
const SR: usize = 0x04;
const SRCR: usize = 0x08;
const IER: usize = 0x0c;
const DLSAR: usize = 0x48;
const SSAR: usize = 0x4c;
const RSAR: usize = 0x50;
const SSTRR: usize = 0x58;
const DSTRR: usize = 0x5c;
const STCR: usize = 0x80;
const RCLR: usize = 0xc0;
const LCOR: usize = 0xcc;
const SCLMAR: usize = 0xd0;
const UCLMIR: usize = 0xd4;
const UCLMAR: usize = 0xd8;

const SR_TRAP: u32 = 4;
const SR_ERROR: u32 = 1;

const MODE_CLIP: u32 = 0x2000;
const MODE_TRANSPARENT: u32 = 0x0800;

/// Longest list the engine follows before giving up on it.
const MAX_WORDS: u32 = 1 << 20;

pub struct Q2 {
    regs: [u32; 0x40],
    /// A list waits to be run (the firmware set the start bit).
    pub pending: bool,
    pub lists: u64,
    /// Command words the model does not know, with how often they came.
    pub unknown: BTreeMap<u32, u32>,
    pub trace: bool,
}

impl Default for Q2 {
    fn default() -> Q2 {
        Q2 { regs: [0; 0x40], pending: false, lists: 0, unknown: BTreeMap::new(), trace: false }
    }
}

/// Guest RAM as the engine sees it: physical addresses, big-endian.
struct Mem<'a> {
    ram: &'a mut [u8],
    base: u32,
}

impl Mem<'_> {
    fn index(&self, addr: u32, len: usize) -> Option<usize> {
        let o = (addr & 0x1fff_ffff).wrapping_sub(self.base) as usize;
        (o.checked_add(len)? <= self.ram.len()).then_some(o)
    }

    fn r32(&self, addr: u32) -> u32 {
        self.index(addr, 4).map_or(0, |o| u32::from_be_bytes([self.ram[o], self.ram[o + 1], self.ram[o + 2], self.ram[o + 3]]))
    }

    fn r16(&self, addr: u32) -> u16 {
        self.index(addr, 2).map_or(0, |o| u16::from_be_bytes([self.ram[o], self.ram[o + 1]]))
    }

    fn w16(&mut self, addr: u32, v: u16) {
        if let Some(o) = self.index(addr, 2) {
            self.ram[o..o + 2].copy_from_slice(&v.to_be_bytes());
        }
    }

    /// Pixel `n` of the surface at `addr`, `size` bytes per pixel.
    fn pixel(&self, addr: u32, n: i32, size: u32) -> u16 {
        let a = addr.wrapping_add((n as u32).wrapping_mul(size));
        match size {
            1 => self.index(a, 1).map_or(0, |o| self.ram[o] as u16),
            _ => self.r16(a),
        }
    }

    fn set_pixel(&mut self, addr: u32, n: i32, size: u32, v: u16) {
        let a = addr.wrapping_add((n as u32).wrapping_mul(size));
        match size {
            1 => {
                if let Some(o) = self.index(a, 1) {
                    self.ram[o] = v as u8;
                }
            }
            _ => self.w16(a, v),
        }
    }
}

fn xy(v: u32) -> (i32, i32) {
    ((v >> 16) as i16 as i32, v as i16 as i32)
}

impl Q2 {
    fn reg(&self, off: usize) -> u32 {
        self.regs[off >> 2]
    }

    /// Bytes per pixel of the surfaces: the firmware sets RCLR to zero
    /// for its 8-bit surfaces and to 0x0036_0000 for the 16-bit ones.
    fn pixel_size(&self) -> u32 {
        if self.reg(RCLR) == 0 { 1 } else { 2 }
    }

    /// Interrupt request line.
    pub fn irq(&self) -> bool {
        self.reg(SR) & self.reg(IER) & 7 != 0
    }

    pub fn read(&self, off: u32) -> u32 {
        self.regs.get((off >> 2) as usize).copied().unwrap_or(0)
    }

    pub fn write(&mut self, off: u32, v: u32) {
        let off = off as usize & !3;
        match off {
            SCLR => {
                if v & 0x8000_0000 != 0 {
                    self.pending = false;
                    self.regs[SR >> 2] = 0;
                }
                if v & 1 != 0 {
                    self.pending = true;
                }
                self.regs[SCLR >> 2] = v & !1;
            }
            SR => {}
            SRCR => self.regs[SR >> 2] &= !v,
            _ => {
                if let Some(r) = self.regs.get_mut(off >> 2) {
                    *r = v;
                }
            }
        }
    }

    /// The rectangle drawing is limited to: (x0, y0, x1, y1), inclusive.
    fn clip(&self, mode: u32) -> (i32, i32, i32, i32) {
        let (sx, sy) = xy(self.reg(SCLMAR));
        if mode & MODE_CLIP == 0 {
            return (0, 0, sx, sy);
        }
        let (x0, y0) = xy(self.reg(UCLMIR));
        let (x1, y1) = xy(self.reg(UCLMAR));
        (x0.max(0), y0.max(0), x1.min(sx), y1.min(sy))
    }

    /// Run the display list the firmware has started, drawing into
    /// `ram`, which begins at physical address `base`.
    pub fn run(&mut self, ram: &mut [u8], base: u32) {
        if !std::mem::take(&mut self.pending) {
            return;
        }
        self.lists += 1;
        let mut mem = Mem { ram, base };
        let mut pc = self.reg(DLSAR);
        let mut status = SR_TRAP;
        // Alpha surface (address, width) set by command 0xab.
        let mut alpha: Option<(u32, i32)> = None;
        for _ in 0..MAX_WORDS {
            let op = mem.r32(pc);
            let arg = |i: u32| mem.r32(pc + 4 * i);
            match op >> 24 {
                0x00 => break,
                0x10 => pc += 4,
                // A variant of WPR the firmware puts after 0xab, with
                // one value for 0xd8: not understood, skipped.
                0x18 if op & 3 != 0 => pc += 12,
                0x18 => {
                    let (count, reg) = ((arg(1) >> 16) + 1, arg(1) & 0xfffc);
                    for i in 0..count {
                        let v = arg(2 + i);
                        if let Some(r) = self.regs.get_mut(((reg + 4 * i) >> 2) as usize) {
                            *r = v;
                        }
                    }
                    pc += 4 * (2 + count);
                }
                0x40 => {
                    self.regs[LCOR >> 2] = arg(1);
                    pc += 8;
                }
                0xa0 | 0xa2 => {
                    let (w, h) = (arg(3) as i32 + 1, arg(4) as i32 + 1);
                    let (ox, oy) = xy(self.reg(LCOR));
                    let (dx, dy) = xy(arg(5));
                    let (dx, dy) = (dx + ox, dy + oy);
                    let (x0, y0, x1, y1) = self.clip(op);
                    let dst = self.reg(RSAR);
                    let dstride = self.reg(DSTRR) as i32;
                    let size = self.pixel_size();
                    let fill = arg(2) as u16;
                    let src = self.reg(SSAR).wrapping_add(arg(2));
                    let sstride = self.reg(SSTRR) as i32;
                    let key = (op & MODE_TRANSPARENT != 0).then_some(self.reg(STCR) as u16 & if size == 1 { 0xff } else { 0xffff });
                    if self.trace {
                        eprintln!("[q2] {:02x} {} {}x{} at {},{} mode {:06x} rop {:02x} operand {:08x} to {:08x}/{} RCLR {:08x} from {:08x}/{}", op >> 24, if op >> 24 == 0xa0 { "fill" } else { "copy" }, w, h, dx, dy, op & 0xff_ffff, arg(1), arg(2), dst, dstride, self.reg(RCLR), src, sstride);
                    }
                    for y in 0..h.min(4096) {
                        let ty = dy + y;
                        if ty < y0 || ty > y1 {
                            continue;
                        }
                        for x in 0..w.min(4096) {
                            let tx = dx + x;
                            if tx < x0 || tx > x1 {
                                continue;
                            }
                            let pixel = if op >> 24 == 0xa0 {
                                fill
                            } else {
                                let p = mem.pixel(src, y * sstride + x, size);
                                if key == Some(p) {
                                    continue;
                                }
                                p
                            };
                            mem.set_pixel(dst, ty * dstride + tx, size, pixel);
                        }
                    }
                    pc += 24;
                }
                // Names the 8-bit surface holding the alpha values of the
                // surface drawn to, for the rectangle about to be drawn.
                // Only the surface's address and width are used here.
                0xab => {
                    alpha = Some((arg(1), arg(2) as i32));
                    if self.trace {
                        let words: Vec<String> = (1..7).map(|i| format!("{:08x}", arg(i))).collect();
                        eprintln!("[q2] ab alpha surface {}", words.join(" "));
                    }
                    pc += 28;
                }
                // Ends what 0xab began (same rectangle).
                0xaa => {
                    alpha = None;
                    pc += 24;
                }
                0xa8 => {
                    // Blend a colour through a coverage map (text): one
                    // byte per pixel, rows as wide as the width given,
                    // which the firmware rounds up to a multiple of 16.
                    let colour = arg(1) as u16;
                    let map = arg(2);
                    let (w, h) = (arg(3) as i32 + 1, arg(4) as i32 + 1);
                    let (ox, oy) = xy(self.reg(LCOR));
                    let (dx, dy) = xy(arg(5));
                    let (dx, dy) = (dx + ox, dy + oy);
                    let (x0, y0, x1, y1) = self.clip(op);
                    let dst = self.reg(RSAR);
                    let dstride = self.reg(DSTRR) as i32;
                    let size = self.pixel_size();
                    if self.trace {
                        eprintln!("[q2] blend {}x{} at {},{} mode {:06x} colour {:04x} map {:08x} to {:08x} RCLR {:08x}", w, h, dx, dy, op & 0xff_ffff, colour, map, dst, self.reg(RCLR));
                    }
                    for y in 0..h.min(4096) {
                        let ty = dy + y;
                        if ty < y0 || ty > y1 {
                            continue;
                        }
                        for x in 0..w.min(4096) {
                            let tx = dx + x;
                            if tx < x0 || tx > x1 {
                                continue;
                            }
                            let cover = mem.pixel(map, y * w + x, 1) as u32;
                            if cover == 0 {
                                continue;
                            }
                            // What is there already counts by its own
                            // alpha, where the surface has an alpha map.
                            let under = match alpha {
                                Some((surface, stride)) => mem.pixel(surface, ty * stride + tx, 1) as u32,
                                None => 255,
                            };
                            let keep = under * (255 - cover) / 255;
                            let total = cover + keep;
                            let old = mem.pixel(dst, ty * dstride + tx, size) as u32;
                            let pixel = if size == 1 {
                                (colour as u32 * cover + old * keep) / total
                            } else {
                                (0..3).fold(0, |v, i| {
                                    let shift = 5 * i;
                                    let (c, o) = (colour as u32 >> shift & 31, old >> shift & 31);
                                    v | ((c * cover + o * keep + total / 2) / total) << shift
                                })
                            };
                            mem.set_pixel(dst, ty * dstride + tx, size, pixel as u16);
                            if let Some((surface, stride)) = alpha {
                                mem.set_pixel(surface, ty * stride + tx, 1, total.min(255) as u16);
                            }
                        }
                    }
                    pc += 24;
                }
                _ => {
                    *self.unknown.entry(op).or_insert(0) += 1;
                    if self.trace {
                        let words: Vec<String> = (0..8).map(|i| format!("{:08x}", arg(i))).collect();
                        eprintln!("[q2] unknown command at {:08x}: {}", pc, words.join(" "));
                    }
                    status |= SR_ERROR;
                    break;
                }
            }
        }
        self.regs[SR >> 2] |= status;
    }

    pub fn describe(&self) -> String {
        let unknown: Vec<String> = self.unknown.iter().map(|(op, n)| format!("{:08x}x{}", op, n)).collect();
        format!(
            "graphics engine: {} lists run, target {:08x} stride {}, source {:08x} stride {}, SR={:x} IER={:x}{}",
            self.lists,
            self.reg(RSAR),
            self.reg(DSTRR),
            self.reg(SSAR),
            self.reg(SSTRR),
            self.reg(SR),
            self.reg(IER),
            if unknown.is_empty() { String::new() } else { format!(", unknown commands: {}", unknown.join(" ")) }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x0800_0000;

    fn put(ram: &mut [u8], addr: u32, words: &[u32]) {
        for (i, w) in words.iter().enumerate() {
            let o = (addr - BASE) as usize + 4 * i;
            ram[o..o + 4].copy_from_slice(&w.to_be_bytes());
        }
    }

    fn pixel(ram: &[u8], surface: u32, stride: u32, x: u32, y: u32) -> u16 {
        let o = (surface - BASE + (y * stride + x) * 2) as usize;
        u16::from_be_bytes([ram[o], ram[o + 1]])
    }

    #[test]
    fn fill_copy_and_finish() {
        let mut ram = vec![0u8; 0x10000];
        let (list, src, dst) = (BASE + 0x100, BASE + 0x1000, BASE + 0x2000);
        // A 2x2 source with one transparent pixel.
        for (i, p) in [0x1111u16, 0x7c1f, 0x3333, 0x4444].iter().enumerate() {
            let o = (src - BASE) as usize + i * 2;
            ram[o..o + 2].copy_from_slice(&p.to_be_bytes());
        }
        put(
            &mut ram,
            list,
            &[
                0x1800_0000, 0x0001_004c, src, dst, // SSAR, RSAR
                0x1800_0000, 0x0001_0058, 2, 16, // strides
                0x1800_0000, 0x0000_0080, 0x7c1f, // transparent colour
                0x1800_0000, 0x0000_00c0, 0x0036_0000, // 16 bits per pixel
                0x1800_0000, 0x0002_00d0, 0x000f_000f, 0x0001_0001, 0x0004_0004, // clips
                0x4000_0000, 0x0001_0000, // offset x by 1
                0xa000_2000, 0xcc, 0xabcd, 7, 7, 0, // fill 8x8 at 0,0, clipped
                0xa200_2900, 0xcc, 0, 1, 1, 0x0001_0002, // copy 2x2 to 1,2
                0,
            ],
        );
        let mut q = Q2::default();
        q.write(0x0c, 7);
        q.write(0x48, list);
        q.write(0x00, 1);
        assert!(q.pending && !q.irq());
        q.run(&mut ram, BASE);
        assert!(q.irq(), "the end of the list raises the interrupt");
        // The fill starts at x = 1 (offset) and stops at the clip, 4.
        assert_eq!(pixel(&ram, dst, 16, 0, 1), 0);
        assert_eq!(pixel(&ram, dst, 16, 1, 1), 0xabcd);
        assert_eq!(pixel(&ram, dst, 16, 4, 4), 0xabcd);
        assert_eq!(pixel(&ram, dst, 16, 5, 4), 0);
        assert_eq!(pixel(&ram, dst, 16, 1, 0), 0, "above the clip");
        // The copy lands at (1 + 1, 2); its transparent pixel leaves the fill.
        assert_eq!(pixel(&ram, dst, 16, 2, 2), 0x1111);
        assert_eq!(pixel(&ram, dst, 16, 3, 2), 0xabcd);
        assert_eq!(pixel(&ram, dst, 16, 2, 3), 0x3333);
        assert_eq!(pixel(&ram, dst, 16, 3, 3), 0x4444);
        q.write(0x08, 7);
        assert!(!q.irq());
        assert!(q.unknown.is_empty());
    }

    #[test]
    fn text_blends_and_raises_alpha() {
        let mut ram = vec![0u8; 0x10000];
        let (list, map, dst, alpha) = (BASE + 0x100, BASE + 0x1000, BASE + 0x2000, BASE + 0x3000);
        // Coverage of a 16x1 map: nothing, half, full.
        ram[(map - BASE) as usize..][..3].copy_from_slice(&[0, 128, 255]);
        // The surface: blue everywhere, transparent (alpha 0) at x = 1.
        for x in 0..16 {
            ram[(dst - BASE) as usize + 2 * x + 1] = 0x1f;
            ram[(alpha - BASE) as usize + x] = if x == 1 { 0 } else { 255 };
        }
        put(
            &mut ram,
            list,
            &[
                0x1800_0000, 0x0000_0050, dst,
                0x1800_0000, 0x0000_005c, 16,
                0x1800_0000, 0x0000_00c0, 0x0036_0000,
                0x1800_0000, 0x0002_00d0, 0x000f_000f, 0, 0x000f_000f,
                0xab00_2100, alpha, 16, 0, 15, 0, 0,
                0xa800_2000, 0x7c00, map, 15, 0, 0,
                0xa800_2000, 0x7c00, map, 15, 0, 0x0003_0000, // again at x = 3
                0,
            ],
        );
        let mut q = Q2::default();
        q.write(0x48, list);
        q.write(0x00, 1);
        q.run(&mut ram, BASE);
        assert!(q.unknown.is_empty());
        assert_eq!(pixel(&ram, dst, 16, 0, 0), 0x001f, "no coverage");
        assert_eq!(pixel(&ram, dst, 16, 1, 0), 0x7c00, "nothing visible underneath: the colour itself");
        assert_eq!(ram[(alpha - BASE) as usize + 1], 128);
        assert_eq!(pixel(&ram, dst, 16, 2, 0), 0x7c00, "full coverage");
        assert_eq!(pixel(&ram, dst, 16, 4, 0), 0x400f, "half coverage over opaque blue");
        assert_eq!(ram[(alpha - BASE) as usize + 4], 255);
    }

    #[test]
    fn eight_bit_surfaces() {
        let mut ram = vec![0u8; 0x10000];
        let (list, dst) = (BASE + 0x100, BASE + 0x2000);
        put(&mut ram, list, &[0x1800_0000, 0x0000_0050, dst, 0x1800_0000, 0x0000_005c, 4, 0x1800_0000, 0x0000_00d0, 0x0003_0003, 0xa000_0000, 0xcc, 0xffff, 3, 3, 0, 0]);
        let mut q = Q2::default();
        q.write(0x48, list);
        q.write(0x00, 1);
        q.run(&mut ram, BASE);
        let o = (dst - BASE) as usize;
        assert!(ram[o..o + 16].iter().all(|&b| b == 0xff));
        assert!(ram[o + 16..o + 64].iter().all(|&b| b == 0), "one byte per pixel: nothing beyond the 4x4 surface");
    }
}
