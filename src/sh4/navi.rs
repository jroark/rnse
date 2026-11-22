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
//! | 0xff80_1000  | DMA channels, 0x80 apart: 3 (0xff80_1180) feeds the |
//! |              | serial sound interface, 4 (0xff80_1200) serves the |
//! |              | SD host interface; at 0xff80_1400 registers common |
//! |              | to the channels.  See `dma_write`                  |
//! | 0xff80_2000  | bus state controller (area set-up; ignored)        |
//! | 0xff80_4000  | interrupt controller, SH7780 INTC2 layout          |
//! | 0xff90_a000  | SD host interface for the two card slots, see      |
//! |              | [`crate::dev::sdhi`]                               |
//! | 0xff91_5000  | serial port to the radio processor: the 192's      |
//! |              | companion-chip UART with its registers 4 apart     |
//! | 0xff92_6000  | SCIF to the faceplate controller                   |
//! | 0xff93_c000  | A/D converter, eight inputs (4 = display position)  |
//! | 0xff93_f000  | port: +4 direction, +8 output, +0xc input.  Bits   |
//! |              | 19/18 run the display motor (closing/opening), bit |
//! |              | 20 is low only when the display is fully open      |
//! | 0xff97_2000  | audio clock: 0x3dff for the 44.1 kHz family of     |
//! |              | sample rates, 0x3eff for the 48 kHz family         |
//! | 0xff98_4000  | serial sound interface (the SSI of other Renesas   |
//! |              | chips): +0 control, +4 status, +8 transmit data.   |
//! |              | Sound decoded by the software (MP3) leaves here    |
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

/// State of the DMA channel that feeds the sound interface, beyond
/// what its registers hold.
#[derive(Default)]
pub struct AudioDma {
    pub running: bool,
    /// The second set of address and count registers is the one in use.
    second: bool,
    /// Time not yet turned into samples, in microseconds times the rate.
    owed: u64,
    /// The interface was on with nothing to send when last looked at.
    starved: bool,
    /// Stereo samples sent since the start.
    pub frames: u64,
    /// Print what the driver does with the channel (`--trace-sound`).
    pub trace: bool,
}

