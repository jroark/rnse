//! NOR flash with the AMD/Fujitsu command set on a 32-bit data bus.
//!
//! The 192 has two chips of 16 MiB side by side in the address space
//! (Fujitsu MBM29XL12DF, by the marking on a unit's board), each with
//! its own command state and its own eight small sectors at the bottom:
//! the stock loader erases 8 KiB sectors from 0x01000000 as it does
//! from 0x0000c000.  The 193's 64 MiB are treated as one chip with
//! uniform sectors of 128 KiB, which is how its stock loader erases
//! them (from 0x00400000 up; below that it erases nothing); its parts
//! are not known.
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
    /// Size of every sector, or 0 for small sectors at the bottom of a
    /// chip and 64 KiB above ([`sector`]).
    uniform: u32,
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

/// Erase block containing `addr` in a chip that starts at address 0:
/// eight 8 KiB boot sectors at the bottom, 64 KiB sectors above.
pub fn sector(addr: u32) -> (u32, u32) {
    if addr < 0x1_0000 {
        (addr & !0x1fff, 0x2000)
    } else {
        (addr & !0xffff, 0x1_0000)
    }
}

impl Flash {
    pub fn new() -> Self {
        Flash { state: [State::Read; 4], chip: 0, uniform: 0, erased_sectors: 0, programmed_words: 0, trace: false }
    }

    /// One chip with sectors of `size` bytes throughout.
    pub fn uniform(size: u32) -> Self {
        Flash { uniform: size, ..Self::new() }
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
        self.state.contains(&State::Autoselect)
    }

    pub fn read(&mut self, mem: &[u8], addr: u32, size: u32) -> u32 {
        if self.state[self.chip_of(addr).0] == State::Autoselect {
            // Word address within the chip selects the ID.
            let word = (addr >> 2) & 0xff;
            let v: u32 = match word {
                0 => 0x04,   // manufacturer: Fujitsu
                1 => 0x2253, // device
                _ => 0,
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
                (_, 0xf0) | (_, 0xff) => State::Read,
                _ => state,
            },
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
                        let (start, len) = match self.uniform {
                            0 => sector(addr - base),
                            size => ((addr - base) / size * size, size),
                        };
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
        // A chip erase takes that chip and leaves the other.
        unlock(&mut f, &mut mem, CHIP, 0x80);
        f.write(&mut mem, CHIP + 0x555 * 4, 4, 0xaa);
        f.write(&mut mem, CHIP + 0x2aa * 4, 4, 0x55);
        f.write(&mut mem, CHIP + 0x555 * 4, 4, 0x10);
        assert!(mem[CHIP as usize..].iter().all(|b| *b == 0xff));
        assert!(mem[..CHIP as usize].iter().all(|b| *b == 0x11));
        // Uniform sectors, as on the 193.
        let mut f = Flash::uniform(0x2_0000);
        let mut mem = vec![0x22u8; 0x8_0000];
        unlock(&mut f, &mut mem, 0, 0x80);
        f.write(&mut mem, 0x555 * 4, 4, 0xaa);
        f.write(&mut mem, 0x2aa * 4, 4, 0x55);
        f.write(&mut mem, 0x2_1000, 4, 0x30);
        assert!(mem[0x2_0000..0x4_0000].iter().all(|b| *b == 0xff));
        assert!(mem[..0x2_0000].iter().all(|b| *b == 0x22) && mem[0x4_0000..].iter().all(|b| *b == 0x22));
        let mut f = Flash::chips(CHIP);
        let mut mem = vec![0x11u8; 2 * CHIP as usize];
        // Programming clears bits, in the chip addressed.
        unlock(&mut f, &mut mem, 0, 0xa0);
        f.write(&mut mem, 0x100, 4, 0x0101_0101);
        assert_eq!(&mem[0x100..0x104], &[0x01; 4]);
    }
}
