title: Investigations
order: 80
summary: Open questions under way: whether a 192 can be an Android Auto head unit with a small board in an SD slot, and the factory telephone module.
---
Not everything is finished. These are the investigations in progress, with what has
been measured kept apart from what is assumed.

## Android Auto on a 192, with nothing but its own hardware

The question: could a 192 be an Android Auto head unit **with its own processor,
display and sound**, without a more powerful computer hidden behind it?

| Android Auto wants | The 192 has |
|---|---|
| a link to the phone: USB host or 5 GHz Wi-Fi | no USB, Wi-Fi or Bluetooth for data. Something must be added, and it only has to carry bytes: a small board in an SD slot (below) |
| H.264 video at 800 × 480, 30 frames a second | a 235 MHz SH-4, 64 MiB of RAM, no video hardware. **This is the deciding question** |
| a picture | a 400 × 240 panel: exactly half of 800 × 480 each way |
| sound at 48 kHz | a card processor's 44.1 kHz output; a resampler and a program of its own for that processor |
| input | the rotary knob and keys, which Android Auto supports |

So the first experiment was decoding. **h264bench** is an SDK program that decodes
H.264 streams from an SD card with a portable baseline decoder. The streams are
synthetic but encoded the way a phone encodes for such a head unit: a panned map,
a scrolling list, full-screen motion, and a map in a 400 × 240 corner. Every
decoded frame is checked against ffmpeg's output by CRC.

