#!/usr/bin/env python3
"""Differential test of the SH-4 core against QEMU.

Generates programs made of random user-mode instructions, runs each one
under `qemu-sh4eb-static` and under `target/release/sh4run`, and compares
the final register and memory state.

    tools/cpufuzz.py [--count N] [--len L] [--seed S] [--fpu] [--keep DIR]

Needs qemu-user-static (Debian/Ubuntu package of the same name).

Instructions that QEMU 8.2 itself gets wrong are left out of the random
pool, because the comparison would only report QEMU's bugs: ADDV and SUBV
(result written to the wrong register), ROTL and ROTR (T always cleared),
MAC.W and MAC.L, and DIV1 with a zero divisor.  Those are covered by the
unit tests in src/sh4/tests.rs instead, which follow the SH-4 manual.
"""
import argparse, os, random, struct, subprocess, sys, tempfile

BASE = 0x00400000
MEM = 0x40000            # image + scratch + dump area
SCRATCH = BASE + 0x20000  # r13/GBR point here (middle of a 64K window)
DUMP_TOP = BASE + 0x3f000 # r15 starts here and is pushed downwards
QEMU = 'qemu-sh4eb-static'

GP = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14]   # r13 = scratch base, r15 = dump pointer
INTERESTING = [0, 1, 2, 0xffffffff, 0x80000000, 0x7fffffff, 0xff, 0xffff, 0x8000, 0x7fff, 0x10000, 31, 32, 33, 0xffffffe0, 0xffffffe1]
FP_INTERESTING = [0.0, -0.0, 1.0, -1.0, 2.0, 0.5, 3.5, 100.25, -7.75, 1e10, -1e10, 1e-10, 16777216.0, 2147483648.0, -2147483649.0, 65535.5, 1e30, 1e-30, 123456.789]


def rnd32(r):
    c = r.random()
    if c < 0.3: return r.choice(INTERESTING)
    if c < 0.5: return r.randrange(0, 256)
    if c < 0.6: return (-r.randrange(1, 256)) & 0xffffffff
    return r.getrandbits(32)