const DU: u32 = 0xfff8_0000;
const DU_END: u32 = 0xfffa_0000;
/// A port (direction +0, data +4); bit 1 of the data register resets
/// the faceplate controller when low.
const PORT_FP: u32 = 0xff90_9000;
const SCIF_DEBUG: u32 = 0xffe4_0000;
/// The DMA channel that feeds the serial sound interface.
const AUDIO_DMA: u32 = 0xff80_1180;
/// Its bit in the common registers.
const AUDIO_DMA_BIT: u32 = 0x40;
/// The DMA channel the SD card driver reads sectors through.
const DMA: u32 = 0xff80_1200;
/// Registers common to the DMA channels.
const DMA_COMMON: u32 = 0xff80_1400;
/// This channel's bit in the common registers.
const DMA_BIT: u32 = 0x100;
/// The serial sound interface: control, status, transmit data.
const SSI: u32 = 0xff98_4000;
const AUDIO_CLOCK: u32 = 0xff97_2000;
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
    /// The sound interface's DMA channel.
    pub audio: AudioDma,
    /// Cycle of the last watchdog strobe edge, once there has been one.
    /// Edges seen on the watchdog strobe (bit 0 of [`WATCHDOG_PORT`]).
    pub watchdog_edges: u64,
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
        let mut navi = Navi { regs: HashMap::new(), du: vec![0; (DU_END - DU) as usize], du_frames: 0, console: Vec::new(), atapi, can: Hcan::rcan_et(), sdhi: Sdhi::default(), audio: AudioDma::default(), watchdog_edges: 0, adc: Adc::with_inputs([0, 0, 0, 0, 1023, 450, 0, 0]), radio_uart: Uart::default(), panel_scif: Scif::default(), debug_scif: Scif::default(), display_pos: 1023, ge: Q2::default(), log: IoLog::new(false, 4), unmodelled: Default::default() };
        // The faceplate's port: the bench 193 read 0xdfffffff and
        // 0xfffffffb there once its software had run, which changes
        // only bit 1 of +4 (the faceplate's reset line, raised).  Its
        // other lines are taken to be so from the start, the reset line
        // low.  (Plain memory reading 0 here hid a kit start-up that
        // drove them all low and set the drive ejecting on the unit.)
        navi.raw_write(PORT_FP, 4, 0xdfff_ffff);
        navi.raw_write(PORT_FP + 4, 4, 0xffff_fff9);
        navi
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

    /// A register of the port blocks at 0xffc40000 and 0xffc44000.  An
    /// access of any size at the register's address gets its low bits:
    /// the loader toggles the watchdog strobe with 32-bit accesses and
    /// the OS toggles the same line by reading 32 bits and writing a
    /// byte to the same address.  (Memory would put that byte in bits
    /// 31..24.)
    fn port_read(&self, addr: u32, size: u32) -> u32 {
        if addr & 3 != 0 {
            return self.raw_read(addr, size);
        }
        let v = self.raw_read(addr, 4);
        if size >= 4 { v } else { v & ((1 << (size * 8)) - 1) }
    }

    fn port_write(&mut self, addr: u32, size: u32, v: u32) {
        if addr & 3 != 0 || size >= 4 {
            return self.raw_write(addr, size, v);
        }
        let mask = (1u32 << (size * 8)) - 1;
        let old = self.raw_read(addr, 4);
        self.raw_write(addr, 4, old & !mask | v & mask);
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
            // A register of the memory controller, as a bench unit has
            // it (rnse-sdk's mon193, 2026-11-21).  The stock loader and
            // the kit's board_init compute what they write to +0x220
            // from its bits 10-18 (0x49 here: 0x1047 written); read as
            // 0 it made a constant of that write, 0xfffffffe, which
            // stopped a unit's RAM.
            0xfe80_0280 if size == 4 => 0x0249_24e5,
            ATAPI_STATUS => self.atapi_status(),
            // Signal levels: bit 0 is the drive's DMA request.
            ATAPI_SIGNALS => self.atapi.dma_request() as u32,
            DU..=0xfff9_ffff if size == 4 => self.du32((addr - DU) & !3),
            SCIF_DEBUG..=0xffe4_003f => self.debug_scif.read(addr - SCIF_DEBUG) as u32,
            // Port blocks: direction, output and the rest hold what is
            // written (the input register is handled above where known).
            0xffc4_0000..=0xffc4_002f | 0xffc4_4000..=0xffc4_402f => self.port_read(addr, size),
            PORT_FP..=0xff90_9007 => self.raw_read(addr, size),
            a if a == AUDIO_DMA + 0x34 => self.audio_dma_status(),
            AUDIO_DMA..=0xff80_14ff | SSI..=0xff98_400f | AUDIO_CLOCK..=0xff97_2003 => self.raw_read(addr, size),
            _ => {
                self.unmodelled.note(addr, false, pc);
                self.raw_read(addr, size)
            }
        };
        self.log.log("soc", false, addr, size, v, pc);
        v
    }

    pub fn write(&mut self, addr: u32, size: u32, v: u32, _now: u64, pc: u32, intc: &mut Intc) {
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
                // The loader toggles bit 0 with 32-bit accesses, the OS
                // with byte accesses to the same address.
                if (self.port_read(addr, size) ^ v) & 1 != 0 {
                    self.watchdog_edges += 1;
                }
                self.port_write(addr, size, v);
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
                    // The key the software writes in the top half (0x7773)
                    // is not kept: the bench 193 read back 0x00000001
                    // and 0x00000011 for 0x77730001 and 0x77730011.
                    0x020 | 0x024 => self.set_du32(off, v & 0xffff),
                    // A palette entry keeps six bits of each colour: the
                    // bench 193 read 0x00fcfcfc back for 0x00ffffff, and
                    // the entries the stock software never writes held
                    // random top bytes with those low bits all clear.
                    0x1000..=0x4fff => self.set_du32(off, v & 0xfffc_fcfc),
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
            0xffc4_0000..=0xffc4_002f | 0xffc4_4000..=0xffc4_402f => self.port_write(addr, size, v),
            PORT_FP..=0xff90_9007 => self.raw_write(addr, size, v),
            AUDIO_DMA..=0xff80_14ff if size == 4 => {
                self.dma_write(addr, v);
                self.audio_irq(intc);
            }
            // Status bits of the sound interface are cleared by
            // writing them as zeroes.
            a if a == SSI + 4 => {
                let left = self.raw_read(a, 4) & v;
                self.raw_write(a, 4, left);
                self.audio_irq(intc);
            }
            SSI..=0xff98_400f | AUDIO_CLOCK..=0xff97_2003 => {
                self.raw_write(addr, size, v);
                self.audio_irq(intc);
            }
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

    /// What the display's motor is driven to do: -1 towards open, 1
    /// towards closed, 0 nothing (or both lines at once).
    pub fn display_motor(&self) -> i32 {
        let drive = self.raw_read(MOTOR_PORT + 8, 4) & self.raw_read(MOTOR_PORT + 4, 4);
        match (drive >> 18) & 3 {
            2 => 1,
            1 => -1,
            _ => 0,
        }
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

    /// The DMA channels, as far as their two users show them.  They
    /// are 0x80 apart from 0xff80_1000; channel 4 (0xff80_1200) is the
    /// SD card driver's, which polls, channel 3 (0xff80_1180) feeds
    /// the serial sound interface and interrupts.
    ///
    /// | register | meaning |
    /// |----------|---------|
    /// | +0x00 | source address |
    /// | +0x04 | destination address |
    /// | +0x08 | count of transfers (16 bits each for the SD host's data port, 32 for the sound interface) |
    /// | +0x0c, +0x10, +0x14 | a second set of the three |
    /// | +0x28 | mode: 0x0040_2112 for the SD driver's reads, 0x0505_0022 for sound |
    /// | +0x2c | control.  Bit 0: run.  Bit 1: the set that is not in use has been loaded, go on with it when this one is done.  Bit 2: stop when this one is done |
    /// | +0x30 | write 1: stop |
    /// | +0x34 | status.  Bit 0: running.  Bit 1: a loaded set is waiting.  Bit 6: the first set is the one in use (the driver loads the second then) |
    ///
    /// In the common block at 0xff80_1400, with bit 2n for channel n:
    /// +0x10 a set's count has run out, +0x14 write 1 to clear that,
    /// +0x18 interrupt enable.  The sound channel's interrupt is INTEVT
    /// 0x980, the SD channel's would be 0x9c0 (its driver clears the
    /// enable bit and looks at +0x10 itself).
    ///
    /// The meaning of the control and status bits is read off the
    /// sound driver, which keeps two blocks queued; that the address
    /// and count registers count during a transfer is assumed.
    fn dma_write(&mut self, addr: u32, v: u32) {
        match addr {
            a if a == DMA + 0x2c => if v & 1 != 0 { self.raw_write(DMA + 0x34, 4, 1) },
            a if a == DMA + 0x30 => if v & 1 != 0 { self.raw_write(DMA + 0x34, 4, 0) },
            a if a == DMA_COMMON + 0x14 => {
                let left = self.raw_read(DMA_COMMON + 0x10, 4) & !v;
                self.raw_write(DMA_COMMON + 0x10, 4, left);
            }
            a if a == AUDIO_DMA + 0x2c => {
                if self.audio.trace {
                    println!("[sound] control {:x} (was {:x}, status {:x}); sets {:08x}+{:x} {:08x}+{:x}; {} samples out", v,
                        self.raw_read(a, 4), self.audio_dma_status(), self.raw_read(AUDIO_DMA, 4), self.raw_read(AUDIO_DMA + 8, 4),
                        self.raw_read(AUDIO_DMA + 0xc, 4), self.raw_read(AUDIO_DMA + 0x14, 4), self.audio.frames);
                }
                self.raw_write(a, 4, v);
                if v & 1 != 0 && !self.audio.running {
                    self.audio.running = true;
                    self.audio.second = false;
                }
            }
            a if a == AUDIO_DMA + 0x30 => if v & 1 != 0 {
                self.audio.running = false;
                self.raw_write(AUDIO_DMA + 0x2c, 4, 0);
            },
            _ => self.raw_write(addr, 4, v),
        }
    }

    fn audio_dma_status(&self) -> u32 {
        if !self.audio.running {
            return 0;
        }
        1 | (self.raw_read(AUDIO_DMA + 0x2c, 4) & 2) | if self.audio.second { 0 } else { 0x40 }
    }

    /// The sample rate the sound interface is set up for.  The driver
    /// has a table of rates with what it writes for each to the
    /// interface's control register (the clock divider among it) and
    /// to the audio clock register; this is that table read backwards.
    pub fn audio_rate(&self) -> u32 {
        let family = if self.raw_read(AUDIO_CLOCK, 4) == 0x3dff { 44_100 } else { 48_000 };
        match self.raw_read(SSI, 4) & 0x000f_0070 {
            0x000b_0060 => 32_000,
            0x000b_0040 => family / 2,
            0x000c_0040 => 16_000,
            0x000d_0040 => family / 4,
            _ => family,
        }
    }

    fn audio_irq(&self, intc: &mut Intc) {
        let common = self.raw_read(DMA_COMMON + 0x10, 4) & self.raw_read(DMA_COMMON + 0x18, 4);
        intc.set_int2(super::onchip::INT2_AUDIO_DMA, common & AUDIO_DMA_BIT != 0);
        // Underflow: bit 27 of the status, enabled by the same bit of
        // the control register (as in the SSI of other Renesas chips:
        // 28 DMA, 27 underflow, 26 overflow, 25 idle).
        let underflow = self.raw_read(SSI + 4, 4) & self.raw_read(SSI, 4) & 0x0800_0000 != 0;
        intc.set_int2(super::onchip::INT2_SSI, underflow);
    }

    /// The sound interface sends for `micros` microseconds: its DMA
    /// channel fetches the samples from memory, a word of 32 bits for
    /// each moment: 16 bits for the left channel in the high half and
    /// 16 for the right one in the low half, so that the buffer is
    /// plain interleaved big-endian PCM (a file with different tones
    /// left and right comes out of the stock decoder that way round).
    /// What goes out is added to `out` as
    /// stereo samples at the interface's rate; nothing is added while
    /// the interface is off.
    pub fn audio_out(&mut self, ram: &[u8], base: u32, micros: u64, mut out: Option<&mut Vec<i16>>, intc: &mut Intc) {
        let control = self.raw_read(SSI, 4);
        // Enabled, transmitting, with DMA.
        if control & 0x1000_0003 != 0x1000_0003 {
            return;
        }
        self.audio.owed += micros * u64::from(self.audio_rate());
        let frames = self.audio.owed / 1_000_000;
        self.audio.owed %= 1_000_000;
        if !self.audio.running {
            // Nothing to send.  The driver lets the channel run out
            // when it has no further block (control bit 2) and turns
            // the interface off in the channel's interrupt handler, so
            // there is always a moment like this; how the hardware
            // keeps that from counting as an underflow is not known.
            // Here it is one only if it lasts into a second period.
            if std::mem::replace(&mut self.audio.starved, true) {
                let status = self.raw_read(SSI + 4, 4) | 0x0800_0000;
                self.raw_write(SSI + 4, 4, status);
            }
            if let Some(out) = out.as_deref_mut() {
                out.resize(out.len() + 2 * frames as usize, 0);
            }
            self.audio_irq(intc);
            return;
        }
        self.audio.starved = false;
        for _ in 0..frames {
            if !self.audio.running {
                if let Some(out) = out.as_deref_mut() {
                    out.extend_from_slice(&[0, 0]);
                }
                continue;
            }
            let set = AUDIO_DMA + if self.audio.second { 0x0c } else { 0 };
            let from = self.raw_read(set, 4);
            let at = (from & 0x1fff_ffff).wrapping_sub(base) as usize;
            let word = ram.get(at..at + 4).map(|b| [b[0], b[1], b[2], b[3]]).unwrap_or([0; 4]);
            if let Some(out) = out.as_deref_mut() {
                out.push(i16::from_be_bytes([word[0], word[1]]));
                out.push(i16::from_be_bytes([word[2], word[3]]));
            }
            self.audio.frames += 1;
            self.raw_write(set, 4, from.wrapping_add(4));
            let left = self.raw_read(set + 8, 4).saturating_sub(1);
            self.raw_write(set + 8, 4, left);
            if left == 0 {
                let done = self.raw_read(DMA_COMMON + 0x10, 4) | AUDIO_DMA_BIT;
                self.raw_write(DMA_COMMON + 0x10, 4, done);
                let control = self.raw_read(AUDIO_DMA + 0x2c, 4);
                if self.audio.trace {
                    println!("[sound] set {} done after {} samples, control {:x}", self.audio.second as u8 + 1, self.audio.frames, control);
                }
                if control & 2 != 0 {
                    self.audio.second = !self.audio.second;
                    self.raw_write(AUDIO_DMA + 0x2c, 4, control & !2);
                } else {
                    self.audio.running = false;
                }
            }
        }
        self.audio_irq(intc);
    }

    /// Move what the SD host interface has ready to memory, if the DMA
    /// channel is running and set up for it.  Memory is written past
    /// the CPU's caches, as DMA does.
    pub fn dma_run<B: super::Bus>(&mut self, bus: &mut B, intc: &mut Intc) {
        if self.raw_read(DMA + 0x34, 4) & 1 == 0 || self.raw_read(DMA, 4) != SDHI + 0x30 {
            return;
        }
        let (mut to, mut count) = (self.raw_read(DMA + 4, 4), self.raw_read(DMA + 8, 4));
        while count != 0 && self.sdhi.data_ready() {
            let word = self.sdhi.read(0x30);
            bus.write16(to & 0x1fff_ffff, word);
            to = to.wrapping_add(2);
            count -= 1;
        }
        self.raw_write(DMA + 4, 4, to);
        self.raw_write(DMA + 8, 4, count);
        if count == 0 {
            self.raw_write(DMA + 0x34, 4, 0);
            let done = self.raw_read(DMA_COMMON + 0x10, 4) | DMA_BIT;
            self.raw_write(DMA_COMMON + 0x10, 4, done);
        }
        intc.set_int2(super::onchip::INT2_SDHI, self.sdhi.irq());
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
            if format == 0 {
                warn_8bpp(n, p.mode);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The watchdog strobe as the loader and as the OS toggle it.
    #[test]
    fn port_registers_take_any_access_size() {
        let mut navi = Navi::new();
        let mut io = crate::sh4::onchip::OnChip::new();
        let intc = &mut io.intc;
        // The loader: 32-bit read, flip bit 0, 32-bit write.
        let v = navi.read(WATCHDOG_PORT, 4, 0, 0, intc);
        navi.write(WATCHDOG_PORT, 4, v ^ 1, 0, 0, intc);
        assert_eq!(navi.watchdog_edges, 1);
        // The OS: 32-bit read, flip bit 0, write the low byte to the
        // same address.
        for _ in 0..3 {
            let v = navi.read(WATCHDOG_PORT, 4, 0, 0, intc);
            let b = navi.read(WATCHDOG_PORT, 1, 0, 0, intc);
            assert_eq!(b, v & 0xff);
            navi.write(WATCHDOG_PORT, 1, (b & !1) | ((v ^ 1) & 1), 0, 0, intc);
        }
        assert_eq!(navi.watchdog_edges, 4);
        assert_eq!(navi.read(WATCHDOG_PORT, 4, 0, 0, intc), 0);
    }
}

/// An 8 bpp plane shown through a palette: drawn here as the emulator
/// understands the display unit, but on the bench 193 such a plane
/// (the kit's plane 6, mode 0x00024100 with palette 2 written in full,
/// as the stock loader sets up its own) came out a flat light grey,
/// while the stock software's 16 bpp planes showed exactly what was
/// written into them (2026-11-21, docs/hardware-193.md, "Display
/// unit").  Said once per run, so that a program relying on it knows.
fn warn_8bpp(plane: u32, mode: u32) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAID: AtomicBool = AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        eprintln!("[du] plane {} shown at 8 bpp through a palette (mode {:08x}): on a real 193 such a plane \
showed flat light grey; draw 16 bpp (RGB555) to be sure (docs/hardware-193.md, \"Display unit\")", plane, mode);
    }
}
