//! Bare CPU-core runner used by the differential tests in `tools/cpufuzz.py`.
//!
//! Loads a flat big-endian image at a fixed address, runs it in user mode
//! and services the two Linux system calls the test programs use (write
//! and exit), so that the same image can be run under `qemu-sh4eb` and
//! the outputs compared byte for byte.

use rnse::sh4::{Bus, Cpu};
use std::io::Write;

struct Ram {
    base: u32,
    mem: Vec<u8>,
}

impl Ram {
    fn idx(&self, a: u32) -> usize {
        let o = a.wrapping_sub(self.base) as usize;
        if o >= self.mem.len() {
            panic!("access outside test image: {:08x}", a);
        }
        o
    }
}

impl Bus for Ram {
    fn read8(&mut self, a: u32) -> u8 {
        self.mem[self.idx(a)]
    }
    fn read16(&mut self, a: u32) -> u16 {
        let i = self.idx(a);
        u16::from_be_bytes([self.mem[i], self.mem[i + 1]])
    }
    fn read32(&mut self, a: u32) -> u32 {
        let i = self.idx(a);
        u32::from_be_bytes([self.mem[i], self.mem[i + 1], self.mem[i + 2], self.mem[i + 3]])
    }
    fn write8(&mut self, a: u32, v: u8) {
        let i = self.idx(a);
        self.mem[i] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        let i = self.idx(a);
        self.mem[i..i + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn write32(&mut self, a: u32, v: u32) {
        let i = self.idx(a);
        self.mem[i..i + 4].copy_from_slice(&v.to_be_bytes());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: sh4run <image.bin> <load-addr-hex> <mem-size-hex> [max-insns]");
        std::process::exit(2);
    }
    let img = std::fs::read(&args[1]).expect("read image");
    let base = u32::from_str_radix(&args[2], 16).expect("load addr");
    let size = usize::from_str_radix(&args[3], 16).expect("mem size");
    let max: u64 = args.get(4).map(|s| s.parse().unwrap()).unwrap_or(10_000_000);
    let mut ram = Ram { base, mem: vec![0; size] };
    ram.mem[..img.len()].copy_from_slice(&img);

    let mut cpu = Cpu::new();
    cpu.set_sr(0); // user mode, like qemu-user
    cpu.pc = base;
    cpu.fpscr = 0x0008_0000;
    let trace = std::env::var_os("SH4RUN_TRACE").is_some();
    let out = std::io::stdout();
    let mut out = out.lock();
    while cpu.cycles < max {
        let pc = cpu.pc;
        let op = ram.read16(pc & 0x1fff_ffff);
        if op & 0xff00 == 0xc300 {
            // trapa: Linux syscall, number in r3
            match cpu.r[3] {
                4 => {
                    let (p, n) = (cpu.r[5], cpu.r[6]);
                    let i = ram.idx(p);
                    out.write_all(&ram.mem[i..i + n as usize]).unwrap();
                    cpu.r[0] = n;
                }
                1 => {
                    out.flush().unwrap();
                    std::process::exit(cpu.r[4] as i32);
                }
                n => panic!("unexpected syscall {}", n),
            }
            cpu.pc = pc.wrapping_add(2);
            continue;
        }
        if trace {
            let mut l = format!("pc={:08x} sr={:08x} pr={:08x} gbr={:08x} mach={:08x} macl={:08x} fpul={:08x}", pc, cpu.sr, cpu.pr, cpu.gbr, cpu.mach, cpu.macl, cpu.fpul);
            for i in 0..16 {
                l += &format!(" r{}={:08x}", i, cpu.r[i]);
            }
            eprintln!("{}", l);
        }
        cpu.step(&mut ram);
        cpu.cycles += 1;
        if let Some(f) = &cpu.fault {
            eprintln!("fault: {}\n{}", f, cpu.dump());
            std::process::exit(3);
        }
        if cpu.io.expevt != 0 {
            eprintln!("exception {:03x} at {:08x}\n{}", cpu.io.expevt, cpu.spc, cpu.dump());
            std::process::exit(4);
        }
    }
    eprintln!("instruction limit reached\n{}", cpu.dump());
    std::process::exit(5);
}
