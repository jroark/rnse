//! SH7750R on-chip peripheral modules reached through the P4 area:
//! exception registers, INTC, TMU, SCI, SCIF, DMAC, BSC, CPG, RTC.

use super::Bus;
use crate::iolog::IoLog;
use std::collections::VecDeque;

/// On-chip interrupt sources, in default priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Irq {
    Hudi,
    Gpio,
    Dmte0,
    Dmte1,
    Dmte2,
    Dmte3,
    Dmte4,
    Dmte5,
    Dmte6,
    Dmte7,
    Dmae,
    Tuni0,
    Tuni1,
    Tuni2,
    Ticpi2,
    Tuni3,
    Tuni4,
    RtcAti,
    RtcPri,
    RtcCui,
    SciEri,
    SciRxi,
    SciTxi,
    SciTei,
    ScifEri,
    ScifRxi,
    ScifBri,
    ScifTxi,
    WdtIti,
    RefRcmi,
    RefRovi,
}

#[derive(Clone, Copy)]
enum Pri {
    A(u32),
    B(u32),
    C(u32),
    /// INTPRI00 nibble shift
    P00(u32),
}

const IRQ_TABLE: [(Irq, u32, Pri); 31] = [
    (Irq::Hudi, 0x600, Pri::C(0)),
    (Irq::Gpio, 0x620, Pri::C(12)),
    (Irq::Dmte0, 0x640, Pri::C(8)),
    (Irq::Dmte1, 0x660, Pri::C(8)),
    (Irq::Dmte2, 0x680, Pri::C(8)),
    (Irq::Dmte3, 0x6a0, Pri::C(8)),
    (Irq::Dmte4, 0x780, Pri::C(8)),
    (Irq::Dmte5, 0x7a0, Pri::C(8)),
    (Irq::Dmte6, 0x7c0, Pri::C(8)),
    (Irq::Dmte7, 0x7e0, Pri::C(8)),
    (Irq::Dmae, 0x6c0, Pri::C(8)),
    (Irq::Tuni0, 0x400, Pri::A(12)),
    (Irq::Tuni1, 0x420, Pri::A(8)),
    (Irq::Tuni2, 0x440, Pri::A(4)),
    (Irq::Ticpi2, 0x460, Pri::A(4)),
    (Irq::Tuni3, 0xb00, Pri::P00(8)),
    (Irq::Tuni4, 0xb80, Pri::P00(12)),
    (Irq::RtcAti, 0x480, Pri::A(0)),
    (Irq::RtcPri, 0x4a0, Pri::A(0)),
    (Irq::RtcCui, 0x4c0, Pri::A(0)),
    (Irq::SciEri, 0x4e0, Pri::B(4)),
    (Irq::SciRxi, 0x500, Pri::B(4)),
    (Irq::SciTxi, 0x520, Pri::B(4)),
    (Irq::SciTei, 0x540, Pri::B(4)),
    (Irq::ScifEri, 0x700, Pri::C(4)),
    (Irq::ScifRxi, 0x720, Pri::C(4)),
    (Irq::ScifBri, 0x740, Pri::C(4)),
    (Irq::ScifTxi, 0x760, Pri::C(4)),
    (Irq::WdtIti, 0x560, Pri::B(12)),
    (Irq::RefRcmi, 0x580, Pri::B(8)),
    (Irq::RefRovi, 0x5a0, Pri::B(8)),
];

/// A request line into the SH7770-style interrupt controller (INT2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Int2Source {
    pub name: &'static str,
    /// INTEVT code.
    pub code: u32,
    /// Priority register (index of INT2PRIn) and bit position of the
    /// 5-bit field; the upper four bits are the level.
    pub pri: (usize, u32),
    /// Bit in the mask register (INT2MSKR), or in the second one when 32
    /// is added.
    pub mask_bit: u32,
}

/// The sources identified so far, from the firmware's own set-up code.
/// The index is the bit number in [`Int2::pending`].
pub const INT2_SOURCES: [Int2Source; 14] = [
    Int2Source { name: "TMU0", code: 0x400, pri: (0, 16), mask_bit: 1 },
    Int2Source { name: "TMU2", code: 0x440, pri: (0, 8), mask_bit: 1 },
    Int2Source { name: "TMU2 capture", code: 0x460, pri: (0, 0), mask_bit: 1 },
    // Vertical blanking.  The OS also sets bit 0 of 0xff80_2000 when it
    // enables this one, so it may really be an interrupt pin.
    Int2Source { name: "display", code: 0x3e0, pri: (0, 24), mask_bit: 0 },
    Int2Source { name: "CAN", code: 0x880, pri: (5, 16), mask_bit: 25 },
    Int2Source { name: "ATAPI", code: 0x5e0, pri: (1, 0), mask_bit: 7 },
    // No mask bit found: the handler re-arms something at 0xff80_4260
    // and 0xff90_2260 instead.
    Int2Source { name: "A/D converter", code: 0xf00, pri: (11, 8), mask_bit: INT2_UNMASKED },
    // The firmware writes the settings of these two to a second block at
    // 0xff90_2000 as well, which is not modelled.
    Int2Source { name: "radio link", code: 0xd20, pri: (9, 8), mask_bit: 32 + 9 },
    Int2Source { name: "faceplate link", code: 0xdc0, pri: (10, 16), mask_bit: 32 + 14 },
    Int2Source { name: "graphics engine", code: 0x780, pri: (3, 0), mask_bit: 19 },
    Int2Source { name: "CAN timer", code: 0x480, pri: (1, 24), mask_bit: 2 },
    Int2Source { name: "SD host", code: 0xea0, pri: (11, 24), mask_bit: 32 + 21 },
    // The DMA channel that feeds the serial sound interface: a block
    // of samples has gone out.
    Int2Source { name: "audio DMA", code: 0x980, pri: (6, 24), mask_bit: 27 },
    // The serial sound interface itself: it ran out of samples.
    Int2Source { name: "sound interface", code: 0xca0, pri: (9, 24), mask_bit: 32 + 5 },
];
pub const INT2_SDHI: usize = 11;
pub const INT2_AUDIO_DMA: usize = 12;
pub const INT2_SSI: usize = 13;
pub const INT2_GE: usize = 9;
pub const INT2_CAN_TIMER: usize = 10;
pub const INT2_RADIO: usize = 7;
pub const INT2_PANEL: usize = 8;
/// `mask_bit` of a source that is not gated by the mask registers.
pub const INT2_UNMASKED: u32 = 64;
pub const INT2_ADC: usize = 6;
pub const INT2_DU: usize = 3;
pub const INT2_CAN: usize = 4;
pub const INT2_ATAPI: usize = 5;

