title: Firmware and update discs
order: 40
summary: What is on an RNS-E update CD, how the unit decides to update, and the boot ROM that is on no disc.
---
All of the RNS-E's software, apart from a small boot ROM, reaches the unit on
**firmware update CDs**. One disc carries software for several hardware variants,
192 and 193 alike. Everything below was worked out from the discs themselves and
from watching their software run in the emulator.

## What is on a disc

A plain ISO 9660 CD-ROM:

| File | Contents |
|---|---|
| `LOADING.KWI` | all the software for all variants, in one container |
| `LDM.BIN`, `574/LDM.BIN`, … | the update loader for each variant, a second time, outside the container |
| `INDEX.TXT`, `574INDEX.TXT`, … | per variant: the software index `SOFIDX`, navigation and audio versions |
| `00.VNS` | a table naming the loader and program set of each variant |
| `A*.MOT` | Motorola S-records for the radio processor |
| `SDS/` | speech data for voice prompts and recognition, per language |

### The container

`LOADING.KWI` is big-endian. A header gives the number of variants. Then comes a
64-byte summary per variant (name, software version), and then a 256-byte block per
variant listing its modules. Each module record has a name, an offset and a size, in
2 KiB sectors.

| Module | Role |
|---|---|
| `LDM` | the update loader; runs from RAM |
| `OSM` | the real-time kernel (µITRON-style), drivers and most middleware |
| `APM` | the main application: user interface and navigation |
| `AUM`, `APFM` | further application modules |
| `FONT` | the font renderer and fonts |
| `VRM` | voice recognition |

Every module starts with the same 128-byte header: a tag such as `.OS `, a version,
the build date and time, `AISIN AW co.LTD`, then its load address, its size and a
checksum (the sum of every 32-bit word after the header), followed by entry points.
All the modules on the discs examined pass their checksums. Apart from the loader,
modules execute in place from flash.

### Variants seen

| Variant | Hardware | Market | Software index on the discs examined |
|---|---|---|---|
| V220 | 192 | Europe | 0650 |
| V222 | 192 | North America | 0350 |
| V574 | 193 | Europe | 0220 |
| V575 | 193 | North America | 0320 |
| VB48 | 193 family | not known | 0700 |

## How an update starts

You might expect the boot ROM to start updates. It doesn't. The **installed
navigation application** does, when it finds a program disc in the drive:

1. It compares the disc's software index with its own. Only the second and third
   characters count, as text: a unit with `0650` updates from `0660`, but not from
   `0651` or `0640`.
2. It checks that the disc's project code matches its own.
3. It adds up every module of the set on the disc and compares the sums with the
   container's.
4. It copies the `LDM` loader into RAM and calls it, with the screen language as an
   argument.

The loader then erases the flash and writes the new modules, and finally writes a
**module directory** that the OS reads at the next start.

That order has a sharp edge, found by giving the emulated loader a module with a
wrong checksum. **The loader erases first and checks each module only when it comes
to write it.** A bad disc, or an interrupted update, leaves a unit with no software
at all. Getting it back is then up to the boot ROM.

The same discovery made custom software possible. Whatever sits in the loader's place
on a disc, as long as it passes these checks, is started by the unit's own software
with the machine to itself. See [Custom firmware](/custom-firmware/).

## The boot ROM

The first 48 KiB of flash hold a boot ROM. Updates never rewrite it, so **it is on
no disc**. For most of this project the emulator therefore ran a stand-in, built
purely from what the other software expects of it:

- The loader ends an update by jumping to address 0. On a 192 that lands in the
  ROM's reset code, by way of a deliberate address error.
- The loader reads a project code from offset `0x406` of the ROM's header (`V220` on
  the 192) to decide which software set it may install.
- Every module calls ROM services through fixed entry points and `TRAPA`
  instructions.

The stand-in puts a reserved SH-4 opcode (`0xFFFD`) at each entry point. When the
CPU executes it, the emulator stops, performs the service in Rust and resumes. That
was enough to boot all five variants.

Then a real one was read. A small [custom program](/custom-firmware/) running on a
North American 192 on the bench sent the 48 KiB out over the CAN bus, where a logic
analyser captured it. The image came back whole, with the CRC-32 the unit had
computed itself. Run in the emulator in place of the stand-in, the real ROM boots
the installed software to the same picture, with the same log line for line. It
also completes an update and restarts into the new software. It needed two small
fixes to the emulated processor: the refresh timer's flag and the level of the NMI
pin.

What the real ROM showed was then folded back into the stand-in: its header,
its three-instruction reset vector, the bus and port settings, the timers and CAN
mailboxes it leaves running, and the exact answers of its services. Its header reads
`.PRG V220100D Sep  3 2003 11:41:33 AISIN AW co.LTD`.

> The real boot ROM image is firmware. Like the discs, it stays out of the project
> and off this site. The emulator takes it only from a file you supply.

## Changing a module

`rnse repack` writes a copy of a disc with modules replaced, checksums put right.
For example, one menu text changed in `APM` makes a disc that differs from the
original in 42 bytes, and the emulated unit installs it and shows the new text.
That is how the format was confirmed end to end.
