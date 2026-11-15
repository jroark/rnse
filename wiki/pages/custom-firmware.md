title: Custom firmware
order: 70
summary: Writing software of one's own for the RNS-E, running it from a disc, an SD card or flash, and always getting the unit back.
---
The emulator exists to make one thing possible: **writing new software for the RNS-E**
and knowing it will behave on a real unit before it gets there. The project includes
a small kit for that, the SDK. Programs written with it have run on a real North
American 192 from a CD, from an SD card and from flash. Nothing has been tried on
a real 193 yet.

## Where a program runs

In the update loader's place. When the installed software finds a disc with a
higher software index, it checks the disc and then copies the loader into RAM and
calls it ([how](/firmware/#how-an-update-starts)). A program of one's own in that
place:

- is started by **the unit's own software**, after the same checks a stock loader
  passes, and from then on has the machine to itself;
- runs from RAM and **writes nothing to flash** unless it chooses to, so the unit's
  own software is still there afterwards.

`rnse custom` takes an image of an update disc you own and writes a copy in which one
variant's loader is your program. The result is for your own unit only: it contains
the rest of your disc, which is not yours to give away.

## The way back

There is a trap here. The installed software starts the program at **every start**
for as long as the disc is in the drive, and does so before it would react to the
eject key. The disc sits behind the motorised display, which a program does not
move unless it is written to. So a program that simply runs takes the unit over until
the disc is out, and the disc does not come out.

What ends that is the stock loader. An update leaves the unit with the disc's
software index, and then the disc is no longer an update. So `rnse custom` packs the
stock loader into the image, and a program can hand over to it. The unit updates
itself from the same disc and is its ordinary self again. On the bench there turned
out to be a second door: after a restart the stock software obeys the eject key for
about 40 s before it starts the program again.

## The rules

After the [eject incident](/reverse-engineering/#when-it-went-wrong), every program
meant for a real unit follows three rules:

1. **Physical ejection always works.** Every program keeps polling the faceplate
   and answers the display/eject key on normal screens, error screens and during
   long operations. No program takes over that key without another way to reach
   the disc.
2. **Hold RETURN for two seconds to get back to the loader.** Programs started by
   the SD loader return to it from any screen, stopping playback first. A short
   press keeps its normal meaning.
3. **No firmware-flash option unless it is explicitly wanted.** Test programs and
   loaders offer no updater, install, restore or erase option, and no hidden key
   sequence that leads to one.

## What a program gets

A C library, built with an ordinary SH-4 GCC (on macOS, one in a container):

| Area | Functions |
|---|---|
| start-up | `board_init()`: brings the board up as the stock loader does, register for register, and starts a 10 ms tick |
| time | `millis`, `delay_ms` |
| screen | 8 bits per pixel through a palette: `screen_fill`, `screen_pixel`, `screen_text` |
| faceplate | `panel_init`, `panel_poll`, `panel_key`, `panel_knob` |
| CD drive | `cd_init`, `cd_wait_ready`, `cd_read`, `cd_find` |
| CAN | `can_init`, `can_send`, `can_receive` |
| SD and sound (192) | the card processors' MP3 decoder, the radio processor's amplifier and source switch |
| the way back | `stock_update`, `loader_return` |

Nobody has a schematic of either unit, so the hardware layers do exactly what the
stock loader does: every register they write is one the stock loader writes, with
the same value, in the same order. Writes whose purpose is unknown are kept anyway
and marked as such, because a real unit may need them even if the emulator doesn't.
(The one exception, the faceplate, follows the stock operating system instead,
since no loader uses it.)

Programs run in the emulator first, in [`--strict` mode](/the-emulator/#how-it-is-tested),
and can be debugged at source level with GDB.

## Programs from an SD card

![An early version of the SD loader](/media/sd-loader-early.jpg "An early version of the SD card loader, waiting for a card in either slot. Its menu still offered a stock update on SETUP; current loaders no longer do.")

Burning a CD for every test is slow and wastes discs. On the 192 there is now a
better way: one **loader disc** that starts the file `RNSE.BIN` from a FAT card in
either slot. The current loader is persistent. The disc stays in, a test starts when
its card goes in, and when the test ends the loader shows its status and waits for
the next card. Trying a new program means copying a file to a card.

![The SDK's MP3 player on the unit](/media/mp3-player.jpg "The SDK's MP3 player, playing test tones from an SD card on the bench unit through its own drivers for the card processor and the radio's amplifier.")

The first programs run this way measured the unit
([Reverse engineering](/reverse-engineering/)). Then came sound: an **MP3 player**
built from the SDK's own drivers for the NEC card processors' decoder and the radio
processor's amplifier. It was heard on the unit, and its volume knob works.

## A program in flash

`rnse firmware` makes an update disc whose operating-system module is replaced by
a program. The unit's own software installs it like any update, and from then on the
**boot ROM starts the program at every power-on**, with no disc and no card. That
only works safely because there are three ways back out, each ending with the boot
ROM reinstalling from the disc in the drive:

- the program can remove itself and restart;
- a **guard** in the start-up code counts starts in battery-backed memory. A
  program that hangs or crashes five times in a row is taken out automatically;
- switching the unit off and on four times, each before the program reports
  itself healthy, does the same by hand.

![The preflight program](/media/preflight.jpg "Preflight: before a program goes into flash, a card-launched check that the guard's counter survives a restart and that both flash chips answer as expected.")

One subtlety: a disc carries its loader twice, and if both were the stock loader, a
unit whose program had just removed itself would immediately reinstall it from the
same disc. So firmware discs carry a small "swap" program in the boot ROM's copy
of the loader. It ejects the disc and asks for the stock one. The whole circle (install,
run, remove, reinstall stock) has been done on the bench unit.

## Built on it

- **base**: a firmware for flash with a radio and an MP3 player, and the way out.
- **A modern interface for the 192**: a replacement operating system with a
  charcoal-and-red interface driven by the rotary knob. It has FM presets, SD
  MP3 playback, local contacts, saved destinations, live vehicle status from CAN,
  an experimental client for the factory telephone module, and small native
  applications loaded from SD cards.
- **h264bench**: a measurement of whether the 192 could decode video at all
  ([Investigations](/investigations/)).

## What this does not establish

The emulator's tests show that the examples build and behave as written on both
emulated units. They also show that the stock software of both units, in both
markets, accepts such a disc and starts the program. On real hardware the
evidence is **one North American 192**. Other units, European ones and every 193,
may differ. The 192's first custom program went dark on a unit even though it ran
perfectly in the emulator; that is reason enough to stay modest.
