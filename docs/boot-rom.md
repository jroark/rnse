# The boot ROM

The first 48 KiB of flash (0x0000-0xbfff on V220 hardware) hold a boot ROM
that an update never rewrites, so it is not on the disc.  The emulator
replaces it with a stand-in (`src/bootrom.rs`) built from what the other
software expects of it.  This file records that evidence.

A real one has since been read: `sdk/examples/report` sent the 48 KiB
of a North American 192 out on the CAN bus, and they came back whole,
with the CRC-32 the unit computed (0xf7f42e55).  The image is firmware
and stays out of the repository.

## The real one, in the emulator

`rnse boot disc.iso --variant V222 --boot-rom rom.bin` puts such an
image where the stand-in is and starts at the reset vector; nothing is
then called or answered by the emulator.  With the image of that unit:

* The installed software starts and comes to the radio's picture, with
  the same picture and, line for line, the same log as with the
  stand-in.  An update runs whole: the application takes a disc with a
  higher index, the loader erases and writes, prints `*** Jump User
  RESET ***`, and the ROM starts the new software.
* Two things had to be added to the emulator for it, both in
  `src/sh4/onchip.rs`: the refresh timer's flags (the ROM clears RTCSR's
  overflow flag and waits for it, four times, before it goes on), and
  the level of the NMI pin in ICR bit 15 (the ROM looks at it first
  thing; with the pin low it goes to a routine that strobes the
  watchdog and waits on timer 0, which nothing has started).

What the image says about itself, and what the stand-in was changed to
after it (for the 192; the 193's is still all inference):

* Its header at 0x400 is `.PRG`, `V220100D`, `Sep  3 2003`, `11:41:33`,
  `AISIN AW co.LTD`, each text ending in CR LF.  The project code there
  is V220 on a unit that runs V222 software; the V222 loader prints it
  (`curprjct=V220`) and updates from a V222 set all the same.  The
  stand-in's header has that layout and says V220 on every 192.
* Offset 0 is code, not an address: `mov.l`, `jmp` to the reset code at
  0xa0000fa0, whose address is the long word at 8.  The loader's
  `jmp @[0]` at the end of an update therefore jumps to 0xd001402b, an
  odd address; the address error is the loader's to handle, and its
  handler goes to the reset code.  So an update ends in the ROM's reset
  code, which sets the ports and the latch anew: that is the "reset" a
  logic analyser sees on a unit then, with no reset line involved.  The
  stand-in has the same three instructions and its reset hook at 0xfa0.
* The reset code sets the bus controller (BCR1 0x01000008, BCR2 0xfd38,
  WCR1 0x11111011, WCR2 0x2082624d, MCR 0x00029018 and then its mode
  bits), the ten registers at 0x16000000-0x16000024 (among them
  0x1600001c = 0x806898c0, which is where that value comes from, and
  the latch 0x16000004 = 0x3f8f) and the direction and data of the
  eight ports (port 7, the CAN transceiver's: direction 0x1f, data
  0x10).  It does not clear RAM.
* What it leaves when it calls the OS's first entry point: SR
  0x40000010 and its own vector base (0x80000470), so its interrupt
  handlers are at work until the OS has its own; the stack at
  0x8fffffb0; DMAOR 0xc001; IPRA 0x7650; the three timers running, each
  at 5 ms (73499 counts) with its interrupt on; CAN mailbox 0 receiving
  everything (identifier 0x2c3), 1 sending 0x436, 2 receiving
  0x400-0x43f, 3 receiving with mask 0x7f.  The stand-in leaves the
  same, except that it cannot take interrupts before the OS can: its
  CAN timer starts with the CAN handover, now at 5 ms where it had 2.5,
  and timer 1, which the ROM's own CAN software uses for half a second,
  it does not run.
* The services, read from the image and compared call by call with the
  stand-in's answers in a start of the same software (`--trace-bios`
  prints the real ROM's in the stand-in's words): see "Services".
  With those changed the two agree on every call of a start, and the
  firmware's log is the same line for line, with RAM full of arbitrary
  data as well (`--strict`).
* It has a driver for the drive (0xa7800000, 0xa7880000), the display,
  the three serial channels 1, 3 and 5, CAN and the A/D converter, and
  nothing for the SD card processors (no 0xb5000002, no 0xb500004a):
  whatever it can start by itself, it starts from the disc.

## What other software reads from it

| what | evidence |
|------|----------|
| Offset 0 | The loader ends an update with `jmp @[0]` (after printing `*** Jump User RESET ***`).  On a 192 what is there is code, and the jump ends in the reset code by way of an address error (above). |
| Offset 0x400: a header of texts; +6 is the version, which begins with a project code, e.g. `V220` | The loader copies four bytes from 0x80000406 as the "current project" (`curprjct`).  With `V220` there the V222 loader takes a V222 set; without a code it knows it tries its built-in list (`V220`, `V264`) and reports "version up not required". |

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

