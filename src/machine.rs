//! Ties the CPU and the board together and knows how to start firmware.

use crate::board::Board;
use crate::bootrom::{self, Hook};
use crate::dev::hcan::Frame;
use crate::dev::uart::SerialPort;
use crate::fw::{IndexInfo, Module, Variant};
use crate::sh4::cpu::Stop;
use std::collections::{BTreeMap, VecDeque};
use crate::sh4::{Cpu, SR_BL, SR_MD};

/// The car in motion, for the navigation's dead reckoning (193): speed
/// pulses arrive on timer 2's capture pin and the gyroscope's voltage on
/// input 1 of the A/D converter; the GPS fix, if there is one, travels
/// along.  See docs/hardware-193.md.
#[derive(Clone, Debug, Default)]
pub struct Drive {
    /// Kilometres per hour.
    pub speed: f64,
    /// Degrees per second, clockwise positive.
    pub turn: f64,
    /// Milliseconds after the start at which the car sets off.
    pub from: u64,
    /// Fractions of a speed pulse not yet sent, and the cycle up to which
    /// the motion has been accounted for.
    pulses: f64,
    last: u64,
}

impl Drive {
    pub fn new(speed: f64, turn: f64, from: u64) -> Drive {
        Drive { speed, turn, from, pulses: 0.0, last: 0 }
    }

    /// `"KMH[,TURN][@MS]"`: speed, degrees per second to the right, and
    /// when to set off.
    pub fn parse(s: &str) -> Result<Drive, String> {
        let (what, from) = match s.split_once('@') {
            Some((what, ms)) => (what, ms.parse().map_err(|_| format!("{} is not a time in milliseconds", ms))?),
            None => (s, 0),
        };
        let (speed, turn) = what.split_once(',').unwrap_or((what, "0"));
        let number = |t: &str| t.trim().parse::<f64>().map_err(|_| format!("{} is not a number", t));
        Ok(Drive::new(number(speed)?, number(turn)?, from))
    }
}

/// Speed pulses per metre: 2500 to the kilometre, which is the distance
/// per pulse the navigation software starts out with (0.4 m).
const PULSES_PER_METRE: f64 = 2.5;
/// A/D reading of the gyroscope at rest (the software's samples are
/// 3720 - 4 * reading, of twelve bits: this is the middle), and the
/// change for one degree per second, for a gyroscope of 25 mV per degree
/// and second on a converter of 5 V and ten bits.  The software works
/// out both for itself in time; which way round the voltage goes is a
/// guess.
const GYRO_REST: f64 = 418.0;
const GYRO_PER_DPS: f64 = 5.12;

/// What the machine keeps about the external watchdog.
#[derive(Default, Clone, Copy)]
pub struct Watchdog {
    /// Strobe edges counted so far.
    pub edges: u64,
    /// Cycle count at the last edge (or at the start).
    pub last: u64,
    /// Longest time between two edges, in cycles.
    pub longest: u64,
}

pub struct Machine {
    pub cpu: Cpu,
    pub board: Board,
    /// Text the firmware printed on its debug serial port.
    pub console: Vec<u8>,
    pub echo_console: bool,
    /// CPU cycles per display frame.
    pub frame_cycles: u64,
    /// The drive's audio output since it was last taken away: 44.1 kHz
    /// stereo, a sample for every moment of emulated time (silence while
    /// nothing plays).  `None`: nobody is listening.  It is the signal
    /// at the drive, before the unit's volume control and amplifier,
    /// which belong to the radio processor.
    pub sound: Option<Vec<i16>>,
    /// The same through the amplifier as far as it is known: silent
    /// unless the unit has selected the CD drive as its source, and
    /// scaled by the volume the unit has set (see `dev/radio.rs`).
    pub speaker: Option<Vec<i16>>,
    /// Sound from the 193's own output (its serial sound interface)
    /// that has not gone into `speaker` yet, at 44.1 kHz.
    decoded: VecDeque<i16>,
    /// The last sample each of the 192's card processors sent.
    media_last: [[i16; 2]; 2],
    /// When the sound output is next served, in CPU cycles.
    next_sound: u64,
    /// The external watchdog as last looked at.
    pub watchdog: Watchdog,
    /// The watchdog bites from the start, not only once the software
    /// has begun to strobe it.
    pub watchdog_always: bool,
    next_frame: u64,
    /// Module entry points still to be called by the boot stand-in.
    boot_calls: VecDeque<(String, u32)>,
    /// Set when the firmware asked for a restart.
    pub reset_requested: bool,
    pub trace_bios: bool,
    /// Number of calls seen per boot ROM service (trap, function code).
    pub bios_calls: BTreeMap<(u8, u32), u64>,
    /// Memory break of the two pools managed by boot ROM services 4/5.
    pub bios_break: [u32; 8],
    /// Address of the firmware's `printf`, if known.  The main software
    /// formats debug messages but does not send them to a serial port;
    /// with this set the emulator prints them itself.
    pub printf_hook: Option<u32>,
    /// Further routines of the same kind: the navigation application
    /// has its own copy of `printf` for its debug messages.
    pub printf_hooks: Vec<u32>,
    /// A GPS fix handed to the navigation application: the fix, the
    /// address of the OS routine it asks, and that of the pointer to the
    /// OS's GPS interface data (flags first, the record for the
    /// application at +0x8ec).  See `dev/gps.rs`.
    pub gps: Option<(crate::dev::gps::Gps, u32, u32)>,
    /// The car's motion, if it moves.
    pub drive: Option<Drive>,
    /// How many of the library's results have been replaced by the fix.
    pub gps_fixes: u64,
    /// Frames sent to the unit periodically, standing in for the rest
    /// of the car: (frame, period in cycles, next due).
    can_cyclic: Vec<(Frame, u64, u64)>,
    /// The far ends of transport channels on the bus (the instrument
    /// cluster, the telephone module), and how many of the unit's frames
    /// they have seen.
    pub can_peers: Vec<crate::dev::tp20::Peer>,
    /// Blocks for the peers to send: (time in ms, peer, block).
    pub can_says: Vec<(u64, usize, Vec<u8>)>,
    can_seen: u64,
    /// Frames due and waiting for the bus, and when it is free again.
    can_waiting: VecDeque<Frame>,
    can_bus_free: u64,
    /// The mailbox whose frame the unit has on the bus right now.
    can_sending: Option<usize>,
    pub ignition: bool,
    /// Print traffic on the radio processor's serial link.
    pub trace_serial: bool,
    /// Stand-in for the radio/audio processor on serial channel 1.
    pub radio: crate::dev::radio::Radio,
    /// When false, nothing answers on that link (as if unplugged).
    pub radio_present: bool,
    /// Stand-in for the faceplate controller on serial channel 3.
    pub panel: crate::dev::panel::Panel,
    pub panel_present: bool,
    /// What the faceplate controller has answered and when it arrives.
    panel_replies: VecDeque<(u64, Vec<u8>)>,
    radio_replies: VecDeque<(u64, Vec<u8>)>,
    /// Scripted faceplate input: (time in ms, action), any order.
    pub panel_script: Vec<(u64, PanelAction)>,
}

/// Raw faceplate input, in terms of the controller's protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Key state bit (0..47) goes down or up.
    Key(usize, bool),
    /// Knob 0 or 1 turned by so many detents.
    Knob(usize, i32),
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

/// Stands for the boot ROM's own handler in the CAN tables it hands to
/// the OS, which only checks that there is one before replacing it.
const HANDOVER_HANDLER: u32 = 0xa000_0000;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
}

impl Machine {
    /// A 192 unit.
    pub fn new() -> Self {
        Self::for_model(crate::board::Model::Rnse192)
    }

    pub fn for_model(model: crate::board::Model) -> Self {
        let mut cpu = Cpu::new();
        // One instruction is counted as one CPU clock cycle.  The CPU
        // clocks are those of the parts: 4 x 58.8 MHz = 235 MHz for the
        // 192's SH7750R (a 240 MHz part), 8 x 50 MHz = 400 MHz for the
        // 193's SH-4A (a 400 MHz part).  The firmware's calibrated delay
        // loops agree in proportion: the 193's does 1056 instructions
        // where the 192's does 304.  The peripheral clocks follow from
        // the operating system's tick being 1 ms (see `cycles_per_ms`).
        cpu.io.pclk_div = 4;
        cpu.io.pclk_hz = 58_800_000;
        if model == crate::board::Model::Rnse193 {
            cpu.io.navi = Some(Box::default());
            cpu.io.intc.int2 = Some(Default::default());
            cpu.io.pclk_div = 8;
            cpu.io.pclk_hz = 50_000_000;
        }
        // A display frame every 60th of a second.
        let per_ms = cpu.io.pclk_div * cpu.io.pclk_hz / 1000;
        let frame = per_ms * 1000 / 60;
        let mut m = Machine { cpu, board: Board::for_model(model), console: Vec::new(), echo_console: true, frame_cycles: frame, sound: None, speaker: None, decoded: VecDeque::new(), media_last: [[0; 2]; 2], next_sound: 0, watchdog: Watchdog::default(), watchdog_always: false, next_frame: frame, boot_calls: VecDeque::new(), reset_requested: false, trace_bios: false, bios_calls: BTreeMap::new(), bios_break: [0; 8], printf_hook: None, printf_hooks: Vec::new(), gps: None, drive: None, gps_fixes: 0, can_cyclic: Vec::new(), can_peers: Vec::new(), can_says: Vec::new(), can_seen: 0, can_waiting: VecDeque::new(), can_bus_free: 0, can_sending: None, ignition: false, trace_serial: false, radio: crate::dev::radio::Radio::new(), radio_present: true, panel: crate::dev::panel::Panel::new(), panel_present: true, panel_replies: VecDeque::new(), radio_replies: VecDeque::new(), panel_script: Vec::new() };
        if model == crate::board::Model::Rnse192 {
            // The 192's drive as one was measured (docs/boot-rom.md, "A
            // disc that goes in while the unit runs"): 2.7 s to load a
            // disc, 3.6 s to push one out, 5.5 ms to read a sector
            // (when its times are on).  A disc it has loaded it does
            // not turn until it is told to start; how long it then
            // takes to be ready is not measured, two seconds are taken.
            m.board.atapi.spin_up_micros = 2_700_000;
            m.board.atapi.needs_start = true;
            m.board.atapi.settle_micros = 2_000_000;
            m.board.atapi.eject_micros = 3_600_000;
            m.board.atapi.sector_micros = 5_500;
            // The levels on the processor's own port pins, all inputs,
            // as a unit's report has them.  What they are is not known.
            m.cpu.io.pdtra_in = 0x3d24;
            m.cpu.io.pdtrb_in = 0xf97f;
        }
        m
    }