/// Interrupt controller for on-chip modules in the SH7770 family: the
/// same arrangement as the SH7780's INTC2, at 0xff80_4000.
///
/// | offset        | register                                          |
/// |---------------|---------------------------------------------------|
/// | +0x00..+0x2c  | INT2PRI0-11, four 5-bit priorities each           |
/// | +0x38, +0x3c  | mask set / mask clear (1 = masked after reset)    |
/// | +0x108, +0x10c| the same for a second group of 32 sources         |
pub struct Int2 {
    pub pri: [u32; 16],
    /// 1 = masked; bits 32.. are the second register.
    pub mask: u64,
    /// Requests, one bit per entry of [`INT2_SOURCES`].
    pub pending: u64,
}

impl Int2 {
    pub fn new() -> Int2 {
        Int2 { pri: [0; 16], mask: u64::MAX, pending: 0 }
    }

    /// Request status registers: +0x30 and +0x100 show every request of
    /// their half, +0x34 and +0x104 only those not masked.
    fn status(&self, off: u32) -> u32 {
        let mut bits = 0u64;
        for (i, src) in INT2_SOURCES.iter().enumerate() {
            if self.pending >> i & 1 != 0 && src.mask_bit < 64 {
                bits |= 1 << src.mask_bit;
            }
        }
        if off & 4 != 0 {
            bits &= !self.mask;
        }
        if off >= 0x100 { (bits >> 32) as u32 } else { bits as u32 }
    }

    fn best(&self) -> (u32, u32) {
        let (mut level, mut code) = (0, 0);
        for (i, src) in INT2_SOURCES.iter().enumerate() {
            if self.pending >> i & 1 != 0 && (src.mask_bit >= 64 || self.mask >> src.mask_bit & 1 == 0) {
                let l = (self.pri[src.pri.0] >> src.pri.1 & 0x1f) >> 1;
                if l > level {
                    level = l;
                    code = src.code;
                }
            }
        }
        (level, code)
    }
}

impl Intc {
    /// Raise or drop a request line of the SH7770-style controller.
    pub fn set_int2(&mut self, source: usize, on: bool) {
        if let Some(int2) = self.int2.as_mut() {
            let old = int2.pending;
            if on {
                int2.pending |= 1 << source;
            } else {
                int2.pending &= !(1 << source);
            }
            if int2.pending != old {
                self.recalc();
            }
        }
    }
}

impl Default for Int2 {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Intc {
    /// Present on the SH7770 family, where it replaces the priority
    /// registers below for on-chip sources.
    pub int2: Option<Int2>,
    pub icr: u16,
    pub ipra: u16,
    pub iprb: u16,
    pub iprc: u16,
    pub iprd: u16,
    pub intpri00: u32,
    pub intmsk00: u32,
    /// Bit per `Irq` discriminant.
    pub pending: u32,
    /// Level encoded on the IRL pins (0 = no request).
    pub irl: u8,
    /// Priority of the best pending request (0 = none).  Compared with
    /// SR.IMASK by the CPU on every instruction.
    pub level: u32,
    code: u32,
    pub nmi_pending: bool,
}

impl Intc {
    fn new() -> Self {
        Intc {
            int2: None,
            icr: 0,
            ipra: 0,
            iprb: 0,
            iprc: 0,
            iprd: 0xda74, // its value after a reset
            intpri00: 0,
            intmsk00: 0x0000_0300, // the two bits it has
            pending: 0,
            irl: 0,
            level: 0,
            code: 0,
            nmi_pending: false,
        }
    }

    fn priority(&self, p: Pri) -> u32 {
        match p {
            Pri::A(s) => (self.ipra as u32 >> s) & 0xf,
            Pri::B(s) => (self.iprb as u32 >> s) & 0xf,
            Pri::C(s) => (self.iprc as u32 >> s) & 0xf,
            Pri::P00(s) => (self.intpri00 >> s) & 0xf,
        }
    }

