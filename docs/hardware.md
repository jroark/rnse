# RNS-E hardware notes (V220 variant)

There is no public documentation for this unit.  Everything below comes
from watching what the firmware does in the emulator and from reading its
code, so each item says how sure it is.  "Loader" is the update loader
(`LDM`), "OS" the main software.

## The boards

From photographs of an opened unit (a European 192 built in 2007; the
parts carry date codes from late 2006 to mid 2007).

**Main board** (Aisin AW; silk-screen names in brackets):

| part | marking | what it is |
|------|---------|------------|
| IC1 | `76C401AH03BT`, `AISIN AW 0707`, `EB28120 JAPAN` | the processor: a lidded ball-grid package with Aisin AW's own number, some 29 balls a side by the silk screen, which is not one of the SH7750R's catalogue packages.  No separate companion chip appears in the photographs, so the companion functions (serial channels, CAN controller, ports) are probably in the same package, there being no other large part on either side of the board; not established |
| X1 | `19600` | 19.6 MHz crystal next to IC1.  Twelve times that is the 235.2 MHz taken for the CPU clock, and 58.8 MHz is three times it |
| IC6, IC7 | Fujitsu `29XL12DF-70PFV` | two NOR flash chips of 128 Mbit: the 32 MiB at address 0.  A unit's two identify themselves as manufacturer 0x04, device 0x2222227e, and answer the common flash interface query: a word is written in 16 us as a rule and 512 at most, a sector erased in 256 ms as a rule and 16 s at most, and each chip has eight sectors of 8 KiB at the bottom, 254 of 64 KiB and eight of 8 KiB at the top.  IC6 has a paper label (`00347E`), as parts programmed before assembly do: it is the one at address 0, with the boot ROM in its first 48 KiB |
| IC2-IC5 | Elpida `D45128163G5-A75` | four SDRAMs of 128 Mbit, 16 bits wide: the 64 MiB of RAM |
| IC8 | NEC `D442000AGU` | static RAM, 256 KiB: the battery-backed memory |
| IC9 | Fujitsu `MB86293` | the graphics controller, "Coral LQ" |
| IC10 | Hynix `HY57V643220D` | SDRAM of 64 Mbit, 32 bits wide: 8 MiB of video memory |
| IC38, IC40 | Toshiba `LCX16245A` | 16-bit bus transceivers; IC38 sits at the drive's connector |
| IC48 | `TJA1054T` (as far as it can be read) | fault-tolerant low-speed CAN transceiver |
| SY1 | `CMA-814A` | the GPS receiver, a shielded module with the aerial's coaxial cable beside it |
| X2 | `32.768k` | clock crystal |
| X4 | `14.27x` (the last digit reads 3 or 9) | a crystal beside the graphics chip: its reference clock.  If the chip's clock generator makes 14 times that and the display divides by 25 (the divider the firmware programs), the 508 x 263 dots of a frame take 1/59.8 s |
| T426-T431 | | six test pads in two rows right under IC1.  Six signals is what the SH-4's debug port (H-UDI: TCK, TMS, TDI, TDO, TRST, ASEBRK) has; a guess by number and place |
| T424, T425 | | two test pads with a series resistor each (R89, R90), beside the others.  Taken for a candidate for the debug serial port, on which the loader prints its log; a logic analyser on eight pads found no serial data on any of them during a stock update ("Seen on a real unit" under CAN) |

Connectors are marked `DVD`, `DISP`, `SD`, `GYRO`, `SW` and `POWER`.

**Card board**, on the `SD` connector (50 pins, marked `NAVI` at its
own end): the two card slots, two NEC `D77115A` (IC2002, IC2003), a
22.5792 MHz crystal (512 x 44.1 kHz) and three multiplexers (two
74VHC153, one 74VHC157).  These are the "SD card processors" below.

**Drive**: an Alpine DV35M110 by its label in the photographs; the
drive of another unit names itself `ALPINE  DV33M01B/` with revision
`2912` when asked (INQUIRY; type 5, removable, 96 bytes of which the
first 36 were read).  A DVD mechanism that pulls the disc in
by motor (its board has test points named `LOADOUT`, `PLAYA`, `PLAYB`),
with a Panasonic `MN103S26EDC` controller, a Panasonic `AN8703FH` and a
chip with Alpine's own marking.

**Faceplate**: its controller is a Fujitsu `MB90561A`, a 16-bit
microcontroller of the F2MC-16LX family with its program in mask ROM.

**Radio board**, made by Grundig (labelled "Aisin RNS-E EU"): the two
tuner modules, an NEC `D70F3079AY` (a V850 microcontroller with flash:
the radio processor the main software talks to on channel 1), a
Philips `SAA7709H` car radio signal processor, which is where source
selection and volume happen, and an ISD `4003-04M` voice recorder chip
(for recorded traffic announcements, presumably).

What this settles in the emulator: the flash is two chips with a
command state each and small sectors at the bottom of both (the stock
loader does erase 8 KiB sectors from 0x01000000, as from 0x0000c000);
the battery-backed memory is 256 KiB (and the stock software uses it
up to offset 0x3f630, no further); the CPU clock estimate has a
crystal behind it.  And the card processors turned out to be a
documented part, which is now emulated: see "SD card processors".

## CPU

Hitachi/Renesas SH-4, big-endian.  A unit's processor identifies itself
with PVR 0x04050086, PRR 0x00000200 and CVR 0x20480000, and the
emulator's does the same now (it had an SH7750R's values by the book:
0x04050005, 0x100, 0x35100000).  The firmware uses DMAC channels 4-7
and their interrupt codes (0x780-0x7e0), which exist on the SH7750R and
SH7751R but not on the original SH7750.