    /// Leave the timers running with their interrupts enabled and open
    /// in the interrupt controller, as a boot ROM that used them before
    /// it started the software may.  (Both stock loaders begin by
    /// stopping the timers and clearing the controller, the 193's nine
    /// timers one by one; nothing says what the boot ROM really leaves.)
    /// Software that enables interrupts without doing the same gets
    /// interrupts it did not ask for.
    pub fn busy_start(&mut self) {
        let is_193 = self.cpu.io.navi.is_some();
        let mut writes: Vec<(u32, u32, u32)> = Vec::new();
        for ch in 0..3u32 {
            let base = 0xffd8_0008 + ch * 12;
            writes.push((base, 4, 0x0000_4000)); // TCOR
            writes.push((base + 4, 4, 0x0000_4000)); // TCNT
            writes.push((base + 8, 2, 0x0020)); // TCR: underflow interrupt
        }
        if is_193 {
            writes.push((0xff80_4000, 4, 0x1f1f_1f00)); // INT2PRI0: the timers
            writes.push((0xff80_403c, 4, 0x0000_0001)); // their mask cleared
        } else {
            writes.push((0xffd0_0004, 2, 0xfff0)); // IPRA: the timers at level 15
        }
        writes.push((0xffd8_0004, 1, 0x07)); // TSTR: all three count
        let now = self.cpu.cycles;
        for (addr, size, v) in writes {
            self.cpu.io.write(&mut self.board, addr, size, v, now, 0);
        }
    }

    /// Model the CPU's caches from here on (see `sh4/cache.rs`), with
    /// the cache control register as the stock operating system of
    /// either unit leaves it, which is what a loader finds.
    pub fn cache_model(&mut self) {
        let ccr = crate::sh4::cache::STOCK_CCR;
        self.cpu.mmu.ccr = ccr;
        self.cpu.cache.enable(ccr);
    }

