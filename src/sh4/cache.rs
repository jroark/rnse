//! The SH-4's caches, for finding the mistakes real hardware punishes
//! and an emulator without caches forgives: code that is written and
//! then run without the instruction cache being told, and data that
//! sits in the write-back cache while something else (a device, an
//! access through the uncached P2 window) looks at memory.
//!
//! Off unless [`Cache::enabled`] is set (`rnse --strict`): it costs
//! time, and the stock software runs the same with or without it.
//!
//! What is modelled:
//!
//! * an instruction cache and an operand cache of 32-byte lines for
//!   accesses to RAM through P0/U0, P1 and P3 (not P2, not P4, not
//!   devices or flash), tagged by physical address;
//! * the operand cache's two ways of storing, chosen by CCR for P1
//!   (bit 2, CB) and for the rest (bit 1, WT): copy-back, where a store
//!   stays in the cache until the line is evicted, written back
//!   (`OCBWB`), purged (`OCBP`) or thrown away (`OCBI`, CCR.OCI), and
//!   write-through, where it goes to memory as well;
//! * `MOVCA.L`, and CCR's enable and invalidate bits.  The model starts
//!   with CCR as the stock operating system of both units sets it,
//!   [`STOCK_CCR`]: both caches on, P1 write-through, the rest
//!   copy-back.  That is what a loader finds (the 192's never writes
//!   CCR; the 193's writes 0x101, which means the same).
//!
//! What is not: the real organisation.  The SH7750R's caches are 16 and
//! 32 KiB, two-way; the SH-4A's 32 KiB, four-way.  Here both are
//! direct-mapped, [`IC_LINES`] and [`OC_LINES`] lines.  Which lines are
//! in a cache at a given moment therefore differs from the real chip:
//! a stale line met here may have been evicted there, and the other way
//! round.  A program that keeps the rules passes both; one that breaks
//! them is caught here when it is unlucky here.

use super::Bus;

pub const LINE: u32 = 32;
pub const IC_LINES: usize = 512;
pub const OC_LINES: usize = 1024;

/// CCR as the stock operating systems leave it.
pub const STOCK_CCR: u32 = 0x8000_8181;

const CCR_OCE: u32 = 1 << 0;
const CCR_WT: u32 = 1 << 1;
const CCR_CB: u32 = 1 << 2;
const CCR_OCI: u32 = 1 << 3;
const CCR_ICE: u32 = 1 << 8;
const CCR_ICI: u32 = 1 << 11;

#[derive(Clone, Copy)]
struct Entry {
    /// Physical address of the line; odd = empty.
    tag: u32,
    dirty: bool,
    data: [u8; LINE as usize],
}

const EMPTY: Entry = Entry { tag: 1, dirty: false, data: [0; LINE as usize] };

/// What went wrong, for the report at the end of a run.
#[derive(Default, Clone, Debug)]
pub struct Trouble {
    /// Instructions executed from instruction cache lines that no
    /// longer matched memory, and where the first one was.
    pub stale_fetches: u64,
    pub first_stale_pc: u32,
    /// Lines with unwritten data thrown away (`OCBI`, CCR.OCI, the
    /// operand cache being switched off), and the first such line.
    pub dirty_discarded: u64,
    pub first_discarded: u32,
}

pub struct Cache {
    pub enabled: bool,
    ic: Vec<Entry>,
    oc: Vec<Entry>,
    ic_on: bool,
    oc_on: bool,
    /// Stores through P1 are copy-back (CCR.CB); stores through the
    /// other cached areas are write-through (CCR.WT).
    p1_copy_back: bool,
    p0_write_through: bool,
    pub trouble: Trouble,
}

impl Default for Cache {
    fn default() -> Self {
        Cache { enabled: false, ic: Vec::new(), oc: Vec::new(), ic_on: true, oc_on: true, p1_copy_back: false, p0_write_through: false, trouble: Trouble::default() }
    }
}

/// Is an access at this virtual address cached at all?  P2 and P4 are
/// not.
#[inline(always)]
pub fn cacheable(addr: u32) -> bool {
    !(0xa000_0000..0xc000_0000).contains(&addr) && addr < 0xe000_0000
}

impl Cache {
    /// Switch the model on, with both caches empty and CCR as given
    /// (normally [`STOCK_CCR`]).
    pub fn enable(&mut self, ccr: u32) {
        self.enabled = true;
        self.ic = vec![EMPTY; IC_LINES];
        self.oc = vec![EMPTY; OC_LINES];
        self.ccr_write(ccr);
    }

    fn fill<B: Bus>(bus: &mut B, line: u32) -> [u8; LINE as usize] {
        let mut data = [0u8; LINE as usize];
        for (i, word) in data.chunks_exact_mut(4).enumerate() {
            word.copy_from_slice(&bus.read32(line + 4 * i as u32).to_be_bytes());
        }
        data
    }

