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
//! | +0x64  | address of the source's alpha values, one byte per pixel    |
//! | +0x68  | width of that map in pixels                                 |
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
//! | 0x08000000  | NOP                                                   |
//! | 0x10000000  | no operand; meaning unknown, skipped                  |
//! | 0x18000000  | WPR: (count - 1) << 16 | register, then the values    |
//! | 0x18000003  | register, value: write the upper half only (seen for   |
//! |             | the user clip's right edge)                            |
//! | 0x40000000  | LCOFS: new coordinate offset                          |
//! | 0x28000000  | JUMP: address the list continues at                   |
//! | 0x80......  | POLYGON4C: colour, four corners: fill the quadrangle; |
//! |             | with mode bit 0x0200 only where the work buffer is set |
//! | 0xb0......  | LINEC: colour << 16 | number of points, line width in |
//! |             | pixels, the points (x << 16 | y): a polyline.  With    |
//! |             | mode bit 0x0400 the address of the points instead      |
//! | 0xb1......  | two colours, address of a bit pattern, its length in  |
//! |             | bits << 16 | 1, first bit used << 16 | number of      |
//! |             | points, line width, the points: a polyline in the two  |
//! |             | colours by the pattern                                 |
//! | 0xd0......  | number of points, two corners of the bounding box,    |
//! |             | address of the points: set the polygon's area in the   |
//! |             | work buffer                                            |
//! | 0xe0......  | two corners: clear that rectangle of the work buffer   |
//! | 0x82......  | source point, source size (both x << 16 | y), a zero  |
//! |             | word, four corners: stretch that part of the source   |
//! |             | surface over the rectangle between the corners        |
//! | 0xa0......  | BITBLTC: raster operation, colour, width - 1,         |
//! |             | height - 1, destination (x << 16 | y): fill           |
//! | 0xa2......  | BITBLTA: raster operation, point of the source        |
//! |             | surface (x << 16 | y), width - 1, height - 1,         |
//! |             | destination: copy.  With mode bit 0x0400 blended by   |
//! |             | the source's alpha map (+0x64)                         |
//! | 0xa8......  | colour, address of a coverage map (one byte per      |
//! |             | pixel), width - 1, height - 1, destination: blend the  |
//! |             | colour in by the map's values (antialiased text)       |
//! | 0xab......  | address and width of the 8-bit surface that holds the |
//! |             | alpha values of the surface drawn to, a point of the   |
//! |             | source's alpha map (+0x64), width - 1, height - 1,     |
//! |             | position: lay the map over those alpha values.  The    |
//! |             | drawing itself follows, possibly in a later list       |
//! | 0xaa......  | two zero words, width - 1, height - 1, position: ends |
//! |             | what 0xab began                                        |
//!
//! Filled shapes of the map (areas, and the round joints of roads) are
//! drawn in two steps, as on Hitachi's older Q2 chips: the shape goes
//! into the work buffer, one bit per pixel, and a POLYGON4C over its
//! bounding box then paints the colour where bits are set.  The work
//! buffer is kept inside the model, not in the guest's memory at WSAR.
//! Lines are drawn without antialiasing (mode bit 2 presumably asks for
//! it).
//!
//! The firmware keeps an 8-bit surface of alpha values directly behind
//! each 16-bit surface of a blended display plane.  For text and for
//! pictures that bring an alpha map it first has 0xab combine the new
//! coverage with those values and then draws; how the engine weighs the
//! colours then is a guess: here the new colour counts by its coverage
//! and what was there by the rest of the combined alpha, so that
//! nothing of an invisible pixel's colour shows.  The model remembers
//! the alpha surface 0xab named until 0xaa, also from one list to the
//! next.
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
const SASAR: usize = 0x64;
const SASTRR: usize = 0x68;
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
const MODE_WORK: u32 = 0x0200;
const MODE_INDIRECT: u32 = 0x0400;
/// On a copy: the source has an alpha map.
const MODE_SOURCE_ALPHA: u32 = 0x0400;

/// Size of the work buffer in pixels, each way.
const WORK: i32 = 1024;

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
    /// The work buffer: one entry per pixel, `WORK` pixels each way.
    work: Vec<bool>,
    /// Alpha surface (address, width) named by command 0xab.  It stays
    /// in force across lists: the firmware sometimes sends the 0xab
    /// commands of several pieces of text in one list and their blends
    /// in the next.
    alpha: Option<(u32, i32)>,
}

