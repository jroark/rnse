//! SH-4 floating point unit.

use super::cpu::{Cpu, Exc, ExcKind};
use super::*;

type R = Result<(), Exc>;

impl Cpu {
    #[inline(always)]
    pub(super) fn fpu_check(&self) -> R {
        if self.sr & SR_FD != 0 {
            Err(Exc::new(if self.in_delay { ExcKind::SlotFpuDisable } else { ExcKind::FpuDisable }))
        } else {
            Ok(())
        }
    }

    #[inline(always)]
    fn fs(&self, n: usize) -> f32 {
        f32::from_bits(self.fr[n])
    }
    #[inline(always)]
    fn set_fs(&mut self, n: usize, v: f32) {
        self.fr[n] = v.to_bits();
    }
    #[inline(always)]
    fn fd(&self, n: usize) -> f64 {
        let n = n & !1;
        f64::from_bits((self.fr[n] as u64) << 32 | self.fr[n + 1] as u64)
    }
    #[inline(always)]
    fn set_fd(&mut self, n: usize, v: f64) {
        let n = n & !1;
        let b = v.to_bits();
        self.fr[n] = (b >> 32) as u32;
        self.fr[n + 1] = b as u32;
    }

    /// Register index for a 64-bit FMOV operand: DRn for even numbers,
    /// XDn (other bank) for odd ones.
    #[inline(always)]
    fn dx(n: usize) -> usize {
        if n & 1 != 0 {
            (n & !1) + 16
        } else {
            n
        }
    }

    // ---- IEEE helpers honouring FPSCR.RM and FPSCR.DN ---------------------
    //
    // The host computes in round-to-nearest.  For round-to-zero (which is
    // the SH-4 reset default and what this firmware runs with) the nearest
    // result is corrected using an exactly computed error term.

    #[inline(always)]
    fn rz(&self) -> bool {
        self.fpscr & FPSCR_RM == 1
    }

    /// FPSCR.DN: denormal operands are read as zero.
    #[inline(always)]
    fn in32(&self, v: f32) -> f32 {
        if self.fpscr & FPSCR_DN != 0 && v.is_subnormal() {
            f32::from_bits(v.to_bits() & 0x8000_0000)
        } else {
            v
        }
    }
    #[inline(always)]
    fn in64(&self, v: f64) -> f64 {
        if self.fpscr & FPSCR_DN != 0 && v.is_subnormal() {
            f64::from_bits(v.to_bits() & 0x8000_0000_0000_0000)
        } else {
            v
        }
    }

    /// Final fix-ups for a single precision result: default NaN, and
    /// flush-to-zero of denormal results when FPSCR.DN is set.
    #[inline(always)]
    fn out32(&self, v: f32) -> f32 {
        if v.is_nan() {
            f32::from_bits(0x7fbf_ffff)
        } else if self.fpscr & FPSCR_DN != 0 && v.is_subnormal() {
            f32::from_bits(v.to_bits() & 0x8000_0000)
        } else {
            v
        }
    }
    #[inline(always)]
    fn out64(&self, v: f64) -> f64 {
        if v.is_nan() {
            f64::from_bits(0x7ff7_ffff_ffff_ffff)
        } else if self.fpscr & FPSCR_DN != 0 && v.is_subnormal() {
            f64::from_bits(v.to_bits() & 0x8000_0000_0000_0000)
        } else {
            v
        }
    }

    #[inline]
    fn toward_zero32(v: f32) -> f32 {
        if v == 0.0 || v.is_nan() {
            v
        } else {
            f32::from_bits(v.to_bits() - 1)
        }
    }
    #[inline]
    fn toward_zero64(v: f64) -> f64 {
        if v == 0.0 || v.is_nan() {
            v
        } else {
            f64::from_bits(v.to_bits() - 1)
        }
    }

