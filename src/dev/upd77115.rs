//! NEC uPD77115: the signal processor behind each SD card slot of the
//! 192 (two of them on the card board; `docs/hardware.md`, "The boards"
//! and "SD card processors").
//!
//! A 16-bit fixed-point processor of the uPD77016 family: instructions
//! of 32 bits, eight registers of 40 bits, two data memories (X and Y)
//! of 16-bit words with four pointers each.  The instruction set is from
//! NEC's "uPD77016 Family Instructions" manual (U13116E), the part's
//! memory map, peripherals and boot procedure from the "uPD77111 Family
//! Architecture" manual (U14623E), chapters 5 to 7.
//!
//! Instruction memory:
//!
//! | range         | contents |
//! |---------------|----------|
//! | 0x0000-0x00ff | boot ROM (mask ROM, not on any disc: its services are performed here, see [`Dsp::rom`]) |
//! | 0x0200-0x023f | vectors, four instructions each: reset, three reserved, INT1-4, serial in, serial out, card data in / buffer, card data out, host in, host out, card response, timer |
//! | 0x0240-0x0fff | RAM, loaded by the boot ROM at reset |
//! | 0x8000-0x9fff | RAM, loaded by the boot ROM's "reboot" service |
//!
//! Data memory, the same in X and Y: RAM at 0x0000-0x0fff, 0x2000-0x2fff
//! and 0x4000-0x5fff, the peripheral registers at 0x3800-0x383f.
//!
//! Time is counted in instruction cycles; a jump or call takes two or
//! three, everything else one.
//!
//! The instruction manual shows where the fields of an instruction word
//! are, but not all of their codes.  These are from the program the
//! 192's software loads (where each can be told from what the program
//! goes on to do with the result), not from NEC:
//!
//! * operations with an immediate operand are numbered like the same
//!   operations between registers (8 add, 9 subtract, 10 and, 11 or,
//!   12 exclusive or, 13-15 the shifts);
//! * the one-operand operations run 0 nop, 1 clear, 2 increment,
//!   3 decrement, 4 absolute value, 5 complement, 6 negate, 7 clip,
//!   8 round, 9 exponent, then 12 copy, 13 divide step, 14 add to,
//!   15 subtract from (5, 10 and 11 do not occur in that program; 5 is
//!   by the manual's order);
//! * the part of a register a load or store names is a mask: 1 = L,
//!   2 = H, 4 = E, 6 = E and H (a load that extends the sign), 7 = all
//!   of it (the same, with L cleared);
//! * registers in transfers: 0-7 DP0-DP7, 8-15 DN0-DN7, 16 DMX, 20 DMY,
//!   32 SR, 33 EIR, 34 the stack (a store pushes, a load pops), 38 the
//!   loop counter.  The others (SP, LSP, the loop stack, ESR) are not
//!   used by that program and stop the model with a message;
//! * conditions: 0 always, 2 = 0, 3 != 0, 4 > 0, 5 <= 0, 6 >= 0, 7 < 0,
//!   8 and 9 "extension" and its opposite.
//!
//! Not modelled: the pipeline's effects the manual forbids programs to
//! rely on (a pointer set by one instruction and used by the next),
//! the three instructions an interrupt enable takes to act, bit-reversed
//! addressing beyond the reversal itself, the serial input.

use std::collections::VecDeque;
use std::fmt::Write as _;

use super::sdcard::{Response, SdCard};

/// The crystal on the 192's card board: the processor's clock input,
/// and (as the emulator takes it) the audio master clock, 512 periods
/// of which are a sample at 44.1 kHz.
pub const CRYSTAL_HZ: u64 = 22_579_200;

/// The levels on the port pins when nothing drives them, as the
/// emulator takes them to be on the 192's card board.  Pins 7-4 are
/// read at reset as the clock multiplier, pins 1 and 0 as the boot
/// mode; neither can be seen on the photographs of the board.
///
/// * Multiplier 3 (pins 5 and 4 high), for 67.7 MHz under the part's
///   75 MHz: the 192's program sets the timer to 53 counts of the clock
///   divided by 128 and counts its timeouts in those ticks, which are
///   100 microseconds at that factor.
/// * Pins 1 and 0 high, "host word boot", which is what the 192's
///   software sends.
/// * Pin 3 high: see `docs/hardware.md`; pin 2 pulled up, being an
///   active-low request once the program makes it an output.
///
/// `sdk/examples/report` reads the pins on a real unit.
pub const PINS_192: u8 = 0x3f;

/// Divisors of the SD card clock, by the DIV field of SDCTL.
const SD_DIV: [u64; 16] = [1, 2, 3, 4, 5, 7, 8, 9, 10, 12, 16, 32, 64, 128, 256, 512];

/// Interrupt sources, as bit numbers of the enable flags in SR; the
/// vector of source n is 0x210 + 4n.
pub mod irq {
    pub const INT1: u32 = 0;
    pub const INT2: u32 = 1;
    pub const INT3: u32 = 2;
    pub const INT4: u32 = 3;
    pub const SI: u32 = 4;
    pub const SO: u32 = 5;
    /// Card data in, or the end of a buffered transfer.
    pub const SDDAT_IN: u32 = 6;
    pub const SDDAT_OUT: u32 = 7;
    pub const HI: u32 = 8;
    pub const HO: u32 = 9;
    pub const SDCR: u32 = 10;
    pub const TIMER: u32 = 11;
}

/// Host status register bits.
const HST_HBM: u16 = 0x800;
const HST_HAWE: u16 = 0x400;
const HST_HREM: u16 = 0x200;
const HST_HWEM: u16 = 0x100;
const HST_HRER: u16 = 0x20;
const HST_HWER: u16 = 0x10;
const HST_HSER: u16 = 8;
const HST_HLER: u16 = 4;
const HST_HREF: u16 = 2;
const HST_HWEF: u16 = 1;

const ESR_OVF: u16 = 8;
const ESR_STE: u16 = 4;
const ESR_LSE: u16 = 2;

/// The processor's registers: everything an instruction can change
/// other than memory, so that an instruction that has to wait for the
/// host can be taken back.
#[derive(Clone, Copy, Debug)]
pub struct Regs {
    /// R0-R7, 40 bits each, kept sign-extended.
    pub r: [i64; 8],
    pub dp: [u16; 8],
    pub dn: [u16; 8],
    /// DMX, DMY.
    pub dm: [u16; 2],
    pub pc: u16,
    pub sp: u16,
    pub stk: [u16; 16],
    /// Interrupt enable flags for each source (0 = enabled), bits 11-0.
    pub sr: u16,
    /// Stack of the global enable flag; bits 15-13 are SR's EI, EP, EB.
    pub eir: u16,
    pub esr: u16,
    /// Loop counter; bit 15 set = not in a loop.
    pub lc: u16,
    pub lsa: u16,
    pub lea: u16,
    pub lsp: u16,
    /// Loop stack: LSA, LEA, LC of the loops outside the current one.
    pub lstk: [[u16; 3]; 8],
}

impl Default for Regs {
    fn default() -> Self {
        Regs {
            r: [0; 8], dp: [0; 8], dn: [0; 8], dm: [0; 2], pc: 0, sp: 0, stk: [0; 16],
            sr: 0, eir: 0xffff, esr: 0, lc: 0x8000, lsa: 0, lea: 0, lsp: 0, lstk: [[0; 3]; 8],
        }
    }
}

/// What the boot ROM is doing, when it is the boot ROM that runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Rom {
    /// The user program runs.
    None,
    /// Reset boot by words from the host: a dummy word, the number of
    /// instructions, the handshake setting for HST, then the instructions,
    /// low half first, to 0x200 on.
    Dummy,
    Count,
    Handshake,
    /// Instructions still to come, where the next goes, and the low half
    /// of it when that has arrived.
    Code { left: u16, at: u16, low: Option<u16>, reboot: bool },
}

/// The SD card interface.
#[derive(Default)]
struct Sd {
    idx: u16,
    agh: u16,
    agl: u16,
    /// SDCTL: the clock division, the stop bit and the error flags.
    ctl: u16,
    /// A command is on the line until then.
    cen_until: u64,
    /// The response register's queue of up to eight words.
    rpr: VecDeque<u16>,
    /// A response on its way from the card, and when it is complete.
    response: Option<(u64, Vec<u16>)>,
    /// A block of data on its way.
    block: Option<(u64, Vec<u16>)>,
    /// Data words that have arrived and not been taken.
    words: VecDeque<u16>,
    // The buffer transfer ("PBU"): start address, size, words left,
    // pointer, control.
    psar: u16,
    psr: u16,
    pscr: u16,
    ppr: u16,
    pcr: u16,
}

