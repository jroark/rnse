# The boot ROM

The first 48 KiB of flash (0x0000-0xbfff on V220 hardware) hold a boot ROM
that an update never rewrites, so it is not on the disc.  The emulator
replaces it with a stand-in (`src/bootrom.rs`) built from what the other
software expects of it.  This file records that evidence.

## What other software reads from it

| what | evidence |
|------|----------|
| Offset 0: address of the reset entry | The loader ends an update with `jmp @[0]` (after printing `*** Jump User RESET ***`). |
| Offset 0x400: a module header; +6 is the project code, e.g. `V220` | The loader copies four bytes from 0x80000406 as the "current project" and only accepts a software set of that name from the disc.  Without it it tries its built-in list (`V220`, `V264`) and reports "version up not required". |

## Starting the loader

In normal use it is not the boot ROM that starts the update loader but
the installed navigation application.  Its disc task finds a program
disc in the drive (`DISCTYPE:CDISC_PRG` in its log), compares the
disc's software index with the installed one and, for an update:

1. checks the project code of the software set in `LOADING.KWI` against
   its own (`プロジェクトコード一致`, "project code matches");
2. adds up every module of the set on the disc and compares with the
   sums in the container (`チェックサムチェック`);
3. finds the module `LDM` in the container, reads it into a buffer,
   copies it to the load address in its header and flushes the cache;
4. writes a zero byte and its complement to the first two bytes of the
   battery-backed memory (0xb0000000);
5. calls the loader's first entry point with the screen language in r4
   (an index 0..8; the loader prints it as `SET Lang_data`) and 1 in r5
   and r6, and after it the third.

The comparison of the indexes is of their second and third characters
only, as text, after checking that the first characters are equal and
one of `0`, `X` and `Y`: installed `0650` takes a disc with `0660` for
an update, and neither `0651` nor `0640`.  When it is no update the log
has `ON ROM IDX = ...` and `ON DISC IDX = ...` instead of `IDX
VersionUP`.

What the loader's first entry point finds (V220, stopped there in the
emulator): privileged mode, SR = 0x40000100, that is interrupts *open*,
with the operating system's vector base, tick and other interrupt
sources as they were a moment before; a return address in the
application; the application task's stack.  Flash is byte for byte as an
ordinary start leaves it at that time: nothing about the update is
recorded there before the loader runs, and the two bytes of
battery-backed memory are the same after a complete stock update as
after step 4.  The stock loader sets up its own stack, vectors and SR.

The 193's application does the same (`DVC:dvc_exeverup` in both), and
so does the North American software of both units.  `rnse boot
<disc> --installed-index NNNN` runs the whole of it: the installed
software then reports that index, and a disc with a higher one is
updated from, by the stock application and the stock loader.

The place the loader is copied to is not reserved for it.  The North
American 192 software (V222 0371) has operating system variables at
0x8df710f0-0x8df71108, in the middle of it, which its interrupt
handlers go on changing after the copy; its own loader has picture
data there.  And step 3's cache flush is `ocbwb` over the copied range
only: the data cache is written back, the instruction cache is not
touched.

On the European 192 software (V220 0815) the copy is worse than that
for the software itself.  The module `APFM` (a small executive that
calls itself REX) keeps the pointer to its state at 0x8df9000c, which
the loader's image covers, and has a handler on the system timer every
100 ms that goes through that pointer.  From the moment the copy
passes that address to the moment the loader has its own vectors
(15 ms in the emulator: the rest of the copy, the cache flush, the two
bytes of battery-backed memory, the call) the handler would read
through loader code and meet an address error, which the OS hands to
the boot ROM's handler at 0x80000900.  Whether a start runs into it
depends on where in those 100 ms it falls, about one start in six by
these numbers.  On a real unit that is a matter of how long the drive
took, different every time, and what the boot ROM then does is not
known (presumably the unit starts again and tries again).  In the
emulator time is the same in every run: `rnse boot <disc> --variant
V220 --installed-index 0640` ends in exactly this fault with the
default clocks and starts the loader with `--cpu-ratio 3` or `5`.
Nothing of the kind happens with the North American 192 software or
on the 193.  A program of one's own started this way inherits the
risk: it is in the stock software, before the program runs.

### A disc that goes in while the unit runs

That is how an update normally starts, and the emulator does not get
there yet: with `--disc-in MS` the drive reports a changed disc and a
few seconds of "becoming ready", the OS mounts the disc and lists it
as media, and the application never looks whether it is an update.