    fn write_back<B: Bus>(bus: &mut B, e: &mut Entry) {
        if e.dirty {
            for (i, word) in e.data.chunks_exact(4).enumerate() {
                bus.write32(e.tag + 4 * i as u32, u32::from_be_bytes([word[0], word[1], word[2], word[3]]));
            }
            e.dirty = false;
        }
    }

    /// The instruction at physical address `pa`, from the instruction
    /// cache: what was in memory when the line was loaded.
    pub fn fetch<B: Bus>(&mut self, bus: &mut B, pa: u32, pc: u32) -> u16 {
        if !self.ic_on {
            return bus.read16(pa);
        }
        let line = pa & !(LINE - 1);
        let e = &mut self.ic[(line / LINE) as usize % IC_LINES];
        if e.tag != line {
            e.tag = line;
            e.data = Self::fill(bus, line);
        }
        let o = (pa & (LINE - 1)) as usize;
        let cached = u16::from_be_bytes([e.data[o], e.data[o + 1]]);
        if cached != bus.read16(pa) {
            if self.trouble.stale_fetches == 0 {
                self.trouble.first_stale_pc = pc;
            }
            self.trouble.stale_fetches += 1;
        }
        cached
    }

    fn oc_line<B: Bus>(&mut self, bus: &mut B, line: u32, load: bool) -> &mut Entry {
        let e = &mut self.oc[(line / LINE) as usize % OC_LINES];
        if e.tag != line {
            Self::write_back(bus, e);
            e.tag = line;
            e.data = if load { Self::fill(bus, line) } else { [0; LINE as usize] };
        }
        e
    }

    /// A load of `size` bytes (1, 2 or 4, aligned) through the operand
    /// cache.
    pub fn read<B: Bus>(&mut self, bus: &mut B, pa: u32, size: u32) -> u32 {
        if !self.oc_on {
            return match size {
                1 => bus.read8(pa) as u32,
                2 => bus.read16(pa) as u32,
                _ => bus.read32(pa),
            };
        }
        let e = self.oc_line(bus, pa & !(LINE - 1), true);
        let o = (pa & (LINE - 1)) as usize;
        e.data[o..o + size as usize].iter().fold(0, |v, b| v << 8 | *b as u32)
    }

    /// A store through the operand cache, made at virtual address
    /// `addr`: in copy-back mode it stays in the cache, in write-through
    /// mode it goes to memory too.
    pub fn write<B: Bus>(&mut self, bus: &mut B, pa: u32, size: u32, v: u32, addr: u32) {
        self.write_policy(bus, pa, size, v, addr, None);
    }

    pub(super) fn write_policy<B: Bus>(&mut self, bus: &mut B, pa: u32, size: u32, v: u32, addr: u32, page_wt: Option<bool>) {
        let through = page_wt.unwrap_or(if (0x8000_0000..0xa000_0000).contains(&addr) {
            !self.p1_copy_back
        } else {
            self.p0_write_through
        });
        if !self.oc_on || through {
            match size {
                1 => bus.write8(pa, v as u8),
                2 => bus.write16(pa, v as u16),
                _ => bus.write32(pa, v),
            }
            if !self.oc_on {
                return;
            }
        }
        let e = self.oc_line(bus, pa & !(LINE - 1), true);
        let o = (pa & (LINE - 1)) as usize;
        for i in 0..size as usize {
            e.data[o + i] = (v >> (8 * (size as usize - 1 - i))) as u8;
        }
        if !through {
            e.dirty = true;
        }
    }

    /// `MOVCA.L`: the line is claimed without being read from memory.
    pub fn movca<B: Bus>(&mut self, bus: &mut B, pa: u32, v: u32) {
        if !self.oc_on {
            bus.write32(pa, v);
            return;
        }
        let e = self.oc_line(bus, pa & !(LINE - 1), false);
        let o = (pa & (LINE - 1)) as usize;
        e.data[o..o + 4].copy_from_slice(&v.to_be_bytes());
        e.dirty = true;
    }

    fn discard(trouble: &mut Trouble, e: &mut Entry) {
        if e.dirty {
            if trouble.dirty_discarded == 0 {
                trouble.first_discarded = e.tag;
            }
            trouble.dirty_discarded += 1;
        }
        *e = EMPTY;
    }

    /// `OCBI` (invalidate), `OCBP` (write back and invalidate), `OCBWB`
    /// (write back) on the line of physical address `pa`.
    pub fn ocb<B: Bus>(&mut self, bus: &mut B, pa: u32, write_back: bool, invalidate: bool) {
        let line = pa & !(LINE - 1);
        let e = &mut self.oc[(line / LINE) as usize % OC_LINES];
        if e.tag != line {
            return;
        }
        if write_back {
            Self::write_back(bus, e);
        }
        if invalidate {
            Self::discard(&mut self.trouble, e);
        }
    }

