title: The emulator
order: 50
summary: rnse, an RNS-E emulator in Rust that runs the unit's real firmware: what works, how it is built, and how to use it.
---
**rnse** is an emulator for both generations of the RNS-E. It is written from
scratch in Rust, with **no dependencies outside the standard library**: the ISO 9660
reader, the PNG writer and the little web server are all its own. It does not
contain any firmware. It runs the real thing, taken from an update CD you own.

![The emulator's browser front end](/shots/ui.png "The browser front end: a photograph of the faceplate with live keys and knobs, the emulated screen, and the firmware's own debug log below.")

> **Run it now:** [rns-e.com/emulator](/emulator/) is the emulator compiled to
> WebAssembly. Choose an image of your update disc (and, optionally, a navigation
> disc for the 193). The page reads them on your computer; nothing is uploaded.

## What runs

| | 192 (V220, V222) | 193 (V574, V575, VB48) |
|---|---|---|
| Update loader | boots, reads the CD, erases and writes the flash; the result is byte for byte the disc's modules | the same, on the 800 × 480 screen |
| Main software | boots to its user interface: radio, media, telephone, navigation, car and set-up screens, drawn through the Coral chip's engine | boots to its user interface through the 2D engine and display planes |
| Keys and knobs | all, through the emulated faceplate controller | all |
| Audio CDs | play, with sound through the amplifier's volume | play |
| MP3 from SD | the emulated NEC card processors run the firmware's own program and decode | the main processor decodes; listing, time and playback |
| Navigation | no map yet (no GPS model and no 3D) | reads a map DVD or card, draws the map, takes a GPS fix and a moving car |
| Updates from a disc | the installed software takes a newer disc and updates itself | the same |

![The 192's update loader in the emulator](/shots/loader.png "The 192's update loader, writing the flash in the emulator.")
![The 193's update loader in the emulator](/shots/loader-193.png "The 193's loader on its 800 × 480 screen.")

## How it is built

From the bottom up:

- **The CPU** (`src/sh4/`): a big-endian SH-4 interpreter covering the integer
  instructions, the FPU and a translation buffer, plus the SH7750R's on-chip
  peripherals (interrupt controller, timers, serial, DMA). It optionally models
  the instruction and data **caches**, so that code which forgets to flush them
  fails here as it would on a unit.
- **The board** (`src/board.rs`): the memory map, chip selects, ports, the
  watchdog and interrupt routing. Two machines share the core: the 192 with its
  chips on the bus, and the 193, where an on-chip block takes almost every
  peripheral address.
- **The devices** (`src/dev/`): real chips modelled from their behaviour (Coral and
  Q2 graphics, ATAPI drive, flash, CAN, serial channels, A/D converter, SD hosts, the
  NEC card processors with their own instruction set), and *stand-ins* for the
  radio processor and faceplate controller. The stand-ins answer just enough of
  their serial protocols for the main software to come up.
- **The boot ROM** (`src/bootrom.rs`): a stand-in whose service entry points stop
  the CPU and are carried out in Rust. A real ROM image can be put in its place
  ([more](/firmware/#the-boot-rom)).
- **The machine** (`src/machine.rs`): start-up sequences, the run loop, emulated
  time (counted in CPU cycles), CAN traffic from the car, and scripted input.
- **Front ends**: a command line, a local web UI, a native macOS app with the
  photographed faceplate, and the WebAssembly build behind this site.

Time is counted in CPU cycles, one instruction per cycle. That is an estimate; the
real unit was found to be a sixth faster in a tight loop and much slower whenever it
goes past the cache. A boot to the user interface takes a few billion instructions.
That is about 6 s of emulated time on the 192 and 15 s on the 193.

## How it is tested

- **Differential fuzzing.** Random SH-4 programs run both in rnse and in QEMU, and
  the first instruction where they disagree is pinpointed. A handful of instructions
  are excluded because QEMU 8.2 itself gets them wrong. Those are tested against
  the SH-4 manual instead.
- **End-to-end runs** with a real disc. Every variant's loader installs the
  disc, and the flash must equal the modules byte for byte. The installed OS must
  reach its idle loop, draw its interface and react to a key. On the 193 a map must
  be drawn; and on both units an MP3 on a card image must come out as sound.
- **`--strict` mode** for realism: memory starts with arbitrary contents, the
  watchdog is armed from the start, caches are modelled, and any use of hardware
  the emulator does not model is listed. Custom software is tested this way before
  it goes near a unit.

## Using it

A disc image is a plain copy of the CD (`dd if=/dev/sr0 of=disc.iso bs=2048`).

```sh
rnse info disc.iso                    # variants and modules on the disc
rnse ui disc.iso --variant V222       # faceplate in a browser, at 127.0.0.1:8080
rnse loader disc.iso --save-flash flash.img   # run an update against an empty unit

# Headless and scripted: open set-up, turn the knob, take a screenshot
rnse boot disc.iso --ignition --cycles 2800000000 \
    --key SETUP@7000 --knob 1:2@8500 --screenshot menu.png
```

### Listening to the firmware

The firmware writes a detailed debug log, but on a real unit the main software
formats its messages and sends them nowhere. The emulator puts a breakpoint on the
firmware's own `printf` and prints each message as an `[fw]` line. Much of it is
Japanese, in EUC-JP:

```text
[fw] _/_/_/ [Fugue]Formatting system ... _/_/_/
[fw] Call PWM_Init_Proc()
[fw] KLINE task started!
[fw] Now State[1] Change State[3]
[fw] [FP] DISP ON
```

Beyond that there are switches to log every access to a device's registers, CAN
frames, serial traffic, drive commands, SD commands and boot ROM calls. There is
also a **GDB remote stub** (`--gdb PORT`) for stepping through firmware or custom
programs at source level. How these tools were used is the subject of
[Reverse engineering](/reverse-engineering/).

## In the browser

The version at [/emulator/](/emulator/) is the same Rust code compiled to
WebAssembly and run in a web worker. It needs no server of its own. You choose your
own disc image; the page reads it in place and never uploads it. It is slower than
a native build, so a boot takes a little patience.
