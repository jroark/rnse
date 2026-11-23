#!/usr/bin/env python3
"""Edit a battery-backed SRAM image for the 193 emulator.

Works on the 256 KiB image file that `rnse <cmd> --nvram FILE` loads
into the emulated unit's battery-backed memory (phys 0x18000000; see
docs/hardware-193.md, "Battery-backed SRAM").  It is a file editor for
the emulator: it does not talk to a real unit.

  nvram.py create FILE                       a fresh image, all zero
  nvram.py set FILE OFF VAL [--size 1|2|4]    write VAL at offset OFF
                                              (OFF and VAL in hex)
  nvram.py fill FILE OFF LEN BYTE             set LEN bytes to BYTE
  nvram.py car-type FILE N                     code the unit for car type
                                              N (1-7): the coding block
  nvram.py show FILE                           the header, counters and
                                              coding block
  nvram.py dump FILE OFF LEN                    a hex dump of a range

Offsets are into the image (0 = phys 0x18000000).  The image is 256
KiB; an offset past that is an error.
"""
import sys

SIZE = 0x40000
CODING = 0x2c4c  # the workshop coding block (docs/hardware-193.md)


def load(path):
    with open(path, "rb") as f:
        d = bytearray(f.read())
    if len(d) != SIZE:
        sys.exit("%s is %#x bytes, not a %#x-byte SRAM image" % (path, len(d), SIZE))
    return d


def save(path, d):
    with open(path, "wb") as f:
        f.write(d)


def num(s):
    return int(s, 16)


def check(off, length):
    if off < 0 or off + length > SIZE:
        sys.exit("offset %#x..%#x is outside the %#x-byte image" % (off, off + length, SIZE))


def set_val(d, off, val, size):
    check(off, size)
    d[off:off + size] = val.to_bytes(size, "big")


def car_type(d, n):
    # As the OS expects it (mirrors Machine::set_car_type): mark,
    # version, a checksum of the bytes that count, the type, inverted
    # mark.  A type outside 1-7 is not something a unit is coded with.
    if not (1 <= n <= 7):
        sys.exit("car types are 1 to 7")
    block = bytearray(64)
    block[0:4] = (0x12345678).to_bytes(4, "big")
    block[4:8] = (0x02220001).to_bytes(4, "big")
    block[0x16] = n
    block[8:12] = sum(block[0x0c:0x1c]).to_bytes(4, "big")
    block[60:64] = (0xEDCBA987).to_bytes(4, "big")
    d[CODING:CODING + 64] = block


def show(d):
    print("header        0x0000:", d[0:0x20].hex())
    print("counters      0x1400:", d[0x1400:0x1404].hex())
    mark = int.from_bytes(d[CODING:CODING + 4], "big")
    if mark == 0x12345678 and int.from_bytes(d[CODING + 60:CODING + 64], "big") == 0xEDCBA987:
        print("coding        0x2c4c: car type %d (checksum %#010x)" % (d[CODING + 0x16], int.from_bytes(d[CODING + 8:CODING + 12], "big")))
    else:
        print("coding        0x2c4c: not set (uncoded unit)")


def dump(d, off, length):
    check(off, length)
    for i in range(off, off + length, 16):
        row = d[i:i + 16]
        text = "".join(chr(b) if 32 <= b < 127 else "." for b in row)
        print("%06x  %-48s %s" % (i, row.hex(" "), text))


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__)
    cmd, path = argv[0], argv[1]
    if cmd == "create":
        save(path, bytearray(SIZE))
        print("wrote a fresh %#x-byte image to %s" % (SIZE, path))
        return
    d = load(path)
    if cmd == "set":
        size = 4
        args = argv[2:]
        if "--size" in args:
            k = args.index("--size")
            size = int(args[k + 1])
            args = args[:k] + args[k + 2:]
        set_val(d, num(args[0]), num(args[1]), size)
    elif cmd == "fill":
        off, length, byte = num(argv[2]), num(argv[3]), num(argv[4])
        check(off, length)
        for i in range(off, off + length):
            d[i] = byte & 0xff
    elif cmd == "car-type":
        car_type(d, int(argv[2]))
    elif cmd == "show":
        show(d)
        return
    elif cmd == "dump":
        dump(d, num(argv[2]), num(argv[3]))
        return
    else:
        sys.exit(__doc__)
    save(path, d)
    print("updated %s" % path)


if __name__ == "__main__":
    main(sys.argv[1:])