    /// `ICBI`.
    pub fn icbi(&mut self, pa: u32) {
        let line = pa & !(LINE - 1);
        let e = &mut self.ic[(line / LINE) as usize % IC_LINES];
        if e.tag == line {
            *e = EMPTY;
        }
    }

    /// A write to an address array (0xF0.., the instruction cache's;
    /// 0xF4.., the operand cache's), the way code that empties the
    /// caches without knowing what they hold does it.  A
    /// non-associative write (address bit 3 clear) whose data leaves
    /// the line invalid (bit 0, V, clear) writes the line back first
    /// if it is dirty (operand cache).  The model is direct-mapped, so
    /// it takes the entry (address bits 14:5, on the chip the index
    /// and the way) for its line number: a sweep over all entries
    /// empties it as it empties the chip.  Associative writes, and
    /// writes that make a line valid, are not modelled.
    pub fn array_write<B: Bus>(&mut self, bus: &mut B, addr: u32, v: u32) {
        if !self.enabled || addr & 8 != 0 || v & 1 != 0 {
            return;
        }
        match addr >> 24 {
            0xf0 => self.ic[(addr >> 5) as usize % IC_LINES] = EMPTY,
            0xf4 => {
                let e = &mut self.oc[(addr >> 5) as usize % OC_LINES];
                Self::write_back(bus, e);
                *e = EMPTY;
            }
            _ => {}
        }
    }

    /// A write to CCR.
    pub fn ccr_write(&mut self, v: u32) {
        if v & CCR_ICI != 0 || v & CCR_ICE == 0 {
            self.ic.iter_mut().for_each(|e| *e = EMPTY);
        }
        if v & CCR_OCI != 0 || v & CCR_OCE == 0 {
            // Nothing is written back: what was only in the cache is
            // gone.
            for e in self.oc.iter_mut() {
                Self::discard(&mut self.trouble, e);
            }
        }
        self.ic_on = v & CCR_ICE != 0;
        self.oc_on = v & CCR_OCE != 0;
        self.p1_copy_back = v & CCR_CB != 0;
        self.p0_write_through = v & CCR_WT != 0;
    }

    /// A byte as the CPU would read it now, without disturbing the
    /// cache: for the debugger and the log hooks.
    pub fn peek(&self, pa: u32) -> Option<u8> {
        if !self.enabled || !self.oc_on {
            return None;
        }
        let line = pa & !(LINE - 1);
        let e = &self.oc[(line / LINE) as usize % OC_LINES];
        (e.tag == line).then(|| e.data[(pa & (LINE - 1)) as usize])
    }

    /// The emulator itself has changed a byte of memory on the CPU's
    /// behalf (a boot ROM service, the debugger): the caches follow, as
    /// if the CPU had done it and done it properly.
    pub fn host_write(&mut self, pa: u32, v: u8) {
        if !self.enabled {
            return;
        }
        let line = pa & !(LINE - 1);
        let o = (pa & (LINE - 1)) as usize;
        let e = &mut self.oc[(line / LINE) as usize % OC_LINES];
        if e.tag == line {
            e.data[o] = v;
        }
        let e = &mut self.ic[(line / LINE) as usize % IC_LINES];
        if e.tag == line {
            e.data[o] = v;
        }
    }

