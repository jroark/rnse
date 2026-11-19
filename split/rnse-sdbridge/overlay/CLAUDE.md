# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

An RP2040 that answers an Audi RNS-E 192's SD slot as a card on one side and is a phone's USB host on the other, and the 192's programs at the other end of that link: Android Auto on the unit's own display, decoded by its own processor, with the phone's sound, driven by its knob and keys. Built on rnse-sdk (the submodule `sdk/`) for the 192's side and the Raspberry Pi pico-sdk for the RP2040's, tested on the host and on the emulator `rnse`. Split out of rnse, with its history.

<!-- FIRMWARE RULE -->
Never commit the pico-sdk (cloned pinned by `build.sh`), `out/`, `work/` (logs from the bench), UF2 files, and **never** a head unit certificate or key: Android Auto's handshake builds only with `AA_CERT`/`AA_KEY` naming files outside every repository.

<!-- MANDATORY HARDWARE RULES -->
## Commands

```sh
git submodule update --init                     # the kit, sdk/
sh tests/run.sh                                 # the card logic on the host, the PIO programs in a simulator (tests/piosim.py)
PATH=$PWD/sdk/tools/sh4-docker:$PATH ./sdk/build.sh 192 $PWD/aaviewer   # the 192's programs: linkbench, h264bench, aaview, aaviewer
cargo test --release --test linkbench --test aaviewer    # the 192's programs against the bridge's card, no firmware needed (needs the SH-4 GCC)
RNSE_DISC=disc.iso cargo test --release -- --nocapture    # and from the SD loader, as a unit runs them
PATH=~/arm-gnu-toolchain/bin:$PATH sh build.sh  # the RP2040 firmwares (out/); AA_CERT=... AA_KEY=... for those with TLS; SYS_KHZ for the clock
```

Without an SH-4 GCC on the PATH install `gcc-sh4-linux-gnu binutils-sh4-linux-gnu` (Debian/Ubuntu) or put the kit's `sdk/tools/sh4-docker` on the PATH (a container; needs Docker). The end-to-end tests return early, and report success, without `RNSE_DISC` (an update disc image: firmware, never committed) or without an SH-4 GCC; a green run without them proves only that things build. A local disc image may be sitting in the repository root as an ignored `*.iso`; use it if present. They run whole boots on the emulator and take minutes.

## Layout

- `src/` the RP2040 firmwares: `SDBRIDGE.UF2` a read-only card made up as it is read (FAT16 with `LINKTEST.BIN`), `SDPHONE.UF2` the phone side on a desk (USB host, Android Open Accessory, `aap.c`), `SDHANDSHAKE.UF2`/`SDSTREAM.UF2` with TLS (`aa_tls.c`, mbedTLS) and a head unit (`aa_hu.c`), `SDLINK.UF2` the bridge whole (`sdlink.c`, `aastream.c`): the phone's video and sound as `AALINK.BIN`. `patches/` (TinyUSB's bulk endpoints on EPX), `link.py` (the bench's serial log, the fast loop), `pem2c.py`, `uf2.py`.
- The 192's programs: `linkbench` (times a slot with a card processor program of its own, words checked against `LINKTEST.BIN` from `tools/linktest.py`), `h264bench` (H.264 decoding speed; streams from `tools/aa-streams.sh`), `aaview` (`AAVIEW.BIN`, loads `AAVIEWER.IMG`) and `aaviewer` (reads `AALINK.BIN`, decodes with `third_party/h264bsd`, shows it, plays the sound, sends the knob and keys back; format in `aaviewer/aalink.h`).
- `docs/sdbridge.md` (wiring, what is tested, what was seen on a unit), `docs/sdbridge-laptop.md` (tools, builds, flashing), `docs/android-auto.md` (the plan and the measurements).
- `tests/` host tests (`host.c`, `tls_host.c`, `piosim.py`, `run.sh`) and the emulator tests (`linkbench.rs`, `aaviewer.rs`, `disc.rs`).

## Things that are easy to miss

- The 192's card interface is not as its manual suggests: its response register is a ring whose read position carries across responses, the response interrupt fires only on a rising flag, and a read stopped by CMD12 leaves a stray word that spoils the next one. `docs/sdbridge.md` and rnse's `docs/hardware.md` say what was measured; the emulator reproduces it.
- The board in the slot never takes power from it (VDD open; only the ground is shared): `docs/sdbridge.md`, "Building one".

## Where things are that are not here

- `sdk/` is the kit, rnse-sdk, as a git submodule pinned by tag (`git submodule update --init`); `RNSE_SDK=/path` uses another checkout. Change the kit in rnse-sdk, tag it, then move the submodule.
- A `docs/....md` or `src/....rs` named in a comment that is not in this repository is in `rnse` (https://github.com/jroark/rnse): the emulator and its reverse-engineering notes (`docs/hardware.md`, `docs/hardware-193.md`, `docs/boot-rom.md`, `docs/firmware-format.md`).
- `rnse-ports/...`, `rnse-linux/...`, `rnse-sdbridge/...`, `rnse-hardware/...` are paths in those repositories (https://github.com/jroark/NAME).
- The tests run the programs on the emulator: the crate `rnse` from git at the commit this repository was split from (`Cargo.toml`, `Cargo.lock`).