pub struct Dsp {
    pub regs: Regs,
    pub imem: Vec<u32>,
    pub x: Vec<u16>,
    pub y: Vec<u16>,
    /// Interrupt requests recorded and not yet taken (bits as in SR).
    pub pending: u16,
    pub halted: bool,
    /// Stopped for good: an instruction or a boot ROM entry this model
    /// does not have.
    pub fault: Option<String>,
    rom: Rom,
    /// Repetitions still to come of the instruction after a REP.
    repeat: u32,
    /// Set by a memory access that has to wait for the host.
    wait: bool,
    /// The instruction at PC is waiting for the host.
    pub waiting: bool,
    /// The STOP instruction has stopped the clock; the wake-up pin
    /// starts it again.
    pub stopped: bool,
    // Host interface.
    hdt_in: u16,
    hdt_out: u16,
    hst: u16,
    // Port: output latch, direction (1 = output), and the levels on the
    // pins that are inputs.
    port_out: u8,
    port_dir: u8,
    pub pins: u8,
    /// Port pin 2 went low since this was last taken.  The 192's program
    /// pulls it low while a word that starts a message waits for the
    /// host: it is the interrupt request to the main processor.
    pub host_irq: bool,
    // Timer.
    tir: u16,
    tcsr: u16,
    tenr: u16,
    timer_at: u64,
    // Audio serial interface.
    sst: u16,
    asst: u16,
    asdt: [u16; 4],
    asdt_n: usize,
    last_sample: [i16; 2],
    frame_at: u64,
    /// Samples sent to the converter, left and right in turn, at
    /// [`Dsp::sample_rate`]; whoever listens takes them from here.
    pub audio: VecDeque<i16>,
    pub frames: u64,
    /// How many of them were not silence.
    pub loud_frames: u64,
    // SD card interface and its buffer transfer.
    pub card: Option<SdCard>,
    sd: Sd,
    /// Peripheral registers this model only stores.
    pub io: [u16; 64],
    next_event: u64,
    pub cycles: u64,
    pub trace: bool,
    pub trace_io: bool,
    /// Addresses at which the registers are printed, each with a number
    /// of instructions to list from there on.
    pub tracepoints: Vec<(u16, u32)>,
    trace_left: u32,
    /// Stores to data memory in this range of addresses are printed.
    pub watch: Option<(u16, u16)>,
    /// For trace lines.
    pub name: &'static str,
}

impl Default for Dsp {
    fn default() -> Self {
        Self::new()
    }
}

fn sx40(v: i64) -> i64 {
    (v << 24) >> 24
}

impl Dsp {
    pub fn new() -> Self {
        let mut d = Dsp {
            regs: Regs::default(),
            imem: vec![0; 0x1_0000],
            x: vec![0; 0x1_0000],
            y: vec![0; 0x1_0000],
            pending: 0,
            halted: false,
            fault: None,
            rom: Rom::Dummy,
            repeat: 0,
            wait: false,
            waiting: false,
            stopped: false,
            hdt_in: 0,
            hdt_out: 0,
            hst: 0,
            port_out: 0,
            port_dir: 0,
            pins: PINS_192,
            host_irq: false,
            tir: 0xffff,
            tcsr: 0,
            tenr: 0,
            timer_at: u64::MAX,
            sst: 2,
            asst: 0x8012,
            asdt: [0; 4],
            asdt_n: 0,
            last_sample: [0; 2],
            frame_at: u64::MAX,
            audio: VecDeque::new(),
            frames: 0,
            loud_frames: 0,
            card: None,
            sd: Sd::default(),
            io: [0; 64],
            next_event: u64::MAX,
            cycles: 0,
            trace: false,
            trace_io: false,
            tracepoints: Vec::new(),
            trace_left: 0,
            watch: None,
            name: "dsp",
        };
        d.reset();
        d
    }

    /// The reset pin.  The memories keep their contents; the boot ROM
    /// then waits for a program from the host (the two mode pins of the
    /// 192's processors are taken to select "host word boot", which is
    /// what its software sends).
    pub fn reset(&mut self) {
        self.regs = Regs::default();
        self.regs.sr = 0;
        self.pending = 0;
        self.halted = false;
        self.fault = None;
        self.repeat = 0;
        self.wait = false;
        self.waiting = false;
        self.rom = Rom::Dummy;
        // Word mode with waits, ready for a word from the host, nothing
        // for the host to read.
        self.hst = HST_HBM | HST_HAWE | HST_HWEF;
        self.stopped = false;
        self.port_out = 0;
        self.port_dir = 0;
        self.host_irq = false;
        self.tir = 0xffff;
        self.tcsr = 0;
        self.tenr = 0;
        self.timer_at = u64::MAX;
        self.sst = 2;
        self.asst = 0x8012;
        self.asdt_n = 0;
        self.frame_at = u64::MAX;
        self.sd = Sd { idx: 0x4800, ctl: 0x0200, ..Sd::default() };
        self.io = [0; 64];
        self.next_event = u64::MAX;
    }

    /// The wake-up pin: the clock runs again after a STOP.
    pub fn wakeup(&mut self) {
        if self.stopped {
            self.stopped = false;
            self.halted = false;
        }
    }

    /// The clock multiplier the pins select: pins 7-4, where 0 stands
    /// for 16.
    pub fn multiplier(&self) -> u64 {
        match u64::from(self.pins >> 4) {
            0 => 16,
            n => n,
        }
    }

    /// System clocks per period of the clock input: one until the
    /// program switches over to the multiplied clock (CLKCNTL bit 7),
    /// which it may once the multiplier has had time to settle.
    fn clock_ratio(&self) -> u64 {
        if self.io[0x2e] & 0x80 != 0 { self.multiplier() } else { 1 }
    }

    /// System clocks per second at the moment.
    pub fn clock_hz(&self) -> u64 {
        CRYSTAL_HZ * self.clock_ratio()
    }

    /// Samples per second of the audio output as it is set up.
    pub fn sample_rate(&self) -> u64 {
        CRYSTAL_HZ * self.clock_ratio() / self.frame_period()
    }

    // ---- host side -------------------------------------------------

    /// The HWE pin: the host may write a word.
    pub fn host_can_write(&self) -> bool {
        self.hst & HST_HWEF != 0 && self.hst & HST_HWEM == 0
    }

    /// The HRE pin: there is a word for the host to read.
    pub fn host_can_read(&self) -> bool {
        self.hst & HST_HREF != 0 && self.hst & HST_HREM == 0
    }

    pub fn host_write(&mut self, v: u16) {
        if self.hst & HST_HWEF == 0 {
            self.hst |= HST_HWER;
        }
        self.hdt_in = v;
        self.hst &= !HST_HWEF;
        self.pending |= 1 << irq::HI;
        self.waiting = false;
        if self.rom != Rom::None {
            self.rom_word();
        }
    }

    pub fn host_read(&mut self) -> u16 {
        if self.hst & HST_HREF == 0 {
            self.hst |= HST_HRER;
        }
        self.hst &= !HST_HREF;
        self.pending |= 1 << irq::HO;
        self.waiting = false;
        self.hdt_out
    }

    /// The boot ROM takes the word the host wrote.
    fn rom_word(&mut self) {
        let v = self.hdt_in;
        self.hst |= HST_HWEF;
        self.pending &= !(1 << irq::HI);
        self.rom = match self.rom {
            Rom::None => Rom::None,
            Rom::Dummy => Rom::Count,
            Rom::Count => {
                self.regs.r[7] = i64::from(v);
                Rom::Handshake
            }
            Rom::Handshake => {
                self.hst = (v & 0x0fc0) | (self.hst & 0x3f);
                Rom::Code { left: self.regs.r[7] as u16, at: 0x200, low: None, reboot: false }
            }
            Rom::Code { left, at, low: None, reboot } => Rom::Code { left, at, low: Some(v), reboot },
            Rom::Code { left, at, low: Some(low), reboot } => {
                self.imem[at as usize] = u32::from(v) << 16 | u32::from(low);
                if left > 1 {
                    Rom::Code { left: left - 1, at: at.wrapping_add(1), low: None, reboot }
                } else {
                    if reboot {
                        self.ret();
                    } else {
                        self.regs.pc = 0x200;
                    }
                    Rom::None
                }
            }
        };
    }

    /// A call into the boot ROM.  Two of its services are documented:
    /// "host reboot" at 6 (R7L instructions from the host to the address
    /// in DP3 on) and the signature at 9.
    fn rom_call(&mut self) {
        match self.regs.pc {
            6 => {
                let left = self.regs.r[7] as u16;
                let at = self.regs.dp[3];
                if self.trace {
                    eprintln!("[{}] boot ROM: {} instructions from the host to {:04x}", self.name, left, at);
                }
                if left == 0 {
                    self.ret();
                } else {
                    self.rom = Rom::Code { left, at, low: None, reboot: true };
                    // A word the host has already written is the first.
                    if self.hst & HST_HWEF == 0 {
                        self.rom_word();
                    }
                }
            }
            9 => {
                // CRC-32 (the IEEE 802.3 polynomial) over R7L
                // instructions from DP3 on, result in R7.  Bit order and
                // start value are not documented: taken as the common
                // reflected form over the bytes of each instruction, low
                // byte first.
                let mut crc = 0xffff_ffffu32;
                for i in 0..self.regs.r[7] as u16 {
                    let w = self.imem[self.regs.dp[3].wrapping_add(i) as usize];
                    for b in w.to_le_bytes() {
                        crc ^= u32::from(b);
                        for _ in 0..8 {
                            crc = if crc & 1 != 0 { crc >> 1 ^ 0xedb8_8320 } else { crc >> 1 };
                        }
                    }
                }
                self.regs.r[7] = sx40(i64::from(!crc));
                self.ret();
            }
            // "Instruction ROM correction": replace the instruction at the
            // address in R7L of the part's ROM by the one in R6.  There
            // is no such ROM here to correct.
            8 => self.ret(),
            // The part's undocumented ROM, which the 192's program calls
            // through a table of twelve entries.  By what the program
            // hands it (64-bit blocks, keys) it holds the cipher and the
            // authentication of the SD card's copy protection.  Not
            // modelled, on purpose: every entry reports failure.
            0x7000..=0x700b => {
                if self.trace || self.trace_io {
                    eprintln!("[{}] call to the protection ROM, entry {:04x}, from {:04x}: refused", self.name, self.regs.pc, self.regs.stk[(self.regs.sp & 15) as usize].wrapping_sub(1));
                }
                self.regs.r[0] = 1;
                self.ret();
            }
            pc => {
                self.fault = Some(format!("call to ROM address {:04x} (from {:04x}), which is not known", pc, self.regs.stk[(self.regs.sp & 15) as usize].wrapping_sub(1)));
            }
        }
    }

