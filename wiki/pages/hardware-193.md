title: The 193's hardware
order: 30
summary: The 2010 RNS-E, a system-on-chip with no public manual, worked out from its firmware and now photographed.
---
The 193 (part number 8P0 035 193, 8J0 035 193 in the TT) looks like a 192 from the
front and shares its update discs, but inside it is a different computer. Almost
everything sits in one system-on-chip, a Renesas **R8A77730**: by Renesas' numbering
an SH7773 of the **"SH-Navi"** family made for car navigation (this page long called
it an SH7770, its older sibling), and like the rest of that family it has **no public
manual**.

> The register-level work on this page was done on the 193's own firmware running in
> the emulator, with two outside aids: the Linux kernel's SH7770 support (for serial
> port addresses) and the Linux driver for the display unit of Renesas' later R-Car
> chips, which inherited the block almost unchanged. Since November 2026 a real 193 is
> on a bench, and its boards have been photographed (below); nothing of ours has run on
> it yet.

![The 193's main software in the emulator](/shots/os-193.png "The 193's main software (V574) in the emulator: the radio set-up screen at 800 × 480.")

## On the boards

![The main board, processor side](/media/193-main.jpg "The main board of an Audi TT's 193 from 2013 (Aisin AW 99370-00492).")

![The SoC, its RAM and Aisin's own chip](/media/193-soc.jpg "Centre: the Renesas R8A77730 with its heat-spreader paste wiped off. Top and left: the two Elpida DDR2 chips, 128 MiB together. Right: the AKM AK5406. Bottom: IC100, a large chip of Aisin's own, not identified.")

![Flash and battery-backed memory](/media/193-flash.jpg "The other side: four Spansion PL127J flash chips, two of them with paper labels, and the Renesas R1LV0208BSA, 256 KiB of battery-backed RAM (right).")

The four flash chips are wired as **two banks of 32 MiB, each two 16-bit chips side by
side** on a 32-bit bus. The stock update loader gives it away: it sends every command
to both halves of a word at once, and it erases in 128 KiB steps except at the joint
between the banks, where it steps by 16 KiB over the chips' small sectors. The
emulator now models the flash that way, and the battery-backed memory as the 256 KiB
part it is.

![IC100](/media/193-ic100.jpg "IC100: marked 64445DA01BGV, 99325 00115, AISIN AW. What it does is not known.")

![The picture path](/media/193-video.jpg "The AKM AK5406 (centre), presumably for the RGB picture input, and the Maxim MAX9247 serializer (left) that sends the picture to the panel over LVDS.")

The radio board is made by Grundig, as in the 192. It has the same NEC V850 radio
processor, but an NXP SAA7706H signal processor where the 192 has the SAA7709H. The
faceplate's Fujitsu MB90561A controller and the Alpine DV35M110 drive are the same
parts as in the 192.

![The radio board](/media/193-radio.jpg "The radio board: the NXP SAA7706H (centre) and the NEC V850 radio processor (bottom right).")

![The faceplate controller](/media/193-faceplate.jpg "The faceplate's Fujitsu MB90561A, with Aisin AW's mark.")

## In one chip

| Block | Address | Notes |
|---|---|---|
| NOR flash, 64 MiB | `0x00000000` | two banks of two 16-bit chips; the 192's command set, 128 KiB sectors (16 KiB at the ends of a bank); boot ROM at the bottom |
| RAM, 128 MiB | `0x08000000` | |
| battery-backed SRAM, 256 KiB | `0x18000000` | the OS has some 1800 references to it |
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

- Apart from the parts on the boards, none of this has been checked on a real 193 yet.
  A program for the bench unit that reads its registers, flash and boot ROM out over
  CAN, as was done on the 192, is ready.
- Several register blocks are written at start-up and never understood. The emulator
  lets them behave like memory, so that what is written can be read back.
- The 193's boot ROM is entirely inferred; nobody has read one yet.

See also [The 192's hardware](/hardware-192/) and [The emulator](/the-emulator/).