* Peripheral clock about 58.8 MHz: the OS loads TMU0 with 14699 at Pclk/4
  for a 1 ms tick, the loader 73499 for 5 ms.  Measured on a unit: a
  program with a tick of 146999 counts at Pclk/4 sent a CAN frame every
  100 ticks, and a logic analyser on the bus had them 1.00003 s apart
  over 18 s ("Seen on a real unit" under CAN).  The reasoning that gave
  the number before: the tick is 1 ms on the 193, where sound gives an
  independent clock (see [hardware-193.md](hardware-193.md), "Clocks and
  time"), the two units' software is the same with every timer value in
  the ratio 58.8 : 50, and 58.8 MHz is also what makes the CAN
  controller's bit timing 100 kbit/s and four times the 14.7 MHz that
  makes the serial channels' divisor 9600 bit/s.  (The emulator used to
  take the tick for 2 ms and the clock for 29.4 MHz.)
* CPU clock: taken as four times that, 235 MHz (the SH7750R is a
  240 MHz part), and the emulator counts one instruction per cycle.
  That is an estimate of the real speed, and a unit has shown how good:
  a loop of 2 000 000 rounds of multiply, add and count took 51.0 ms
  there and takes 59.5 ms here (the processor does two instructions
  in a cycle where it can); adding up a megabyte of words through the
  cache took 8.7 ms there and 7.8 ms here; and 8192 words read past
  the cache took 915 us there, 112 ns a word, against 245 us here,
  where memory costs nothing: the emulator has no times for the bus.
  The processor's clock chip is not running on a unit (RCR2 0, the
  64 Hz counter standing still), and neither is the emulator's now.  The OS's
  calibrated delay routine (0x800ab794 in V2200815) runs 304
  instructions per unit of its argument.
* FPSCR is 0x00040001 in every task: round to zero, denormals as zero.
* The MMU is not enabled by anything run so far.

Caches: the OS writes 0x80008181 to CCR (0xff00001c): both caches on in
the SH7750R's two-way mode with the index bits from the address, stores
through P1 written through to memory, stores through P0, U0 and P3
copy-back.  The update loader never writes CCR and runs with that.  The
application copies the loader to RAM through a P0 address and then uses
`ocbwb` over the range.  `--strict` models the caches
(`src/sh4/cache.rs`).

## Memory map

Physical addresses.  The firmware mostly uses the P1 (0x8...) and P2
(0xa...) windows; the applications also run from P0.

| range                 | device | confidence |
|-----------------------|--------|------------|
| 0x00000000-0x01ffffff | NOR flash, 32 MiB in two chips of 16 MiB, 32-bit data bus, AMD command set | certain (loader programs it; the chips are on the board) |
| 0x07800000            | ATAPI drive, command block (register n at +2n+1, data at +0) | certain |
| 0x07880000            | ATAPI drive, DMA data port | certain |
| 0x07900000            | ATAPI drive, control block (alternate status at +0x0d) | certain |
| 0x07aN0000            | serial channels, 16-bit registers (N = 1, 2, 3, 5, 6, a seen) | 1, 3 and 5 identified, see below |
| 0x07b00000            | interrupt priority and status registers | mostly understood |
| 0x07b80000            | unknown (3 registers written at start-up) | - |
| 0x07c00000            | CAN controller with the Renesas HCAN2 register layout (mailboxes from +0x100, 0x20 bytes each) | emulated as far as the OS uses it |
| 0x07d00000, 0x07d80000 | A/D converter (results at +0..+6, control at +8) | certain |
| 0x07e00000            | unknown | - |
| 0x0c000000-0x0fffffff | SDRAM, 64 MiB | certain (loader heap ends at 0x8fffffff) |
| 0x10000000-0x1003ffff | battery-backed SRAM for settings, 256 KiB | likely (OS reads and writes it heavily; loader reads a "slot number" with complement check at +0, +1) |
| 0x12N00000, 0x12N00001 | byte-wide ports, N = 3..a: direction register (1 = output) and data | see "Ports" |
| 0x12d00000-6          | output latches | bits unknown; 0x12d00001 is driven around CD DMA transfers |
| 0x15000002, 0x1500004a | 16-bit host ports of the two SD card/MP3 processors (NEC µPD77115A) | certain; see "SD card processors" |
| 0x16000000-0x16000020 | 32-bit control registers | 0x16000004 is a latch of output lines; see "The latch at 0x16000004" |
| 0x17c00000-0x17c00014 | GPS baseband, control registers | see "GPS" |
| 0x17e00400, 0x17e00800 | GPS baseband, further register blocks | see "GPS" |
| 0x18000000-0x19fbffff | Coral video memory | certain |
| 0x19fc0000            | Coral host interface registers | certain |
| 0x19fd0000            | Coral display controller registers | certain |
| 0x19ff0000            | Coral drawing engine registers | by the chip's documentation; unused by the loader |

What an access to each part takes was measured on a unit
(`sdk/examples/timing`): 8192 accesses in a loop of five instructions,
of which the loop itself is some 20 ns each.  The emulator has no time
for the bus at all (every one of these takes it 21 ns), so a program
that waits by reading a register waits five to fifteen times as long
on a unit.

| what | ns an access |
|---|---|
| main memory written, uncached | 95 |
| display memory written | 131 |
| a port of the companion chip (0x12300001), the card processors' status (0x12400001) | 171 |
| a register of the graphics chip (host interface, +0x20) | 188 |
| display memory read | 279 |
| the drive's status (0x0790000d) | 308 |
| a register of the CAN controller | 342 |

### Ports

The OS addresses its byte-wide ports through a table of eight (index 0-7
= 0x1230.. 0x12a0, step 0x100000).  What is known of the bits:

