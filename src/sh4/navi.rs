//! On-chip peripherals of the SH7770-family system-on-chip in the 193
//! unit.
//!
//! Renesas never published a manual for these parts, so everything here
//! comes from how the firmware uses the registers, cross-checked against
//! the few public sources (the Linux kernel's SH7770 support, and later
//! Renesas chips that inherited modules such as the display unit).
//!
//! The CPU core registers (0xff00_00xx), the first timer unit
//! (0xffd8_0000, same layout as the SH7750's) and the interrupt
//! controller (0xff80_4000, in `onchip.rs`) are handled by the SH-4
//! model; every other P4 address comes here.  Registers without a model
//! behave like memory so that read-back works.
//!
//! | address      | module                                             |
//! |--------------|----------------------------------------------------|
//! | 0xff80_2000  | bus state controller (area set-up; ignored)        |
//! | 0xff80_4000  | interrupt controller, SH7780 INTC2 layout          |
//! | 0xffc4_4008  | port data; bit 0 is the strobe for the external    |
//! |              | watchdog (the firmware toggles it from its tick)    |
//! | 0xffc6_2100  | ATAPI interface: task file at +0x00..+0x1c (one   |
//! |              | register per 32-bit word), alternate status at +0x38, |
//! |              | interface control from +0x80                        |
//! | 0xffd8_0000  | timer unit 0; units 1, 2 at 0xffd8_1000, 0xffd8_2000 |
//! | 0xffdf_0000  | CAN controller (RCAN-ET)                           |
//! | 0xffe4_0000  | serial port with FIFO: the loader's debug console  |
//! | 0xfff8_0000  | display unit                                       |

use super::onchip::{Intc, INT2_ATAPI, INT2_CAN, INT2_DU};
use crate::dev::atapi::Atapi;
use crate::dev::hcan::Hcan;
use crate::iolog::IoLog;
use std::collections::HashMap;

const DU: u32 = 0xfff8_0000;
const DU_END: u32 = 0xfffa_0000;
const SCIF_DEBUG: u32 = 0xffe4_0000;
const ATAPI: u32 = 0xffc6_2100;
const CAN: u32 = 0xffdf_0000;
const WATCHDOG_PORT: u32 = 0xffc4_4008;

// Display unit registers (offsets from 0xfff8_0000).  The same block
// lives on in Renesas' R-Car chips, whose Linux driver names these.
const DSYSR: u32 = 0x000;
const DSSR: u32 = 0x008;
const DSRCR: u32 = 0x00c;
const DIER: u32 = 0x010;
const DPPR: u32 = 0x018;
const HDSR: u32 = 0x040;
const HDER: u32 = 0x044;
const VDSR: u32 = 0x048;
const VDER: u32 = 0x04c;
const DESR: u32 = 0x078;
const DEWR: u32 = 0x07c;
const DSSR_VBK: u32 = 1 << 11;
const DSSR_FRM: u32 = 1 << 14;

pub struct Navi {
    /// Backing store for registers without a model, by address.
    pub regs: HashMap<u32, u8>,
    /// Display unit register file, big-endian bytes.
    pub du: Vec<u8>,
    pub du_frames: u64,
    /// Bytes the firmware wrote to its debug console.
    pub console: Vec<u8>,
    /// The CD/DVD drive on the on-chip ATAPI interface.
    pub atapi: Atapi,
    /// The on-chip CAN controller (infotainment bus).
    pub can: Hcan,
    /// Cycle of the last watchdog strobe edge, once there has been one.
    pub watchdog_strobe: Option<u64>,
    pub log: IoLog,
}

impl Default for Navi {
    fn default() -> Self {
        Self::new()
    }
}

/// One of the display unit's eight planes, as programmed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plane {
    pub mode: u32,
    /// Line pitch of the surface in pixels.
    pub stride: u32,
    pub width: u32,
    pub height: u32,
    /// Position on the screen.
    pub x: u32,
    pub y: u32,
    /// Physical address of the surface, and the first pixel shown.
    pub addr: u32,
    pub src_x: u32,
    pub src_y: u32,
    pub transparent: u32,
}