    /// Round the exact value `s + e` to single precision, where `s` is a
    /// double and `e` an error term far smaller than one ulp of `s`
    /// (only its sign matters).
    fn round32(&self, s: f64, e: f64) -> f32 {
        let r = s as f32;
        if r.is_nan() {
            return r;
        }
        let rd = r as f64;
        if self.rz() {
            if rd.abs() > s.abs() || (rd == s && e != 0.0 && (e < 0.0) != (s < 0.0)) {
                return Self::toward_zero32(r);
            }
            return r;
        }
        if e != 0.0 && rd != s && r.is_finite() {
            // If `s` sat exactly half way between two floats, the error
            // term decides the direction instead of ties-to-even.
            let other = if (s > rd) == (r > 0.0) || r == 0.0 {
                // neighbour further from zero (or first denormal from 0)
                if r == 0.0 {
                    f32::from_bits(1 | if s < 0.0 { 0x8000_0000 } else { 0 })
                } else {
                    f32::from_bits(r.to_bits() + 1)
                }
            } else {
                Self::toward_zero32(r)
            };
            if rd + other as f64 == 2.0 * s && (e > 0.0) == (other as f64 > rd) {
                return other;
            }
        }
        r
    }

    /// Exact sum of two doubles as (rounded sum, error).
    #[inline]
    fn two_sum(a: f64, b: f64) -> (f64, f64) {
        let s = a + b;
        let bv = s - a;
        let e = (a - (s - bv)) + (b - bv);
        (s, e)
    }

    fn add32(&self, a: f32, b: f32) -> f32 {
        let (a, b) = (self.in32(a), self.in32(b));
        let (s, e) = Self::two_sum(a as f64, b as f64);
        let e = if s.is_finite() { e } else { 0.0 };
        self.out32(self.round32(s, e))
    }
    fn mul32(&self, a: f32, b: f32) -> f32 {
        let (a, b) = (self.in32(a), self.in32(b));
        self.out32(self.round32(a as f64 * b as f64, 0.0))
    }
    fn div32(&self, a: f32, b: f32) -> f32 {
        let (a, b) = (self.in32(a), self.in32(b));
        let q = a / b;
        if self.rz() && a.is_finite() && b.is_finite() && b != 0.0 && !q.is_nan() {
            if q.is_infinite() {
                return self.out32(Self::toward_zero32(q));
            }
            // a - q*b is exact in double precision.
            let rem = a as f64 - q as f64 * b as f64;
            if rem != 0.0 && ((rem < 0.0) != (b < 0.0)) != (q < 0.0) && q != 0.0 {
                return self.out32(Self::toward_zero32(q));
            }
        }
        self.out32(q)
    }
    fn sqrt32(&self, a: f32) -> f32 {
        let a = self.in32(a);
        let r = a.sqrt();
        if self.rz() && r.is_finite() && (r as f64) * (r as f64) > a as f64 {
            return self.out32(Self::toward_zero32(r));
        }
        self.out32(r)
    }

    fn add64(&self, a: f64, b: f64) -> f64 {
        let (a, b) = (self.in64(a), self.in64(b));
        let (s, e) = Self::two_sum(a, b);
        if self.rz() && a.is_finite() && b.is_finite() {
            if s.is_infinite() {
                return self.out64(Self::toward_zero64(s));
            }
            if e != 0.0 && (e < 0.0) != (s < 0.0) {
                return self.out64(Self::toward_zero64(s));
            }
        }
        self.out64(s)
    }
    /// Split a finite, non-zero double into an integer mantissa with its
    /// top bit at position 52 and a binary exponent: |x| = m * 2^e.
    fn split64(x: f64) -> (u64, i32) {
        let bits = x.to_bits();
        let exp = ((bits >> 52) & 0x7ff) as i32;
        let frac = bits & 0x000f_ffff_ffff_ffff;
        if exp == 0 {
            let sh = frac.leading_zeros() as i32 - 11;
            (frac << sh, -1074 - sh)
        } else {
            (frac | (1 << 52), exp - 1075)
        }
    }

