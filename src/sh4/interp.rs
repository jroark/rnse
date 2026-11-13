//! Instruction interpreter for the integer and system instruction set.
//! FPU instructions (0xFxxx) live in `fpu.rs`.

use super::cpu::{Cpu, Exc, ExcKind};
use super::*;

type R = Result<(), Exc>;

#[inline(always)]
fn sext8(v: u16) -> u32 {
    (v as u8) as i8 as i32 as u32
}
#[inline(always)]
fn sext12(v: u16) -> u32 {
    (((v & 0xfff) << 4) as i16 >> 4) as i32 as u32
}

impl Cpu {
    #[inline(always)]
    fn illegal(&self) -> Exc {
        Exc::new(if self.in_delay { ExcKind::SlotIllegalInsn } else { ExcKind::IllegalInsn })
    }

    #[inline(always)]
    fn need_priv(&self) -> R {
        if self.sr & SR_MD == 0 {
            Err(self.illegal())
        } else {
            Ok(())
        }
    }

    /// Execute the instruction in the delay slot and then continue at
    /// `target`.
    #[inline(always)]
    fn delay<B: Bus>(&mut self, bus: &mut B, target: u32) -> R {
        if self.in_delay {
            return Err(Exc::new(ExcKind::SlotIllegalInsn));
        }
        let slot_pc = self.pc;
        self.insn_pc = slot_pc;
        self.in_delay = true;
        let op = self.fetch(bus, slot_pc)?;
        self.pc = slot_pc.wrapping_add(2);
        self.exec_slot(bus, op)?;
        self.in_delay = false;
        self.pc = target;
        self.cycles += 1;
        Ok(())
    }

    #[inline(always)]
    fn no_slot(&self) -> R {
        if self.in_delay {
            Err(Exc::new(ExcKind::SlotIllegalInsn))
        } else {
            Ok(())
        }
    }

    /// Fast path for the classic busy-wait `1: dt Rn ; bf 1b`.
    ///
    /// Called when the BF is taken back to the instruction before it.  If
    /// that is a DT, the remaining iterations do nothing but count Rn
    /// down, so most of them are applied at once: Rn and the cycle
    /// counter move exactly as if the loop had run, stopping short of the
    /// next peripheral event so interrupt timing is unchanged.
    #[inline]
    fn skip_delay_loop<B: Bus>(&mut self, bus: &mut B, target: u32) {
        let Ok(prev) = self.fetch(bus, target) else { return };
        if prev & 0xf0ff != 0x4010 {
            return;
        }
        let n = ((prev >> 8) & 0xf) as usize;
        let left = self.r[n] as u64;
        if left < 4 {
            return;
        }
        let budget = self.io.next_event.min(self.run_end).saturating_sub(self.cycles) / 2;
        let k = (left - 2).min(budget.saturating_sub(1));
        if k > 0 {
            self.r[n] -= k as u32;
            self.cycles += 2 * k;
        }
    }