impl Navi {
    pub fn new() -> Navi {
        Navi { regs: HashMap::new(), du: vec![0; (DU_END - DU) as usize], du_frames: 0, console: Vec::new(), atapi: Atapi::new(), can: Hcan::rcan_et(), watchdog_strobe: None, log: IoLog::new(false, 4) }
    }

    /// Whether this module, rather than the SH-4 model, owns `addr`.
    #[inline]
    pub fn owns(addr: u32) -> bool {
        // CCN and the first timer unit stay with the core model.
        !(0xff00_0000..0xff00_0100).contains(&addr) && !(0xffd8_0000..0xffd8_0030).contains(&addr)
    }

    fn raw_read(&self, addr: u32, size: u32) -> u32 {
        (0..size).fold(0, |v, i| (v << 8) | *self.regs.get(&(addr + i)).unwrap_or(&0) as u32)
    }

    fn raw_write(&mut self, addr: u32, size: u32, v: u32) {
        for i in 0..size {
            self.regs.insert(addr + i, (v >> ((size - 1 - i) * 8)) as u8);
        }
    }

    pub fn du32(&self, off: u32) -> u32 {
        let o = off as usize;
        u32::from_be_bytes([self.du[o], self.du[o + 1], self.du[o + 2], self.du[o + 3]])
    }