Why, as far as it has been followed (European 192, application at
0x00280ef0-0x00281400): a task polls the drive's state every 100 ms and
turns changes into events for the disc task.  Its states are 2 (no
disc, from the OS's state -5), 0 (ready, from 0 or 1), 6 (from the
OS's -8 or -9), 3 (from 2) and -1 (errors).  "Ready" becomes the event
the disc task acts on (0x88000007, logged as `DVCEV_DISKOK(DISKIN)`)
only coming from 6, or from the state the task starts in; coming from
"no disc" it is no event at all.  The emulated drive goes from "no
disc" to "ready" without the OS ever reporting -8 or -9, whatever
those stand for (the drive's own loading of a disc, perhaps: the real
drive pulls the disc in).  With a disc in the drive from the start the
task's first look gives the event, which is the case the emulator and
its tests use.

### Only from the CD drive

Nothing in the stock software updates from an SD card.  The loaders
(`LDM`) carry a driver for the drive and nothing for the cards: the
193's has the drive's registers (0xffc62100) in its literal pools and
no address of the SD host interface (0xff90a000); the 192's has the
drive's data port (0xa7880000) and neither port of the SD card
processors (0xb5000002, 0xb500004a), nor their program to download; no
FAT code and no card messages in either.  On the application's side
the check is part of the disc task (`dvccd_tsk`, `dvccd_dsklblread`),
and `LOADING.KWI` is opened through a "CD data handle"
(`CDデータハンドル作成エラー` is its failure message).  The loader does
have a message `Loading from Multi-Changer` next to `Slotno Error`,
so a slot number is among what it is handed; that is a disc changer,
not a card.

A run agrees as far as the application's side goes.  A card with the
update disc's volume in a type-0x96 partition (the layout of the cards
that carry map data for the 193), an empty drive and a lower installed
index: the 193's software mounts the card's FAT partition, lists it for
music and reads not one sector of the other partition, in the stock
European software and in the modified North American one alike
(`docs/hardware-193.md`, "SD cards").

A program of one's own is not bound by this on the 193, where the SD
host interface belongs to the main processor and the stock OS drives
it directly; such a program would bring its own card driver, and would
itself still have to arrive on a disc.  On the 192 the cards are behind
the two SD card processors (`docs/hardware.md`), whose program and
host protocol would be needed first.

Whether the boot ROM can also start a loader from a disc by itself, as
it would have to after an update that was cut short, is not known.
`rnse loader` starts one directly, with interrupts masked and zeroes in
r4-r6 unless told otherwise (`--args`).

## Starting the OS

The boot ROM calls the OS module's entry points 1, 2 and 3 in turn, as
ordinary functions in privileged mode.  Entry 3 loads the first task's
context and never returns.  Evidence: entries 1 and 2 are functions that
return with `rts`; entry 2 installs the OS's vector base; entry 3 ends in
`rte`.

In the oldest software (V220) the OS itself walks the module directory
and calls the same three entry points of every other module at the
matching stage.  Every later software, including the North American one
for the same hardware (V222), expects the boot ROM to do that: stage 1
of every module in directory order, then stage 2, then stage 3, with the
OS's third entry last.  The routine that did it is still in those OS
builds but is not reached.  Without the calls the OS runs alone: its
tasks start and it talks to the radio processor and the faceplate, but
the application never asks it to draw anything and the screen stays
dark.

State the OS inherits:

* A stack above 0x8fc80000.  After a cold start the OS clears RAM from
  0x8c000000 up to that address while still running on the boot stack.
* TMU0 interrupt priority.  The OS programs and starts TMU0 for its 1 ms
  tick but only ever sets the *TMU2* field of IPRA.  The loader, which
  shares its hardware layer with the boot ROM, uses IPRA = 0x7000.
* On the 192, DMA master enable (`DMAOR.DME = 1`). The SD/MP3 driver
  selects its transfer mode by ORing `0xc300` into DMAOR but never sets
  DME. Starting with the CPU reset value prevents its first block command
  from transferring. Supplying DME at OS handover is inferred from this
  behavior. The register bits, including DBL for eight DDT channels, are
  described in section 14.7.5 of the
  [SH7750R hardware manual](https://www.renesas.com/en/document/mah/sh7750-sh7750s-sh7750r-group-users-manual-hardware).
* A running CAN controller.  The OS sets up its mailboxes but never
  takes the controller out of its reset/halt state itself.
* A running CAN timer: TMU2 on the 192, unit 1 channel 0 on the 193.
  The OS installs its handler and clears underflows but does not start
  the timer. The stand-in supplies a 2.5 ms interval, the one the
  loaders program themselves; the handlers divide this by two to
  service CAN every 5 ms. The exact boot-ROM reload is unavailable, so
  this interval is an emulation assumption.
* On the 192, shared receive mailboxes 0 (general), 2 (network management,
  mask `0x03f`) and 3 (`0x600..0x67f`, mask `0x07f`). The driver changes
  their identifiers as clients register but skips programming their mode.
  A general acceptance mask on mailbox 0 and receive mode on all three
  allow the software filters to dispatch multiple registered identifiers.

## Vector tables

When the OS takes over the vector base register it copies, from the
previous one:

    +0x000  60 handler addresses, indexed by exception/interrupt code / 0x20
    +0x400  60 bytes: interrupt mask level to run each handler at
    +0x43c  49 handler addresses for TRAPA 15..63

and then overrides the entries it implements itself.  TRAPA 0x3d, 0x3e and
0x3f are never overridden: they are boot ROM services.

## Services

Called with `TRAPA #n`, function code in r0, arguments in r4-r7, result in
r0.  Function codes seen in the modules:

| trap | codes |
|------|-------|
| 0x3d | 0102 0103, 0201-0205, 0401 0404 0405, 0507 0509 050a 050b 050d, 0601-0603, 0901-0903, 0c01-0c04 |
| 0x3e | 01-0a, 12, 13 |
| 0x3f | 03 04 06 |

Understood so far, trap 0x3d:

| fn | arguments | meaning | evidence |
|----|-----------|---------|----------|
| 0201 | - | DMA master enable: DMAOR = (DMAOR & 0xc000) \| 1 | The 192's OS calls 0201, 0202, 0203 in this order before every disc read, with the arguments the update loader passes to three plain subroutines of its own (0x8df928b8, 0x8df92838, 0x8df92850 in V220's) that do these things; the loader shares its hardware layer with the boot ROM. |
| 0202 | channel, destination, source, count | sets a DMA channel's DAR, SAR and DMATCR | For the drive: channel 1, the buffer, 0xa7880000, words. |
| 0203 | channel, value | writes the channel's CHCR and returns it read back | The drive's is 0x00094021. |
| 0204 | channel | returns the channel's CHCR | By the place in the loader's code; the OS has the call but does not use it. |

