title: The 192's hardware
order: 20
summary: The boards and chips of the original RNS-E, identified from photographs, with the memory map the firmware uses.
---
Everything below was identified from photographs of an opened European 192 built
in 2007, then checked against what the firmware does with each address. Some of it
was later measured on a North American 192 running on a bench. Where something is
inferred rather than seen or measured, the text says so.

## The main board

The main board carries the processor, both kinds of memory, the graphics chip and the
CAN transceiver. One side holds the large chips; the other holds the flash and most
of the RAM.

![The main board, processor side](/media/192-main-board.jpg "The main board. Centre: the processor under its metal lid. Below it: the Fujitsu graphics chip. Top right: the shielded GPS module. The silk screen reads AISIN AW CO LTD.")

![The main board, memory side](/media/192-main-board-memory.jpg "The other side: the two Fujitsu flash chips (top left, one with a paper label), Elpida SDRAMs and the bus transceivers.")

| Part | Marking | What it is |
|---|---|---|
| IC1 | `76C401AH03BT`, `AISIN AW 0707` | the processor (below) |
| IC6, IC7 | Fujitsu `29XL12DF-70PFV` | two 128 Mbit NOR flash chips: 32 MiB at address 0 |
| IC2–IC5 | Elpida `D45128163G5-A75` | four 128 Mbit SDRAMs: 64 MiB of main memory |
| IC8 | NEC `D442000AGU` | 256 KiB static RAM, kept alive by the car's battery: the settings |
| IC9 | Fujitsu `MB86293` | "Coral LQ" graphics controller |
| IC10 | Hynix `HY57V643220D` | 8 MiB of video memory |
| IC48 | `TJA1054T` | fault-tolerant low-speed CAN transceiver |
| SY1 | `CMA-814A` | the GPS module, beside the aerial's coaxial cable |
| X1 | `19600` | 19.6 MHz crystal; twelve times that is the CPU clock |

### The processor

![The processor, marked 76C401AH03BT AISIN AW](/media/192-cpu.jpg "IC1: a lidded ball-grid package with Aisin AW's own number rather than a Renesas catalogue part.")

The package carries no Renesas name, but the software is unmistakably SH-4 code. It
uses DMA channels that only the SH7750R/SH7751R have, and a unit's processor
identifies itself with version registers of the SH7750 family (PVR `0x04050086`,
read off a real unit). It runs at about **235 MHz**: twelve times the
19.6 MHz crystal, and four times the 58.8 MHz peripheral clock that the OS's 1 ms
tick implies. A unit has since confirmed the clock: CAN frames that a program sent
100 ticks apart arrived 1.00003 s apart on a logic analyser.

No separate companion chip appears anywhere on the board, so the serial channels,
CAN controller and ports are probably in the same package. That is inferred, not
seen.

![One of the two Fujitsu flash chips](/media/192-flash.jpg "IC7, one of the two 16 MiB flash chips.")
![The Coral graphics chip](/media/192-coral.jpg "IC9, the Fujitsu MB86293 “Coral LQ”. It has a drawing engine and layers; the 3D half is unused by what has been emulated so far.")

### Memory map

The firmware talks to everything through a 29-bit physical address space. It is
summarised here; the emulator's source has the full table, with a confidence for
each line.