def gen_insn(r, fpu, state):
    """Return a list of 16-bit opcodes forming one unit."""
    n = r.choice(GP); m = r.choice(GP); i8 = r.getrandbits(8); d4 = r.getrandbits(4)
    def nm(base): return [base | (n << 8) | (m << 4)]
    def n_(base): return [base | (n << 8)]
    # tst rm,rm ; bt over ; sett/clrt ; div1 rm,rn
    # QEMU 8.2 mis-computes the borrow of a DIV1 step when the divisor is zero and
    # reads the shifted value when Rm == Rn, so those cases are kept out of the
    # comparison.
    def div1():
        mm = m if m != n else GP[(GP.index(m) + 1) % len(GP)]
        return [0x2008 | (mm << 8) | (mm << 4), 0x8901, r.choice([0x0008, 0x0018]), 0x3004 | (n << 8) | (mm << 4)]
    choices = [
        lambda: nm(0x6003), lambda: [0xe000 | (n << 8) | i8], lambda: [0x7000 | (n << 8) | i8],
        lambda: nm(0x300c), lambda: nm(0x300e), lambda: nm(0x3008), lambda: nm(0x300a),
        lambda: nm(0x3000), lambda: nm(0x3002), lambda: nm(0x3003), lambda: nm(0x3006), lambda: nm(0x3007),
        lambda: [0x8800 | i8], lambda: n_(0x4011), lambda: n_(0x4015), lambda: nm(0x200c),
        lambda: nm(0x2007), lambda: [0x0019], lambda: div1(), lambda: div1(),
        lambda: nm(0x0007), lambda: nm(0x200e), lambda: nm(0x200f), lambda: nm(0x3005), lambda: nm(0x300d),
        lambda: n_(0x4010),
        lambda: nm(0x600c), lambda: nm(0x600d), lambda: nm(0x600e), lambda: nm(0x600f), lambda: nm(0x6008), lambda: nm(0x6009), lambda: nm(0x200d),
        lambda: nm(0x600b), lambda: nm(0x600a), lambda: nm(0x6007),
        lambda: nm(0x2008), lambda: nm(0x2009), lambda: nm(0x200a), lambda: nm(0x200b),
        lambda: [0xc800 | i8], lambda: [0xc900 | i8], lambda: [0xca00 | i8], lambda: [0xcb00 | i8],
        lambda: n_(0x4000), lambda: n_(0x4020), lambda: n_(0x4001), lambda: n_(0x4021), lambda: n_(0x4024), lambda: n_(0x4025),
        lambda: n_(0x4008), lambda: n_(0x4018), lambda: n_(0x4028), lambda: n_(0x4009), lambda: n_(0x4019), lambda: n_(0x4029),
        lambda: nm(0x400c), lambda: nm(0x400d), lambda: nm(0x400c), lambda: nm(0x400d),
        lambda: [0x0008], lambda: [0x0018], lambda: n_(0x0029), lambda: [0x0048], lambda: [0x0058], lambda: [0x0028],
        lambda: n_(0x000a), lambda: n_(0x001a), lambda: n_(0x002a), lambda: n_(0x400a), lambda: n_(0x401a), lambda: n_(0x402a), lambda: n_(0x0012),
        # memory through r13
        lambda: [0x1d00 | (m << 4) | d4], lambda: [0x5000 | (n << 8) | (0xd << 4) | d4],
        lambda: [0x80d0 | d4], lambda: [0x81d0 | d4], lambda: [0x84d0 | d4], lambda: [0x85d0 | d4],
        lambda: [0x2d00 | (m << 4)], lambda: [0x2d01 | (m << 4)], lambda: [0x2d02 | (m << 4)],
        lambda: [0x60d0 | (n << 8)], lambda: [0x60d1 | (n << 8)], lambda: [0x60d2 | (n << 8)],
        lambda: [0x2d06 | (m << 4)], lambda: [0x60d6 | (n << 8)], lambda: [0x4d1b],
        # GBR relative
        lambda: [0xc000 | i8], lambda: [0xc100 | i8], lambda: [0xc200 | i8], lambda: [0xc400 | i8], lambda: [0xc500 | i8], lambda: [0xc600 | i8],
        lambda: [0xc97c, r.choice([0xcc00, 0xcd00, 0xce00, 0xcf00]) | i8],                    # and #0x7c,r0 ; op.b #imm,@(r0,gbr)
        lambda: [0xc97c, r.choice([0x0d04, 0x0d05, 0x0d06]) | (m << 4)],                      # and #0x7c,r0 ; mov.x rm,@(r0,r13)
        lambda: [0xc97c, r.choice([0x00dc, 0x00dd, 0x00de]) | (n << 8)],                      # and #0x7c,r0 ; mov.x @(r0,r13),rn
        # pc relative
        lambda: [0xc700 | i8], lambda: [0x9000 | (n << 8) | i8], lambda: [0xd000 | (n << 8) | i8],
    ]
    if fpu:
        fn = r.randrange(16); fm = r.randrange(16)
        pr, sz = state['pr'], state['sz']
        if pr:
            fn &= 0xe; fm &= 0xe          # double precision operands are DR0..DR14
        def f(base): return [base | (fn << 8) | (fm << 4)]
        def mem(op_with_reg):
            """FMOV through memory.  With SZ=1 the access is 8 bytes wide and has to be
            8-aligned for QEMU, so go through r12 = r13 & ~7."""
            if not sz:
                return [op_with_reg(0xd)]
            return [0x6cd3, 0x4c09, 0x4c01, 0x4c08, 0x4c00, op_with_reg(0xc)]
        fch = [
            lambda: f(0xf000), lambda: f(0xf001), lambda: f(0xf002), lambda: f(0xf003), lambda: f(0xf004), lambda: f(0xf005),
            lambda: [0xf00c | (r.randrange(16) << 8) | (r.randrange(16) << 4)],
            lambda: [0xf00d | (r.randrange(16) << 8)], lambda: [0xf01d | (r.randrange(16) << 8)],      # fsts / flds
            lambda: [0xf02d | (fn << 8)],                                                              # float
            # fcmp/eq frn,frn ; bf over ; ftrc -- QEMU converts NaN to 0x7fffffff, the manual says 0x80000000
            lambda: [0xf004 | (fn << 8) | (fn << 4), 0x8b00, 0xf03d | (fn << 8)],
            lambda: [0xf04d | (fn << 8)], lambda: [0xf05d | (fn << 8)], lambda: [0xf06d | (fn << 8)],  # fneg / fabs / fsqrt
            lambda: [0x405a | (n << 8)], lambda: [0x005a | (n << 8)],                                  # lds rn,fpul / sts fpul,rn
            lambda: mem(lambda b: 0xf00a | (b << 8) | (r.randrange(16) << 4)),                         # fmov frm,@rb
            lambda: mem(lambda b: 0xf008 | (r.randrange(16) << 8) | (b << 4)),                         # fmov @rb,frn
            lambda: mem(lambda b: 0xf00b | (b << 8) | (r.randrange(16) << 4)),                         # fmov frm,@-rb
            lambda: mem(lambda b: 0xf009 | (r.randrange(16) << 8) | (b << 4)),                         # fmov @rb+,frn
        ]
        if not sz:
            fch += [lambda: [0xc978, 0xf007 | (0xd << 8) | (r.randrange(16) << 4)],                    # fmov frm,@(r0,r13)
                    lambda: [0xc978, 0xf006 | (r.randrange(16) << 8) | (0xd << 4)]]
        if not pr:
            # FIPR and FTRV are left out: QEMU 8.2 only accepts them with PR=1.
            fch += [lambda: f(0xf00e), lambda: [0xf08d | (fn << 8)], lambda: [0xf09d | (fn << 8)], lambda: [0xfbfd]]
        else:
            fch += [lambda: [0xf0ad | (fn << 8)], lambda: [0xf0bd | (fn << 8)]]                        # fcnvsd / fcnvds
        c = r.random()
        if c < 0.03:
            # full FPSCR load: PR, rounding mode and DN change together (SZ=0, as PR=1 requires)
            state['pr'] = r.random() < 0.4; state['sz'] = False
            v = (0x00080000 if state['pr'] else 0) | r.choice([0, 1]) | (r.choice([0, 0x00040000]) if state['dn'] else 0)
            return ['FPSCR', v]
        if not pr and c < 0.05:
            state['sz'] = not sz
            return [0x0009, 0xf3fd]      # two words, so that it is shielded from stray branches
        if c < 0.5:
            return r.choice(fch)()
    return r.choice(choices)()


