//! SH-4 MMU: CCN translation registers and the 64-entry UTLB.
//!
//! The ITLB is not modelled separately; instruction fetches are resolved
//! through the UTLB (on hardware an ITLB miss is refilled from the UTLB
//! automatically, so software cannot normally tell the difference).

use super::cpu::{Access, Exc, ExcKind};

pub const MMUCR_AT: u32 = 1 << 0;
pub const MMUCR_TI: u32 = 1 << 2;
pub const MMUCR_SV: u32 = 1 << 8;
pub const MMUCR_SQMD: u32 = 1 << 9;

#[derive(Clone, Copy, Default, Debug)]
pub struct TlbEntry {
    pub vpn: u32,  // virtual address of page start
    pub mask: u32, // address mask covering the page number bits
    pub ppn: u32,  // physical address of page start
    pub asid: u8,
    pub valid: bool,
    pub sz: u8,
    pub pr: u8,
    pub c: bool,
    pub d: bool,
    pub sh: bool,
    pub wt: bool,
    pub ptea: u32,
}

const SZ_MASK: [u32; 4] = [0xffff_fc00, 0xffff_f000, 0xffff_0000, 0xfff0_0000];

pub struct Mmu {
    pub pteh: u32,
    pub ptel: u32,
    pub ttb: u32,
    pub tea: u32,
    pub mmucr: u32,
    pub ptea: u32,
    pub qacr0: u32,
    pub qacr1: u32,
    pub ccr: u32,
    /// The bits of CCR the processor has: the SH7750R's by default.
    /// The 193's (an SH-4A) has no double-size or index modes: a bench
    /// unit reads 0x101 where 0x80008181 was written.
    pub ccr_mask: u32,
    pub utlb: [TlbEntry; 64],
    pub itlb: [TlbEntry; 4],
    /// One-entry-per-slot cache of recent translations, flushed on any
    /// change to the TLB, ASID or privilege mode.
    soft: [(u32, u32, u8); 64],
    pub ldtlb_count: u64,
}

impl Default for Mmu {
    fn default() -> Self {
        Self::new()
    }
}

impl Mmu {
    pub fn new() -> Self {
        Mmu {
            pteh: 0,
            ptel: 0,
            ttb: 0,
            tea: 0,
            mmucr: 0,
            ptea: 0,
            qacr0: 0,
            qacr1: 0,
            ccr: 0,
            ccr_mask: 0x8000_89af,
            utlb: [TlbEntry::default(); 64],
            itlb: [TlbEntry::default(); 4],
            soft: [(1, 0, 0); 64],
            ldtlb_count: 0,
        }
    }

    #[inline(always)]
    pub fn enabled(&self) -> bool {
        self.mmucr & MMUCR_AT != 0
    }
    #[inline]
    pub fn sqmd(&self) -> bool {
        self.mmucr & MMUCR_SQMD != 0
    }

    pub fn flush_soft(&mut self) {
        self.soft = [(1, 0, 0); 64];
    }

    pub fn set_fault_addr(&mut self, addr: u32) {
        self.tea = addr;
        self.pteh = (self.pteh & 0x3ff) | (addr & 0xffff_fc00);
    }

