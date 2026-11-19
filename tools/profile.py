#!/usr/bin/env python3
"""Add up a profile from `rnse ... --profile FILE` by function.

    tools/profile.py PROGRAM.elf PROFILE [--top N] [--nm NM]

PROGRAM.elf is the program as sdk/build.sh links it (sdk/out/NAME-192.elf).
The function an address belongs to is the nearest text symbol at or below
it, as `nm -n` lists them; NM defaults to sh4-linux-gnu-nm (with
sdk/tools/sh4-docker on the PATH, the one in the container).  Samples outside
the ELF's symbols (the unit's own software, the boot ROM) are counted as
"(elsewhere)".
"""
import argparse
import bisect
import subprocess
import sys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('elf')
    ap.add_argument('profile')
    ap.add_argument('--top', type=int, default=30)
    ap.add_argument('--nm', default='sh4-linux-gnu-nm')
    a = ap.parse_args()

    out = subprocess.run([a.nm, '-n', a.elf], capture_output=True, text=True, check=True).stdout
    syms = []
    for line in out.splitlines():
        f = line.split()
        if len(f) == 3 and f[1] in 'tT':
            syms.append((int(f[0], 16), f[2]))
    syms.sort()
    starts = [s[0] for s in syms]
    end = syms[-1][0] + 0x10000 if syms else 0

    total = 0
    by = {}
    for line in open(a.profile):
        pc, n = line.split()
        pc, n = int(pc, 16), int(n)
        total += n
        # The program runs from P1; its symbols are at the same addresses.
        i = bisect.bisect_right(starts, pc) - 1
        name = syms[i][1] if i >= 0 and pc < end else '(elsewhere)'
        by[name] = by.get(name, 0) + n
    if not total:
        sys.exit('no samples')
    print('%6s %8s  %s' % ('%', 'samples', 'function'))
    for name, n in sorted(by.items(), key=lambda kv: -kv[1])[:a.top]:
        print('%6.2f %8d  %s' % (100.0 * n / total, n, name))
    print('%6s %8d  (all)' % ('', total))


if __name__ == '__main__':
    main()