    fn set_du32(&mut self, off: u32, v: u32) {
        let o = off as usize;
        self.du[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    fn du_irq(&self, intc: &mut Intc) {
        intc.set_int2(INT2_DU, self.du32(DSSR) & self.du32(DIER) != 0);
    }

    pub fn read(&mut self, addr: u32, size: u32, _now: u64, pc: u32, intc: &mut Intc) -> u32 {
        let v = match addr {
            CAN..=0xffdf_0fff => {
                let v = self.can.read(addr - CAN, size, pc);
                intc.set_int2(INT2_CAN, self.can.irq());
                return v;
            }
            ATAPI..=0xffc6_213f => {
                let v = match (addr - ATAPI) >> 2 {
                    // Data: the first byte of each pair in the low half.
                    0 => (self.atapi.read(0, 2, pc) as u16).swap_bytes() as u32,
                    n @ 1..=7 => self.atapi.read(n, 1, pc),
                    14 => self.atapi.read(8, 1, pc),
                    _ => self.raw_read(addr, size),
                };
                intc.set_int2(INT2_ATAPI, self.atapi.irq());
                v
            }
            DU..=0xfff9_ffff if size == 4 => self.du32((addr - DU) & !3),
            // Debug console status: transmitter empty, FIFO has room.
            a if a == SCIF_DEBUG + 0x10 => 0x0060,
            _ => self.raw_read(addr, size),
        };
        self.log.log("soc", false, addr, size, v, pc);
        v
    }

    pub fn write(&mut self, addr: u32, size: u32, v: u32, now: u64, pc: u32, intc: &mut Intc) {
        self.log.log("soc", true, addr, size, v, pc);
        match addr {
            WATCHDOG_PORT => {
                if (self.raw_read(addr, size) ^ v) & 1 != 0 {
                    self.watchdog_strobe = Some(now);
                }
                self.raw_write(addr, size, v);
            }
            CAN..=0xffdf_0fff => {
                self.can.write(addr - CAN, size, v, pc);
                intc.set_int2(INT2_CAN, self.can.irq());
            }
            ATAPI..=0xffc6_213f => {
                match (addr - ATAPI) >> 2 {
                    0 => self.atapi.write(0, 2, (v as u16).swap_bytes() as u32, pc),
                    n @ 1..=7 => self.atapi.write(n, 1, v & 0xff, pc),
                    14 => self.atapi.write(8, 1, v & 0xff, pc),
                    _ => self.raw_write(addr, size, v),
                }
                intc.set_int2(INT2_ATAPI, self.atapi.irq());
            }
            DU..=0xfff9_ffff if size == 4 => {
                let off = (addr - DU) & !3;
                match off {
                    // Status bits are cleared through DSRCR.
                    DSSR => {}
                    DSRCR => {
                        let s = self.du32(DSSR) & !v;
                        self.set_du32(DSSR, s);
                    }
                    _ => self.set_du32(off, v),
                }
                self.du_irq(intc);
            }
            a if a == SCIF_DEBUG + 0x0c => self.console.push(v as u8),
            _ => self.raw_write(addr, size, v),
        }
    }

    /// Start of vertical blanking.
    pub fn vsync(&mut self, intc: &mut Intc) {
        self.du_frames += 1;
        let s = self.du32(DSSR) | DSSR_VBK | DSSR_FRM;
        self.set_du32(DSSR, s);
        self.du_irq(intc);
        intc.set_int2(INT2_ATAPI, self.atapi.irq());
        intc.set_int2(INT2_CAN, self.can.irq());
    }

    /// Recompute the request lines of the devices the host can poke
    /// between instructions (frames arriving on the CAN bus).
    pub fn update_irqs(&mut self, intc: &mut Intc) {
        intc.set_int2(INT2_ATAPI, self.atapi.irq());
        intc.set_int2(INT2_CAN, self.can.irq());
    }

    // ---- display ---------------------------------------------------------

    /// Plane `n` (1..=8).
    pub fn plane(&self, n: u32) -> Plane {
        let b = 0x100 * n;
        Plane {
            mode: self.du32(b),
            stride: self.du32(b + 0x04) & 0xfff,
            width: self.du32(b + 0x10) & 0xfff,
            height: self.du32(b + 0x14) & 0xfff,
            x: self.du32(b + 0x18) & 0xfff,
            y: self.du32(b + 0x1c) & 0xfff,
            addr: self.du32(b + 0x20),
            src_x: self.du32(b + 0x30) & 0xfff,
            src_y: self.du32(b + 0x34) & 0xfff,
            transparent: self.du32(b + 0x44),
        }
    }

    /// Planes shown, back to front.  DPPR holds eight nibbles; each names
    /// a plane (low three bits, 0 = plane 1) and enables it (bit 3).  The
    /// lowest nibble is taken as the front-most position.
    pub fn plane_order(&self) -> Vec<u32> {
        let dppr = self.du32(DPPR);
        (0..8).rev().filter(|pos| dppr >> (pos * 4 + 3) & 1 != 0).map(|pos| (dppr >> (pos * 4) & 7) + 1).collect()
    }

    /// Size of the picture.  Horizontally this is the data-enable width
    /// (the panel's 800 pixels).  Vertically the firmware programs a
    /// display period of 496 lines for a 480-line panel; the panel is
    /// assumed to show the middle 480.
    pub fn resolution(&self) -> (usize, usize) {
        let w = match self.du32(DEWR) & 0xfff {
            0 => self.du32(HDER).wrapping_sub(self.du32(HDSR)) & 0xfff,
            w => w,
        };
        let h = self.du32(VDER).wrapping_sub(self.du32(VDSR)) & 0xfff;
        (if w == 0 { 800 } else { w as usize }, if h == 0 { 480 } else { (h as usize).min(480) })
    }

    /// Display lines above the panel's first line.
    fn y_offset(&self) -> usize {
        let h = (self.du32(VDER).wrapping_sub(self.du32(VDSR)) & 0xfff) as usize;
        h.saturating_sub(480) / 2
    }

    /// Compose the planes into RGB888.  `mem` reads a byte of physical
    /// memory (the planes' surfaces are ordinary RAM).
    pub fn render(&self, mem: &dyn Fn(u32) -> u8) -> (usize, usize, Vec<u8>) {
        let (w, h) = self.resolution();
        let mut out = vec![0u8; w * h * 3];
        // The display period starts a little before data enable; planes
        // are positioned relative to the former.
        let x_off = (self.du32(DESR) & 0xfff).saturating_sub(self.du32(HDSR) & 0xfff).saturating_sub(2) as i64;
        let y_off = self.y_offset();
        for n in self.plane_order() {
            let p = self.plane(n);
            if p.stride == 0 {
                continue;
            }
            let sixteen = p.mode & 3 == 1;
            let argb = p.mode & 3 == 2;
            let palette = 0x1000 * (1 + (p.mode >> 8 & 3));
            let keyed = p.mode & (1 << 14) == 0 && p.mode >> 12 & 3 == 0;
            for py in 0..p.height {
                let sy = (p.y + py) as usize;
                if sy < y_off {
                    continue;
                }
                let sy = sy - y_off;
                if sy >= h {
                    break;
                }
                for px in 0..p.width {
                    let sx = (p.x + px) as i64 - x_off;
                    if sx < 0 || sx as usize >= w {
                        continue;
                    }
                    let (mx, my) = (p.src_x + px, p.src_y + py);
                    let rgb = if sixteen || argb {
                        let a = (p.addr & 0x1fff_ffff) + (my * p.stride + mx) * 2;
                        let v = (mem(a) as u32) << 8 | mem(a + 1) as u32;
                        if keyed && v == p.transparent & 0xffff {
                            continue;
                        }
                        if argb {
                            if v & 0x8000 == 0 {
                                continue;
                            }
                            let (r, g, b) = (v >> 10 & 0x1f, v >> 5 & 0x1f, v & 0x1f);
                            [(r << 3 | r >> 2) as u8, (g << 3 | g >> 2) as u8, (b << 3 | b >> 2) as u8]
                        } else {
                            let (r, g, b) = (v >> 11 & 0x1f, v >> 5 & 0x3f, v & 0x1f);
                            [(r << 3 | r >> 2) as u8, (g << 2 | g >> 4) as u8, (b << 3 | b >> 2) as u8]
                        }
                    } else {
                        let v = mem((p.addr & 0x1fff_ffff) + my * p.stride + mx) as u32;
                        if keyed && v == p.transparent & 0xff {
                            continue;
                        }
                        let c = self.du32(palette + v * 4);
                        [(c >> 16) as u8, (c >> 8) as u8, c as u8]
                    };
                    let o = (sy * w + sx as usize) * 3;
                    out[o..o + 3].copy_from_slice(&rgb);
                }
            }
        }
        (w, h, out)
    }

    /// Human-readable summary of the display configuration.
    pub fn describe(&self) -> String {
        let (w, h) = self.resolution();
        let mut s = format!(
            "display unit: {}x{} DSYSR={:08x} DSSR={:08x} DIER={:08x} DPPR={:08x} period x {}..{} y {}..{} DE {}+{} frames={}\n",
            w,
            h,
            self.du32(DSYSR),
            self.du32(DSSR),
            self.du32(DIER),
            self.du32(DPPR),
            self.du32(HDSR),
            self.du32(HDER),
            self.du32(VDSR),
            self.du32(VDER),
            self.du32(DESR),
            self.du32(DEWR),
            self.du_frames
        );
        let shown = self.plane_order();
        for n in 1..=8 {
            let p = self.plane(n);
            if p.mode != 0 || p.stride != 0 || p.addr != 0 {
                s += &format!(
                    "  plane {} {} mode={:08x} stride {} size {}x{} at {},{} surface {:08x}+{},{} key {:08x}\n",
                    n,
                    if shown.contains(&n) { "on " } else { "off" },
                    p.mode,
                    p.stride,
                    p.width,
                    p.height,
                    p.x,
                    p.y,
                    p.addr,
                    p.src_x,
                    p.src_y,
                    p.transparent
                );
            }
        }
        s
    }
}