    /// Translate through the UTLB.
    pub fn lookup(&mut self, addr: u32, acc: Access, privileged: bool) -> Result<u32, Exc> {
        // Soft cache: keyed on the 4K page, with permission bits
        // (bit0 = readable, bit1 = writable) valid for the current mode.
        let slot = ((addr >> 12) & 63) as usize;
        let (tag, base, perm) = self.soft[slot];
        let need = if acc == Access::Write { 2 } else { 1 };
        if tag == (addr & 0xffff_f000) && perm & need != 0 {
            return Ok(base | (addr & 0xfff));
        }
        let asid = (self.pteh & 0xff) as u8;
        let single = self.mmucr & MMUCR_SV != 0;
        let mut hit: Option<usize> = None;
        for (i, e) in self.utlb.iter().enumerate() {
            if e.valid && (addr & e.mask) == e.vpn {
                let asid_ok = e.sh || e.asid == asid || (privileged && single);
                if asid_ok {
                    hit = Some(i);
                    break;
                }
            }
        }
        let write = acc == Access::Write;
        let Some(i) = hit else {
            return Err(Exc::mem(
                if write { ExcKind::TlbMissWrite } else { ExcKind::TlbMissRead },
                addr,
            ));
        };
        let e = self.utlb[i];
        // PR: 00 = priv RO, 01 = priv RW, 10 = all RO, 11 = all RW
        let (can_r, can_w) = if privileged {
            (true, e.pr & 1 != 0)
        } else {
            (e.pr & 2 != 0, e.pr == 3)
        };
        if write {
            if !can_w {
                return Err(Exc::mem(ExcKind::TlbProtWrite, addr));
            }
            if !e.d {
                return Err(Exc::mem(ExcKind::InitialPageWrite, addr));
            }
        } else if !can_r {
            return Err(Exc::mem(ExcKind::TlbProtRead, addr));
        }
        let pa = (e.ppn | (addr & !e.mask)) & 0x1fff_ffff;
        if e.sz != 0 {
            let perm = (can_r as u8) | (((can_w && e.d) as u8) << 1);
            self.soft[slot] = (addr & 0xffff_f000, pa & 0xffff_f000, perm);
        }
        Ok(pa)
    }

    fn make_entry(pteh: u32, ptel: u32, ptea: u32) -> TlbEntry {
        let sz = (((ptel >> 7) & 1) << 1 | ((ptel >> 4) & 1)) as u8;
        let mask = SZ_MASK[sz as usize];
        TlbEntry {
            vpn: pteh & mask,
            mask,
            ppn: ptel & 0x1fff_fc00 & mask,
            asid: (pteh & 0xff) as u8,
            valid: ptel & 0x100 != 0,
            sz,
            pr: ((ptel >> 5) & 3) as u8,
            c: ptel & 8 != 0,
            d: ptel & 4 != 0,
            sh: ptel & 2 != 0,
            wt: ptel & 1 != 0,
            ptea,
        }
    }

    /// LDTLB: load PTEH/PTEL/PTEA into the UTLB entry selected by URC.
    pub fn ldtlb(&mut self) {
        let urc = ((self.mmucr >> 10) & 0x3f) as usize;
        self.utlb[urc] = Self::make_entry(self.pteh, self.ptel, self.ptea);
        self.ldtlb_count += 1;
        // URC is a free-running counter on hardware; advancing it here
        // gives round-robin replacement.
        let urb = ((self.mmucr >> 18) & 0x3f) as usize;
        let mut next = urc + 1;
        if next >= 64 || (urb != 0 && next >= urb) {
            next = 0;
        }
        self.mmucr = (self.mmucr & !(0x3f << 10)) | ((next as u32) << 10);
        self.flush_soft();
    }

    pub fn reg_read(&self, addr: u32) -> Option<u32> {
        Some(match addr {
            0xff00_0000 => self.pteh,
            0xff00_0004 => self.ptel,
            0xff00_0008 => self.ttb,
            0xff00_000c => self.tea,
            0xff00_0010 => self.mmucr,
            0xff00_001c => self.ccr,
            0xff00_0034 => self.ptea,
            0xff00_0038 => self.qacr0,
            0xff00_003c => self.qacr1,
            _ => return None,
        })
    }