    // ---- execution -------------------------------------------------

    /// Ask for an interrupt (a bit number of [`irq`]).
    pub fn request(&mut self, source: u32) {
        self.pending |= 1 << source;
    }

    /// True when running would do nothing until something happens
    /// outside: halted with no interrupt to take, waiting for the host,
    /// in the boot ROM's download, or stopped by a fault.
    pub fn idle(&self) -> bool {
        self.fault.is_some() || self.rom != Rom::None || self.waiting || (self.halted && !self.can_interrupt())
    }

    fn can_interrupt(&self) -> bool {
        // (None is taken between a REP and the end of its repetitions.)
        self.regs.eir & 0x8000 == 0 && self.pending & !self.regs.sr & 0xfff != 0 && self.repeat == 0
    }

    /// Let `cycles` system clocks pass: instructions, and what the
    /// peripherals do in that time.
    pub fn run(&mut self, cycles: u64) {
        let end = self.cycles + cycles;
        while self.cycles < end {
            if self.stopped {
                // No clock: nothing counts, and what was due stays as
                // far away as it was.
                let rest = end - self.cycles;
                for at in [&mut self.timer_at, &mut self.frame_at, &mut self.next_event] {
                    *at = at.saturating_add(rest);
                }
                self.cycles = end;
                return;
            }
            if self.cycles >= self.next_event {
                self.events();
            }
            if self.idle() {
                // Nothing to execute: on to whatever happens next.
                self.cycles = end.min(self.next_event.max(self.cycles + 1));
            } else {
                self.step();
            }
        }
    }

    /// Run until the processor has nothing to do without the host or
    /// the passing of time, for at most `cycles`: what a host that polls
    /// between its accesses gives it.
    pub fn settle(&mut self, cycles: u64) {
        let end = self.cycles + cycles;
        while self.cycles < end && !self.idle() && !self.stopped {
            if self.cycles >= self.next_event {
                self.events();
            }
            self.step();
        }
    }

    fn schedule(&mut self) {
        let mut next = self.timer_at.min(self.frame_at);
        for at in [self.sd.response.as_ref().map(|r| r.0), self.sd.block.as_ref().map(|b| b.0)].into_iter().flatten() {
            next = next.min(at);
        }
        self.next_event = next;
    }

    /// Whatever the peripherals have due.
    fn events(&mut self) {
        let now = self.cycles;
        if now >= self.timer_at {
            self.request(irq::TIMER);
            if self.tcsr & 0x400 != 0 {
                self.timer_at = u64::MAX;
            } else {
                self.timer_at += self.timer_period();
            }
        }
        if now >= self.frame_at {
            self.frame();
            self.frame_at += self.frame_period();
        }
        if self.sd.response.as_ref().is_some_and(|r| now >= r.0) {
            let (_, words) = self.sd.response.take().unwrap();
            self.sd.rpr = words.into();
            self.request(irq::SDCR);
        }
        if self.sd.block.as_ref().is_some_and(|b| now >= b.0) {
            let (_, words) = self.sd.block.take().unwrap();
            self.sd.words.extend(words);
            self.sd_pump();
            // A multiple-block read goes on until it is stopped.
            if let Some(next) = self.card.as_mut().and_then(|c| c.next_block()) {
                let at = now + (16 + 8 * 512 + 18) * self.sd_div();
                self.sd.block = Some((at, be_words(&next)));
            }
        }
        self.schedule();
    }

    // ---- timer -------------------------------------------------------

    fn timer_period(&self) -> u64 {
        let source = match (self.tcsr >> 4) & 3 {
            0 => 1,
            1 => self.sd_div(),
            // The serial clock: the audio bit clock.
            2 => self.frame_period() / 32,
            // The INT4 pin, which counts card removals here.
            _ => return u64::MAX / 2,
        };
        (u64::from(self.tir) + 1) * source << (self.tcsr & 7)
    }

    // ---- audio output --------------------------------------------------

    /// System clocks per sample.
    fn frame_period(&self) -> u64 {
        self.clock_ratio() * match (self.asst >> 10) & 7 {
            0 | 1 => 256,
            2 | 3 => 384,
            _ => 512,
        }
    }

    fn frame_words(&self) -> usize {
        if self.asst & 0x40 != 0 { 2 } else { 4 }
    }

    /// One period of the left/right clock: the words stored for it go
    /// out, and the program is asked for the next.
    fn frame(&mut self) {
        let n = self.frame_words();
        if self.asdt_n == n {
            // The data register's contents go to the shift register,
            // which is what the program is told with an interrupt: the
            // data register is free again.
            self.last_sample = [self.asdt[0] as i16, self.asdt[n / 2] as i16];
            self.asdt_n = 0;
            self.request(irq::SO);
            self.waiting = false;
        }
        self.frames += 1;
        // The pins are held low until the program releases them.
        let out = if self.sst & 0x40 != 0 { self.last_sample } else { [0; 2] };
        self.audio.extend(out);
        if out != [0; 2] {
            self.loud_frames += 1;
        }
        // Nobody listening: keep a second at most.
        if self.audio.len() > 2 * 48_000 {
            self.audio.drain(..2 * 4_800);
        }
    }

    // ---- SD card interface ---------------------------------------------

    fn sd_div(&self) -> u64 {
        SD_DIV[((self.sd.ctl >> 10) & 15) as usize]
    }

    /// The command in the command registers goes to the card.
    fn sd_command(&mut self) {
        let now = self.cycles;
        let div = self.sd_div();
        let cmd = (self.sd.idx & 0x3f) as u8;
        let arg = u32::from(self.sd.agh) << 16 | u32::from(self.sd.agl);
        self.sd.rpr.clear();
        self.sd.response = None;
        self.sd.cen_until = now + 48 * div;
        if cmd == 12 || self.sd.idx & 0x300 == 0x100 {
            // What was still coming is no longer wanted.
            self.sd.block = None;
            self.sd.words.clear();
        }
        // The stop bit holds the card's clock low.
        let (response, data) = match (&mut self.card, self.sd.ctl & 0x200 == 0) {
            (Some(card), true) => card.command(cmd, arg),
            _ => (Response::None, None),
        };
        if self.trace_io {
            eprintln!("[{}] sd CMD{} {:08x} ({:04x}) -> {:02x?}{}", self.name, cmd, arg, self.sd.idx, response.bytes(),
                data.as_ref().map_or(String::new(), |d| format!(", {} bytes", d.len())));
        }
        let bytes = response.bytes();
        if self.sd.idx & 0x400 != 0 && !bytes.is_empty() {
            let long = self.sd.idx & 0x40 != 0;
            let mut words = be_words(&bytes);
            words.resize(if long { 8 } else { 3 }, 0xffff);
            self.sd.response = Some((now + (48 + 8 + if long { 136 } else { 48 }) * div, words));
        }
        if let (Some(data), 0x100) = (data, self.sd.idx & 0x300) {
            let at = now + (48 + 8 + 48 + 16 + 8 * data.len() as u64 + 18) * div;
            self.sd.block = Some((at, be_words(&data)));
        }
        self.schedule();
    }

    /// Data words that have arrived go where they are wanted: to data
    /// memory when a buffer transfer is set up, which ends with an
    /// interrupt, or one by one to the data register, with an interrupt
    /// for each.
    fn sd_pump(&mut self) {
        if self.sd.pcr & 2 != 0 {
            while self.sd.pscr != 0 {
                let Some(w) = self.sd.words.pop_front() else { return };
                let a = 0x5000 + (self.sd.ppr & 0xfff) as usize;
                if self.sd.pcr & 1 != 0 {
                    self.y[a] = w;
                } else {
                    self.x[a] = w;
                }
                self.sd.ppr = self.sd.ppr.wrapping_add(1) & 0xfff;
                self.sd.pscr -= 1;
            }
            self.sd.pcr &= !2;
            self.request(irq::SDDAT_IN);
        } else if !self.sd.words.is_empty() {
            self.request(irq::SDDAT_IN);
        }
    }

    // ---- port ------------------------------------------------------------

    /// The levels on the eight port pins.
    pub fn port(&self) -> u8 {
        (self.port_out & self.port_dir) | (self.pins & !self.port_dir)
    }

    fn port_set(&mut self, out: u8, dir: u8) {
        let before = self.port();
        self.port_out = out;
        self.port_dir = dir;
        if before & 4 != 0 && self.port() & 4 == 0 {
            self.host_irq = true;
        }
    }

    /// The registers on one line.
    pub fn dump(&self) -> String {
        let r = &self.regs;
        let mut s = String::new();
        for (i, v) in r.r.iter().enumerate() {
            let _ = write!(s, "r{}={:010x} ", i, v & 0xff_ffff_ffff);
        }
        let _ = write!(s, "dp={:04x?} dn={:04x?} dm={:04x?} sp={} stk={:04x?} lc={:04x} pc={:04x} sr={:04x} eir={:04x}", r.dp, r.dn, r.dm, r.sp, &r.stk[1..=(r.sp as usize).min(15)], r.lc, r.pc, self.sr(), r.eir);
        s
    }

    /// SR as the program reads it.
    fn sr(&self) -> u16 {
        let r = &self.regs;
        (r.eir & 0xe000) | (r.lc >> 3 & 0x1000) | (r.sr & 0xfff)
    }

