title: RNS-E
order: 0
summary: The Audi RNS-E navigation unit, an emulator that runs its real firmware, and how both were worked out.
---
The **RNS-E** is the navigation and radio unit that Aisin AW built for Audi in the
2000s. It sits in the dashboard of an A3, A4 or TT and has a fold-down screen, a DVD
drive for maps and two SD card slots. Inside is a Hitachi SH-4 computer running a
real-time operating system and an application written in Japan.

This site collects what has been learned about it while writing **rnse**: an
emulator, written from scratch, that runs the unit's own firmware from an ordinary
update CD. It also describes the work that made it possible: photographs of opened
units, measurements from a real unit on a bench, and custom software that now runs
on that unit.

![A North American RNS-E on the bench, showing its version screen](/media/bench-version.jpg "A North American RNS-E (part number 8P0 035 192) running on a bench, outside a car. The Car → Version screen shows the installed software index.")

> **Try it.** The emulator runs in your browser: [rns-e.com/emulator](/emulator/).
> Bring an image of an RNS-E firmware update CD you own. The page reads it on your
> own computer; nothing is uploaded, and no firmware or map data is distributed
> here.

## What is here

{{pages}}

## The short version

- There are two different computers behind the same name. The original **192**
  has an SH-4 processor of the SH7750R kind and separate chips for graphics, CAN and the SD cards. The
  **193** (2010) is built around one Renesas "SH-Navi" system-on-chip with an
  800 × 480 screen. See [The unit](/the-unit/).
- The firmware arrives on **update CDs**: a container of modules for every hardware
  variant, plus a loader for each. The unit's first 48 KiB, a boot ROM, are on no disc
  at all. See [Firmware and update discs](/firmware/).
- The **emulator** boots both generations to their user interfaces. It plays audio
  CDs and MP3s from SD cards, and on the 193 it reads a navigation DVD and draws the
  map. See [The emulator](/the-emulator/).
- It was **reverse-engineered** from the firmware's own behaviour and then
  checked against a real unit on a bench: a logic analyser on its CAN bus, and
  programs that read the unit's registers and boot ROM and send them back. See
  [Reverse engineering](/reverse-engineering/).
- **Software of one's own** now runs on a real 192. It is started from a CD or an SD
  card, or installed in flash, always with a way back to the stock software. Doom,
  Quake, Windows 3.11, a Mac Plus and an NES emulator have run on the bench unit. See
  [Custom firmware](/custom-firmware/#games-and-other-computers).
- A small board in an SD slot could make a 192 an **Android Auto** head unit. On a
  Raspberry Pi Pico, a phone already projects Android Auto over USB. See
  [Investigations](/investigations/).