    /// The double nearest to zero-ward of m * 2^e (i.e. truncated).
    fn pack_trunc64(neg: bool, m: u128, e: i32) -> f64 {
        let sign = (neg as u64) << 63;
        if m == 0 {
            return f64::from_bits(sign);
        }
        let len = 128 - m.leading_zeros() as i32; // significant bits
        let top = e + len - 1; // exponent of the leading bit
        if top > 1023 {
            return f64::from_bits(sign | 0x7fef_ffff_ffff_ffff);
        }
        // Position the value so that bit 0 has weight 2^lsb.
        let lsb = if top >= -1022 { top - 52 } else { -1074 };
        let sh = lsb - e;
        let mant = if sh >= 128 {
            0
        } else if sh >= 0 {
            (m >> sh) as u64
        } else {
            (m << -sh) as u64
        };
        if top >= -1022 {
            f64::from_bits(sign | (((top + 1023) as u64) << 52) | (mant & 0x000f_ffff_ffff_ffff))
        } else {
            f64::from_bits(sign | mant)
        }
    }

    fn mul64(&self, a: f64, b: f64) -> f64 {
        let (a, b) = (self.in64(a), self.in64(b));
        if self.rz() && a.is_finite() && b.is_finite() && a != 0.0 && b != 0.0 {
            let (ma, ea) = Self::split64(a);
            let (mb, eb) = Self::split64(b);
            let r = Self::pack_trunc64((a < 0.0) != (b < 0.0), ma as u128 * mb as u128, ea + eb);
            return self.out64(r);
        }
        self.out64(a * b)
    }
    fn div64(&self, a: f64, b: f64) -> f64 {
        let (a, b) = (self.in64(a), self.in64(b));
        if self.rz() && a.is_finite() && b.is_finite() && a != 0.0 && b != 0.0 {
            let (ma, ea) = Self::split64(a);
            let (mb, eb) = Self::split64(b);
            // 2^74 <= quotient < 2^76, so truncating it keeps 53+ bits.
            let q = ((ma as u128) << 75) / mb as u128;
            let r = Self::pack_trunc64((a < 0.0) != (b < 0.0), q, ea - eb - 75);
            return self.out64(r);
        }
        self.out64(a / b)
    }
    fn sqrt64(&self, a: f64) -> f64 {
        let a = self.in64(a);
        let r = a.sqrt();
        if self.rz() && r.is_finite() && r > 0.0 {
            // Scale tiny operands so that the residual a - r*r is exact.
            let (a2, r2) = if a < 1e-200 {
                (a * 2f64.powi(600), r * 2f64.powi(300))
            } else {
                (a, r)
            };
            if (-r2).mul_add(r2, a2) < 0.0 {
                return self.out64(Self::toward_zero64(r));
            }
        }
        self.out64(r)
    }

    fn ftrc(v: f64) -> u32 {
        if v.is_nan() {
            0x8000_0000
        } else if v >= 2147483648.0 {
            0x7fff_ffff
        } else if v <= -2147483649.0 {
            0x8000_0000
        } else {
            v as i32 as u32
        }
    }

