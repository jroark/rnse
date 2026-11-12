//! Unit tests for instructions that the QEMU comparison (tools/cpufuzz.py)
//! cannot cover because QEMU 8.2 implements them incorrectly, plus a few
//! end-to-end checks of exception handling.  Expected values follow the
//! SH-4 software manual.

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
        let a = a as usize;
        u32::from_be_bytes([self.0[a], self.0[a + 1], self.0[a + 2], self.0[a + 3]])
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

/// Run `code` from address 0x1000 until it has been executed once.
fn run(code: &[u16], setup: impl FnOnce(&mut Cpu, &mut Ram)) -> (Cpu, Ram) {
    let mut ram = Ram(vec![0; 0x10000]);
    for (i, op) in code.iter().enumerate() {
        ram.write16(0x1000 + i as u32 * 2, *op);
    }
    let mut cpu = Cpu::new();
    cpu.set_sr(SR_MD);
    cpu.pc = 0x8000_1000;
    setup(&mut cpu, &mut ram);
    let end = 0x8000_1000 + code.len() as u32 * 2;
    let mut guard = 0;
    while cpu.pc != end {
        cpu.step(&mut ram);
        guard += 1;
        assert!(guard < 10_000, "runaway test program at {:08x}", cpu.pc);
        assert!(cpu.fault.is_none(), "{:?}", cpu.fault);
    }
    (cpu, ram)
}

#[test]
fn addv_writes_rn_and_flags_overflow() {
    // addv r1,r2
    let (c, _) = run(&[0x321f], |c, _| {
        c.r[1] = 1;
        c.r[2] = 0x7fff_ffff;
    });
    assert_eq!(c.r[2], 0x8000_0000);
    assert_eq!(c.r[1], 1);
    assert!(c.t());
    let (c, _) = run(&[0x321f], |c, _| {
        c.r[1] = 2;
        c.r[2] = 3;
    });
    assert_eq!(c.r[2], 5);
    assert!(!c.t());
}

#[test]
fn subv_writes_rn_and_flags_overflow() {
    // subv r1,r2
    let (c, _) = run(&[0x321b], |c, _| {
        c.r[1] = 1;
        c.r[2] = 0x8000_0000;
    });
    assert_eq!(c.r[2], 0x7fff_ffff);
    assert!(c.t());
    let (c, _) = run(&[0x321b], |c, _| {
        c.r[1] = 3;
        c.r[2] = 10;
    });
    assert_eq!(c.r[2], 7);
    assert!(!c.t());
}

#[test]
fn rotl_rotr_set_t_from_the_bit_rotated_out() {
    let (c, _) = run(&[0x4104], |c, _| c.r[1] = 0x8000_0001); // rotl r1
    assert_eq!(c.r[1], 0x0000_0003);
    assert!(c.t());
    let (c, _) = run(&[0x4104], |c, _| c.r[1] = 0x4000_0000);
    assert_eq!(c.r[1], 0x8000_0000);
    assert!(!c.t());
    let (c, _) = run(&[0x4105], |c, _| c.r[1] = 0x0000_0001); // rotr r1
    assert_eq!(c.r[1], 0x8000_0000);
    assert!(c.t());
    let (c, _) = run(&[0x4105], |c, _| c.r[1] = 0x0000_0002);
    assert_eq!(c.r[1], 0x0000_0001);
    assert!(!c.t());
}

/// The standard 32/32 unsigned division sequence built from DIV0U and
/// 32 ROTCL/DIV1 steps must agree with host division.
#[test]
fn div1_unsigned_division() {
    let mut code = vec![0x0019]; // div0u
    for _ in 0..32 {
        code.push(0x4024); // rotcl r0   (dividend low, quotient shifts in)
        code.push(0x3214); // div1 r1,r2 (r2 = remainder / dividend high)
    }
    code.push(0x4024); // rotcl r0
    let cases = [(100u32, 7u32), (0xffff_ffff, 3), (0x1234_5678, 0x1000), (5, 9), (0x8000_0000, 1), (0xdead_beef, 0xbeef), (1, 1)];
    for (n, d) in cases {
        let (c, _) = run(&code, |c, _| {
            c.r[0] = n;
            c.r[1] = d;
            c.r[2] = 0;
        });
        assert_eq!(c.r[0], n / d, "{} / {}", n, d);
    }
}