    pub fn recalc(&mut self) {
        let mut level = 0;
        let mut code = 0;
        if self.irl != 0 {
            level = self.irl as u32;
            code = 0x200 + (15 - level) * 0x20;
        }
        if let Some(int2) = self.int2.as_mut() {
            // The timers are the same module as on the SH7750; their
            // requests arrive through `pending` and are routed here.
            for (i, irq) in [Irq::Tuni0, Irq::Tuni2, Irq::Ticpi2].into_iter().enumerate() {
                if self.pending & (1 << irq as u32) != 0 {
                    int2.pending |= 1 << i;
                } else {
                    int2.pending &= !(1 << i);
                }
            }
            if self.pending & (1 << Irq::Tuni3 as u32) != 0 {
                int2.pending |= 1 << INT2_CAN_TIMER;
            } else {
                int2.pending &= !(1 << INT2_CAN_TIMER);
            }
            let (l, c) = int2.best();
            if l > level {
                level = l;
                code = c;
            }
        } else if self.pending != 0 {
            for (irq, c, p) in IRQ_TABLE.iter() {
                if self.pending & (1 << *irq as u32) != 0 {
                    let pr = self.priority(*p);
                    if pr > level {
                        level = pr;
                        code = *c;
                    }
                }
            }
        }
        self.level = level;
        self.code = code;
    }

    /// (level, INTEVT code) of the request that would be accepted now.
    pub fn best(&self) -> (u32, u32) {
        (self.level, self.code)
    }

    pub fn set(&mut self, irq: Irq, on: bool) {
        let bit = 1u32 << irq as u32;
        let new = if on { self.pending | bit } else { self.pending & !bit };
        if new != self.pending {
            self.pending = new;
            self.recalc();
        }
    }

    pub fn set_irl(&mut self, level: u8) {
        self.irl = level & 0xf;
        self.recalc();
    }
}

#[derive(Default, Clone, Copy)]
pub struct TmuCh {
    pub tcor: u32,
    pub tcnt: u32,
    pub tcr: u16,
    pub running: bool,
    last: u64,
}

#[derive(Default, Clone, Copy)]
pub struct DmaCh {
    pub sar: u32,
    pub dar: u32,
    pub tcr: u32,
    pub chcr: u32,
    /// Cycle at which a started transfer is reported complete.
    done_at: u64,
    busy: bool,
}

pub struct OnChip {
    pub tra: u32,
    pub expevt: u32,
    pub intevt: u32,
    pub sq: [[u32; 8]; 2],
    pub intc: Intc,

    /// Unit 0's three channels, then the 193's unit 1 channel 0.
    pub tmu: [TmuCh; 4],
    pub tocr: u8,
    /// CPU cycles per peripheral clock (P-phi) cycle.
    pub pclk_div: u64,
    /// The peripheral clock, in Hz.
    pub pclk_hz: u64,

    pub dma: [DmaCh; 8],
    pub dmaor: u32,

    // SCIF (console)
    pub scif_scr: u16,
    pub scif_smr: u16,
    pub scif_fcr: u16,
    pub scif_brr: u8,
    pub scif_sptr: u16,
    pub scif_rx: VecDeque<u8>,
    pub scif_tx: Vec<u8>,

    // SCI
    pub sci_scr: u8,
    pub sci_smr: u8,
    pub sci_brr: u8,
    pub sci_ssr: u8,
    pub sci_sptr: u8,
    pub sci_rx: VecDeque<u8>,
    pub sci_tx: Vec<u8>,

    // BSC / port
    pub bsc: [u32; 32],
    pub pctra: u32,
    pub pdtra: u16,
    /// Levels presented on port A pins configured as inputs.
    pub pdtra_in: u16,
    pub pctrb: u32,
    pub pdtrb: u16,
    pub pdtrb_in: u16,
    rfcr: u16,

    // CPG / WDT
    pub frqcr: u16,
    pub stbcr: u8,
    pub stbcr2: u8,
    pub wtcnt: u8,
    pub wtcsr: u8,

    // RTC
    pub rtc: [u8; 16],
    rtc_last: u64,

    pub next_event: u64,
    /// Timer 2's input capture register.
    pub tcpr2: u32,
    pub log: IoLog,
    /// Present when the CPU is the SH7770-family system-on-chip of the
    /// 193 unit: its own peripherals replace the SH7750's.
    pub navi: Option<Box<super::navi::Navi>>,
}

impl Default for OnChip {
    fn default() -> Self {
        Self::new()
    }
}

const TMU_BASE: u32 = 0xffd8_0000;

impl OnChip {
    pub fn new() -> Self {
        let mut tmu = [TmuCh::default(); 4];
        for t in tmu.iter_mut() {
            t.tcor = 0xffff_ffff;
            t.tcnt = 0xffff_ffff;
        }
        OnChip {
            tra: 0,
            expevt: 0,
            intevt: 0,
            sq: [[0; 8]; 2],
            intc: Intc::new(),
            tmu,
            tocr: 0,
            pclk_div: 4,
            pclk_hz: 58_800_000,
            dma: [DmaCh::default(); 8],
            dmaor: 0,
            scif_scr: 0,
            scif_smr: 0,
            scif_fcr: 0,
            scif_brr: 0xff,
            scif_sptr: 0,
            scif_rx: VecDeque::new(),
            scif_tx: Vec::new(),
            sci_scr: 0,
            sci_smr: 0,
            sci_brr: 0xff,
            sci_ssr: 0x84,
            sci_sptr: 0,
            sci_rx: VecDeque::new(),
            sci_tx: Vec::new(),
            bsc: [0; 32],
            pctra: 0,
            pdtra: 0,
            pdtra_in: 0xffff,
            pctrb: 0,
            pdtrb: 0,
            pdtrb_in: 0xffff,
            rfcr: 0,
            frqcr: 0x0e0a,
            stbcr: 0,
            stbcr2: 0,
            wtcnt: 0,
            wtcsr: 0,
            rtc: [0; 16],
            rtc_last: 0,
            next_event: u64::MAX,
            tcpr2: 0,
            log: IoLog::new(false, 4),
            navi: None,
        }
    }

