#!/usr/bin/env python3
"""
Build a bootable RNS-E 193 (V575) firmware update ISO with drive-agnostic PIN decoding (Approach A1).

Patches V575 OSM.BIN so that when the stock PIN decoder (0x8058cc7e) fails to decrypt
flash 0x03ff403a with the currently installed DVD drive's SCSI INQUIRY key (0x8e9de900),
it falls back to sweeping "DV33M01B 0000   ".."DV33M01B 9999   " (~10,000 keys) using
OSM's own AES-128 decrypt (0x80681196) and 14-byte serial verification (0x806800e8).

- Preserves the unit's real factory PIN and anti-theft lock.
- Never touches the 0x03fe0000 factory parameter sector.
- Installed by the unmodified stock 193 LDM update loader (`rnse repack`).
- Bumps V575 SOFIDX by +1 in the middle two digits (e.g. 0230 -> 0240, 0320 -> 0330).

Usage:
    ./patch_osm_drive_swap.py <stock.iso> <out.iso> [--index NNNN] [--rnse PATH]
"""

from __future__ import annotations

import argparse
import os
import shutil
import struct
import subprocess
import sys
import tempfile

BASE_ADDR = 0x80400200
CAVE_ADDR = 0x80400400  # 656 bytes of dead VBR+0x170..0x3ff alignment padding (0x80400400..0x8040068f)
CAVE_MAX_SIZE = 0x200

HOOK_ADDR = 0x8058CDCA  # failure entry of 0x8058cc7e
# Exact original bytes at 0x8058cdca..0x8058cdd4 in V5751050 (0230) and V57510AA (0320):
#   e400 65b3 e044 0f46 64f3
EXPECTED_HOOK_BYTES = bytes.fromhex("e40065b3e0440f4664f3")

ADDR_AES_DECRYPT = 0x80681196
ADDR_MEMCMP = 0x806800E8
ADDR_MEMCPY = 0x804DAF56
ADDR_FPSCR_MASK = 0xFFE7FFFF
ADDR_DRIVE_ID = 0x8E9DE900
ADDR_OK_RESUME = 0x8058CD12
ADDR_ERR_RESUME = 0x8058CDF2


