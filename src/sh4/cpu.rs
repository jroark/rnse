//! CPU state, memory access, exception and interrupt entry.

use super::mmu::Mmu;
use super::onchip::OnChip;
use super::*;

/// Exception kinds that an instruction can raise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcKind {
    TlbMissRead,
    TlbMissWrite,
    InitialPageWrite,
    TlbProtRead,
    TlbProtWrite,
    AddrErrRead,
    AddrErrWrite,
    Trapa(u8),
    IllegalInsn,
    SlotIllegalInsn,
    FpuDisable,
    SlotFpuDisable,
    FpuException,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exc {
    pub kind: ExcKind,
    /// Faulting virtual address for memory exceptions.
    pub addr: u32,
}

impl Exc {
    pub fn new(kind: ExcKind) -> Self {
        Exc { kind, addr: 0 }
    }
    pub fn mem(kind: ExcKind, addr: u32) -> Self {
        Exc { kind, addr }
    }
    pub fn code(&self) -> u32 {
        match self.kind {
            ExcKind::TlbMissRead => 0x040,
            ExcKind::TlbMissWrite => 0x060,
            ExcKind::InitialPageWrite => 0x080,
            ExcKind::TlbProtRead => 0x0a0,
            ExcKind::TlbProtWrite => 0x0c0,
            ExcKind::AddrErrRead => 0x0e0,
            ExcKind::AddrErrWrite => 0x100,
            ExcKind::FpuException => 0x120,
            ExcKind::Trapa(_) => 0x160,
            ExcKind::IllegalInsn => 0x180,
            ExcKind::SlotIllegalInsn => 0x1a0,
            ExcKind::FpuDisable => 0x800,
            ExcKind::SlotFpuDisable => 0x820,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Exec,
}

/// Why `run` returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Ran the requested number of cycles.
    Budget,
    /// A breakpoint address was reached (PC is at the breakpoint).
    Breakpoint,
    /// The CPU took an exception while SR.BL was set, or hit another
    /// unrecoverable condition.  Real hardware would reset.
    Fault,
    /// The reserved opcode 0xFFFD was executed while hypercalls are
    /// enabled; `Cpu::hypercall` holds its address.  Used by high-level
    /// emulation of code that is not available (the boot ROM).
    Hypercall,
}

pub struct Cpu {
    pub r: [u32; 16],
    /// R0-R7 of the register bank that is currently *not* selected.
    pub r_bank: [u32; 8],
    pub pc: u32,
    pub pr: u32,
    pub sr: u32,
    pub ssr: u32,
    pub spc: u32,
    pub gbr: u32,
    pub vbr: u32,
    pub sgr: u32,
    pub dbr: u32,
    pub mach: u32,
    pub macl: u32,
    pub fpscr: u32,
    pub fpul: u32,
    /// FR0-15 of the selected bank in 0..16, the other bank in 16..32.
    pub fr: [u32; 32],

    pub mmu: Mmu,
    pub io: OnChip,

    /// Instructions executed (also the emulated time base).
    pub cycles: u64,
    pub sleeping: bool,
    /// Something the run loop has to look at happened: sleep, a fault
    /// or a hypercall.  Lets it test one flag per instruction.
    pub(super) attention: bool,
    pub(super) in_delay: bool,
    /// Set while executing the delay slot of RTE (accesses are privileged).
    pub(super) rte_slot: bool,
    /// The page instructions were last fetched from: its virtual
    /// address and the physical address it translates to.  Saves the
    /// translation for every fetch but the first in a page; 1 (no page
    /// address) when nothing is cached.  Dropped whenever the
    /// translation could change: SR, the MMU registers, an RTE's slot.
    pub(super) fetch_page: u32,
    fetch_phys: u32,
    pub(super) insn_pc: u32,

    pub fault: Option<String>,
    pub breakpoints: Vec<u32>,
    /// Ring buffer of recently executed PCs (for post-mortem traces).
    pub pc_hist: Vec<u32>,
    pub pc_hist_pos: usize,
    pub trace_exceptions: bool,
    /// Cycle at which the current `run` call ends (fast paths must not
    /// run past it).
    pub(super) run_end: u64,
    /// Interrupts accepted per INTEVT code / 0x20, for statistics.
    pub irq_counts: [u64; 128],
    /// When set, every subroutine call is recorded here: target ->
    /// (times called, cycle of the first call, first caller).  A reverse
    /// engineering aid; see `--trace-calls`.
    pub calls: Option<std::collections::BTreeMap<u32, (u64, u64, u32)>>,
    pub hypercalls_enabled: bool,
    /// Address of the hypercall instruction that stopped execution.
    pub hypercall: Option<u32>,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    #[inline(always)]
    pub(super) fn note_call(&mut self, target: u32) {
        if let Some(calls) = &mut self.calls {
            let e = calls.entry(target).or_insert((0, self.cycles, self.pc));
            e.0 += 1;
        }
    }

