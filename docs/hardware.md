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
| 0x07aN0000            | serial channels, 16-bit registers (N = 1, 2, 3, 5, 6, a seen) | 1, 3 and 5 identified, see below |
| 0x07b00000            | interrupt priority and status registers | mostly understood |
| 0x07b80000            | unknown (3 registers written at start-up) | - |
| 0x07c00000            | CAN controller with the Renesas HCAN2 register layout (mailboxes from +0x100, 0x20 bytes each) | emulated as far as the OS uses it |
| 0x07d00000, 0x07d80000 | A/D converter (results at +0..+6, control at +8) | certain |
| 0x07e00000            | unknown | - |
| 0x0c000000-0x0fffffff | SDRAM, 64 MiB | certain (loader heap ends at 0x8fffffff) |
| 0x10000000-           | battery-backed SRAM for settings | likely (OS reads and writes it heavily; loader reads a "slot number" with complement check at +0, +1) |
| 0x12N00000, 0x12N00001 | byte-wide ports, N = 3..a: direction register (1 = output) and data | see "Ports" |
| 0x12d00000-6          | output latches | bits unknown; 0x12d00001 is driven around CD DMA transfers |
| 0x15000002, 0x1500004a | 16-bit host ports of the two SD card/MP3 processors | see "SD card processors" |
| 0x16000000-0x16000020 | 32-bit control registers | unknown |
| 0x17c00000-0x17c00014 | GPS baseband, control registers | see "GPS" |
| 0x17e00400, 0x17e00800 | GPS baseband, further register blocks | see "GPS" |
| 0x18000000-0x19fbffff | Coral video memory | certain |
| 0x19fc0000            | Coral host interface registers | certain |
| 0x19fd0000            | Coral display controller registers | certain |
| 0x19ff0000            | Coral drawing engine registers | by the chip's documentation; unused by the loader |

### Ports

The OS addresses its byte-wide ports through a table of eight (index 0-7
= 0x1230.. 0x12a0, step 0x100000).  What is known of the bits:

| port (data register) | bit | direction | meaning |
|----------------------|-----|-----------|---------|
| 0 (0x12300001) | 0 | out | watchdog strobe, toggled periodically |
| 2 (0x12500001) | 4 | in  | supply voltage good.  Low for about six seconds and the OS masks all interrupts and waits for the watchdog to reset it |
| 3 (0x12600001) | 4 | out | display motor, towards closed |
| 3              | 3 | out | display motor, towards open |
| 3              | 2 | in  | display position switch: low only when fully open |
| 3              | 1 | in  | polled every tick, unknown |
| 4 (0x12700001) | 7 | in  | polled every tick, unknown |

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
faceplate.  The others presumably serve GPS and diagnostics.

### Radio/audio processor (channel 1)

A second processor handles tuner, amplifier and power.  The OS will not
leave its "system software is being checked" notice until this
processor has reported a software version that matches the `AUDVER`
entry of the installed set.

Frames: `start, command, length (16 bit, big-endian), data, checksum`
with start = 0x01 from the unit and 0x00 from the radio, and the
checksum the complement of the sum of everything before it.  The unit
repeats an unanswered frame five times, 600 ms apart.  Command 0x0B
asks for the version; the answer carries 16 bytes of which the first
eight are the version text (`A2210141` for this disc).

The emulator's stand-in answers 0x0B and acknowledges everything else
with an empty frame of the same command, which is a guess: the proper
replies to 0x80, 0x81, 0x17 and 0x19 are not known and the unit keeps
retrying them.  The firmware also contains a second, FT1.2-style parser
(`68 L L 68 ... CS 16`) for this processor's boot loader.

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

The power button is not among the keys: it has to work while the main
processor is off, so it presumably goes to the radio processor.

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
identified; they read 0 in the emulator.

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
  READ SUB-CHANNEL, as on the 193.  No sound comes out.  The display-
  open key works (the display's position follows the motor outputs) and
  shows the drive and the two card slots.
* The SD/MP3 processor stand-ins accept the firmware download and startup
  commands. Both slots report empty; card images and MP3 audio are not
  implemented (see "SD card processors").
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
their own (the firmware calls them DSP 0 and DSP 1), each behind one
16-bit host port: 0x15000002 and 0x1500004a. `src/dev/media.rs` answers
their host protocol, without executing the downloaded DSP program.

The following was traced through the V2220371 driver and checked with
V2220317 as well:

| Register | Meaning |
|----------|---------|
| 0x12400001 bit 2 / bit 0 | DSP0 / DSP1 receive FIFO empty (1 = empty) |
| 0x12400001 bit 3 / bit 1 | DSP0 / DSP1 transmit busy (1 = busy) |
| 0x16000004 bit 2 / bit 3 | DSP0 / DSP1 reset release; the driver pulses these low |
| 0x07b00016 bit 2 / bit 3 | DSP0 / DSP1 receive edge latch; cleared by writing 0 |
| 0x07b0001e bit 2 / bit 3 | Receive interrupt mask; the driver clears these to enable |
| 0x07b00000 bits 11:8 / 15:12 | DSP0 / DSP1 interrupt priority |

At startup the OS downloads the same embedded program to each port.
There are **four sections**, each starting with a block count. Each
block sends a length, an address and its data, then reads one acknowledgement
word. Lengths in the first two sections count pairs of 16-bit words;
the final two sections count individual words. Empty sections are allowed.
V2220371 sends 29,452 payload words in nine blocks. The acknowledgement
contents are not checked by this uploader; the stand-in returns zero.
The firmware's source file uses little-endian lengths and addresses,
converted by the SH-4 driver before writing these 16-bit port words.

Runtime commands have a header whose low byte counts following words.
The receive handler puts only the response header into a software FIFO;
the task reads the response payload directly from the host port. The
stand-in raises one receive edge for each response, not each payload word.

| Command and arguments | Stand-in reply | Purpose / confidence |
|-----------------------|----------------|----------------------|
| `1202 d762 0034` | `f000` | System initialization; the driver logs `DSPC_SYS_INIT Success` |
| `7301 mode` | `f000` | Host transfer mode; startup selects 2 |
| `3301 enable`, `3101 enable` | `f000` | Library/output startup controls; exact semantics inferred |
| `3201 0000` | `d503 0000 0000 0000` | Library identity; synthetic version 0.0, build 0 |
| `2203 0001 5000 selector` | `d301 0000` | Card status; both slots absent |

Other commands are left unanswered. The first command is sent with
polled I/O; later blocks use SH-4 DMA channel **7 for DSP0**, **3 for DSP1**,
with 16-bit transfers. The device provides requests only for available
FIFO words on reads, and accepts writes immediately. The companion DMA
request selectors are at 0x12d00007 and 0x12d00003. This is a simplified
data-ready model, not a complete emulation of the DDT request bus.

The OS also configures port interrupt controls at 0x12500000 and
0x12900000. Card insertion and removal events on those ports are not
implemented. The firmware regression test `media_dsps_boot` checks both
downloads, the initialization replies, and the switch to DMA without
`BOOT ERROR` or `ERR_cmd_dma` messages.

The processor type is not known: the program is 16-bit words with what
looks like a table of eight-word interrupt vectors at its start and no
readable text.  Emulating SD card access and MP3 playback the way the
unit does it means either running that program (which needs the
processor identified) or extending the host command stand-in. The next
missing pieces are card/image access and file commands, followed by MP3
decoding and an audio output path. The 193 uses a different media
interface; this device is only mapped on the 192.

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
