title: The unit
order: 10
summary: What the RNS-E is, the cars it fitted, its controls, and how to tell the two generations and their variants apart.
---
The RNS-E ("Audi Navigation plus" on its bezel) is a double-DIN head unit. It
combines a radio, a CD/DVD player, an MP3 player for SD cards and DVD-based
navigation. It was made by **Aisin AW**, the Toyota-group company behind many
Japanese navigation systems, and sold by Audi as factory equipment in the A3 (8P),
A4 (B6/B7, including the cabriolet) and TT (8J) among others. Second-hand units are
common, and they turn up on benches like this one.

## The front

The faceplate has a colour screen. Below it is a row of function keys
(RADIO, CD/TV, NAME, TEL, NAV, INFO, CAR, SETUP). Four soft keys sit at the corners
of the screen, and on the right are two knobs: a large rotary controller that is
also a push button, and a small volume knob. A
RETURN key sits beside the large knob.

The screen is motorised. One key folds it down, uncovering the media slots behind it.

![The display folded down, showing the two SD slots, the DVD slot and the eject key](/media/display-folded.jpg "Folded down: SD slots 1 and 2, the DVD slot and the red eject key. The screen keeps working in this position; here it shows a picture from a custom program.")

That arrangement matters more than it seems. A disc can only be taken out with the
screen folded down, and the screen is folded down by the unit's software. If the
software stops listening to the keys, the disc stays in. That is the reason for
[the rules custom software here follows](/custom-firmware/#the-way-back).

## Two generations

The name covers two different computers. They share the faceplate and the update
discs, and nothing else that matters to software.

| | 8P0 035 **192** | 8P0 035 **193** |
|---|---|---|
| Built | mid 2000s (the photographed board carries 2006–2007 date codes) | from 2010 |
| Processor | Hitachi/Renesas SH-4 (SH7750R kind) at about 235 MHz, in a package marked with Aisin AW's own part number | a Renesas SH7770-family "SH-Navi" system-on-chip |
| Graphics | separate Fujitsu MB86293 "Coral" chip, 400 × 240 panel | on-chip 2D engine and display unit with blended planes, 800 × 480 |
| SD cards and MP3 | two NEC µPD77115A signal processors on a card board | on-chip SD host interface; the main processor decodes MP3 |
| CAN | an HCAN2 controller | on chip |
| Memory | 64 MiB RAM, 32 MiB NOR flash, 256 KiB battery-backed SRAM | 128 MiB RAM, 64 MiB NOR flash |
| Firmware variants | V220 (Europe), V222 (North America) | V574 (Europe), V575 (North America), VB48 |

The part number is on the label on the unit's case. On a working unit the
CAR → Version screen shows the installed software index (for example `SW: 0260`).
The [firmware](/firmware/) page explains what that number means for updates.

## Inside

A 192 opens into a stack of boards: a main board with the processor, memory and
graphics chip, a small card board with the two SD slots, a radio board made by
Grundig with two tuner modules, and an Alpine DVD mechanism with its own controller.

![An opened 192 with its radio board exposed](/media/192-opened.jpg "An opened European 192. The radio board, made by Grundig, with its two shielded tuner modules on the right; the main board sits underneath.")

![The DVD mechanism, marked DV35M110](/media/192-drive.jpg "The drive: an Alpine DVD mechanism (DV35M110 on its label) that pulls the disc in by motor.")

The [192's hardware](/hardware-192/) page goes through every board, chip by chip.
For the 193 there are no photographs yet; what is known about it comes from its
firmware ([the 193's hardware](/hardware-193/)).

## What else is in the unit

The main processor is not alone. Several other processors carry their own programs,
and the main software talks to them:

- a **radio processor** (an NEC V850 on the radio board) that tunes, switches sources
  and sets the volume through a Philips car-audio signal processor;
- the **faceplate controller** (a Fujitsu 16-bit microcontroller with a mask ROM)
  that scans the keys and knobs and drives the display lighting;
- the **DVD drive's** own controller, reached over an ATAPI bus;
- on the 192, the two **card processors**, which receive their program from the main
  software at start-up;
- a **GPS receiver** module, and the car itself on the **CAN** bus (ignition and
  speed among much else).

The emulator has to answer all of them, more or less convincingly.
[The emulator](/the-emulator/) page says how much.
