#!/usr/bin/env python3
"""Locate telephone strings, pointer references and task records in an OSM.

Usage: tools/telephone_probe.py out/V222/OSM.BIN out/V575/OSM.BIN
Reads local firmware only; prints addresses, not firmware contents. No
third-party packages or version-specific addresses are required.
"""
import argparse
from pathlib import Path
import struct


def probe(path):
    data = path.read_bytes()
    if len(data) < 0x80 or not data.startswith(b".OS "):
        raise ValueError(f"{path}: expected an OSM module with its 0x80-byte header")
    base, size = struct.unpack_from(">II", data, 0x70)
    if size < 0x80 or size > len(data):
        raise ValueError(f"{path}: invalid module size")
    data = data[:size]
    base |= 0x80000000
    version = data[6:14].decode("ascii", errors="replace")
    print(f"{path}: {version}, cached base {base:08x}")
    for string in (b"TEL_TSK", b"TELCAN_TSK", b"^CTEL=1", b"[TEL]: Ring Over",
                   b"/affm/FLASH/tel_phonebook"):
        offset = data.find(string + b"\0")
        if offset < 0:
            print(f"  {string.decode()}: absent")
            continue
        address = base + offset
        refs = [p for p in range(0, len(data) - 3, 2)
                if struct.unpack_from(">I", data, p)[0] == address]
        print(f"  {string.decode()}: {address:08x}; pointers "
              + ", ".join(f"{base + p:08x}" for p in refs))
        if string.endswith(b"_TSK"):
            for p in refs:
                if p + 20 > len(data):
                    continue
                entry, stack, exit_entry = struct.unpack_from(">III", data, p + 8)
                print(f"    task entry {entry:08x}, stack {stack:x}, exit {exit_entry:08x}")
        else:
            # SH-4 mov.l @(disp,PC),Rn points to an aligned literal pool.
            ref_set = set(refs)
            loads = []
            for p in range(0x80, len(data) - 1, 2):
                op = struct.unpack_from(">H", data, p)[0]
                literal = ((base + p + 4) & ~3) + (op & 255) * 4 - base
                if op >> 12 == 0xD and literal in ref_set:
                    loads.append(f"{base + p:08x}")
            print("    PC-relative loads (candidates; data can resemble code): "
                  + ", ".join(loads))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("modules", type=Path, nargs="+")
    args = parser.parse_args()
    for path in args.modules:
        try:
            probe(path)
        except (OSError, ValueError) as error:
            parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