    /// The peripherals' state on one line, for traces.
    pub fn dump_io(&self) -> String {
        format!("sst={:04x} asst={:04x} words={} frames={} frame in {} timer in {} next event in {} sd ctl={:04x} pcr={:x}",
            self.sst, self.asst, self.asdt_n, self.frames, self.frame_at.wrapping_sub(self.cycles) as i64,
            self.timer_at.wrapping_sub(self.cycles) as i64, self.next_event.wrapping_sub(self.cycles) as i64, self.sd.ctl, self.sd.pcr)
    }

    fn push(&mut self, v: u16) {
        self.regs.sp = self.regs.sp.wrapping_add(1);
        if self.regs.sp > 15 {
            self.regs.esr |= ESR_STE;
        }
        self.regs.stk[(self.regs.sp & 15) as usize] = v;
    }

    fn ret(&mut self) {
        self.regs.pc = self.regs.stk[(self.regs.sp & 15) as usize];
        if self.regs.sp == 0 {
            self.regs.esr |= ESR_STE;
        }
        self.regs.sp = self.regs.sp.wrapping_sub(1);
    }

    /// One instruction (of a repeated one, one repetition), or the
    /// taking of an interrupt.
    pub fn step(&mut self) {
        if self.can_interrupt() {
            let n = (self.pending & !self.regs.sr & 0xfff).trailing_zeros();
            self.pending &= !(1 << n);
            self.halted = false;
            self.waiting = false;
            let pc = self.regs.pc;
            self.push(pc);
            self.regs.eir = self.regs.eir >> 1 | 0x8000;
            self.regs.pc = 0x210 + 4 * n as u16;
            self.cycles += 2;
            if self.trace {
                eprintln!("[{}] interrupt {} from {:04x}", self.name, n, pc);
            }
            return;
        }
        if self.halted {
            self.cycles += 1;
            return;
        }
        let pc = self.regs.pc;
        if pc < 0x200 || (0x1000..0x8000).contains(&pc) {
            self.rom_call();
            self.cycles += 1;
            return;
        }
        let w = self.imem[pc as usize];
        if !self.tracepoints.is_empty() {
            if let Some((_, n)) = self.tracepoints.iter().find(|t| t.0 == pc) {
                eprintln!("[{}] at {:04x}: {}", self.name, pc, self.dump());
                self.trace_left = self.trace_left.max(*n);
            }
        }
        if self.trace_left != 0 {
            self.trace_left -= 1;
            eprintln!("[{}] {:04x}: {:<44} {}", self.name, pc, disasm(pc, w), self.dump());
        } else if self.trace {
            eprintln!("[{}] {:04x}: {:08x}  {}", self.name, pc, w, disasm(pc, w));
        }
        let saved = self.regs;
        self.regs.pc = pc.wrapping_add(1);
        self.wait = false;
        self.cycles += 1;
        if w >> 26 == 0b000111 || w >> 26 == 0b000110 {
            // REP: the next instruction, that many times; they follow
            // one to a step.
            let count = if w >> 26 == 0b000111 { w & 0x7fff } else { self.regs.r[(w & 7) as usize] as u32 & 0xffff };
            self.repeat = count.max(1);
            return;
        }
        if self.repeat != 0 {
            // A repetition: the program counter stays on the repeated
            // instruction until the last.
            self.exec(w);
            if self.wait {
                self.regs = saved;
                self.waiting = true;
                return;
            }
            self.repeat -= 1;
            self.regs.pc = if self.repeat != 0 { pc } else { pc.wrapping_add(1) };
            return;
        }
        self.exec(w);
        if self.wait {
            // Not now: the host has to read or write first.
            self.regs = saved;
            self.waiting = true;
            return;
        }
        // The end of a loop's body.
        if pc == self.regs.lea && self.regs.lc & 0x8000 == 0 && self.regs.pc == pc.wrapping_add(1) {
            if self.regs.lc > 1 {
                self.regs.lc -= 1;
                self.regs.pc = self.regs.lsa;
            } else {
                self.loop_pop();
            }
        }
    }

    fn loop_pop(&mut self) {
        let l = self.regs.lstk[(self.regs.lsp & 7) as usize];
        self.regs.lsa = l[0];
        self.regs.lea = l[1];
        self.regs.lc = l[2];
        if self.regs.lsp == 0 {
            self.regs.esr |= ESR_LSE;
        }
        self.regs.lsp = self.regs.lsp.wrapping_sub(1);
    }

    fn cond(&self, w: u32) -> bool {
        let v = self.regs.r[(w & 7) as usize];
        match (w >> 3) & 15 {
            0 => true,
            2 => v == 0,
            3 => v != 0,
            4 => v > 0,
            5 => v <= 0,
            6 => v >= 0,
            7 => v < 0,
            // Extension: the value does not fit in 32 bits.
            8 => v != i64::from(v as i32),
            9 => v == i64::from(v as i32),
            _ => false,
        }
    }

    /// Store a result, noting an overflow of the 40 bits.
    fn set_ovf(&mut self, n: usize, v: i64) {
        if v != sx40(v) {
            self.regs.esr |= ESR_OVF;
        }
        self.regs.r[n] = sx40(v);
    }

    fn exec(&mut self, w: u32) {
        if w & 0x8000_0000 != 0 {
            // Three operands, with parallel load/store.
            let (a, b, c) = (((w >> 28) & 7) as usize, ((w >> 21) & 7) as usize, ((w >> 18) & 7) as usize);
            let stores = self.parallel_stores(w);
            let (ra, rb, rc) = (self.regs.r[a], self.regs.r[b], self.regs.r[c]);
            let (ah, bh) = (i64::from((ra >> 16) as i16), i64::from((rb >> 16) as i16));
            let (al, bl) = (ra & 0xffff, rb & 0xffff);
            match (w >> 24) & 15 {
                0 => self.regs.r[c] = i64::from(ra < rb),
                1 => self.regs.r[c] = (ah * bh) << 1,
                2 => self.set_ovf(c, rc + ((ah * bh) << 1)),
                3 => self.set_ovf(c, rc - ((ah * bh) << 1)),
                4 => self.set_ovf(c, rc + ((ah * bl) << 1)),
                5 => self.set_ovf(c, rc + ((al * bl) << 1)),
                6 => self.set_ovf(c, (rc >> 1) + ((ah * bh) << 1)),
                7 => self.set_ovf(c, (rc >> 16) + ((ah * bh) << 1)),
                8 => self.set_ovf(c, ra + rb),
                9 => self.set_ovf(c, ra - rb),
                10 => self.regs.r[c] = ra & rb,
                11 => self.regs.r[c] = ra | rb,
                12 => self.regs.r[c] = ra ^ rb,
                13 => self.regs.r[c] = sra(ra, bl as u32),
                14 => self.regs.r[c] = srl(ra, bl as u32),
                _ => self.regs.r[c] = sll(ra, bl as u32),
            }
            self.parallel_loads(w, stores);
            return;
        }
        match w >> 28 {
            7 => {
                let stores = self.parallel_stores(w);
                self.monomial(w);
                self.parallel_loads(w, stores);
            }
            6 => {
                if self.cond(w) {
                    self.monomial(w);
                }
            }
            5 => {
                let (a, b) = (((w >> 21) & 7) as usize, ((w >> 18) & 7) as usize);
                let ra = self.regs.r[a];
                let imm = i64::from(w & 0xffff);
                match (w >> 24) & 15 {
                    8 => self.set_ovf(b, ra + imm),
                    9 => self.set_ovf(b, ra - imm),
                    10 => self.regs.r[b] = ra & imm,
                    11 => self.regs.r[b] = ra | imm,
                    12 => self.regs.r[b] = ra ^ imm,
                    13 => self.regs.r[b] = sra(ra, w & 0x3f),
                    14 => self.regs.r[b] = srl(ra, w & 0x3f),
                    15 => self.regs.r[b] = sll(ra, w & 0x3f),
                    _ => self.bad(w),
                }
            }
            4 => match (w >> 26) & 3 {
                3 => {
                    // Load/store of parts of registers, X and Y.
                    let x = self.side(w >> 8, false);
                    let y = self.side(w, true);
                    let (sx, sy) = ((w >> 23) & 7, (w >> 20) & 7);
                    let (lx, ly) = (w & 0x2_0000 != 0, w & 0x1_0000 != 0);
                    let (rx, ry) = (((w >> 8) & 7) as usize, (w & 7) as usize);
                    // Stores take the registers as they were.
                    let (vx, vy) = (part(self.regs.r[rx], sx), part(self.regs.r[ry], sy));
                    if let Some((a, _)) = x {
                        if lx {
                            let m = self.rd(false, a);
                            self.regs.r[rx] = load(self.regs.r[rx], sx, m);
                        } else {
                            self.wr(false, a, vx);
                        }
                    }
                    if let Some((a, _)) = y {
                        if ly {
                            let m = self.rd(true, a);
                            self.regs.r[ry] = load(self.regs.r[ry], sy, m);
                        } else {
                            self.wr(true, a, vy);
                        }
                    }
                    if let Some((_, (n, v))) = x {
                        self.regs.dp[n] = v;
                    }
                    if let Some((_, (n, v))) = y {
                        self.regs.dp[n] = v;
                    }
                }
                2 => {
                    // Direct address.
                    let (r, s) = (((w >> 23) & 7) as usize, (w >> 20) & 7);
                    let y = w & 0x8_0000 != 0;
                    let a = w as u16;
                    if w & 0x1_0000 != 0 {
                        let m = self.rd(y, a);
                        self.regs.r[r] = load(self.regs.r[r], s, m);
                    } else {
                        self.wr(y, a, part(self.regs.r[r], s));
                    }
                }
                1 => {
                    // Through a pointer, which then moves by an immediate.
                    let (r, s) = (((w >> 23) & 7) as usize, (w >> 20) & 7);
                    let n = ((w >> 17) & 7) as usize;
                    let a = self.regs.dp[n];
                    if w & 0x1_0000 != 0 {
                        let m = self.rd(n >= 4, a);
                        self.regs.r[r] = load(self.regs.r[r], s, m);
                    } else {
                        self.wr(n >= 4, a, part(self.regs.r[r], s));
                    }
                    self.regs.dp[n] = a.wrapping_add(w as u16);
                }
                _ => self.bad(w),
            },
            3 => match (w >> 26) & 3 {
                3 => {
                    // Between the low part of a register and another register.
                    let r = ((w >> 23) & 7) as usize;
                    let n = (w >> 17) & 0x3f;
                    if n < 32 && !self.cond(w) {
                        return;
                    }
                    if w & 0x1_0000 == 0 {
                        self.put_reg(n, self.regs.r[r] as u16, w);
                    } else {
                        let v = self.get_reg(n, w);
                        self.regs.r[r] = (self.regs.r[r] & !0xffff) | i64::from(v);
                    }
                }
                2 => {
                    if w & 0x1_0000 == 0 {
                        self.put_reg((w >> 17) & 0x3f, w as u16, w);
                    } else {
                        let r = ((w >> 23) & 7) as usize;
                        self.regs.r[r] = (self.regs.r[r] & !0xffff) | i64::from(w & 0xffff);
                    }
                }
                _ => self.bad(w),
            },
            2 => {
                if !self.cond(w) {
                    return;
                }
                let call = w & 0x0200_0000 != 0;
                let here = self.regs.pc.wrapping_sub(1);
                match (w >> 26) & 3 {
                    3 => {
                        if call {
                            self.push(here.wrapping_add(1));
                        }
                        self.regs.pc = here.wrapping_add((w >> 7) as u16);
                        self.cycles += 1;
                    }
                    2 => {
                        if call {
                            self.push(here.wrapping_add(1));
                        }
                        self.regs.pc = self.regs.dp[((w >> 17) & 7) as usize];
                        self.cycles += 2;
                    }
                    1 => {
                        self.ret();
                        if call {
                            // RETI: the enable flags as they were.
                            self.regs.eir = self.regs.eir << 1 | 1;
                        }
                        self.cycles += 1;
                    }
                    _ => self.bad(w),
                }
            }
            1 => {
                // LOOP (REP is taken care of in `step`).
                let count = if w & 0x0400_0000 != 0 { (w & 0x7fff) as u16 } else { self.regs.r[(w & 7) as usize] as u16 };
                let here = self.regs.pc.wrapping_sub(1);
                self.regs.lsp = self.regs.lsp.wrapping_add(1);
                if self.regs.lsp > 4 {
                    self.regs.esr |= ESR_LSE;
                }
                self.regs.lstk[(self.regs.lsp & 7) as usize] = [self.regs.lsa, self.regs.lea, self.regs.lc];
                self.regs.lsa = here.wrapping_add(1);
                self.regs.lea = here.wrapping_add(((w >> 16) & 0xff) as u16);
                self.regs.lc = count & 0x7fff;
            }
            _ => match w {
                0 => {}
                1 => self.halted = true,
                2 => self.loop_pop(),
                4 => self.pending = 0,
                // STOP: the clock stops until a reset or the wake-up pin.
                9 => self.stopped = true,
                _ => self.bad(w),
            },
        }
    }

