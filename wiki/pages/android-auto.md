title: Android Auto
order: 77
summary: A phone's Android Auto on an RNS-E's own display, decoded by its own processor and driven by its own knob, through a small board in an SD slot: on the 192 since 18 November, on the 193 since 21 November, and since 23 November over Wi-Fi from a single ESP32-S3.
---
**A 192 can be an Android Auto head unit.** On 18 November a phone's Google Maps ran
on the bench unit's own display at 10 to 13 pictures a second, decoded by the unit's
own processor and driven with its own knob and keys, through a small board in an SD
slot ([on the unit](#on-the-unit)). The same evening the phone's music played through
the unit's own sound. On 21 November it ran on a **193** too, full screen at the
phone's own 30 pictures a second ([on the 193](#on-the-193)). On 23 November a single
**ESP32-S3** took the place of the bridge and of the phone's cable: the phone reaches
it over Wi-Fi, and it is itself the SD card the unit reads
([the whole bridge on an ESP32-S3](#the-whole-bridge-on-an-esp32-s3)).

This page follows how it got there: whether the 192 could decode the video at all,
how fast an SD slot can carry it, the bridge between the phone and the slot, the
result on the unit, and how to build the bridge.

## With nothing but its own hardware

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

## The way in: an SD slot

The 192 has no USB port, but it has two SD slots, each behind its own NEC signal
processor. The plan is a microcontroller that answers the slot as an SD card on one
side and is the phone's USB host on the other, carrying bytes and nothing else.

How fast can a slot go? The stock route, the unit's own card program with a message
for every sector, measured **297 KB/s** on the bench unit. That is not enough for
video and sound together. A benchmark, **linkbench**, loads a program of its own
into the card processor that runs the card itself and streams sectors without a
message each, every word checked against a test file. On the bench unit, with a real
card and with the bridge below, it reads **about 2 MB/s** in long multi-block reads
at the stock card clock and 2.6 MB/s at a faster one, every word right: seven times
the stock route, and more than Android Auto's video needs.

Getting there took two days of runs on the unit, because the card interface does not
behave as its manual suggests: the response register is a small ring whose read
position carries over from one answer to the next, the response interrupt fires only
on a rising flag, and a read stopped early leaves a stray word behind that spoils the
next one. Each was found on the unit, then modelled in the emulator, which now
reproduces the unit's screens.

## A bridge on a Raspberry Pi Pico

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

## On the unit

![Google Maps from a phone on the 192's display](/media/android-auto-maps.jpg "Android Auto on the bench 192, 18 November: Google Maps from a Pixel, through the bridge's ribbon in slot 2, decoded by the 192's own processor and shown on its own display. Street names blurred.")

On 18 November the two halves were joined, and **a phone's Android Auto ran on the
192's own display**. The bridge, one RP2040, is at once the phone's USB host and a
card in slot 2. It decrypts the phone's video and serves it to the slot as a file
that grows as it is read: a window onto the stream. A picture is acknowledged to the
phone only once the 192 has read its last block, so the phone sends at the 192's pace.

On the 192, a viewer started from the SD loader takes the slot over with the card
processor program from linkbench, reads the pictures in long runs, decodes each one
with the same H.264 decoder as h264bench, and shows it at half size on the 400 × 240
panel, in 256 colours.

- Google Maps came through at **10 to 13 pictures a second**, the bridge sending
  120 to 150 KB/s and spending about 30 % of its time decrypting.
- **The unit's knob and keys drive the phone.** The bridge's card has no write path
  yet, so the viewer sends input by which sector it reads: the bridge turns each into
  Android Auto's input event. The select knob is the rotary controller and its push
  ENTER, RETURN is BACK, NAV is home, MEDIA, NEXT and PREV are the media keys, and the
  corner keys move. Holding RETURN, the display key and eject stay the unit's own.
- Its first pictures were grey with a lilac cast, and the unit reset after a few of
  them. Both were found from the unit's screen and fixed: the colour conversion's
  centre was off and the palette was rebuilt, and a 48-byte buffer overflowed the stack.

![Android Auto's split screen on the 192](/media/android-auto-split.jpg "The same evening: Android Auto's split screen, media on the left and directions on the right, with the bridge on top of the unit, its ribbon to slot 2 and the phone's USB cable behind it. Street names blurred.")

**The phone's sound plays too.** Its music (48 kHz stereo) and its navigation voice
and own sounds (16 kHz mono) travel in the same stream as the pictures, their blocks
between a picture's. On the 192 the card processor that the bridge is *not* in plays
them: each source is resampled to the processor's 44.1 kHz and mixed, its pace
nudged by how much is waiting so that the phone's clock and the unit's never drift
apart, and the sound plays on by itself while a picture is decoded. The volume knob
sets the unit's amplifier. Apple Music from the phone plays through the bench unit.

Turning the select knob shows its effect a picture or two later, because each turn
travels to the phone and comes back as video.

## Building one

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

## On the 193

The 193 has a faster system-on-chip, an 800 × 480 screen and an SD host of its own,
so the same viewer was carried over to it. On 21 November, on a bench 193 (the
North American 8J0 035 193), **a phone's Android Auto ran full screen** on the
unit's own display: Google Maps in 3D beside the media card and the app bar, at the
phone's own **30 pictures a second**, every picture shown.

![Android Auto full screen on the 193's display](/media/android-auto-193.jpg "Android Auto on the bench 193, 21 November: a Pixel's Google Maps in 3D beside the media card and the app bar, full screen at 800 × 480, through the bridge in slot 2, decoded by the 193's own processor. The map is blurred.")
![The bridge's ribbon in the 193's second slot](/media/android-auto-193-slots.jpg "The same day with the display folded down: the bridge's ribbon goes into the second card slot behind it, and the Pico sits on top of the unit. The map is blurred.")

- **Its own processor decodes, about three times as fast as a 192's.** A picture
  takes some 17 ms to decode, 15 ms to draw and 3 ms to read, so there is room to
  spare at 30 a second.
- **The SD slot is read by the chip itself**, not through a card processor, in long
  multi-block reads: 2.9 MB/s from the bridge on the unit, every word right. The
  phone's video needs about a tenth of that.
- **It is shown whole**, 800 × 480, not halved.
- The same bridge serves it, on a Raspberry Pi Pico running at 250 MHz, in the
  unit's second slot; the viewer starts from a card in the first.
- The unit's knob and keys drive the phone, as on the 192, and the eject key works.
- **Sound came later.** The 193 plays sound through its own serial sound interface,
  not a card processor; with a driver for it in the kit, the phone's music and
  navigation voice were heard on the unit on 23 November.

Two things had to be found on the unit first. The 193's display unit showed the
kit's 256-colour picture as a flat light grey, though the stock software's own
16-bit planes showed exactly what was written into them; the kit now draws the 193's
screen in 16-bit colour. And the 193 reads the bridge so fast that the viewer took
in a whole backlog of pictures each time and, decoding them one at a time, never
reached the newest to show it; now it decodes them all and shows the last.

Still open: drawing faster, which now costs nearly as much as decoding; the
decoder's hottest loops in hand-written code; and a board that fits in the car in place of a Pico on
a ribbon, which the next section begins.

## The whole bridge on an ESP32-S3

![Android Auto on the 193 through an ESP32-S3](/media/android-auto-esp32.jpg "The bench 193 on 23 November, with no RP2040 and no cable to the phone: the phone's music and Google Maps through an ESP32-S3 that is both the phone's Wi-Fi access point and the SD card in slot 2. The map is blurred.")

On 23 November one **ESP32-S3** took the place of the Pico, and of the phone's cable
too. The phone reaches it over Wi-Fi; it runs the Android Auto session, decrypts the
video, and is itself the SD card the unit reads, through wires soldered inside the
unit to the second slot's contacts. Android Auto ran on the bench 193 at **25 to 30
pictures a second, with its sound**.

![An ESP32-S3 development board](/media/esp32-s3-board.jpg "The board: an ESP32-S3-N16R8 development board, 16 MB of flash and 8 MB of PSRAM, with two USB-C sockets (a USB serial port for the log and the chip's own USB).")

**The phone over Wi-Fi, with no Bluetooth.** Wireless Android Auto is normally
started over classic Bluetooth, which the ESP32-S3 does not have (it has Bluetooth
LE only). Android Auto's developer settings have another way in: a *head unit
server* on the phone, on TCP port 5277, that a head unit connects to over the
network. A Mac tried it first: a full session with a Pixel, every channel opened, no
Bluetooth. On the ESP32-S3 the bridge is its own Wi-Fi access point (2.4 GHz, WPA2);
the phone joins it, and the bridge connects to the phone's server, every two seconds
until it answers. A phone that leaves the network and comes back gets its session
again by itself. The server has to be started on the phone (Android Auto's developer
settings, *Start head unit server*).

**The SD card without programmable I/O.** The RP2040 answers the card's commands
with its PIO state machines, on the host's clock. The ESP32-S3 has nothing like them
and no SD card (slave) peripheral, so three of its peripherals share the work:

- the **camera interface** (LCD_CAM) takes CMD and DAT0 at every rising edge of the
  slot's clock and the DMA controller writes a byte for each into a ring in memory;
- a loop on the chip's **second core**, which does nothing else and takes no
  interrupt, finds the host's commands in that ring and answers them with the same
  card code as the RP2040's;
- **two SPI controllers as slaves** on the slot's clock shift the answers out: one
  the responses on CMD, the other, fed by DMA, the blocks of data on DAT0. The loop
  turns each on and off at the right clock by tying its select line from inside the
  chip.

Wi-Fi, the encryption and the session run on the first core. A response begins about
80 of the host's clocks after its command, a little more than the SD specification's
64, which the 193 accepts (as it accepted the Pico's 70).

![The ribbon soldered to slot 2's contacts](/media/esp32-slot-wires.jpg "Inside the 193: the wires soldered to the back of the second card slot's contacts, CLK, CMD, DAT0 and ground, leaving the slot itself empty.")

**How fast.** A test program on the 193 read the bridge's 8 MB test file and checked
every word, at the stock clock of the 193's SD host (24.9 MHz, measured by the
bridge) and at half of it:

| Read | Clock | KB/s | Wrong sectors |
|---|---|---|---|
| one sector a command, 256 sectors | 24.9 MHz | 1687 | 0 |
| 8 sectors a command, 1024 sectors | 24.9 MHz | 2617 | 0 |
| 64 sectors a command, 2048 sectors | 24.9 MHz | 2809 | 0 |
| 256 sectors a command, 4096 sectors | 24.9 MHz | 2814 | 0 |
| one sector a command, 256 sectors | half | 1075 | 0 |
| 256 sectors a command, 4096 sectors | half | 1331 | 0 |

The same with the Wi-Fi access point running. The Pico gave 2.9 MB/s; the phone's
video needs 200 to 400 KB/s.

Three faults behind stalls and stuttering sound were found on the unit and fixed:

- **The card damaged its own commands.** The system's timer interrupted the card's
  loop every millisecond; when that fell on a response, the loop let the CMD line go
  late, the SPI controller drove it low after its last bit, and the loop took that for
  a damaged command and reported it in its next answer. About one command in 1500;
  the unit then gave up a read. The card's core now takes no interrupt at all.
- **Sound dropped whenever a big picture came.** The stream to the unit kept the
  Pico's 63 KB of blocks, and the first picture of a new screen (50 KB and more) nearly
  filled it: at the start and whenever Maps opened, the bridge dropped sound. The
  stream is now half a megabyte, in the board's PSRAM.
- **The link was slower than the music.** The network stack's receive window was
  its default of 5.6 KB, so whenever a round trip over Wi-Fi was slow the phone's bytes
  came at 150 to 230 KB/s, and music alone is about 190 KB/s: the unit's sound ran
  dry every few seconds. With a 64 KB window it did not run dry once in the two
  minutes measured.

![The 193's main board wired to the ESP32-S3](/media/esp32-main-board.jpg "The 193's main board out of the unit, with the ribbon from the second slot's contacts to the ESP32-S3: three signal lines and ground.")

**Wiring.** Three signal lines and ground, from the second slot's contacts to the
board, each signal through a 100 Ω resistor at the board's end:

| ESP32-S3 | Signal | SD contact |
|---|---|---|
| GPIO 12 | CLK, through 100 Ω | 5 |
| GPIO 11 | CMD, through 100 Ω | 2 |
| GPIO 13 | DAT0, through 100 Ω | 7 |
| GND | ground | 3 and 6 (VSS) |
| none | VDD | 4: not connected |
| none | DAT1, DAT2, DAT3 | 8, 9, 1: not connected |

GPIO 4 is used inside the chip and stays unconnected. The board is powered from its
own USB socket. The slot's 3.3 V is there even with no card in, but how much current
it can give has not been measured, and the ESP32-S3 draws up to some 350 mA in bursts
while it transmits, so it does not power the board. The 193 started the card with the
slot itself empty.

The firmware is built with Espressif's ESP-IDF (v5.5) and flashed over the board's
USB serial port; its source is the `esp32/` directory of
[rnse-sdbridge](https://github.com/jroark/rnse-sdbridge), and the bridge needs a
head unit certificate of its own, which is not supplied.

Not done yet:

- **A 192.** Its card processors clock the card at 16.9 MHz, slower than the 193's
  host, but the 192 knows a card by the slot's switch, which a card end or the
  wires inside would have to close.
- **Starting without touching the phone.** The head unit server is a developer
  setting started by hand; in a car, Android Auto would be started over Bluetooth or
  by a helper app on the phone.
- **The picture.** The phone sends its video at 200 to 400 KB/s over this link, and
  the picture looks compressed; why the phone chooses so low a rate is not known yet.
- **Power and a home for the board** in the car.