| port (data register) | bit | direction | meaning |
|----------------------|-----|-----------|---------|
| 0 (0x12300001) | 0 | out | watchdog strobe, toggled every 10 ms from the tick; and the watchdog is quick: a unit was reset between 105 and 115 ms after the last edge of a strobe that was then left alone (`sdk/examples/suite`).  The emulator resets the unit when, after the first two edges, 105 ms pass without one; it had 1.5 s, and its stock software, which strobes every 10 ms and never pauses for more than about 30 ms once it runs, did not notice the difference |
| 2 (0x12500001) | 6 | in  | switch of the first SD card slot (the one of DSP 0): high with a card in it |
| 2 (0x12500001) | 4 | in  | supply voltage good.  Low for about three seconds and the OS masks all interrupts and waits for the watchdog to reset it |
| 3 (0x12600001) | 4 | out | display motor, towards closed |
| 3              | 3 | out | display motor, towards open |
| 3              | 2 | in  | display position switch: low only when fully open |
| 3              | 1 | in  | polled every tick, unknown |
| 4 (0x12700001) | 7 | in  | polled every tick, unknown |
| 6 (0x12900001) | 0 | in  | switch of the second SD card slot (DSP 1) |
| 7 (0x12a00001) | 3, 2 | out | mode lines of the CAN transceiver: both high = awake.  The OS clears bit 3 and then bit 2 when it lets the bus sleep and sets both at once to wake it (0x801c57f8-0x801c581e in V2220371); the loader sets both.  Both lines are on test pads of the main board and follow these bits on a logic analyser, bit 3 falling some 9 ms before bit 2 at a sleep |

What a unit's eight data registers read with its software running, the
radio on and nothing else going on (`sdk/examples/report`): 0xef, 0x75,
0xbf, 0xe5, 0x3f, 0x61, 0xde, 0xe0 for ports 0 to 7, with the
directions 0x01, 0x80, 0x02, 0x18, 0x80, 0xfe, 0x20, 0x1f.  The
emulator's inputs read those levels (`board::PORT_IDLE`) where no model
moves them; before, an input without a model read 0.  The processor's
own port pins, all inputs, read 0x3d24 (PDTRA) and 0xf97f (PDTRB).

A port has more registers than the two: the card driver writes +2 and
+4 of ports 2 and 6 (0x40 and 0x01 to +2: the bits of the two card
switches, so these select the pins that interrupt).  The emulator
stores them and does nothing with them; it used to take every even
address of a port for its direction register.

### The latch at 0x16000004

A 32-bit register of output lines that reads back.  Known bits: 0 is
the faceplate controller's reset (low = reset), 2 and 3 the resets of
the two card processors, 12 a line the loader's CAN set-up takes low
and, after a pause, high again (0x8df85bf4 in loader V222007A; the OS
never touches it).  0x04, 0x08, 0x10, 0x20 and 0x40 are what the OS
has set once it is up (0x7d in the emulator); a real North American
unit with software 0260, read by a program in the loader's place,
answered 0x00003fff.

**Everybody who writes it reads it first.**  The loader's first write
is `read, or 0x10, write`; started on its own in the emulator, where
the latch is zero, that looks like the value 0x10, and a program that
writes that value on a real unit takes every other line away.  That
was tried: the screen went dark (the faceplate controller, which has
the lighting, in reset), and the stock loader, started afterwards, put
up its picture unseen, sat for 90.6 s without reading the disc and
reset the unit; a stock update that works takes 148 s on the same
unit.  Which of the lost lines the drive needs is not known.  The same
holds for 0x1600001c and 0x16000020, of which the loader's CAN set-up
sets and clears single bits (0x1c: sets 0x1800, clears 0x4000; 0x20:
clears 0x0200, 0x0040 and 0x0030), and for the ports.

## Interrupts

Devices do not use the SH-4's interrupt pins individually.  A priority
encoder in the companion logic drives the encoded IRL0-3 inputs; the
level of each source is programmable, so the interrupt code the CPU sees
(0x200 + (15 - level) * 0x20) depends on the firmware's choice.

Registers at 0x07b00000, 16 bits each:

| offset | contents |
|--------|----------|
| +0x00 .. +0x14 | four 4-bit levels per register; 0 masks the source |
| +0x16 | latched request bits for edge sources; the handler clears its bit by writing 0 |
| +0x1e | enable bits matching +0x16 |

Sources identified:

| source | level field | latch bit | level used |
|--------|-------------|-----------|------------|
| ATAPI INTRQ | +0x00 bits 7:4 | - | 3 |
| display vertical sync | +0x02 bits 11:8 | 6 | 14 |
| Coral drawing engine | +0x02 bits 7:4 | - | 7 |
| serial channel 1 (radio processor) | +0x08 bits 3:0 | - | 2 |
| serial channel 3 (faceplate) | +0x08 bits 11:8 | - | 9 |
| CAN | +0x12 bits 11:8 | - | 5 |
| A/D converter | +0x12 bits 3:0 | - | 11 |

Several sources can share a level.  Their handlers then look at the
32-bit status register at +0x18 to see who is asking: bit 14 = CAN, bit
24 = serial channel 1.  (Bits 2, 13 and 29 are tested too; their sources
are not identified.)

The OS's table of interrupt handlers (vector = interrupt code / 0x20,
level) is at 0x800328c4 in build V2200815.

On-chip sources in use: TMU0 (system tick, level 7 inherited from the
boot ROM), TMU2, DMAC channels 2, 3, 4 and 7.

## Display