def build(seed, length, fpu, dn=False):
    """Return (words, literals).  `words` is a list of opcodes in which
    each literal load is a placeholder; `literals` maps the placeholder
    index to (register, value)."""
    r = random.Random(seed)
    code = []
    lits = {}
    def w(*ops): code.extend(ops)
    def movl(reg, val):
        lits[len(code)] = (reg, val); w(0)
    # --- prologue
    movl(15, DUMP_TOP); movl(13, SCRATCH)
    w(0x4d1e)                        # ldc r13,gbr
    movl(0, 0); w(0x406a)            # lds r0,fpscr  (PR=0 SZ=0 RN)
    movl(1, 'FPTBL')
    for i in range(16): w(0xf019 | (i << 8))     # fmov.s @r1+,fr(i)
    w(0xfbfd)
    for i in range(16): w(0xf019 | (i << 8))
    w(0xfbfd)
    movl(0, rnd32(r)); w(0x400a); movl(0, rnd32(r)); w(0x401a); movl(0, rnd32(r)); w(0x402a)  # mach macl pr
    movl(0, rnd32(r)); w(0x405a)                                                             # fpul
    for reg in GP: movl(reg, rnd32(r))
    w(0x0018 if r.random() < 0.5 else 0x0008)
    # --- body
    state = {'pr': False, 'sz': False, 'dn': dn}
    body_start = len(code)
    prev_delayed = False
    count = 0
    while count < length:
        c = r.random()
        if c < 0.06 and not prev_delayed:
            # forward branch over 1..4 instructions
            k = r.randrange(0, 4)
            kind = r.choice([0x8900, 0x8b00, 0x8d00, 0x8f00, 0xa000, 0xb000])
            w(kind | k)
            prev_delayed = kind in (0x8d00, 0x8f00, 0xa000, 0xb000)
            count += 1
            continue
        ops = gen_insn(r, fpu, state)
        if ops and ops[0] == 'FPSCR':
            if prev_delayed: w(0x0009)
            # a literal load expands to several words; keep stray forward branches off it
            w(0x0009, 0x0009, 0x0009, 0x0009)
            movl(0, ops[1]); w(0x406a); prev_delayed = False; count += 2
            continue
        if prev_delayed:
            # delay slot: no PC-relative forms, single instruction only
            if len(ops) != 1 or (ops[0] >> 12) in (0x9, 0xd) or (ops[0] & 0xff00) == 0xc700:
                ops = [0x0009]
        if len(ops) > 1:
            # multi-instruction unit: stop an earlier forward branch landing inside it
            w(0x0009, 0x0009, 0x0009, 0x0009)
        w(*ops); count += len(ops); prev_delayed = False
    if prev_delayed: w(0x0009)
    body_end = len(code)
    for _ in range(8): w(0x0009)
    # --- epilogue: push everything below r15
    for reg in range(15): w(0x2f06 | (reg << 4))
    w(0x4f02, 0x4f12, 0x4f22, 0x4f13)            # sts.l mach/macl/pr, stc.l gbr
    w(0x0029, 0x2f06)                            # movt r0 ; push
    w(0x4f52, 0x4f62)                            # sts.l fpul / fpscr
    w(0xe000, 0x406a)                            # fpscr = 0
    for i in range(16): w(0xff0b | (i << 4))     # fmov.s fr(i),@-r15
    w(0xfbfd)
    for i in range(16): w(0xff0b | (i << 4))
    movl(4, DUMP_TOP); w(0x65f3, 0x6643, 0x36f8) # r5=r15 ; r6=r4 ; r6-=r15
    w(0xe401, 0xe304, 0xc313)                    # write(1, r15, n)
    movl(5, SCRATCH - 0x8000); movl(6, 0x10000)
    w(0xe401, 0xe304, 0xc313)                    # write(1, scratch, 64K)
    w(0xe400, 0xe301, 0xc311)                    # exit(0)
    for _ in range(600): w(r.getrandbits(16))    # landing zone for pc-relative reads
    return code, lits, (body_start, body_end)