    // ---- TMU ------------------------------------------------------------

    fn tmu_cpt(&self, ch: usize) -> u64 {
        let div = match self.tmu[ch].tcr & 7 {
            0 => 4,
            1 => 16,
            2 => 64,
            3 => 256,
            4 => 1024,
            _ => 1024,
        };
        self.pclk_div * div
    }

    fn tmu_sync(&mut self, ch: usize, now: u64) {
        let cpt = self.tmu_cpt(ch);
        let t = &mut self.tmu[ch];
        if !t.running {
            t.last = now;
            return;
        }
        let ticks = (now.saturating_sub(t.last)) / cpt;
        if ticks == 0 {
            return;
        }
        t.last += ticks * cpt;
        if ticks > t.tcnt as u64 {
            let rem = ticks - t.tcnt as u64 - 1;
            let period = t.tcor as u64 + 1;
            t.tcnt = (t.tcor as u64 - (rem % period)) as u32;
            t.tcr |= 0x100; // UNF
        } else {
            t.tcnt -= ticks as u32;
        }
    }

    /// An edge on the TCLK pin: with input capture enabled (TCR2 bits
    /// 7:6), timer 2's counter is copied to TCPR2 and the capture flag
    /// set, with an interrupt if both bits are set.  The RNS-E has the
    /// car's speed pulses on this pin.
    pub fn tmu2_capture(&mut self, now: u64) {
        self.tmu_sync(2, now);
        if self.tmu[2].tcr & 0xc0 != 0 {
            self.tcpr2 = self.tmu[2].tcnt;
            self.tmu[2].tcr |= 0x200;
            self.tmu_irq();
        }
    }

    fn tmu_irq(&mut self) {
        const IRQS: [Irq; 4] = [Irq::Tuni0, Irq::Tuni1, Irq::Tuni2, Irq::Tuni3];
        for ch in 0..4 {
            let on = self.tmu[ch].tcr & 0x120 == 0x120;
            self.intc.set(IRQS[ch], on);
        }
        self.intc.set(Irq::Ticpi2, self.tmu[2].tcr & 0x2c0 == 0x2c0);
    }

    fn reschedule(&mut self, now: u64) {
        let mut next = u64::MAX;
        for ch in 0..4 {
            let t = &self.tmu[ch];
            if t.running {
                let at = t.last + (t.tcnt as u64 + 1) * self.tmu_cpt(ch);
                next = next.min(at);
            }
        }
        for d in self.dma.iter() {
            if d.busy {
                next = next.min(d.done_at);
            }
        }
        // The RTC ticks along at a coarse rate.
        next = next.min(self.rtc_last + self.rtc_period());
        self.next_event = next.max(now + 1);
    }

    fn rtc_period(&self) -> u64 {
        // One emulated second.
        self.pclk_div * self.pclk_hz
    }

    /// Called by the CPU when `cycles >= next_event`.
    pub fn run_events<B: Bus>(&mut self, _bus: &mut B, now: u64) {
        for ch in 0..4 {
            self.tmu_sync(ch, now);
        }
        self.tmu_irq();
        for ch in 0..8 {
            if self.dma[ch].busy && now >= self.dma[ch].done_at {
                self.dma[ch].busy = false;
                self.dma[ch].chcr |= 2; // TE
                self.dma_irq();
            }
        }
        while now >= self.rtc_last + self.rtc_period() {
            self.rtc_last += self.rtc_period();
            self.rtc_tick();
        }
        self.reschedule(now);
    }

    // ---- RTC ------------------------------------------------------------

    fn bcd_inc(v: &mut u8, max: u8, min: u8) -> bool {
        let mut d = (*v >> 4) * 10 + (*v & 0xf) + 1;
        let carry = d > max;
        if carry {
            d = min;
        }
        *v = ((d / 10) << 4) | (d % 10);
        carry
    }

    fn rtc_tick(&mut self) {
        // 1=sec 2=min 3=hour 4=wk 5=day 6=mon 7=year
        if self.rtc[15] & 0x08 == 0 {
            // RCR2.START clear: clock stopped
            return;
        }
        if Self::bcd_inc(&mut self.rtc[1], 59, 0)
            && Self::bcd_inc(&mut self.rtc[2], 59, 0)
            && Self::bcd_inc(&mut self.rtc[3], 23, 0)
        {
            self.rtc[4] = (self.rtc[4] + 1) % 7;
            if Self::bcd_inc(&mut self.rtc[5], 28, 1) && Self::bcd_inc(&mut self.rtc[6], 12, 1) {
                Self::bcd_inc(&mut self.rtc[7], 99, 0);
            }
        }
    }

    /// Set the RTC from broken-down time.
    pub fn rtc_set(&mut self, year: u32, mon: u32, day: u32, hour: u32, min: u32, sec: u32) {
        let bcd = |v: u32| (((v / 10) << 4) | (v % 10)) as u8;
        self.rtc[1] = bcd(sec);
        self.rtc[2] = bcd(min);
        self.rtc[3] = bcd(hour);
        self.rtc[5] = bcd(day);
        self.rtc[6] = bcd(mon);
        self.rtc[7] = bcd(year % 100);
        self.rtc[15] = 0x09;
    }

    // ---- DMAC -----------------------------------------------------------

    fn dma_irq(&mut self) {
        const IRQS: [Irq; 8] = [
            Irq::Dmte0,
            Irq::Dmte1,
            Irq::Dmte2,
            Irq::Dmte3,
            Irq::Dmte4,
            Irq::Dmte5,
            Irq::Dmte6,
            Irq::Dmte7,
        ];
        for ch in 0..8 {
            let c = self.dma[ch].chcr;
            self.intc.set(IRQS[ch], c & 0x6 == 0x6);
        }
    }