Fujitsu Coral graphics controller on chip select 6: an MB86293 ("Coral
LQ") by the marking on the board, with 8 MiB of video memory.  The
firmware's own strings name it ("wait_coral_draw_begin").

* Panel timing as programmed: 400 visible pixels of 508 per line, 235
  visible lines of 263.  The loader's layout tables use 400 x 240 with a
  448 byte line pitch.
* The chip is little-endian internally and sits on a big-endian bus:
  32-bit registers read naturally, 16- and 8-bit registers appear at the
  documented offset XOR 2 or 3, and pixel N of an 8 bpp surface is the
  byte at bus address N XOR 3.  (The loader's pixel-fill routine does
  exactly this XOR.)
* The loader draws by writing pixels straight into video memory (8 bpp,
  frame buffer at 0x110000, shown on the "BL" layer with the palette at
  +0x800) and keeps shadow copies of the display registers that its
  vertical-sync handler flushes to the chip.
* The OS draws with the chip's drawing engine.  It writes display lists
  into video memory and starts them through the host interface (list
  address, count, request); the engine's "done" interrupt paces it.  A
  list is a sequence of 32-bit words, `type << 24 | command << 16 |
  parameter` followed by operands.  The emulator implements the types
  the firmware uses: register writes (0xF1, 0xC0, 0xCE), rectangle fill
  (0x09, 0x41), lines (0x04/0x05, one packed vertex per command), blits
  within and between surfaces (0x0D, 0x0F/0x1F), bitmap drawing (0x0B)
  and the control words.  Triangle set-up (0x40-0x46) is skipped: the 3D
  half of the chip is not emulated.
* A frame takes 16716 us (59.82 Hz): the time from one vertical sync
  flag of the host interface (+0x20, bit 2) to the next on a unit,
  16715 to 16717 in eight, 120 in two seconds, under the SDK's display
  settings, which are the loader's.  The emulator still gives every
  frame a 60th of a second (16667 us) whatever the display registers
  say; the period ought to come from those registers and the dot
  clock, which would also say what the OS's settings make of it.
* Six layers.  The OS runs them in the chip's extended mode: the order
  comes from the layer select register (+0x180), each layer has a
  transparent colour (+0x1a0 + 4n), and bit 0 of its extended mode
  register (+0x110 + 0x10n) confines it to a window given by the four
  registers that follow.  The user interface uses a 16 bpp layer for the
  frame, titles and soft key labels and another, windowed to the middle
  of the screen, for lists.

## CD/DVD drive

Standard ATAPI.  The firmware sends SET FEATURES (transfer mode 0x12),
then PACKET commands with the DMA bit set; data moves through DMAC
channel 1 in 16-bit units from 0x07880000.  The drive is wired so that the
first byte of each pair is the high half of the word.

Registers: the ATA task file as bytes at 0x07800001 + 2n (n = 1 error
and features ... 7 status and command), alternate status and device
control at 0x0790000d, and the 16-bit data register at 0x07880000, the
address the DMA controller reads from.  The loader writes the twelve
bytes of a packet there as six words; the OS also reads the answers to
commands other than disc reads from it, word by word.

The OS does not program the DMA controller itself: before a disc read
it calls three boot ROM services (trap 0x3d, 0201 to 0203, see
`boot-rom.md`) that enable the controller, set channel 1's addresses
and count and write its control register, then raises bit 0 of the
latch at 0x12d00001 and sends the command.

## Serial

Each channel at 0x07aN0000 has 16-bit registers:

| offset | register |
|--------|----------|
| +0x00, +0x02 | mode, baud rate divisor |
| +0x04 | control: 7 = TX interrupt enable, 6 = RX interrupt enable, 5 = transmitter on, 4 = receiver on, 2 = TX-end interrupt enable |
| +0x06 | transmit data |
| +0x08 | status: 9 = TX FIFO full, 8 = TX ready, 6 = RX data ready, 5..3 = receive errors (write 0 to clear), 2 = TX finished |
| +0x0a | receive data |
| +0x0c | FIFO control: 15 = reset RX FIFO, 7 = reset TX FIFO |

Channel 5 is the debug console: the loader's log comes out there (`rnse`
prints it).  Channel 1 goes to the radio processor, channel 3 to the
faceplate.  The North American software uses no other: on a unit,
channels 2 and 4 were as after a reset (divisor 0xff, control 0, FIFO
control 0x1f, status 0x0184) and where channel 0 would be every
register read 0x1fff.  The installed software leaves channel 1 with
control 0x0058, its transmitter off between frames, and with nobody
taking the bytes out its receiver stops at 32 with an error flag
(status 0x01a4) and hears nothing more until that is cleared.

### Radio/audio processor (channel 1)

