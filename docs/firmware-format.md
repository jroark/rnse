# Firmware update disc and file formats

Everything here was worked out from one disc (volume label
`FW_0220_0650`, three hardware variants).  The parsers are in
`src/fw.rs`.

## Disc layout

A plain ISO 9660 CD-ROM:

| file | contents |
|------|----------|
| `00.VNS` | Text-ish table naming the files below per variant (`LDR` = loader, `PRG` = program set). |
| `LDM.BIN`, `574/LDM.BIN`, `B48/LDM.BIN` | The update loader for each hardware variant.  The boot ROM loads the matching one into RAM and runs it. |
| `LOADING.KWI` | All software for all variants (see below).  Contains the loaders again. |
| `INDEX.TXT`, `574INDEX.TXT`, `B48INDEX.TXT` | Version summary per variant: `SOFIDX` (the software index shown in the unit's version screen), `NAVVER`, `AUDVER`. |
| `A*.MOT` | Motorola S-record images for a second processor ("AudiRnse", the radio/audio controller).  Not examined yet. |
| `SDS/` | Speech dialogue data (prompts, recogniser data) per language. |

On this disc:

| variant | SOFIDX | NAVVER   | AUDVER   |
|---------|--------|----------|----------|
| V220    | 0650   | V2200815 | A2210141 |
| V574    | 0220   | V5741040 | A5740022 |
| VB48    | 0700   | VB481000 | A5740022 |

## LOADING.KWI

Big-endian.  Offsets in bytes.

    0x00   header (not decoded; 0x11 holds the number of variants)
    0x14   one 0x40 byte summary per variant
             +0x04  name, e.g. "V220"
             +0x38  software version, 4 characters, e.g. "0815"
    then   one 0x100 byte block per variant
             +0x04  name
             +0x44  number of modules (16 bit)
             +0x48  module records, 16 bytes each:
                      name[8]  ("LDM", "OSM", "APM", ...)
                      offset   (32 bit, in units of 2 bytes from the
                                start of this variant's data)
                      size     (32 bit, bytes, a multiple of 0x800)
             +0xf8  first sector of this variant's data (5 bytes)
             +0xfd  number of sectors (3 bytes)

Sectors are 0x800 bytes.

## Modules

| name | tag    | role |
|------|--------|------|
| LDM  | `.LDR` | update loader; runs from RAM |
| OSM  | `.OS ` | RTOS kernel, drivers, most middleware |
| APM  | `.APL` | main application (navigation, user interface) |
| AUM  | `.APL` | second application module (not examined yet) |
| APFM | `.APL` | small application support module |
| FONT | `.FNT` | font renderer and fonts |
| VRM  | `.ETC` | voice recognition |
| DIG  | `.ETC` | (V574/VB48 only) |

Every module starts with the same header:

    0x00  tag, 4 characters, then CR LF
    0x06  version, 8 characters, then CR LF
    0x10  build date "Mmm dd yyyy"
    0x20  build time "hh:mm:ss"
    0x30  "AISIN AW co.LTD"
    0x6d  CR LF 0x1a
    0x70  load address of the header itself (32 bit)
    0x74  image size in bytes including the header
    0x78  checksum: sum of the big-endian 32-bit words from 0x80 to the
          end of the image (last word zero-padded)
    0x80  entry points: init 1, init 2, start, (a fourth, unused so far)

All 23 modules on the disc pass the checksum.

Code is SH-4, big-endian.  Modules other than the loader execute in place
from flash.  Load addresses with the top bits set (0x80xxxxxx) are the
same physical location seen through the CPU's P1 window.

### V220 flash layout

| address    | module |
|------------|--------|
| 0x0000000  | boot ROM (not on the disc), six 8 KiB sectors |
| 0x000c000  | module directory, 256 bytes |
| 0x000c100  | OSM |
| 0x0280000  | APM |
| 0x0ae0000  | AUM |
| 0x0dfb000  | APFM |
| 0x0e00000  | not touched by an update: user data (flash file system) |
| 0x1000000  | VRM |
| 0x1180000  | FONT |

The loader erases 0x00c000-0xdfffff and 0x1000000-0x17fffff before
programming.

### Module directory

Written by the loader after a successful update, read by the OS at
start-up to verify and initialise the other modules:

    0x00  ".OS "  version[8]  address(32)     the OS module itself
    0x10  "00 0815"       project code, space, software version
    0x1c  "0650"          SOFIDX
    0x20  NAVVER[8]  AUDVER[8]
    0x30  ".IDX"
    0x5c  number of further modules (32 bit)
    0x60  tag[4] version[8] address(32)       one record per module,
                                              in the order APM, APFM,
                                              AUM, FONT, VRM

`fw::module_directory` reproduces it; the test in `tests/disc.rs` compares
it with what the emulated loader writes.