    fn dma_rw<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u32, write: Option<u32>, now: u64) -> u32 {
        if addr >= 0xff00_0000 || (addr & 0x1fff_ffff) >= 0x1f00_0000 {
            let a = addr | 0xe000_0000;
            match write {
                Some(v) => {
                    self.write(bus, a, size, v, now, 0);
                    0
                }
                None => self.read(bus, a, size, now, 0),
            }
        } else {
            let a = addr & 0x1fff_ffff;
            match (write, size) {
                (Some(v), 1) => {
                    bus.write8(a, v as u8);
                    0
                }
                (Some(v), 2) => {
                    bus.write16(a, v as u16);
                    0
                }
                (Some(v), _) => {
                    bus.write32(a, v);
                    0
                }
                (None, 1) => bus.read8(a) as u32,
                (None, 2) => bus.read16(a) as u32,
                (None, _) => bus.read32(a),
            }
        }
    }

    /// Run up to `max_units` transfer units on a channel.  Returns the
    /// number of units moved.  Used both for auto-request transfers and by
    /// devices that model a DREQ line.
    pub fn dma_transfer<B: Bus>(&mut self, bus: &mut B, ch: usize, max_units: u32, now: u64) -> u32 {
        let c = self.dma[ch].chcr;
        if c & 1 == 0 || c & 2 != 0 || self.dmaor & 7 != 1 || self.dma[ch].busy {
            return 0;
        }
        let ts = (c >> 4) & 7;
        let (unit, parts): (u32, u32) = match ts {
            0 => (4, 2),
            1 => (1, 1),
            2 => (2, 1),
            3 => (4, 1),
            _ => (4, 8),
        };
        let step = unit * parts;
        let sm = (c >> 12) & 3;
        let dm = (c >> 14) & 3;
        let mut n = 0;
        let mut count = if self.dma[ch].tcr == 0 { 0x0100_0000 } else { self.dma[ch].tcr };
        while count > 0 && n < max_units {
            let (sar, dar) = (self.dma[ch].sar, self.dma[ch].dar);
            for p in 0..parts {
                let v = self.dma_rw(bus, sar.wrapping_add(p * unit), unit, None, now);
                self.dma_rw(bus, dar.wrapping_add(p * unit), unit, Some(v), now);
            }
            match sm {
                1 => self.dma[ch].sar = sar.wrapping_add(step),
                2 => self.dma[ch].sar = sar.wrapping_sub(step),
                _ => {}
            }
            match dm {
                1 => self.dma[ch].dar = dar.wrapping_add(step),
                2 => self.dma[ch].dar = dar.wrapping_sub(step),
                _ => {}
            }
            count -= 1;
            n += 1;
        }
        self.dma[ch].tcr = count & 0x00ff_ffff;
        if count == 0 {
            self.dma[ch].busy = true;
            self.dma[ch].done_at = now + 32 + n as u64 * 2;
            self.next_event = self.next_event.min(self.dma[ch].done_at);
        }
        n
    }

    /// A transfer whose data has been moved but whose end is still to
    /// come (it is given the time its bus cycles take) ends now.  For
    /// when the software looks at the channel or sets it up again in
    /// the meantime: the end, when it came, would otherwise be marked
    /// on the next transfer, which then never starts.  That happened
    /// to the drive's channel when the 192's software read two long
    /// pieces of a disc one right after the other.
    fn dma_finish(&mut self, ch: usize) {
        if self.dma[ch].busy {
            self.dma[ch].busy = false;
            self.dma[ch].chcr |= 2; // TE
            self.dma_irq();
        }
    }

    fn dma_kick<B: Bus>(&mut self, bus: &mut B, now: u64) {
        for ch in 0..8 {
            let c = self.dma[ch].chcr;
            if c & 3 != 1 {
                continue;
            }
            let rs = (c >> 8) & 0xf;
            match rs {
                // Auto request, and transmit-side serial requests (the
                // transmitter is always ready).
                4..=6 | 8 | 10 => {
                    self.dma_transfer(bus, ch, u32::MAX, now);
                }
                _ => {}
            }
        }
    }

    // ---- serial ---------------------------------------------------------

    fn scif_irq(&mut self) {
        let tie = self.scif_scr & 0x80 != 0;
        let rie = self.scif_scr & 0x40 != 0;
        self.intc.set(Irq::ScifTxi, tie);
        self.intc.set(Irq::ScifRxi, rie && !self.scif_rx.is_empty());
    }

    fn sci_irq(&mut self) {
        let tie = self.sci_scr & 0x80 != 0;
        let rie = self.sci_scr & 0x40 != 0;
        let teie = self.sci_scr & 0x04 != 0;
        self.intc.set(Irq::SciTxi, tie && self.sci_ssr & 0x80 != 0);
        self.intc.set(Irq::SciRxi, rie && self.sci_ssr & 0x40 != 0);
        self.intc.set(Irq::SciTei, teie && self.sci_ssr & 0x04 != 0);
    }

    /// Queue bytes for the firmware to receive on the SCIF.
    pub fn scif_receive(&mut self, data: &[u8]) {
        self.scif_rx.extend(data.iter().copied());
        self.scif_irq();
    }

    /// Queue bytes for the firmware to receive on the SCI.
    pub fn sci_receive(&mut self, data: &[u8]) {
        self.sci_rx.extend(data.iter().copied());
        if !self.sci_rx.is_empty() {
            self.sci_ssr |= 0x40;
        }
        self.sci_irq();
    }

