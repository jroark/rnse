//! NOR flash with the AMD/Fujitsu command set on a 32-bit data bus.
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
    state: State,
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

/// Erase block containing `addr`: eight 8 KiB boot sectors at the bottom,
/// 64 KiB sectors above.
pub fn sector(addr: u32) -> (u32, u32) {
    if addr < 0x1_0000 {
        (addr & !0x1fff, 0x2000)
    } else {
        (addr & !0xffff, 0x1_0000)
    }
}

impl Flash {
    pub fn new() -> Self {
        Flash { state: State::Read, erased_sectors: 0, programmed_words: 0, trace: false }
    }

    /// True when reads must not come straight from the array.
    pub fn special_read(&self) -> bool {
        self.state == State::Autoselect
    }

    pub fn read(&mut self, mem: &[u8], addr: u32, size: u32) -> u32 {
        if self.state == State::Autoselect {
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
        if self.trace {
            eprintln!("[flash] {:?} W {:08x}.{} <= {:x}", self.state, addr, size, v);
        }
        self.state = match self.state {
            State::Read | State::Autoselect => match (key, cmd) {
                (0x555, 0xaa) => State::Unlock1,
                (_, 0xf0) | (_, 0xff) => State::Read,
                _ => self.state,
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
                        let (base, len) = sector(addr);
                        let end = ((base + len) as usize).min(mem.len());
                        mem[base as usize..end].iter_mut().for_each(|b| *b = 0xff);
                        self.erased_sectors += 1;
                    }
                    0x10 => {
                        mem.iter_mut().for_each(|b| *b = 0xff);
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
                if self.state == State::Program {
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