class Sh4Asm:
    def __init__(self, base: int):
        self.base = base
        self.words: list[int] = []
        self.labels: dict[str, int] = {}
        self.branch_fixups: list[tuple[int, str, str]] = []  # (idx, kind, label)
        self.lit32_fixups: list[tuple[int, int]] = []  # (idx, value)
        self.lit16_fixups: list[tuple[int, int]] = []  # (idx, value)

    def pc(self) -> int:
        return self.base + len(self.words) * 2

    def label(self, name: str):
        self.labels[name] = len(self.words)

    def emit(self, w: int):
        self.words.append(w & 0xFFFF)

    def mov_reg(self, rm: int, rn: int):
        self.emit(0x6003 | (rn << 8) | (rm << 4))

    def mov_imm(self, imm: int, rn: int):
        assert -128 <= imm <= 127
        self.emit(0xE000 | (rn << 8) | (imm & 0xFF))

    def add_imm(self, imm: int, rn: int):
        assert -128 <= imm <= 127
        self.emit(0x7000 | (rn << 8) | (imm & 0xFF))

    def mov_l_lit(self, val: int, rn: int):
        idx = len(self.words)
        self.emit(0xD000 | (rn << 8))
        self.lit32_fixups.append((idx, val & 0xFFFFFFFF))

    def mov_w_lit(self, val: int, rn: int):
        idx = len(self.words)
        self.emit(0x9000 | (rn << 8))
        self.lit16_fixups.append((idx, val & 0xFFFF))

    def mov_l_load_postinc(self, rm: int, rn: int):
        self.emit(0x6006 | (rn << 8) | (rm << 4))

    def mov_l_store(self, rm: int, rn: int, disp_bytes: int = 0):
        if disp_bytes == 0:
            self.emit(0x2002 | (rn << 8) | (rm << 4))
        else:
            assert disp_bytes % 4 == 0 and 0 <= disp_bytes // 4 <= 15
            self.emit(0x1000 | (rn << 8) | (rm << 4) | (disp_bytes // 4))

    def mov_w_load_r0(self, rm: int, rn: int):
        self.emit(0x000D | (rn << 8) | (rm << 4))

    def extu_w(self, rm: int, rn: int):
        self.emit(0x600D | (rn << 8) | (rm << 4))

    def mov_b_load(self, rm: int, rn: int):
        self.emit(0x6000 | (rn << 8) | (rm << 4))

    def mov_b_store(self, rm: int, rn: int):
        self.emit(0x2000 | (rn << 8) | (rm << 4))

    def jsr(self, rn: int):
        self.emit(0x400B | (rn << 8))

    def jmp(self, rn: int):
        self.emit(0x402B | (rn << 8))

    def nop(self):
        self.emit(0x0009)

    def tst(self, rm: int, rn: int):
        self.emit(0x2008 | (rn << 8) | (rm << 4))

    def cmp_hs(self, rm: int, rn: int):
        # T = (unsigned rn >= unsigned rm)
        self.emit(0x3002 | (rn << 8) | (rm << 4))

    def cmp_gt(self, rm: int, rn: int):
        # T = (signed rn > signed rm)
        self.emit(0x3007 | (rn << 8) | (rm << 4))

    def dt(self, rn: int):
        self.emit(0x4010 | (rn << 8))

    def bt(self, label: str):
        self.branch_fixups.append((len(self.words), "bt", label))
        self.emit(0x8900)

    def bf(self, label: str):
        self.branch_fixups.append((len(self.words), "bf", label))
        self.emit(0x8B00)

    def bf_s(self, label: str):
        self.branch_fixups.append((len(self.words), "bf/s", label))
        self.emit(0x8F00)

    def bra(self, label: str):
        self.branch_fixups.append((len(self.words), "bra", label))
        self.emit(0xA000)

    def finish(self) -> bytes:
        # Resolve branches
        for idx, kind, lbl in self.branch_fixups:
            target = self.labels[lbl]
            disp = target - (idx + 2)
            if kind == "bra":
                assert -2048 <= disp <= 2047, f"bra {lbl} out of range: {disp}"
                self.words[idx] |= disp & 0xFFF
            else:
                assert -128 <= disp <= 127, f"{kind} {lbl} out of range: {disp}"
                self.words[idx] |= disp & 0xFF

        # Emit 16-bit literals (deduplicated)
        pool16: dict[int, int] = {}
        for idx, val in self.lit16_fixups:
            if val not in pool16:
                pool16[val] = len(self.words)
                self.emit(val)
            lit_idx = pool16[val]
            disp = lit_idx - (idx + 2)
            assert 0 <= disp <= 255, f"mov.w literal out of range: {disp}"
            self.words[idx] |= disp

        # Align to 4 bytes for 32-bit literal pool
        if (self.base + len(self.words) * 2) % 4 != 0:
            self.nop()

        pool32: dict[int, int] = {}
        for idx, val in self.lit32_fixups:
            if val not in pool32:
                pool32[val] = self.base + len(self.words) * 2
                self.emit((val >> 16) & 0xFFFF)
                self.emit(val & 0xFFFF)
            lit_addr = pool32[val]
            pc = self.base + idx * 2
            ea_base = (pc & ~3) + 4
            assert (lit_addr - ea_base) % 4 == 0
            disp = (lit_addr - ea_base) // 4
            assert 0 <= disp <= 255, f"mov.l literal out of range: {disp}"
            self.words[idx] |= disp

        return b"".join(struct.pack(">H", w) for w in self.words)


def build_cave() -> bytes:
    a = Sh4Asm(CAVE_ADDR)

    # 1) Initialize r15+0x20..0x2f to "DV33M01B 0000   "
    a.mov_reg(15, 4)
    a.add_imm(0x20, 4)
    a.mov_l_lit(0x44563333, 0)  # "DV33"
    a.mov_l_store(0, 4, 0)
    a.mov_l_lit(0x4D303142, 0)  # "M01B"
    a.mov_l_store(0, 4, 4)
    a.mov_l_lit(0x20303030, 0)  # " 000"
    a.mov_l_store(0, 4, 8)
    a.mov_l_lit(0x30202020, 0)  # "0   "
    a.mov_l_store(0, 4, 12)

    # 2) Load function pointers and iteration counter (r11, r13, r14 saved by 0x8058cc7e)
    a.mov_l_lit(ADDR_AES_DECRYPT, 11)
    a.mov_l_lit(ADDR_MEMCMP, 13)
    a.mov_w_lit(10000, 14)

    a.label("loop")
    # AES128_decrypt(ct=r15+0x00, out=r15+0x10, key=r15+0x20)
    a.mov_reg(15, 4)
    a.mov_reg(15, 5)
    a.add_imm(0x10, 5)
    a.mov_reg(15, 6)
    a.jsr(11)
    a.add_imm(0x20, 6)  # delay slot

    # memcmp(serial=r15+0x30, out+2=r15+0x12, 14)
    a.mov_reg(15, 4)
    a.add_imm(0x30, 4)
    a.mov_reg(15, 5)
    a.add_imm(0x12, 5)
    a.jsr(13)
    a.mov_imm(14, 6)  # delay slot

    a.tst(0, 0)
    a.bf("next")

    # Serial matched! Verify u16_be(r15+0x10) < 10000
    a.mov_imm(0x10, 0)
    a.mov_w_load_r0(15, 1)
    a.extu_w(1, 1)
    a.mov_w_lit(10000, 2)
    a.cmp_hs(2, 1)  # T = (pin >= 10000)
    a.bt("next")

    # Match! Cache the winning 16-byte key into DRAM 0x8e9de900 so all subsequent
    # calls to 0x8058cc7e succeed immediately on the 1-iteration stock fast path.
    a.mov_reg(15, 4)
    a.add_imm(0x20, 4)
    a.mov_l_lit(ADDR_DRIVE_ID, 5)
    a.mov_l_load_postinc(4, 0)
    a.mov_l_store(0, 5, 0)
    a.mov_l_load_postinc(4, 0)
    a.mov_l_store(0, 5, 4)
    a.mov_l_load_postinc(4, 0)
    a.mov_l_store(0, 5, 8)
    a.mov_l_load_postinc(4, 0)
    a.mov_l_store(0, 5, 12)

    # Restore r13=memcpy, r14=fpscr_mask and resume stock formatter at 0x8058cd12
    a.mov_l_lit(ADDR_MEMCPY, 13)
    a.mov_l_lit(ADDR_FPSCR_MASK, 14)
    a.mov_l_lit(ADDR_OK_RESUME, 0)
    a.jmp(0)
    a.nop()

    a.label("next")
    a.dt(14)
    a.bt("fail")

    # Increment ASCII digits at r15+0x2c down to r15+0x29
    a.mov_reg(15, 4)
    a.add_imm(0x2C, 4)
    a.mov_imm(0x39, 5)  # '9'
    a.mov_imm(0x30, 6)  # '0'
    a.label("inc")
    a.mov_b_load(4, 0)
    a.add_imm(1, 0)
    a.cmp_gt(5, 0)  # T = (r0 > '9')
    a.bf_s("loop")
    a.mov_b_store(0, 4)  # delay slot
    a.mov_b_store(6, 4)  # wrap to '0'
    a.bra("inc")
    a.add_imm(-1, 4)  # delay slot

    a.label("fail")
    a.mov_l_lit(ADDR_ERR_RESUME, 0)
    a.jmp(0)
    a.nop()

    code = a.finish()
    assert len(code) <= CAVE_MAX_SIZE, f"cave too large: {len(code)}"
    return code


def build_trampoline() -> bytes:
    # At 0x8058cdca (2 mod 4):
    #   0x8058cdca: d001       mov.l @(0x8058cdd0), r0  ; (0x8058cdc8)+4+4*1 = 0x8058cdd0
    #   0x8058cdcc: 402b       jmp @r0
    #   0x8058cdce: 0009       nop
    #   0x8058cdd0: 80400400   .long CAVE_ADDR
    return struct.pack(">HHHI", 0xD001, 0x402B, 0x0009, CAVE_ADDR)


def patch_osm(osm_path: str):
    data = bytearray(open(osm_path, "rb").read())
    load = struct.unpack(">I", data[0x70:0x74])[0]
    size = struct.unpack(">I", data[0x74:0x78])[0]
    if 0x80000000 + load != BASE_ADDR:
        raise RuntimeError(f"Unexpected OSM load address: {load:#x}")

    cave_off = CAVE_ADDR - BASE_ADDR
    hook_off = HOOK_ADDR - BASE_ADDR
    cave = build_cave()
    tramp = build_trampoline()

    if cave_off + len(cave) > size or hook_off + len(tramp) > size:
        raise RuntimeError("Patch outside module checksummed size")

    if any(b != 0 for b in data[cave_off : cave_off + len(cave)]):
        raise RuntimeError(f"Cave at {CAVE_ADDR:#x} is not all zeros — refusing to patch")

    if bytes(data[hook_off : hook_off + len(tramp)]) != EXPECTED_HOOK_BYTES:
        got = bytes(data[hook_off : hook_off + len(tramp)]).hex()
        raise RuntimeError(
            f"Unexpected bytes at {HOOK_ADDR:#x}: got {got}, expected {EXPECTED_HOOK_BYTES.hex()}"
        )

    data[cave_off : cave_off + len(cave)] = cave
    data[hook_off : hook_off + len(tramp)] = tramp

    with open(osm_path, "wb") as f:
        f.write(data)
    print(f"Patched {osm_path}: cave={len(cave)}B @ {CAVE_ADDR:#x}, trampoline={len(tramp)}B @ {HOOK_ADDR:#x}")


def bump_v575_sofidx(iso_path: str, new_index: str | None) -> tuple[str, str]:
    """Find V575's 575INDEX.TXT inside the ISO and bump SOFIDX by +10 (or to new_index)."""
    data = bytearray(open(iso_path, "rb").read())
    # Locate 575INDEX.TXT via ISO-9660 root directory
    pvd = data[16 * 2048 : 17 * 2048]
    if pvd[1:6] != b"CD001":
        raise RuntimeError("Not an ISO-9660 image")
    root_lba = struct.unpack("<I", pvd[156 + 2 : 156 + 6])[0]
    root_len = struct.unpack("<I", pvd[156 + 10 : 156 + 14])[0]
    root = data[root_lba * 2048 : root_lba * 2048 + root_len]

    entry_lba = None
    entry_len = None
    i = 0
    while i < len(root):
        rec_len = root[i]
        if rec_len == 0:
            next_sec = ((i // 2048) + 1) * 2048
            i = next_sec
            continue
        name_len = root[i + 32]
        name = root[i + 33 : i + 33 + name_len].decode("ascii", errors="ignore").split(";")[0]
        if name.upper() == "575INDEX.TXT":
            entry_lba = struct.unpack("<I", root[i + 2 : i + 6])[0]
            entry_len = struct.unpack("<I", root[i + 10 : i + 14])[0]
            break
        i += rec_len

    if entry_lba is None:
        raise RuntimeError("575INDEX.TXT not found in ISO root directory")

    start = entry_lba * 2048
    text = data[start : start + entry_len]
    pos = text.find(b"SOFIDX:")
    if pos < 0 or pos + 11 > len(text):
        raise RuntimeError("SOFIDX: not found in 575INDEX.TXT")

    old_idx = bytes(text[pos + 7 : pos + 11]).decode("ascii")
    if new_index is None:
        # Bump middle two digits by +1 (e.g. 0230 -> 0240, 0320 -> 0330)
        mid = int(old_idx[1:3]) + 1
        new_index = f"{old_idx[0]}{mid:02d}{old_idx[3]}"

    if len(new_index) != 4 or not new_index.isdigit():
        raise RuntimeError(f"Invalid SOFIDX: {new_index!r}")

    data[start + pos + 7 : start + pos + 11] = new_index.encode("ascii")
    with open(iso_path, "wb") as f:
        f.write(data)
    return old_idx, new_index


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("stock_iso", help="Stock RNS-E 193 (V575) firmware ISO")
    ap.add_argument("out_iso", help="Output patched ISO path")
    ap.add_argument("--index", help="Override V575 SOFIDX (default: +1 in middle two digits)")
    ap.add_argument(
        "--rnse",
        default=os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "target/release/rnse"),
        help="Path to rnse binary",
    )
    args = ap.parse_args()

    with tempfile.TemporaryDirectory(prefix="rnse-drive-swap-") as tmp:
        ext_dir = os.path.join(tmp, "ext")
        subprocess.run([args.rnse, "extract", args.stock_iso, ext_dir], check=True)
        osm_path = os.path.join(ext_dir, "V575", "OSM.BIN")
        if not os.path.exists(osm_path):
            raise RuntimeError(f"No V575/OSM.BIN in {args.stock_iso}")

        patch_osm(osm_path)

        # Keep only V575/OSM.BIN so repack touches nothing else
        only_dir = os.path.join(tmp, "only")
        os.makedirs(os.path.join(only_dir, "V575"))
        shutil.copy2(osm_path, os.path.join(only_dir, "V575", "OSM.BIN"))

        subprocess.run([args.rnse, "repack", args.stock_iso, only_dir, args.out_iso], check=True)
        old_idx, new_idx = bump_v575_sofidx(args.out_iso, args.index)
        print(f"Updated 575INDEX.TXT SOFIDX: {old_idx} -> {new_idx}")
        subprocess.run([args.rnse, "info", args.out_iso], check=True)


if __name__ == "__main__":
    main()
