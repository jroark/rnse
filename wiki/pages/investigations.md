title: Investigations
order: 80
summary: Open questions under way: the factory telephone module, and whether a 192 could be an Android Auto head unit on its own.
---
Not everything is finished. These are the investigations in progress, with what has
been measured kept apart from what is assumed.

## Android Auto on a 192, with nothing but its own hardware

The question: could a 192 be an Android Auto head unit **with its own processor,
display and sound**, without a more powerful computer hidden behind it?

| Android Auto wants | The 192 has |
|---|---|
| a link to the phone: USB host or 5 GHz Wi-Fi | no USB, Wi-Fi or Bluetooth for data. Something must be added, and it only has to carry bytes, for example through an SD slot |
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

Still open: hand-written code for the decoder's hottest loops, sound, the cost of
TLS, and the link itself. The certificate a head unit needs can only come from a
commercial head unit, and like firmware it will never be published.

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