![h264bench's results on the unit](/media/h264bench.jpg "The first run on the bench unit: frames per second for each stream, with memory through the write-through and copy-back cache settings. Every CRC matches ffmpeg.")

The first run on the unit decoded every frame correctly, but slowly: 5 to 12 frames a
second, about half of what the emulator's instruction count had suggested. The
difference was the cache. The stock setting writes through to memory for one of the
two address windows, and the decoder's working buffers were in it. With the caches
set up for copy-back, a second run reached **10 to 15 frames a second** for typical
content, and 39 for a list screen. That is the line where it starts to look usable.
The unit now runs within a few per cent of the emulator's figures for this code, so
the emulator is a fair guide from here.

### The way in: an SD slot

The 192 has no USB port, but it has two SD slots, each behind its own NEC signal
processor. The plan is a microcontroller that answers the slot as an SD card on one
side and is the phone's USB host on the other, carrying bytes and nothing else.

How fast can a slot go? The stock route, the unit's own card program with a message
for every sector, measured **297 KB/s** on the bench unit. That is not enough for
video and sound together. A benchmark, **linkbench**, loads a program of its own
into the card processor that runs the card itself and streams sectors without a
message each. In the emulator it reaches the card interface's modelled rate, about
**2.1 MB/s** at the stock clock, with every word checked against a test file. The
unit's own figure is still to be taken.

### A bridge on a Raspberry Pi Pico

![A Raspberry Pi Pico wired to slot 1](/media/sdbridge-in-slot.jpg "The bridge prototype: a Raspberry Pi Pico on an SD card extension in slot 1, with a resistor in each signal line. The display stays open while the ribbon is in.")

The bridge is an RP2040 board, first a Raspberry Pi Pico. Its programmable I/O
answers the SD bus with exact bit timing, and its USB port is the phone's host. Its
firmware is tested on a computer, against a simulated SD host and a stand-in phone,
before it goes near either.

The phone side has run with a real phone, a Pixel, on the Pico:

- the phone switched into accessory mode, and its Android Auto answered the head
  unit's version request;
- the TLS handshake completed (the Pico's slow step, the key exchange, takes 2 to 4
  seconds, and the phone waits for it), every channel opened, and **Android Auto
  projected Maps** at 800 × 480;
- the video is **H.264 Baseline**, the profile the 192's decoder takes, at about
  **30 frames a second** when asked for 30;
- full-speed USB carried 310 to 400 KB/s with the RP2040 at 125 MHz, where
  decrypting the stream took nearly all of its time, and up to 660 KB/s at 250 MHz,
  where decrypting takes 34 to 45 % of it.

The head unit certificate the phone requires is not published here, and like firmware
it never will be.

### Building one

![The bridge prototype on a Raspberry Pi Pico](/media/sdbridge-pico.jpg "The prototype: a Raspberry Pi Pico, three 100 Ω resistors in the signal lines to an SD card extension's ribbon, and a cut USB cable bringing 5 V to VBUS for the phone side.")

Everything is off the shelf and soldered by hand; there is no board to order.

| Part | Notes |
|---|---|
| Raspberry Pi Pico | any RP2040 board will do; the firmware is also built for a Waveshare RP2040-Zero, whose pins differ |
| SD card extension cable | a card-shaped end on a ribbon. Its card end goes in the slot; the ribbon's far end is cut open and its wires soldered to the Pico |
| 3 resistors, 100 Ω | in series in CLK, CMD and DAT0; ordinary ¼ W through-hole ones |
| micro-USB OTG adapter | micro-USB (male) to USB-A (female), for the phone side |
| 5 V supply, 1 A or more | for the phone side: a USB charger or power bank; a cut USB cable works |
| USB serial adapter (3.3 V) | optional, to read the firmware's log |

![Schematic of the bridge on a Raspberry Pi Pico](/media/sdbridge-schematic.svg "The wiring. GP2, GP3 and GP4 reach the card end's CLK, CMD and DAT0 through 100 Ω each; the grounds are joined; the card end's VDD and DAT1 to DAT3 stay unconnected.")

| Pico pin | Signal | SD card end contact |
|---|---|---|
| 4 (GP2) | CLK, through 100 Ω | 5 |
| 5 (GP3) | CMD, through 100 Ω | 2 |
| 6 (GP4) | DAT0, through 100 Ω | 7 |
| 3 (GND) | ground | 3 and 6 (VSS) |
| none | VDD | 4: **not connected** |
| none | DAT1, DAT2, DAT3 | 8, 9, 1: not connected |
| 40 (VBUS), 38 (GND) | 5 V in, phone side only | none |
| 1 (GP0) | the log's TX, to the adapter's RX, optional | none |

On the card end, contact 1 is at the notched corner and the others follow in order:
1 DAT3, 2 CMD, 3 VSS, 4 VDD, 5 CLK, 6 VSS, 7 DAT0, 8 DAT1, 9 DAT2. On the Pico,
pin 1 is the corner next to the micro-USB socket; pins 1 to 20 run down that side and
21 to 40 back up the other, so pin 40 is the other corner by the socket.

Points that matter:

- **The Pico never takes power from the slot.** The card end's VDD is left open: two
  supplies joined would fight. Only the ground is shared.
- **Power the Pico before the unit, and keep it powered while it is plugged in.** An
  unpowered RP2040 takes current through its pins from a host that drives them; the
  resistors limit that, and the current while both sides drive a line. At the slot's
  16.9 MHz and with a short ribbon, 100 Ω costs nothing.
- **One supply at a time.** With the card firmware the Pico is powered from its
  micro-USB socket, from a computer that also shows its log. With the phone firmware the
  socket belongs to the phone, through the OTG adapter and the phone's own USB-A to
  USB-C cable, and the Pico is powered with 5 V on VBUS (pin 40, ground on pin 38),
  which also feeds the phone's side of the cable.
- **Keep the ribbon short**, with a ground wire beside CLK.
- **The slot knows a card by its switch**, so the card end has to go in fully.
- **Leave the display open** while the ribbon is in, and lay the cable clear of the
  display's motor and the drive's opening.

The firmware is built with the Raspberry Pi pico-sdk, and each part comes as a UF2 file:
hold BOOTSEL while plugging the Pico into a computer and copy the file onto the drive
that appears. On a Pico the board's LED shows the state by blinking: a short flash every
2 s while it waits, steady once it is working, three short flashes after a failure.

Still open: the Pico as a card in the slot, measured on the unit; writing to the
card for the way back to the phone; the two halves joined; and on the 192, the
decoder's hottest loops in hand-written code and the sound.

## The factory telephone module

Cars with the factory phone have a separate telephone module, and the RNS-E talks
to it over CAN. With no module present the TEL screen says "Telephone is not
installed" (in the emulator, with no module answering).

What has been found:

- The unit opens a **transport channel** on CAN (identifiers `0x6C4`/`0x6C5`) and keeps
  it alive, but says nothing on it. The module has to speak first.
- What travels on the channel is **AT commands as text**: the standard GSM ones (`+CPIN?`,
  `+COPS?`, `+CLCC`, `+CPBR` for the phone book) and the module's own for status
  and Bluetooth pairing (`^CTEL?`, `^BTCN`, `^BPIN`). To the unit, the telephone is a
  modem with a phone book.
- The emulator answers the transport (channel set-up, keep-alive, acknowledgements).
  But no single bit of the module's status frame has yet made the unit begin its
  dialogue. Which combination does is the next thing to find.

The [modern 192 interface](/custom-firmware/#built-on-it) already has an
experimental client for a real module, built to dial and answer through those same
commands.
