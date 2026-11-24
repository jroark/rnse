title: Reverse engineering
order: 60
summary: How the unit was worked out from one update CD, then checked against a real unit on a bench, mistakes included.
---
There is no schematic of the RNS-E, no public manual for the 193's chip, and the boot
ROM is on no disc. The starting point was **one firmware update CD**. This page
follows the method from there: first letting the firmware explain the hardware,
then putting a real unit on a bench to find out where that explanation was wrong.

## Rule one: the firmware knows

The update CD contains everything the unit runs, compiled for an SH-4. So the
first piece of work was a processor that runs it correctly. Everything after that
followed one loop:

1. Run the firmware until it stops making progress.
2. See what it was waiting for: a register it polls, an interrupt that never comes,
   an answer that never arrives on a serial line.
3. Model just that, with the smallest model that the firmware's own behaviour
   supports.
4. Write down how sure that is, and why.

Three techniques made that loop fast.

**Unknown hardware behaves like memory.** Any address with no model reads back
what was last written to it, so the firmware keeps going. Every access can be
logged. Bringing up a new peripheral starts with a log of what the firmware does to
its registers, in what order and with what values, and only then a model.

**The firmware's own log.** The update loader prints a log on a debug serial port.
The main software formats a much richer log and then sends it nowhere. The emulator
plants a breakpoint on the firmware's `printf`, so every message appears, largely in
Japanese. A few lines from an update:

```text
[fw] プロジェクトコード一致            (project code matches)
[fw] チェックサムチェック              (checksum check)
[fw] DISCTYPE:CDISC_PRG
[fw] IDX VersionUP
```