A second processor handles tuner, amplifier and power: an NEC V850
microcontroller (`D70F3079AY`) on a board of its own, made by Grundig,
with a Philips SAA7709H signal processor for the sound (see "The
boards").  The OS will not
leave its "system software is being checked" notice until this
processor has reported a software version that matches the `AUDVER`
entry of the installed set.

Frames: `start, command, length (16 bit, big-endian), data, checksum`
with start = 0x01 from the unit and 0x00 from the radio, and the
checksum the complement of the sum of everything before it.  The unit
repeats an unanswered frame five times, 600 ms apart.  Command 0x0B
asks for the version; the answer carries 16 bytes of which the first
eight are the version text (`A2210141` for this disc).

The unit sends one command at a time and knows two kinds.  A request
(0x0B, 0x88, 0x8A, 0xD0, 0xD1, 0xD9, 0xDD, 0xDE) is answered by a frame
with the same command number.  A setting (0x80, 0x81, 0x17, 0x19) is
answered by an acknowledgement: command 0x20 with one status byte, zero
for "done"; the unit ignores an acknowledgement with another status and
goes on waiting.  The emulator's stand-in answers accordingly, with
empty data for the requests other than 0x0B.  Answering the settings
with an echo instead, as it first did, made the unit repeat each of
them five times and stretched the start-up by most of a minute.  The firmware also contains a second, FT1.2-style parser
(`68 L L 68 ... CS 16`) for this processor's boot loader.

Volume and source go to this processor too: `19 [01 level]`, level 0
to 30, and `81 [08 source level 00 00 00 00 01 03]`, 3 the tuner, 1 the
CD drive and 4 the card processors' converter.  The tuner goes to a
frequency with `82 [03 02 hi lo]`, in steps of 50 kHz on FM (0x07fa is
102.1 MHz), which a real radio processor acknowledges like a setting.
All three were sent to a unit's by a program of the SDK
(`sdk/examples/player`, `tuner`), with the sound to hear for it: they
are all it wants, from the state the stock software leaves it in.
Asked for its status (0x88) 50 ms after a new frequency it answered
with an acknowledgement that was not zero: busy, presumably.  (The
software sends more under 0x82 that is not understood: `03 04`, and
`09 01 28 01`.)
A real radio processor, asked by a program as its own software asks
(`sdk/examples/suite`), answered the version request with
`A2220141-080807` and a zero byte, and the status request with 57
bytes: `20 10 0c 14 07 fa 3d 0e`, `00 00 00 00`, `23 31 20 66 6f 72 20
20`, which is the text "#1 for  " of the station it was tuned to,
`00 00 00 0a 0a 20 01 65 00 00 00 01 ff`, then from offset 33 `03 07
01 00 00 00 01 00 00 00 00 00 03 00 00 00` (the tuner as source,
volume 7) and eight zero bytes.  The first eight bytes hold what the
tuner is tuned to: given this block the emulated unit shows 102.1 on
its radio screen, which was that unit's station.  The stand-in sends
this block now, with its own source and volume and the text blank; it
had 49 bytes of zeroes around those two.
Its status block, the answer to request 0x88 and also sent unasked,
carries four fields in its first eight bytes (not understood) and
sixteen bytes of amplifier state from offset 33: source, volume, and
fourteen more of which the eighth, while not zero, makes the OS ignore
the volume knob.  The OS will not raise its own volume more than five
steps above the volume reported there, so the volume climbs in steps
of at most five per report.  The stand-in keeps source and volume and
reports them back at once; `--wav` gives the CD drive's sound through
that: silent unless the CD is the source, 2 dB a volume step (the
step is an assumption).  The volume knob's direction bit means
"quieter".  The 193 does all of this the same way.

### Faceplate (channel 3)

Keys, the two rotary encoders and the lighting belong to a controller in
the faceplate.  Protocol, key table and what is still unknown are
described in [`src/dev/panel.rs`](../src/dev/panel.rs).  In short: frames
are `command, L, data, sum` where L is the index of the last byte; each
is answered with 0x15, or with 0x16 and a frame in return.  The
controller announces itself with command 0x0F, after which the unit
configures it (commands 0x02, 0x03, 0x0A, 0x09), sets the lighting (0x06,
repeated while fading, and 0x07) and waits for key state (0x0C, six
bytes, one bit per key) and encoder (0x0D) frames.

The screen's lighting is this controller's.  On a unit, a program that
held the controller in reset had a dark screen, the picture being
there all the while, and the screen lit when the controller was started
and given its settings again.  The emulator shows a black picture
accordingly: from a reset of the controller until a command 0x06 with
a level that is not zero (which of the six levels is the screen's is
not known).  The boot ROM gives the controller its settings before it
starts anything, a loader from a disc included.

The power button is not among the keys: it has to work while the main
processor is off, so it presumably goes to the radio processor.

The controller's reset line is bit 0 of the 32-bit latch at 0x16000004,
low = reset (the other bits the OS sets there, 0x04, 0x08, 0x10, 0x20
and 0x40, are unidentified).  The OS starts with the line low, sets the
port up with its transmitter and receiver off, raises the line and
switches the port on; the controller's 0x0F follows.  Whenever the OS
has sent a command frame it starts a one-second timer that any byte
from the controller cancels; when it runs out the OS prints `fpdrv hard
reset`, pulses the line low for one tick and expects the start-up
again.  (The emulated controller used to answer in no time at all, that
is before the timer was started, which left the timer running after the
last frame and produced such a reset a few seconds after every start.  It now
answers after the time the bytes take on the line.)

Format.  The radio processor's channel has mode 0 (eight bits, no
parity), the faceplate's 0x0011 (a parity bit).  The boot ROM and the
OS set channel 1 the same way: control 0, mode 0, divisor 0x5f, 0 to
+0x0c and +0x0e, 0x0104 to the status, 0x8000 and then 0 to +0x0c,
and control 0x0058.  A unit's radio processor answered nothing that
was sent in the faceplate's format.

Bit rates.  All four serial channels in use have divisor 0x5f (channels
1 and 3 and the loader's use of 5) or 0x1f (the application's use of
channel 5), a ratio of three as between 9600 and 28800, or 38400 and
115200 bit/s.  The 193 gives its faceplate port 0xa1, which is 9600
bit/s from its 50 MHz peripheral clock, so 9600 it is if both
faceplates talk at the same rate; that makes the companion chip's
serial clock 14.7 MHz, a quarter of the 58.8 MHz peripheral clock.
Format on the 193: 8 bits, even parity (the 192's mode value 0x0011
presumably says the same).  The emulator does not model bit rates or
parity beyond delaying the faceplate's answers by ten bit times a byte
at 9600.

## CAN

Telephone-module investigation and the remaining protocol prerequisites
are recorded in [telephone.md](telephone.md).

The unit is a node on the car's infotainment CAN bus (100 kbit/s).  The
controller is register-compatible with Renesas' HCAN2: mailbox n at
+0x100 + 0x20n holds the identifier (standard id << 4 in the first
word), a control word (bits 10:8 = mailbox type, 0 transmit, 2/3
receive; low nibble = length), eight data bytes at +8 and an acceptance
mask at +0x10.

Without the ignition lock status message the power manager (`PWM` in
the debug log: `Now skon[0] Now Kl15[0]`) keeps the unit in standby.
Identifier 0x271, first byte: bit 0 = key in the lock, bit 1 = ignition
on.  (0x2C3 serves the same purpose for a second car type.)  The
emulator sends it every 100 ms while its ignition switch is on.  The
unit's own network management identifier is 0x436.

Bit timing as the loader sets it: BCR0 (+0x06) = 0x000d, BCR1 (+0x04) =
0xf300, that is a prescaler of 14 and 1 + 16 + 4 = 21 quanta a bit.  A
quantum of the HCAN2 is two clock periods times the prescaler, so the
bit rate is the clock divided by 588: 100 kbit/s, the rate of Audi's
infotainment bus, from 58.8 MHz, the peripheral clock.  The loader does not put the controller in reset
before it sets it up, and the OS never starts it at all: the boot ROM
leaves it running, and the loader's BCR writes presumably only repeat
what is there.

Seen on a real unit (a North American 192 on a bench, nothing else on
the bus, a logic analyser on both wires and on eight test pads):

* Alone, the unit repeats its first unacknowledged frame.  After the
  start that is 0x436 `16 02 ..`, `16 01 ..`, then 0x361, and after
  4.7 s the OS lets the bus sleep: the transceiver's two mode lines
  fall (port 7, bits 3 and 2) and nothing the controller is given to
  send after that reaches the wires, although the unit is on and shows
  the radio.  A program that wants to be heard sets the two bits.
