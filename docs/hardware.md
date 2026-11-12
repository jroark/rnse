# RNS-E hardware notes (V220 variant)

There is no public documentation for this unit.  Everything below comes
from watching what the firmware does in the emulator and from reading its
code, so each item says how sure it is.  "Loader" is the update loader
(`LDM`), "OS" the main software.

## CPU

Hitachi/Renesas SH-4, big-endian.  The firmware uses DMAC channels 4-7
and their interrupt codes (0x780-0x7e0), which exist on the SH7750R and
SH7751R but not on the original SH7750.

* Peripheral clock about 29.4 MHz: the OS loads TMU0 with 14699 at Pclk/4
  for a 2 ms tick, the loader 73499 for 10 ms.
* FPSCR is 0x00040001 in every task: round to zero, denormals as zero.
* The MMU is not enabled by anything run so far.

## Memory map

Physical addresses.  The firmware mostly uses the P1 (0x8...) and P2
(0xa...) windows; the applications also run from P0.

| range                 | device | confidence |
|-----------------------|--------|------------|
| 0x00000000-0x01ffffff | NOR flash, 32 MiB, 32-bit data bus, AMD command set | certain (loader programs it) |
| 0x07800000            | ATAPI drive, command block (register n at +2n+1, data at +0) | certain |
| 0x07880000            | ATAPI drive, DMA data port | certain |
| 0x07900000            | ATAPI drive, control block (alternate status at +0x0d) | certain |
| 0x07aN0000            | serial channels, 16-bit registers (N = 1, 2, 3, 5, 6, a seen) | registers partly understood |
| 0x07b00000            | interrupt priority and status registers | partly understood |
| 0x07b80000            | unknown (3 registers written at start-up) | - |
| 0x07c00000            | CAN controller with the Renesas HCAN2 register layout (mailboxes from +0x100, 0x20 bytes each) | layout recognised, not emulated |
| 0x07d80000, 0x07e00000 | unknown | - |
| 0x0c000000-0x0fffffff | SDRAM, 64 MiB | certain (loader heap ends at 0x8fffffff) |
| 0x10000000-           | battery-backed SRAM for settings | likely (OS reads and writes it heavily; loader reads a "slot number" with complement check at +0, +1) |
| 0x12300001            | watchdog strobe: bit 0 is toggled periodically | likely |
| 0x12500001, 0x12600001, 0x12700001 | input ports | polled by the OS; bits unknown |
| 0x12800001, 0x12900000, 0x12a00000-2, 0x12d00000-6 | output latches | bits unknown; 0x12d00001 is driven around CD DMA transfers |
| 0x16000000-0x16000020 | 32-bit control registers | unknown |
| 0x18000000-0x19fbffff | Coral video memory | certain |
| 0x19fc0000            | Coral host interface registers | certain |
| 0x19fd0000            | Coral display controller registers | certain |
| 0x19ff0000            | Coral drawing engine registers | by the chip's documentation; unused by the loader |

The OS addresses its byte-wide ports through a table of eight (index 0-7
= 0x1230.. 0x12a0, step 0x100000).

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
| CAN | +0x12 bits 11:8 | - | 5 |

The OS registers handlers for levels 1-3, 5-9 and 11-14 in total, so a
dozen sources remain to be found.

On-chip sources in use: TMU0 (system tick), TMU2, DMAC channels 2, 3, 4
and 7.

## Display

Fujitsu Coral graphics controller (MB8629x family) on chip select 6.  The
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
* The OS also builds display lists for the drawing engine; that part of
  the chip is not emulated yet.

## CD/DVD drive

Standard ATAPI.  The firmware sends SET FEATURES (transfer mode 0x12),
then PACKET commands with the DMA bit set; data moves through DMAC
channel 1 in 16-bit units from 0x07880000.  The drive is wired so that the
first byte of each pair is the high half of the word.

## Serial

Each channel at 0x07aN0000 has 16-bit registers: +0 and +2 mode/baud,
+4 control, +6 transmit data, +8 status (bit 9 = transmitter busy), +0xc
control.  Channel 5 is the debug console: the loader's log comes out
there (`rnse` prints it).  The others presumably serve GPS, the radio
controller and diagnostics; the OS keeps channel 1 busy.

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

## What the OS is waiting for

Read from its debug messages after start-up:

* `CAN_Pass_mzas() Error!!` and `Now skon[0] Now Kl15[0]`: the power
  manager (`PWM`) wants the ignition lock status message from the car's
  infotainment CAN bus.  With neither key contact nor ignition it runs
  its standby and sleep timers instead of bringing the system up.
* `ROMID RETRY!!`, repeated: something is asked for an identification
  and does not answer (not traced yet).
* `SDCARD DSP0/1 BOOT ERROR`: the SD card reader's controllers are
  missing.
* `SWM::*** FATAL_ERROR *** ... Adc_get_scrntype return ZERO`: the key
  handling asks the application for the current screen type before
  there is one; probably a consequence of the above.
