title: The 193's hardware
order: 30
summary: The 2010 RNS-E, a system-on-chip with no public manual, worked out from its firmware alone.
---
The 193 (part number 8P0 035 193) looks like a 192 from the front and shares its
update discs, but inside it is a different computer. Almost everything sits in one
system-on-chip of Renesas' **SH7770 "SH-Navi"** family, a chip made for car
navigation that has **no public manual**.

> No photographs of an opened 193 exist yet, and none of the work on this page has
> been checked against a real 193. Everything here was worked out from the 193's own
> firmware running in the emulator, with two outside aids: the Linux kernel's SH7770
> support (for serial port addresses) and the Linux driver for the display unit of
> Renesas' later R-Car chips, which inherited the block almost unchanged.

![The 193's main software in the emulator](/shots/os-193.png "The 193's main software (V574) in the emulator: the radio set-up screen at 800 × 480.")

## In one chip

| Block | Address | Notes |
|---|---|---|
| NOR flash, 64 MiB | `0x00000000` | same command set as the 192's, 128 KiB sectors; boot ROM at the bottom |
| RAM, 128 MiB | `0x08000000` | |
| battery-backed SRAM | `0x18000000` | the OS has some 1800 references to it |
| DMA channels | `0xff801000` | channel 4 for SD cards, channel 3 for sound |
| SD host interface | `0xff90a000` | the register layout of Toshiba/Renesas SD hosts |
| serial ports | `0xff915000`, `0xff926000` | radio processor; faceplate controller |
| serial sound interface | `0xff984000` | MP3 samples go out here |
| CAN controller | `0xffdf0000` | RCAN-ET: the 192's HCAN2 registers with a different mailbox layout |
| 2D graphics engine | `0xffe80000` | "Q2" in the firmware's messages; Renesas later called it M2DG |
| display unit | `0xfff80000` | 800 × 480, blended planes, the R-Car "DU" layout |
| GPS block | `0xff938000` | passes messages to a GPS library on the main processor |

The peripheral clock is **50 MHz** and the OS's tick is 1 ms. Three independent
pieces of evidence agree on this. The playing time the unit shows for an MP3 rises by
one second for every 44,100 samples it sends out. The faceplate port's divisor gives
9600 bit/s from 50 MHz. And the CAN controller's bit timing gives the infotainment
bus's 100 kbit/s. The CPU is taken to run at 400 MHz, the family's rated speed; that
is an estimate.

## Graphics

The 192 draws through a separate Coral chip. The 193 instead has a 2D engine that
runs display lists from RAM, plus a display unit that blends several planes into the
picture on the panel. The commands are not the Coral's. The emulator implements
what the firmware uses: fills, lines, blits, blended text and the filled polygons of the map.
That was enough for every screen, including the navigation map.

![The 193's navigation map in the emulator](/shots/nav-193.png "The 193 drawing a map from a navigation disc in the emulator, with a GPS position given on the command line.")

## Cards, sound and navigation

Unlike the 192, the 193 has no separate signal processors for the SD cards. The SD
host interface is on the chip, and the main processor decodes MP3 in software,
feeding the samples to the serial sound interface by DMA. The emulator follows the
whole path, from a card image through to the samples that come out.

Navigation reads its map from a DVD: `ALLDATA.KWI` in Aisin's KIWI format, with
index files for destination entry. SD cards are also sold that carry the same
volume in a hidden partition of type `0x96`. Some map updates are made for **one
unit only**: the map's header begins with that unit's serial number, which the
unit's software checks. The emulator can be given a serial number for such a card.

Between GPS fixes the navigation reckons its position from speed pulses and a
gyroscope, sampled by the A/D converter. The emulator can set the car moving at a
given speed and rate of turn, and the map follows.

## What is still open

- None of this has been seen on a real 193. A 193 on a bench, with the same kind of
  report program that was run on the 192, would settle much of it.
- Several register blocks are written at start-up and never understood. The emulator
  lets them behave like memory, so that what is written can be read back.
- The 193's boot ROM is entirely inferred; nobody has read one yet.

See also [The 192's hardware](/hardware-192/) and [The emulator](/the-emulator/).