    /// Everything that is only in the operand cache goes to memory and
    /// both caches are emptied.  For the moments at which the emulator
    /// acts in the CPU's place (a boot ROM service done in Rust): it
    /// reads and writes memory, and what it leaves there is what the
    /// CPU must see afterwards.
    pub fn sync<B: Bus>(&mut self, bus: &mut B) {
        if !self.enabled {
            return;
        }
        for e in self.oc.iter_mut() {
            Self::write_back(bus, e);
            *e = EMPTY;
        }
        self.ic.iter_mut().for_each(|e| *e = EMPTY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl Bus for Ram {
        fn read8(&mut self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn read16(&mut self, a: u32) -> u16 {
            u16::from_be_bytes([self.0[a as usize], self.0[a as usize + 1]])
        }
        fn read32(&mut self, a: u32) -> u32 {
            (self.read16(a) as u32) << 16 | self.read16(a + 2) as u32
        }
        fn write8(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
        fn write16(&mut self, a: u32, v: u16) {
            self.0[a as usize..a as usize + 2].copy_from_slice(&v.to_be_bytes());
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.0[a as usize..a as usize + 4].copy_from_slice(&v.to_be_bytes());
        }
    }

    #[test]
    fn stores_stay_in_the_cache_until_written_back() {
        let mut ram = Ram(vec![0; 0x10000]);
        let mut c = Cache::default();
        c.enable(STOCK_CCR);
        c.write(&mut ram, 0x100, 4, 0x1122_3344, 0x100);
        assert_eq!(c.read(&mut ram, 0x100, 4), 0x1122_3344);
        assert_eq!(c.read(&mut ram, 0x102, 2), 0x3344);
        assert_eq!(ram.read32(0x100), 0, "memory has not seen it");
        assert_eq!(c.peek(0x101), Some(0x22));
        c.ocb(&mut ram, 0x104, true, false); // OCBWB anywhere in the line
        assert_eq!(ram.read32(0x100), 0x1122_3344);
        // Another line with the same index evicts it, written back.
        c.write(&mut ram, 0x200, 1, 0x55, 0x200);
        c.read(&mut ram, 0x200 + LINE * OC_LINES as u32, 1);
        assert_eq!(ram.read8(0x200), 0x55);
        // Invalidating a line with unwritten data loses the data.
        c.write(&mut ram, 0x300, 1, 0x77, 0x300);
        c.ocb(&mut ram, 0x300, false, true); // OCBI
        assert_eq!(c.read(&mut ram, 0x300, 1), 0);
        assert_eq!((c.trouble.dirty_discarded, c.trouble.first_discarded), (1, 0x300));
        // Purging keeps it.
        c.write(&mut ram, 0x400, 1, 0x99, 0x400);
        c.ocb(&mut ram, 0x400, true, true); // OCBP
        assert_eq!((ram.read8(0x400), c.peek(0x400)), (0x99, None));
        // Through P1 the stock setting is write-through: memory has it
        // at once, and the cache agrees.
        c.write(&mut ram, 0x500, 2, 0xabcd, 0x8000_0500);
        assert_eq!((ram.read16(0x500), c.read(&mut ram, 0x500, 2)), (0xabcd, 0xabcd));
        // With CCR.CB that is copy-back as well.
        c.ccr_write(STOCK_CCR | CCR_CB);
        c.write(&mut ram, 0x600, 1, 0x11, 0x8000_0600);
        assert_eq!(ram.read8(0x600), 0);
    }

    #[test]
    fn a_sweep_of_the_address_array_writes_everything_back() {
        let mut ram = Ram(vec![0; 0x10000]);
        let mut c = Cache::default();
        c.enable(STOCK_CCR);
        c.write(&mut ram, 0x100, 4, 0x1122_3344, 0x100);
        c.write(&mut ram, 0x8000, 1, 0x55, 0x8000);
        // An associative write, or one that leaves a line valid, does
        // nothing here.
        c.array_write(&mut ram, 0xf400_0108, 0);
        c.array_write(&mut ram, 0xf400_0100, 1);
        assert_eq!(ram.read32(0x100), 0);
        // What Linux and the SDK's cache_setup do: zero into every
        // entry of the operand cache's address array.
        for entry in 0..1024u32 {
            c.array_write(&mut ram, 0xf400_0000 | entry << 5, 0);
        }
        assert_eq!((ram.read32(0x100), ram.read8(0x8000)), (0x1122_3344, 0x55));
        assert_eq!((c.peek(0x100), c.peek(0x8000)), (None, None));
        // Then CCR's invalidate bit loses nothing.
        c.ccr_write(STOCK_CCR | CCR_OCI);
        assert_eq!(c.trouble.dirty_discarded, 0);
    }

    #[test]
    fn the_instruction_cache_keeps_what_it_loaded() {
        let mut ram = Ram(vec![0; 0x10000]);
        let mut c = Cache::default();
        c.enable(STOCK_CCR);
        ram.write16(0x1000, 0x0009);
        assert_eq!(c.fetch(&mut ram, 0x1000, 0x8c00_1000), 0x0009);
        // New code in memory, the cache not told: the old instruction.
        ram.write16(0x1000, 0x000b);
        assert_eq!(c.fetch(&mut ram, 0x1000, 0x8c00_1000), 0x0009);
        assert_eq!((c.trouble.stale_fetches, c.trouble.first_stale_pc), (1, 0x8c00_1000));
        c.ccr_write(CCR_ICE | CCR_OCE | CCR_ICI);
        assert_eq!(c.fetch(&mut ram, 0x1000, 0x8c00_1000), 0x000b);
        ram.write16(0x1000, 0x0019);
        c.icbi(0x1002);
        assert_eq!(c.fetch(&mut ram, 0x1000, 0x8c00_1000), 0x0019);
        assert_eq!(c.trouble.stale_fetches, 1);
        // New code written through the operand cache and never written
        // back is not what gets executed either.
        c.write(&mut ram, 0x2000, 2, 0x000b, 0x2000);
        assert_eq!(c.fetch(&mut ram, 0x2000, 0), 0x0000);
    }
}
