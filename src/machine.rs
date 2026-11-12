//! Ties the CPU and the board together and knows how to start firmware.

use crate::board::Board;
use crate::fw::{Module, Variant};
use crate::sh4::cpu::Stop;
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
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    pub fn new() -> Self {
        Machine { cpu: Cpu::new(), board: Board::new(), console: Vec::new(), echo_console: true, frame_cycles: 2_200_000, next_frame: 2_200_000 }
    }

    /// Place every module of a variant at its load address, the way the
    /// update loader leaves the flash.
    pub fn install(&mut self, v: &Variant) {
        for m in &v.modules {
            if m.kind != "LDR" {
                self.board.load(m.phys_addr(), &m.data[..m.size as usize]);
            }
        }
    }

    /// Write the parts of the boot ROM that other software looks at.
    ///
    /// The real boot block (flash 0x0000-0xc0ff on V220 hardware) is not
    /// part of an update disc.  What is known about it comes from how the
    /// modules on the disc use it:
    ///
    /// * 0x0400 holds a module header like the ones in `fw.rs`; the
    ///   loader reads the four characters at 0x0406 as the hardware's
    ///   project code and only installs software sets with that name.
    pub fn install_boot_stub(&mut self, project: &str) {
        let mut hdr = [0u8; 0x80];
        hdr[..6].copy_from_slice(b".BOT\r\n");
        hdr[6..10].copy_from_slice(&project.as_bytes()[..4]);
        hdr[10..14].copy_from_slice(b"0000");
        hdr[14..16].copy_from_slice(b"\r\n");
        hdr[0x30..0x3f].copy_from_slice(b"rnse emulator  ");
        self.board.load(0x400, &hdr);
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
            if self.cpu.cycles >= self.next_frame {
                self.next_frame += self.frame_cycles;
                self.board.vsync();
            }
            if stop != Stop::Budget {
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