    pub fn new() -> Self {
        let mut c = Cpu {
            r: [0; 16],
            r_bank: [0; 8],
            pc: 0xa000_0000,
            pr: 0,
            sr: 0,
            ssr: 0,
            spc: 0,
            gbr: 0,
            vbr: 0,
            sgr: 0,
            dbr: 0,
            mach: 0,
            macl: 0,
            fpscr: 0,
            fpul: 0,
            fr: [0; 32],
            mmu: Mmu::new(),
            io: OnChip::new(),
            cycles: 0,
            sleeping: false,
            attention: true,
            in_delay: false,
            rte_slot: false,
            fetch_page: 1,
            fetch_phys: 0,
            insn_pc: 0,
            fault: None,
            breakpoints: Vec::new(),
            pc_hist: vec![0; 256],
            calls: None,
            pc_hist_pos: 0,
            trace_exceptions: false,
            run_end: u64::MAX,
            irq_counts: [0; 128],
            hypercalls_enabled: false,
            hypercall: None,
        };
        c.reset();
        c
    }

    /// Power-on reset state.
    pub fn reset(&mut self) {
        self.sr = SR_MD | SR_RB | SR_BL | (0xf << SR_IMASK_SHIFT);
        self.fetch_page = 1;
        self.vbr = 0;
        self.pc = 0xa000_0000;
        self.fpscr = 0x0004_0001;
        self.sleeping = false;
        self.in_delay = false;
        self.rte_slot = false;
        self.io.expevt = 0;
    }

    // ---- status register / banking -------------------------------------

    #[inline(always)]
    pub fn t(&self) -> bool {
        self.sr & SR_T != 0
    }
    #[inline(always)]
    pub fn set_t(&mut self, v: bool) {
        self.sr = (self.sr & !SR_T) | v as u32;
    }
    #[inline(always)]
    pub fn privileged(&self) -> bool {
        self.sr & SR_MD != 0
    }
    #[inline(always)]
    pub fn imask(&self) -> u32 {
        (self.sr >> SR_IMASK_SHIFT) & 0xf
    }

    #[inline]
    fn bank1(sr: u32) -> bool {
        sr & (SR_MD | SR_RB) == (SR_MD | SR_RB)
    }

    pub fn set_sr(&mut self, val: u32) {
        let val = val & SR_MASK;
        if Self::bank1(val) != Self::bank1(self.sr) {
            for i in 0..8 {
                std::mem::swap(&mut self.r[i], &mut self.r_bank[i]);
            }
        }
        if (val ^ self.sr) & SR_MD != 0 {
            self.mmu.flush_soft();
        }
        self.fetch_page = 1;
        self.sr = val;
    }

    pub fn set_fpscr(&mut self, val: u32) {
        let val = val & FPSCR_MASK;
        if (val ^ self.fpscr) & FPSCR_FR != 0 {
            let (a, b) = self.fr.split_at_mut(16);
            a.swap_with_slice(b);
        }
        self.fpscr = val;
    }

    /// R0-R7 of bank 0 (what non-exception code sees), whichever bank
    /// is selected right now.
    pub fn bank0(&mut self) -> &mut [u32] {
        if Self::bank1(self.sr) {
            &mut self.r_bank[..]
        } else {
            &mut self.r[..8]
        }
    }

    /// Value of Rn_BANK (the non-selected bank) register.
    #[inline]
    pub(super) fn bank_reg(&mut self, n: usize) -> &mut u32 {
        &mut self.r_bank[n & 7]
    }

    // ---- memory ---------------------------------------------------------

