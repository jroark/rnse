//! NOR flash with the AMD/Fujitsu command set on a 32-bit data bus.
//!
//! The 192 has two chips of 16 MiB side by side in the address space
//! (Fujitsu MBM29XL12DF, by the marking on a unit's board), each with
//! its own command state and its own eight small sectors at the bottom:
//! the stock loader erases 8 KiB sectors from 0x01000000 as it does
//! from 0x0000c000.
//!
//! The 193 has four Spansion S99PL127J (128 Mbit, 16 bits wide, by the
//! markings on the bench unit's board) in two banks of 32 MiB, each bank
//! a pair of chips side by side on the 32-bit bus: one half of every
//! word in each chip.  Its stock loader writes every command to both at
//! once (0x00aa00aa at 0x1554) and erases in 128 KiB steps, two 64 KiB
//! sectors, except for 16 KiB steps from 0x01fe0000 to 0x0201c000: the
//! pairs of 8 KiB sectors at the top of the first bank and at the
//! bottom of the second.  A pair is emulated as one device with one
//! command state, sectors twice the size of a chip's.
//!
//! Code executes in place from this memory, so plain reads are served
//! directly by the board; this module only sees writes (the command
//! state machine) and reads while the chip is in a non-array mode
//! (autoselect / CFI query).
//!
//! Erase and program complete instantly, which satisfies both DQ7 and
//! toggle-bit polling.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Read,
    Unlock1,
    Unlock2,
    /// AA 55 80 seen
    EraseSetup,
    EraseUnlock1,
    EraseUnlock2,
    /// AA 55 A0 seen: next write programs a word
    Program,
    Autoselect,
    /// The common flash interface query (98 at 0x55)
    Query,
    /// Unlock bypass mode (AA 55 20)
    Bypass,
    BypassProgram,
    BypassReset,
}

pub struct Flash {
    /// One command state per chip.
    state: [State; 4],
    /// Size of a chip in bytes; 0 when the whole memory is one chip.
    chip: u32,
    /// Chips side by side on the bus as one device: 1 on the 192, 2 on
    /// the 193 (two 16-bit chips), which doubles every sector.
    width: u32,
    /// Statistics, for logs and tests.
    pub erased_sectors: u64,
    pub programmed_words: u64,
    pub trace: bool,
}

impl Default for Flash {
    fn default() -> Self {
        Self::new()
    }
}

/// Erase block containing `addr` in a chip of `size` bytes that starts
/// at address 0: eight 8 KiB sectors at the bottom, eight more at the
/// top, 64 KiB sectors between.  That is what the 192's chips say of
/// themselves ([`QUERY`]), and the 193's by their data sheet; a `size`
/// of 0 leaves the top ones out.
pub fn sector(addr: u32, size: u32) -> (u32, u32) {
    sector_of(addr, size, 1)
}

/// [`sector`] for `width` chips side by side, every sector `width`
/// times the size.
fn sector_of(addr: u32, size: u32, width: u32) -> (u32, u32) {
    let (small, large) = (0x2000 * width, 0x1_0000 * width);
    if addr < large || (size != 0 && addr >= size - large) {
        (addr / small * small, small)
    } else {
        (addr / large * large, large)
    }
}

