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
//! | 0xff91_5000  | serial port to the radio processor: the 192's      |
//! |              | companion-chip UART with its registers 4 apart     |
//! | 0xff92_6000  | SCIF to the faceplate controller                   |
//! | 0xff93_c000  | A/D converter, eight inputs (4 = display position)  |
//! | 0xff93_f000  | port: +4 direction, +8 output, +0xc input.  Bits   |
//! |              | 19/18 run the display motor (closing/opening), bit |
//! |              | 20 is low only when the display is fully open      |
//! | 0xffc4_4008  | port data; bit 0 is the strobe for the external    |
//! |              | watchdog (the firmware toggles it from its tick)    |
//! | 0xffc6_2100  | ATAPI interface: task file at +0x00..+0x1c (one   |
//! |              | register per 32-bit word), alternate status at +0x38, |
//! |              | interface control from +0x80                        |
//! | 0xffd8_0000  | timer unit 0; unit 1 channel 0 (CAN timer) at 0xffd8_1000; unit 2 at 0xffd8_2000 |
//! | 0xffdf_0000  | CAN controller (RCAN-ET)                           |
//! | 0xffe8_0000  | 2D graphics engine, see [`crate::dev::q2`]         |
//! | 0xffe4_0000  | serial port with FIFO: the loader's debug console  |
//! | 0xfff8_0000  | display unit                                       |

use super::onchip::{Intc, INT2_ADC, INT2_ATAPI, INT2_CAN, INT2_DU, INT2_GE, INT2_PANEL, INT2_RADIO};
use crate::dev::q2::Q2;
use crate::dev::scif::Scif;
use crate::dev::uart::Uart;
use crate::dev::adc::Adc;
use crate::dev::atapi::Atapi;
use crate::dev::hcan::Hcan;
use crate::dev::sdhi::Sdhi;
use crate::iolog::IoLog;
use std::collections::HashMap;

const DU: u32 = 0xfff8_0000;
const DU_END: u32 = 0xfffa_0000;
/// A port (direction +0, data +4); bit 1 of the data register resets
/// the faceplate controller when low.
const PORT_FP: u32 = 0xff90_9000;
const SCIF_DEBUG: u32 = 0xffe4_0000;
const ATAPI: u32 = 0xffc6_2100;
const ATAPI_CONTROL: u32 = 0xffc6_2180;
const ATAPI_STATUS: u32 = 0xffc6_2184;
const ATAPI_DMA_ADDR: u32 = 0xffc6_219c;
const ATAPI_DMA_COUNT: u32 = 0xffc6_21a0;
const ATAPI_SIGNALS: u32 = 0xffc6_21b0;
const CAN: u32 = 0xffdf_0000;
const WATCHDOG_PORT: u32 = 0xffc4_4008;
const GE: u32 = 0xffe8_0000;
const ADC: u32 = 0xff93_c000;
const MOTOR_PORT: u32 = 0xff93_f000;
const RADIO_UART: u32 = 0xff91_5000;
const PANEL_SCIF: u32 = 0xff92_6000;
const SDHI: u32 = 0xff90_a000;

// Display unit registers (offsets from 0xfff8_0000).  The same block
// lives on in Renesas' R-Car chips, whose Linux driver names these.
/// Columns between the start of data enable (DESR) and the panel's
/// first pixel, and the frame line of the panel's first line: chosen so
/// that both the loader's and the OS's pictures fill the panel.
const DE_DELAY: u32 = 18;
const PANEL_FIRST_LINE: u32 = 32;

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
    pub sdhi: Sdhi,
    /// Cycle of the last watchdog strobe edge, once there has been one.
    pub watchdog_strobe: Option<u64>,
    pub adc: Adc,
    /// Link to the radio/audio processor.
    pub radio_uart: Uart,
    /// Link to the faceplate controller.
    pub panel_scif: Scif,
    /// The debug console's port.
    pub debug_scif: Scif,
    /// Position of the motorised display, 0 (open) to 1023 (closed).
    pub display_pos: u16,
    /// The 2D graphics engine.
    pub ge: Q2,
    pub log: IoLog,
    /// Accesses to on-chip registers without a model.
    pub unmodelled: crate::iolog::Unmodelled,
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
    /// Colour not drawn (PnTC1R for 8-bit planes, PnTC2R otherwise).
    pub transparent: u32,
    /// PnALPHAR, used in the alpha blending mode.  Bits 10..8 choose
    /// where the blending ratio comes from: 0 = the low byte, 3 = the
    /// pixels of the 8-bit plane whose number (from 0) is in the low
    /// bits.  The firmware also uses 7, with 0 or 1 in the low bits;
    /// what that selects is not known (the A bit of the pixels would be
    /// a candidate) and such planes are shown without blending.
    pub alpha: u32,
}

