# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Other people's software ported to the Audi RNS-E 192 (Aisin AW, SH-4) as programs started from an SD card through the kit's SD loader: Doom, Quake, umac (a Mac Plus), Tiny386 (a PC with Windows 3.11), an NES emulator (nofrendo) and a DVD player. Built on rnse-sdk (the submodule `sdk/`), tested on the emulator `rnse`. Split out of rnse, with its history. GPL-2.0-only (`LICENSE`): the ports build or vendor GPLv2 code.

<!-- FIRMWARE RULE -->
Never commit what the build scripts download or make: `*/work/`, `*/out/`, the games' data (WADs, PAKs), anyone's ROMs, Mac ROMs and disks, PC disk images.

<!-- MANDATORY HARDWARE RULES -->
## Commands

```sh
git submodule update --init                         # the kit, sdk/
PATH=$PWD/sdk/tools/sh4-docker:$PATH                # or Debian's gcc-sh4-linux-gnu
sh doom/build.sh                                    # doom/out/: DOOMGAME.BIN, DOOM1.WAD (downloaded, pinned)
sh quake/build.sh; sh umac/build.sh; sh win311/build.sh
sh nes/build.sh                                     # sdk/out/nes/: NESCORE.BIN, RNSE.BIN
./sdk/build.sh 192 $PWD/doom/doomboot               # a launcher (DOOM.BIN on the card); likewise quake/quakeboot, umac/macboot, win311/pcboot, dvd/dvd
sh dvd/check.sh                                     # the DVD decoders against ffmpeg, then the player (ffmpeg, dvdauthor, genisoimage)
RNSE_DISC=disc.iso RNSE_DOOM=doom/out cargo test --release --test disc doom_runs_from_an_sd_card -- --nocapture
```

Without an SH-4 GCC on the PATH install `gcc-sh4-linux-gnu binutils-sh4-linux-gnu` (Debian/Ubuntu) or put the kit's `sdk/tools/sh4-docker` on the PATH (a container; needs Docker). The end-to-end tests return early, and report success, without `RNSE_DISC` (an update disc image: firmware, never committed) or without an SH-4 GCC; a green run without them proves only that things build. A local disc image may be sitting in the repository root as an ignored `*.iso`; use it if present. They run whole boots on the emulator and take minutes. Each port's test needs its built game and data too: `RNSE_DOOM=doom/out`, `RNSE_QUAKE=quake/out`, `RNSE_UMAC=umac/out` with `RNSE_MAC_ROM` and `RNSE_MAC_DISK` (Apple's, the user's own), `RNSE_WIN311` (a folder with `win311/out`'s files and `WIN311.IMG`, made from the user's own floppies by `win311/install.py`), `RNSE_DVD` (`out/dvd/ntsc.iso` from `dvd/check.sh`), and for the NES `nes/build.sh` first (`RNSE_NES_ROM` for a ROM of one's own). To compare builds of a game, pin the date and the paths: Quake embeds its build time (so two builds never agree), and Doom, umac and the PC a few source paths (`-include` a header that redefines `__DATE__`/`__TIME__`, and `-fmacro-prefix-map`, through `EXTRA_CFLAGS`).

## Layout

- `doom/`, `quake/`, `umac/`, `win311/`: `build.sh` (downloads the upstream source and data pinned by SHA-256 into `work/`, patches it from `patches/`, builds it in the kit's container with `port/` and the kit's `sdk/libc/`), and the launcher that loads the game from the card and is its way back (`doomboot`, `quakeboot`, `macboot`, `pcboot`). Docs: `docs/doom.md`, `quake.md`, `umac.md`, `win311.md`.
- `nes/`: the emulator (`nes/`, with nofrendo vendored in `nofrendo/`), its launcher `nesboot`, `build.sh`, `tests/protocol.c` (the game's side of the controller adapter's CAN protocol; the adapter is rnse-hardware's `nes-can`), `package.py`. `docs/nes.md`.
- `dvd/`: the player (`dvd/`), `check.sh`, `tests/dvd_host.c`. `docs/dvd.md`.
- `tests/disc.rs` the end-to-end tests (the helpers are copies of rnse's), `tests/dvd.rs`.

## Things that are easy to miss

- **No CSS descrambler.** The DVD player recognises CSS and refuses the disc; this repository ships only the empty weak hooks of `dvd/dvd/scramble.c` for one's own. Keep it that way.
- Nothing proprietary is committed: the build scripts download only what they pin; the user's ROMs, disks and disk images stay outside.
- Quake is built with the FPU (`-m4`) while the kit's files stay without it; it reads `PAK0.PAK` from the card as it goes and plays its sound from the *other* card processor, since the card's own runs the installed program that reads the card.
- The launchers' folder names are their output names (`build.sh` names `out/NAME-UNIT.bin` after the folder): keep them unique.

## Where things are that are not here

- `sdk/` is the kit, rnse-sdk, as a git submodule pinned by tag (`git submodule update --init`); `RNSE_SDK=/path` uses another checkout. Change the kit in rnse-sdk, tag it, then move the submodule.
- A `docs/....md` or `src/....rs` named in a comment that is not in this repository is in `rnse` (https://github.com/jroark/rnse): the emulator and its reverse-engineering notes (`docs/hardware.md`, `docs/hardware-193.md`, `docs/boot-rom.md`, `docs/firmware-format.md`).
- `rnse-ports/...`, `rnse-linux/...`, `rnse-sdbridge/...`, `rnse-hardware/...` are paths in those repositories (https://github.com/jroark/NAME).
- The tests run the programs on the emulator: the crate `rnse` from git at the commit this repository was split from (`Cargo.toml`, `Cargo.lock`).