    // ---- register access --------------------------------------------------

    pub fn read<B: Bus>(&mut self, _bus: &mut B, addr: u32, size: u32, now: u64, pc: u32) -> u32 {
        if self.navi.is_some() {
            match addr {
                0xffd8_1004 => return self.tmu[3].running as u32,
                0xffd8_1008 | 0xffd8_100c | 0xffd8_1010 => {
                    self.tmu_sync(3, now);
                    self.tmu_irq();
                    return match addr {
                        0xffd8_1008 => self.tmu[3].tcor,
                        0xffd8_100c => self.tmu[3].tcnt,
                        _ => self.tmu[3].tcr as u32,
                    };
                }
                _ => {}
            }
        }
        if let Some(n) = self.navi.as_mut() {
            if let (Some(int2), 0xff80_4000..=0xff80_41ff) = (self.intc.int2.as_ref(), addr) {
                let v = match addr & 0x1ff {
                    o @ 0x00..=0x2f => int2.pri[(o >> 2) as usize],
                    0x38 => int2.mask as u32,
                    0x3c => !int2.mask as u32,
                    o @ (0x30 | 0x34 | 0x100 | 0x104) => int2.status(o),
                    0x108 => (int2.mask >> 32) as u32,
                    0x10c => !(int2.mask >> 32) as u32,
                    _ => 0,
                };
                n.log.log("intc", false, addr, size, v, pc);
                return v;
            }
            if super::navi::Navi::owns(addr) {
                return n.read(addr, size, now, pc, &mut self.intc);
            }
        }
        let v = match addr {
            0xff00_0020 => self.tra,
            0xff00_0024 => self.expevt,
            0xff00_0028 => self.intevt,
            // What a 192 answers (its report, docs/custom-firmware.md,
            // "On a real unit"); the emulator used to say 0x04050005,
            // 0x35100000 and 0x100, an SH7750R's by the book.
            0xff00_0030 => 0x0405_0086, // PVR
            0xff00_0040 => 0x2048_0000, // CVR
            0xff00_0044 => 0x0000_0200, // PRR
            // Timer channels 3 and 4, which nothing uses: the reload
            // and the counter as after a reset.
            0xfe10_0008 | 0xfe10_000c | 0xfe10_0014 | 0xfe10_0018 => 0xffff_ffff,

            // BSC
            0xff80_0028 => {
                // RFCR: refresh count; advance on every read so that
                // start-up delay loops terminate.
                self.rfcr = (self.rfcr + 1) & 0x3ff;
                self.rfcr as u32
            }
            0xff80_002c => self.pctra,
            0xff80_0030 => {
                // Bits configured as outputs read back the latch.
                let mut out_mask = 0u16;
                for b in 0..16 {
                    if self.pctra >> (b * 2) & 1 != 0 {
                        out_mask |= 1 << b;
                    }
                }
                ((self.pdtra & out_mask) | (self.pdtra_in & !out_mask)) as u32
            }
            0xff80_0040 => self.pctrb,
            0xff80_0044 => {
                let mut out_mask = 0u16;
                for b in 0..4 {
                    if self.pctrb >> (b * 2) & 1 != 0 {
                        out_mask |= 1 << b;
                    }
                }
                ((self.pdtrb & out_mask) | (self.pdtrb_in & !out_mask)) as u32
            }
            // RTCSR: while the refresh timer counts (a clock is selected)
            // its compare match and its overflow of the refresh count
            // come round all the time; the 192's boot ROM clears the
            // overflow flag and waits for it four times before it goes
            // on.  The timer itself is not modelled: both flags read as
            // set whenever it runs.
            0xff80_001c => {
                let v = self.bsc[7];
                if v & 0x38 != 0 { v | 0x84 } else { v }
            }
            0xff80_0000..=0xff80_007f => self.bsc[((addr & 0x7f) >> 2) as usize],

            // DMAC
            0xffa0_0040 => self.dmaor,
            0xffa0_0000..=0xffa0_008f => {
                let off = addr & 0xff;
                let ch = if off < 0x40 { (off >> 4) as usize } else { (off >> 4) as usize - 1 };
                self.dma_finish(ch);
                let d = &self.dma[ch];
                match off & 0xc {
                    0 => d.sar,
                    4 => d.dar,
                    8 => d.tcr,
                    _ => d.chcr,
                }
            }

            // CPG
            0xffc0_0000 => self.frqcr as u32,
            0xffc0_0004 => self.stbcr as u32,
            0xffc0_0008 => self.wtcnt as u32,
            0xffc0_000c => self.wtcsr as u32,
            0xffc0_0010 => self.stbcr2 as u32,

            // RTC
            0xffc8_0000..=0xffc8_003f => {
                let i = ((addr & 0x3f) >> 2) as usize;
                if i == 0 {
                    // R64CNT: sub-second counter
                    ((now / (self.rtc_period() / 64).max(1)) & 0x7f) as u32
                } else {
                    self.rtc[i] as u32
                }
            }

            // INTC
            // ICR.  Bit 15 is the level on the NMI pin, which is high
            // on a 192 that runs (a unit's report has 0x8000 here); its
            // boot ROM looks at it first thing after a reset and goes
            // off to wait when it is low.  The 193's software does not
            // read the bit.
            0xffd0_0000 => self.intc.icr as u32 | if self.intc.int2.is_none() { 0x8000 } else { 0 },
            0xffd0_0004 => self.intc.ipra as u32,
            0xffd0_0008 => self.intc.iprb as u32,
            0xffd0_000c => self.intc.iprc as u32,
            0xffd0_0010 => self.intc.iprd as u32,
            0xfe08_0000 => self.intc.intpri00,
            0xfe08_0020 => 0,
            0xfe08_0040 => self.intc.intmsk00,

            // TMU
            0xffd8_0000 => self.tocr as u32,
            0xffd8_0004 => {
                (self.tmu[0].running as u32) | (self.tmu[1].running as u32) << 1 | (self.tmu[2].running as u32) << 2
            }
            0xffd8_0008..=0xffd8_002b => {
                let off = addr - TMU_BASE - 8;
                let ch = (off / 12) as usize;
                self.tmu_sync(ch, now);
                self.tmu_irq();
                match off % 12 {
                    0 => self.tmu[ch].tcor,
                    4 => self.tmu[ch].tcnt,
                    _ => self.tmu[ch].tcr as u32,
                }
            }
            0xffd8_002c => self.tcpr2,

            // SCI
            0xffe0_0000 => self.sci_smr as u32,
            0xffe0_0004 => self.sci_brr as u32,
            0xffe0_0008 => self.sci_scr as u32,
            0xffe0_000c => 0xff,
            0xffe0_0010 => self.sci_ssr as u32,
            0xffe0_0014 => self.sci_rx.front().copied().unwrap_or(0) as u32,
            0xffe0_0018 => 0,
            0xffe0_001c => self.sci_sptr as u32,

            // SCIF
            0xffe8_0000 => self.scif_smr as u32,
            0xffe8_0004 => self.scif_brr as u32,
            0xffe8_0008 => self.scif_scr as u32,
            0xffe8_0010 => 0x0060 | if self.scif_rx.is_empty() { 0 } else { 0x0002 },
            0xffe8_0014 => {
                let b = self.scif_rx.pop_front().unwrap_or(0);
                self.scif_irq();
                b as u32
            }
            0xffe8_0018 => self.scif_fcr as u32,
            0xffe8_001c => self.scif_rx.len().min(16) as u32,
            0xffe8_0020 => self.scif_sptr as u32,
            0xffe8_0024 => 0,

            _ => {
                self.log.log("p4?", false, addr, size, 0, pc);
                return 0;
            }
        };
        self.log.log("onchip", false, addr, size, v, pc);
        v
    }