    /// Fill RAM with arbitrary data, as it is after power-up on real
    /// hardware (and with the last run's leftovers after a reset).  To
    /// be called before anything is loaded.  Software that only works
    /// when memory starts out as zeroes shows itself this way.
    pub fn scramble_ram(&mut self) {
        let mut x: u32 = 0x2545_f491;
        for b in self.board.ram.chunks_mut(4) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let w = x.to_be_bytes();
            b.copy_from_slice(&w[..b.len()]);
        }
    }

    /// Place every module of a variant at its load address, the way the
    /// update loader leaves the flash.
    pub fn install(&mut self, v: &Variant, index: &IndexInfo) {
        for m in &v.modules {
            if m.kind != "LDR" {
                self.board.load(m.phys_addr(), &m.data[..m.size as usize]);
            }
        }
        if index.audver.len() == 8 {
            // A radio processor running the software that goes with this set.
            self.radio.version.copy_from_slice(index.audver.as_bytes());
        }
        if let Some(os) = v.module("OSM") {
            // The module directory sits right below the OS.
            let dir = crate::fw::module_directory_for(v, index);
            self.board.load(os.phys_addr() - dir.len() as u32, &dir);
        }
    }

    /// Give the unit its serial number (fourteen characters, "AUZ..." on
    /// an Audi unit).  The 193 keeps it in the parameter block of its
    /// flash, at 0x3ff401f; that block is written at the factory and is
    /// never on an update disc, so an emulated unit has none unless it
    /// is given one.  Navigation discs can be tied to one unit's serial
    /// number, see docs/hardware-193.md.
    pub fn set_serial(&mut self, serial: &str) -> Result<(), String> {
        if self.board.model != crate::board::Model::Rnse193 {
            return Err("where the 192 keeps its serial number is not known".into());
        }
        if serial.len() != 14 || !serial.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("a serial number is 14 letters and digits".into());
        }
        self.board.load(0x03ff_401f, serial.as_bytes());
        Ok(())
    }

    /// Write the boot ROM stand-in (see `bootrom.rs`).
    pub fn install_boot_stub(&mut self, project: &str) {
        bootrom::install(&mut self.board, project);
        self.cpu.hypercalls_enabled = true;
        if self.board.model == crate::board::Model::Rnse192 {
            // What a 192's ROM sets in its reset code before anything
            // else runs (docs/boot-rom.md, "The real one"): the ten
            // registers at 0x16000000, the latch among them, and the
            // data and direction of the eight ports.  The software
            // that comes after it changes single bits of these and
            // sets no directions.
            use crate::sh4::Bus;
            for (addr, v) in [
                (0x1600_0000u32, 0xffff_ffc0u32),
                (0x1600_0004, 0x0000_3f8f),
                (0x1600_0008, 0x7e00_0004),
                (0x1600_000c, 0x0003_0000),
                (0x1600_0010, 0x0200_0000),
                (0x1600_0014, 0x0155_0000),
                (0x1600_0018, 0x0caa_0000),
                (0x1600_001c, 0x8068_98c0),
                (0x1600_0020, 0x0001_280b),
                (0x1600_0024, 0x4000_0000),
            ] {
                self.board.write32(addr, v);
            }
            for (port, data, dir) in [(0u32, 0x00u8, 0x01u8), (1, 0x80, 0x80), (2, 0x02, 0x02), (3, 0x18, 0x18), (4, 0x00, 0x00), (5, 0x60, 0xfe), (6, 0x00, 0x20), (7, 0x10, 0x1f)] {
                let base = 0x1230_0000 + port * 0x10_0000;
                self.board.write8(base + 1, data);
                self.board.write8(base, dir);
            }
            self.board.watchdog_edges = 0;
            // And the bus controller, which nothing after the ROM
            // touches: what a unit's report has there.
            let now = self.cpu.cycles;
            for (addr, size, v) in [
                (0xff80_0000u32, 4u32, 0x0100_0008u32), // BCR1
                (0xff80_0004, 2, 0xfd38),               // BCR2
                (0xff80_0008, 4, 0x1111_1011),          // WCR1
                (0xff80_000c, 4, 0x2082_624d),          // WCR2
                (0xff80_0014, 4, 0x0002_901c),          // MCR
                (0xff80_0024, 2, 0xa5e7),               // RTCOR
                (0xff80_001c, 2, 0xa508),               // RTCSR: the refresh timer runs
            ] {
                self.cpu.io.write(&mut self.board, addr, size, v, now, 0);
            }
        }
    }

    /// Put a real boot ROM (read from a unit; never on a disc) where
    /// the stand-in is and start at the reset vector.  From here on
    /// nothing is done for the software: no entry point is called, no
    /// service answered, and the stand-in's hypercall opcode is an
    /// instruction like any other.
    pub fn start_from_reset(&mut self, rom: &[u8]) -> Result<(), String> {
        if self.board.model != crate::board::Model::Rnse192 {
            return Err("a real boot ROM is only known for the 192".into());
        }
        if rom.len() > 0xc000 {
            return Err(format!("the 192's boot ROM is 0xc000 bytes, the file has {:#x}", rom.len()));
        }
        self.board.flash[..rom.len()].copy_from_slice(rom);
        self.cpu.hypercalls_enabled = false;
        self.cpu.trace_services = self.trace_bios;
        self.boot_calls.clear();
        self.cpu.reset();
        Ok(())
    }

    /// Start the update loader (LDM) the way the boot ROM does after it
    /// has copied it from the disc into RAM: call its first entry point
    /// in privileged mode.  The three byte arguments are whatever the
    /// boot ROM passes in r4-r6; their meaning is not known yet.
    pub fn start_loader(&mut self, ldm: &Module, args: [u32; 3]) {
        self.board.load(ldm.phys_addr(), &ldm.data[..ldm.size as usize]);
        self.cpu.reset();
        self.cpu.set_sr(SR_MD | SR_BL | 0xf0);
        self.cpu.pc = ldm.entries[0];
        self.cpu.r[4] = args[0];
        self.cpu.r[5] = args[1];
        self.cpu.r[6] = args[2];
        // The 193's loader sets its own stack; the 192's uses this one.
        self.cpu.r[15] = 0x8000_0000 + self.board.ram_base + self.board.ram.len() as u32 - 0x0200_0100;
        self.cpu.pr = 0;
    }

    /// Address of `printf` in OS builds that have been looked at, keyed
    /// by the OS module's version string.
    pub fn known_printf(os_version: &str) -> Option<u32> {
        match os_version {
            "V2200815" => Some(0x801e_b344),
            "V2220371" => Some(0x801d_e25c),
            // The 193's loaders format their log the same way and then
            // drop it.
            "V5740002" => Some(0x09e3_937a),
            "vB480000" => Some(0x09e3_9372),
            // The 193's OS builds.
            "V5741040" => Some(0x8068_8afe),
            "VB481000" => Some(0x8068_8146),
            "V57510AA" => Some(0x8067_b886),
            _ => None,
        }
    }

    /// The debug `printf` of the navigation application (module `APM`),
    /// by its version.
    pub fn known_app_printf(apm_version: &str) -> Option<u32> {
        match apm_version {
            "V57510AA" => Some(0x021a_2b16),
            // The 192's European application.
            "V2200815" => Some(0x0071_9898),
            _ => None,
        }
    }

    /// Print the debug messages of a variant's software: of its update
    /// loader, or of the OS and every other module that has a `printf`
    /// of its own (the navigation application and, on the 193, the
    /// others too).  The addresses are found in the code where they can
    /// be and come from the tables by version otherwise.
    pub fn hook_debug_output(&mut self, v: &Variant, loader: bool) {
        let main = v.module(if loader { "LDM" } else { "OSM" });
        if let Some(a) = main.and_then(|m| Self::known_printf(&m.version).or_else(|| Self::find_printf(m))) {
            self.hook_printf(a);
        }
        if loader {
            return;
        }
        for m in v.modules.iter().filter(|m| m.kind == "APL") {
            let found = if m.name == "APM" { Self::known_app_printf(&m.version) } else { None };
            if let Some(a) = found.or_else(|| Self::find_printf(m)) {
                self.hook_printf_also(a);
            }
        }
    }

    /// The debug `printf` of a module of the 193's software, found by
    /// its code: the C library there has three routines of the same
    /// beginning in a row (0x24 bytes apart), of which `printf` is the
    /// last.  Every module with debug messages has its own copy.
    pub fn find_printf(module: &Module) -> Option<u32> {
        const START: [(u16, u16); 10] = [(0x4f22, !0), (0x66f3, !0), (0x7604, !0), (0x6563, !0), (0x7504, !0), (0x6053, !0), (0xc903, !0), (0x6203, !0), (0x2228, !0), (0x8901, !0)];
        let found = module.find_code(&START);
        found.windows(3).rev().find(|w| w[1] == w[0] + 0x24 && w[2] == w[1] + 0x24).map(|w| w[2])
    }

    /// The routine through which the 193's navigation application gets
    /// the GPS library's latest fix (service 0x10a of trap 33), found by
    /// its code, and the address of the pointer it reads the interface
    /// task's data through.
    pub fn find_gps_getter(os: &Module) -> Option<(u32, u32)> {
        const CODE: [(u16, u16); 14] = [
            (0x4f22, !0), (0xd100, 0xff00), (0x6612, !0), (0x6062, !0), (0xc801, !0), (0x890f, !0), (0x2668, !0),
            (0x890d, !0), (0x9100, 0xff00), (0xd700, 0xff00), (0x361c, !0), (0x6563, !0), (0x470b, !0), (0xe62f, !0),
        ];
        let found = os.find_code(&CODE);
        let [getter] = found[..] else { return None };
        // The second instruction loads the pointer's address from the
        // literal pool.
        let load = getter + 2;
        let disp = (os.long_at(load & !3)? >> if load & 2 != 0 { 0 } else { 16 }) & 0xff;
        let context = os.long_at((load & !3) + 4 + disp * 4)?;
        Some((getter, context))
    }

    /// Code the unit for a kind of car, as a workshop does (193).  The
    /// coding is kept in battery-backed memory in a block of 64 bytes at
    /// 0xb8002c4c: a mark, a version, a sum of the bytes that count, six
    /// bytes, four bytes, six bytes of which the first is the car type
    /// (1 to 7), and the mark inverted at the end.  The OS takes the
    /// block as it finds it if mark and sum are right, and otherwise
    /// clears it: a unit that has never been coded has car type 0.
    ///
    /// The type decides, among other things, where the navigation takes
    /// the distance driven from: only type 5 counts the speed pulses on
    /// timer 2's capture pin.
    pub fn set_car_type(&mut self, car_type: u8) -> Result<(), String> {
        if self.board.model != crate::board::Model::Rnse193 {
            return Err("where the 192 keeps its coding is not known".into());
        }
        if !(1..=7).contains(&car_type) {
            return Err("car types are 1 to 7".into());
        }
        let mut block = [0u8; 64];
        block[0..4].copy_from_slice(&0x1234_5678u32.to_be_bytes());
        block[4..8].copy_from_slice(&0x0222_0001u32.to_be_bytes());
        block[0x16] = car_type;
        let sum: u32 = block[0x0c..0x1c].iter().map(|&b| b as u32).sum();
        block[8..12].copy_from_slice(&sum.to_be_bytes());
        block[60..64].copy_from_slice(&0xedcb_a987u32.to_be_bytes());
        self.board.nvram[0x2c4c..0x2c8c].copy_from_slice(&block);
        Ok(())
    }

    /// Have the unit receive a GPS fix: `getter` and `context` are what
    /// [`find_gps_getter`](Self::find_gps_getter) finds.
    pub fn set_gps(&mut self, fix: crate::dev::gps::Gps, getter: u32, context: u32) {
        self.gps = Some((fix, getter, context));
        if !self.cpu.breakpoints.contains(&getter) {
            self.cpu.breakpoints.push(getter);
        }
    }

    /// Print what a further `printf` of the firmware is given, like
    /// [`hook_printf`](Self::hook_printf).
    pub fn hook_printf_also(&mut self, addr: u32) {
        self.printf_hooks.push(addr);
        if !self.cpu.breakpoints.contains(&addr) {
            self.cpu.breakpoints.push(addr);
        }
    }

    /// Print the firmware's own debug messages (see `printf_hook`).
    pub fn hook_printf(&mut self, addr: u32) {
        self.printf_hook = Some(addr);
        if !self.cpu.breakpoints.contains(&addr) {
            self.cpu.breakpoints.push(addr);
        }
    }

    /// Change the CPU clock to `ratio` times the peripheral clock (4
    /// on the 192 and 8 on the 193 unless told otherwise), before the
    /// machine runs.  The emulator counts an instruction as a cycle
    /// and the real ratio of the CPU's pace to the timers' is not
    /// known, so anything that depends on it deserves a look at other
    /// ratios.
    pub fn set_cpu_ratio(&mut self, ratio: u64) {
        self.cpu.io.pclk_div = ratio.max(1);
        self.board.media.per_second = self.cycles_per_ms() * 1000;
        self.frame_cycles = self.cycles_per_ms() * 1000 / 60;
        self.next_frame = self.cpu.cycles + self.frame_cycles;
    }

    /// CPU cycles per millisecond of emulated time.
    pub fn cycles_per_ms(&self) -> u64 {
        // Peripheral clock: 58.8 MHz on the 192, 50 MHz on the 193.
        // The operating system loads its tick timer with 14700 counts
        // of a quarter of that clock on the 192 and 12488 on the 193,
        // and the tick is 1 ms: on the 193 the time the unit shows for
        // a playing MP3 file advances by a second for every 44100
        // samples it sends out, the faceplate port's divisor makes
        // 9600 bit/s and the CAN controller's bit timing 100 kbit/s
        // only from 50 MHz.  The 192's software is the same with every
        // timer value scaled by 58.8/50, so its tick is taken to be
        // 1 ms as well; nothing measures it independently.
        self.cpu.io.pclk_div * self.cpu.io.pclk_hz / 1000
    }

    /// Send `frame` to the unit every `period_ms` milliseconds, as another
    /// control unit on the bus would.  A second call with the same
    /// identifier replaces the earlier frame.
    pub fn can_send_cyclic(&mut self, frame: Frame, period_ms: u64) {
        let period = period_ms.max(1) * self.cycles_per_ms();
        let due = self.cpu.cycles + period;
        self.can_cyclic.retain(|(f, _, _)| f.id != frame.id);
        self.can_cyclic.push((frame, period, due));
    }

    /// Ignition lock status as broadcast on the infotainment CAN bus
    /// (identifier 0x271, byte 0: bit 0 = key in the lock, bit 1 =
    /// ignition on).  The power manager reads it to decide whether to
    /// run.
    pub fn set_ignition(&mut self, on: bool) {
        self.ignition = on;
        self.can_send_cyclic(Frame::new(0x271, &[if on { 0x03 } else { 0x00 }]), 100);
    }

    /// A faceplate key went down or up.  Names are those of
    /// [`crate::dev::panel::KEYS`]; returns false for a key the emulated
    /// faceplate controller does not have (the power button).
    pub fn panel_key(&mut self, name: &str, down: bool) -> bool {
        match crate::dev::panel::key_bit(name) {
            Some(bit) => {
                self.panel.key(bit, down);
                true
            }
            None => false,
        }
    }

    /// A faceplate knob ("main" or "volume") was turned by `delta`
    /// detents, positive = clockwise.
    pub fn panel_knob(&mut self, name: &str, delta: i32) -> bool {
        let knob = match name {
            "main" => crate::dev::panel::KNOB_SELECT,
            "volume" => crate::dev::panel::KNOB_VOLUME,
            _ => return false,
        };
        self.panel.knob(knob, delta);
        true
    }

    /// The CD/DVD drive, wherever this model has it.
    pub fn atapi(&mut self) -> &mut crate::dev::atapi::Atapi {
        match self.cpu.io.navi.as_mut() {
            Some(navi) => &mut navi.atapi,
            None => &mut self.board.atapi,
        }
    }

    /// The CAN controller, wherever this model has it.
    pub fn can(&mut self) -> &mut crate::dev::hcan::Hcan {
        match self.cpu.io.navi.as_mut() {
            Some(navi) => &mut navi.can,
            None => &mut self.board.can,
        }
    }

    /// The picture on the unit's screen: width, height, RGB888.
    pub fn render(&self) -> (usize, usize, Vec<u8>) {
        match self.cpu.io.navi.as_ref() {
            Some(navi) => {
                let (ram, base) = (&self.board.ram, self.board.ram_base);
                navi.render(&|a| ram.get(a.wrapping_sub(base) as usize).copied().unwrap_or(0))
            }
            None => self.board.coral.render(),
        }
    }

    /// Hardware the software touched that the emulator has no model of:
    /// (address, reads, writes, PC of the first access).
    pub fn unmodelled(&self) -> Vec<(u32, u32, u32, u32)> {
        let mut all: Vec<_> = self.board.unmodelled.seen.iter().map(|(a, (r, w, pc))| (*a, *r, *w, *pc)).collect();
        if let Some(navi) = self.cpu.io.navi.as_ref() {
            all.extend(navi.unmodelled.seen.iter().map(|(a, (r, w, pc))| (*a, *r, *w, *pc)));
        }
        all.sort();
        all
    }

    /// Position of the motorised display, 0 (open) to 1023 (closed).
    pub fn display_pos(&self) -> u16 {
        match self.cpu.io.navi.as_ref() {
            Some(navi) => navi.display_pos,
            None => self.board.display_pos,
        }
    }

    /// Summary of the display hardware's state, for diagnostics.
    pub fn describe_display(&self) -> String {
        match self.cpu.io.navi.as_ref() {
            Some(navi) => navi.describe(),
            None => self.board.coral.describe(),
        }
    }

    /// Exchange bytes with the faceplate controller.
    fn panel_link(&mut self) {
        let ms = self.cpu.cycles / self.cycles_per_ms();
        if !self.panel_script.is_empty() {
            let mut i = 0;
            while i < self.panel_script.len() {
                if self.panel_script[i].0 <= ms {
                    match self.panel_script.remove(i).1 {
                        PanelAction::Key(bit, down) => self.panel.key(bit, down),
                        PanelAction::Knob(knob, delta) => self.panel.knob(knob, delta),
                    }
                } else {
                    i += 1;
                }
            }
        }
        let reset = match self.cpu.io.navi.as_ref() {
            Some(navi) => navi.panel_reset(),
            None => self.board.latch16() & 1 == 0,
        };
        if reset != self.panel.in_reset {
            if self.trace_serial {
                eprintln!("[panel] {:>7} ms  reset line {}", ms, if reset { "low" } else { "high" });
            }
            self.panel.set_reset(reset, ms);
            if reset {
                self.panel_replies.clear();
            }
        }
        let now = self.cpu.cycles;
        // One character on the line: ten bits at 9600 baud (assumed
        // from the 192's divisor of 95 and a 14.7456 MHz clock).
        let char_time = self.cycles_per_ms() * 10 / 96 * 10;
        let uart: &mut dyn SerialPort = match self.cpu.io.navi.as_mut() {
            Some(navi) => &mut navi.panel_scif,
            None => &mut self.board.uart[3],
        };
        let mut changed = false;
        let bytes = uart.take_tx();
        if !bytes.is_empty() {
            if self.trace_serial {
                eprintln!("[panel] {:>7} ms  unit -> {}", ms, hex(&bytes));
            }
            if self.panel_present {
                let reply = self.panel.receive(&bytes, ms);
                if !reply.is_empty() {
                    // The answer cannot arrive before the unit's bytes
                    // have gone out and one of the controller's come in.
                    self.panel_replies.push_back((now + (bytes.len() as u64 + 1) * char_time, reply));
                }
            }
            changed = true;
        }
        while self.panel_replies.front().is_some_and(|r| r.0 <= now) {
            let reply = self.panel_replies.pop_front().unwrap().1;
            if self.trace_serial {
                eprintln!("[panel] {:>7} ms  unit <- {}", ms, hex(&reply));
            }
            uart.feed(&reply);
            changed = true;
        }
        // The controller only talks once the unit's receiver is on, and
        // not into the middle of something the unit has yet to read.
        if self.panel_present && self.panel_replies.is_empty() && uart.rx_idle() {
            if let Some(f) = self.panel.poll(ms) {
                if self.trace_serial {
                    eprintln!("[panel] {:>7} ms  unit <- {}", ms, hex(&f));
                }
                uart.feed(&f);
                changed = true;
            }
        }
        if changed {
            self.update_irqs();
        }
    }

    /// Exchange bytes with the radio processor.
    fn radio_link(&mut self) {
        let ms = self.cpu.cycles / self.cycles_per_ms();
        let now = self.cpu.cycles;
        let char_time = self.cycles_per_ms() * 10 / 96 * 10;
        let uart: &mut dyn SerialPort = match self.cpu.io.navi.as_mut() {
            Some(navi) => &mut navi.radio_uart,
            None => &mut self.board.uart[1],
        };
        let mut changed = false;
        let bytes = uart.take_tx();
        if !bytes.is_empty() {
            if self.trace_serial {
                eprintln!("[radio] {:>7} ms  unit -> {}", ms, hex(&bytes));
            }
            if self.radio_present {
                let reply = self.radio.from_unit(&bytes);
                if !reply.is_empty() {
                    // As with the faceplate: an answer takes at least
                    // the time of the bytes on the line.
                    self.radio_replies.push_back((now + (bytes.len() as u64 + 1) * char_time, reply));
                }
            }
            changed = true;
        }
        while self.radio_replies.front().is_some_and(|r| r.0 <= now) {
            let reply = self.radio_replies.pop_front().unwrap().1;
            if self.trace_serial {
                eprintln!("[radio] {:>7} ms  unit <- {}", ms, hex(&reply));
            }
            uart.feed(&reply);
            changed = true;
        }
        if changed {
            self.update_irqs();
        }
    }

    /// Bring the interrupt lines up to date after a device changed state.
    fn update_irqs(&mut self) {
        self.board.update_irl();
        if let Some(navi) = self.cpu.io.navi.as_mut() {
            navi.update_irqs(&mut self.cpu.io.intc);
        }
    }

    /// Let the car move for the time that has passed.
    fn drive_tick(&mut self) {
        let now = self.cpu.cycles;
        let per_ms = self.cycles_per_ms() as f64;
        let Some(drive) = self.drive.as_mut() else { return };
        let seconds = (now - drive.last.min(now)) as f64 / per_ms / 1000.0;
        if drive.last == 0 || seconds > 1.0 {
            drive.last = now;
            return;
        }
        drive.last = now;
        let started = now as f64 / per_ms >= drive.from as f64;
        let metres = if started { drive.speed / 3.6 * seconds } else { 0.0 };
        drive.pulses += metres * PULSES_PER_METRE;
        let edges = drive.pulses as u32;
        drive.pulses -= edges as f64;
        let (turn, moving) = (drive.turn, started && drive.speed != 0.0);
        for _ in 0..edges {
            self.cpu.io.tmu2_capture(now);
        }
        if let Some(navi) = self.cpu.io.navi.as_mut() {
            navi.adc.input[1] = (GYRO_REST + if moving { turn * GYRO_PER_DPS } else { 0.0 }).clamp(0.0, 1023.0) as u16;
        }
        if let Some((fix, _, _)) = self.gps.as_mut() {
            fix.speed = if moving { metres / seconds * 3.6 } else { 0.0 };
            if moving {
                fix.heading += turn * seconds;
                fix.advance(metres);
            }
        }
    }

    /// Show the transport peers what the unit has sent since they last
    /// looked, and put their answers on the bus.
    fn can_peers_tick(&mut self) {
        if !self.can_says.is_empty() {
            let ms = self.cpu.cycles / self.cycles_per_ms();
            let mut i = 0;
            while i < self.can_says.len() {
                if self.can_says[i].0 <= ms {
                    let (_, peer, block) = self.can_says.remove(i);
                    if let Some(p) = self.can_peers.get_mut(peer) {
                        if p.open {
                            let frames = p.send(&block);
                            self.can_waiting.extend(frames);
                        }
                    }
                } else {
                    i += 1;
                }
            }
        }
        let seen = self.can_seen;
        let (count, new): (u64, Vec<Frame>) = {
            let can = self.can();
            if can.tx_count == seen {
                return;
            }
            let n = ((can.tx_count - seen) as usize).min(can.tx.len());
            (can.tx_count, can.tx.iter().skip(can.tx.len() - n).cloned().collect())
        };
        self.can_seen = count;
        for f in &new {
            for peer in self.can_peers.iter_mut() {
                self.can_waiting.extend(peer.from_unit(f));
            }
        }
    }

    /// A frame from another node: it goes onto the bus when the bus is
    /// free and reaches the controller with its interrupt.
    pub fn can_frame(&mut self, frame: Frame) {
        self.can_waiting.push_back(frame);
    }

    fn can_tick(&mut self) {
        let now = self.cpu.cycles;
        for (frame, period, due) in self.can_cyclic.iter_mut() {
            if now >= *due {
                *due += *period;
                if *due <= now {
                    *due = now + *period;
                }
                // Onto the bus, behind whatever is waiting for it.
                if self.can_waiting.len() < 64 {
                    self.can_waiting.push_back(frame.clone());
                }
            }
        }
        // One frame at a time: at 100 kbit/s a frame of n data bytes
        // takes 47 + 8n bits, a fifth more at worst for stuff bits, and
        // three bits of pause, 10 microseconds each.
        if now >= self.can_bus_free {
            // What the unit was sending has gone out and been
            // acknowledged.
            if let Some(n) = self.can_sending.take() {
                self.can().tx_complete(n);
                self.board.update_irl();
                if let Some(navi) = self.cpu.io.navi.as_mut() {
                    navi.update_irqs(&mut self.cpu.io.intc);
                }
            }
            // The unit's own frame next, if it has one waiting: the
            // other nodes here are not in a hurry.
            if let Some((n, len)) = self.can().tx_next() {
                let bits = 50 + 8 * len as u64 + (34 + 8 * len as u64) / 5;
                self.can_bus_free = now + bits * self.cycles_per_ms() / 100;
                self.can_sending = Some(n);
            } else if let Some(frame) = self.can_waiting.pop_front() {
                let bits = 50 + 8 * frame.len as u64 + (34 + 8 * frame.len as u64) / 5;
                self.can_bus_free = now + bits * self.cycles_per_ms() / 100;
                match self.cpu.io.navi.as_mut() {
                    Some(navi) => navi.can.receive(&frame),
                    None => self.board.can.receive(&frame),
                };
                self.board.update_irl();
                if let Some(navi) = self.cpu.io.navi.as_mut() {
                    navi.update_irqs(&mut self.cpu.io.intc);
                }
            }
        }
    }

    /// Start the installed firmware the way the boot ROM does: call the
    /// OS module's entry points in order (init 1, init 2, start).  The
    /// last one does not return; it dispatches the first task.
    pub fn start_os(&mut self, v: &Variant) -> Result<(), String> {
        let os = v.module("OSM").ok_or("variant has no OSM module")?;
        self.cpu.reset();
        self.cpu.set_sr(SR_MD | 0xf0);
        self.cpu.vbr = 0x8000_0000 | bootrom::VBR;
        self.cpu.r[15] = 0x8fff_ff00;
        // The OS starts TMU0 for its 1 ms tick but never gives the timer an
        // interrupt priority, so it relies on the boot ROM having done
        // that.  The loader, which shares the boot ROM's hardware layer,
        // uses level 7 (IPRA = 0x7000).
        self.cpu.io.intc.ipra = 0x7000;
        if let Some(int2) = self.cpu.io.intc.int2.as_mut() {
            // The same on the 193, in its own controller: the loader
            // gives timer 0 level 7 and unmasks the timers; the OS only
            // sets up the other fields of that register.
            int2.pri[0] |= 0x000e_0000;
            int2.mask &= !(1 << 1);
        }
        if let Some(navi) = self.cpu.io.navi.as_mut() {
            // On the 193 the boot ROM also programs the CAN receive
            // mailboxes; the OS only unmasks their interrupts and looks
            // at the identifier of whatever arrives.  The one fixed point
            // is mailbox 24, which the OS (like the loader) treats as the
            // network management range 0x400-0x43f.  The real assignment
            // of the others is not known; a single catch-all mailbox
            // serves the OS just as well.
            navi.can.configure_mailbox(24, 0x400, 0x03f, 3);
            navi.can.configure_mailbox(31, 0x000, 0x7ff, 3);
        } else {
            // What follows is what a 192's ROM was seen to leave when it
            // calls the OS (docs/boot-rom.md, "The real one").
            //
            // The media driver preserves DMAOR.DME while selecting its
            // transfer mode; it never sets master enable itself.
            self.cpu.io.dmaor = 0xc001;
            // Timers 0 and 2 at levels 7 and 5.  Timer 1, which the
            // ROM's own CAN software uses for half a second at level 6,
            // is left as that leaves it: running at 5 ms like the
            // others, without a level.
            self.cpu.io.intc.ipra = 0x7050;
            let now = self.cpu.cycles;
            for (addr, size, v) in [(0xffd8_0014u32, 4u32, 0x0001_1f1bu32), (0xffd8_0018, 4, 0x0001_1f1b), (0xffd8_001c, 2, 0x0020)] {
                self.cpu.io.write(&mut self.board, addr, size, v, now, 0);
            }
            let started = self.cpu.io.read(&mut self.board, 0xffd8_0004, 1, now, 0);
            self.cpu.io.write(&mut self.board, 0xffd8_0004, 1, started | 2, now, 0);
            self.cpu.r[15] = 0x8fff_ffb0;
            self.bios_break = [0x8c00_0100, 0x9000_0000, 0, 0, 0, 0, 0, 0];
            // The start of the battery-backed memory is the ROM's: it
            // clears it after a reset, but for the word at 0x18.
            if self.board.nvram.len() >= 0x1c {
                self.board.nvram[..0x18].fill(0);
                self.board.nvram[0x1a..0x1c].fill(0);
            }
            // The HCAN driver leaves these three shared receive boxes'
            // modes alone (unlike its dedicated receive boxes). It uses
            // box 3 for 0x600..0x67f, box 2 for network management, and
            // box 0 for other identifiers dispatched through software
            // filters. Their masks must survive identifier writes when
            // further software consumers register.
            // As the ROM has them: 0 takes everything (its identifier
            // is the second ignition message's), 1 sends the unit's
            // network management message and 2 receives that range.
            self.board.can.configure_mailbox(0, 0x2c3, 0x7ff, 3);
            self.board.can.configure_mailbox(1, 0x436, 0, 0);
            self.board.can.configure_mailbox(2, 0x436, 0x03f, 3);
            self.board.can.configure_mailbox(3, 0, 0x07f, 3);
        }
        // Likewise the CAN controller: the OS sets up mailboxes and
        // handles interrupts but never takes the chip out of reset.
        self.can().start();
        self.boot_calls.clear();
        if v.name != "V220" {
            // The boot ROM calls the entry points of every module, stage
            // by stage, in directory order.  Only the oldest software
            // (V220) has its OS do that for the other modules itself; the
            // same code is still present in the later OS builds but is
            // not reached.  The OS's third entry does not return, so it
            // has to come last.
            let order = ["OSM", "APM", "APFM", "AUM", "FONT", "VRM", "DIG"];
            let modules: Vec<&Module> = order.iter().filter_map(|n| v.module(n)).collect();
            for (i, name) in ["init1", "init2", "start"].iter().enumerate() {
                for m in modules.iter().filter(|m| !(i == 2 && m.name == "OSM")) {
                    self.boot_calls.push_back((format!("{} {}", m.name, name), m.entries[i]));
                }
            }
            self.boot_calls.push_back(("OSM start".into(), os.entries[2]));
        } else {
            for (i, name) in ["init1", "init2", "start"].iter().enumerate() {
                self.boot_calls.push_back((format!("OSM {}", name), os.entries[i]));
            }
        }
        self.next_boot_call();
        Ok(())
    }

    fn next_boot_call(&mut self) {
        match self.boot_calls.pop_front() {
            Some((name, addr)) => {
                eprintln!("[boot] calling {} at {:08x}", name, addr);
                self.cpu.pc = addr;
                self.cpu.pr = 0x8000_0000 | bootrom::CALL_RETURN;
            }
            None => {
                self.cpu.fault = Some("boot sequence ran off its end".into());
            }
        }
    }

    /// Service a hypercall.  Returns false if execution cannot continue.
    fn hypercall(&mut self) -> bool {
        let Some(pc) = self.cpu.hypercall.take() else { return true };
        match bootrom::classify(pc) {
            Some(Hook::CallReturn) => {
                eprintln!("[boot] returned r0={:08x}", self.cpu.r[0]);
                self.next_boot_call();
            }
            Some(Hook::Reset) => {
                eprintln!("[boot] firmware jumped to the reset entry");
                self.reset_requested = true;
                self.cpu.fault = Some("reset requested".into());
            }
            Some(Hook::Trap(n)) => {
                // Reached through a trap dispatcher as an ordinary function:
                // function code in r0, arguments in r4-r7, result in r0.
                // The OS dispatcher pushed SPC, SSR and PR of the caller.
                let sp = self.cpu.r[15];
                let caller = self.peek32(sp.wrapping_add(8)).unwrap_or(0);
                self.bios_trap(n, caller);
            }
            Some(Hook::Exception) => {
                // The boot ROM's own vector base is still active.
                let code = self.cpu.io.expevt;
                if code == 0x160 {
                    let n = (self.cpu.io.tra >> 2) as u8;
                    let caller = self.cpu.pr;
                    self.bios_trap(n, caller);
                    // Return from the exception.
                    let (ssr, spc) = (self.cpu.ssr, self.cpu.spc);
                    self.cpu.set_sr(ssr);
                    self.cpu.pc = spc;
                } else {
                    self.cpu.fault = Some(format!("exception {:03x} at {:08x} before the OS installed its handlers", code, self.cpu.spc));
                }
            }
            Some(Hook::Interrupt) => {
                self.cpu.fault = Some(format!("interrupt {:03x} before the OS installed its handlers", self.cpu.io.intevt));
            }
            Some(Hook::Unhandled) => {
                self.cpu.fault = Some(format!(
                    "unhandled exception: EXPEVT={:03x} INTEVT={:03x} SPC={:08x} TEA={:08x}",
                    self.cpu.io.expevt, self.cpu.io.intevt, self.cpu.spc, self.cpu.mmu.tea
                ));
            }
            None => {
                self.cpu.fault = Some(format!("stray hypercall opcode at {:08x}", pc));
            }
        }
        self.cpu.fault.is_none()
    }

    /// Boot ROM system services (`TRAPA #0x3d..0x3f`, function code in
    /// r0).  The ones identified so far, all on trap 0x3e:
    ///
    /// | fn | arguments            | meaning                                  |
    /// |----|----------------------|------------------------------------------|
    /// | 1  | component number     | pointer to its version string ("V220...")|
    /// | 4  | pool                 | return the pool's memory break           |
    /// | 5  | pool, address        | set the pool's memory break              |
    /// | 6  | three out-pointers   | start-up status; 9 = cold start          |
    /// | 13 | -                    | pointer to SOFIDX/NAVVER/AUDVER strings  |
    ///
    /// Modules claim their static RAM with 4 and 5 (`base = get(); ...;
    /// set(base + size)`), the OS first sets the break to the end of its
    /// own data.  After a cold start the OS clears RAM from 0x8c00_0000
    /// to 0x8fc8_0000, which is why the boot stack lives above that.
    ///
    /// Anything else is logged and answered with 0.
    fn bios_trap(&mut self, n: u8, caller: u32) {
        let regs: Vec<u32> = self.cpu.bank0().to_vec();
        let code = regs[0];
        let is_192 = self.board.model == crate::board::Model::Rnse192;
        let count = self.bios_calls.entry((n, code)).or_insert(0);
        *count += 1;
        let shown = *count <= 8;
        let last = *count == 8;
        let ret = match (n, code) {
            // Eight pools on the 192, as its ROM has them (others answer
            // 0 and -1); what the 193's does with a pool number above 1
            // is not known.
            (0x3e, 4) if is_192 => self.bios_break.get(regs[4] as usize).copied().unwrap_or(0),
            (0x3e, 5) if is_192 => match self.bios_break.get_mut(regs[4] as usize) {
                Some(b) => {
                    *b = regs[5];
                    0
                }
                None => u32::MAX,
            },
            (0x3e, 4) => self.bios_break[(regs[4] & 1) as usize],
            (0x3e, 5) => {
                self.bios_break[(regs[4] & 1) as usize] = regs[5];
                0
            }
            // A text of the boot ROM's header: 1 its version (the
            // application checks the first three characters for other
            // hardware projects), 2 the version without the project, 3
            // and 4 its date and time, 6 its maker; anything else the
            // header itself.  (16 and 17 are two texts in the ROM's
            // data that the stand-in has not got.)
            (0x3e, 1) if is_192 => match regs[4] {
                1 => 0x8000_0406,
                2 => 0x8000_040a,
                3 => 0x8000_0410,
                4 => 0x8000_0420,
                5 => 0x8000_0430,
                6 => 0x8000_0440,
                _ => 0x8000_0400,
            },
            (0x3e, 1) => 0x8000_0406,
            (0x3e, 2) if is_192 => 0,
            // Start again: 0 through the reset code, 1 without it.
            (0x3e, 3) if is_192 && regs[4] <= 1 => {
                eprintln!("[boot] firmware asked the boot ROM for a restart ({})", regs[4]);
                self.reset_requested = true;
                self.cpu.fault = Some("reset requested".into());
                0
            }
            // A counter kept in battery-backed memory, eight of them at
            // 0x8c, each with its complement behind it: r5 is added and
            // the sum returned.  The OS adds 1 to counter 0 from its
            // tick now and then.
            (0x3e, 7) if is_192 => {
                let n = regs[4] as usize;
                if n < 8 && self.board.nvram.len() >= 0xcc {
                    let at = 0x8c + 8 * n;
                    let word = |nv: &[u8], o: usize| u32::from_be_bytes(nv[o..o + 4].try_into().unwrap());
                    let mut v = word(&self.board.nvram, at);
                    if v != !word(&self.board.nvram, at + 4) {
                        v = 0;
                    }
                    v = v.wrapping_add(regs[5]);
                    self.board.nvram[at..at + 4].copy_from_slice(&v.to_be_bytes());
                    self.board.nvram[at + 4..at + 8].copy_from_slice(&(!v).to_be_bytes());
                    v
                } else {
                    u32::MAX
                }
            }
            (0x3e, 8) if is_192 => {
                if regs[4] == u32::MAX && self.board.nvram.len() >= 0x20 {
                    self.board.nvram[0x1c..0x20].copy_from_slice(&0x4000_0000u32.to_be_bytes());
                    0
                } else {
                    u32::MAX
                }
            }
            // The count of restarts after an exception is cleared.
            (0x3e, 0x0a) if is_192 => {
                if self.board.nvram.len() >= 0x1c {
                    self.board.nvram[0x1a..0x1c].fill(0);
                }
                u32::MAX
            }
            // Numbers the ROM has no service for: its dispatcher's -1.
            (0x3e, 0x0b..=0x0f | 0x14..) if is_192 => u32::MAX,
            // The module directory: entry n of it (0 the directory
            // itself), and for service 0x12 the text six bytes into
            // that module's header.
            (0x3e, 0x10 | 0x12) if is_192 => {
                let count = u32::from_be_bytes(self.board.flash[0xc018..0xc01c].try_into().unwrap());
                let n = regs[4];
                if n == 0 && code == 0x10 {
                    0x8000_c000
                } else if n == 0 || n > count || n > 64 {
                    0
                } else {
                    let entry = 0xc01c + (n - 1) * 16;
                    if code == 0x10 {
                        0x8000_0000 | entry
                    } else {
                        let e = entry as usize + 12;
                        u32::from_be_bytes(self.board.flash[e..e + 4].try_into().unwrap()).wrapping_add(6)
                    }
                }
            }
            // Version block of the installed software: SOFIDX[4],
            // NAVVER[8], AUDVER[8] as stored in the module directory.
            (0x3e, 0x13) => {
                // The block follows the OS entry in V220's directory and
                // the module entries in the compact one (see fw.rs).
                let base: u32 = if self.board.model == crate::board::Model::Rnse193 { 0x40_0000 } else { 0xc000 };
                let b = base as usize;
                if self.board.flash[b + 0x1c] == b'.' {
                    let count = u32::from_be_bytes(self.board.flash[b + 0x18..b + 0x1c].try_into().unwrap()).min(10);
                    0x8000_0000 + base + 0x1c + count * 16
                } else {
                    0x8000_0000 + base + 0x1c
                }
            }
            // Called from the tick.
            (0x3e, 7) => {
                0
            }
            // Hand-over of the boot ROM's CAN state.  r4 points at two
            // pointers to lists of blocks, each list ended by 0xffff:
            // {number, -, buffer, size.w, -} (ten network-state blocks)
            // and {number.w, size.w, buffer}. Service 9 on the 192 uses
            // the first layout for BOTH lists. In the second
            // list block 2 is the table of received messages (32/36 entries
            // of 40 bytes: enabled.w, identifier.w, then three times
            // {handler, buffer, length.w, -}) and block 8 the state of the
            // ignition lock message that woke the unit: the identifiers
            // 0x271 and 0x2c3 (the same message on two generations of
            // car) and a "received" flag for each.  The OS swaps the
            // handlers of those two for its own and reads the first byte
            // of the message from the buffer to decide whether to start.
            (0x3e, 9) | (0x3e, 0xb) if code == 0xb || regs[7] == 2 => {
                let mut network = BTreeMap::new();
                let mut p = self.peek32(regs[4]).unwrap_or(0);
                for _ in 0..16 {
                    match self.peek16(p) {
                        Some(0xffff) | None => break,
                        Some(n) => network.insert(n, (self.peek32(p + 4).unwrap_or(0), self.peek16(p + 8).unwrap_or(0) as u32)),
                    };
                    p += 12;
                }
                let mut blocks = BTreeMap::new();
                let mut p = self.peek32(regs[4] + 4).unwrap_or(0);
                for _ in 0..16 {
                    match self.peek16(p) {
                        Some(0xffff) | None => break,
                        Some(n) => blocks.insert(n, (self.peek32(p + 4).unwrap_or(0), self.peek16(p + if code == 9 { 8 } else { 2 }).unwrap_or(0) as u32)),
                    };
                    p += if code == 9 { 12 } else { 8 };
                }
                if self.trace_bios {
                    for (n, (addr, size)) in &network {
                        eprintln!("[bios]   CAN network block {}: {:#x} bytes at {:08x}", n, size, addr);
                    }
                    for (n, (addr, size)) in &blocks {
                        eprintln!("[bios]   CAN state block {}: {:#x} bytes at {:08x}", n, size, addr);
                    }
                }
                if is_192 {
                    // The ROM fills every one of these blocks with the
                    // state of its own CAN software.  The stand-in knows
                    // a few fields of it and leaves the rest zero, and
                    // zero it has to be: neither the ROM nor, after a
                    // clean start, the OS clears RAM first.
                    for &(addr, size) in network.values().chain(blocks.values()) {
                        for i in 0..size.min(0x2000) {
                            self.poke8(addr + i, 0);
                        }
                    }
                }
                // An awake boot ROM has enabled CAN transmission and
                // entered the network's start state. These values mirror
                // the OS's cold initialization (NM start state 1, flag
                // 0x04, transmit enable 1), through its handover buffers.
                if let Some(&(addr, 2)) = blocks.get(&5) {
                    self.board.load(addr & 0x1fff_ffff, &1u16.to_be_bytes());
                }
                if let Some(&(addr, 2)) = network.get(&1) {
                    self.board.load(addr & 0x1fff_ffff, &[4, 0]);
                }
                if let Some(&(addr, 4)) = network.get(&5) {
                    self.poke32(addr, 1);
                }
                // The CAN timer handler clears its underflow flag,
                // but the OS never programs its reload or starts it.
                // On the 192 its ROM has it at 5 ms (73499 counts, like
                // its other two timers).  For the 193, whose ROM has
                // not been seen: 2.5 ms, the period the loaders give
                // this timer themselves.
                let eighths = if is_192 { 10 } else { 5 };
                let reload = (self.cycles_per_ms() / self.cpu.io.pclk_div * eighths / 8 - 1) as u32;
                let now = self.cpu.cycles;
                let pc = self.cpu.pc;
                let (timer, start, bit) = if self.cpu.io.navi.is_some() {
                    (0xffd8_1008, 0xffd8_1004, 1)
                } else {
                    (0xffd8_0020, 0xffd8_0004, 4)
                };
                self.cpu.io.write(&mut self.board, timer, 4, reload, now, pc);
                self.cpu.io.write(&mut self.board, timer + 4, 4, reload, now, pc);
                self.cpu.io.write(&mut self.board, timer + 8, 2, 0x20, now, pc);
                let started = self.cpu.io.read(&mut self.board, start, 1, now, pc);
                self.cpu.io.write(&mut self.board, start, 1, started | bit, now, pc);
                let lock = self.can_cyclic.iter().map(|(f, _, _)| f.clone()).find(|f| f.id == 0x271 || f.id == 0x2c3);
                if let (Some(&(table, tsize)), Some(&(state, 8))) = (blocks.get(&2), blocks.get(&8)) {
                    if tsize >= 2 * 40 {
                        // The last two entries, out of the way of those
                        // the OS adds itself.  The message bytes live in
                        // the spare end of the state block.
                        for (i, id) in [0x271u32, 0x2c3].into_iter().enumerate() {
                            let e = table + tsize - 40 * (2 - i as u32);
                            self.poke32(e, 0x0001_0000 | id);
                            self.poke32(e + 4, HANDOVER_HANDLER);
                            self.poke32(e + 8, state + 6 + i as u32);
                            self.poke32(e + 12, 0x0001_0000);
                        }
                        let (flags, data) = match lock {
                            Some(f) if f.id == 0x271 => (0x0100, (f.data[0] as u32) << 8),
                            Some(f) => (0x0001, f.data[0] as u32),
                            None => (0, 0),
                        };
                        self.poke32(state, 0x0271_02c3);
                        self.poke32(state + 4, flags << 16 | data);
                    }
                }
                0
            }
            // What the 192's ROM keeps about restarts after an
            // exception, in battery-backed memory from 0x10: where the
            // exception was (SPC, SSR), a word about it, and how many
            // there have been since the last reset, which is the result.
            // After a reset all of it is zero.  (The OS clears all RAM
            // on a 9; the stand-in used to answer that.)
            (0x3e, 6) if is_192 => {
                for p in [regs[4], regs[5], regs[6]] {
                    self.poke32(p, 0);
                }
                0
            }
            (0x3e, 6) => {
                for p in [regs[4], regs[5], regs[6]] {
                    self.poke32(p, 0);
                }
                9
            }
            // The DMA controller, for the 192's drive: master enable,
            // a channel's addresses and count, its control register.
            // The update loader has the same routines as plain
            // subroutines (at 0x8df928b8, 0x8df92838 and 0x8df92850 in
            // V220's), and the OS passes these services what the loader
            // passes those.
            (0x3d, 0x0201) => {
                let now = self.cpu.cycles;
                let dmaor = self.cpu.io.read(&mut self.board, 0xffa0_0040, 4, now, caller);
                self.cpu.io.write(&mut self.board, 0xffa0_0040, 4, (dmaor & 0xc000) | 1, now, caller);
                (dmaor & 0xc000) | 1
            }
            (0x3d, 0x0202) => {
                let now = self.cpu.cycles;
                let base = 0xffa0_0000 + (regs[4] & 7) * 16;
                self.cpu.io.write(&mut self.board, base + 4, 4, regs[5], now, caller); // destination
                self.cpu.io.write(&mut self.board, base, 4, regs[6], now, caller); // source
                self.cpu.io.write(&mut self.board, base + 8, 4, regs[7], now, caller); // count
                // What the ROM's routine happens to leave in r0.
                (regs[4] & 7) << 4
            }
            // Stop a channel: its enable bit is taken away (the ROM
            // first waits for the transfer's end; here it has ended),
            // or, after an address error, that flag in DMAOR, whose old
            // value is then the result.
            (0x3d, 0x0205) if is_192 => {
                let now = self.cpu.cycles;
                let chcr = 0xffa0_000c + (regs[4] & 7) * 16;
                let dmaor = self.cpu.io.read(&mut self.board, 0xffa0_0040, 4, now, caller);
                if dmaor & 4 != 0 {
                    self.cpu.io.write(&mut self.board, 0xffa0_0040, 4, dmaor & !4, now, caller);
                    dmaor
                } else {
                    let v = self.cpu.io.read(&mut self.board, chcr, 4, now, caller);
                    self.cpu.io.write(&mut self.board, chcr, 4, v & !1, now, caller);
                    0
                }
            }
            (0x3d, 0x0203) => {
                let now = self.cpu.cycles;
                let chcr = 0xffa0_000c + (regs[4] & 7) * 16;
                self.cpu.io.read(&mut self.board, chcr, 4, now, caller);
                self.cpu.io.write(&mut self.board, chcr, 4, regs[5], now, caller);
                self.cpu.io.read(&mut self.board, chcr, 4, now, caller)
            }
            (0x3d, 0x0204) => {
                let now = self.cpu.cycles;
                self.cpu.io.read(&mut self.board, 0xffa0_000c + (regs[4] & 7) * 16, 4, now, caller)
            }
            _ => 0,
        };
        if self.trace_bios && shown {
            eprintln!(
                "[bios] trap {:02x} fn {:04x} (r4={:08x} r5={:08x} r6={:08x} r7={:08x}) caller {:08x} -> {:08x}{}",
                n, code, regs[4], regs[5], regs[6], regs[7], caller, ret,
                if last { "  (further calls not shown)" } else { "" }
            );
        }
        self.cpu.bank0()[0] = ret;
    }

    /// Put an SD card into a slot (0 or 1; the 193 model has the one its
    /// software calls SD2 as slot 0).  False when there is no such slot.
    pub fn insert_card(&mut self, slot: usize, card: crate::dev::sdhi::Card) -> bool {
        match self.cpu.io.navi.as_mut() {
            Some(navi) if slot == 0 => navi.sdhi.cards[0] = Some(card),
            None if slot < 2 => self.board.media.insert(slot, card),
            _ => return false,
        }
        true
    }

    /// Take the card out of a slot.
    pub fn eject_card(&mut self, slot: usize) {
        match self.cpu.io.navi.as_mut() {
            Some(navi) if slot == 0 => navi.sdhi.cards[0] = None,
            None if slot < 2 => self.board.media.eject(slot),
            _ => {}
        }
    }

    /// Write a byte of RAM; false when the address is not RAM.
    pub fn poke8(&mut self, addr: u32, v: u8) -> bool {
        let o = (addr & 0x1fff_ffff).wrapping_sub(self.board.ram_base) as usize;
        if o < self.board.ram.len() {
            self.board.ram[o] = v;
            self.cpu.cache.host_write(addr & 0x1fff_ffff, v);
            true
        } else {
            false
        }
    }

    /// Write a big-endian word to RAM through a P0/P1/P2 address.
    pub fn poke32(&mut self, addr: u32, v: u32) {
        let o = (addr & 0x1fff_ffff).wrapping_sub(self.board.ram_base) as usize;
        if o + 4 <= self.board.ram.len() {
            self.board.ram[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
    }

    /// Run for `cycles` CPU cycles, forwarding serial output.
    pub fn run(&mut self, cycles: u64) -> Stop {
        let mut left = cycles;
        self.board.refresh_logging();
        loop {
            let slice = left.min(20_000);
            self.board.now = self.cpu.cycles;
            let stop = self.cpu.run(&mut self.board, slice);
            self.drain_console();
            if self.board.atapi.dma_request() {
                // The drive's DMARQ line is DREQ1 of the SH-4 DMAC.
                let now = self.cpu.cycles;
                self.cpu.io.dma_transfer(&mut self.board, 1, u32::MAX, now);
                self.board.update_irl();
            }
            if self.board.model == crate::board::Model::Rnse192 && self.cpu.io.dmaor & 0xc000 == 0xc000 {
                // DSP0 uses DMA channel 7, DSP1 channel 3. Unlike auto-request DMA,
                // a receive transfer can move only the words available
                // in the host FIFO. Both directions use 16-bit units.
                for ch in [3, 7] {
                    // A word at a time, as the processor's pins ask for
                    // them; what it does not get to in this time slice
                    // waits for the next.
                    for _ in 0..512 {
                        let dma = self.cpu.io.dma[ch];
                        if dma.chcr & 0xf73 != 0x21 { break; }
                        if !self.board.media.dma_request(dma.dar, true) && !self.board.media.dma_request(dma.sar, false) {
                            break;
                        }
                        if self.cpu.io.dma_transfer(&mut self.board, ch, 1, self.cpu.cycles) == 0 {
                            break;
                        }
                    }
                    self.board.update_irl();
                }
            }
            if self.board.model == crate::board::Model::Rnse192 {
                // The card processors, up to now.
                self.board.media.sync(self.cpu.cycles);
                self.board.media_edges();
            }
            if !self.can_peers.is_empty() {
                self.can_peers_tick();
            }
            if !self.can_cyclic.is_empty() || !self.can_waiting.is_empty() || self.can_sending.is_some() || self.can().tx_next().is_some() {
                self.can_tick();
            }
            if self.drive.is_some() {
                self.drive_tick();
            }
            // `bra .` with every interrupt masked: the firmware has given
            // up and is waiting for the watchdog.
            if self.cpu.imask() == 15 && self.peek16(self.cpu.pc) == Some(0xaffe) {
                self.cpu.fault = Some(format!("the firmware stopped itself at {:08x} with interrupts off (it expects the watchdog to reset the unit)", self.cpu.pc));
                return Stop::Fault;
            }
            self.radio_link();
            if let Some(navi) = self.cpu.io.navi.as_mut() {
                if navi.ge.pending {
                    navi.ge.run(&mut self.board.ram, self.board.ram_base);
                    navi.update_irqs(&mut self.cpu.io.intc);
                }
                navi.atapi_dma(&mut self.board.ram, self.board.ram_base, &mut self.cpu.io.intc);
            }
            self.panel_link();
            if self.atapi().trace_mark {
                self.atapi().trace_mark = false;
                eprintln!("[atapi]   at {} ms, {}", self.cpu.cycles / self.cycles_per_ms(), self.stack_trace(24));
            }
            // The 193's own sound output (what its software decodes),
            // a millisecond at a time: the driver queues blocks as
            // short as 12 ms and has to hear promptly that one is done.
            if self.cpu.cycles >= self.next_sound {
                self.next_sound += self.cycles_per_ms();
                // The drive's own time: spinning up, seeking, reading.
                let waiting = { let a = self.atapi(); a.advance(1000); a.irq() || a.dma_request() };
                if waiting {
                    self.board.update_irl();
                    if let Some(navi) = self.cpu.io.navi.as_mut() {
                        navi.update_irqs(&mut self.cpu.io.intc);
                    }
                }
                let listening = self.sound.is_some() || self.speaker.is_some();
                if let Some(navi) = self.cpu.io.navi.as_mut() {
                    let rate = navi.audio_rate() as usize;
                    let mut sent = Vec::new();
                    navi.audio_out(&self.board.ram, self.board.ram_base, 1000, listening.then_some(&mut sent), &mut self.cpu.io.intc);
                    // To the 44.1 kHz of the recording, by the nearest
                    // sample where the rate is another.
                    let (have, want) = (sent.len() / 2, sent.len() / 2 * 44_100 / rate.max(1));
                    for i in 0..want {
                        let at = if rate == 44_100 { i } else { (i * have / want).min(have - 1) };
                        self.decoded.extend([sent[2 * at], sent[2 * at + 1]]);
                    }
                }
            }
            if self.cpu.cycles >= self.next_frame {
                self.next_frame += self.frame_cycles;
                // The drive's audio player keeps time with the machine;
                // what it plays is kept when somebody wants to hear it.
                let frame_micros = self.frame_cycles * 1000 / self.cycles_per_ms();
                let listening = self.sound.is_some() || self.speaker.is_some();
                if listening {
                    let mut played = Vec::new();
                    self.atapi().play_out(frame_micros, Some(&mut played));
                    // The 193's own sound output, collected below.
                    let mut decoded = Vec::new();
                    for _ in 0..played.len() {
                        decoded.push(self.decoded.pop_front().unwrap_or(0));
                    }
                    if self.board.model == crate::board::Model::Rnse192 {
                        // What the 192's card processors have sent to
                        // their converters, the two added.  Each keeps
                        // its own time, so a sample may not be there
                        // yet: the last one stands in for it.
                        for (dsp, last) in self.board.media.dsps.iter_mut().zip(self.media_last.iter_mut()) {
                            let rate = dsp.sample_rate() as usize;
                            let have = dsp.audio.len() / 2;
                            let take = (played.len() / 2 * rate / 44_100).min(have);
                            let sent: Vec<i16> = dsp.audio.drain(..2 * take).collect();
                            for (i, out) in decoded.chunks_mut(2).enumerate() {
                                let at = i * rate / 44_100;
                                if at < take {
                                    *last = [sent[2 * at], sent[2 * at + 1]];
                                }
                                out[0] = out[0].saturating_add(last[0]);
                                out[1] = out[1].saturating_add(last[1]);
                            }
                        }
                    }
                    if let Some(speaker) = self.speaker.as_mut() {
                        let gain = self.radio.cd_gain();
                        let own = self.radio.gain(crate::dev::radio::Radio::SOURCE_DECODER);
                        speaker.extend(played.iter().zip(&decoded)
                            .map(|(cd, own_sample)| (*cd as f32 * gain + *own_sample as f32 * own) as i16));
                    }
                    if let Some(sound) = self.sound.as_mut() {
                        sound.append(&mut played);
                    }
                } else {
                    self.atapi().play_out(frame_micros, None);
                }
                self.board.vsync();
                if let Some(navi) = self.cpu.io.navi.as_mut() {
                    navi.vsync(&mut self.cpu.io.intc);
                }
                // The external watchdog wants an edge on its strobe
                // (192: port 0 bit 0; 193: 0xffc44008 bit 0) regularly.
                // It is taken to be armed by the first two edges, or
                // from the start when `watchdog_always` is set; the real
                // period is not known, 1.5 s is assumed.  (The 193's
                // update loader ends by letting it expire on purpose.)
                let edges = match self.cpu.io.navi.as_ref() {
                    Some(navi) => navi.watchdog_edges,
                    None => self.board.watchdog_edges,
                };
                let now = self.cpu.cycles;
                if edges != self.watchdog.edges {
                    if self.watchdog.edges != 0 || self.watchdog_always {
                        self.watchdog.longest = self.watchdog.longest.max(now - self.watchdog.last);
                    }
                    self.watchdog.edges = edges;
                    self.watchdog.last = now;
                } else if (edges >= 2 || self.watchdog_always) && now - self.watchdog.last > 1500 * self.cycles_per_ms() {
                    self.watchdog.last = now;
                    eprintln!("[boot] the watchdog was not served: reset");
                    self.reset_requested = true;
                    self.cpu.fault = Some("reset requested".into());
                    return Stop::Fault;
                }
                self.board.step_display((self.frame_cycles / self.cycles_per_ms()) as u32);
            }
            if stop == Stop::Hypercall {
                self.cpu.cache.sync(&mut self.board);
                if !self.hypercall() {
                    return Stop::Fault;
                }
            } else if stop == Stop::Breakpoint && self.gps.as_ref().is_some_and(|g| g.1 == self.cpu.pc) {
                // The application asks for the GPS library's latest result.
                // When the OS has one for it (bit 0 of the interface
                // task's flags; without satellites it says "no fix" once a
                // second), the fix is written over it first.
                let now = self.cpu.cycles / self.cycles_per_ms();
                let (fix, _, pointer) = self.gps.clone().unwrap();
                let context = self.peek32(pointer).unwrap_or(0);
                if context != 0 && self.peek32(context).unwrap_or(0) & 1 != 0 {
                    let o = ((context + 0x8ec) & 0x1fff_ffff).wrapping_sub(self.board.ram_base) as usize;
                    if o + 47 <= self.board.ram.len() {
                        fix.patch(&mut self.board.ram[o..o + 47], now);
                        self.gps_fixes += 1;
                        // The date and time are also kept apart, for
                        // another service: a flag and six bytes.
                        let time: [u8; 6] = self.board.ram[o + 22..o + 28].try_into().unwrap();
                        let t = ((pointer + 4) & 0x1fff_ffff).wrapping_sub(self.board.ram_base) as usize;
                        self.board.ram[t..t + 2].copy_from_slice(&1u16.to_be_bytes());
                        self.board.ram[t + 2..t + 8].copy_from_slice(&time);
                    }
                }
                let saved = std::mem::take(&mut self.cpu.breakpoints);
                self.cpu.run(&mut self.board, 1);
                self.cpu.breakpoints = saved;
            } else if stop == Stop::Breakpoint && (Some(self.cpu.pc) == self.printf_hook || self.printf_hooks.contains(&self.cpu.pc)) {
                if let Some(text) = self.format_printf(self.cpu.r[15]) {
                    let line = format!("[fw] {}\n", text.trim_end_matches(['\n', '\r']));
                    let bytes: Vec<u8> = line.chars().map(|c| c as u8).collect();
                    if self.echo_console {
                        use std::io::Write;
                        let _ = std::io::stdout().write_all(&bytes);
                    }
                    self.console.extend_from_slice(&bytes);
                }
                // Step over the hooked instruction.
                let saved = std::mem::take(&mut self.cpu.breakpoints);
                self.cpu.run(&mut self.board, 1);
                self.cpu.breakpoints = saved;
            } else if stop != Stop::Budget {
                return stop;
            }
            left -= slice;
            if left == 0 {
                return Stop::Budget;
            }
        }
    }

    /// A rough call chain: the return address and every word on the
    /// stack that points into flash, nearest first.
    pub fn stack_trace(&self, max: usize) -> String {
        let mut out = format!("pc {:08x} pr {:08x} stack", self.cpu.pc, self.cpu.pr);
        let flash = self.board.flash.len() as u32;
        let mut n = 0;
        for i in 0..1024u32 {
            let Some(w) = self.peek32(self.cpu.r[15].wrapping_add(i * 4)) else { break };
            if w & 1 == 0 && (w >> 29 == 4 || w >> 29 == 0) && (w & 0x1fff_ffff) < flash && (w & 0x1fff_ffff) >= 0xc000 {
                out.push_str(&format!(" {:08x}", w));
                n += 1;
                if n == max {
                    break;
                }
            }
        }
        out
    }

    /// Read memory through a P1/P2/P0 address without side effects on
    /// devices (RAM and flash only).
    pub fn peek8(&self, addr: u32) -> Option<u8> {
        let pa = addr & 0x1fff_ffff;
        // As the CPU would read it: what its data cache holds comes
        // first (only when the caches are modelled).
        if crate::sh4::cache::cacheable(addr) {
            if let Some(b) = self.cpu.cache.peek(pa) {
                return Some(b);
            }
        }
        let o = pa.wrapping_sub(self.board.ram_base) as usize;
        if o < self.board.ram.len() {
            Some(self.board.ram[o])
        } else if (pa as usize) < self.board.flash.len() {
            Some(self.board.flash[pa as usize])
        } else {
            // Battery-backed memory.
            let n = pa.wrapping_sub(self.board.model.nvram_base()) as usize;
            self.board.nvram.get(n).copied()
        }
    }

    pub fn peek16(&self, addr: u32) -> Option<u16> {
        Some(u16::from_be_bytes([self.peek8(addr)?, self.peek8(addr.wrapping_add(1))?]))
    }

    pub fn peek32(&self, addr: u32) -> Option<u32> {
        Some(u32::from_be_bytes([self.peek8(addr)?, self.peek8(addr + 1)?, self.peek8(addr + 2)?, self.peek8(addr + 3)?]))
    }

    /// A printable C string at `addr`, if there is one.
    pub fn peek_str(&self, addr: u32, max: usize) -> Option<String> {
        let mut s = String::new();
        for i in 0..max as u32 {
            let b = self.peek8(addr.wrapping_add(i))?;
            if b == 0 {
                break;
            }
            if !(0x20..0x7f).contains(&b) {
                return if s.len() >= 3 { Some(s) } else { None };
            }
            s.push(b as char);
        }
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// Format a C printf call made by the firmware: format string and
    /// arguments are 32-bit words on the stack at `sp` (the Hitachi C
    /// compiler passes all variadic arguments there).
    pub fn format_printf(&self, sp: u32) -> Option<String> {
        let fmt_ptr = self.peek32(sp)?;
        let mut out = String::new();
        let mut arg = sp.wrapping_add(4);
        let mut p = fmt_ptr;
        fn next(m: &Machine, arg: &mut u32) -> u32 {
            let v = m.peek32(*arg).unwrap_or(0);
            *arg = arg.wrapping_add(4);
            v
        }
        for _ in 0..512 {
            let c = self.peek8(p)?;
            p = p.wrapping_add(1);
            if c == 0 {
                break;
            }
            if c != b'%' {
                // Keep bytes above 0x7f (EUC-JP text) as they are.
                out.push(c as char);
                continue;
            }
            let mut spec = String::from("%");
            let mut conv = 0u8;
            for _ in 0..12 {
                let c = self.peek8(p)?;
                p = p.wrapping_add(1);
                if c == 0 {
                    break;
                }
                if c.is_ascii_alphabetic() && c != b'l' && c != b'h' {
                    conv = c;
                    break;
                }
                if c == b'%' {
                    conv = c;
                    break;
                }
                spec.push(c as char);
            }
            let flags: String = spec[1..].chars().filter(|c| !matches!(c, 'l' | 'h')).collect();
            let zero = flags.starts_with('0');
            let left = flags.starts_with('-');
            let width: usize = flags.trim_start_matches(['0', '-', '+', ' ', '#']).split('.').next().unwrap_or("").parse().unwrap_or(0);
            let pad = |s: String| -> String {
                if s.len() >= width {
                    s
                } else if left {
                    format!("{:<w$}", s, w = width)
                } else if zero {
                    format!("{:0>w$}", s, w = width)
                } else {
                    format!("{:>w$}", s, w = width)
                }
            };
            match conv {
                b'%' => out.push('%'),
                b'd' | b'i' => out += &pad((next(self, &mut arg) as i32).to_string()),
                b'u' => out += &pad(next(self, &mut arg).to_string()),
                b'x' | b'p' => out += &pad(format!("{:x}", next(self, &mut arg))),
                b'X' => out += &pad(format!("{:X}", next(self, &mut arg))),
                b'c' => out.push((next(self, &mut arg) as u8) as char),
                b's' => {
                    let sp = next(self, &mut arg);
                    let mut t = String::new();
                    for i in 0..256 {
                        match self.peek8(sp.wrapping_add(i)) {
                            Some(0) | None => break,
                            Some(b) => t.push(b as char),
                        }
                    }
                    out += &pad(t);
                }
                b'f' | b'g' | b'e' => {
                    // Doubles take two words, aligned to 8 bytes.
                    if arg & 4 != 0 {
                        arg = arg.wrapping_add(4);
                    }
                    let hi = next(self, &mut arg) as u64;
                    let lo = next(self, &mut arg) as u64;
                    out += &format!("{}", f64::from_bits(hi << 32 | lo));
                }
                _ => {
                    out += &spec;
                    if conv != 0 {
                        out.push(conv as char);
                    }
                }
            }
        }
        Some(out)
    }

    fn drain_console(&mut self) {
        // The firmware's debug text goes out on serial channel 5 of the
        // companion chip; the SH-4's own SCIF is forwarded as well.
        let mut out: Vec<u8> = std::mem::take(&mut self.cpu.io.scif_tx);
        out.append(&mut self.board.uart[5].tx);
        if let Some(navi) = self.cpu.io.navi.as_mut() {
            out.append(&mut navi.console);
        }
        if out.is_empty() {
            return;
        }
        if self.echo_console {
            use std::io::Write;
            let _ = std::io::stdout().write_all(&out);
            let _ = std::io::stdout().flush();
        }
        self.console.extend_from_slice(&out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program for the 192 that puts `edges` edges on the watchdog
    /// strobe and then does nothing.
    fn strobe_then_hang(edges: usize) -> Machine {
        let mut code: Vec<u16> = vec![0xd100 | ((edges as u16 * 4 + 6) / 4 + 1)]; // mov.l port,r1
        for i in 0..edges {
            code.push(0xe000 | (i as u16 + 1) & 1); // mov #0 or #1,r0
            code.push(0x2100); // mov.b r0,@r1
        }
        code.extend([0xaffe, 0x0009]); // bra . ; nop
        while code.len() % 2 != 0 || code.len() * 2 < ((edges * 4 + 6) / 4 + 2) * 4 {
            code.push(0x0009);
        }
        let mut bytes: Vec<u8> = code.iter().flat_map(|w| w.to_be_bytes()).collect();
        bytes.extend(0xb230_0001u32.to_be_bytes());
        let mut m = Machine::new();
        m.echo_console = false;
        m.board.load(0x0c00_1000, &bytes);
        m.cpu.reset();
        m.cpu.set_sr(SR_MD);
        m.cpu.pc = 0x8c00_1000;
        m
    }

    #[test]
    fn watchdog_192() {
        // Never strobed: the watchdog is taken not to be armed (a guess
        // that keeps software alive which does not know about it; real
        // hardware may well differ).
        let mut m = strobe_then_hang(0);
        let ms = m.cycles_per_ms();
        assert_eq!(m.run(3_000 * ms), Stop::Budget);
        // Strobed and then abandoned: reset after 1.5 s.
        let mut m = strobe_then_hang(3);
        assert_eq!(m.board.watchdog_edges, 0);
        assert_eq!(m.run(1_400 * ms), Stop::Budget);
        assert_eq!(m.board.watchdog_edges, 3);
        assert_eq!(m.run(300 * ms), Stop::Fault);
        assert!(m.reset_requested);
    }
}
