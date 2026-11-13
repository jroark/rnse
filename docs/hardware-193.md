# RNS-E hardware notes: the 193 unit (variants V574, V575 and VB48)

The 2010 unit (8P0 035 193) is a different computer from the 192
described in [hardware.md](hardware.md).  Almost everything is inside
one system-on-chip of Renesas' SH7770 "SH-Navi" family, for which no
manual is public.  What follows was worked out from the firmware, with
two outside aids: the Linux kernel's SH7770 support (serial port
addresses) and the Linux driver for the display unit of the later R-Car
chips, which inherited the block almost unchanged.

Variant `V574` (software index 0220) and `VB48` (0700) use the same
memory map and the same radio processor software; their loaders differ
by a few bytes.  Which cars `VB48` is for is not known.  `V575` is the
North American software (index 0320) for the same hardware.

![The 193's update loader in the emulator](loader-193.png)

![The 193's main software (V574) in the emulator: radio set-up](os-193.png)

Caches: the OS writes 0x80008181 to CCR (0xff00001c) like the 192's,
the update loader 0x00000101; either way both caches are on, stores
through P1 are written through and stores through P0 are copy-back.
The loader, linked at 0x09b60000, runs in P0.

## Memory map

| range                 | device |
|-----------------------|--------|
| 0x00000000-0x03ffffff | NOR flash, 64 MiB, same command set as the 192's |
| 0x08000000-0x0fffffff | RAM, 128 MiB (the loader's stack starts at 0x8fffdf00) |
| 0x18000000-           | battery-backed SRAM (the OS has some 1800 references to it) |
| 0xfe008000, 0xfe800100 | not identified (a few writes at start-up; 0xfe800100 takes the key 0xa55a) |
| 0xff802000            | interrupt pin control (the layout of the SH7780's INTC: sense, mask and mask-clear registers) |
| 0xff804000            | interrupt controller |
| 0xff902000            | a second block of interrupt priority and mask registers, written alongside 0xff804000 (not emulated) |
| 0xff907000            | a clock that keeps running, used by the GPS library only: six 16-bit values to set at +0x06..+0x10 (the last one shifted left by five), taken over by writing 0xffff to +0x02; the running values at +0x12..+0x1c, read once bit 1 of +0x04 is clear.  Which value is which (date and time, by their number) is not established; not emulated, the registers read back what was written |
| 0xff908000            | ten registers written in groups at start-up, purpose not established (not emulated) |
| 0xff909000            | a port: +0 written 0, 0xc0000000 and later 0xc0100000 (direction or pin function, not understood), data at +4.  Data bit 1 is the faceplate controller's reset line, low = reset; bits 0 and 3 are set at start-up and unidentified |
| 0xff915000            | serial port to the radio processor |
| 0xff926000            | serial port with FIFO to the faceplate controller |
| 0xff923000, 0xff927000 | further serial ports with FIFO (not emulated) |
| 0xff93c000            | A/D converter |
| 0xff93e000, 0xff93f000, 0xff940000 | ports; 0xff93f000 has the display motor and the supply monitor |
| 0xffc40000, 0xffc44000 | ports; 0xffc44008 bit 0 strobes the external watchdog |
| 0xffc62100            | ATAPI interface |
| 0xffd80000, 0xffd81000, 0xffd82000 | timer units, three channels each, SH7750 layout |
| 0xffdf0000            | CAN controller (RCAN-ET) |
| 0xffe40000            | serial port with FIFO, initialised by the loader but silent |
| 0xffe80000            | 2D graphics engine |
| 0xfff80000            | display unit |

Flash layout as the loader leaves it: boot ROM from 0 (not on the disc),
the module directory at 0x400000, the OS at 0x400200, the other modules
between 0xb00000 and 0x2f50000, and data from the disc's `SDS` directory
in between.  A module is stored with its 0x80-byte header, at its load
address.

## Interrupts

On-chip sources go through a controller with the layout of the SH7780's
INTC2: twelve priority registers from 0xff804000 holding four fields
each (the firmware writes them as bytes; the level is the value divided
by two), mask-set and mask-clear registers at +0x38/+0x3c and a second
pair at +0x108/+0x10c.  Everything is masked after reset.  +0x30 and
+0x100 show the requests of the two halves, +0x34 and +0x104 those that
are not masked; the radio link's handler looks at +0x100 before it
touches its port.

| source | INTEVT | priority field | mask bit | level used |
|--------|--------|----------------|----------|------------|
| display (vertical blanking) | 0x3e0 | PRI0 bits 31:24 | 0 | 14 (loader), 8 (OS) |
| timer 0 (tick) | 0x400 | PRI0 bits 23:16 | 1 | 7 |
| timer 2, underflow and capture | 0x440, 0x460 | PRI0 bits 15:8 and 7:0 | 1 | 14, 12 (OS) |
| timer unit 1, channel 0 (CAN) | 0x480 | PRI1 bits 31:24 | 2 | 8 |
| ATAPI | 0x5e0 | PRI1 bits 7:0 | 7 | 3 |
| CAN | 0x880 | PRI5 bits 23:16 | 25 | 5 |
| graphics engine | 0x780 | PRI3 bits 7:0 | 19 | 7 |
| serial port, radio processor | 0xd20 | PRI9 bits 15:8 | 9 of the second pair | 2 |
| serial port, faceplate | 0xdc0 | PRI10 bits 23:16 | 14 of the second pair | 9 |
| A/D converter | 0xf00 | PRI11 bits 15:8 | none found | 11 |

The levels are the ones the 192 uses for the same jobs.  The timer
fields are an inference: the loader sets bits 23:16 when it starts timer
0 and the OS sets the two lower fields for timer 2, whose handlers sit
at 0x440 and 0x460.  When the OS enables the display interrupt it also
sets bit 0 of 0xff802000, so that one may be a pin rather than the
display unit's own request line; the emulator raises it from the display
unit's vertical-blanking flag either way.  The loader's
table of handlers also has entries for 0x220-0x3c0 (external interrupt
levels), 0x4c0, 0x660, 0x680, 0xaa0, 0xac0 and 0xe80, which it does not
use.

The CAN timer is a separate channel at `0xffd81008` (reload),
`0xffd8100c` (counter), `0xffd81010` (control), started by bit 0 of
`0xffd81004`. V57510AA's interrupt table installs wrapper `804cc58c`
at exception `480`; it calls `806305a0`, which clears this timer's
underflow and wakes the CAN task every second interrupt. The boot ROM
stand-in starts it at 5 ms (see [boot-rom.md](boot-rom.md)).

## Display unit

800 x 480 panel.  Register names below are those of the Linux R-Car
driver.

* Timing: a display period of 832 x 496 inside 1056 x 525, data enable
  800 wide.  The loader starts the period at column 181 and line 24 and
  draws its picture 16 columns and 8 lines into its surface; the OS
  starts it at 197/32 and draws from the surface's corner.  So the panel
  begins at a fixed place in the frame, 18 columns before the start of
  data enable (215) and at line 32, and that is what the emulator shows.
* Eight planes at +0x100 * n: mode at +0x00 (bits 1:0 = 8 bpp with
  palette, RGB 565 or ARGB 1555; bits 9:8 = which of four palettes;
  bits 14:12 = how the plane is laid over those behind it: bit 14 = no
  transparent colour, 1 in the low two = alpha blending), memory width
  +0x04, alpha +0x08, size +0x10/+0x14, position +0x18/+0x1c, surface
  address +0x20 (repeated at +0x24 and +0x28), start position in the
  surface +0x30/+0x34, transparent colour +0x44 (8 bpp) and +0x48 (16
  bpp), memory length +0x50.  Surfaces are ordinary RAM.
* Alpha: bits 10:8 of the alpha register say where the blending ratio
  comes from.  0: the low byte.  3: per pixel from another plane, whose
  number (from 0) is in the low bits: an 8 bpp plane that is enabled
  like the others but is not a picture.  The OS's frame and note
  overlays work this way, each with its alpha plane's surface directly
  behind its own in memory.  7 (with 0 or 1 in the low bits) is used
  for the main picture and its pop-up lists while menus are shown; what
  it selects is not known, and the emulator shows such planes without
  blending.  The A bit of ARGB 1555 pixels is not interpreted.
* Plane priority register (+0x18): eight nibbles, each naming a plane
  and enabling it with bit 3; the lowest nibble is the front.  (The OS
  puts its blended overlays in the low nibbles and the alpha planes in
  the highest.)
* Four palettes of 256 entries at +0x1000 * n, 0x00RRGGBB.
* Status at +0x08 (bit 11 = vertical blanking, bit 14 = frame), cleared
  through +0x0c, enabled as interrupts through +0x10.

The loader draws 8 bpp into a surface at 0x08110000 (the same frame
buffer offset as on the 192) shown on plane 6 with palette 2.  The OS
uses plane 6 (16 bpp, transparent colour 0x7c1f) for its main picture
and planes 1 and 3 as blended overlays (the note in the picture above
is one), with planes 8 and 7 as their alpha.

## Graphics engine

Everything the OS shows is drawn by a 2D engine at 0xffe80000 that runs
display lists from RAM; the firmware's messages call it "Q2", and the
register layout is that of the engine Renesas later named M2DG.  The
commands are not those of the 192's Coral chip.  [src/dev/q2.rs](../src/dev/q2.rs)
describes registers and commands as far as they are known: register
writes, rectangle fills, copies with a transparent colour, stretched
copies, and blending a colour through an 8-bit coverage map, which is
how text is drawn (the OS rasterises antialiased glyphs itself).  Surfaces are 16 bits per
pixel, or 8 for alpha.

The map is not a picture from the disc but drawn from its vectors with
further commands: polylines of a given width (roads; one variant takes
two colours and a bit pattern, for railways), and filled areas.  An
area is done in two steps, as on Hitachi's Q2 chips of the 1990s from
which the engine evidently descends: the polygon is rasterised into a
one-bit work buffer, then a quadrangle over its bounding box paints the
colour where the work buffer is set.  The round ends and joints of roads
are small octagons drawn the same way.  The navigation application
draws the map into a surface of 928 x 608 pixels and copies the middle
800 x 480 to the screen's plane, so that the map can be moved a little
without drawing it again.

Pictures that are laid over others (the panel at the right of the map,
the mark left of a list) come with an alpha map, named by the registers
at +0x64/+0x68.  The software first has command 0xab combine that map
with the alpha values of the plane drawn to and then copies with mode
bit 0x400, which blends; text goes the same way with its coverage map
as the alpha map.  The 0xab commands of several pieces can be in one
list and the drawing in the next.

The OS starts a list, waits for the interrupt its end raises (status bit
2, INTEVT 0x780) for up to four seconds and logs `Q2 TIME OUT` if it
does not come.

Two commands (0xab, 0xaa) bracket drawing into a surface that has alpha
values; what the engine does with them is partly a guess, see the source.

## ATAPI interface

Task file registers at 0xffc62100 + 4n (n = 0..7), alternate status and
device control at +0x38, one register per 32-bit word.  Data words carry
the first byte of each pair in the low half.  Interface control,
status, interrupt enable and timing registers follow from +0x80 (the
layout of the later R-Car ATAPI block); the loader sets them up once and
then works by programmed I/O.

The OS works by interrupt.  The status register (+0x84) has the drive's
interrupt line in bit 4, the end of a DMA transfer in bit 1 and an error
in bit 2; +0x88 enables each as an interrupt, and the handler switches
them off again until the driver is ready for the next.  The driver
expects the drive to interrupt when it is ready for the twelve bytes of
a packet command, which newer ATAPI drives do not do.  Disc reads go by
DMA: bit 2 of the control register (+0x80), the memory address in +0x9c
and the byte count in +0xa0 are set before the command; once the status
shows DRQ and the signal register (+0xb0) bit 0 shows the drive's DMA
request, control bit 0 starts the transfer.  Bytes land in memory in
the order the drive delivers them.

What kind of disc is in the drive the OS finds out with READ
SUB-CHANNEL: a CD answers, a DVD refuses with "illegal mode for this
track" (sense 5/64), and that refusal is the OS's test for a DVD (its
log then says `media judge = 1`; 2 is an audio CD, 3 a data CD).  For a
DVD it goes on to READ DVD STRUCTURE and wants a DVD-ROM with one
read-only layer.  The emulated drive reports any data disc larger than
a CD as such a DVD.

Audio CDs are played by the drive itself.  For the MEDIA source the OS
reads the full table of contents (READ TOC format 2), starts the player
with PLAY CD (0xbc, addresses in minutes, seconds and frames, from the
first track to the lead-out) and then polls READ SUB-CHANNEL for the
position it shows.  The emulated drive keeps time for that to the
sample, and what it plays can be had as sound (`--wav`): the drive's
analogue output, that is, before the radio processor's volume control
and amplifier.

## CAN

Telephone-module investigation and the remaining protocol prerequisites
are recorded in [telephone.md](telephone.md).

RCAN-ET at 0xffdf0000: the control registers of the 192's HCAN2, but a
mailbox has the identifier at `STDID << 2`, the acceptance mask at +4
and the control word (type, length) at +0x10.  The loader sends the same
messages as the 192's: 0x661 and 0x604 status frames and network
management on 0x436.

Bit timing as the loader sets it, after putting the controller in reset
(MCR = 1): BCR0 (+0x06) = 0x000b, BCR1 (+0x04) = 0xb730, a prescaler of
12 and 1 + 12 + 8 = 21 quanta a bit.  With the HCAN2's rule (a quantum
is two clock periods times the prescaler) that is 100 kbit/s from 50.4
MHz.  The faceplate port's divisor says 50 MHz too (9600 bit/s), while
the timers count at 6.25 MHz, which is a quarter of 25 MHz as on every
SH-4: the serial peripherals seem to run from twice the clock the timers
divide.  (Both timer periods match the 192's: 2 ms for the OS's tick,
5 ms for the CAN timer.)

### Who sets up the mailboxes

The OS programs its transmit mailboxes but no receive mailbox: it only
unmasks mailbox interrupts and then looks at the identifier of whatever
arrives, so the boot ROM must have set the receive side up.  The one
fixed point in the OS's receive path is mailbox 24, treated (as in the
loader) as the network management range 0x400-0x43f.  The emulator's
boot ROM stand-in programs that one and a single catch-all mailbox.

The OS's table of expected messages (36 slots of 40 bytes at 0x8eac58a8
in V5741040) lists 0x200, 0x271, 0x2c3, 0x351, 0x353, 0x359, 0x497,
0x523, 0x527, 0x543, 0x5c3, 0x602, 0x621, 0x623, 0x625, 0x627, 0x635,
0x63c, 0x651, 0x65d, 0x65f, 0x662, 0x665, 0x6c1, 0x6c5, 0x6d0, 0x6d1,
0x6da and 0x6dc; it transmits 0x252, 0x25b, 0x261, 0x263, 0x361, 0x461,
0x5ab, 0x604, 0x661, 0x67a, 0x6c0, 0x6c4, 0x6c6 and 0x6ca.  With the
ignition message (0x271) arriving, the power manager goes on as on the
192.