    fn bad(&mut self, w: u32) {
        self.fault = Some(format!("instruction {:08x} at {:04x} is not known", w, self.regs.pc.wrapping_sub(1)));
    }

    fn monomial(&mut self, w: u32) {
        let (a, b) = (((w >> 21) & 7) as usize, ((w >> 18) & 7) as usize);
        let ra = self.regs.r[a];
        match (w >> 24) & 15 {
            0 => {}
            1 => self.regs.r[b] = 0,
            2 => self.set_ovf(b, ra + 1),
            3 => self.set_ovf(b, ra - 1),
            4 => self.set_ovf(b, ra.abs()),
            5 => self.regs.r[b] = !ra,
            6 => self.set_ovf(b, -ra),
            7 => self.regs.r[b] = ra.clamp(-0x8000_0000, 0x7fff_ffff),
            8 => {
                self.regs.r[b] = if ra > 0x7fff_0000 {
                    0x7fff_0000
                } else if ra < -0x8000_0000 {
                    -0x8000_0000
                } else {
                    (ra + 0x8000) & !0xffff
                }
            }
            9 => {
                // The shift that normalises the low 32 bits.
                let v = ra as i32;
                self.regs.r[b] = match v {
                    0 => 0,
                    -1 => 31,
                    _ => i64::from((if v < 0 { !v } else { v }).leading_zeros() - 1),
                };
            }
            12 => self.regs.r[b] = ra,
            13 => {
                // One step of a division: b by a.
                let rb = self.regs.r[b];
                let t = sx40(if (rb < 0) == (ra < 0) { rb - ra } else { rb + ra });
                self.regs.r[b] = sx40(t << 1 | i64::from(t >= 0));
            }
            14 => self.set_ovf(b, self.regs.r[b] + ra),
            15 => self.set_ovf(b, self.regs.r[b] - ra),
            _ => self.bad(w),
        }
    }

    /// One side (X with the fields at bit 8 on, Y at bit 0 on) of an
    /// instruction with parallel load/store: the address to use and the
    /// pointer's new value, or None when that side does nothing.
    fn side(&self, f: u32, y: bool) -> Option<(u16, (usize, u16))> {
        let mode = (f >> 3) & 7;
        if mode == 0 {
            return None;
        }
        let n = ((f >> 6) & 3) as usize + if y { 4 } else { 0 };
        let dp = self.regs.dp[n];
        let dn = self.regs.dn[n];
        Some(match mode {
            1 => (dp, (n, dp)),
            2 => (dp, (n, dp.wrapping_add(1))),
            3 => (dp, (n, dp.wrapping_sub(1))),
            4 => (dp, (n, dp.wrapping_add(dn))),
            5 => {
                // Around a ring of DM + 1 words that starts where the
                // low bits (as many as DM has) are zero.
                let dm = self.regs.dm[usize::from(y)];
                let mask = (1u32 << (16 - dm.leading_zeros())) - 1;
                let mut l = (u32::from(dp) & mask) as i32 + i32::from(dn as i16);
                if l > i32::from(dm) {
                    l -= i32::from(dm) + 1;
                } else if l < 0 {
                    l += i32::from(dm) + 1;
                }
                (dp, (n, (u32::from(dp) & !mask | l as u32 & mask) as u16))
            }
            _ => (dp.reverse_bits(), (n, dp.wrapping_add(dn))),
        })
    }

    /// The stores of a parallel load/store, which take the registers as
    /// they are before the operation.  Returns what `parallel_loads`
    /// needs.
    fn parallel_stores(&mut self, w: u32) -> [Option<(u16, (usize, u16))>; 2] {
        let x = self.side(w >> 8, false);
        let y = self.side(w, true);
        if let (Some((a, _)), false) = (x, w & 0x2_0000 != 0) {
            let v = (self.regs.r[((w >> 8) & 7) as usize] >> 16) as u16;
            self.wr(false, a, v);
        }
        if let (Some((a, _)), false) = (y, w & 0x1_0000 != 0) {
            let v = (self.regs.r[(w & 7) as usize] >> 16) as u16;
            self.wr(true, a, v);
        }
        [x, y]
    }

    fn parallel_loads(&mut self, w: u32, sides: [Option<(u16, (usize, u16))>; 2]) {
        if let (Some((a, _)), true) = (sides[0], w & 0x2_0000 != 0) {
            let m = self.rd(false, a);
            self.regs.r[((w >> 8) & 7) as usize] = i64::from(m as i16) << 16;
        }
        if let (Some((a, _)), true) = (sides[1], w & 0x1_0000 != 0) {
            let m = self.rd(true, a);
            self.regs.r[(w & 7) as usize] = i64::from(m as i16) << 16;
        }
        for (_, (n, v)) in sides.into_iter().flatten() {
            self.regs.dp[n] = v;
        }
    }

    /// The registers a transfer instruction names, other than R0-R7.
    /// The numbers are not in the manual; they are from the 192's
    /// program: 0-7 DP, 8-15 DN, 16 DMX, 20 DMY, 32 SR, 33 EIR, 34 the
    /// stack, 38 the loop counter.
    fn get_reg(&mut self, n: u32, w: u32) -> u16 {
        let r = &self.regs;
        match n {
            0..=7 => r.dp[n as usize],
            8..=15 => r.dn[(n - 8) as usize],
            16 => r.dm[0],
            20 => r.dm[1],
            32 => self.sr(),
            33 => r.eir,
            34 => {
                // Reading the stack takes its top off.
                let v = r.stk[(r.sp & 15) as usize];
                self.regs.sp = self.regs.sp.wrapping_sub(1);
                v
            }
            // The loop counter: what is left of the current loop.
            38 => r.lc,
            // (SP, LSP, the loop stack and ESR have numbers too; the
            // 192's program uses none of them, and the manual does not
            // give them.)
            _ => {
                self.bad(w);
                0
            }
        }
    }