impl Default for Q2 {
    fn default() -> Q2 {
        Q2 { regs: [0; 0x40], pending: false, lists: 0, unknown: BTreeMap::new(), trace: false, work: Vec::new(), alpha: None }
    }
}

/// Call `put` for every pixel of the line from `a` to `b`.
fn line(a: (i32, i32), b: (i32, i32), put: &mut dyn FnMut(i32, i32)) {
    let (dx, dy) = ((b.0 - a.0).abs(), -(b.1 - a.1).abs());
    let (sx, sy) = (if a.0 < b.0 { 1 } else { -1 }, if a.1 < b.1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (a.0, a.1, dx + dy);
    for _ in 0..8192 {
        put(x, y);
        if (x, y) == b {
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

/// Call `put` for every pixel of a polygon, outline included (even-odd
/// rule inside).
fn polygon(points: &[(i32, i32)], put: &mut dyn FnMut(i32, i32)) {
    let n = points.len();
    if n == 0 {
        return;
    }
    let top = points.iter().map(|p| p.1).min().unwrap().max(-WORK);
    let bottom = points.iter().map(|p| p.1).max().unwrap().min(2 * WORK);
    let mut xs: Vec<i32> = Vec::new();
    for y in top..=bottom {
        xs.clear();
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (a, b) = if a.1 <= b.1 { (a, b) } else { (b, a) };
            if y >= a.1 && y < b.1 {
                // Rounded to the nearest pixel.
                let num = (b.0 - a.0) as i64 * (y - a.1) as i64 * 2 + (b.1 - a.1) as i64;
                xs.push(a.0 + num.div_euclid(2 * (b.1 - a.1) as i64) as i32);
            }
        }
        xs.sort_unstable();
        for pair in xs.chunks_exact(2) {
            for x in pair[0].max(-WORK)..=pair[1].min(2 * WORK) {
                put(x, y);
            }
        }
    }
    for i in 0..n {
        line(points[i], points[(i + 1) % n], put);
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
        let mut alpha = self.alpha;
        for _ in 0..MAX_WORDS {
            let op = mem.r32(pc);
            let arg = |i: u32| mem.r32(pc + 4 * i);
            match op >> 24 {
                0x00 => break,
                0x10 => pc += 4,
                // WPR of half a register: the firmware uses it (low bits
                // 3) to move the right edge of the user clip, in the upper
                // half of 0xd8, for a picture whose rows are longer than
                // it is wide, and back again.
                0x18 if op & 3 != 0 => {
                    let reg = (arg(1) & 0xfffc) as usize >> 2;
                    if let Some(r) = self.regs.get_mut(reg) {
                        *r = match op & 3 {
                            3 => (*r & 0x0000_ffff) | (arg(2) & 0xffff_0000),
                            _ => *r,
                        };
                    }
                    pc += 12;
                }
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
                0x28 => pc = arg(1),
                0x08 => pc += 4,
                // A polyline in two colours after a bit pattern (railway
                // lines on the map).  Which colour the set bits take is
                // a guess.
                0xb1 => {
                    let (ox, oy) = xy(self.reg(LCOR));
                    let colours = [(arg(1) >> 16) as u16, arg(1) as u16];
                    let pattern = arg(2);
                    let bits = (arg(3) >> 16).clamp(1, 1024);
                    let count = (arg(4) & 0xffff).min(4096);
                    let width = (arg(5) as i32).clamp(1, 64);
                    let points: Vec<(i32, i32)> = (0..count).map(|i| xy(arg(6 + i))).map(|(x, y)| (x + ox, y + oy)).collect();
                    let (x0, y0, x1, y1) = self.clip(op);
                    let dst = self.reg(RSAR);
                    let dstride = self.reg(DSTRR) as i32;
                    let size = self.pixel_size();
                    if self.trace {
                        eprintln!("[q2] b1 patterned line of {} points, width {}, colours {:04x} {:04x} mode {:06x}", count, width, colours[0], colours[1], op & 0xff_ffff);
                    }
                    // Where in the pattern the line begins.
                    let mut along = arg(4) >> 16;
                    for pair in points.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        let mut steps: Vec<(i32, i32)> = Vec::new();
                        line(a, b, &mut |x, y| steps.push((x, y)));
                        for (x, y) in steps {
                            let bit = along % bits;
                            along += 1;
                            let set = mem.r32(pattern + 4 * (bit / 32)) >> (31 - bit % 32) & 1 != 0;
                            let colour = colours[if set { 0 } else { 1 }];
                            for py in y - (width - 1) / 2..=y + width / 2 {
                                for px in x - (width - 1) / 2..=x + width / 2 {
                                    if px >= x0 && px <= x1 && py >= y0 && py <= y1 {
                                        mem.set_pixel(dst, py * dstride + px, size, colour);
                                    }
                                }
                            }
                        }
                    }
                    pc += 4 * (6 + count);
                }
                // Clear a rectangle of the work buffer.
                0xe0 => {
                    let (ox, oy) = xy(self.reg(LCOR));
                    let ((ax, ay), (bx, by)) = (xy(arg(1)), xy(arg(2)));
                    if !self.work.is_empty() {
                        for y in (ay + oy).max(0)..=(by + oy).min(WORK - 1) {
                            for x in (ax + ox).max(0)..=(bx + ox).min(WORK - 1) {
                                self.work[(y * WORK + x) as usize] = false;
                            }
                        }
                    }
                    pc += 12;
                }
                // A polygon into the work buffer.
                0xd0 => {
                    let (ox, oy) = xy(self.reg(LCOR));
                    let count = arg(1).min(4096);
                    let table = arg(4);
                    let points: Vec<(i32, i32)> = (0..count).map(|i| xy(mem.r32(table + 4 * i))).map(|(x, y)| (x + ox, y + oy)).collect();
                    if self.trace {
                        eprintln!("[q2] d0 polygon of {} points into the work buffer, mode {:06x}", count, op & 0xff_ffff);
                    }
                    if self.work.is_empty() {
                        self.work = vec![false; (WORK * WORK) as usize];
                    }
                    let work = &mut self.work;
                    polygon(&points, &mut |x, y| {
                        if (0..WORK).contains(&x) && (0..WORK).contains(&y) {
                            work[(y * WORK + x) as usize] = true;
                        }
                    });
                    pc += 20;
                }
                // Fill a quadrangle, or the part of it the work buffer marks.
                0x80 => {
                    let (ox, oy) = xy(self.reg(LCOR));
                    let colour = arg(1) as u16;
                    let points: Vec<(i32, i32)> = (2..6).map(|i| xy(arg(i))).map(|(x, y)| (x + ox, y + oy)).collect();
                    let (x0, y0, x1, y1) = self.clip(op);
                    let dst = self.reg(RSAR);
                    let dstride = self.reg(DSTRR) as i32;
                    let size = self.pixel_size();
                    let masked = op & MODE_WORK != 0;
                    if self.trace {
                        eprintln!("[q2] 80 quadrangle {:?} colour {:04x} mode {:06x} to {:08x}/{}", points, colour, op & 0xff_ffff, dst, dstride);
                    }
                    let work = &self.work;
                    polygon(&points, &mut |x, y| {
                        if x < x0 || x > x1 || y < y0 || y > y1 {
                            return;
                        }
                        if masked && !((0..WORK).contains(&x) && (0..WORK).contains(&y) && work.get((y * WORK + x) as usize) == Some(&true)) {
                            return;
                        }
                        mem.set_pixel(dst, y * dstride + x, size, colour);
                    });
                    pc += 24;
                }
                // A polyline of a given width.
                0xb0 => {
                    let (ox, oy) = xy(self.reg(LCOR));
                    let colour = (arg(1) >> 16) as u16;
                    let count = (arg(1) & 0xffff).min(4096);
                    let width = (arg(2) as i32).clamp(1, 64);
                    // The points follow, or (mode bit 0x400) their address does.
                    let indirect = op & MODE_INDIRECT != 0;
                    let table = if indirect { arg(3) } else { pc + 12 };
                    let points: Vec<(i32, i32)> = (0..count).map(|i| xy(mem.r32(table + 4 * i))).map(|(x, y)| (x + ox, y + oy)).collect();
                    let (x0, y0, x1, y1) = self.clip(op);
                    let dst = self.reg(RSAR);
                    let dstride = self.reg(DSTRR) as i32;
                    let size = self.pixel_size();
                    if self.trace {
                        eprintln!("[q2] b0 line of {} points, width {}, colour {:04x} mode {:06x} to {:08x}/{}", count, width, colour, op & 0xff_ffff, dst, dstride);
                    }
                    let mut put = |x: i32, y: i32| {
                        if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
                            mem.set_pixel(dst, y * dstride + x, size, colour);
                        }
                    };
                    for pair in points.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        if width <= 1 {
                            line(a, b, &mut put);
                            continue;
                        }
                        // A rectangle around the segment: half the width
                        // to either side.
                        let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
                        let len = (dx * dx + dy * dy).sqrt();
                        if len == 0.0 {
                            continue;
                        }
                        let half = (width - 1) as f64 / 2.0;
                        let (nx, ny) = (-dy / len * half, dx / len * half);
                        let corner = |p: (i32, i32), s: f64| ((p.0 as f64 + s * nx).round() as i32, (p.1 as f64 + s * ny).round() as i32);
                        polygon(&[corner(a, 1.0), corner(b, 1.0), corner(b, -1.0), corner(a, -1.0)], &mut put);
                    }
                    pc += if indirect { 16 } else { 4 * (3 + count) };
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
                    // The copy's source: a point of the source surface.
                    let src = self.reg(SSAR);
                    let (sx, sy) = xy(arg(2));
                    let sstride = self.reg(SSTRR) as i32;
                    let key = (op & MODE_TRANSPARENT != 0).then_some(self.reg(STCR) as u16 & if size == 1 { 0xff } else { 0xffff });
                    // A copy can come with a map of alpha values for
                    // the source, one byte per pixel.
                    let source_alpha = (op >> 24 == 0xa2 && op & MODE_SOURCE_ALPHA != 0 && size == 2).then(|| (self.reg(SASAR), self.reg(SASTRR) as i32));
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
                                let p = mem.pixel(src, (sy + y) * sstride + sx + x, size);
                                if key == Some(p) {
                                    continue;
                                }
                                if let Some((map, stride)) = source_alpha {
                                    // Blended over what is there, as text is.
                                    let cover = mem.pixel(map, (sy + y) * stride + sx + x, 1) as u32;
                                    if cover == 0 {
                                        continue;
                                    }
                                    let total = match alpha {
                                        Some((surface, stride)) => (mem.pixel(surface, ty * stride + tx, 1) as u32).max(cover),
                                        None => 255,
                                    };
                                    let keep = total - cover;
                                    let old = mem.pixel(dst, ty * dstride + tx, size) as u32;
                                    let mixed = (0..3).fold(0, |v, i| {
                                        let shift = 5 * i;
                                        let (c, o) = (p as u32 >> shift & 31, old >> shift & 31);
                                        v | ((c * cover + o * keep + total / 2) / total) << shift
                                    });
                                    mem.set_pixel(dst, ty * dstride + tx, size, mixed as u16);
                                    continue;
                                }
                                p
                            };
                            mem.set_pixel(dst, ty * dstride + tx, size, pixel);
                        }
                    }
                    pc += 24;
                }
                // Stretch a rectangle of the source surface over a
                // rectangle of the target.  The command gives four
                // corners; only upright rectangles are handled.
                0x82 => {
                    let (sx, sy) = xy(arg(1));
                    let (sw, sh) = xy(arg(2));
                    let (ox, oy) = xy(self.reg(LCOR));
                    let corners: Vec<(i32, i32)> = (4..8).map(|i| xy(arg(i))).collect();
                    let (left, top) = (corners.iter().map(|c| c.0).min().unwrap() + ox, corners.iter().map(|c| c.1).min().unwrap() + oy);
                    let (right, bottom) = (corners.iter().map(|c| c.0).max().unwrap() + ox, corners.iter().map(|c| c.1).max().unwrap() + oy);
                    let (w, h) = (right - left + 1, bottom - top + 1);
                    let (x0, y0, x1, y1) = self.clip(op);
                    let dst = self.reg(RSAR);
                    let dstride = self.reg(DSTRR) as i32;
                    let size = self.pixel_size();
                    let src = self.reg(SSAR);
                    let sstride = self.reg(SSTRR) as i32;
                    let key = (op & MODE_TRANSPARENT != 0).then_some(self.reg(STCR) as u16 & if size == 1 { 0xff } else { 0xffff });
                    if self.trace {
                        eprintln!("[q2] stretch {}x{} from {},{} to {}x{} at {},{} mode {:06x} to {:08x}/{} RCLR {:08x} from {:08x}/{}", sw, sh, sx, sy, w, h, left, top, op & 0xff_ffff, dst, dstride, self.reg(RCLR), src, sstride);
                    }
                    if sw > 0 && sh > 0 {
                        for y in 0..h.min(4096) {
                            let ty = top + y;
                            if ty < y0 || ty > y1 {
                                continue;
                            }
                            let v = sy + y * sh / h;
                            for x in 0..w.min(4096) {
                                let tx = left + x;
                                if tx < x0 || tx > x1 {
                                    continue;
                                }
                                let p = mem.pixel(src, v * sstride + sx + x * sw / w, size);
                                if key != Some(p) {
                                    mem.set_pixel(dst, ty * dstride + tx, size, p);
                                }
                            }
                        }
                    }
                    pc += 32;
                }
                // Lays the alpha map of what is about to be drawn (+0x64)
                // over the alpha values of the surface drawn to, which are
                // in a surface of their own: address, width, a point of
                // the map, size and position of the rectangle.
                0xab => {
                    let (surface, stride) = (arg(1), arg(2) as i32);
                    let (sx, sy) = xy(arg(3));
                    let (w, h) = (arg(4) as i32 + 1, arg(5) as i32 + 1);
                    let (ox, oy) = xy(self.reg(LCOR));
                    let (dx, dy) = xy(arg(6));
                    let (dx, dy) = (dx + ox, dy + oy);
                    let (x0, y0, x1, y1) = self.clip(op);
                    let (map, mstride) = (self.reg(SASAR), self.reg(SASTRR) as i32);
                    if self.trace {
                        eprintln!("[q2] ab alpha {}x{} at {},{} from {:08x}/{} to {:08x}/{}", w, h, dx, dy, map, mstride, surface, stride);
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
                            let cover = mem.pixel(map, (sy + y) * mstride + sx + x, 1) as u32;
                            if cover != 0 {
                                let under = mem.pixel(surface, ty * stride + tx, 1) as u32;
                                mem.set_pixel(surface, ty * stride + tx, 1, (cover + under * (255 - cover) / 255).min(255) as u16);
                            }
                        }
                    }
                    alpha = Some((surface, stride));
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
                            // Where the surface has alpha values, 0xab has
                            // already laid this coverage over them: what
                            // was there before counts by the rest.
                            let total = match alpha {
                                Some((surface, stride)) => (mem.pixel(surface, ty * stride + tx, 1) as u32).max(cover),
                                None => 255,
                            };
                            let keep = total - cover;
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
                        }
                    }
                    pc += 24;
                }
                _ => {
                    *self.unknown.entry(op).or_insert(0) += 1;
                    if self.trace {
                        let words: Vec<String> = (0..8).map(|i| format!("{:08x}", arg(i))).collect();
                        eprintln!("[q2] unknown command at {:08x} of the list at {:08x}: {}", pc, self.reg(DLSAR), words.join(" "));
                    }
                    status |= SR_ERROR;
                    break;
                }
            }
        }
        self.alpha = alpha;
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
                0x1800_0000, 0x0001_0064, map, 16, // the coverage is the alpha map
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

    /// What the map is drawn with: lines of a width, and areas that go
    /// through the work buffer.
    #[test]
    fn lines_and_areas() {
        let mut ram = vec![0u8; 0x10000];
        let (list, dst) = (BASE + 0x100, BASE + 0x2000);
        let points = BASE + 0x800;
        put(&mut ram, points, &[0x0002_0008, 0x0008_0008, 0x0008_000d, 0x0002_000d, 0x0002_0008]);
        put(
            &mut ram,
            list,
            &[
                0x1800_0000, 0x0000_0050, dst,
                0x1800_0000, 0x0000_005c, 32,
                0x1800_0000, 0x0000_00c0, 0x0036_0000,
                0x1800_0000, 0x0002_00d0, 0x001f_001f, 0, 0x001f_001f,
                // A line three pixels wide from (2,3) to (12,3), then one
                // pixel wide down to (12,6), the points given in place.
                0xb000_2002, 0x1111_0002, 3, 0x0002_0003, 0x000c_0003,
                0xb000_2002, 0x2222_0002, 1, 0x000c_0003, 0x000c_0006,
                // A square into the work buffer, its points elsewhere,
                // painted through a quadrangle over more than its box.
                0xe000_2020, 0x0000_0006, 0x000a_000f,
                0xd000_2438, 5, 0x0002_0008, 0x0008_000d, points,
                0x2800_0000, list + 4 * 35, // over a word that is not a command
                0xffff_ffff,
                0x8000_2200, 0x3333, 0x0000_0006, 0x000a_0006, 0x000a_000f, 0x0000_000f,
                // The outline again, as a line with its points by address.
                0xb000_2402, 0x4444_0005, 0, points,
                // A plain quadrangle.
                0x8000_2000, 0x5555, 0x0014_0014, 0x0016_0014, 0x0016_0016, 0x0014_0016,
                0,
            ],
        );
        let mut q = Q2::default();
        q.write(0x48, list);
        q.write(0x00, 1);
        q.run(&mut ram, BASE);
        assert!(q.unknown.is_empty(), "{:x?}", q.unknown);
        let at = |x, y| pixel(&ram, dst, 32, x, y);
        // The wide line covers its row and one to either side.
        assert_eq!([at(5, 1), at(5, 2), at(5, 3), at(5, 4), at(5, 5)], [0, 0x1111, 0x1111, 0x1111, 0]);
        assert_eq!([at(12, 5), at(11, 5), at(13, 5)], [0x2222, 0, 0]);
        // The area: filled inside, outlined, nothing in the rest of the
        // quadrangle that painted it.
        assert_eq!(at(5, 10), 0x3333);
        assert_eq!([at(2, 8), at(8, 13), at(5, 8)], [0x4444; 3]);
        assert_eq!([at(1, 10), at(9, 10), at(5, 7), at(5, 14)], [0; 4]);
        assert_eq!([at(20, 20), at(22, 22), at(21, 21), at(23, 21)], [0x5555, 0x5555, 0x5555, 0]);
    }

    /// A picture with an alpha map of its own, laid over a surface that
    /// has alpha values: 0xab combines the alpha, the copy blends.
    #[test]
    fn pictures_with_alpha() {
        let mut ram = vec![0u8; 0x10000];
        let (list, src, map, dst, alpha) = (BASE + 0x100, BASE + 0x1000, BASE + 0x1800, BASE + 0x2000, BASE + 0x3000);
        // Three red pixels: invisible, half and fully opaque.
        for x in 0..3 {
            ram[(src - BASE) as usize + 2 * x] = 0x7c;
        }
        ram[(map - BASE) as usize..][..3].copy_from_slice(&[0, 128, 255]);
        // The surface: blue, opaque except at x = 1 of the second run.
        for x in 0..16 {
            ram[(dst - BASE) as usize + 2 * x + 1] = 0x1f;
            ram[(alpha - BASE) as usize + x] = if x == 9 { 0 } else { 255 };
        }
        let copy = |at: u32| [0xab00_2100, alpha, 16, 0, 2, 0, at, 0xa200_2d00, 0xcc, 0, 2, 0, at];
        let mut words = vec![
            0x1800_0000, 0x0001_004c, src, dst,
            0x1800_0000, 0x0001_0058, 4, 16,
            0x1800_0000, 0x0001_0064, map, 4,
            0x1800_0000, 0x0000_0080, 0x7c1f,
            0x1800_0000, 0x0000_00c0, 0x0036_0000,
            0x1800_0000, 0x0002_00d0, 0x000f_000f, 0, 0x000f_000f,
        ];
        words.extend(copy(0));
        words.extend(copy(0x0008_0000));
        words.push(0);
        put(&mut ram, list, &words);
        let mut q = Q2::default();
        q.write(0x48, list);
        q.write(0x00, 1);
        q.run(&mut ram, BASE);
        assert!(q.unknown.is_empty(), "{:x?}", q.unknown);
        let at = |x| pixel(&ram, dst, 16, x, 0);
        assert_eq!([at(0), at(1), at(2)], [0x001f, 0x400f, 0x7c00], "nothing, half and all of the picture over blue");
        assert_eq!(at(9), 0x7c00, "over an invisible pixel only the picture's colour counts");
        let a = (alpha - BASE) as usize;
        assert_eq!([ram[a + 1], ram[a + 8], ram[a + 9], ram[a + 10]], [255, 255, 128, 255]);
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