/// What a 192's flash chips answer to the common flash interface query,
/// word 0x10 onwards, read from a unit (`sdk/examples/suite`): "QRY",
/// the AMD command set, 2.7 to 3.6 V, **a word written in 16 us as a
/// rule (2^4) and in 512 at most, a sector erased in 256 ms as a rule
/// (2^8) and in 16 s at most**, 16 MiB (2^0x18), and three regions of
/// sectors: 8 of 8 KiB, 254 of 64 KiB, 8 of 8 KiB.
pub const QUERY: [u8; 48] = [
    0x51, 0x52, 0x59, 0x02, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x27, 0x36, 0x00, 0x00, 0x04, 0x00, 0x08, 0x00, 0x05, 0x00, 0x06, 0x00, 0x18,
    0x05, 0x00, 0x00, 0x00, 0x03, 0x07, 0x00, 0x20, 0x00, 0xfd, 0x00, 0x00, 0x01, 0x07, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// The 193's chips, Spansion S29PL127J, from the data sheet, not read
/// from a unit yet: what one chip answers to the query, word 0x10
/// onwards, as far as the sheet's structure goes ("QRY", the AMD
/// command set, 16 MiB, 16 bits wide, three regions: 8 of 4 Kwords,
/// 254 of 32 Kwords, 8 of 4 Kwords).  The voltages and times (words
/// 0x1b-0x26) are left 0 until a unit has said them.
pub const QUERY_PL127J: [u8; 48] = [
    0x51, 0x52, 0x59, 0x02, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18,
    0x01, 0x00, 0x00, 0x00, 0x03, 0x07, 0x00, 0x20, 0x00, 0xfd, 0x00, 0x00, 0x01, 0x07, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

impl Flash {
    pub fn new() -> Self {
        Flash { state: [State::Read; 4], chip: 0, width: 1, erased_sectors: 0, programmed_words: 0, trace: false }
    }

    /// Banks of `size` bytes, each two 16-bit chips side by side (the
    /// 193).
    pub fn paired(size: u32) -> Self {
        Flash { chip: size, width: 2, ..Self::new() }
    }

    /// Chips of `size` bytes each, one after the other.
    pub fn chips(size: u32) -> Self {
        Flash { chip: size, ..Self::new() }
    }

    /// The chip an address belongs to, and where that chip starts.
    fn chip_of(&self, addr: u32) -> (usize, u32) {
        if self.chip == 0 {
            (0, 0)
        } else {
            (((addr / self.chip) & 3) as usize, addr / self.chip * self.chip)
        }
    }

    /// True when reads must not all come straight from the array.
    pub fn special_read(&self) -> bool {
        self.state.contains(&State::Autoselect) || self.state.contains(&State::Query)
    }

    pub fn read(&mut self, mem: &[u8], addr: u32, size: u32) -> u32 {
        let chip_state = self.state[self.chip_of(addr).0];
        if chip_state == State::Autoselect || chip_state == State::Query {
            // Word address within the chip selects the ID.
            let word = (addr >> 2) & 0xff;
            let v: u32 = if self.width == 2 {
                // Each chip answers in its half of the word.  Data
                // sheet values, as with QUERY_PL127J.
                let half = match (chip_state, word) {
                    (State::Autoselect, 0) => 0x0001, // manufacturer: Spansion
                    (State::Autoselect, 1) => 0x227e,
                    (State::Autoselect, 0x0e) => 0x2220,
                    (State::Autoselect, 0x0f) => 0x2200,
                    (State::Query, 0x10..=0x3f) => QUERY_PL127J[word as usize - 0x10] as u32,
                    _ => 0,
                };
                half << 16 | half
            } else {
                match (chip_state, word) {
                    (State::Autoselect, 0) => 0x04, // manufacturer: Fujitsu
                    // The device, as a unit's two chips say (the emulator
                    // had 0x2253).
                    (State::Autoselect, 1) => 0x2222_227e,
                    (State::Query, 0x10..=0x3f) if self.chip != 0 => QUERY[word as usize - 0x10] as u32,
                    _ => 0,
                }
            };
            return match size {
                4 => v,
                2 => (v >> ((2 - (addr & 2)) * 8)) & 0xffff,
                _ => (v >> ((3 - (addr & 3)) * 8)) & 0xff,
            };
        }
        let a = addr as usize;
        let mut v = 0;
        for i in 0..size as usize {
            v = (v << 8) | mem[a + i] as u32;
        }
        v
    }

    pub fn write(&mut self, mem: &mut [u8], addr: u32, size: u32, v: u32) {
        let cmd = v & 0xff;
        // Unlock addresses are 0x555 / 0x2aa on the word address bus.
        let key = (addr >> 2) & 0x7ff;
        let (c, base) = self.chip_of(addr);
        let state = self.state[c];
        if self.trace {
            eprintln!("[flash] {:?} W {:08x}.{} <= {:x}", state, addr, size, v);
        }
        self.state[c] = match state {
            State::Read | State::Autoselect => match (key, cmd) {
                (0x555, 0xaa) => State::Unlock1,
                (0x55, 0x98) => State::Query,
                (_, 0xf0) | (_, 0xff) => State::Read,
                _ => state,
            },
            State::Query => {
                if cmd == 0xf0 || cmd == 0xff {
                    State::Read
                } else {
                    State::Query
                }
            }
            State::Unlock1 => {
                if key == 0x2aa && cmd == 0x55 {
                    State::Unlock2
                } else {
                    State::Read
                }
            }
            State::Unlock2 => match cmd {
                0x80 => State::EraseSetup,
                0xa0 => State::Program,
                0x90 => State::Autoselect,
                0x20 => State::Bypass,
                _ => State::Read,
            },
            State::EraseSetup => {
                if key == 0x555 && cmd == 0xaa {
                    State::EraseUnlock1
                } else {
                    State::Read
                }
            }
            State::EraseUnlock1 => {
                if key == 0x2aa && cmd == 0x55 {
                    State::EraseUnlock2
                } else {
                    State::Read
                }
            }
            State::EraseUnlock2 => {
                match cmd {
                    0x30 => {
                        let (start, len) = sector_of(addr - base, self.chip, self.width);
                        let start = (base + start) as usize;
                        let end = (start + len as usize).min(mem.len());
                        mem[start..end].iter_mut().for_each(|b| *b = 0xff);
                        self.erased_sectors += 1;
                    }
                    0x10 => {
                        // The whole chip, and no other.
                        let end = if self.chip == 0 { mem.len() } else { ((base + self.chip) as usize).min(mem.len()) };
                        mem[base as usize..end].iter_mut().for_each(|b| *b = 0xff);
                        self.erased_sectors += 1;
                    }
                    _ => {}
                }
                State::Read
            }
            State::Program | State::BypassProgram => {
                let a = addr as usize;
                for i in 0..size as usize {
                    if a + i < mem.len() {
                        // Programming can only clear bits.
                        mem[a + i] &= (v >> ((size as usize - 1 - i) * 8)) as u8;
                    }
                }
                self.programmed_words += 1;
                if state == State::Program {
                    State::Read
                } else {
                    State::Bypass
                }
            }
            State::Bypass => match cmd {
                0xa0 => State::BypassProgram,
                0x90 => State::BypassReset,
                _ => State::Bypass,
            },
            State::BypassReset => State::Read,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unlock(f: &mut Flash, mem: &mut [u8], base: u32, cmd: u32) {
        f.write(mem, base + 0x555 * 4, 4, 0xaa);
        f.write(mem, base + 0x2aa * 4, 4, 0x55);
        f.write(mem, base + 0x555 * 4, 4, cmd);
    }

    /// Two chips, as in the 192: each with its own command state and
    /// its own small sectors at the bottom.
    #[test]
    fn two_chips() {
        const CHIP: u32 = 0x4_0000;
        let mut mem = vec![0x11u8; 2 * CHIP as usize];
        let mut f = Flash::chips(CHIP);
        // The second chip shows its identification; the first one is
        // still read as memory.
        unlock(&mut f, &mut mem, CHIP, 0x90);
        assert!(f.special_read());
        assert_eq!(f.read(&mem, CHIP, 4), 0x04);
        assert_eq!(f.read(&mem, 0, 4), 0x1111_1111);
        f.write(&mut mem, CHIP, 4, 0xf0);
        assert!(!f.special_read());
        // A sector erase at the bottom of the second chip takes 8 KiB.
        unlock(&mut f, &mut mem, CHIP, 0x80);
        f.write(&mut mem, CHIP + 0x555 * 4, 4, 0xaa);
        f.write(&mut mem, CHIP + 0x2aa * 4, 4, 0x55);
        f.write(&mut mem, CHIP + 0x2000, 4, 0x30);
        assert!(mem[(CHIP + 0x2000) as usize..(CHIP + 0x4000) as usize].iter().all(|b| *b == 0xff));
        assert_eq!(mem[(CHIP + 0x1fff) as usize], 0x11);
        assert_eq!(mem[(CHIP + 0x4000) as usize], 0x11);
        // And at the top of a chip, where the sectors are small again.
        mem[(2 * CHIP - 0x4000) as usize..].iter_mut().for_each(|b| *b = 0x33);
        unlock(&mut f, &mut mem, CHIP, 0x80);
        f.write(&mut mem, CHIP + 0x555 * 4, 4, 0xaa);
        f.write(&mut mem, CHIP + 0x2aa * 4, 4, 0x55);
        f.write(&mut mem, 2 * CHIP - 0x2000, 4, 0x30);
        assert!(mem[(2 * CHIP - 0x2000) as usize..].iter().all(|b| *b == 0xff));
        assert_eq!(mem[(2 * CHIP - 0x2001) as usize], 0x33);
        // What the chip says of itself.
        f.write(&mut mem, CHIP + 0x55 * 4, 4, 0x98);
        assert_eq!([f.read(&mem, CHIP + 0x40, 4), f.read(&mem, CHIP + 0x44, 4), f.read(&mem, CHIP + 0x48, 4)], [0x51, 0x52, 0x59]);
        assert_eq!(f.read(&mem, CHIP + 0x21 * 4, 4), 8);
        f.write(&mut mem, CHIP, 4, 0xf0);
        assert!(!f.special_read());
        // A chip erase takes that chip and leaves the other.
        unlock(&mut f, &mut mem, CHIP, 0x80);
        f.write(&mut mem, CHIP + 0x555 * 4, 4, 0xaa);
        f.write(&mut mem, CHIP + 0x2aa * 4, 4, 0x55);
        f.write(&mut mem, CHIP + 0x555 * 4, 4, 0x10);
        assert!(mem[CHIP as usize..].iter().all(|b| *b == 0xff));
        assert!(mem[..CHIP as usize].iter().all(|b| *b == 0x11));
        let mut f = Flash::chips(CHIP);
        let mut mem = vec![0x11u8; 2 * CHIP as usize];
        // Programming clears bits, in the chip addressed.
        unlock(&mut f, &mut mem, 0, 0xa0);
        f.write(&mut mem, 0x100, 4, 0x0101_0101);
        assert_eq!(&mem[0x100..0x104], &[0x01; 4]);
    }

    /// A sector erase, its unlock cycles given in the bank addressed.
    fn erase(f: &mut Flash, mem: &mut [u8], addr: u32) {
        let b = addr & !0x01ff_ffff;
        f.write(mem, b + 0x555 * 4, 4, 0x00aa_00aa);
        f.write(mem, b + 0x2aa * 4, 4, 0x0055_0055);
        f.write(mem, b + 0x555 * 4, 4, 0x0080_0080);
        f.write(mem, b + 0x555 * 4, 4, 0x00aa_00aa);
        f.write(mem, b + 0x2aa * 4, 4, 0x0055_0055);
        f.write(mem, addr, 4, 0x0030_0030);
    }

    /// The 193: two banks of 32 MiB, each a pair of 16-bit chips, with
    /// sectors of 16 KiB at both ends of a bank and 128 KiB between, as
    /// its stock loader erases them.
    #[test]
    fn paired_banks() {
        const BANK: u32 = 0x0200_0000;
        let mut mem = vec![0x11u8; 2 * BANK as usize];
        let mut f = Flash::paired(BANK);
        let erased = |mem: &[u8]| {
            let first = mem.iter().position(|b| *b == 0xff).unwrap();
            let len = mem[first..].iter().take_while(|b| **b == 0xff).count();
            (first as u32, len as u32)
        };
        for (at, expect) in [
            (0x0040_1000, (0x0040_0000, 0x2_0000)),
            (0x01fe_4100, (0x01fe_4000, 0x4000)),
            (0x0200_0000, (0x0200_0000, 0x4000)),
            (0x0201_c000, (0x0201_c000, 0x4000)),
            (0x0202_0000, (0x0202_0000, 0x2_0000)),
            (0x03ff_401f, (0x03ff_4000, 0x4000)),
        ] {
            mem.iter_mut().for_each(|b| *b = 0x11);
            erase(&mut f, &mut mem, at);
            assert_eq!(erased(&mem), expect, "erase at {:08x}", at);
        }
        // The second bank tells what it is; the first is still memory.
        mem.iter_mut().for_each(|b| *b = 0x11);
        f.write(&mut mem, BANK + 0x555 * 4, 4, 0x00aa_00aa);
        f.write(&mut mem, BANK + 0x2aa * 4, 4, 0x0055_0055);
        f.write(&mut mem, BANK + 0x555 * 4, 4, 0x0090_0090);
        assert_eq!(f.read(&mem, BANK, 4), 0x0001_0001);
        assert_eq!(f.read(&mem, BANK + 4, 4), 0x227e_227e);
        assert_eq!(f.read(&mem, BANK + 6, 2), 0x227e);
        assert_eq!(f.read(&mem, 0, 4), 0x1111_1111);
        f.write(&mut mem, BANK, 4, 0x00f0_00f0);
        f.write(&mut mem, BANK + 0x55 * 4, 4, 0x0098_0098);
        assert_eq!(f.read(&mem, BANK + 0x10 * 4, 4), 0x0051_0051);
        f.write(&mut mem, BANK, 4, 0x00f0_00f0);
        assert!(!f.special_read());
    }
}