    #[inline(always)]
    fn translate(&mut self, addr: u32, acc: Access) -> Result<u32, Exc> {
        if addr & 0x8000_0000 == 0 {
            if !self.mmu.enabled() {
                return Ok(addr & 0x1fff_ffff);
            }
            return self
                .mmu
                .lookup(addr, acc, self.sr & SR_MD != 0 || self.rte_slot);
        }
        if self.sr & SR_MD == 0 && !self.rte_slot {
            let kind = if acc == Access::Write {
                ExcKind::AddrErrWrite
            } else {
                ExcKind::AddrErrRead
            };
            return Err(Exc::mem(kind, addr));
        }
        if addr < 0xc000_0000 || !self.mmu.enabled() {
            return Ok(addr & 0x1fff_ffff);
        }
        // P3 with the MMU on.
        self.mmu.lookup(addr, acc, true)
    }

    #[inline(always)]
    pub fn read8<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Result<u8, Exc> {
        if addr >= 0xe000_0000 {
            return self.p4_read(bus, addr, 1).map(|v| v as u8);
        }
        let pa = self.translate(addr, Access::Read)?;
        Ok(bus.read8(pa))
    }

    #[inline(always)]
    pub fn read16<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Result<u16, Exc> {
        if addr & 1 != 0 {
            return Err(Exc::mem(ExcKind::AddrErrRead, addr));
        }
        if addr >= 0xe000_0000 {
            return self.p4_read(bus, addr, 2).map(|v| v as u16);
        }
        let pa = self.translate(addr, Access::Read)?;
        Ok(bus.read16(pa))
    }

    #[inline(always)]
    pub fn read32<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Result<u32, Exc> {
        if addr & 3 != 0 {
            return Err(Exc::mem(ExcKind::AddrErrRead, addr));
        }
        if addr >= 0xe000_0000 {
            return self.p4_read(bus, addr, 4);
        }
        let pa = self.translate(addr, Access::Read)?;
        Ok(bus.read32(pa))
    }

    #[inline(always)]
    pub fn write8<B: Bus>(&mut self, bus: &mut B, addr: u32, v: u8) -> Result<(), Exc> {
        if addr >= 0xe000_0000 {
            return self.p4_write(bus, addr, 1, v as u32);
        }
        let pa = self.translate(addr, Access::Write)?;
        bus.write8(pa, v);
        Ok(())
    }

    #[inline(always)]
    pub fn write16<B: Bus>(&mut self, bus: &mut B, addr: u32, v: u16) -> Result<(), Exc> {
        if addr & 1 != 0 {
            return Err(Exc::mem(ExcKind::AddrErrWrite, addr));
        }
        if addr >= 0xe000_0000 {
            return self.p4_write(bus, addr, 2, v as u32);
        }
        let pa = self.translate(addr, Access::Write)?;
        bus.write16(pa, v);
        Ok(())
    }

    #[inline(always)]
    pub fn write32<B: Bus>(&mut self, bus: &mut B, addr: u32, v: u32) -> Result<(), Exc> {
        if addr & 3 != 0 {
            return Err(Exc::mem(ExcKind::AddrErrWrite, addr));
        }
        if addr >= 0xe000_0000 {
            return self.p4_write(bus, addr, 4, v);
        }
        let pa = self.translate(addr, Access::Write)?;
        bus.write32(pa, v);
        Ok(())
    }

    #[inline(always)]
    pub(super) fn fetch<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Result<u16, Exc> {
        // Same page as the last fetch (an odd address never matches).
        if addr & 0xffff_f001 == self.fetch_page {
            return Ok(bus.read16(self.fetch_phys | (addr & 0xffe)));
        }
        if addr & 1 != 0 || addr >= 0xe000_0000 {
            return Err(Exc::mem(ExcKind::AddrErrRead, addr));
        }
        let pa = self.translate(addr, Access::Exec)?;
        self.fetch_page = addr & 0xffff_f000;
        self.fetch_phys = pa & 0xffff_f000;
        Ok(bus.read16(pa))
    }

    fn p4_read<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u32) -> Result<u32, Exc> {
        if self.sr & SR_MD == 0 && !self.rte_slot {
            return Err(Exc::mem(ExcKind::AddrErrRead, addr));
        }
        if addr < 0xe400_0000 {
            // Store queues read back as undefined on real hardware.
            return Ok(0);
        }
        if (0xf000_0000..0xf800_0000).contains(&addr) {
            return Ok(self.mmu.array_read(addr));
        }
        if let Some(v) = self.mmu.reg_read(addr) {
            return Ok(v);
        }
        Ok(self.io.read(bus, addr, size, self.cycles, self.insn_pc))
    }

