# The boot ROM

The first 48 KiB of flash (0x0000-0xbfff on V220 hardware) hold a boot ROM
that an update never rewrites, so it is not on the disc.  The emulator
replaces it with a stand-in (`src/bootrom.rs`) built from what the other
software expects of it.  This file records that evidence.

## What other software reads from it

| what | evidence |
|------|----------|
| Offset 0: address of the reset entry | The loader ends an update with `jmp @[0]` (after printing `*** Jump User RESET ***`). |
| Offset 0x400: a module header; +6 is the project code, e.g. `V220` | The loader copies four bytes from 0x80000406 as the "current project" and only accepts a software set of that name from the disc.  Without it it tries its built-in list (`V220`, `V264`) and reports "version up not required". |

## Starting the loader

The boot ROM copies `LDM.BIN` to its load address (0x8df00000) and calls
entry point 1 with three byte arguments in r4-r6.  r4 is an index 0..8
selecting the language of the loader's screens (the loader prints it as
`SET Lang_data`); r5 and r6 are stored but their use is unknown.  The
loader sets up its own stack, vectors and SR.

## Starting the OS

The boot ROM calls the OS module's entry points 1, 2 and 3 in turn, as
ordinary functions in privileged mode.  Entry 3 loads the first task's
context and never returns.  Evidence: entries 1 and 2 are functions that
return with `rts`; entry 2 installs the OS's vector base; entry 3 ends in
`rte`.

The OS itself walks the module directory and calls the same three entry
points of every other module at the matching stage.

State the OS inherits:

* A stack above 0x8fc80000.  After a cold start the OS clears RAM from
  0x8c000000 up to that address while still running on the boot stack.
* TMU0 interrupt priority.  The OS programs and starts TMU0 for its 2 ms
  tick but only ever sets the *TMU2* field of IPRA.  The loader, which
  shares its hardware layer with the boot ROM, uses IPRA = 0x7000.
* A running CAN controller.  The OS sets up its mailboxes but never
  takes the controller out of its reset/halt state itself.

## Vector tables

When the OS takes over the vector base register it copies, from the
previous one:

    +0x000  60 handler addresses, indexed by exception/interrupt code / 0x20
    +0x400  60 bytes: interrupt mask level to run each handler at
    +0x43c  49 handler addresses for TRAPA 15..63

and then overrides the entries it implements itself.  TRAPA 0x3d, 0x3e and
0x3f are never overridden: they are boot ROM services.

## Services

Called with `TRAPA #n`, function code in r0, arguments in r4-r7, result in
r0.  Function codes seen in the modules:

| trap | codes |
|------|-------|
| 0x3d | 0102 0103, 0201-0205, 0401 0404 0405, 0507 0509 050a 050b 050d, 0601-0603, 0901-0903, 0c01-0c04 |
| 0x3e | 01-0a, 12, 13 |
| 0x3f | 03 04 06 |

Understood so far (all trap 0x3e):

| fn | arguments | meaning | evidence |
|----|-----------|---------|----------|
| 1 | n | pointer to a version string | The application compares the first 3 characters of `fn1(1)` with "V25" and "V35". |
| 4 | pool | get the pool's memory break | Modules do `base = fn4(0); ...; fn5(0, base + size)` to claim their static RAM, |
| 5 | pool, address | set the pool's memory break | and the OS starts by setting it to the end of its own data (0x8c19c84c). |
| 6 | three out-pointers | start-up status | The OS clears all RAM when this returns 9. |
| 0x13 | - | pointer to the software index block (SOFIDX, NAVVER, AUDVER) | The OS compares the radio processor's reported version with the AUDVER text found through it; the block sits in the module directory at flash 0xC01C. |

Seen but not understood:

| trap/fn | arguments observed | notes |
|---------|--------------------|-------|
| 3e/3 | 0 | Called after the module directory fails verification, right after 3d/0901(0xc000, 0x100).  Probably "restart". |
| 3e/7 | 0, 1, 10, -2 | Called periodically from the tick. |
| 3e/9 | three out-pointers, 2 | Once during OS init 2. |
| 3d/0901 | address, size | See 3e/3: probably erases flash. |

Unknown services return 0.  `rnse boot --trace-bios` logs every call with
its caller.

## The 193's boot ROM

The stand-in serves both models; what differs for the 193 (variants V574,
V575 and VB48):

* Reset is entered as code at address 0: the loader ends an update with
  a jump there (after giving the watchdog time to do it first).
* Vector tables: 64 handlers at the vector base, used both for exception
  codes 0x000-0x1e0 (the first 16) and for TRAPA 0-63, and 125 interrupt
  mask bytes at +0x780.
* It starts the installed software itself, module by module; see
  [hardware-193.md](hardware-193.md).
* Service 0x13 returns the address of the version block behind the
  entries of the directory at 0x400000 (0x8040007c with six modules).
* Service 0x0b is new: it hands the state of the boot ROM's CAN software
  to the OS.  The argument points at two list addresses; each list ends
  in 0xffff and names blocks of the OS's memory to fill: ten small ones
  in the first (number, buffer, size; purpose unknown, left alone by the
  stand-in) and nine in the second (number, size, buffer).  Of the
  second, block 2 is the table of received messages (36 entries of 40
  bytes: enabled, identifier, then three times handler, buffer and
  length) and block 8 holds the identifiers 0x271 and 0x2c3 and a
  "received" flag for each: the ignition lock's message, which is what
  wakes the unit.  The OS replaces the handlers of those two entries
  with its own and reads the first data byte through the entry's buffer
  pointer to decide whether to start.  The stand-in fills in the two
  entries and the flags from the ignition frame the emulated car is
  sending; without that the OS logs `CAN_Pass_mzas() Error` and treats
  the ignition as off.
* Services 1, 4, 5 and 6 are used as on the 192.