    fn put_reg(&mut self, n: u32, v: u16, w: u32) {
        let r = &mut self.regs;
        match n {
            0..=7 => r.dp[n as usize] = v,
            8..=15 => r.dn[(n - 8) as usize] = v,
            16 => r.dm[0] = v,
            20 => r.dm[1] = v,
            32 => {
                r.sr = v & 0xfff;
                r.eir = (r.eir & 0x1fff) | (v & 0xe000);
            }
            33 => r.eir = v,
            34 => {
                // Writing the stack puts a word on top.
                r.sp = r.sp.wrapping_add(1);
                r.stk[(r.sp & 15) as usize] = v;
            }
            _ => self.bad(w),
        }
    }

    // ---- data memory and peripherals ---------------------------------

    fn rd(&mut self, y: bool, a: u16) -> u16 {
        if a & 0xffc0 == 0x3800 {
            return self.io_read(a & 0x3f);
        }
        if y { self.y[a as usize] } else { self.x[a as usize] }
    }

    fn wr(&mut self, y: bool, a: u16, v: u16) {
        if a & 0xffc0 == 0x3800 {
            return self.io_write(a & 0x3f, v);
        }
        if let Some((from, to)) = self.watch {
            if (from..=to).contains(&a) {
                eprintln!("[{}] {:04x}: {:04x}:{} <= {:04x}", self.name, self.regs.pc.wrapping_sub(1), a, if y { 'y' } else { 'x' }, v);
            }
        }
        if y {
            self.y[a as usize] = v;
        } else {
            self.x[a as usize] = v;
        }
    }

    fn io_read(&mut self, n: u16) -> u16 {
        let v = match n {
            // HDT: the word the host wrote.
            6 => {
                if self.hst & HST_HWEF != 0 {
                    if self.hst & HST_HAWE != 0 {
                        self.wait = true;
                        return 0;
                    }
                    self.hst |= HST_HLER;
                }
                self.hst |= HST_HWEF;
                self.hdt_in
            }
            7 => self.hst,
            // The audio output's registers: no input is modelled.
            0 => 0,
            1 => self.sst,
            2 => (self.asst & 0xfdfc) | if self.asdt_n < self.frame_words() { 2 } else { 0 },
            4 => u16::from(self.port()),
            5 => u16::from(self.port_dir),
            0x11 => self.sd.idx,
            0x12 => self.sd.agh,
            0x13 => self.sd.agl,
            0x14 => {
                let errors = self.sd.ctl & 0x78;
                (self.sd.ctl & 0x3e78)
                    | if errors != 0 { 0x8000 } else { 0 }
                    | if self.cycles < self.sd.cen_until { 0x4000 } else { 0 }
                    | if self.sd.rpr.is_empty() { 0 } else { 4 }
                    | 2
                    | u16::from(!self.sd.words.is_empty() && self.sd.pcr & 2 == 0)
            }
            0x15 => self.sd.rpr.pop_front().unwrap_or(0),
            // Never busy, and no data has been refused.
            0x16 => 2,
            0x10 => {
                let v = self.sd.words.pop_front().unwrap_or(0);
                if !self.sd.words.is_empty() {
                    self.request(irq::SDDAT_IN);
                }
                v
            }
            0x20 => self.tir,
            0x21 => {
                if self.timer_at == u64::MAX {
                    self.tir
                } else {
                    ((self.timer_at.saturating_sub(self.cycles)) * (u64::from(self.tir) + 1) / self.timer_period().max(1)) as u16
                }
            }
            0x22 => self.tcsr,
            0x23 => self.tenr,
            0x30 => self.sd.psar,
            0x31 => self.sd.psr,
            0x32 => self.sd.ppr,
            0x33 => self.sd.pcr,
            _ => self.io[n as usize],
        };
        // (Not the host port, nor the card interface's status, which the
        // 192's program reads all the time.)
        if self.trace_io && !matches!(n, 6 | 7 | 0x14 | 0x16 | 0x33) {
            eprintln!("[{}] {:04x}: io R {:04x} => {:04x}", self.name, self.regs.pc.wrapping_sub(1), 0x3800 + n, v);
        }
        v
    }

    fn io_write(&mut self, n: u16, v: u16) {
        if self.trace_io && !matches!(n, 0 | 6 | 7) {
            eprintln!("[{}] {:04x}: io W {:04x} <= {:04x}", self.name, self.regs.pc.wrapping_sub(1), 0x3800 + n, v);
        }
        match n {
            // HDT: a word for the host.
            6 => {
                if self.hst & HST_HREF != 0 {
                    if self.hst & HST_HAWE != 0 {
                        self.wait = true;
                        return;
                    }
                    self.hst |= HST_HSER;
                }
                self.hdt_out = v;
                self.hst |= HST_HREF;
            }
            7 => self.hst = (v & 0x0ffc) | (self.hst & 3),
            0 => {
                // A word of the next sample, the high one first.
                let n = self.frame_words();
                if self.asdt_n == n {
                    if self.asst & 0x100 != 0 && self.frame_at != u64::MAX {
                        self.wait = true;
                    } else {
                        self.asst |= 8;
                    }
                    return;
                }
                self.asdt[self.asdt_n] = v;
                self.asdt_n += 1;
            }
            1 => self.sst = v,
            2 => {
                self.asst = v & 0xfdfc;
                if v & 0x200 != 0 {
                    self.asdt_n = 0;
                    self.asst &= !0xc;
                }
                // The frame clock runs when this is the audio interface,
                // its output enabled, and the clocks come from here.
                let on = v & 0xc010 == 0xc000;
                if !on {
                    self.frame_at = u64::MAX;
                } else if self.frame_at == u64::MAX {
                    self.frame_at = self.cycles + self.frame_period();
                }
                self.schedule();
            }
            4 => self.port_set(v as u8, self.port_dir),
            5 => {
                let (mut out, mut dir) = (self.port_out, self.port_dir);
                if v & 0x2000 != 0 {
                    dir = if v & 0x1000 != 0 { dir | v as u8 } else { dir & !(v as u8) };
                }
                if v & 0x8000 != 0 {
                    let bit = 1u8 << ((v >> 8) & 7);
                    out = if v & 0x4000 != 0 { out | bit } else { out & !bit };
                }
                self.port_set(out, dir);
            }
            0x10 => {}
            0x11 => self.sd.idx = v & 0x7fff,
            0x12 => self.sd.agh = v,
            0x13 => self.sd.agl = v,
            0x14 => {
                // The error flags are cleared by storing zeros.
                self.sd.ctl = (v & 0x3e00) | (self.sd.ctl & v & 0x78);
                if v & 0x100 != 0 {
                    // Reset of the interface.
                    self.sd = Sd { idx: 0x4800, ctl: v & 0x3e00, ..Sd::default() };
                    self.schedule();
                }
                if v & 0x4000 != 0 {
                    self.sd_command();
                }
            }
            0x15 | 0x16 => {}
            0x20 => self.tir = v,
            0x21 => {}
            0x22 => {
                if self.tenr & 1 != 0 {
                    self.tcsr |= 0x200;
                } else {
                    self.tcsr = v & 0x637;
                }
            }
            0x23 => {
                let on = v & 1 != 0;
                if on && self.tenr & 1 == 0 {
                    self.timer_at = self.cycles + self.timer_period();
                } else if !on {
                    self.timer_at = u64::MAX;
                }
                self.tenr = v & 1;
                self.schedule();
            }
            0x30 => self.sd.psar = v & 0xfff,
            0x31 => self.sd.psr = v & 0x1fff,
            0x32 => {}
            0x33 => {
                if self.sd.pcr & 2 != 0 && v & 2 != 0 {
                    // A store while a transfer runs is an error, and
                    // changes nothing.
                    self.sd.pcr |= 4;
                } else {
                    let start = v & 2 != 0;
                    self.sd.pcr = v & 3;
                    if start {
                        self.sd.pscr = self.sd.psr;
                        self.sd.ppr = self.sd.psar;
                        self.sd_pump();
                    }
                }
            }
            _ => self.io[n as usize] = v,
        }
    }
}

/// Bytes as the 16-bit words the card interface makes of them, the
/// first byte in the high half.
fn be_words(bytes: &[u8]) -> Vec<u16> {
    bytes.chunks(2).map(|c| u16::from(c[0]) << 8 | u16::from(*c.get(1).unwrap_or(&0))).collect()
}

fn sra(v: i64, n: u32) -> i64 {
    v >> (n & 63).min(40)
}

fn srl(v: i64, n: u32) -> i64 {
    let n = n & 63;
    if n >= 40 { 0 } else { sx40(((v as u64 & 0xff_ffff_ffff) >> n) as i64) }
}

fn sll(v: i64, n: u32) -> i64 {
    let n = n & 63;
    if n >= 40 { 0 } else { sx40(v << n) }
}

/// The part of a register a store takes.  The suffix field of an
/// instruction is a mask of the register's parts: 1 = L, 2 = H, 4 = E.
fn part(r: i64, suffix: u32) -> u16 {
    match suffix {
        1 => r as u16,
        2 => (r >> 16) as u16,
        _ => (r >> 32) as u16 & 0xff,
    }
}

/// A load into part of a register: 1 = L, 2 = H, 4 = E, 6 = EH (the
/// word in H, its sign in E, L kept), 7 = the whole register (the same
/// with L cleared).
fn load(r: i64, suffix: u32, m: u16) -> i64 {
    match suffix {
        1 => (r & !0xffff) | i64::from(m),
        2 => (r & !0xffff_0000) | i64::from(m) << 16,
        4 => sx40((r & 0xffff_ffff) | i64::from(m & 0xff) << 32),
        6 => i64::from(m as i16) << 16 | (r & 0xffff),
        _ => i64::from(m as i16) << 16,
    }
}