    pub fn write<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u32, v: u32, now: u64, pc: u32) {
        if self.navi.is_some() {
            match addr {
                0xffd8_1004 => {
                    self.tmu_sync(3, now);
                    let run = v & 1 != 0;
                    if run && !self.tmu[3].running { self.tmu[3].last = now; }
                    self.tmu[3].running = run;
                    self.tmu_irq();
                    self.reschedule(now);
                    return;
                }
                0xffd8_1008 | 0xffd8_100c | 0xffd8_1010 => {
                    self.tmu_sync(3, now);
                    match addr {
                        0xffd8_1008 => self.tmu[3].tcor = v,
                        0xffd8_100c => self.tmu[3].tcnt = v,
                        _ => {
                            let unf = self.tmu[3].tcr & v as u16 & 0x100;
                            self.tmu[3].tcr = (v as u16 & 0x2ff & !0x100) | unf;
                        }
                    }
                    self.tmu_irq();
                    self.reschedule(now);
                    return;
                }
                _ => {}
            }
        }
        if let Some(n) = self.navi.as_mut() {
            if let (Some(int2), 0xff80_4000..=0xff80_41ff) = (self.intc.int2.as_mut(), addr) {
                n.log.log("intc", true, addr, size, v, pc);
                match addr & 0x1ff {
                    o @ 0x00..=0x2f => int2.pri[(o >> 2) as usize] = v,
                    0x38 => int2.mask |= v as u64,
                    0x3c => int2.mask &= !(v as u64),
                    0x108 => int2.mask |= (v as u64) << 32,
                    0x10c => int2.mask &= !((v as u64) << 32),
                    _ => {}
                }
                self.intc.recalc();
                return;
            }
            if super::navi::Navi::owns(addr) {
                n.write(addr, size, v, now, pc, &mut self.intc);
                // The SD host interface's data goes out by DMA.
                if matches!(addr, 0xff80_1200..=0xff80_14ff | 0xff90_a000..=0xff90_a0ff) {
                    n.dma_run(bus, &mut self.intc);
                }
                return;
            }
        }
        self.log.log("onchip", true, addr, size, v, pc);
        match addr {
            0xff00_0020 => self.tra = v & 0x3fc,
            0xff00_0024 => self.expevt = v & 0xfff,
            0xff00_0028 => self.intevt = v & 0xfff,

            // BSC
            0xff80_0028 => self.rfcr = (v & 0x3ff) as u16,
            0xff80_002c => self.pctra = v,
            0xff80_0030 => self.pdtra = v as u16,
            0xff80_0040 => self.pctrb = v,
            0xff80_0044 => self.pdtrb = v as u16,
            // The refresh timer's registers take a byte, and only with
            // 0xa5 above it.
            0xff80_001c | 0xff80_0020 | 0xff80_0024 => {
                if v >> 8 == 0xa5 {
                    self.bsc[((addr & 0x7f) >> 2) as usize] = v & 0xff;
                }
            }
            0xff80_0000..=0xff80_007f => self.bsc[((addr & 0x7f) >> 2) as usize] = v,
            0xff90_0000..=0xff9f_ffff => {} // SDMR2/SDMR3: SDRAM mode register set

            // DMAC
            0xffa0_0040 => {
                // AE and NMIF can only be cleared.
                let keep = self.dmaor & v & 0x6;
                // SH7750R: DDT, DBL (eight DDT channels), priority and
                // master enable. The 193 uses its existing register mask.
                let mask = if self.navi.is_some() { 0x8201 } else { 0xc301 };
                self.dmaor = (v & mask) | keep;
                self.dma_kick(bus, now);
            }
            0xffa0_0000..=0xffa0_008f => {
                let off = addr & 0xff;
                let ch = if off < 0x40 { (off >> 4) as usize } else { (off >> 4) as usize - 1 };
                self.dma_finish(ch);
                match off & 0xc {
                    0 => self.dma[ch].sar = v,
                    4 => self.dma[ch].dar = v,
                    8 => self.dma[ch].tcr = v & 0x00ff_ffff,
                    _ => {
                        // TE can only be cleared by software.  Bit 3 on
                        // the SH7750R clears the request queue and
                        // reads as 0: the 192's software sets it after
                        // every transfer.
                        let te = self.dma[ch].chcr & v & 2;
                        let v = if self.navi.is_none() { v & !8 } else { v };
                        self.dma[ch].chcr = (v & !2) | te;
                        self.dma_irq();
                        self.dma_kick(bus, now);
                    }
                }
            }

            // CPG
            0xffc0_0000 => self.frqcr = v as u16,
            0xffc0_0004 => self.stbcr = v as u8,
            0xffc0_0008 => {
                if v & 0xff00 == 0x5a00 {
                    self.wtcnt = v as u8;
                }
            }
            0xffc0_000c => {
                if v & 0xff00 == 0xa500 {
                    self.wtcsr = v as u8;
                }
            }
            0xffc0_0010 => self.stbcr2 = v as u8,

            // RTC
            0xffc8_0000..=0xffc8_003f => {
                let i = ((addr & 0x3f) >> 2) as usize;
                self.rtc[i] = v as u8;
            }

            // INTC
            0xffd0_0000 => self.intc.icr = v as u16,
            0xffd0_0004 => {
                self.intc.ipra = v as u16;
                self.intc.recalc();
            }
            0xffd0_0008 => {
                self.intc.iprb = v as u16;
                self.intc.recalc();
            }
            0xffd0_000c => {
                self.intc.iprc = v as u16;
                self.intc.recalc();
            }
            0xffd0_0010 => {
                self.intc.iprd = v as u16;
                self.intc.recalc();
            }
            0xfe08_0000 => {
                self.intc.intpri00 = v;
                self.intc.recalc();
            }
            0xfe08_0040 => self.intc.intmsk00 |= v & 0x300,
            0xfe08_0060 => self.intc.intmsk00 &= !v,

            // TMU
            0xffd8_0000 => self.tocr = v as u8,
            0xffd8_0004 => {
                for ch in 0..3 {
                    self.tmu_sync(ch, now);
                    let run = v >> ch & 1 != 0;
                    if run && !self.tmu[ch].running {
                        self.tmu[ch].last = now;
                    }
                    self.tmu[ch].running = run;
                }
                self.tmu_irq();
                self.reschedule(now);
            }
            0xffd8_0008..=0xffd8_002b => {
                let off = addr - TMU_BASE - 8;
                let ch = (off / 12) as usize;
                self.tmu_sync(ch, now);
                match off % 12 {
                    0 => self.tmu[ch].tcor = v,
                    4 => self.tmu[ch].tcnt = v,
                    _ => {
                        // UNF (bit 8) and the capture flag (bit 9) can
                        // only be cleared by writing 0.
                        let flags = self.tmu[ch].tcr & (v as u16) & 0x300;
                        self.tmu[ch].tcr = (v as u16 & 0x0ff) | flags;
                    }
                }
                self.tmu_irq();
                self.reschedule(now);
            }
            0xffd8_002c => {}

            // SCI
            0xffe0_0000 => self.sci_smr = v as u8,
            0xffe0_0004 => self.sci_brr = v as u8,
            0xffe0_0008 => {
                self.sci_scr = v as u8;
                self.sci_irq();
            }
            0xffe0_000c => {
                self.sci_tx.push(v as u8);
                self.sci_ssr |= 0x84;
                self.sci_irq();
            }
            0xffe0_0010 => {
                // Flags are cleared by writing 0; TDRE/TEND stay set since
                // transmission is instantaneous here.
                let v = v as u8;
                if v & 0x40 == 0 && self.sci_ssr & 0x40 != 0 {
                    self.sci_rx.pop_front();
                    if self.sci_rx.is_empty() {
                        self.sci_ssr &= !0x40;
                    }
                }
                self.sci_ssr = (self.sci_ssr & 0xc4) | (self.sci_ssr & v & 0x38);
                self.sci_irq();
            }
            0xffe0_0018 => {}
            0xffe0_001c => self.sci_sptr = v as u8,

            // SCIF
            0xffe8_0000 => self.scif_smr = v as u16,
            0xffe8_0004 => self.scif_brr = v as u8,
            0xffe8_0008 => {
                self.scif_scr = v as u16;
                self.scif_irq();
            }
            0xffe8_000c => self.scif_tx.push(v as u8),
            0xffe8_0010 => {}
            0xffe8_0018 => {
                self.scif_fcr = v as u16;
                if v & 2 != 0 {
                    self.scif_rx.clear();
                    self.scif_irq();
                }
            }
            0xffe8_0020 => self.scif_sptr = v as u16,
            0xffe8_0024 => {}

            _ => {
                self.log.log("p4?", true, addr, size, v, pc);
            }
        }
    }
}
