title: Linux on the 192
order: 75
summary: A Linux kernel and a BusyBox shell on a real 192, started from an SD card and typed on with the faceplate's knob.
---
The 192's main processor is an SH-4, a CPU that Linux has supported for twenty years.
The project runs a current **Linux kernel (6.18) with a BusyBox shell** on it. It
starts from an SD card, shows its console on the unit's own screen, and takes its
typing from the faceplate's knob. It was brought up in the emulator first and then
started on the bench unit.

![Linux on the bench unit](/media/linux-shell.jpg "A North American 192 running Linux. The first screen shows the kernel, the processor and its caches, the memory, the uptime (2.8 s from the kernel's start), BusyBox, the screen, the two input devices and the serial ports. Below that is how to type with the knob, then the shell's prompt.")

## How it starts

Nothing in the unit's flash is changed. The same chain as for any
[program from an SD card](/custom-firmware/#programs-from-an-sd-card) does the
starting:

1. The stock software sees a custom disc, the SD loader, and starts it as it would an
   update.
2. The loader starts `RNSE.BIN` from the card. Here that is a small second stage,
   written with the project's SDK.
3. The second stage reads the kernel, `VMLINUX.BIN`, from the same card and enters
   it. The file is 3.1 MB: the kernel, with BusyBox in a compressed in-memory root
   file system. At the card processors' pace, measured at 293 KB/s, it takes about
   11 seconds to read.

![The SD loader on the unit](/media/sd-loader.jpg "The SD loader on the unit, waiting for a card. Its footer gives the two physical controls that work on every screen: the key by the power button for the display, CD EJECT for the disc.")

Before the jump the second stage leaves the hardware quiet: interrupts masked,
timers and DMA stopped, the operand cache written back, and the instruction cache
emptied. The stock operating system ran from the same addresses, so its code would
otherwise still be sitting in the instruction cache. It also writes its own address
into the kernel's command line, so that Linux knows where the way back is.

The kernel is built from unmodified upstream source, downloaded at build time and
checked against its hash. The project adds:

- a board file;
- a driver for the controls;
- a framebuffer driver for the graphics chip;
- a small patch that adds the board to the kernel's build and makes it recognise the
  unit's processor;
- a big-endian musl toolchain and a static BusyBox 1.37.0.

## What the 192 wanted of Linux

Each of these was worked out in the emulator before the kernel reached the unit.

- **A watchdog with 105 ms of patience.** An external watchdog on the board resets
  the unit unless its strobe line toggles at least every 105 to 115 ms. The board
  code toggles it every 10 ms from a kernel timer started as early as possible. The
  stretch before that, the kernel's own start-up, is the danger. Early serial output
  (`earlycon`) alone, at 115200 bit/s with nothing else running, was long enough to
  reset the emulated unit. So there is no early console, and the delay loop's
  calibration is preset rather than measured; the BogoMIPS figure on the screen
  comes from that preset. The longest pause the emulator now sees is 16 to 33 ms.
- **A processor revision Linux did not know.** The unit's SH-4 reports a product
  revision register (PRR) of 0x200. Linux knows the SH7750R only by 0x10, so it
  treated the chip as an unknown SH-4. The patch adds the unit's value, and the
  caches then come out right: 16 KiB for instructions and 32 KiB for data, both
  two-way.
- **Interrupts nobody takes.** The board's companion chip drives the processor's
  external interrupt lines for the drive, CAN, the card processors and more. Linux
  runs with interrupts open, so a request that no driver takes becomes an endless
  stream. The board code sets every one of those sources to level 0.
- **A little-endian screen on a big-endian bus.** The Fujitsu graphics chip is
  little-endian and its byte lanes are not swapped, so pixel *N* is the byte at
  *N* XOR 3. To the kernel that is a "foreign-endian" framebuffer, which its own
  drawing code already handles. The console is 400×235 at 8 bits through the chip's
  palette: 50 columns by 29 lines in the 8×8 font.
- **A faceplate that wants to be talked to.** The screen's backlight belongs to the
  faceplate controller, which has to keep being answered. The kernel takes the
  controller over as the loader left it, lit, and carries on answering it.

## Typing with a knob

The faceplate has no keyboard, so the controls driver types with what is there. The
select knob runs through a ring of characters, and the character it stands on is
typed at once and replaced as the knob turns. The shell sees ordinary keystrokes and
knows nothing of the knob.

| Control | Types |
|---|---|
| Select knob | The next or previous character |
| Pressing it | Keeps the character; with nothing being chosen, Enter |
| NEXT, PREV | Jump between `a`–`z`, `0`–`9`, symbols, `A`–`Z` |
| RETURN, short | Backspace |
| Soft keys | Tab, Ctrl-C (top), Space, Enter (bottom) |
| Volume knob | The shell's history, up and down |

The typed keys come from an input device of their own, "RNS-E rotary keyboard". The
raw buttons and knobs are a second device, "RNS-E faceplate", for programs that want
the controls themselves.

## Eject and the way back

The project's [rules](/custom-firmware/#the-rules) hold under Linux too. A kernel
thread looks after them every 10 ms, so they do not depend on anything in user
space:

- The key by the power button folds the display down and back up. The kernel runs
  the display's motor and stops it once the position has stood still for 300 ms, or
  after 8 s in any case.
- CD EJECT folds the display if needed and ejects the disc.
- Holding RETURN for two seconds returns to the SD loader:
  1. The kernel stops its timers, DMA and every interrupt source.
  2. It writes the caches back to memory.
  3. It switches the MMU off and calls back into the second stage, which puts the
     loader back.

  The loader then waits; it does not start Linux again by itself.

The way back was also the first thing the unit corrected. On the bench, holding
RETURN under Linux went straight to the stock software instead of the loader. To
leave Linux the kernel masks interrupts, which also stops its watchdog timer. It then
flushes all 64 MiB through the data cache: two million cache instructions with no
toggle on the strobe. The emulator, which counts every instruction as one cycle, put
that at 50 ms. The real unit took more than 105 ms, and the watchdog reset it.

Slowing the emulated processor against its timers reproduced the reset. The fix
toggles the strobe every 128 KiB of the flush, which keeps the longest gap to 16 ms,
and with it the slowed emulator returns to the loader. This is the pattern of the
whole project: the emulator is fast at finding problems, and the unit is the judge
of timing. See [how it is tested](/reverse-engineering/#how-it-is-tested).

## What is not done

Linux on the 192 has its console, its controls, its serial ports and its way out.
It has no drivers yet for:

- CAN;
- sound, which needs the radio processor and the card processors;
- the SD cards themselves (the kernel arrives on one, but cannot read it);
- the companion chip's interrupts.

The same work continues toward a Linux that can be installed in flash and started by
the unit by itself, with drivers for the rest of the hardware and a user interface
of its own.
