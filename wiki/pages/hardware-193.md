title: The 193's hardware
order: 30
summary: The 2010 RNS-E, a system-on-chip with no public manual, worked out from its firmware, photographed, and since November 2026 checked on a bench unit.
---
The 193 (part number 8P0 035 193, 8J0 035 193 in the TT) looks like a 192 from the
front and shares its update discs, but inside it is a different computer. Almost
everything sits in one system-on-chip, a Renesas **R8A77730**, of the **"SH-Navi"**
family made for car navigation (this page long called it an SH7770, an older
member). Its core is an **SH-4A**, the SH-4's successor, and like the rest of that
family the chip has **no public manual**.

> The register-level work on this page was done on the 193's own firmware running in
> the emulator, with two outside aids: the Linux kernel's SH7770 support (for serial
> port addresses) and the Linux driver for the display unit of Renesas' later R-Car
> chips, which inherited the block almost unchanged. Since 20 November 2026 a real 193
> is on a bench: its boards have been photographed (below), programs of our own have
> run on it, and what they measured is [below](#on-the-bench).

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

## On the bench

The bench unit is a North American 193 from an Audi TT, built in 2013, running V575
software at index 0230. Programs of our own reach it the way they reach a 192: the
unit's own software starts them from a CD-R in place of the update loader. A CD-RW is
not read by its drive. From there a small loader on the disc starts programs from an
SD card, so later tests needed no new disc. What they found:

- **The processor** identifies itself as an SH-4A: processor version 0x10300700 (the
  same as Renesas' SH7785), product register 0x00000412, cache version 0x71440211.
- **A copied constant nearly killed every test.** The kit's start-up wrote a value
  it had taken from the emulator's log to a register at 0xfe800220, apparently the
  memory controller's. The stock loader computes that value from another register,
  which the emulator read as 0. On the unit, RAM failed about 100 ms after the write.
  Now the kit computes the value the way the loader does.
- **The screen.** Planes of 16 bits a pixel show exactly what is written into them.
  A 256-colour plane through a palette, set up as the stock loader sets up its own,
  showed only a flat light grey. Why is not known, so the kit now draws the 193's
  screen in 16-bit colour. A palette entry keeps six bits of each colour.
- **The panel stays dark until POWER is pressed**, even while our own program runs
  and has set the display up. Whatever switches it on (the radio processor or the
  faceplate) is outside the chip.
- **The faceplate** sends the same key codes as the 192's. CD EJECT ejects the
  disc; the display key arrives too, but the display's motor has not been driven yet.
- **The SD host** reads both slots, in long multi-block reads at 2.9 MB/s. Changing
  slots takes 10 to 20 ms before commands reach the new one. The selects are the
  other way round from the slots' numbers: select 0 is the unit's slot 2.
- **CAN** works through the controller as the stock software leaves it set up. The
  kit's own CAN start left it silent, until the monitor below showed why.

With that, a phone's [Android Auto](/android-auto/#on-the-193) ran full screen on
the unit at 30 pictures a second.

### Driven from scripts

A second 193 (V575, index 0310) now sits on the bench with a small **monitor**
running on it. The loader on the disc takes the monitor over the CAN bus in six
seconds; after that the monitor does what frames on the bus ask of it: read a
register, write one, send a stretch of memory, pass on what the faceplate and the
radio processor say. A script on the Mac drives it, and the same script runs against
the emulator, so the two answers can be laid side by side. The monitor never writes
the flash or the battery-backed memory, and it leaves the watchdog to its own main
loop: a request that hangs the unit ends in a reset, the unit's own software starts,
the loader takes over and the monitor is pushed again, about 95 seconds in all,
with nobody at the bench. The first evening of it found:

- **The kit's clock ran twice too fast on the real chip.** A timer goes on asking
  for its interrupt for a moment after its flag is cleared. The kit's handler
  returned before the request had gone, so it ran twice for every tick: forty
  seconds counted in twenty, and every delay and time-out in a program half as long
  as written. Reading the timer's control register back after clearing the flag is
  enough. The emulator now keeps the request the same way, so a handler like the
  old one counts double there as well.
- **The clock itself**: the timers count 12.462 MHz, a quarter of a peripheral
  clock of 49.85 MHz and not of 50. The stock software's millisecond is 0.2 % long.
- **The silent CAN bus.** The kit's CAN start cleared the top bit of the
  controller's control register, which the stock software leaves set. With that bit
  clear, the controller keeps a mailbox's identifier two bits further up: the unit
  was not silent at all, it sent its frames under other identifiers (0x1FE for
  0x7F8) and took ours for something else. Started with the bit kept, the
  controller talks and listens from reset.
- **The CAN mailboxes** as the boot ROM sets them up: ranges of identifiers in
  seven of them, single identifiers in six, and one for everything else. The
  emulator had a single catch-all in their place.
- **What keeps the unit from restarting.** The radio processor starts the unit
  again when the main software stops talking to it: after 13½ minutes of nothing.
  The stock software sends it two frames over and over, `8a` every 30 seconds and
  `81 [06]` every 60. The second alone does not help (the unit restarted after 15
  minutes); with both, the unit stayed up. The front stays lit either way.
- **The tuner and the amplifier** take the 192's frames: a frequency set and a
  volume set both show in the status block the radio processor sends back.
- **The drive** reads a disc from a program: five stretches of a CD-R came back as
  the disc's image has them, 3.7 ms a sector in a row, 170 to 290 ms for a read
  somewhere else.
- **Registers that read differently**: the cache control register has only an
  SH-4A's bits, the interrupt pins' masks are set and cleared by writing ones, and
  the ports' input registers have real levels on them. Some sixty of 328 registers
  compared still differ, most of them state the stock software sets later on the
  unit than in the emulator.

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

- Much of the address table above is still inferred. The processor, the timers, the
  display unit, the faceplate's serial port, the radio processor's, the drive, the SD
  host, CAN and the sound interface have been used on a bench unit; the GPS block,
  the 2D engine, the I2C masters and two more serial ports have not.
- What switches the panel on is not known: the frame the radio processor sends when
  POWER is pressed has not been caught yet.
- The front goes dark for good when a program leaves the faceplate's start
  unanswered. How long it waits has not been measured.
- Ten interrupt sources the stock software uses have no model in the emulator, and
  several register blocks written at start-up are not understood. The emulator lets
  those behave like memory, so that what is written can be read back.
- The boot ROM has been read from a bench unit, but the emulator still runs its
  stand-in for it.
- How long a timer's request really lingers is not known; the emulator's figure is
  only long enough to catch a handler like the kit's old one.

See also [The 192's hardware](/hardware-192/) and [The emulator](/the-emulator/).