    /// The instruction of a delay slot: a call of its own, since the
    /// executor is otherwise inlined into the run loop and a branch
    /// executes its slot from inside it.
    #[inline(never)]
    fn exec_slot<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        self.exec(bus, op)
    }

    /// Execute one instruction.  Inlined into its two callers, the run
    /// loop and [`Self::exec_slot`]: a call per instruction, with this
    /// function's large frame, cost more than most instructions do.
    #[inline(always)]
    pub(super) fn exec<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let n = ((op >> 8) & 0xf) as usize;
        let m = ((op >> 4) & 0xf) as usize;
        match op >> 12 {
            0x0 => match op & 0xf {
                0x2 => {
                    // STC xx,Rn
                    let v = match m {
                        0 => {
                            self.need_priv()?;
                            self.sr
                        }
                        1 => self.gbr,
                        2 => {
                            self.need_priv()?;
                            self.vbr
                        }
                        3 => {
                            self.need_priv()?;
                            self.ssr
                        }
                        4 => {
                            self.need_priv()?;
                            self.spc
                        }
                        8..=15 => {
                            self.need_priv()?;
                            *self.bank_reg(m)
                        }
                        _ => return Err(self.illegal()),
                    };
                    self.r[n] = v;
                }
                0x3 => match m {
                    0 => {
                        // BSRF Rn
                        self.no_slot()?;
                        let target = self.pc.wrapping_add(2).wrapping_add(self.r[n]);
                        self.pr = self.pc.wrapping_add(2);
                        self.note_call(target);
                        self.delay(bus, target)?;
                    }
                    2 => {
                        // BRAF Rn
                        self.no_slot()?;
                        let target = self.pc.wrapping_add(2).wrapping_add(self.r[n]);
                        self.delay(bus, target)?;
                    }
                    8 => {
                        // PREF @Rn
                        let a = self.r[n];
                        if (0xe000_0000..0xe400_0000).contains(&a) {
                            self.sq_flush(bus, a)?;
                        }
                    }
                    // OCBI / OCBP / OCBWB: nothing to do unless the
                    // caches are modelled.
                    9 | 0xa | 0xb => self.cache_op(bus, self.r[n], m as u16)?,
                    0xc => {
                        // MOVCA.L R0,@Rn
                        self.movca(bus, self.r[n], self.r[0])?;
                    }
                    _ => return Err(self.illegal()),
                },
                0x4 => self.write8(bus, self.r[n].wrapping_add(self.r[0]), self.r[m] as u8)?,
                0x5 => self.write16(bus, self.r[n].wrapping_add(self.r[0]), self.r[m] as u16)?,
                0x6 => self.write32(bus, self.r[n].wrapping_add(self.r[0]), self.r[m])?,
                0x7 => {
                    // MUL.L
                    self.macl = self.r[n].wrapping_mul(self.r[m]);
                }
                0x8 => match m {
                    0 => self.set_t(false),
                    1 => self.set_t(true),
                    2 => {
                        self.mach = 0;
                        self.macl = 0;
                    }
                    3 => {
                        self.need_priv()?;
                        self.mmu.ldtlb();
                        self.fetch_page = 1;
                    }
                    4 => self.sr &= !SR_S,
                    5 => self.sr |= SR_S,
                    _ => return Err(self.illegal()),
                },
                0x9 => match m {
                    0 => {} // NOP
                    1 => {
                        // DIV0U
                        self.sr &= !(SR_M | SR_Q | SR_T);
                    }
                    2 => self.r[n] = self.sr & 1, // MOVT
                    _ => return Err(self.illegal()),
                },
                0xa => {
                    let v = match m {
                        0 => self.mach,
                        1 => self.macl,
                        2 => self.pr,
                        3 => {
                            self.need_priv()?;
                            self.sgr
                        }
                        5 => {
                            self.fpu_check()?;
                            self.fpul
                        }
                        6 => {
                            self.fpu_check()?;
                            self.fpscr
                        }
                        0xf => {
                            self.need_priv()?;
                            self.dbr
                        }
                        _ => return Err(self.illegal()),
                    };
                    self.r[n] = v;
                }
                0xb => match m {
                    0 => {
                        // RTS
                        self.no_slot()?;
                        let target = self.pr;
                        self.delay(bus, target)?;
                    }
                    1 => {
                        // SLEEP
                        self.need_priv()?;
                        self.sleeping = true;
                        self.attention = true;
                    }
                    2 => {
                        // RTE
                        self.need_priv()?;
                        self.no_slot()?;
                        let target = self.spc;
                        let ssr = self.ssr;
                        self.set_sr(ssr);
                        self.fetch_page = 1;
                        self.rte_slot = true;
                        let r = self.delay(bus, target);
                        self.rte_slot = false;
                        self.fetch_page = 1;
                        r?;
                    }
                    _ => return Err(self.illegal()),
                },
                0xc => {
                    let v = self.read8(bus, self.r[m].wrapping_add(self.r[0]))?;
                    self.r[n] = v as i8 as i32 as u32;
                }
                0xd => {
                    let v = self.read16(bus, self.r[m].wrapping_add(self.r[0]))?;
                    self.r[n] = v as i16 as i32 as u32;
                }
                0xe => {
                    self.r[n] = self.read32(bus, self.r[m].wrapping_add(self.r[0]))?;
                }
                0xf => {
                    // MAC.L @Rm+,@Rn+
                    let a = self.read32(bus, self.r[n])? as i32 as i64;
                    self.r[n] = self.r[n].wrapping_add(4);
                    let b = self.read32(bus, self.r[m])? as i32 as i64;
                    self.r[m] = self.r[m].wrapping_add(4);
                    let mac = ((self.mach as u64) << 32 | self.macl as u64) as i64;
                    let mut res = mac.wrapping_add(a.wrapping_mul(b));
                    if self.sr & SR_S != 0 {
                        // 48-bit saturation
                        let prod = a.wrapping_mul(b);
                        let sum = (mac as i128) + (prod as i128);
                        let max = 0x0000_7fff_ffff_ffffi128;
                        let min = -0x0000_8000_0000_0000i128;
                        res = sum.clamp(min, max) as i64;
                    }
                    self.mach = (res as u64 >> 32) as u32;
                    self.macl = res as u32;
                }
                _ => return Err(self.illegal()),
            },

            0x1 => {
                // MOV.L Rm,@(disp,Rn)
                let a = self.r[n].wrapping_add(((op & 0xf) as u32) << 2);
                self.write32(bus, a, self.r[m])?;
            }

            0x2 => match op & 0xf {
                0x0 => self.write8(bus, self.r[n], self.r[m] as u8)?,
                0x1 => self.write16(bus, self.r[n], self.r[m] as u16)?,
                0x2 => self.write32(bus, self.r[n], self.r[m])?,
                0x4 => {
                    let a = self.r[n].wrapping_sub(1);
                    self.write8(bus, a, self.r[m] as u8)?;
                    self.r[n] = a;
                }
                0x5 => {
                    let a = self.r[n].wrapping_sub(2);
                    self.write16(bus, a, self.r[m] as u16)?;
                    self.r[n] = a;
                }
                0x6 => {
                    let a = self.r[n].wrapping_sub(4);
                    self.write32(bus, a, self.r[m])?;
                    self.r[n] = a;
                }
                0x7 => {
                    // DIV0S
                    let q = self.r[n] >> 31;
                    let mm = self.r[m] >> 31;
                    self.sr = (self.sr & !(SR_Q | SR_M | SR_T)) | (q << 8) | (mm << 9) | (q ^ mm);
                }
                0x8 => self.set_t(self.r[n] & self.r[m] == 0),
                0x9 => self.r[n] &= self.r[m],
                0xa => self.r[n] ^= self.r[m],
                0xb => self.r[n] |= self.r[m],
                0xc => {
                    // CMP/STR
                    let x = self.r[n] ^ self.r[m];
                    let t = (x & 0xff00_0000 == 0) || (x & 0x00ff_0000 == 0) || (x & 0xff00 == 0) || (x & 0xff == 0);
                    self.set_t(t);
                }
                0xd => self.r[n] = (self.r[m] << 16) | (self.r[n] >> 16), // XTRCT
                0xe => self.macl = (self.r[n] & 0xffff) * (self.r[m] & 0xffff), // MULU.W
                0xf => {
                    self.macl = ((self.r[n] as i16 as i32).wrapping_mul(self.r[m] as i16 as i32)) as u32
                }
                _ => return Err(self.illegal()),
            },

            0x3 => match op & 0xf {
                0x0 => self.set_t(self.r[n] == self.r[m]),
                0x2 => self.set_t(self.r[n] >= self.r[m]),
                0x3 => self.set_t(self.r[n] as i32 >= self.r[m] as i32),
                0x4 => {
                    // DIV1 Rm,Rn
                    let old_q = self.sr & SR_Q != 0;
                    let mflag = self.sr & SR_M != 0;
                    let mut q = self.r[n] & 0x8000_0000 != 0;
                    let rm = self.r[m];
                    let mut rn = (self.r[n] << 1) | (self.sr & SR_T);
                    let tmp0 = rn;
                    // Per the manual: subtract when Q == M, add otherwise;
                    // the new Q is (shifted-out bit) ^ M ^ (borrow or carry).
                    let tmp1 = if old_q == mflag {
                        rn = rn.wrapping_sub(rm);
                        rn > tmp0
                    } else {
                        rn = rn.wrapping_add(rm);
                        rn < tmp0
                    };
                    q = q ^ tmp1 ^ mflag;
                    self.r[n] = rn;
                    let t = q == mflag;
                    self.sr = (self.sr & !(SR_Q | SR_T)) | ((q as u32) << 8) | t as u32;
                }
                0x5 => {
                    let p = (self.r[n] as u64) * (self.r[m] as u64);
                    self.mach = (p >> 32) as u32;
                    self.macl = p as u32;
                }
                0x6 => self.set_t(self.r[n] > self.r[m]),
                0x7 => self.set_t(self.r[n] as i32 > self.r[m] as i32),
                0x8 => self.r[n] = self.r[n].wrapping_sub(self.r[m]),
                0xa => {
                    // SUBC
                    let (a, b) = (self.r[n], self.r[m]);
                    let t = self.sr & 1;
                    let (r1, c1) = a.overflowing_sub(b);
                    let (r2, c2) = r1.overflowing_sub(t);
                    self.r[n] = r2;
                    self.set_t(c1 || c2);
                }
                0xb => {
                    // SUBV
                    let (r, v) = (self.r[n] as i32).overflowing_sub(self.r[m] as i32);
                    self.r[n] = r as u32;
                    self.set_t(v);
                }
                0xc => self.r[n] = self.r[n].wrapping_add(self.r[m]),
                0xd => {
                    let p = (self.r[n] as i32 as i64) * (self.r[m] as i32 as i64);
                    self.mach = (p as u64 >> 32) as u32;
                    self.macl = p as u32;
                }
                0xe => {
                    // ADDC
                    let (a, b) = (self.r[n], self.r[m]);
                    let t = self.sr & 1;
                    let (r1, c1) = a.overflowing_add(b);
                    let (r2, c2) = r1.overflowing_add(t);
                    self.r[n] = r2;
                    self.set_t(c1 || c2);
                }
                0xf => {
                    let (r, v) = (self.r[n] as i32).overflowing_add(self.r[m] as i32);
                    self.r[n] = r as u32;
                    self.set_t(v);
                }
                _ => return Err(self.illegal()),
            },

            0x4 => match op & 0xff {
                0x00 => {
                    self.set_t(self.r[n] >> 31 != 0);
                    self.r[n] <<= 1;
                }
                0x10 => {
                    self.r[n] = self.r[n].wrapping_sub(1);
                    self.set_t(self.r[n] == 0);
                }
                0x20 => {
                    self.set_t(self.r[n] >> 31 != 0);
                    self.r[n] <<= 1;
                }
                0x01 => {
                    self.set_t(self.r[n] & 1 != 0);
                    self.r[n] >>= 1;
                }
                0x11 => self.set_t(self.r[n] as i32 >= 0),
                0x21 => {
                    self.set_t(self.r[n] & 1 != 0);
                    self.r[n] = ((self.r[n] as i32) >> 1) as u32;
                }
                0x02 | 0x12 | 0x22 | 0x32 | 0x52 | 0x62 | 0xf2 => {
                    // STS.L / STC.L xx,@-Rn
                    let v = match m {
                        0 => self.mach,
                        1 => self.macl,
                        2 => self.pr,
                        3 => {
                            self.need_priv()?;
                            self.sgr
                        }
                        5 => {
                            self.fpu_check()?;
                            self.fpul
                        }
                        6 => {
                            self.fpu_check()?;
                            self.fpscr
                        }
                        _ => {
                            self.need_priv()?;
                            self.dbr
                        }
                    };
                    let a = self.r[n].wrapping_sub(4);
                    self.write32(bus, a, v)?;
                    self.r[n] = a;
                }
                0x03 | 0x13 | 0x23 | 0x33 | 0x43 | 0x83 | 0x93 | 0xa3 | 0xb3 | 0xc3 | 0xd3 | 0xe3 | 0xf3 => {
                    // STC.L xx,@-Rn
                    let v = match m {
                        0 => {
                            self.need_priv()?;
                            self.sr
                        }
                        1 => self.gbr,
                        2 => {
                            self.need_priv()?;
                            self.vbr
                        }
                        3 => {
                            self.need_priv()?;
                            self.ssr
                        }
                        4 => {
                            self.need_priv()?;
                            self.spc
                        }
                        _ => {
                            self.need_priv()?;
                            *self.bank_reg(m)
                        }
                    };
                    let a = self.r[n].wrapping_sub(4);
                    self.write32(bus, a, v)?;
                    self.r[n] = a;
                }
                0x04 => {
                    // ROTL
                    let t = self.r[n] >> 31;
                    self.r[n] = (self.r[n] << 1) | t;
                    self.set_t(t != 0);
                }
                0x24 => {
                    // ROTCL
                    let t = self.r[n] >> 31;
                    self.r[n] = (self.r[n] << 1) | (self.sr & 1);
                    self.set_t(t != 0);
                }
                0x05 => {
                    // ROTR
                    let t = self.r[n] & 1;
                    self.r[n] = (self.r[n] >> 1) | (t << 31);
                    self.set_t(t != 0);
                }
                0x15 => self.set_t(self.r[n] as i32 > 0),
                0x25 => {
                    // ROTCR
                    let t = self.r[n] & 1;
                    self.r[n] = (self.r[n] >> 1) | ((self.sr & 1) << 31);
                    self.set_t(t != 0);
                }
                0x06 | 0x16 | 0x26 | 0x56 | 0x66 | 0xf6 => {
                    // LDS.L / LDC.L @Rn+,xx
                    if m == 5 || m == 6 {
                        self.fpu_check()?;
                    }
                    if m == 0xf {
                        self.need_priv()?;
                    }
                    let v = self.read32(bus, self.r[n])?;
                    self.r[n] = self.r[n].wrapping_add(4);
                    match m {
                        0 => self.mach = v,
                        1 => self.macl = v,
                        2 => self.pr = v,
                        5 => self.fpul = v,
                        6 => self.set_fpscr(v),
                        _ => self.dbr = v,
                    }
                }
                0x07 | 0x17 | 0x27 | 0x37 | 0x47 | 0x87 | 0x97 | 0xa7 | 0xb7 | 0xc7 | 0xd7 | 0xe7 | 0xf7 => {
                    // LDC.L @Rn+,xx
                    if m != 1 {
                        self.need_priv()?;
                    }
                    let v = self.read32(bus, self.r[n])?;
                    self.r[n] = self.r[n].wrapping_add(4);
                    match m {
                        0 => self.set_sr(v),
                        1 => self.gbr = v,
                        2 => self.vbr = v,
                        3 => self.ssr = v,
                        4 => self.spc = v,
                        _ => *self.bank_reg(m) = v,
                    }
                }
                0x08 => self.r[n] <<= 2,
                0x18 => self.r[n] <<= 8,
                0x28 => self.r[n] <<= 16,
                0x09 => self.r[n] >>= 2,
                0x19 => self.r[n] >>= 8,
                0x29 => self.r[n] >>= 16,
                0x0a => self.mach = self.r[n],
                0x1a => self.macl = self.r[n],
                0x2a => self.pr = self.r[n],
                0x5a => {
                    self.fpu_check()?;
                    self.fpul = self.r[n];
                }
                0x6a => {
                    self.fpu_check()?;
                    self.set_fpscr(self.r[n]);
                }
                0xfa => {
                    self.need_priv()?;
                    self.dbr = self.r[n];
                }
                0x0b => {
                    // JSR @Rn
                    self.no_slot()?;
                    let target = self.r[n];
                    self.pr = self.pc.wrapping_add(2);
                    self.note_call(target);
                    self.delay(bus, target)?;
                }
                0x1b => {
                    // TAS.B @Rn
                    let a = self.r[n];
                    let v = self.read8(bus, a)?;
                    self.write8(bus, a, v | 0x80)?;
                    self.set_t(v == 0);
                }
                0x2b => {
                    // JMP @Rn
                    self.no_slot()?;
                    let target = self.r[n];
                    self.delay(bus, target)?;
                }
                0x0e | 0x1e | 0x2e | 0x3e | 0x4e | 0x8e | 0x9e | 0xae | 0xbe | 0xce | 0xde | 0xee | 0xfe => {
                    // LDC Rn,xx
                    if m != 1 {
                        self.need_priv()?;
                    }
                    let v = self.r[n];
                    match m {
                        0 => self.set_sr(v),
                        1 => self.gbr = v,
                        2 => self.vbr = v,
                        3 => self.ssr = v,
                        4 => self.spc = v,
                        _ => *self.bank_reg(m) = v,
                    }
                }
                _ => match op & 0xf {
                    0xc => {
                        // SHAD Rm,Rn
                        let s = self.r[m] as i32;
                        if s >= 0 {
                            self.r[n] <<= s & 31;
                        } else if s & 31 == 0 {
                            self.r[n] = ((self.r[n] as i32) >> 31) as u32;
                        } else {
                            self.r[n] = ((self.r[n] as i32) >> ((!s & 31) + 1)) as u32;
                        }
                    }
                    0xd => {
                        // SHLD Rm,Rn
                        let s = self.r[m] as i32;
                        if s >= 0 {
                            self.r[n] <<= s & 31;
                        } else if s & 31 == 0 {
                            self.r[n] = 0;
                        } else {
                            self.r[n] >>= (!s & 31) + 1;
                        }
                    }
                    0xf => {
                        // MAC.W @Rm+,@Rn+
                        let a = self.read16(bus, self.r[n])? as i16 as i64;
                        self.r[n] = self.r[n].wrapping_add(2);
                        let b = self.read16(bus, self.r[m])? as i16 as i64;
                        self.r[m] = self.r[m].wrapping_add(2);
                        let prod = a * b;
                        if self.sr & SR_S != 0 {
                            let sum = (self.macl as i32 as i64) + prod;
                            if sum > i32::MAX as i64 {
                                self.macl = 0x7fff_ffff;
                                self.mach |= 1;
                            } else if sum < i32::MIN as i64 {
                                self.macl = 0x8000_0000;
                                self.mach |= 1;
                            } else {
                                self.macl = sum as u32;
                            }
                        } else {
                            let mac = ((self.mach as u64) << 32 | self.macl as u64) as i64;
                            let res = mac.wrapping_add(prod);
                            self.mach = (res as u64 >> 32) as u32;
                            self.macl = res as u32;
                        }
                    }
                    _ => return Err(self.illegal()),
                },
            },

            0x5 => {
                // MOV.L @(disp,Rm),Rn
                let a = self.r[m].wrapping_add(((op & 0xf) as u32) << 2);
                self.r[n] = self.read32(bus, a)?;
            }

            0x6 => match op & 0xf {
                0x0 => self.r[n] = self.read8(bus, self.r[m])? as i8 as i32 as u32,
                0x1 => self.r[n] = self.read16(bus, self.r[m])? as i16 as i32 as u32,
                0x2 => self.r[n] = self.read32(bus, self.r[m])?,
                0x3 => self.r[n] = self.r[m],
                0x4 => {
                    let v = self.read8(bus, self.r[m])? as i8 as i32 as u32;
                    if n != m {
                        self.r[m] = self.r[m].wrapping_add(1);
                    }
                    self.r[n] = v;
                }
                0x5 => {
                    let v = self.read16(bus, self.r[m])? as i16 as i32 as u32;
                    if n != m {
                        self.r[m] = self.r[m].wrapping_add(2);
                    }
                    self.r[n] = v;
                }
                0x6 => {
                    let v = self.read32(bus, self.r[m])?;
                    if n != m {
                        self.r[m] = self.r[m].wrapping_add(4);
                    }
                    self.r[n] = v;
                }
                0x7 => self.r[n] = !self.r[m],
                0x8 => self.r[n] = (self.r[m] & 0xffff_0000) | ((self.r[m] & 0xff) << 8) | ((self.r[m] >> 8) & 0xff),
                0x9 => self.r[n] = self.r[m].rotate_left(16),
                0xa => {
                    // NEGC
                    let t = self.sr & 1;
                    let tmp = 0u32.wrapping_sub(self.r[m]);
                    let res = tmp.wrapping_sub(t);
                    self.set_t(tmp > 0 || res > tmp);
                    self.r[n] = res;
                }
                0xb => self.r[n] = 0u32.wrapping_sub(self.r[m]),
                0xc => self.r[n] = self.r[m] & 0xff,
                0xd => self.r[n] = self.r[m] & 0xffff,
                0xe => self.r[n] = self.r[m] as i8 as i32 as u32,
                _ => self.r[n] = self.r[m] as i16 as i32 as u32,
            },

            0x7 => self.r[n] = self.r[n].wrapping_add(sext8(op)),

            0x8 => match n {
                0x0 => {
                    let a = self.r[m].wrapping_add((op & 0xf) as u32);
                    self.write8(bus, a, self.r[0] as u8)?;
                }
                0x1 => {
                    let a = self.r[m].wrapping_add(((op & 0xf) as u32) << 1);
                    self.write16(bus, a, self.r[0] as u16)?;
                }
                0x4 => {
                    let a = self.r[m].wrapping_add((op & 0xf) as u32);
                    self.r[0] = self.read8(bus, a)? as i8 as i32 as u32;
                }
                0x5 => {
                    let a = self.r[m].wrapping_add(((op & 0xf) as u32) << 1);
                    self.r[0] = self.read16(bus, a)? as i16 as i32 as u32;
                }
                0x8 => self.set_t(self.r[0] == sext8(op)),
                0x9 => {
                    // BT
                    self.no_slot()?;
                    if self.t() {
                        self.pc = self.pc.wrapping_add(2).wrapping_add(sext8(op) << 1);
                    }
                }
                0xb => {
                    // BF
                    self.no_slot()?;
                    if !self.t() {
                        let target = self.pc.wrapping_add(2).wrapping_add(sext8(op) << 1);
                        if op == 0x8bfd {
                            self.skip_delay_loop(bus, target);
                        }
                        self.pc = target;
                    }
                }
                0xd => {
                    // BT/S
                    self.no_slot()?;
                    let target = if self.t() {
                        self.pc.wrapping_add(2).wrapping_add(sext8(op) << 1)
                    } else {
                        self.pc.wrapping_add(2)
                    };
                    self.delay(bus, target)?;
                }
                0xf => {
                    // BF/S
                    self.no_slot()?;
                    let target = if !self.t() {
                        self.pc.wrapping_add(2).wrapping_add(sext8(op) << 1)
                    } else {
                        self.pc.wrapping_add(2)
                    };
                    self.delay(bus, target)?;
                }
                _ => return Err(self.illegal()),
            },

            0x9 => {
                // MOV.W @(disp,PC),Rn
                let a = self.pc.wrapping_add(2).wrapping_add(((op & 0xff) as u32) << 1);
                self.r[n] = self.read16(bus, a)? as i16 as i32 as u32;
            }

            0xa => {
                // BRA
                self.no_slot()?;
                let target = self.pc.wrapping_add(2).wrapping_add(sext12(op) << 1);
                self.delay(bus, target)?;
            }
            0xb => {
                // BSR
                self.no_slot()?;
                let target = self.pc.wrapping_add(2).wrapping_add(sext12(op) << 1);
                self.pr = self.pc.wrapping_add(2);
                self.note_call(target);
                self.delay(bus, target)?;
            }

            0xc => {
                let imm = (op & 0xff) as u32;
                match n {
                    0x0 => self.write8(bus, self.gbr.wrapping_add(imm), self.r[0] as u8)?,
                    0x1 => self.write16(bus, self.gbr.wrapping_add(imm << 1), self.r[0] as u16)?,
                    0x2 => self.write32(bus, self.gbr.wrapping_add(imm << 2), self.r[0])?,
                    0x3 => {
                        self.no_slot()?;
                        return Err(Exc::new(ExcKind::Trapa(imm as u8)));
                    }
                    0x4 => self.r[0] = self.read8(bus, self.gbr.wrapping_add(imm))? as i8 as i32 as u32,
                    0x5 => self.r[0] = self.read16(bus, self.gbr.wrapping_add(imm << 1))? as i16 as i32 as u32,
                    0x6 => self.r[0] = self.read32(bus, self.gbr.wrapping_add(imm << 2))?,
                    0x7 => {
                        // MOVA
                        self.r[0] = (self.pc.wrapping_add(2) & !3).wrapping_add(imm << 2);
                    }
                    0x8 => self.set_t(self.r[0] & imm == 0),
                    0x9 => self.r[0] &= imm,
                    0xa => self.r[0] ^= imm,
                    0xb => self.r[0] |= imm,
                    0xc => {
                        let v = self.read8(bus, self.gbr.wrapping_add(self.r[0]))?;
                        self.set_t(v as u32 & imm == 0);
                    }
                    0xd => {
                        let a = self.gbr.wrapping_add(self.r[0]);
                        let v = self.read8(bus, a)?;
                        self.write8(bus, a, v & imm as u8)?;
                    }
                    0xe => {
                        let a = self.gbr.wrapping_add(self.r[0]);
                        let v = self.read8(bus, a)?;
                        self.write8(bus, a, v ^ imm as u8)?;
                    }
                    _ => {
                        let a = self.gbr.wrapping_add(self.r[0]);
                        let v = self.read8(bus, a)?;
                        self.write8(bus, a, v | imm as u8)?;
                    }
                }
            }

            0xd => {
                // MOV.L @(disp,PC),Rn
                let a = (self.pc.wrapping_add(2) & !3).wrapping_add(((op & 0xff) as u32) << 2);
                self.r[n] = self.read32(bus, a)?;
            }

            0xe => self.r[n] = sext8(op),

            _ => return self.exec_fpu(bus, op),
        }
        Ok(())
    }
}