impl Navi {
    pub fn new() -> Navi {
        let mut atapi = Atapi::new();
        atapi.packet_irq = true;
        // A/D inputs: the display closed (4) and 25 degrees (5).
        Navi { regs: HashMap::new(), du: vec![0; (DU_END - DU) as usize], du_frames: 0, console: Vec::new(), atapi, can: Hcan::rcan_et(), sdhi: Sdhi::default(), watchdog_strobe: None, adc: Adc::with_inputs([0, 0, 0, 0, 1023, 450, 0, 0]), radio_uart: Uart::default(), panel_scif: Scif::default(), debug_scif: Scif::default(), display_pos: 1023, ge: Q2::default(), log: IoLog::new(false, 4), unmodelled: Default::default() }
    }

    /// Whether this module, rather than the SH-4 model, owns `addr`.
    #[inline]
    pub fn owns(addr: u32) -> bool {
        // CCN and the first timer unit stay with the core model.
        !(0xff00_0000..0xff00_0100).contains(&addr) && !(0xffd8_0000..0xffd8_0030).contains(&addr)
    }

    /// Status of the ATAPI interface: bit 4 is the drive's interrupt
    /// line, the others (transfer end, error) are written by software
    /// only, as long as the interface's DMA is not modelled.
    fn atapi_status(&self) -> u32 {
        (self.raw_read(ATAPI_STATUS, 4) & !0x10) | (self.atapi.irq() as u32) << 4
    }

    /// Software has started the interface's DMA (control bit 0) and the
    /// drive has data: move it to memory.  `ram` begins at physical
    /// address `base`.  The end of the count sets status bit 1.
    pub fn atapi_dma(&mut self, ram: &mut [u8], base: u32, intc: &mut Intc) {
        if self.raw_read(ATAPI_CONTROL, 4) & 1 == 0 || !self.atapi.dma_request() {
            return;
        }
        let addr = self.raw_read(ATAPI_DMA_ADDR, 4) & 0x1fff_ffff;
        let count = self.raw_read(ATAPI_DMA_COUNT, 4);
        let mut done = 0;
        while done < count && self.atapi.dma_request() {
            let byte = self.atapi.dma_read(1) as u8;
            if let Some(b) = ram.get_mut(addr.wrapping_add(done).wrapping_sub(base) as usize) {
                *b = byte;
            }
            done += 1;
        }
        let control = self.raw_read(ATAPI_CONTROL, 4) & !1;
        self.raw_write(ATAPI_CONTROL, 4, control);
        let status = self.raw_read(ATAPI_STATUS, 4) | 2;
        self.raw_write(ATAPI_STATUS, 4, status);
        intc.set_int2(INT2_ATAPI, self.atapi_request());
    }

    /// The interface requests an interrupt for the status bits enabled
    /// in the register after the status.
    fn atapi_request(&self) -> bool {
        self.atapi_status() & self.raw_read(ATAPI_STATUS + 4, 4) != 0
    }