/// A DIV1 step with a zero divisor subtracts nothing: no borrow, Q stays
/// clear and T is set (manual pseudo-code; QEMU 8.2 gets this wrong).
#[test]
fn div1_zero_divisor_step() {
    let (c, _) = run(&[0x3944], |c, _| {
        c.r[9] = 0x0001_0000;
        c.r[4] = 0;
    }); // div1 r4,r9
    assert_eq!(c.r[9], 0x0002_0000);
    assert_eq!(c.sr & SR_Q, 0);
    assert!(c.t());
}

#[test]
fn mac_w_accumulates_signed_words() {
    // mac.w @r1+,@r2+
    let (c, _) = run(&[0x421f], |c, m| {
        c.r[1] = 0x8000_2000;
        c.r[2] = 0x8000_2004;
        m.write16(0x2000, 0xfffe); // -2
        m.write16(0x2004, 0x0003);
        c.mach = 0;
        c.macl = 10;
    });
    assert_eq!(c.macl, 4);
    assert_eq!(c.mach, 0);
    assert_eq!(c.r[1], 0x8000_2002);
    assert_eq!(c.r[2], 0x8000_2006);
}

#[test]
fn mac_w_saturates_to_32_bits_when_s_is_set() {
    let (c, _) = run(&[0x421f], |c, m| {
        c.sr |= SR_S;
        c.r[1] = 0x8000_2000;
        c.r[2] = 0x8000_2004;
        m.write16(0x2000, 0x7fff);
        m.write16(0x2004, 0x7fff);
        c.macl = 0x7fff_0000;
    });
    assert_eq!(c.macl, 0x7fff_ffff);
    assert_eq!(c.mach & 1, 1);
}

#[test]
fn mac_l_accumulates_64_bits() {
    // mac.l @r1+,@r2+
    let (c, _) = run(&[0x021f], |c, m| {
        c.r[1] = 0x8000_2000;
        c.r[2] = 0x8000_2004;
        m.write32(0x2000, 0xffff_ffff); // -1
        m.write32(0x2004, 0x0000_0005);
        c.mach = 0;
        c.macl = 7;
    });
    assert_eq!((c.mach, c.macl), (0, 2));
}

#[test]
fn mac_l_saturates_to_48_bits_when_s_is_set() {
    let (c, _) = run(&[0x021f], |c, m| {
        c.sr |= SR_S;
        c.r[1] = 0x8000_2000;
        c.r[2] = 0x8000_2004;
        m.write32(0x2000, 0x7fff_ffff);
        m.write32(0x2004, 0x7fff_ffff);
        c.mach = 0x0000_7fff;
        c.macl = 0xffff_0000;
    });
    assert_eq!((c.mach, c.macl), (0x0000_7fff, 0xffff_ffff));
}

#[test]
fn fipr_and_ftrv() {
    // fipr fv4,fv0 : fr3 = fv0 . fv4
    let (c, _) = run(&[0xf1ed], |c, _| {
        c.fpscr = 0;
        for i in 0..4 {
            c.fr[i] = (i as f32 + 1.0).to_bits();
            c.fr[4 + i] = 2.0f32.to_bits();
        }
    });
    assert_eq!(f32::from_bits(c.fr[3]), 20.0);

    // ftrv xmtrx,fv0 with a matrix that scales by 3
    let (c, _) = run(&[0xf1fd], |c, _| {
        c.fpscr = 0;
        for i in 0..4 {
            c.fr[i] = (i as f32 + 1.0).to_bits();
            c.fr[16 + i * 5] = 3.0f32.to_bits();
        }
    });
    let v: Vec<f32> = (0..4).map(|i| f32::from_bits(c.fr[i])).collect();
    assert_eq!(v, [3.0, 6.0, 9.0, 12.0]);
}