That is how an update normally starts, and it works in the emulator
for all the software seen (`rnse boot disc.iso --installed-index 0640
--disc-in 9000` runs the whole update on the European 192;
`tests/disc.rs` `a_disc_put_in_while_the_unit_runs_starts_the_update`
has both units of both discs start their loader that way).  It did not
for a long time, and why is worth keeping, because it rests on an
assumption about the drive that only a real drive can confirm.

How the software sees a disc arrive (European 192: application at
0x00281168 and 0x002812e2, its request at 0x0071392a; OS at 0x800a8a36,
its table of the drive's answers at 0x800aaa56):

* A task of the application asks for the drive's state once a second
  and turns changes into events for its disc task.  Its states are 2
  (no disc), 0 (a disc, ready), 6 (a disc, not ready yet), 3 and -1.
  "Ready" becomes the event the disc task acts on (0x88000007, logged
  as `DVCEV_DISKOK(DISKIN)`) **only coming from 6, or from 1, the state
  the task starts in**; straight from "no disc" it is no event at all.
  With a disc in the drive from the start the task's first look gives
  the event.
* State 6 is what the application makes of a request to the OS that
  fails eight times in a row with the OS's codes -8 or -9, or that
  succeeds while the OS's record of the kind of disc is still empty.
* The OS makes its codes from the drive's sense data: no medium
  (2/3A/00) is -5, "becoming ready" (2/04/01, and 2/04/04) is -8, "not
  ready, cause not reportable" (2/04/00) is -9, "the medium may have
  changed" (key 6) is -6.  And it treats them differently: **on -8 it
  does not answer the application at all**, it goes on asking the drive
  itself, every 100 ms, for up to eight seconds (then four at a time);
  **-9 goes straight back**.

So what the drive says while it loads a disc decides everything:

| while loading the drive says | European 192 (V220) | North American 192 (V222) | 193 (V574) |
|---|---|---|---|
| 2/04/00, not ready | taken, for any loading time from 4 s | taken, from 2 s (shorter not tried) | taken, from 2 s |
| 2/04/01, becoming ready | only for 5 to 8 s | never (1.5 to 14 s tried) | only for 8.5 to 12 s |
| 2/3A/00, still no medium | never | for 6 s and 11 s, not 2 s | not tried |

With 2/04/00 the application's eight failures come at once, it is in
state 6 for as long as the drive loads, and "ready" is the event.  With
2/04/01 the OS swallows the wait, and the disc is taken only if the
application's request happens to return in the tenth of a second in
which the OS has seen the drive ready and not yet settled the kind of
disc: a different range of loading times for each software, with no
loading time that suits them all.  A drive that these softwares all
work with says 2/04/00, then; that is what the emulator's drive now
says (`Atapi::loading_sense`), followed as before by one "the medium
may have changed" when the disc is ready.

It was an inference; it has since been measured, on a North American
192 (its drive an Alpine DV35M110) with `sdk/examples/drive`, which
asks the drive ten times a second and logs every change, here with a
CD-R, ejected by command and pushed back in by hand (times in ms from
the program's start; the same lines came over the CAN bus):

    0000000 READY
    0005210 2/04/00     the eject command: not ready
    0008820 6/28/00     the medium may have changed
    0008930 2/3A/01     no medium (the "tray closed" form)
    0011940 2/3A/00     no medium
    0016350 2/04/00     the disc pushed in: not ready
    0019060 6/28/00     the medium may have changed
    0019170 2/04/04     not ready, a third way

**While it loads a disc the drive says 2/04/00, as inferred**, for 2.6
to 2.7 s, and the attention comes after that, not at the insertion.

**Then it waits to be told to start.**  The 2/04/04 that follows the
attention does not pass by itself: a second run left the drive alone
for 23 s and for 24 s, reading failed all the while, and whoever
watched saw the disc stand still.  The stock software does not leave
it alone: whenever the drive is not ready it sends START STOP UNIT with
the start bit, again and again, between its questions (it can be seen
doing so in the emulator).  The program had only asked.  How long the
drive takes from that command to READY is not measured yet; the
program has a key for it now (PREV).

On the way out the drive says 2/04/00 for 2.8 to 3.7 s, then the
attention, then 2/3A/01 for 2 to 5.5 s before 2/3A/00.

And it reads slowly: 256 sectors from the start of a CD-R took 1510 ms
through the program's own loop, that is some 5.5 ms a sector or 370
KiB/s.  That number accounts for something else: a unit needs about 42
s from a reset to starting a program from the disc, the emulator with
1.5 ms a sector needed 15 s, and with 5.5 ms it needs 46.

The emulated 192's drive now does all this by default: 2.7 s of
loading, the attention, 2/04/04 until it is told to start and for 2 s
after (the 2 s are a guess), 3.6 s for an eject and 3 s of 2/3A/01, and
with `--drive-times` 5.5 ms a sector.  The program's log in the
emulator is the unit's, line for line, the failed reads included.  The
North American 192 software still takes a disc that goes in while it
runs.  The 193's drive, which nobody has measured, is as it was: 6 s
of loading, nothing after it, 1.5 ms a sector.  `--spin-up`,
`--settle` and `--eject-time` change the times.

The program for it is in `docs/custom-firmware.md`.  The switches to try other drives are
`--loading-sense K/CC/QQ`, `--spin-up MS`, `--attention-first` and
`--attention-both` ("the medium may have changed" at insertion instead
of, or as well as, when the disc is ready).

Other things about the drive that were wrong and are fixed: a disc that
goes in is not ready for a while (`--spin-up`; the
real mechanism pulls the disc in by motor), and "the medium may have
changed" is reported once at the end of that, not at the moment of
insertion.  And reading can be given its
time (`--drive-times`, part of `--strict`: 120 ms for a seek, 1.5 ms a
sector).  The drive also obeys the eject and load commands now (START
STOP UNIT: the disc comes out and waits in the slot; `--disc-back MS`
or the load command puts it back).

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

The 192's boot ROM starts a loader from a disc by itself when there is
no software to start.  In the emulator, with a unit's ROM, a flash that
is empty but for the ROM (`--flash` with a file of 0xff, `--boot-rom`)
and the update disc in the drive: the ROM reads the loader from the
disc and calls it (with language 1; the loader's log has `ROMに
プログラム存在しない`, "no program in the ROM"), the loader writes all
of the software and jumps to the reset code, and the ROM starts what
was written.  So a unit whose software is gone, after an update that
was cut short or after software of one's own that does not start, is
brought back by an update disc for as long as the first 48 KiB of the
flash are whole, which no update and no loader touches.  Not tried on a
unit.  It has nothing for the SD cards.
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
  the timer.  On the 192 the ROM has it at 5 ms, and so has the
  stand-in.  For the 193 the stand-in supplies 2.5 ms, the interval the
  loaders program themselves, which is an assumption.
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
| 0205 | channel | waits for the channel's transfer to end and takes its enable bit away; after an address error clears that flag in DMAOR instead and returns DMAOR | From the ROM. |

(0201 returns the new DMAOR and 0202 sixteen times the channel number,
which is what its code leaves in r0; the stand-in does the same now.)

Without these the 192's installed software could send the drive its
commands but never got any disc data: every read of a data disc timed
out (`$CD$ disc_read(Error3)`).

Trap 0x3e:

| fn | arguments | meaning | evidence |
|----|-----------|---------|----------|
| 1 | n | pointer to a text of the ROM's header: 1 the version, 2 the version without the project, 3 date, 4 time, 6 maker, else the header | The application compares the first 3 characters of `fn1(1)` with "V25" and "V35".  The rest from the ROM. |
| 3 | 0 or 1 | restart: through the reset code, or without it | From the ROM. |
| 4 | pool | get the pool's memory break | Modules do `base = fn4(0); ...; fn5(0, base + size)` to claim their static RAM, |
| 5 | pool, address | set the pool's memory break | and the OS starts by setting it to the end of its own data (0x8c19c84c).  Eight pools in the ROM, at 0x8fffd000; the first two start as 0x8c000100 and 0x90000000. |
| 6 | three out-pointers | how often the ROM has started the software again after an exception since the last reset; through the pointers where the last one was (SPC, SSR) and a word about it.  Kept in battery-backed memory from 0x10, zero after a reset | From the ROM.  The OS clears all RAM when this returns 9, which made it look like a start-up status; the stand-in answered 9 and now answers 0. |
| 7 | n, amount | adds to counter n of eight in battery-backed memory (0x8c + 8n, with its complement) and returns it | From the ROM.  The OS adds 1 to counter 0 from its tick now and then. |
| 8 | -1 | writes 0x40000000 to battery-backed memory at 0x1c | From the ROM; purpose unknown. |
| 0x0a | - | clears the count that 6 returns | From the ROM. |
| 0x10 | n | address of entry n of the module directory (0: of the directory) | From the ROM. |
| 0x12 | n | address of the text six bytes into module n's header | From the ROM; the stand-in answered 0. |
| 9 | pointer to two block lists, two further arguments, 2 | 192 CAN state handover | V2220371 calls it from `801c75c0`; the two lists are built with `801c8568` and `801c4dcc`. |
| 0x13 | - | pointer to the software index block (SOFIDX, NAVVER, AUDVER) | The OS compares the radio processor's reported version with the AUDVER text found through it; the block sits in the module directory at flash 0xC01C. |

Seen but not understood:

| trap/fn | arguments observed | notes |
|---------|--------------------|-------|
| 3d/0901 | address, size | Called before 3e/3 (a restart) when the module directory fails verification: probably erases flash. |

Trap 0x3f is not the ROM's: its table has no handler for it, the OS
answers those itself.  A number the ROM has no service for gets -1 from
its dispatcher, and so from the stand-in on the 192 for trap 0x3e;
services the ROM has and the stand-in does not answer 0.  `rnse boot --trace-bios` logs every call with
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