// ---- disassembly -----------------------------------------------------

fn reg_name(n: u32) -> String {
    match n {
        0..=7 => format!("dp{}", n),
        8..=15 => format!("dn{}", n - 8),
        16 => "dmx".into(),
        20 => "dmy".into(),
        32 => "sr".into(),
        33 => "eir".into(),
        34 => "stk".into(),
        38 => "lc".into(),
        _ => format!("reg{}", n),
    }
}

fn cond_text(w: u32) -> String {
    let c = match (w >> 3) & 15 {
        0 => return String::new(),
        2 => "== 0",
        3 => "!= 0",
        4 => "> 0",
        5 => "<= 0",
        6 => ">= 0",
        7 => "< 0",
        8 => "== ex",
        9 => "!= ex",
        _ => "?",
    };
    format!("if (r{} {}) ", w & 7, c)
}

fn side_text(f: u32, y: bool, load: bool, reg: &str) -> String {
    let n = ((f >> 6) & 3) + if y { 4 } else { 0 };
    let p = match (f >> 3) & 7 {
        0 => return String::new(),
        1 => format!("*dp{}", n),
        2 => format!("*dp{}++", n),
        3 => format!("*dp{}--", n),
        4 => format!("*dp{}##", n),
        5 => format!("*dp{}%%", n),
        6 => format!("*!dp{}##", n),
        _ => format!("*dp{}?", n),
    };
    if load { format!("  {} = {}", reg, p) } else { format!("  {} = {}", p, reg) }
}

const SUFFIX: [&str; 8] = ["?0", "l", "h", "?3", "e", "?5", "eh", ""];