#[test]
fn round_to_zero_mode() {
    // fdiv fr1,fr0 : 1/3 rounds down in RZ, up to nearest in RN
    let third_rn = (1.0f32 / 3.0).to_bits();
    let (c, _) = run(&[0xf013], |c, _| {
        c.fpscr = 0;
        c.fr[0] = 1.0f32.to_bits();
        c.fr[1] = 3.0f32.to_bits();
    });
    assert_eq!(c.fr[0], third_rn);
    let (c, _) = run(&[0xf013], |c, _| {
        c.fpscr = 1;
        c.fr[0] = 2.0f32.to_bits();
        c.fr[1] = 3.0f32.to_bits();
    });
    assert_eq!(c.fr[0], (2.0f32 / 3.0).to_bits() - 1);
}

#[test]
fn denormals_read_as_zero_with_dn_set() {
    // fadd fr1,fr0
    let (c, _) = run(&[0xf010], |c, _| {
        c.fpscr = FPSCR_DN;
        c.fr[0] = 1; // smallest denormal
        c.fr[1] = 1;
    });
    assert_eq!(c.fr[0], 0);
}

#[test]
fn trapa_enters_the_handler_and_rte_returns() {
    // 0x1000: trapa #0x21 ; mov #5,r2
    // handler at VBR+0x100: stc spc,r1(bank1) ... rte ; nop
    let mut ram = Ram(vec![0; 0x10000]);
    ram.write16(0x1000, 0xc321);
    ram.write16(0x1002, 0xe205);
    ram.write16(0x3100, 0xe77b); // mov #123,r7   (bank 1 register)
    ram.write16(0x3102, 0x002b); // rte
    ram.write16(0x3104, 0x0009); // nop
    let mut cpu = Cpu::new();
    cpu.set_sr(SR_MD);
    cpu.vbr = 0x8000_3000;
    cpu.pc = 0x8000_1000;
    cpu.r[7] = 1;
    cpu.step(&mut ram);
    assert_eq!(cpu.pc, 0x8000_3100);
    assert_eq!(cpu.io.tra, 0x21 << 2);
    assert_eq!(cpu.io.expevt, 0x160);
    assert_eq!(cpu.spc, 0x8000_1002);
    assert_ne!(cpu.sr & SR_BL, 0);
    cpu.step(&mut ram); // mov #123,r7 in bank 1
    cpu.step(&mut ram); // rte + delay slot
    assert_eq!(cpu.pc, 0x8000_1002);
    assert_eq!(cpu.sr, SR_MD);
    assert_eq!(cpu.r[7], 1, "bank 0 register must be untouched by the handler");
    cpu.step(&mut ram);
    assert_eq!(cpu.r[2], 5);
}

#[test]
fn exception_in_delay_slot_restarts_at_the_branch() {
    // 0x1000: bra +2 ; mov.l @r1,r0 (unaligned -> address error)
    let mut ram = Ram(vec![0; 0x10000]);
    ram.write16(0x1000, 0xa002);
    ram.write16(0x1002, 0x6012);
    let mut cpu = Cpu::new();
    cpu.set_sr(SR_MD);
    cpu.vbr = 0x8000_3000;
    cpu.pc = 0x8000_1000;
    cpu.r[1] = 0x8000_2001;
    cpu.step(&mut ram);
    assert_eq!(cpu.io.expevt, 0x0e0);
    assert_eq!(cpu.spc, 0x8000_1000);
    assert_eq!(cpu.pc, 0x8000_3100);
    assert_eq!(cpu.mmu.tea, 0x8000_2001);
}