    pub fn reg_write(&mut self, addr: u32, v: u32) -> bool {
        match addr {
            0xff00_0000 => {
                if (v ^ self.pteh) & 0xff != 0 {
                    self.flush_soft();
                }
                self.pteh = v & 0xffff_fcff;
            }
            0xff00_0004 => self.ptel = v & 0x1fff_fdff,
            0xff00_0008 => self.ttb = v,
            0xff00_000c => self.tea = v,
            0xff00_0010 => {
                self.mmucr = v & 0xfcfc_ff05 & !MMUCR_TI;
                if v & MMUCR_TI != 0 {
                    for e in self.utlb.iter_mut() {
                        e.valid = false;
                    }
                    for e in self.itlb.iter_mut() {
                        e.valid = false;
                    }
                }
                self.flush_soft();
            }
            0xff00_001c => self.ccr = v & self.ccr_mask & !0x808, // ICI/OCI read as 0
            0xff00_0034 => self.ptea = v & 0xf,
            0xff00_0038 => self.qacr0 = v & 0x1c,
            0xff00_003c => self.qacr1 = v & 0x1c,
            _ => return false,
        }
        true
    }

    /// Memory-mapped cache/TLB arrays (0xF000_0000 - 0xF7FF_FFFF).
    pub fn array_read(&self, addr: u32) -> u32 {
        let e = ((addr >> 8) & 0x3f) as usize;
        match addr >> 24 {
            0xf2 => {
                let t = &self.itlb[e & 3];
                t.vpn | ((t.valid as u32) << 8) | t.asid as u32
            }
            0xf3 => {
                let t = &self.itlb[e & 3];
                Self::data1(t)
            }
            0xf6 => {
                let t = &self.utlb[e];
                t.vpn | ((t.d as u32) << 9) | ((t.valid as u32) << 8) | t.asid as u32
            }
            0xf7 => {
                let t = &self.utlb[e];
                if addr & 0x0080_0000 != 0 {
                    t.ptea
                } else {
                    Self::data1(t)
                }
            }
            _ => 0, // cache arrays: nothing cached
        }
    }

    fn data1(t: &TlbEntry) -> u32 {
        t.ppn
            | ((t.valid as u32) << 8)
            | (((t.sz >> 1) as u32) << 7)
            | ((t.pr as u32) << 5)
            | (((t.sz & 1) as u32) << 4)
            | ((t.c as u32) << 3)
            | ((t.d as u32) << 2)
            | ((t.sh as u32) << 1)
            | t.wt as u32
    }

    pub fn array_write(&mut self, addr: u32, v: u32) {
        let e = ((addr >> 8) & 0x3f) as usize;
        match addr >> 24 {
            0xf2 => {
                let t = &mut self.itlb[e & 3];
                t.vpn = v & t.mask.max(0xffff_fc00);
                t.valid = v & 0x100 != 0;
                t.asid = v as u8;
            }
            0xf3 => {
                let old = self.itlb[e & 3];
                let mut t = Self::make_entry(old.vpn | old.asid as u32, v, old.ptea);
                t.valid = v & 0x100 != 0;
                self.itlb[e & 3] = t;
            }
            0xf6 => {
                if addr & 0x80 != 0 {
                    // Associative write: update D and V of the matching entry.
                    let asid = v as u8;
                    for t in self.utlb.iter_mut() {
                        if t.valid && t.vpn == (v & t.mask) && (t.sh || t.asid == asid) {
                            t.d = v & 0x200 != 0;
                            t.valid = v & 0x100 != 0;
                        }
                    }
                } else {
                    let t = &mut self.utlb[e];
                    t.vpn = v & t.mask.max(0xffff_fc00) & 0xffff_fc00;
                    t.d = v & 0x200 != 0;
                    t.valid = v & 0x100 != 0;
                    t.asid = v as u8;
                }
            }
            0xf7 => {
                if addr & 0x0080_0000 != 0 {
                    self.utlb[e].ptea = v & 0xf;
                } else {
                    let old = self.utlb[e];
                    let t = Self::make_entry(old.vpn | old.asid as u32, v, old.ptea);
                    self.utlb[e] = t;
                }
            }
            _ => {} // cache arrays
        }
        self.flush_soft();
    }
}