Lines like these explained how an update is started, step by step
([Firmware](/firmware/#how-an-update-starts)), long before any unit was involved.

**Infer what is missing from its callers.** The boot ROM was not available, so its
stand-in was built from what the other software expects of it: where the loader
jumps, which header bytes it reads, which services every module calls and with what
arguments. That stand-in booted all five variants for two days before a real ROM
was read, and then it turned out to be right in most of what mattered.

## Making sure the processor is right

A subtle CPU bug looks exactly like a hardware mystery, so the processor is tested
on its own. Random SH-4 programs are generated and run both in the emulator and in
QEMU, and a tool finds the first instruction where the two disagree. This turned up
a second lesson: QEMU 8.2 gets a few SH-4 instructions wrong itself (ADDV, SUBV,
ROTL/ROTR, MAC.W, MAC.L, DIV1 by zero). For those the reference is the SH-4 manual,
in unit tests, and the emulator deliberately does not match QEMU.

## The other processors

The main software talks to the radio processor, the faceplate controller, the DVD
drive and the car. None of their programs could be run. For the radio and the
faceplate, the emulator has **stand-ins**: small programs that answer just enough of
the serial protocol, as reconstructed from what the main software sends and
expects, for it to come up. The drive is an ATAPI device, and its model follows the
standard. The car is a stream of CAN frames: the ignition status first, since without
it the unit does not wake up.

The 193 was harder. Its system-on-chip has no public manual. Two pieces of outside
knowledge helped: Linux's support for the SH7770 (serial port addresses), and the
Linux driver for Renesas' later R-Car display unit, which turned out to be the same
block. The rest came from the firmware.

## Photographs

![The main board of an opened 192](/media/192-main-board.jpg "Taking a unit apart settled questions the firmware could not: how much flash and RAM, which graphics chip, what sits behind the SD slots.")

Photographs of an opened European 192 named the parts. Some answers were
immediate: two flash chips with small sectors at the bottom of each, 256 KiB of
battery-backed RAM, and the 19.6 MHz crystal behind the CPU-clock estimate. One
answer changed the project. The SD card and MP3 side turned out to be two **NEC
µPD77115A** signal processors, a documented part. Instead of guessing the protocol
between the main processor and the "card processors", the emulator now models the
NEC processor itself and runs the program the main software downloads into it.

## A unit on the bench

![A 192 on the bench with a logic analyser](/media/bench-analyser.jpg "The bench: a North American 192 out of its car, the faceplate on its cable, a fuse block for power, and a Saleae logic analyser on the CAN lines and eight test pads.")

An emulator built from a program's behaviour is right where that program exercises
the hardware and unknown elsewhere. The only way to find the rest is a real unit. A
North American 192 was set up on a bench, powered through its own connector and
alone on its CAN bus. A **logic analyser** was clipped to the bus and
to eight test pads on the main board.

Two practical facts shaped everything that followed:

- **The bench has no other CAN node.** Nobody acknowledges the unit's frames, and an
  idle bus goes to sleep after 4.7 s. A program that wants to be heard first wakes
  the transceiver.
- **No debug serial port** was found on any of the pads. So whatever a program
  wants to report has to go out on the screen or on the CAN bus, where the analyser
  captures it.

### The first program on a unit

The first program was a "probe" that should show a picture and hand the unit
back. The screen went dark and stayed dark. A minute and a half later the unit reset
and started normally.

The cause was one register write. The SDK's start-up copied the value that the stock
loader leaves in a latch at `0x16000004`. But the loader *adds a bit* to what is
there. Writing the whole value took the **faceplate controller's reset line** down
with it, and the display lighting belongs to that controller. The picture had been
there all along, in the dark. Once fixed, the same program ran, showed its picture,
sent CAN frames and handed back to the stock loader, which updated the unit.

![The SDK's beacon program running on the unit](/media/sdk-beacon.jpg "The beacon, the first SDK program to run cleanly on the unit: colour bars, progress on CAN identifier 0x7F6, and a hand-over to the stock update.")

Two more assumptions fell on the way. The watchdog waits only **105–115 ms**, not the
1.5 s the emulator had assumed, and a hand-over that copied for 107–112 ms with
interrupts off sometimes ran into it. And a CAN frame given up after 100 ms made
the controller cancel the next one too.

### Reading the unit out

![The report program showing the boot ROM's checksums](/media/report-boot-rom.jpg "The report program, page 11 of 11: a CRC for each 4 KiB of the boot ROM. All of it also went out on CAN, where the logic analyser caught it.")

The **report** program reads what the emulator had to assume and sends it out:
104 processor registers as the boot ROM and stock software leave them, both card
processors' pin straps and clocks, and the whole 48 KiB boot ROM. Since nothing
acknowledges on the bench, each frame is simply sent for 3 ms. The analyser caught
6.8 rounds of the stream in 265 s, and the emulator's `rnse report` command put them
back together, ending in the CRC the unit had computed itself.

What it corrected:

- the processor's identification registers (PVR, PRR, CVR), which the emulator had
  taken from the SH7750R's manual;
- the **peripheral clock**, now pinned at 58.803 MHz by timing the card processors'
  crystal against the main timer;
- a handful of reset values, a DMA bit that reads 0, and the card processors' clock
  register;
- and the **boot ROM** itself, which now runs in the emulator in place of the
  stand-in.

With those changes the same program, run in the emulator under the unit's own boot
ROM, reports the same as the unit in all but a handful of its 104 values. Those few
are the levels of lines that depend on the moment.

### What the drive really does

![The drive log program](/media/drive-log.jpg "The drive's log: each line is a time in milliseconds and the drive's answer (sense key, code, qualifier) while a disc goes out and back in. READ 256 IN 1510 MS is the read test.")

One question mattered for safety: what happens when a disc is inserted while the
unit runs. The **drive log** program recorded each ATAPI answer with its time while
a disc went out and back in. It showed something the emulator had wrong: **the
drive does not spin up a disc by itself**. It waits to be told to start, which the
stock software does and the SDK did not. With the command sent, the drive was ready
5.65 s later.

### Discs are expensive

![A pile of burned CD-Rs](/media/discs.jpg "Every early test on the unit meant a burned CD-R.")

Each experiment meant building an image, burning a CD-R, inserting it and waiting
for the unit to start it. After a few rounds of this, the next tool was obvious: a
**loader on one disc that runs programs from an SD card**. With it, a test is a
file copied to a card. The [Custom firmware](/custom-firmware/) page describes it.

A **suite** program then put many questions to the unit in one run (18 at its
largest). Results included every key and knob code, both SD slots' card-detect
lines, the display's open/closed sensor, how fast each kind of memory really is, the
flash chips' own description of their erase and write times, the radio processor's
version and status, and the whole battery-backed memory. That memory image lets the
emulator start with a unit's real settings, and it is private: it holds
destinations and the car's details.

## Measured or assumed

The project keeps one distinction above all: what has been **measured on a unit**
and what is **inferred from the software**. The documentation says which is which,
line by line. A unit's answer outranks any inference, however tidy. The 193 was all inference
until a bench unit arrived on 20 November; [its page](/hardware-193/#on-the-bench)
says what that unit has confirmed and what is still inferred.

## Timeline

| Date (2026) | Step |
|---|---|
| 12 Nov | SH-4 core and disc parser; the 192's update loader runs end to end; the main OS boots with a boot ROM stand-in; its debug log is surfaced; CAN, graphics, faceplate and a browser front end; the 193's loader and OS start |
| 13 Nov | the 193 reads a map DVD and draws the map; GPS fix and a moving car; the WebAssembly build goes on rns-e.com; the SDK and a GDB stub; the cache model; audio CDs play |
| 14 Nov | the boards photographed and their parts identified; the NEC card processors emulated, MP3 from SD on the 192; the first programs on a real unit; the report reads the boot ROM; drive, watchdog and clocks measured; programs from an SD card; the SDK's MP3 player heard on the unit |
| 15 Nov | a program installed in flash and removed again; a base firmware with radio and MP3 player; a modern interface for the 192; a persistent SD test loader |
| 16–17 Nov | Doom, Quake, Windows 3.11, a Mac Plus and an NES emulator, each from an SD card; a DVD player; a benchmark of the SD slots; an SD bridge on a Raspberry Pi Pico, through which a phone projects Android Auto |
| 18 Nov | the SD slots read at 2 MB/s on the unit; Android Auto on the 192's own display, Google Maps at 10 to 13 pictures a second, driven by the unit's knob and keys, with the phone's sound |
| 20 Nov | a 193 on the bench: its boards photographed, the first programs on it, its processor identified as an SH-4A; an SD loader for the 193, the screen, the faceplate and both SD slots working |
| 21 Nov | Android Auto on the 193's display, full screen at 30 pictures a second; a monitor on a bench 193 driven over CAN from scripts, which finds the kit's clock running twice too fast, the reason for its silent CAN bus, and what the radio processor wants so as not to restart the unit |
| 23 Nov | Android Auto on the 193 through one ESP32-S3: the phone over Wi-Fi with no Bluetooth, and the ESP32-S3 itself the SD card, read at 2.8 MB/s with every word right |
