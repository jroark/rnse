//! Stand-in for the boot ROM.
//!
//! The first 48 KiB of flash hold a boot ROM that is never part of an
//! update disc, so its code is not available.  This module provides what
//! the rest of the firmware needs from it:
//!
//! * the data other modules read directly (reset pointer, ID header),
//! * the exception/trap tables the OS inherits when it installs its own
//!   vector base,
//! * the system services other modules reach through `TRAPA #0x3d..0x3f`.
//!
//! Instead of SH-4 code, the stub places a reserved opcode (0xFFFD) at
//! each entry point.  Executing it returns control to the emulator, which
//! performs the service in Rust (see [`Hook`]) and resumes the CPU.
//!
//! Everything here is inferred from how the modules on the disc use the
//! boot ROM; see docs/boot-rom.md for the evidence.

use crate::board::Board;

pub const HYPERCALL: u16 = 0xfffd;

/// Flash offsets of the stub's entry points.
pub const RESET_ENTRY: u32 = 0x0800;
pub const CALL_RETURN: u32 = 0x0a00;
pub const UNHANDLED: u32 = 0x0900;
pub const VBR: u32 = 0x1000;
pub const TRAP_STUBS: u32 = 0x2000;

/// What a hypercall address means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    /// Something jumped to the reset entry (flash offset 0 holds its address).
    Reset,
    /// A module entry point called by the stub returned.
    CallReturn,
    /// An exception vector that nobody registered a handler for.
    Unhandled,
    /// General exception while the boot ROM's vector base is active.
    Exception,
    /// Interrupt while the boot ROM's vector base is active.
    Interrupt,
    /// System service `TRAPA #n`, reached through the trap table.
    Trap(u8),
}

pub fn classify(pc: u32) -> Option<Hook> {
    let a = pc & 0x1fff_ffff;
    Some(match a {
        // The 193's loader restarts the unit by jumping to address 0.
        0 | RESET_ENTRY => Hook::Reset,
        CALL_RETURN => Hook::CallReturn,
        UNHANDLED => Hook::Unhandled,
        a if a == VBR + 0x100 => Hook::Exception,
        a if a == VBR + 0x600 => Hook::Interrupt,
        a if (TRAP_STUBS..TRAP_STUBS + 0x800).contains(&a) && a & 7 == 0 => Hook::Trap(((a - TRAP_STUBS) / 8) as u8),
        _ => return None,
    })
}

fn put16(flash: &mut [u8], off: u32, v: u16) {
    flash[off as usize..off as usize + 2].copy_from_slice(&v.to_be_bytes());
}
fn put32(flash: &mut [u8], off: u32, v: u32) {
    flash[off as usize..off as usize + 4].copy_from_slice(&v.to_be_bytes());
}

/// Write the stub into flash.  `project` is the four character hardware
/// project code ("V220").
pub fn install(board: &mut Board, project: &str) {
    let model = board.model;
    let f = &mut board.flash;
    f[..0x4000].iter_mut().for_each(|b| *b = 0xff);

    // Offset 0: address of the reset entry.  The update loader finishes
    // with `jmp @[0]`.
    put32(f, 0, 0xa000_0000 | RESET_ENTRY);
    if model == crate::board::Model::Rnse193 {
        // Here the reset vector is entered as code.
        put16(f, 0, HYPERCALL);
        put16(f, 2, 0x0009);
    }

    // 0x400: module-style header; the loader reads the project code at
    // +6 to decide which software set on a disc fits this hardware.
    let mut hdr = [0u8; 0x80];
    hdr[..6].copy_from_slice(b".BOT\r\n");
    hdr[6..10].copy_from_slice(&project.as_bytes()[..4]);
    hdr[10..14].copy_from_slice(b"0000");
    hdr[14..16].copy_from_slice(b"\r\n");
    hdr[0x30..0x3f].copy_from_slice(b"rnse emulator  ");
    f[0x400..0x480].copy_from_slice(&hdr);

    for at in [RESET_ENTRY, CALL_RETURN, UNHANDLED] {
        put16(f, at, HYPERCALL);
        put16(f, at + 2, 0x000b); // rts
        put16(f, at + 4, 0x0009); // nop
    }

    // Vector base area, in the layout the OS copies from when it takes
    // over (see the routine at the start of the OS module):
    //   +0x000  60 exception/interrupt handlers, indexed by event code / 0x20
    //   +0x100  general exception entry
    //   +0x400  60 interrupt mask bytes, same index
    //   +0x43c  handlers for TRAPA 15..63
    //   +0x600  interrupt entry
    for i in 0..60 {
        put32(f, VBR + i * 4, 0x8000_0000 | UNHANDLED);
    }
    put16(f, VBR + 0x100, HYPERCALL);
    put16(f, VBR + 0x600, HYPERCALL);
    for i in 0..60u32 {
        // IRL levels 15..1 (event codes 0x200..0x3c0) mask themselves.
        let mask = if (16..31).contains(&i) { ((31 - i) << 4) as u8 } else { 0 };
        f[(VBR + 0x400 + i) as usize] = mask;
    }
    for n in 15..64u32 {
        put32(f, VBR + 0x400 + n * 4, 0x8000_0000 | (TRAP_STUBS + n * 8));
    }
    if model == crate::board::Model::Rnse193 {
        // The 193's OS expects a different arrangement (see the routine
        // at the start of its OS module): one table of 64 handlers at
        // +0, the first 16 for exception codes 0x000-0x1e0 and all of
        // them for TRAPA 0-63, and 125 interrupt mask bytes at +0x780.
        for n in 16..64u32 {
            put32(f, VBR + n * 4, 0x8000_0000 | (TRAP_STUBS + n * 8));
        }
        for i in 0..125u32 {
            let mask = if (16..31).contains(&i) { ((31 - i) << 4) as u8 } else { 0 };
            f[(VBR + 0x780 + i) as usize] = mask;
        }
    }
    for n in 0..256u32 {
        let at = TRAP_STUBS + n * 8;
        put16(f, at, HYPERCALL);
        put16(f, at + 2, 0x000b); // rts
        put16(f, at + 4, 0x0009); // nop
        put16(f, at + 6, 0x0009);
    }
}