    pub(super) fn exec_fpu<B: Bus>(&mut self, bus: &mut B, op: u16) -> R {
        let n = ((op >> 8) & 0xf) as usize;
        let m = ((op >> 4) & 0xf) as usize;
        self.fpu_check()?;
        let pr = self.fpscr & FPSCR_PR != 0;
        let sz = self.fpscr & FPSCR_SZ != 0;
        match op & 0xf {
            0x0 => {
                if pr {
                    self.set_fd(n, self.add64(self.fd(n), self.fd(m)));
                } else {
                    self.set_fs(n, self.add32(self.fs(n), self.fs(m)));
                }
            }
            0x1 => {
                if pr {
                    self.set_fd(n, self.add64(self.fd(n), -self.fd(m)));
                } else {
                    self.set_fs(n, self.add32(self.fs(n), -self.fs(m)));
                }
            }
            0x2 => {
                if pr {
                    self.set_fd(n, self.mul64(self.fd(n), self.fd(m)));
                } else {
                    self.set_fs(n, self.mul32(self.fs(n), self.fs(m)));
                }
            }
            0x3 => {
                if pr {
                    self.set_fd(n, self.div64(self.fd(n), self.fd(m)));
                } else {
                    self.set_fs(n, self.div32(self.fs(n), self.fs(m)));
                }
            }
            0x4 => {
                let t = if pr {
                    self.in64(self.fd(n)) == self.in64(self.fd(m))
                } else {
                    self.in32(self.fs(n)) == self.in32(self.fs(m))
                };
                self.set_t(t);
            }
            0x5 => {
                let t = if pr {
                    self.in64(self.fd(n)) > self.in64(self.fd(m))
                } else {
                    self.in32(self.fs(n)) > self.in32(self.fs(m))
                };
                self.set_t(t);
            }
            0x6 => {
                // FMOV @(R0,Rm),FRn
                let a = self.r[m].wrapping_add(self.r[0]);
                if sz {
                    let d = Self::dx(n);
                    let hi = self.read32(bus, a)?;
                    let lo = self.read32(bus, a.wrapping_add(4))?;
                    self.fr[d] = hi;
                    self.fr[d + 1] = lo;
                } else {
                    self.fr[n] = self.read32(bus, a)?;
                }
            }
            0x7 => {
                // FMOV FRm,@(R0,Rn)
                let a = self.r[n].wrapping_add(self.r[0]);
                if sz {
                    let s = Self::dx(m);
                    self.write32(bus, a, self.fr[s])?;
                    self.write32(bus, a.wrapping_add(4), self.fr[s + 1])?;
                } else {
                    self.write32(bus, a, self.fr[m])?;
                }
            }
            0x8 => {
                // FMOV @Rm,FRn
                let a = self.r[m];
                if sz {
                    let d = Self::dx(n);
                    let hi = self.read32(bus, a)?;
                    let lo = self.read32(bus, a.wrapping_add(4))?;
                    self.fr[d] = hi;
                    self.fr[d + 1] = lo;
                } else {
                    self.fr[n] = self.read32(bus, a)?;
                }
            }
            0x9 => {
                // FMOV @Rm+,FRn
                let a = self.r[m];
                if sz {
                    let d = Self::dx(n);
                    let hi = self.read32(bus, a)?;
                    let lo = self.read32(bus, a.wrapping_add(4))?;
                    self.fr[d] = hi;
                    self.fr[d + 1] = lo;
                    self.r[m] = a.wrapping_add(8);
                } else {
                    self.fr[n] = self.read32(bus, a)?;
                    self.r[m] = a.wrapping_add(4);
                }
            }
            0xa => {
                // FMOV FRm,@Rn
                let a = self.r[n];
                if sz {
                    let s = Self::dx(m);
                    self.write32(bus, a, self.fr[s])?;
                    self.write32(bus, a.wrapping_add(4), self.fr[s + 1])?;
                } else {
                    self.write32(bus, a, self.fr[m])?;
                }
            }
            0xb => {
                // FMOV FRm,@-Rn
                if sz {
                    let s = Self::dx(m);
                    let a = self.r[n].wrapping_sub(8);
                    self.write32(bus, a, self.fr[s])?;
                    self.write32(bus, a.wrapping_add(4), self.fr[s + 1])?;
                    self.r[n] = a;
                } else {
                    let a = self.r[n].wrapping_sub(4);
                    self.write32(bus, a, self.fr[m])?;
                    self.r[n] = a;
                }
            }
            0xc => {
                // FMOV FRm,FRn
                if sz {
                    let (s, d) = (Self::dx(m), Self::dx(n));
                    self.fr[d] = self.fr[s];
                    self.fr[d + 1] = self.fr[s + 1];
                } else {
                    self.fr[n] = self.fr[m];
                }
            }
            0xd => match m {
                0x0 => self.fr[n] = self.fpul,                 // FSTS
                0x1 => self.fpul = self.fr[n],                 // FLDS
                0x2 => {
                    // FLOAT
                    if pr {
                        self.set_fd(n, self.fpul as i32 as f64);
                    } else {
                        self.set_fs(n, self.round32(self.fpul as i32 as f64, 0.0));
                    }
                }
                0x3 => {
                    // FTRC
                    self.fpul = if pr {
                        Self::ftrc(self.in64(self.fd(n)))
                    } else {
                        Self::ftrc(self.in32(self.fs(n)) as f64)
                    };
                }
                0x4 => {
                    // FNEG
                    let i = if pr { n & !1 } else { n };
                    self.fr[i] ^= 0x8000_0000;
                }
                0x5 => {
                    // FABS
                    let i = if pr { n & !1 } else { n };
                    self.fr[i] &= 0x7fff_ffff;
                }
                0x6 => {
                    // FSQRT
                    if pr {
                        self.set_fd(n, self.sqrt64(self.fd(n)));
                    } else {
                        self.set_fs(n, self.sqrt32(self.fs(n)));
                    }
                }
                0x7 => {
                    // FSRRA
                    let v = self.in32(self.fs(n)) as f64;
                    self.set_fs(n, self.out32(self.round32(1.0 / v.sqrt(), 0.0)));
                }
                0x8 => self.fr[n] = 0,                       // FLDI0
                0x9 => self.fr[n] = 0x3f80_0000,             // FLDI1
                0xa => {
                    // FCNVSD FPUL,DRn
                    let v = self.in32(f32::from_bits(self.fpul)) as f64;
                    self.set_fd(n, self.out64(v));
                }
                0xb => {
                    // FCNVDS DRn,FPUL
                    let v = self.in64(self.fd(n));
                    self.fpul = self.out32(self.round32(v, 0.0)).to_bits();
                }
                0xe => {
                    // FIPR FVm,FVn
                    let fn_ = n & 0xc;
                    let fm = (n & 3) << 2;
                    let mut acc = 0.0f64;
                    for i in 0..4 {
                        acc += self.in32(self.fs(fn_ + i)) as f64 * self.in32(self.fs(fm + i)) as f64;
                    }
                    self.set_fs(fn_ + 3, self.out32(self.round32(acc, 0.0)));
                }
                0xf => {
                    if n & 1 == 0 {
                        // FSCA FPUL,DRn
                        let ang = (self.fpul & 0xffff) as f64 * (std::f64::consts::PI / 32768.0);
                        let d = n & 0xe;
                        self.set_fs(d, ang.sin() as f32);
                        self.set_fs(d + 1, ang.cos() as f32);
                    } else if n & 3 == 1 {
                        // FTRV XMTRX,FVn
                        let fv = n & 0xc;
                        let v: [f64; 4] = [
                            self.fs(fv) as f64,
                            self.fs(fv + 1) as f64,
                            self.fs(fv + 2) as f64,
                            self.fs(fv + 3) as f64,
                        ];
                        for i in 0..4 {
                            let mut acc = 0.0f64;
                            for (j, vj) in v.iter().enumerate() {
                                acc += self.in32(self.fs(16 + j * 4 + i)) as f64 * vj;
                            }
                            self.set_fs(fv + i, self.out32(self.round32(acc, 0.0)));
                        }
                    } else if op == 0xf3fd {
                        // FSCHG
                        self.fpscr ^= FPSCR_SZ;
                    } else if op == 0xfbfd {
                        // FRCHG
                        self.set_fpscr(self.fpscr ^ FPSCR_FR);
                    } else {
                        return Err(Exc::new(if self.in_delay {
                            ExcKind::SlotIllegalInsn
                        } else {
                            ExcKind::IllegalInsn
                        }));
                    }
                }
                _ => {
                    return Err(Exc::new(if self.in_delay {
                        ExcKind::SlotIllegalInsn
                    } else {
                        ExcKind::IllegalInsn
                    }))
                }
            },
            0xe => {
                // FMAC FR0,FRm,FRn
                if pr {
                    return Err(Exc::new(ExcKind::IllegalInsn));
                }
                // Fused: the product is exact in double precision, and the
                // error of the addition is carried into the final rounding.
                let p = self.in32(self.fs(0)) as f64 * self.in32(self.fs(m)) as f64;
                let (sum, e) = Self::two_sum(p, self.in32(self.fs(n)) as f64);
                let e = if sum.is_finite() { e } else { 0.0 };
                self.set_fs(n, self.out32(self.round32(sum, e)));
            }
            _ => {
                return Err(Exc::new(if self.in_delay {
                    ExcKind::SlotIllegalInsn
                } else {
                    ExcKind::IllegalInsn
                }))
            }
        }
        Ok(())
    }
}