Without these the 192's installed software could send the drive its
commands but never got any disc data: every read of a data disc timed
out (`$CD$ disc_read(Error3)`).

Trap 0x3e:

| fn | arguments | meaning | evidence |
|----|-----------|---------|----------|
| 1 | n | pointer to a version string | The application compares the first 3 characters of `fn1(1)` with "V25" and "V35". |
| 4 | pool | get the pool's memory break | Modules do `base = fn4(0); ...; fn5(0, base + size)` to claim their static RAM, |
| 5 | pool, address | set the pool's memory break | and the OS starts by setting it to the end of its own data (0x8c19c84c). |
| 6 | three out-pointers | start-up status | The OS clears all RAM when this returns 9. |
| 9 | pointer to two block lists, two further arguments, 2 | 192 CAN state handover | V2220371 calls it from `801c75c0`; the two lists are built with `801c8568` and `801c4dcc`. |
| 0x13 | - | pointer to the software index block (SOFIDX, NAVVER, AUDVER) | The OS compares the radio processor's reported version with the AUDVER text found through it; the block sits in the module directory at flash 0xC01C. |

Seen but not understood:

| trap/fn | arguments observed | notes |
|---------|--------------------|-------|
| 3e/3 | 0 | Called after the module directory fails verification, right after 3d/0901(0xc000, 0x100).  Probably "restart". |
| 3e/7 | 0, 1, 10, -2 | Called periodically from the tick. |
| 3d/0901 | address, size | See 3e/3: probably erases flash. |

Unknown services return 0.  `rnse boot --trace-bios` logs every call with
its caller.

## The 193's boot ROM

The stand-in serves both models; what differs for the 193 (variants V574,
V575 and VB48):

* Reset is entered as code at address 0: the loader ends an update with
  a jump there (after giving the watchdog time to do it first).
* Vector tables: 64 handlers at the vector base, used both for exception
  codes 0x000-0x1e0 (the first 16) and for TRAPA 0-63, and 125 interrupt
  mask bytes at +0x780.
* It starts the installed software itself, module by module; see
  [hardware-193.md](hardware-193.md).
* Service 0x13 returns the address of the version block behind the
  entries of the directory at 0x400000 (0x8040007c with six modules).
* Service 0x0b replaces the 192's service 9: it hands the state of the boot ROM's CAN software
  to the OS.  The argument points at two list addresses; each list ends
  in 0xffff and names blocks of the OS's memory to fill: ten small ones
  in the first (number, padding, buffer, size, padding; 12 bytes per
  record) and nine in the second (number, size, buffer; 8 bytes). The
  192 uses 12-byte records for both lists. Of the
  second, block 2 is the table of received messages (36 entries of 40
  bytes: enabled, identifier, then three times handler, buffer and
  length) and block 8 holds the identifiers 0x271 and 0x2c3 and a
  "received" flag for each: the ignition lock's message, which is what
  wakes the unit.  The OS replaces the handlers of those two entries
  with its own and reads the first data byte through the entry's buffer
  pointer to decide whether to start.  The stand-in fills in the two
  entries and the flags from the ignition frame the emulated car is
  sending; without that the OS logs `CAN_Pass_mzas() Error` and treats
  the ignition as off.
* The first list's block 1 is the network flags (two bytes), block 5 is
  its state (four bytes). The second list's block 5 is transmit enable
  (two bytes). The stand-in supplies flags `04 00`, network start state
  1 and transmit enable 1, following the OS's own cold initialization.
  Without transmit enable or the CAN timer, transport opens and periodic
  status messages remain queued even though the UI and CAN reception work.
* Services 1, 4, 5 and 6 are used as on the 192.

