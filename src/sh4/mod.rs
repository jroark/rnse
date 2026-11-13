//! Hitachi/Renesas SH-4 (SH7750R class) CPU core, big-endian.
//!
//! The core is an interpreter.  External memory is reached through the
//! [`Bus`] trait using 29-bit physical addresses; the P4 control region
//! (0xE000_0000 and up) is handled inside the core by [`onchip`].

pub mod cache;
pub mod cpu;
mod fpu;
mod interp;
pub mod mmu;
pub mod navi;
pub mod onchip;
#[cfg(test)]
mod tests;

pub use cpu::{Cpu, Exc, ExcKind};
pub use onchip::{Irq, OnChip};

/// External (off-chip) bus as seen by the SH-4, addressed physically.
///
/// All multi-byte accesses are big-endian; the bus implementation is
/// responsible for byte order of its backing stores.
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, val: u8);
    fn write16(&mut self, addr: u32, val: u16);
    fn write32(&mut self, addr: u32, val: u32);

    /// Called before each instruction with its address and the current
    /// return address, so that devices can attribute accesses when
    /// logging.  Optional.
    #[inline(always)]
    fn set_pc(&mut self, _pc: u32, _pr: u32) {}

    /// Is this physical address RAM?  Only RAM goes through the cache
    /// model ([`cache`]).
    #[inline(always)]
    fn is_ram(&self, _addr: u32) -> bool {
        false
    }

    /// Level currently encoded on the IRL3-0 pins (0 = none, 15 = highest).
    #[inline(always)]
    fn irl(&self) -> u8 {
        0
    }
}

pub const SR_T: u32 = 1 << 0;
pub const SR_S: u32 = 1 << 1;
pub const SR_Q: u32 = 1 << 8;
pub const SR_M: u32 = 1 << 9;
pub const SR_FD: u32 = 1 << 15;
pub const SR_BL: u32 = 1 << 28;
pub const SR_RB: u32 = 1 << 29;
pub const SR_MD: u32 = 1 << 30;
pub const SR_IMASK_SHIFT: u32 = 4;
pub const SR_MASK: u32 = 0x7000_83f3;

pub const FPSCR_RM: u32 = 3;
pub const FPSCR_DN: u32 = 1 << 18;
pub const FPSCR_PR: u32 = 1 << 19;
pub const FPSCR_SZ: u32 = 1 << 20;
pub const FPSCR_FR: u32 = 1 << 21;
pub const FPSCR_MASK: u32 = 0x003f_ffff;