| Address | Device |
|---|---|
| `0x00000000` | NOR flash, 32 MiB. The first 48 KiB are the [boot ROM](/firmware/#the-boot-rom) |
| `0x07800000` | ATAPI DVD drive (command block, DMA port, control block) |
| `0x07aN0000` | serial channels: radio processor (1), faceplate (3) and a debug port (5) |
| `0x07b00000` | interrupt priority and status |
| `0x07c00000` | CAN controller with the Renesas HCAN2 register layout |
| `0x07d00000` | A/D converter (the display's position, among others) |
| `0x0c000000` | SDRAM, 64 MiB |
| `0x10000000` | battery-backed SRAM, 256 KiB |
| `0x12N00000` | byte-wide ports (direction and data) |
| `0x15000002`, `0x1500004a` | host ports of the two SD card processors |
| `0x16000000` | control registers, among them a latch of output lines |
| `0x17c00000` | GPS baseband |
| `0x18000000` | Coral: video memory, then its host, display and drawing registers |

One register taught a lesson the hard way. The latch at `0x16000004` holds, among
other lines, the **faceplate controller's reset**. The first custom program written
here copied a value that the stock loader writes there. That took the faceplate
controller down with it, and the screen went dark on the real unit. More on this in
[Reverse engineering](/reverse-engineering/#the-first-program-on-a-unit).

### Test pads

![Six test pads under the processor](/media/192-test-pads.jpg "T426–T431 in two rows right under the processor. Six signals is what the SH-4's H-UDI debug port has, which makes them a likely JTAG; that is a guess by count and position.")

Two more pads nearby, each with a series resistor, looked like a debug serial port.
A logic analyser on eight pads during a stock update found no serial data on any of
them, so the loader's log stays out of reach on a real unit. Two other pads turned
out to carry the CAN transceiver's mode lines, which a custom program sets first, so
their rising edge marks the moment that program starts.

## The card board

The two SD slots do not belong to the main processor. They sit on a small board of
their own, on a 50-pin connector, with **two NEC µPD77115A** 16-bit signal
processors, a 22.5792 MHz crystal (512 × 44.1 kHz) and three multiplexers.

![The card board](/media/192-card-board.jpg "The card board: the connector to the main board, the two NEC processors in the middle, the SD slot cage at the bottom.")

![NEC µPD77115A](/media/192-card-processor.jpg "IC2003, one of the two NEC D77115A. Each one has an SD card interface and an audio output.")

The main software downloads a program into each of them at start-up. That program
reads files from the card and decodes MP3 itself, and the main processor just
asks for the files. The part is documented by NEC, so the emulator models the
processor itself (instruction set, host port, timer, SD interface) and runs the
real downloaded program. That means the protocol between the two processors is
the firmware's own, not a guess. A few instruction codes are missing from NEC's
manual and were worked out from that program.

## The GPS module

![The GPS module](/media/192-gps.jpg "SY1, CMA-814A: the receiver's front end in a can.")

There is no complete GPS receiver with a serial port. The module is a front end, and
the GPS software (`GPSLIB-R01.01.01`) runs on the main processor as a set of
operating-system tasks, reading a baseband device at `0x17c00000`. The emulator does
not model it on the 192 yet.

## The radio board

![Grundig's label on the radio board](/media/192-radio-modules.jpg "The radio board is made by Grundig and labelled “Aisin RNS-E EU”. Its two tuner modules sit in the metal cans.")

Made by Grundig. It carries the two tuner modules and an **NEC V850** microcontroller
(`D70F3079AY`), the "radio processor" that the main software talks to over serial
channel 1. It also has a Philips `SAA7709H` car-radio signal processor, where source
selection and volume happen, and an ISD voice recorder chip, presumably for recorded
traffic announcements. The radio processor's program is on the update disc as
Motorola S-records.

## The faceplate controller

![The faceplate controller, a Fujitsu MB90561A](/media/192-faceplate-controller.jpg "The faceplate's own controller: a Fujitsu MB90561A (F2MC-16LX) with its program in mask ROM.")

It scans the keys and knobs, drives the lighting and talks to the main processor
over serial channel 3. Its mask ROM cannot be updated or read from outside. What it
says on the serial line was worked out from the main software and confirmed on a
unit, where every key and both knobs answered with the codes the SDK expects.

## The DVD drive

![The drive's board](/media/192-drive-board.jpg "The drive's underside: Panasonic MN103S26EDC controller (bottom), a chip with Alpine's own marking (top), test points named LOADOUT, PLAYA, PLAYB.")

An Alpine DVD mechanism with a Panasonic controller. On the bench unit the drive
identifies itself over ATAPI as `ALPINE DV33M01B/`. It was measured to read 256
sectors in 1.3–1.5 s, to need 200–280 ms per seek on a small disc, and to be ready
5.65 s after it is told to start. It does **not** spin up a disc by itself after one
is inserted; the software has to tell it to. All of that is now in the emulator's
drive model.

## Related

- [The 193's hardware](/hardware-193/): the 2010 unit, which works quite differently
- [Firmware and update discs](/firmware/): what runs on all this
- [Anti-theft PIN](/anti-theft/): how the 192 and 193 store and verify the four-digit PIN
- [Reverse engineering](/reverse-engineering/): how the numbers above were measured