The OS does not start from nothing here either: early on it asks the
boot ROM (service 0x0b) for the state of the boot ROM's own CAN software,
including the table of received messages and whether the ignition lock's
message (0x271, or 0x2c3 on another generation of car) has been seen,
and decides from the first byte of that message (bit 0 = key in the
lock, bit 1 = ignition) whether to start up or go back to sleep.
See [boot-rom.md](boot-rom.md).

## Serial links

* 0xff915000, to the radio processor: the 192's companion-chip serial
  port with its registers four bytes apart (mode +0, baud rate +4,
  control +8, transmit +0xc, status +0x10, receive +0x14, FIFO control
  +0x18).  The protocol is the 192's (`01 command length data
  checksum`), and the OS asks the same things in the same order.
* 0xff926000, to the faceplate controller: an ordinary SCIF (control
  +0x08, transmit +0x0c, status +0x10, receive +0x14, FIFO control
  +0x18, receive count +0x20).  Same frames as on the 192; the OS sends
  commands 0x02, 0x03, 0x06, 0x07, 0x09 and 0x0a once the controller has
  announced itself with 0x0f.
  Set-up by the OS: control 0, FIFO control 6 (both FIFOs held empty),
  status flags cleared, mode 0x0020 (8 bits, even parity), bit rate
  0xa1; then the controller's reset line (bit 1 of 0xff909004) goes
  high, FIFO control 0, control 0x30.  0xa1 is 9600 bit/s if the
  peripheral clock is 50 MHz and 4800 if it is 25 MHz; the debug port's
  divisor (0x36 at +0x30, sixteen samples a bit) then gives 57600 or
  28800.  The CAN controller's bit timing also points at 50 MHz (see
  CAN), and the 192's notes say why 9600 is the likelier for the
  faceplate.  As on the 192 the OS resets the controller when it leaves
  a command unanswered for two seconds.

## A/D converter and ports

The converter at 0xff93c000 has eight results at +0x00..+0x0e and a
control register at +0x10 (bit 15 = done, bit 14 = interrupt enable,
bit 13 = start).  The OS converts all inputs every few milliseconds and
averages them:

* input 4: position of the motorised display, as on the 192 (low =
  open; the OS logs `init_dsp_pos = 2` when it reads as closed);
* input 5: a temperature (the OS computes 25 + (1.45 V - U) / 8.2 mV);
* another input tells the type of screen fitted; the emulator leaves it
  at zero, which the OS logs as an error (`Adc_get_scrntype return
  ZERO`) and survives.

Port 0xff93f000 (direction +4, output +8, input +0xc): output bits 19
and 18 drive the display motor (assumed: 19 towards closed), input bit
20 is low only when the display is fully open, and input bit 22 has to
be high or the OS stops itself after nine seconds, waiting for the
watchdog: presumably the supply voltage monitor.

## Watchdog

External, strobed through bit 0 of the port at 0xffc44008 from the
tick, every 20 ms; the longest pause the OS leaves while it starts is
about 150 ms.  The loader ends an update by printing `*** WDT Reset ***`
and no longer strobing; when it gets impatient it jumps to address 0
itself.  The emulator resets after 1.5 s without an edge (the real
limit is not known).

The loader toggles the bit with 32-bit accesses.  The OS reads the
register as 32 bits, flips bit 0 and writes the result back as a *byte*
at the same address, through a routine it uses for all its ports.  In
memory that byte would be bits 31..24 and the strobe would never move;
since the OS evidently keeps a real unit alive, the registers of the
port blocks at 0xffc40000 and 0xffc44000 must give an access of any
size their low bits, and they are emulated so.

## Debug serial port

A SCIF at 0xffe40000 with the register layout of the faceplate's
(16-bit registers four apart: mode +0, bit rate +4, control +8, transmit
data +0xc, status +0x10, receive data +0x14, FIFO control +0x18).  The
loader and the OS's low-level output write characters to it after
setting control bit 5 (transmitter enable); the emulator prints what is
sent as the console, and nothing while the transmitter is off.

## Loader

Entry and arguments as on the 192, but the loader sets its own stack
and vector base (inside its own image).  It reads the project code from
the boot ROM header at flash 0x406, like the 192's.  Its log goes
through a `printf` (0x09e3937a in V5740002) that formats and discards;
`rnse loader` hooks it.

## Main software

The OS and the other modules have the same structure as on the 192
(three entry points each, a uITRON-style kernel, the same task names in
the debug log), with two differences in how they are started:

* The boot ROM, not the OS, calls every module's entry points: stage 1
  of each module in directory order, then stage 2, then stage 3, with
  the OS's third entry (which never returns) last.  The 192's first OS
  (V220) does this for the other modules itself; the same routine is
  still in the 193's OS but nothing calls it (V222, the later 192
  software, already works the new way).  The order matters: the application's
  first entry moves the memory break (boot ROM service 5) from the end
  of the OS's data down to the end of its own, at 0x0ca57004, and the
  heap the OS creates later runs from the final break up to 0x0e8cfff0,
  just below the OS's data.
* The OS replaces the boot ROM's vector tables with its own, in RAM at
  0x8e8d0000 (TRAPA handlers), 0x8e8d0400 (exception and interrupt
  handlers) and 0x8e8d0600 (interrupt mask levels), and seeds them from
  the boot ROM's: 64 handlers at the old vector base (the first 16 for
  exception codes 0x000-0x1e0, all of them for TRAPA 0-63) and 125 mask
  bytes at +0x780.  See [boot-rom.md](boot-rom.md).

As on the 192, the OS starts timer 0 for its 2 ms tick without giving it
a priority; the boot ROM has done that (the loader's value is level 7,
with the timers' mask bit cleared).  The peripheral clock is 25 MHz,
going by the tick's reload value.  The CPU clock is taken as sixteen
times that, 400 MHz, the rated speed of this family, and the emulator
counts one instruction per cycle; this is an estimate.  The firmware's
calibrated delay routine (0x804dd960 in V5741040) runs 1056 instructions
per unit of its argument where the 192's runs 304, which fits a CPU
about three and a half times as fast.

In the emulator the OS starts all its tasks, checks its flash file
system, sees the ignition, talks to the radio processor and the
faceplate, shows its start-up picture after about half a minute and
then its user interface, on all three variants.  It reads the disc in
the drive, takes keys from the faceplate and changes screens (radio,
set-up and so on).  A freshly installed unit first asks for the
navigation disc to load a language from ("Bitte Navigations-Medium
einlegen"), or says that the disc in the drive is not the right one; the
function keys work regardless.  `printf` is at
0x80688afe in V5741040, 0x80688146 in VB481000 and 0x8067b886 in
V57510AA.

## Navigation

The map comes from a DVD (ISO 9660): `ALLDATA.KWI` with the map in
Aisin's KIWI format, index files for destination entry under `IDX`,
`VOICEDAT.KWI`, and a copy of the unit's software for updating older
units from.  The unit's SD card slots take the same thing: a card with
the DVD's volume in a second partition of type 0x96, behind a FAT
partition.  The SD card processors are not emulated, so the emulator
puts that partition into the DVD drive instead (`--nav-disc card.img`).

![The map (V575) in the emulator](nav-193.png)

The navigation application (module `APM`, running from flash at
0x01c00000) has its own copy of the debug `printf`, at 0x021a2b16 in
V57510AA, hooked like the OS's; most of what is known about its disc
handling comes from that log.  When a disc arrives:

1. The OS reads the volume descriptor and the directory (its `CDSFS`).
   A second reader, in module `AUM`, looks for update files and
   addresses the disc by byte offsets of 32 bits, so on a DVD whose
   directory lies beyond 4 GB it reads the wrong sectors (0x0da35e for
   0x2da35e) and finds nothing.  That is the firmware's doing, not the
   emulator's, and harmless.
2. The application (`DVC`) reads `05.CMP`, a table of the disc's files,
   and the first 15 sectors of `ALLDATA.KWI`, whose header it unpacks
   and checks: the system identifiers (`dvc_chksysmid`), the format
   version (`dvc_chkafmtver`) and the metadata (`dvc_chkmetadata`).  If
   all three pass the disc is a map disc (`DISCTYPE:CDDISC_MAP`),
   otherwise at best a program disc.
3. It compares the software index on the disc (`575INDEX.TXT`) with its
   own and would offer an update if the disc's were newer.
4. Then it reads map data as it needs it (`CDREAD:` lines).

### Map data made for one unit

Map updates are also sold for a single unit, as an SD card image that
goes with a modified version of the unit's software.  On such a card
the identifiers in the header of `ALLDATA.KWI` begin with the unit's
serial number, fourteen characters and a comma, before the format
identifier (`AFUS:2.45` on a North American disc).  The modified
software (index 0320, V57510AA) has a matching addition in
`dvc_chkafmtver`, a jump out of the routine to hand-written code at
0x023468a8: if a comma follows fourteen characters, they are compared
with the fourteen bytes at 0x03ff401f in flash, the unit's own serial
number, and the check goes on behind the comma only if they are the
same.  Otherwise it fails, and a second hook replaces the text of the
resulting note with "Navigation media not authorised for use on this
unit."  A disc without the prefix, such as an original DVD, is checked
as before.

The serial number is in a parameter block of the flash (0x03ff4000,
0x4000 bytes, which the OS reads and writes by offset) that is written
at the factory and is on no update disc.  An emulated unit therefore
has none, and refuses such a card as any other unit would.  To emulate
the unit a card was made for, give the emulator that unit's serial
number with `--serial` (`Machine::set_serial`); the emulator does not
take it from the card.  The card's licence file (`licence.key` in its
FAT partition) plays no part in this check and has not been looked
into.

### What works

With a map disc accepted, NAV shows the destination menu with the
disc's regions, the entry screens with their letter wheel come up, and
the map is drawn at every scale, with street names, the scroll mode
with its cross hairs and coordinates, and the panel at the right.
Without a GPS fix the position is wherever the unit thinks it was last,
the place the software starts with; with one (`--gps`, below) the map is
where the fix says.  There is no gyroscope and no speed signal, so the
position does not move by itself.

### GPS

The receiver is not a module with a serial port.  A block of the
system-on-chip at 0xff938000 passes messages to and from a GPS library
in the OS (`GPSLIB-R03.00.07`, the code from 0x80400c00 to about
0x80436000, with tasks of its own for events, subframes, position and
so on), which works the position out itself.  The block has control and
status bits in six registers from 0xff938000, a buffer the library
writes a message to at 0xff938400 (a long word, a length, a checksum,
data from +8; bit 16 of the first register sends it) and one it reads
from at 0xff938800 (bit 16 of the register at +0x0c says there is one).
The 192 has the same thing on chip select 5 (see hardware.md).  None of
that is emulated: the library finds no satellites and reports no fix.

What it reports goes this way.  The library keeps its results in nine
blocks that `0x80423630(number, buffer)` hands out.  An interface task
of the OS (`GPSIF_TSK`) fetches them when the library signals new ones,
once a second, and makes records for the navigation application, which
asks for them through trap 33 (the sensor service: r4 = group << 8 |
function, r5 = argument): 0x10a for a record of 47 bytes with the
position, 0x10b for 48 bytes about the satellites, 0x10c for 7 bytes,
0x10e for date and time.

`--gps LAT,LON` gives the unit a fix without any of the radio side:
when the application asks for the position record and the OS has one
for it, the emulator first writes the fix over the library's "no fix"
(`src/dev/gps.rs`, which also lists what is known of the record:
status, latitude and longitude in 1/256 of an arc second, date and time
in BCD and Japanese time).  The map then shows that place and the clock
runs.  The routine and the pointer it reads the record through are
found by their code (`Machine::find_gps_getter`), in the three builds
of the V575 software seen so far.

![The map with a fix in Chicago](gps-193.png)

### Speed and gyroscope

Between fixes the navigation reckons the position from distance and
turning, which the OS samples in the A/D converter's interrupt (the
routine at 0x805462f8) into a ring of 512 long words that the
application empties through service 0x304 of trap 33.  A sample has the
gyroscope in bits 31:20, flags in bits 15:14 and the number of speed
pulses since the last sample (15 at most) in bits 19:16.

* The gyroscope is input 1 of the A/D converter: the sample is
  (930 - reading) * 4, of the ten-bit reading.  The software finds the
  voltage at rest and the scale for itself.
* The speed signal is on the TCLK pin: timer 2 runs with input capture
  (TCR2 bits 7:6; the count goes to TCPR2 at 0xffd8002c and bit 9 of
  TCR2 is set), and the capture interrupt (INTEVT 0x460) counts a pulse
  and measures the time since the last.  The software starts out with
  0.4 m to the pulse.
* The pulses only count in a unit coded for one kind of car, see
  "Coding" below.

`--drive KMH[,TURN][@MS]` moves the car: pulses on TCLK at 2.5 to the
metre, the gyroscope's reading away from 418 by 5.12 for a degree per
second (a gyroscope of 25 mV per degree and second; which way round is
a guess), and the GPS fix along its course (`--gps LAT,LON,COURSE`)
with speed and course in the record.  It codes the unit for car type 5
unless `--car-type` says otherwise.  With that the map follows the
car, turned to the direction of travel, and stays on the fix.

![Driving west on the Eisenhower Expressway](drive-193.png)

### Coding

What a workshop codes into the unit is kept in battery-backed memory,
in a block of 64 bytes at 0xb8002c4c: the mark 0x12345678, a version
(0x02220001), the sum of the sixteen bytes that follow, then six, four
and six bytes of coding, 32 further bytes, and the mark inverted at
+0x3c.  The OS uses the block if mark and sum are right.  Otherwise it
looks for a copy in a record of 224 bytes with the same mark and a sum
of its own (checked at 0x80666bda), which it reads out of the flash
parameter block, and failing that clears the block: an uncoded unit.

The first of the last six bytes is the car type, 1 to 7 (the routine at
0x806677e8 hands the six out).  The software sorts the types into four
classes (1; 2, 6, 7; 3, 4; 5).  Only for type 5 does the capture
interrupt count speed pulses; for every other the OS notes at start-up
that the handler is to ignore them.  Where those cars' distance comes
from has not been followed up: the CAN bus carries 0x351 and 0x359,
whose handlers are at 0x8066f6dc and 0x8066f7a8.  With a type coded the
instrument cluster's task logs `car_model = 4` instead of `ffffffff`.

`--car-type N` codes an emulated unit (`Machine::set_car_type`) by
writing that block before the start.

## Not done

* Antialiasing of the map's lines (drawing mode bit 2), the A bit of
  ARGB pixels, and any drawing command the screens seen so far did not
  use.
* Where a text field lies over the frame at the screen's edge (the
  entry screens of the navigation), every other line of the frame shows
  through.
* The pictures on the map (road number shields, symbols for points of
  interest): only their shadows appear, blended through an alpha map;
  no command that would draw the picture itself is in the lists, so the
  software probably fails to load it.
* The last blocks of the voice data are asked for beyond the end of
  the disc's table of compressed blocks (`exp_read ERR`) on the 2010
  disc.
* The GPS block itself and the satellite record; the rest of the
  coding, and where a car without the pulse wire gets its distance
  from; the SD card processors; route guidance has not been tried.
* The second interrupt register block at 0xff902000, the I2C masters,
  the remaining serial ports, DMA for the drive.
* Interrupt sources the OS registers handlers for and that have no
  model: 0x480, 0x760, 0x7a0, 0x940-0x980, 0xc00, 0xca0-0xce0, 0xde0,
  0xe20, 0xea0, 0xfa0.