* The loader wakes the transceiver itself and sends 0x436 `16 02 00 80
  00 00` and `16 01 ..` in turns for 1.3 s, then `16 04 00 00 00 00`
  until it ends: 148 s for an update that works.
* The SDK's set-up (`sdk/src/hw192.c`, `can_init`) sends: its frames
  are on the wires with the right identifiers, data and check sums.
  The controller's status then reads 0x0026 or 0x0022 and its error
  counters 0x8800, an unacknowledged sender in error-passive state.  A
  frame that is given to the controller right after the one before was
  cancelled does not go out, though: of frames sent back to back,
  each given up after 100 ms, every second one was missing.
* None of the eight pads carries the debug serial port, neither from
  the OS (which prints nothing) nor from a loader.  Two of them are the
  transceiver's mode lines.  Two more change only at a reset: one goes
  low and one high for about 1.5 s, and at a power-up, though not at
  the reset a loader ends with, another carries a burst of 2.4 ms (27
  ms at a cold start).  Which pad is which of T424-T431 was not noted.

The emulated bus carries one frame at a time, each for as long as its
bits take at 100 kbit/s (some 1.3 ms for eight data bytes), so that
frames sent "at the same time" by `--can` arrive one after the other as
on a wire.

## A/D converter and the fold-down display

The display folds down on a motor to uncover the CD drive and the SD
card slots.  Its position comes back through the A/D converter, a
ten-bit, seven-input converter laid out like Hitachi's on-chip ones:
results A-D left-justified at +0, +2, +4, +6, control/status at +8 (bit
15 = done, written 0 to clear; 14 = interrupt enable; 13 = start; 2..0 =
channel).  The OS converts inputs 0-3, then 4-6, on a timer, and
averages eight samples of each (dropping the lowest and highest).

Input 3 is the display position: at most 94 means fully open, at least
894 closed (the OS prints `init_dsp_pos = 1` or `2` accordingly at
start-up; 0 means somewhere in between).  The other inputs are not
identified.  On a unit with the display closed and nothing going on
the seven read 459, 331, 933, 982, 424, 589 and 423, and that is what
the emulator's read now (input 3 by its model: 1023 closed).

This matters more than it seems.  While the display is open the screen
cannot be seen, so the OS shows only the media overview and turns every
key press into a request to close the display.  An emulator that leaves
the converter at 0 therefore gets a unit that ignores its keys.

The motor is driven from port 3 (see "Ports"); the OS watches the
position while it runs and stops when the reading no longer changes.
The key that opens the display is honoured only once the CD drive's
state is known to the OS, which is not the case in the emulator yet.

## Software structure

* A uITRON-style RTOS (`cre_tsk`, `slp_tsk`, ... through the "AVE"
  wrapper) lives in the OS module.  System calls from the application
  modules go through `TRAPA #0x20` to `#0x4c`, one trap number per
  subsystem, function code in r0.
* Task names in the OS: AWG (graphics), CD, FSCD/FSRAM/FSSD (file
  systems), GPSIF, KLINE, KMB (instrument cluster), MP3, PER, RCON, SCR,
  SDCARD, SOUND, TEL, TELCAN, TMC, ...
* The OS inherits a few services from the boot ROM; see
  [boot-rom.md](boot-rom.md).
* Debug output.  The loader prints through serial channel 5.  The OS and
  applications call a C `printf` (0x801eb344 in OS build V2200815) whose
  output never reaches a port in this build; `rnse boot` intercepts the
  call and formats the message from the emulated stack.

## Open ends