def make(seed, length, fpu, dn=False, keep=None):
    """Assemble a test image.  With `keep`, only the first `keep` words of the
    random body are kept and the rest become NOPs without moving anything, which
    lets a failure be bisected."""
    code, lits, (b0, b1) = build(seed, length, fpu, dn)
    def emit(fp_addr):
        out = bytearray()
        for pos, op in enumerate(code):
            dropped = keep is not None and b0 + keep <= pos < b1
            if pos in lits and dropped:
                if len(out) % 4 == 2: out += struct.pack('>H', 0x0009)
                out += struct.pack('>6H', *([0x0009] * 6))
            elif dropped:
                out += struct.pack('>H', 0x0009)
            elif pos in lits:
                reg, val = lits[pos]
                if val == 'FPTBL': val = fp_addr
                if len(out) % 4 == 2: out += struct.pack('>H', 0x0009)
                # mov.l @(4,pc),reg ; bra over literal ; nop ; nop ; .long val
                out += struct.pack('>HHHH', 0xd001 | (reg << 8), 0xa003, 0x0009, 0x0009)
                out += struct.pack('>I', val & 0xffffffff)
            else:
                out += struct.pack('>H', op)
        while len(out) % 4: out += b'\x00\x09'
        return out
    body = emit(0)
    fp_addr = BASE + len(body)
    body = emit(fp_addr)
    r = random.Random(seed ^ 0xf00d)
    for i in range(32):
        v = r.choice(FP_INTERESTING) if r.random() < 0.7 else r.uniform(-1000, 1000)
        body += struct.pack('>f', v)
    assert len(body) < 0x10000, len(body)
    img = bytearray(MEM)
    img[:len(body)] = body
    for off in range(0x18000, 0x28000, 4):       # scratch window gets random contents
        img[off:off+4] = struct.pack('>I', rnd32(r))
    return bytes(img)


