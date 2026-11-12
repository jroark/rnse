#!/usr/bin/env python3
"""Find the first instruction where the core diverges from QEMU.

    tools/cpudiff.py <test.elf> <test.bin>

(files as left behind by `cpufuzz.py --keep DIR`)
"""
import os, re, subprocess, sys, struct
elf, binp = sys.argv[1], sys.argv[2]
runner = os.path.join(os.path.dirname(__file__), '..', 'target', 'release', 'sh4run')
q = subprocess.run(['qemu-sh4eb-static', '-one-insn-per-tb', '-d', 'cpu,nochain', elf], capture_output=True).stderr.decode()
m = subprocess.run([runner, binp, '400000', '40000'], capture_output=True, env=dict(os.environ, SH4RUN_TRACE='1')).stderr.decode()
qs = []
for blk in re.split(r'(?=^pc=)', q, flags=re.M):
    kv = dict((k, int(v, 16)) for k, v in re.findall(r'(\w+)=0x([0-9a-f]+)', blk))
    if 'pc' in kv: qs.append(kv)
ms = [dict((k, int(v, 16)) for k, v in re.findall(r'(\w+)=([0-9a-f]+)', l)) for l in m.splitlines() if l.startswith('pc=')]
img = open(binp, 'rb').read()
keys = ['sr', 'pr', 'gbr', 'fpul'] + [f'r{i}' for i in range(16)]
j = 0; prev = None
for e in qs[1:]:
    # Delay-slot instructions are not traced separately by our runner.
    k = j
    while k < len(ms) and k < j + 3 and ms[k]['pc'] != e['pc']: k += 1
    if k >= len(ms) or ms[k]['pc'] != e['pc']: continue
    j = k
    o = ms[j]
    diff = [(k, e[k], o[k]) for k in keys if (e[k] ^ o[k]) & (0x303 if k == 'sr' else 0xffffffff)]
    if diff:
        print('state differs on entry to pc=%08x' % e['pc'])
        for k, a, b in diff: print(f'   {k}: qemu={a:08x} ours={b:08x}')
        lo = max(0, j - 6)
        for t in ms[lo:j]:
            off = t['pc'] - 0x400000
            op = struct.unpack_from('>H', img, off)[0]
            n, mm = (op >> 8) & 15, (op >> 4) & 15
            print('   executed %08x: %04x   r0=%08x rn=%08x rm=%08x sr=%03x mach=%08x macl=%08x' % (t['pc'], op, t['r0'], t[f'r{n}'], t[f'r{mm}'], t['sr'] & 0x3ff, t['mach'], t['macl']))
        break
    j += 1
else:
    print('no divergence in integer state')