* The CD drive is answered for data and audio discs.  The OS's driver
  takes the answers to packet commands from the data register's second
  address, 0x07880000, word by word (the loader reads 0x07800000); disc
  data goes through the DMA controller from the same address.  Audio
  discs are played by the drive: READ TOC format 2, PLAY CD (0xbc) and
  READ SUB-CHANNEL, as on the 193.  What the drive plays can be had as
  sound (`--wav`), as it leaves the drive.  Disc data needs three boot
  ROM services for the DMA controller (see `boot-rom.md`).  The display-
  open key works (the display's position follows the motor outputs) and
  shows the drive and the two card slots.
* The two SD/MP3 processors are emulated and run their downloaded
  program: cards are read and MP3 files play (see "SD card processors").
* The radio processor stand-in knows one command (see above); there is
  no tuner, no sound and no power button.
* Navigation stays at "Navigation wird initialisiert": no GPS receiver
  (see "GPS"), no map DVD.
* The battery-backed SRAM starts empty, so the unit is uncoded
  (`car_model = ffffffff`).
* Interrupt status bits 2, 13 and 29, the ports' remaining bits and the
  devices at 0x07b80000, 0x07e00000 and 0x16000000 are unidentified.

## SD card processors

The two SD card slots and MP3 decoding belong to two processors of
their own on the card board (the firmware calls them DSP 0 and DSP 1):
two NEC µPD77115A, 16-bit fixed-point signal processors of NEC's
µPD77016 family, with a 22.5792 MHz crystal (512 x 44.1 kHz).  The
emulator has a model of the part (`src/dev/upd77115.rs`) and runs the
program the main software downloads into it; `src/dev/media.rs` is the
wiring.  Nothing of the protocol below is answered by the emulator
itself any more: it is what the downloaded program does.

Sources: NEC's "µPD77016 Family Instructions" manual (U13116E) for the
instruction set, the "µPD77111 Family Architecture" manual (U14623E),
chapters 5 to 7, for the µPD77115's memory map, peripherals and boot
procedure, and the µPD77115 data sheet (U14867E).  Where they leave
something open, the answer is from the 192's program and marked so in
the model's comments: the instruction manual gives the fields of an
instruction word but not all of their codes (the numbers of the
monomial and immediate operations, of the register parts in load/store
instructions, of the registers a transfer names).

### Wiring

| Where | What |
|-------|------|
| 0x15000002, 0x1500004a | host data register of DSP 0 / DSP 1, 16 bits |
| 0x12400001 bit 2 / bit 0 | DSP 0 / DSP 1: nothing to read (the part's HRE pin) |
| 0x12400001 bit 3 / bit 1 | DSP 0 / DSP 1: cannot take a word (its HWE pin) |
| 0x16000004 bit 2 / bit 3 | reset of DSP 0 / DSP 1, active low; the driver pulses it |
| 0x07b00016 bit 2 / bit 3 | interrupt request of DSP 0 / DSP 1, latched; cleared by writing 0 |
| 0x07b0001e bit 2 / bit 3 | mask of that request |
| 0x07b00000 bits 11:8 / 15:12 | its priority |
| 0x12500001 bit 6 | switch of DSP 0's slot: 1 = a card is in |
| 0x12900001 bit 0 | switch of DSP 1's slot |
| SH-4 DMA channel 7 / 3 | words to and from DSP 0 / DSP 1, 16 bits each, on the pins above (request selectors at 0x12d00007 and 0x12d00003) |

On the processor's side, by what its program does with its eight port
pins and interrupt pins:

| Pin | Use |
|-----|-----|
| P2 (output) | interrupt request to the main processor: pulled low while the first word of a message waits in the host data register, high again once it is read.  This is what 0x07b00016 latches |
| P0, P1 (outputs) | set by a command of the main processor; what they drive is not known |
| P3 (input) | read at card start-up and reported to the main processor on request; low lets the program accept commands that write.  The card's write-protect switch, by that.  The emulator holds it high: card images are not written |
| INT4 | the program takes it for "the card has gone" |
| MCLK | the audio master clock: the crystal, by the division the program selects (512 per sample) |

The processor runs on the crystal's clock after a reset and, once its
program switches over (the stock program does, first thing, after
giving the multiplier 20,000 clocks to settle), on that clock times a
factor that pins 7 to 4 of its port set at reset.  The emulator takes
three (67.7 MHz, under the part's 75 MHz): the program sets its timer
to 53 counts of the clock divided by 128 and counts its own timeouts
in those ticks, which at that factor are 100 µs.  The pins can be read
and the clock measured on a real unit: `sdk/examples/report` does both
(`docs/custom-firmware.md`, "The card processors of the 192").

### Start-up

The driver resets a processor and writes its program through the host
port.  The first part is taken by the processor's boot ROM, as the
manual describes ("host word boot"): a dummy word, the number of
instructions (0x0e00: all of the RAM at 0x200-0xfff), a word for the
host interface's mode register, then the instructions, low half first.
That part of the program then takes the rest itself, in three sections
that each start with a count of blocks, a block being a length, an
address and its words: instructions for the RAM at 0x8000 (through the
boot ROM's "reboot" service at address 6), then data for the X memory
and for the Y memory.  After the boot ROM's part and after every block
it sends a word (zero) that the driver reads.  V2220371 sends 29,363
words in all to each processor.

The program then calls address 8 of the boot ROM four times with an
address around 0x7b00-0x7ea0 in R7 and an instruction in R6: the
"instruction ROM correction" the manual describes for other members of
the family.  So this part has a mask ROM of its own at 0x7000 on, which
the manual does not mention (it calls 0x1000-0x7fff "system"), and the
program patches four instructions of it.  It reaches that ROM through
twelve entries at 0x7000-0x700b, and by what it hands them (blocks of
64 bits, keys kept exclusive-ored with constants, a self-test against
fixed results) the ROM holds the cipher and authentication of the SD
card's copy protection, which the manual says is kept out of sight in
parts made for licensees.  **The emulator does not model that ROM, and
is not meant to**: every entry returns "failed".  Plain MP3 files do
not go through it (the decoder calls it only for a frame flagged as
encrypted).

### Messages

Everything after that is messages of 16-bit words, in both directions
with a header whose low byte counts the words that follow.  From what
has been seen so far:

| From the main processor | Answer | Meaning |
|-------------------------|--------|---------|
| `1202 asst tir` | `f000` | start: `asst` (0xd762) goes into the audio serial interface's mode register (output on, master, 16 bits a channel, 512 master clocks a sample), `tir` (0x34) into the timer's period register.  The driver logs `DSPC_SYS_INIT Success` |
| a card command: the interface's command register value, argument high, argument low | `d104` + 3 words of response + the control register, or `d209` + 8 words for a long response | **the main processor runs the card itself**: CMD0, ACMD41, CMD2, CMD3, CMD9, CMD10, CMD7, CMD16 arrive this way, the program only passes them to the card interface |
| a read of sectors | data by DMA | the program has the card interface fill its data memory through the buffer transfer unit, a sector (CMD17) at a time, and hands the words on |
| play: where the file's data is on the card, as a byte address and a length (8000 bytes at a time) | `c701 001f` when it wants the next piece | the program reads the sectors itself, finds the MP3 frames, decodes, and sends the samples out of the audio serial interface; `c500` ends it (also on an error of the decoder) |
| others | `f000`, or `ee01 code` for an error | port pins, clock stop (the processor's STOP instruction), status |

Playing, word for word, as the North American driver does it (and
`sdk/src/sd.c` after it): read register 0x3801 (`2203 0000 3801 0001`,
answered `d301 0001 value`) and write it back with bit 6 set (`2303
0000 3801 0001`, `f000`, the value): the audio output on; `3301 0081`
and `3101 0001`, each `f000`; data word 0x0a5c cleared the same way;
then `3400`, answered `f000` and at once `c701 001f`: "more".  To each
`c701 n` the driver says `0401 0001` (`f000`) and eight words: the
place on the card in bytes, 32 bits, the length in bytes, 32 bits
(5000 at a time, not on sector boundaries), and four zeros.  When the
file is at its end it says `0401 0000` instead (`f000`), and `c400`
comes when the last of it has been played, to which the driver says
`3901 0000`.  To stop in the middle it says `3500` (`f000`) and a
second later takes bit 6 of 0x3801 back.  What the place looks like
for a card that counts in sectors has not been seen.

The file system is the main processor's business: it reads the
partition table, the FAT and the directories through the sector
command, lists the files in its own format, and tells the processor
which bytes of the card to play.

### In the emulator

`rnse boot --sd-card FILE` puts an image (see `rnse card`) into the
first slot, a second `--sd-card` into the other; `--sd-in MS` and
`--sd-out MS` put the cards in and take them out while the unit runs.
The unit sees the card by the slot's switch, starts it, reads it,
offers "MP3" in the media sources, and plays; a card pulled out while
it plays stops the processor (by its INT4 pin) and the main software
(by the switch), which then stops the card interface's clock: `tests/disc.rs`
`mp3_from_a_card_plays_on_the_192` goes from the card image to a tone
of the right pitch on the right channel, on both the European and the
North American software.  `--trace-sd` shows the card commands and the
processors' peripheral registers, `--trace-dsp` the words on the host
ports, `--dsp-tp` and `--dsp-watch` look into a processor's program.

What is not modelled, or assumed:

* The processor's time: one instruction per clock (two or three for a
  jump), which is what the manual gives, at the assumed 67.7 MHz.  The
  processors keep the main processor's time exactly: whenever a port
  or the pins are touched they are first brought up to that instruction
  of the main processor.  (The driver polls the pins between words, and
  the North American software does not forgive a processor that reacts
  a time slice late.)  Only a DMA transfer, which the emulator does in
  a burst, lets a processor run ahead for the words of the burst, by
  no more than 4096 clocks, which are taken off its time afterwards.
  In HALT the real part runs on a divided clock; whether that slows
  its timer is not clear from the manual, and the emulator does not
  slow it.
* Cards other than a plain SD card are the model's guesses made to
  see what the stock driver does with them, not measurements:
  `--sd-old` (no answer to CMD8, busy for three rounds), `--sd-deaf N`
  (no answer to anything after the first N starts) and `--sd-mmc` (a
  MultiMediaCard: no CMD8, no CMD55, CMD1, the host's address, and
  selected by the address 0 the stock driver gives it, which the
  specification reserves).
* The time a card takes.  On a unit a sector from a 32 MB card came in
  1.29 ms (64 one after the other in 82.6 ms, 64 a thousand sectors
  apart in 80.7 ms, a single one in 1.30 ms), through the SDK's driver
  with its 20 us before each word; the same driver gets one in 0.72 ms
  from the emulator's card, which answers at once.  The card's start
  took 4.61 s there against 3.96 s here.
* **A word sent too soon after another is lost, by the program's own
  design.**  It takes the words of a message's head in its host
  interrupt (0x0230, into a ring of 32 words at 0x0a64), but the words
  that follow a head it fetches itself with that interrupt masked
  (0x0381: it waits on the status register and reads the data register
  directly), sets a flag (0x0a61) and lets the next host interrupt pass
  without reading, as the one the fetched word had raised.  A word the
  host writes between that fetch and the unmasking eight instructions
  later raises no second request, the one interrupt is let pass, and
  the word stays in the register for good: the processor halts in its
  main loop and the host waits on "cannot take a word".  This is in
  the program, not the model; the emulator met it with a driver of the
  SDK that wrote each word the moment the status allowed, at one
  alignment of the two processors' time and not at others.  Whether a
  real main processor can be fast enough is not known (two accesses to
  the port against eight instructions of the card processor: about the
  same time).  The SDK's driver waits 20 us before each word.
* Writing to a card (the image is never changed: a write command gets
  the card's "write protect violation"), the copy protection ROM, the
  serial input, the processor's stop mode beyond stopping (what wakes
  it on the board is not known; the driver resets it).
* The card is a standard-capacity one by its registers when the image
  is no larger than about 2 GiB, a high-capacity one above (the rule
  the 193's driver forces, see `sdhi.rs`).  The 192's software (V220
  0650) does not take a high-capacity card: it never sends the command
  that tells such a card the host knows its kind (CMD8, and the bit in
  ACMD41), so the card stays "busy" for good, as a real one would.
* The pins' assignment above is from the program's use of them, not
  from the board: the photographs do not show the tracks.

The 193 has none of this: its main processor has an SD host interface
of its own and decodes MP3 in software (see
[hardware-193.md](hardware-193.md)).

## GPS

There is no self-contained GPS receiver with a serial port.  The OS
contains a GPS software library (`GPSLIB-R01.01.01`, code from about
0x8000c800 to 0x80030000) with its own tasks: initialisation, events,
navigation message subframes, position, satellite selection,
differential corrections and external input/output, plus an interface
task towards the navigation application and one watching the antenna.
The hardware underneath is a baseband device on chip select 5:

* 0x17c00000-0x17c00014: six 32-bit control registers.  +0x00 and
  +0x04 are written at start-up (0x00020000, by the antenna task, and
  0x00000300); the low half of +0x08 is set to 1 when the library
  starts; +0x0c has a two-bit state in its low bits; +0x14 is the
  interrupt status (bit 31: a periodic event that the handler counts,
  bit 16, bits 1 and 0: events that wake the library's tasks; bits are
  cleared by writing them as 0) and bit 0 of +0x10 is cleared at the end
  of the handler.
* 0x17e00400 and 0x17e00800: referenced only by the library; presumably
  the per-channel measurements.
* Its interrupt is vector 0x1e, INTEVT 0x3c0, the lowest external level.
* 0x07e00000-0x07e00008 (companion chip) is used by the library to set
  and read three 16-bit values at once, a counter or clock of some kind.

The library keeps its satellite data in the battery-backed SRAM.

None of this is emulated: the interrupt never comes, so the library
waits forever.  Emulating it properly means producing the baseband's
measurements for a simulated sky, which needs the device identified;
the alternative is to hand a position to the navigation software above
the library.

The OS's table of interrupt handlers (handler, vector << 16 | 0x0800)
is at 0x80032990 in V2200815: vectors 0x1e (GPS), 0x19, 0x11, 0x18,
0x1a, 0x34, 0x35, 0x3c and 0x3f.