    fn p4_write<B: Bus>(&mut self, bus: &mut B, addr: u32, size: u32, v: u32) -> Result<(), Exc> {
        self.fetch_page = 1;
        if addr < 0xe400_0000 {
            if self.sr & SR_MD == 0 && self.mmu.sqmd() {
                return Err(Exc::mem(ExcKind::AddrErrWrite, addr));
            }
            let q = ((addr >> 5) & 1) as usize;
            let i = ((addr >> 2) & 7) as usize;
            match size {
                4 => self.io.sq[q][i] = v,
                _ => {
                    // Sub-word store queue writes are not defined; keep
                    // them harmless.
                    let sh = (3 - (addr & 3)) * 8;
                    let mask = if size == 1 { 0xff } else { 0xffff };
                    let sh = if size == 2 { (2 - (addr & 2)) * 8 } else { sh };
                    self.io.sq[q][i] = (self.io.sq[q][i] & !(mask << sh)) | ((v & mask) << sh);
                }
            }
            return Ok(());
        }
        if self.sr & SR_MD == 0 && !self.rte_slot {
            return Err(Exc::mem(ExcKind::AddrErrWrite, addr));
        }
        if (0xf000_0000..0xf800_0000).contains(&addr) {
            self.mmu.array_write(addr, v);
            return Ok(());
        }
        if self.mmu.reg_write(addr, v) {
            return Ok(());
        }
        self.io.write(bus, addr, size, v, self.cycles, self.insn_pc);
        Ok(())
    }

    /// PREF on a store-queue address: flush 32 bytes to memory.
    pub(super) fn sq_flush<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Result<(), Exc> {
        let q = ((addr >> 5) & 1) as usize;
        let base = if self.mmu.enabled() {
            let pa = self.mmu.lookup(addr & !0x1f, Access::Write, self.sr & SR_MD != 0)?;
            pa & !0x1f
        } else {
            let qacr = if q == 0 { self.mmu.qacr0 } else { self.mmu.qacr1 };
            (addr & 0x03ff_ffe0) | ((qacr & 0x1c) << 24)
        };
        for i in 0..8 {
            bus.write32(base + i * 4, self.io.sq[q][i as usize]);
        }
        Ok(())
    }

    // ---- exceptions and interrupts -------------------------------------

    pub(super) fn raise(&mut self, e: Exc) {
        if self.trace_exceptions {
            eprintln!(
                "[exc] {:?} addr={:08x} pc={:08x} sr={:08x} pr={:08x}",
                e.kind, e.addr, self.insn_pc, self.sr, self.pr
            );
        }
        if self.sr & SR_BL != 0 {
            self.fault = Some(format!(
                "exception {:?} (addr {:08x}) at pc {:08x} while SR.BL=1",
                e.kind, e.addr, self.insn_pc
            ));
            self.attention = true;
            return;
        }
        let mut spc = self.insn_pc;
        if self.in_delay {
            spc = spc.wrapping_sub(2);
        }
        let mut vector = 0x100;
        match e.kind {
            ExcKind::Trapa(n) => {
                spc = self.insn_pc.wrapping_add(2);
                self.io.tra = (n as u32) << 2;
            }
            ExcKind::TlbMissRead | ExcKind::TlbMissWrite => {
                vector = 0x400;
                self.mmu.set_fault_addr(e.addr);
            }
            ExcKind::InitialPageWrite | ExcKind::TlbProtRead | ExcKind::TlbProtWrite => {
                self.mmu.set_fault_addr(e.addr);
            }
            ExcKind::AddrErrRead | ExcKind::AddrErrWrite => {
                self.mmu.set_fault_addr(e.addr);
            }
            _ => {}
        }
        self.io.expevt = e.code();
        self.spc = spc;
        self.ssr = self.sr;
        self.sgr = self.r[15];
        self.set_sr(self.sr | SR_MD | SR_RB | SR_BL);
        self.pc = self.vbr.wrapping_add(vector);
        self.in_delay = false;
        self.rte_slot = false;
    }

    fn accept_interrupt(&mut self) {
        let (_, code) = self.io.intc.best();
        self.io.intevt = code;
        self.irq_counts[((code >> 5) & 127) as usize] += 1;
        self.spc = self.pc;
        self.ssr = self.sr;
        self.sgr = self.r[15];
        self.set_sr(self.sr | SR_MD | SR_RB | SR_BL);
        self.pc = self.vbr.wrapping_add(0x600);
        self.sleeping = false;
        if self.trace_exceptions {
            eprintln!("[irq] intevt={:03x} spc={:08x} ssr={:08x}", code, self.spc, self.ssr);
        }
    }

