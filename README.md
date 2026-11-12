# rnse

An emulator for the Audi RNS-E navigation unit (built by Aisin AW around a
Hitachi/Renesas SH-4), written from scratch in Rust.

It runs the unit's real firmware, taken from an ordinary firmware update
CD.  No firmware is included in this repository; you need your own disc
image.

![The emulator's browser front end](docs/ui.png)

## Status

Work in progress.  What runs today, on the `V220` hardware variant:

| | |
|---|---|
| Update loader (`LDM`) | Boots, shows its screens, reads the CD, erases and programs the flash and finishes the update, exactly as on the unit. |
| Main software (`OSM` + applications) | Boots, switches on and draws its user interface through the graphics chip's drawing engine.  It currently stops at a "system software is being checked" notice: it is waiting for the radio/audio processor, which is not emulated, to report its version. |
| Front end | `rnse ui` shows the unit's faceplate in a browser with the live screen, the firmware's debug log and controls for ignition and the disc.  The keys and knobs are click targets but are not connected to the emulated hardware yet. |
| Other variants on the disc (`V574`, `VB48`) | Parsed and extractable; not bootable yet (different memory map). |

Not done yet: front panel keys, the serial links to the radio and GPS
processors, sound, the 3D half of the graphics chip, the SD card reader.

See [docs/hardware.md](docs/hardware.md) for what is known about the
machine, [docs/firmware-format.md](docs/firmware-format.md) for the disc
and file formats, and [docs/boot-rom.md](docs/boot-rom.md) for how the
missing boot ROM is replaced.

## Building

    cargo build --release

There are no dependencies outside the Rust standard library.

## Using it

Make an ISO image of a firmware update CD (2048-byte sectors, e.g.
`dd if=/dev/sr0 of=disc.iso bs=2048`).  Then:

    # What is on the disc?
    rnse info disc.iso

    # Unpack the firmware modules of every hardware variant
    rnse extract disc.iso out/

    # Run the update loader against an empty unit and keep the result
    rnse loader disc.iso --cycles 6000000000 \
        --screenshot update.png --save-flash flash.img

    # Run the installed firmware with the faceplate in a browser
    rnse ui disc.iso                 # then open http://127.0.0.1:8080/

    # Or headless, for investigating
    rnse boot disc.iso --ignition --trace-bios --screenshot screen.png

The firmware's own debug log is printed on standard output.  The loader
writes it to a serial port inside the unit; the main software formats its
messages but sends them nowhere, so the emulator hooks its `printf` and
prints them as `[fw] ...` lines.  Much of the text is Japanese in EUC-JP;
pipe it through `iconv -f EUC-JP -t UTF-8 -c` to read it.

What the main software says while it starts (shortened):

    [fw] _/_/_/ [Fugue]Formatting system ... _/_/_/
    [fw] Call PWM_Init_Proc()
    [fw] ---------------------PWM_Tpm_Boot !!
    [fw] KLINE task started!
    [fw] PWM_LAST_ON [1]
    [fw] Now State[1] Change State[3]
    [fw] MRADIO4 ON1
    [fw] [FP] DISP ON
    [fw] LAST_ON = TRUE!!
    [fw] ROMID RETRY!!

Useful options while investigating (see `rnse` without arguments for the
full list):

    --io-log [N]       log the first N accesses to every device register
    --log-after N      ... but only start after N CPU cycles
    --onchip-log       include the SH-4's own peripheral registers
    --ignition         put the ignition status frame on the CAN bus
    --can ID:HEX[@MS]  send any other CAN frame periodically
    --trace-can        CAN frames in both directions
    --trace-coral      display lists sent to the graphics chip
    --surface ...      save an off-screen drawing surface as a PNG
    --trace-atapi      CD drive commands
    --trace-bios       calls into the boot ROM
    --trace-exc        every exception and interrupt
    --tp ADDR          print the registers whenever PC reaches ADDR
    --break ADDR       stop there

## Tests

    cargo test --release

runs the unit tests.  With a disc image available,

    RNSE_DISC=disc.iso cargo test --release

additionally runs the whole update in the emulator and checks that the
flash contents the loader produces are byte-for-byte the modules from the
disc, and that the installed OS reaches its idle loop.

The CPU core is also checked against QEMU with randomly generated
programs:

    cargo build --release
    tools/cpufuzz.py --count 1000            # integer instructions
    tools/cpufuzz.py --count 1000 --fpu      # plus floating point

(needs `qemu-user-static`).  `tools/cpudiff.py` finds the first diverging
instruction of a failing program.  A handful of instructions are excluded
there because QEMU 8.2 gets them wrong; those are covered by unit tests
written from the SH-4 manual instead (`src/sh4/tests.rs`).

## Layout

    src/sh4/        SH-4 CPU: interpreter, FPU, MMU, on-chip peripherals
    src/dev/        devices: Coral graphics chip, ATAPI drive, flash, CAN
    src/ui.rs       HTTP server for the browser front end
    assets/         the page and the faceplate picture it uses
    src/board.rs    memory map and interrupt routing of the main board
    src/bootrom.rs  stand-in for the boot ROM (not on the disc)
    src/machine.rs  start-up sequences, boot ROM services, run loop
    src/fw.rs       ISO 9660, LOADING.KWI container, module headers
    src/bin/rnse.rs command line front end
    tools/          CPU test tooling
    docs/           notes on the hardware and the firmware

## Legal

This is an independent hobby project for interoperability and
preservation.  It is not affiliated with Audi, Aisin AW or anyone else.
The firmware is copyrighted by its owners and is deliberately kept out of
the repository (`.gitignore` blocks the usual file types); use an image
of a disc you own.