def elf(img):
    ehdr = struct.pack('>4sBBBB8xHHIIIIIHHHHHH', b'\x7fELF', 1, 2, 1, 0, 2, 42, 1, BASE, 52, 0, 0x9, 52, 32, 1, 0, 0, 0)
    phdr = struct.pack('>IIIIIIII', 1, 0x1000, BASE, BASE, len(img), len(img), 7, 0x1000)
    hdr = ehdr + phdr
    return hdr + b'\0' * (0x1000 - len(hdr)) + img


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--count', type=int, default=200)
    ap.add_argument('--len', type=int, default=400)
    ap.add_argument('--seed', type=int, default=1)
    ap.add_argument('--fpu', action='store_true', help='mix in FPU instructions')
    ap.add_argument('--dn', action='store_true', help='also exercise FPSCR.DN (denormals as zero)')
    ap.add_argument('--keep')
    ap.add_argument('--bisect', type=int, metavar='SEED', help='find the shortest failing prefix of one program')
    ap.add_argument('--runner', default=os.path.join(os.path.dirname(__file__), '..', 'target', 'release', 'sh4run'))
    a = ap.parse_args()
    tmp = a.keep or tempfile.mkdtemp(prefix='cpufuzz', dir=os.environ.get('TMPDIR'))
    os.makedirs(tmp, exist_ok=True)
    def run(seed, length, keep=None):
        img = make(seed, length, a.fpu, a.dn, keep)
        binp = os.path.join(tmp, f't{seed}.bin'); elfp = os.path.join(tmp, f't{seed}.elf')
        open(elfp, 'wb').write(elf(img)); os.chmod(elfp, 0o755)
        open(binp, 'wb').write(img)
        q = subprocess.run([QEMU, elfp], capture_output=True, timeout=20)
        m = subprocess.run([a.runner, binp, f'{BASE:x}', f'{MEM:x}'], capture_output=True, timeout=20)
        return q, m
    if a.bisect is not None:
        code, lits, (b0, b1) = build(a.bisect, a.len, a.fpu, a.dn)
        lo, hi = 0, b1 - b0        # invariant: keeping lo body words passes, keeping hi fails
        while hi - lo > 1:
            mid = (lo + hi) // 2
            q, m = run(a.bisect, a.len, mid)
            ok = q.returncode != 0 or (m.returncode == 0 and normalise(q.stdout) == normalise(m.stdout))
            if ok: lo = mid
            else: hi = mid
        q0, _ = run(a.bisect, a.len, lo)
        q, m = run(a.bisect, a.len, hi)
        print(f'seed {a.bisect}: diverges once body word {hi - 1} (0-based) is included')
        show = lambda i: f'{code[i]:04x}' if i not in lits else f'<r{lits[i][0]}={lits[i][1]:#x}>'
        print('   body words up to it:', ' '.join(show(i) for i in range(max(b0, b0 + hi - 10), b0 + hi)))
        print('   state before (qemu):')
        dump(q0.stdout)
        print('   differences after:')
        report(normalise(q.stdout), normalise(m.stdout))
        return
    bad = 0; nan = 0
    for k in range(a.count):
        seed = a.seed + k
        q, m = run(seed, a.len)
        if q.returncode != 0:
            print(f'seed {seed}: qemu rc={q.returncode} {q.stderr[:60]!r} (skipped)'); continue
        qo, mo = normalise(q.stdout), normalise(m.stdout)
        if m.returncode != 0 or mo != qo:
            if m.returncode == 0 and has_nan(q.stdout):
                nan += 1
                continue
            bad += 1
            print(f'seed {seed}: MISMATCH rc={m.returncode} {m.stderr.decode(errors="replace")[:120]}')
            report(qo, mo)
            if bad >= 5: break
    print(f'{a.count} programs, {bad} mismatches, {nan} not compared because a NaN was produced (files in {tmp})')
    sys.exit(1 if bad else 0)


NAMES = ([f'xf{i}' for i in range(15, -1, -1)] + [f'fr{i}' for i in range(15, -1, -1)] + ['fpscr', 'fpul', 'T', 'gbr', 'pr', 'macl', 'mach'] + [f'r{i}' for i in range(14, -1, -1)])


FPSCR_OFF = 32 * 4


def normalise(out):
    """FPSCR cause/flag bits are not modelled; compare only the mode bits."""
    if len(out) < FPSCR_OFF + 4: return out
    v = struct.unpack_from('>I', out, FPSCR_OFF)[0] & 0x003c0003
    return out[:FPSCR_OFF] + struct.pack('>I', v) + out[FPSCR_OFF + 4:]


def has_nan(out):
    """True if any FP register in the reference dump holds a NaN.  QEMU propagates
    NaN payloads and converts NaN to integer differently from the hardware manual,
    so such runs are not comparable."""
    for i in range(32):
        v = struct.unpack_from('>I', out, i * 4)[0]
        if (v & 0x7f800000) == 0x7f800000 and (v & 0x007fffff): return True
        if i % 2 == 0 and (v & 0x7ff00000) == 0x7ff00000: return True
    return False


def dump(out):
    vals = {name: struct.unpack_from('>I', out, i * 4)[0] for i, name in enumerate(NAMES)}
    for group in ([f'r{i}' for i in range(15)], ['mach', 'macl', 'pr', 'gbr', 'T', 'fpul', 'fpscr'], [f'fr{i}' for i in range(16)], [f'xf{i}' for i in range(16)]):
        print('     ' + ' '.join(f'{k}={vals[k]:08x}' for k in group))


def report(q, m):
    nreg = len(NAMES) * 4
    for i, name in enumerate(NAMES):
        a = q[i*4:i*4+4]; b = m[i*4:i*4+4]
        if a != b: print(f'    {name}: qemu={a.hex()} ours={b.hex()}')
    if q[nreg:] != m[nreg:]:
        for off in range(nreg, min(len(q), len(m)), 4):
            if q[off:off+4] != m[off:off+4]:
                print(f'    mem[{SCRATCH - 0x8000 + off - nreg:08x}]: qemu={q[off:off+4].hex()} ours={m[off:off+4].hex()}'); break
        if len(q) != len(m): print(f'    output length qemu={len(q)} ours={len(m)}')


if __name__ == '__main__':
    main()
