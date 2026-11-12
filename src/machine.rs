//! Ties the CPU and the board together and knows how to start firmware.

use crate::board::Board;
use crate::bootrom::{self, Hook};
use crate::dev::hcan::Frame;
use crate::fw::{IndexInfo, Module, Variant};
use crate::sh4::cpu::Stop;
use std::collections::{BTreeMap, VecDeque};
use crate::sh4::{Cpu, SR_BL, SR_MD};

pub struct Machine {
    pub cpu: Cpu,
    pub board: Board,
    /// Text the firmware printed on its debug serial port.
    pub console: Vec<u8>,
    pub echo_console: bool,
    /// CPU cycles per display frame.
    pub frame_cycles: u64,
    next_frame: u64,
    /// Module entry points still to be called by the boot stand-in.
    boot_calls: VecDeque<(String, u32)>,
    /// Set when the firmware asked for a restart.
    pub reset_requested: bool,
    pub trace_bios: bool,
    /// Number of calls seen per boot ROM service (trap, function code).
    pub bios_calls: BTreeMap<(u8, u32), u64>,
    /// Memory break of the two pools managed by boot ROM services 4/5.
    pub bios_break: [u32; 2],
    /// Address of the firmware's `printf`, if known.  The main software
    /// formats debug messages but does not send them to a serial port;
    /// with this set the emulator prints them itself.
    pub printf_hook: Option<u32>,
    /// Frames sent to the unit periodically, standing in for the rest
    /// of the car: (frame, period in cycles, next due).
    can_cyclic: Vec<(Frame, u64, u64)>,
    pub ignition: bool,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    pub fn new() -> Self {
        Machine { cpu: Cpu::new(), board: Board::new(), console: Vec::new(), echo_console: true, frame_cycles: 2_200_000, next_frame: 2_200_000, boot_calls: VecDeque::new(), reset_requested: false, trace_bios: false, bios_calls: BTreeMap::new(), bios_break: [0; 2], printf_hook: None, can_cyclic: Vec::new(), ignition: false }
    }

    /// Place every module of a variant at its load address, the way the
    /// update loader leaves the flash.
    pub fn install(&mut self, v: &Variant, index: &IndexInfo) {
        for m in &v.modules {
            if m.kind != "LDR" {
                self.board.load(m.phys_addr(), &m.data[..m.size as usize]);
            }
        }
        if let Some(os) = v.module("OSM") {
            // The module directory sits in the 256 bytes below the OS.
            let dir = crate::fw::module_directory(v, index);
            self.board.load(os.phys_addr() - 0x100, &dir);
        }
    }

    /// Write the boot ROM stand-in (see `bootrom.rs`).
    pub fn install_boot_stub(&mut self, project: &str) {
        bootrom::install(&mut self.board, project);
        self.cpu.hypercalls_enabled = true;
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
        self.cpu.r[15] = 0x8dff_ff00;
        self.cpu.pr = 0;
    }

    /// Address of `printf` in OS builds that have been looked at, keyed
    /// by the OS module's version string.
    pub fn known_printf(os_version: &str) -> Option<u32> {
        match os_version {
            "V2200815" => Some(0x801e_b344),
            _ => None,
        }
    }

    /// Print the firmware's own debug messages (see `printf_hook`).
    pub fn hook_printf(&mut self, addr: u32) {
        self.printf_hook = Some(addr);
        if !self.cpu.breakpoints.contains(&addr) {
            self.cpu.breakpoints.push(addr);
        }
    }

    /// CPU cycles per millisecond of emulated time.
    pub fn cycles_per_ms(&self) -> u64 {
        self.cpu.io.pclk_div * 29_400
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

    /// A faceplate key was pressed or released.  Returns whether the
    /// event reached the firmware; the front panel's interface has not
    /// been reverse engineered yet, so for now it never does.
    pub fn panel_key(&mut self, _name: &str, _down: bool) -> bool {
        false
    }

    /// A faceplate knob was turned (see [`Machine::panel_key`]).
    pub fn panel_knob(&mut self, _name: &str, _delta: i32) -> bool {
        false
    }

    fn can_tick(&mut self) {
        let now = self.cpu.cycles;
        let mut any = false;
        for (frame, period, due) in self.can_cyclic.iter_mut() {
            if now >= *due {
                *due += *period;
                if *due <= now {
                    *due = now + *period;
                }
                self.board.can.receive(frame);
                any = true;
            }
        }
        if any {
            self.board.update_irl();
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
        // The OS starts TMU0 for its 2 ms tick but never gives the timer an
        // interrupt priority, so it relies on the boot ROM having done
        // that.  The loader, which shares the boot ROM's hardware layer,
        // uses level 7 (IPRA = 0x7000).
        self.cpu.io.intc.ipra = 0x7000;
        // Likewise the CAN controller: the OS sets up mailboxes and
        // handles interrupts but never takes the chip out of reset.
        self.board.can.start();
        self.boot_calls.clear();
        for (i, name) in ["init1", "init2", "start"].iter().enumerate() {
            self.boot_calls.push_back((format!("OSM {}", name), os.entries[i]));
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
        let count = self.bios_calls.entry((n, code)).or_insert(0);
        *count += 1;
        let shown = *count <= 8;
        let last = *count == 8;
        let ret = match (n, code) {
            (0x3e, 4) => self.bios_break[(regs[4] & 1) as usize],
            (0x3e, 5) => {
                self.bios_break[(regs[4] & 1) as usize] = regs[5];
                0
            }
            // Version string of a boot component; the application checks
            // the first three characters for other hardware projects.
            (0x3e, 1) => 0x8000_0406,
            (0x3e, 6) => {
                for p in [regs[4], regs[5], regs[6]] {
                    self.poke32(p, 0);
                }
                9
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

    /// Write a big-endian word to RAM through a P0/P1/P2 address.
    pub fn poke32(&mut self, addr: u32, v: u32) {
        let o = (addr & 0x1fff_ffff).wrapping_sub(crate::board::RAM_BASE) as usize;
        if o + 4 <= self.board.ram.len() {
            self.board.ram[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
    }

    /// Run for `cycles` CPU cycles, forwarding serial output.
    pub fn run(&mut self, cycles: u64) -> Stop {
        let mut left = cycles;
        loop {
            let slice = left.min(20_000);
            let stop = self.cpu.run(&mut self.board, slice);
            self.drain_console();
            if self.board.atapi.dma_request() {
                // The drive's DMARQ line is DREQ1 of the SH-4 DMAC.
                let now = self.cpu.cycles;
                self.cpu.io.dma_transfer(&mut self.board, 1, u32::MAX, now);
                self.board.update_irl();
            }
            if !self.can_cyclic.is_empty() {
                self.can_tick();
            }
            if self.cpu.cycles >= self.next_frame {
                self.next_frame += self.frame_cycles;
                self.board.vsync();
            }
            if stop == Stop::Hypercall {
                if !self.hypercall() {
                    return Stop::Fault;
                }
            } else if stop == Stop::Breakpoint && Some(self.cpu.pc) == self.printf_hook {
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

    /// Read memory through a P1/P2/P0 address without side effects on
    /// devices (RAM and flash only).
    pub fn peek8(&self, addr: u32) -> Option<u8> {
        let pa = addr & 0x1fff_ffff;
        let o = pa.wrapping_sub(crate::board::RAM_BASE) as usize;
        if o < self.board.ram.len() {
            Some(self.board.ram[o])
        } else if (pa as usize) < self.board.flash.len() {
            Some(self.board.flash[pa as usize])
        } else {
            None
        }
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
        out.append(&mut self.board.uart_tx[5]);
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
