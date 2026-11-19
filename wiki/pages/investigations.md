title: Investigations
order: 80
summary: Open questions under way: the factory telephone module.
---
Not everything is finished. These are the investigations in progress, with what has
been measured kept apart from what is assumed. Android Auto, which began here, has
a page of its own: [Android Auto](/android-auto/).

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