    /// The faceplate controller's reset line is low.
    pub fn panel_reset(&self) -> bool {
        self.raw_read(PORT_FP + 4, 4) & 2 == 0
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
            SDHI..=0xff90_a0ff if size == 2 => {
                let value = u32::from(self.sdhi.read(addr - SDHI));
                intc.set_int2(super::onchip::INT2_SDHI, self.sdhi.irq());
                self.log.log("sdhi", false, addr, size, value, pc);
                return value;
            }
            0xff93_e00c if size == 4 => {
                (self.raw_read(addr, size) & !0xc00)
                    | (self.sdhi.present(0) as u32) << 10 | (self.sdhi.present(1) as u32) << 11
            }
            GE..=0xffe8_00ff if size == 4 => self.ge.read(addr - GE),
            RADIO_UART..=0xff91_503f => {
                let v = self.radio_uart.read((addr - RADIO_UART) >> 1) as u32;
                intc.set_int2(INT2_RADIO, self.radio_uart.irq());
                self.log.log("radio", false, addr, size, v, pc);
                return v;
            }
            PANEL_SCIF..=0xff92_603f => {
                let v = self.panel_scif.read(addr - PANEL_SCIF) as u32;
                intc.set_int2(INT2_PANEL, self.panel_scif.irq());
                self.log.log("panel", false, addr, size, v, pc);
                return v;
            }
            ADC..=0xff93_c01f => {
                let v = self.adc.read_wide(addr - ADC) as u32;
                self.log.log("adc", false, addr, size, v, pc);
                return v;
            }
            a if a == MOTOR_PORT + 0xc && size == 4 => {
                // Input levels.  Bit 20 is the display's position switch.
                // Bit 22 has to be high or the OS stops itself after nine
                // seconds: presumably the supply voltage monitor.
                (self.raw_read(addr, size) & !(1 << 20)) | ((self.display_pos > 94) as u32) << 20 | 1 << 22
            }
            CAN..=0xffdf_0fff => {
                let v = self.can.read(addr - CAN, size, pc);
                intc.set_int2(INT2_CAN, self.can.irq());
                return v;
            }
            ATAPI..=0xffc6_213f => {
                let v = match (addr - ATAPI) >> 2 {
                    // Data: the first byte of each pair in the low half.
                    0 => (self.atapi.read(0, 2, pc) as u16).swap_bytes() as u32,
                    // The OS wants to see DRQ before it starts a DMA
                    // transfer.
                    7 => self.atapi.read(7, 1, pc) | (self.atapi.dma_request() as u32) << 3,
                    n @ 1..=6 => self.atapi.read(n, 1, pc),
                    14 => self.atapi.read(8, 1, pc) | (self.atapi.dma_request() as u32) << 3,
                    _ => self.raw_read(addr, size),
                };
                intc.set_int2(INT2_ATAPI, self.atapi_request());
                v
            }
            ATAPI_STATUS => self.atapi_status(),
            // Signal levels: bit 0 is the drive's DMA request.
            ATAPI_SIGNALS => self.atapi.dma_request() as u32,
            DU..=0xfff9_ffff if size == 4 => self.du32((addr - DU) & !3),
            SCIF_DEBUG..=0xffe4_003f => self.debug_scif.read(addr - SCIF_DEBUG) as u32,
            // Port blocks: direction, output and the rest hold what is
            // written (the input register is handled above where known).
            0xffc4_0000..=0xffc4_002f | 0xffc4_4000..=0xffc4_402f | PORT_FP..=0xff90_9007 => self.raw_read(addr, size),
            _ => {
                self.unmodelled.note(addr, false, pc);
                self.raw_read(addr, size)
            }
        };
        self.log.log("soc", false, addr, size, v, pc);
        v
    }

    pub fn write(&mut self, addr: u32, size: u32, v: u32, now: u64, pc: u32, intc: &mut Intc) {
        self.log.log("soc", true, addr, size, v, pc);
        match addr {
            SDHI..=0xff90_a0ff if size == 2 => {
                self.sdhi.write(addr - SDHI, v as u16);
                intc.set_int2(super::onchip::INT2_SDHI, self.sdhi.irq());
            }
            RADIO_UART..=0xff91_503f => {
                self.radio_uart.write((addr - RADIO_UART) >> 1, v as u16);
                intc.set_int2(INT2_RADIO, self.radio_uart.irq());
            }
            PANEL_SCIF..=0xff92_603f => {
                self.panel_scif.write(addr - PANEL_SCIF, v as u16);
                intc.set_int2(INT2_PANEL, self.panel_scif.irq());
            }
            ADC..=0xff93_c01f => {
                self.adc.write_wide(addr - ADC, v as u16);
                intc.set_int2(INT2_ADC, self.adc.irq());
            }
            GE..=0xffe8_00ff if size == 4 => {
                self.ge.write(addr - GE, v);
                intc.set_int2(INT2_GE, self.ge.irq());
            }
            ATAPI_CONTROL..=0xffc6_218b => {
                self.raw_write(addr, size, v);
                intc.set_int2(INT2_ATAPI, self.atapi_request());
            }
            WATCHDOG_PORT => {
                // Only the loader is known to strobe this way (a 32-bit
                // read-modify-write); how the OS serves the watchdog is
                // not known, so it is left unwatched.
                if size == 4 && (self.raw_read(addr, size) ^ v) & 1 != 0 {
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
                intc.set_int2(INT2_ATAPI, self.atapi_request());
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
            SCIF_DEBUG..=0xffe4_003f => {
                // The debug console.  Nothing leaves the port unless the
                // transmitter has been switched on (SCSCR bit 5).
                self.debug_scif.write(addr - SCIF_DEBUG, v as u16);
                let mut sent = std::mem::take(&mut self.debug_scif.tx);
                self.console.append(&mut sent);
            }
            0xffc4_0000..=0xffc4_002f | 0xffc4_4000..=0xffc4_402f | PORT_FP..=0xff90_9007 => self.raw_write(addr, size, v),
            _ => {
                self.unmodelled.note(addr, true, pc);
                self.raw_write(addr, size, v)
            }
        }
    }

    /// Start of vertical blanking.
    pub fn vsync(&mut self, intc: &mut Intc) {
        self.du_frames += 1;
        let s = self.du32(DSSR) | DSSR_VBK | DSSR_FRM;
        self.set_du32(DSSR, s);
        self.du_irq(intc);
        intc.set_int2(INT2_ATAPI, self.atapi_request());
        intc.set_int2(INT2_CAN, self.can.irq());
        self.step_display(16);
    }

    /// Move the fold-down display by `ms` milliseconds of motor time (a
    /// full sweep takes about 1.5 s) and update its sensors.
    pub fn step_display(&mut self, ms: u32) {
        let drive = self.raw_read(MOTOR_PORT + 8, 4) & self.raw_read(MOTOR_PORT + 4, 4);
        let step = (ms * 1023 / 1500).max(1) as i32;
        let mut pos = self.display_pos as i32;
        match (drive >> 18) & 3 {
            2 => pos += step,
            1 => pos -= step,
            _ => {}
        }
        self.display_pos = pos.clamp(0, 1023) as u16;
        self.adc.input[4] = self.display_pos;
    }

    /// Recompute the request lines of the devices the host can poke
    /// between instructions (frames arriving on the CAN bus).
    pub fn update_irqs(&mut self, intc: &mut Intc) {
        intc.set_int2(super::onchip::INT2_SDHI, self.sdhi.irq());
        intc.set_int2(INT2_ATAPI, self.atapi_request());
        intc.set_int2(INT2_CAN, self.can.irq());
        intc.set_int2(INT2_RADIO, self.radio_uart.irq());
        intc.set_int2(INT2_PANEL, self.panel_scif.irq());
        intc.set_int2(INT2_GE, self.ge.irq());
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
            transparent: self.du32(if self.du32(b) & 3 == 0 { b + 0x44 } else { b + 0x48 }),
            alpha: self.du32(b + 0x08) & 0x7ff,
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

    /// Display lines above the panel's first line.  The panel starts
    /// at a fixed line of the frame: the loader begins its display
    /// period 8 lines earlier than the OS and draws 8 lines lower.
    fn y_offset(&self) -> usize {
        (PANEL_FIRST_LINE.saturating_sub(self.du32(VDSR) & 0xfff)) as usize
    }

    /// Compose the planes into RGB888.  `mem` reads a byte of physical
    /// memory (the planes' surfaces are ordinary RAM).
    pub fn render(&self, mem: &dyn Fn(u32) -> u8) -> (usize, usize, Vec<u8>) {
        let (w, h) = self.resolution();
        let mut out = vec![0u8; w * h * 3];
        // Planes are positioned relative to the display period, the
        // panel's first column follows data enable by a fixed delay.
        let x_off = (self.du32(DESR) & 0xfff).saturating_sub(self.du32(HDSR) & 0xfff).saturating_sub(DE_DELAY) as i64;
        let y_off = self.y_offset();
        let order = self.plane_order();
        // Planes that only supply alpha values are not pictures.
        let blended = |p: &Plane| p.mode >> 12 & 3 == 1;
        let alpha_plane = |p: &Plane| (blended(p) && p.alpha >> 8 == 3).then_some((p.alpha & 7) + 1);
        let alpha_planes: Vec<u32> = order.iter().filter_map(|&n| alpha_plane(&self.plane(n))).collect();
        for n in order {
            let p = self.plane(n);
            if p.stride == 0 || alpha_planes.contains(&n) {
                continue;
            }
            let ratios = alpha_plane(&p).map(|n| self.plane(n));
            // Pixel format: 0 = 8 bits through a palette, 1 = RGB565,
            // 2 = ARGB1555 (the A bit is not interpreted here).
            let format = p.mode & 3;
            let palette = 0x1000 * (1 + (p.mode >> 8 & 3));
            // Superimposing mode (bits 14..12): bit 2 switches the
            // transparent colour off, 1 in the low bits blends the plane
            // with what is behind it.
            let spim = p.mode >> 12 & 7;
            let keyed = spim & 4 == 0;
            let blend = (blended(&p) && p.alpha >> 8 == 0).then_some(p.alpha & 0xff);
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
                    let rgb = if format != 0 {
                        let a = (p.addr & 0x1fff_ffff) + (my * p.stride + mx) * 2;
                        let v = (mem(a) as u32) << 8 | mem(a + 1) as u32;
                        if keyed && v == p.transparent & 0xffff {
                            continue;
                        }
                        if format == 2 {
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
                    let blend = match (&ratios, blend) {
                        (Some(r), _) => Some(mem((r.addr & 0x1fff_ffff) + (r.src_y + py) * r.stride + r.src_x + px) as u32),
                        (None, b) => b,
                    };
                    match blend {
                        Some(alpha) => {
                            for (d, s) in out[o..o + 3].iter_mut().zip(rgb) {
                                *d = ((s as u32 * alpha + *d as u32 * (255 - alpha)) / 255) as u8;
                            }
                        }
                        None => out[o..o + 3].copy_from_slice(&rgb),
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
        s.push_str(&self.ge.describe());
        s.push('\n');
        s
    }
}