#[test]
fn user_mode_cannot_touch_privileged_space() {
    let mut ram = Ram(vec![0; 0x10000]);
    ram.write16(0x1000, 0x6012); // mov.l @r1,r0
    let mut cpu = Cpu::new();
    cpu.set_sr(0);
    cpu.vbr = 0x8000_3000;
    cpu.pc = 0x0000_1000;
    cpu.r[1] = 0x8000_2000;
    cpu.step(&mut ram);
    assert_eq!(cpu.io.expevt, 0x0e0);
    assert_ne!(cpu.sr & SR_MD, 0);
}

#[test]
fn tmu_underflow_raises_its_interrupt() {
    let mut ram = Ram(vec![0; 0x10000]);
    for i in 0..0x800u32 {
        ram.write16(0x1000 + i * 2, 0x0009); // nops
    }
    let mut cpu = Cpu::new();
    cpu.set_sr(SR_MD);
    cpu.vbr = 0x8000_3000;
    cpu.pc = 0x8000_1000;
    let now = cpu.cycles;
    cpu.io.write(&mut ram, 0xffd0_0004, 2, 0x7000, now, 0); // IPRA: TMU0 level 7
    cpu.io.write(&mut ram, 0xffd8_0008, 4, 9, now, 0); // TCOR0
    cpu.io.write(&mut ram, 0xffd8_000c, 4, 9, now, 0); // TCNT0
    cpu.io.write(&mut ram, 0xffd8_0010, 2, 0x20, now, 0); // TCR0: UNIE, Pclk/4
    cpu.io.write(&mut ram, 0xffd8_0004, 1, 1, now, 0); // TSTR
    cpu.run(&mut ram, 400);
    assert_eq!(cpu.io.intevt, 0x400);
    assert_eq!(cpu.irq_counts[0x400 >> 5], 1);
}

#[test]
fn delay_loop_fast_path_keeps_register_and_time_exact() {
    // mov.l count,r4 is done by setup; 1: dt r4 ; bf 1b ; mov #7,r1
    let code = [0x4410, 0x8bfd, 0xe107];
    let (c, _) = run(&code, |c, _| c.r[4] = 100_000);
    assert_eq!(c.r[4], 0);
    assert!(c.t());
    assert_eq!(c.r[1], 7);
    // The same loop one step at a time takes 2 instructions per count.
    // `run` here only calls step(), which does not advance `cycles`
    // itself, so the counter holds exactly what the fast path added.
    assert!(c.cycles > 0 && c.cycles < 200_000);

    // A timer interrupt due in the middle of the loop still arrives on
    // time: run with TMU0 set to underflow after 400 cycles.
    let mut ram = Ram(vec![0; 0x10000]);
    for (i, op) in code.iter().enumerate() {
        ram.write16(0x1000 + i as u32 * 2, *op);
    }
    ram.write16(0x3600, 0x0009);
    let mut cpu = Cpu::new();
    cpu.set_sr(SR_MD);
    cpu.vbr = 0x8000_3000;
    cpu.pc = 0x8000_1000;
    cpu.r[4] = 1_000_000;
    cpu.io.write(&mut ram, 0xffd0_0004, 2, 0x7000, 0, 0);
    cpu.io.write(&mut ram, 0xffd8_0008, 4, 24, 0, 0);
    cpu.io.write(&mut ram, 0xffd8_000c, 4, 24, 0, 0);
    cpu.io.write(&mut ram, 0xffd8_0010, 2, 0x20, 0, 0);
    cpu.io.write(&mut ram, 0xffd8_0004, 1, 1, 0, 0);
    while cpu.io.intevt == 0 {
        cpu.run(&mut ram, 16);
        assert!(cpu.cycles < 2000, "interrupt was skipped over");
    }
    // 25 timer ticks of 16 cycles each = 400 cycles.
    assert!((400..=404).contains(&cpu.cycles), "interrupt at cycle {}", cpu.cycles);
    // Loop iterations done so far: two cycles each.
    // (The handler runs on register bank 1; the loop counter is in bank 0.)
    let done = 1_000_000 - cpu.r_bank[4] as u64;
    assert!((198..=202).contains(&done), "{} iterations in 400 cycles", done);
}