/// One instruction as text, in the notation of NEC's assembler.
pub fn disasm(pc: u16, w: u32) -> String {
    let mut s = String::new();
    let parallel = |s: &mut String| {
        let (lx, ly) = (w & 0x2_0000 != 0, w & 0x1_0000 != 0);
        s.push_str(&side_text(w >> 8, false, lx, &format!("r{}{}", (w >> 8) & 7, if lx { "" } else { "h" })));
        s.push_str(&side_text(w, true, ly, &format!("r{}{}", w & 7, if ly { "" } else { "h" })));
    };
    let monomial = |s: &mut String| {
        let (a, b) = ((w >> 21) & 7, (w >> 18) & 7);
        let _ = match (w >> 24) & 15 {
            0 => write!(s, "nop"),
            1 => write!(s, "clr(r{})", b),
            2 => write!(s, "r{} = r{} + 1", b, a),
            3 => write!(s, "r{} = r{} - 1", b, a),
            4 => write!(s, "r{} = abs(r{})", b, a),
            5 => write!(s, "r{} = ~r{}", b, a),
            6 => write!(s, "r{} = -r{}", b, a),
            7 => write!(s, "r{} = clip(r{})", b, a),
            8 => write!(s, "r{} = round(r{})", b, a),
            9 => write!(s, "r{} = exp(r{})", b, a),
            12 => write!(s, "r{} = r{}", b, a),
            13 => write!(s, "r{} /= r{}", b, a),
            14 => write!(s, "r{} += r{}", b, a),
            15 => write!(s, "r{} -= r{}", b, a),
            n => write!(s, "?mono{}", n),
        };
    };
    if w & 0x8000_0000 != 0 {
        let (a, b, c) = ((w >> 28) & 7, (w >> 21) & 7, (w >> 18) & 7);
        let _ = match (w >> 24) & 15 {
            0 => write!(s, "r{} = lt(r{}, r{})", c, a, b),
            1 => write!(s, "r{} = r{}h * r{}h", c, a, b),
            2 => write!(s, "r{} = r{} + r{}h * r{}h", c, c, a, b),
            3 => write!(s, "r{} = r{} - r{}h * r{}h", c, c, a, b),
            4 => write!(s, "r{} = r{} + r{}h * r{}l", c, c, a, b),
            5 => write!(s, "r{} = r{} + r{}l * r{}l", c, c, a, b),
            6 => write!(s, "r{} = (r{} >> 1) + r{}h * r{}h", c, c, a, b),
            7 => write!(s, "r{} = (r{} >> 16) + r{}h * r{}h", c, c, a, b),
            8 => write!(s, "r{} = r{} + r{}", c, a, b),
            9 => write!(s, "r{} = r{} - r{}", c, a, b),
            10 => write!(s, "r{} = r{} & r{}", c, a, b),
            11 => write!(s, "r{} = r{} | r{}", c, a, b),
            12 => write!(s, "r{} = r{} ^ r{}", c, a, b),
            13 => write!(s, "r{} = r{} sra r{}l", c, a, b),
            14 => write!(s, "r{} = r{} srl r{}l", c, a, b),
            _ => write!(s, "r{} = r{} sll r{}l", c, a, b),
        };
        parallel(&mut s);
        return s;
    }
    match w >> 28 {
        7 => {
            monomial(&mut s);
            parallel(&mut s);
        }
        6 => {
            s.push_str(&cond_text(w));
            monomial(&mut s);
        }
        5 => {
            let (a, b, imm) = ((w >> 21) & 7, (w >> 18) & 7, w & 0xffff);
            let op = ["+", "-", "&", "|", "^", "sra", "srl", "sll"];
            match (w >> 24) & 15 {
                n @ 8..=15 => {
                    let _ = write!(s, "r{} = r{} {} {:#x}", b, a, op[n as usize - 8], imm);
                }
                n => {
                    let _ = write!(s, "?imm{}", n);
                }
            }
        }
        4 => match (w >> 26) & 3 {
            3 => {
                let (lx, ly) = (w & 0x2_0000 != 0, w & 0x1_0000 != 0);
                s.push_str(side_text(w >> 8, false, lx, &format!("r{}{}", (w >> 8) & 7, SUFFIX[((w >> 23) & 7) as usize])).trim_start());
                s.push_str(&side_text(w, true, ly, &format!("r{}{}", w & 7, SUFFIX[((w >> 20) & 7) as usize])));
            }
            2 => {
                let r = format!("r{}{}", (w >> 23) & 7, SUFFIX[((w >> 20) & 7) as usize]);
                let m = format!("*{:#06x}:{}", w & 0xffff, if w & 0x8_0000 != 0 { 'y' } else { 'x' });
                let _ = if w & 0x1_0000 != 0 { write!(s, "{} = {}", r, m) } else { write!(s, "{} = {}", m, r) };
            }
            1 => {
                let r = format!("r{}{}", (w >> 23) & 7, SUFFIX[((w >> 20) & 7) as usize]);
                let m = format!("*dp{}##{:#x}", (w >> 17) & 7, w & 0xffff);
                let _ = if w & 0x1_0000 != 0 { write!(s, "{} = {}", r, m) } else { write!(s, "{} = {}", m, r) };
            }
            _ => s.push('?'),
        },
        3 => match (w >> 26) & 3 {
            3 => {
                let n = (w >> 17) & 0x3f;
                if n < 32 {
                    s.push_str(&cond_text(w));
                }
                let _ = if w & 0x1_0000 == 0 {
                    write!(s, "{} = r{}l", reg_name(n), (w >> 23) & 7)
                } else {
                    write!(s, "r{}l = {}", (w >> 23) & 7, reg_name(n))
                };
            }
            2 => {
                let _ = if w & 0x1_0000 == 0 {
                    write!(s, "{} = {:#x}", reg_name((w >> 17) & 0x3f), w & 0xffff)
                } else {
                    write!(s, "r{}l = {:#x}", (w >> 23) & 7, w & 0xffff)
                };
            }
            _ => s.push('?'),
        },
        2 => {
            s.push_str(&cond_text(w));
            let call = w & 0x0200_0000 != 0;
            let _ = match (w >> 26) & 3 {
                3 => write!(s, "{} {:#06x}", if call { "call" } else { "jmp" }, pc.wrapping_add((w >> 7) as u16)),
                2 => write!(s, "{} dp{}", if call { "call" } else { "jmp" }, (w >> 17) & 7),
                1 => write!(s, "{}", if call { "reti" } else { "ret" }),
                _ => write!(s, "?"),
            };
        }
        1 => {
            let _ = match (w >> 26) & 3 {
                3 => write!(s, "rep {:#x}", w & 0x7fff),
                2 => write!(s, "rep r{}l", w & 7),
                1 => write!(s, "loop {:#x} {{ to {:#06x} }}", w & 0x7fff, pc.wrapping_add(((w >> 16) & 0xff) as u16)),
                _ => write!(s, "loop r{}l {{ to {:#06x} }}", w & 7, pc.wrapping_add(((w >> 16) & 0xff) as u16)),
            };
        }
        _ => s.push_str(match w {
            0 => "nop",
            1 => "halt",
            2 => "lpop",
            4 => "fint",
            9 => "stop",
            _ => "?",
        }),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A processor with a program at 0x200, past the boot ROM's download.
    fn with(program: &[u32]) -> Dsp {
        let mut d = Dsp::new();
        for w in [0, program.len() as u16, 0x0c00] {
            d.host_write(w);
        }
        for w in program {
            d.host_write(*w as u16);
            d.host_write((*w >> 16) as u16);
        }
        assert_eq!(d.regs.pc, 0x200);
        d
    }

    #[test]
    fn boot_arithmetic_and_loops() {
        // r0l = 5; r1l = 3; loop 4 { r2 += r0; r2 = r2 - r1 (via r3) }; halt
        let mut d = with(&[
            0x3801_0005, // r0l = 5
            0x3881_0003, // r1l = 3
            0x1402_0004, // loop 4 { two instructions }
            0x6e08_0000, // r2 += r0
            0x6f28_0000, // r2 -= r1
            0x0000_0001, // halt
        ]);
        assert_eq!(disasm(0x202, 0x1402_0004), "loop 0x4 { to 0x0204 }");
        assert_eq!(disasm(0x203, 0x6e08_0000), "r2 += r0");
        d.run(100);
        assert!(d.halted && d.fault.is_none(), "{:?}", d.fault);
        assert_eq!(d.regs.r[2], 8);
        assert_eq!(d.regs.lc & 0x8000, 0x8000);
    }

    #[test]
    fn multiply_and_memory() {
        let mut d = with(&[
            0x3800_0010, // dp0 = 0x10
            0x3808_0020, // dp4 = 0x20
            0x0000_0000, // nop
            0x7003_1110, // nop  r1 = *dp0++  r0 = *dp4++
            0x9108_0000, // r2 = r1h * r0h
            0x4d00_0a00, // *dp0 = r2h
            0x0000_0001,
        ]);
        d.x[0x10] = 0x4000; // 0.5
        d.y[0x20] = 0xc000; // -0.5
        d.run(100);
        assert!(d.halted && d.fault.is_none(), "{:?}", d.fault);
        assert_eq!(d.regs.r[2], sx40(0xff_e000_0000u64 as i64));
        assert_eq!(d.x[0x11], 0xe000);
        assert_eq!((d.regs.dp[0], d.regs.dp[4]), (0x11, 0x21));
    }

    #[test]
    fn timer_interrupts_wake_a_halt() {
        let mut program = vec![0u32; 0x50];
        program[0] = 0x2c00_2000; // jmp 0x240
        program[0x3c] = 0x6224_0000; // timer vector: r1 = r1 + 1
        program[0x3d] = 0x2600_0000; // reti
        program[0x40..0x4d].copy_from_slice(&[
            0x3801_0009, // r0l = 9
            0x4810_3820, // TIR = r0l: ten counts
            0x6100_0000, // clr(r0)
            0x4810_3822, // TCSR = 0: the system clock, repeating
            0x3801_0001, // r0l = 1
            0x4810_3823, // TENR = 1
            0x3801_7fff, // r0l = 0x7fff
            0x3c42_0000, // eir = r0l: interrupts on
            0x0000_0001, // halt
            0x0000_0001, // halt
            0x0000_0001, // halt
            0x3901_0001, // r2l = 1
            0x0000_0001, // halt
        ]);
        let mut d = with(&program);
        // Each halt ends with a period of the timer, ten clocks.
        let mut woken = Vec::new();
        while d.regs.r[2] == 0 && d.cycles < 1000 {
            let before = d.regs.r[1];
            d.run(1);
            if d.regs.r[1] != before {
                woken.push(d.cycles);
            }
        }
        d.run(2);
        assert_eq!(woken.len(), 3, "{:?}", woken);
        assert_eq!((woken[1] - woken[0], woken[2] - woken[1]), (10, 10));
        assert!(d.halted && d.fault.is_none(), "{:?}", d.fault);
        // The enable flags are back as they were before the interrupt.
        assert_eq!(d.regs.eir & 0x8000, 0);
        // Stopped, the timer stands still.
        d.stopped = true;
        d.run(1000);
        assert_eq!(d.regs.r[1], 3);
        d.wakeup();
        d.run(12);
        assert_eq!(d.regs.r[1], 4);
    }

    #[test]
    fn rings_and_register_parts() {
        let mut d = with(&[
            0x3820_0007, // dmx = 7: a ring of eight
            0x3810_0001, // dn0 = 1
            0x3800_0106, // dp0 = 0x106
            0x0000_0000,
            0x4c82_2800, // r0l = *dp0%%
            0x4d02_2900, // r1h = *dp0%%
            0x4f82_2a00, // r2 = *dp0%%
            0x4f02_2b00, // r3eh = *dp0%%
            0x0000_0001,
        ]);
        d.x[0x106] = 0x1111;
        d.x[0x107] = 0x2222;
        d.x[0x100] = 0x8333;
        d.x[0x101] = 0x8444;
        d.regs.r[1] = 0x12_0000_5678;
        d.regs.r[3] = 0x9abc;
        d.run(50);
        assert!(d.halted && d.fault.is_none(), "{:?}", d.fault);
        assert_eq!(d.regs.dp[0], 0x102);
        assert_eq!(d.regs.r[0], 0x1111);
        assert_eq!(d.regs.r[1], 0x12_2222_5678);
        assert_eq!(d.regs.r[2], sx40(0xff_8333_0000u64 as i64));
        assert_eq!(d.regs.r[3], sx40(0xff_8444_9abcu64 as i64));
    }

    #[test]
    fn audio_frames() {
        let mut d = with(&[1]);
        d.run(10);
        // As the 192's software sets it: output, master, 16 bits a
        // channel, 512 master clocks a sample.
        d.io_write(0x2e, 0x180); // the multiplied clock
        assert_eq!(d.clock_hz(), 67_737_600);
        d.io_write(1, 0x0ac0);
        d.io_write(2, 0xd762);
        assert_eq!(d.sample_rate(), 44_100);
        d.io_write(0, 0x1234);
        d.io_write(0, 0xfffe);
        assert_eq!(d.io_read(2) & 2, 0, "the data register is full");
        d.run(1537);
        assert_eq!(d.audio.iter().copied().collect::<Vec<_>>(), [0x1234, -2]);
        assert_ne!(d.pending & 1 << irq::SO, 0);
        assert_eq!(d.io_read(2) & 2, 2);
        // Nothing new: the sample stays, and no interrupt asks again.
        d.pending = 0;
        d.run(1536);
        assert_eq!(d.audio.len(), 4);
        assert_eq!(d.pending, 0);
        assert_eq!((d.frames, d.loud_frames), (2, 2));
    }

    #[test]
    fn a_card_on_the_interface() {
        use crate::dev::sdhi::Card;
        let mut image = vec![0u8; 1024 * 1024];
        image[512 * 5..512 * 5 + 4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let mut d = with(&[1]);
        d.run(10);
        d.card = Some(SdCard::new(Card::from_bytes(image).unwrap()));
        // A command the way the 192's program sends one, and the words
        // of its response.
        fn command(d: &mut Dsp, idx: u16, arg: u32) -> Vec<u16> {
            d.pending = 0;
            d.io_write(0x11, idx);
            d.io_write(0x12, (arg >> 16) as u16);
            d.io_write(0x13, arg as u16);
            let ctl = d.io_read(0x14);
            d.io_write(0x14, (ctl & 0x7f87) | 0x4000);
            d.run(200_000);
            let mut words = Vec::new();
            while d.io_read(0x14) & 4 != 0 {
                words.push(d.io_read(0x15));
            }
            words
        }
        // The clock is stopped after a reset: no answer.
        assert_eq!(d.io_read(0x14) & 0x200, 0x200);
        assert!(command(&mut d, 0x4c37, 0).is_empty());
        d.io_write(0x14, 0x3800); // clock on, divided by 512
        assert!(command(&mut d, 0x4800, 0).is_empty(), "CMD0 has no response");
        assert_eq!(command(&mut d, 0x4c37, 0), [0x3700, 0x0001, 0x2083], "CMD55");
        assert_ne!(d.pending & 1 << irq::SDCR, 0);
        assert_eq!(command(&mut d, 0x4c29, 0x00ff_8000), [0x3f80, 0xff80, 0x00ff], "ACMD41");
        assert_eq!(command(&mut d, 0x4c42, 0).len(), 8, "CMD2, a long response");
        let r = command(&mut d, 0x4c03, 0);
        let rca = u32::from(r[0] & 0xff) << 24 | u32::from(r[1] & 0xff00) << 8;
        assert_eq!(command(&mut d, 0x4c07, rca).len(), 3, "CMD7");
        // A sector into Y memory at 0x5010 by the buffer transfer, with
        // the clock divided by four as the program has it from here on.
        d.io_write(0x14, 0x0c00);
        d.io_write(0x30, 0x10);
        d.io_write(0x31, 0x100);
        d.io_write(0x33, 3);
        assert_eq!(command(&mut d, 0x4d11, 5 * 512)[1] & 0x1e, 8, "CMD17, card in the transfer state");
        assert_eq!(&d.y[0x5010..0x5012], &[0xdead, 0xbeef]);
        assert_eq!(d.io_read(0x33), 1, "the transfer has ended");
        assert_ne!(d.pending & 1 << irq::SDDAT_IN, 0);
        assert_eq!(d.io_read(0x32), 0x110);
    }

    #[test]
    fn host_words_wait_and_interrupt() {
        // Echo: r0l = HDT; r0 = r0 + 1; HDT = r0l; halt.
        let mut d = with(&[
            0x4811_3806, // r0l = *0x3806:x
            0x5800_0001, // r0 = r0 + 1
            0x4810_3806, // *0x3806:x = r0l
            0x0000_0001,
        ]);
        d.run(100);
        assert!(d.waiting && !d.halted);
        assert!(d.host_can_write() && !d.host_can_read());
        d.host_write(0x1233);
        d.run(100);
        assert!(d.halted);
        assert!(d.host_can_read());
        assert_eq!(d.host_read(), 0x1234);
        assert!(!d.host_can_read());
    }
}