    // ---- execution ------------------------------------------------------

    /// Execute one instruction (or accept one interrupt).
    #[inline(always)]
    pub fn step<B: Bus>(&mut self, bus: &mut B) {
        // Interrupts are held off by SR.BL, except that they still end
        // sleep mode.
        if self.io.intc.level > self.imask() && (self.sr & SR_BL == 0 || self.sleeping) {
            self.accept_interrupt();
        } else if self.io.intc.nmi_pending && (self.sr & SR_BL == 0 || self.sleeping) {
            self.io.intc.nmi_pending = false;
            self.io.intevt = 0x1c0;
            self.spc = self.pc;
            self.ssr = self.sr;
            self.sgr = self.r[15];
            self.set_sr(self.sr | SR_MD | SR_RB | SR_BL);
            self.pc = self.vbr.wrapping_add(0x600);
            self.sleeping = false;
        }
        if self.sleeping {
            return;
        }
        let pc = self.pc;
        self.insn_pc = pc;
        self.pc_hist[self.pc_hist_pos & 255] = pc;
        self.pc_hist_pos = self.pc_hist_pos.wrapping_add(1);
        bus.set_pc(pc, self.pr);
        match self.fetch(bus, pc) {
            Ok(op) => {
                self.pc = pc.wrapping_add(2);
                if let Err(e) = self.exec(bus, op) {
                    self.raise(e);
                }
            }
            Err(e) => self.raise(e),
        }
    }

    /// Run for up to `budget` cycles.  Peripheral time advances with the
    /// cycle counter; while the CPU sleeps, time skips to the next event.
    pub fn run<B: Bus>(&mut self, bus: &mut B, budget: u64) -> Stop {
        let end = self.cycles + budget;
        self.run_end = end;
        // Breakpoints (the firmware's log hooks are some) are looked up
        // through a small filter: one bit test per instruction instead
        // of a search of the list.
        let mut bp_filter = [false; 4096];
        for bp in &self.breakpoints {
            bp_filter[(bp >> 1) as usize & 4095] = true;
        }
        // Whatever was set from outside since the last call.
        self.attention = true;
        while self.cycles < end {
            if self.cycles >= self.io.next_event {
                self.io.run_events(bus, self.cycles);
            }
            let irl = bus.irl();
            if irl != self.io.intc.irl {
                self.io.intc.set_irl(irl);
            }
            if self.sleeping && self.io.intc.level <= self.imask() && !self.io.intc.nmi_pending {
                // Idle: skip ahead to the next peripheral event.
                let next = self.io.next_event.min(end);
                self.cycles = next.max(self.cycles + 1);
                continue;
            }
            while self.cycles < end && self.cycles < self.io.next_event {
                self.step(bus);
                self.cycles += 1;
                let irl = bus.irl();
                if irl != self.io.intc.irl {
                    self.io.intc.set_irl(irl);
                }
                if self.attention {
                    if self.sleeping {
                        break;
                    }
                    if self.fault.is_some() {
                        return Stop::Fault;
                    }
                    if self.hypercall.is_some() {
                        return Stop::Hypercall;
                    }
                    self.attention = false;
                }
                if bp_filter[(self.pc >> 1) as usize & 4095] && self.breakpoints.contains(&self.pc) {
                    return Stop::Breakpoint;
                }
            }
        }
        Stop::Budget
    }

    /// Recent PCs, oldest first.
    pub fn history(&self) -> Vec<u32> {
        (0..256)
            .map(|i| self.pc_hist[(self.pc_hist_pos + i) & 255])
            .collect()
    }

    pub fn dump(&self) -> String {
        let mut s = String::new();
        for i in 0..16 {
            s += &format!("r{:<2}={:08x}{}", i, self.r[i], if i % 4 == 3 { "\n" } else { " " });
        }
        s += &format!(
            "pc={:08x} pr={:08x} sr={:08x} gbr={:08x} vbr={:08x}\nssr={:08x} spc={:08x} mach={:08x} macl={:08x} fpscr={:08x} fpul={:08x}\n",
            self.pc, self.pr, self.sr, self.gbr, self.vbr, self.ssr, self.spc, self.mach, self.macl, self.fpscr, self.fpul
        );
        s
    }
}
