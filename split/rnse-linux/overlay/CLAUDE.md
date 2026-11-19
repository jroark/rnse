# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Linux for the Audi RNS-E 192 (SH7750R): board support and drivers for the screen, the controls, CAN, the CD drive, the SD cards and sound (the card processors), the radio processor's line, the A/D converter, flash (read only) and the battery-backed memory; a GUI (`rnse-gui`) run with the faceplate and a BusyBox shell behind it, typed on with the knob ("Rotary typing" in `docs/linux.md`). Started from an SD card by `linuxboot`, or installed in flash in the operating system's place. Built on rnse-sdk (the submodule `sdk/`), tested on the emulator `rnse`. Split out of rnse, with its history. GPL-2.0-only.

<!-- FIRMWARE RULE -->
Never commit what `build.sh` downloads or makes (`work/`, `out/`: the kernel, the toolchain, BusyBox, minimp3).

<!-- MANDATORY HARDWARE RULES -->
## Commands

```sh
git submodule update --init                       # the kit, sdk/
sh build.sh                                       # out/VMLINUX.BIN (and vmlinux): kernel, a big-endian musl toolchain, BusyBox, all downloaded pinned; in Docker
PATH=$PWD/sdk/tools/sh4-docker:$PATH ./sdk/build.sh 192 $PWD/linuxboot     # sdk/out/linuxboot-192.bin, RNSE.BIN on the card beside VMLINUX.BIN
RNSE_DISC=disc.iso RNSE_LINUX=out/VMLINUX.BIN cargo test --release -- --nocapture
rnse firmware disc.iso V222 sdk/out/linuxboot-192.bin fw.iso --kernel out/VMLINUX.BIN --swap ...   # Linux in flash (the emulator's CLI)
```

Without an SH-4 GCC on the PATH install `gcc-sh4-linux-gnu binutils-sh4-linux-gnu` (Debian/Ubuntu) or put the kit's `sdk/tools/sh4-docker` on the PATH (a container; needs Docker). The end-to-end tests return early, and report success, without `RNSE_DISC` (an update disc image: firmware, never committed) or without an SH-4 GCC; a green run without them proves only that things build. A local disc image may be sitting in the repository root as an ignored `*.iso`; use it if present. They run whole boots on the emulator and take minutes. The tests also need `RNSE_LINUX`, a kernel from today's `build.sh` (an older one boots but its user space may predate what they wait for), and those in flash `RNSE_BOOT_ROM`, a real 192's boot ROM (the file `rnse report --rom` writes; firmware too).

## Layout

- `build.sh`, `Dockerfile`, `rnse192_defconfig`, `busybox.config`, `initramfs.list`: the build.
- `overlay/` the board support and drivers (copied into the kernel tree), `patches/` changes to the kernel, `toolchain/`.
- `userland/` (`rnse-gui`, `rnse-can`, ...) and `rootfs/`: user space.
- `linuxboot/` the second stage that starts the kernel, from a card or from flash.
- `docs/linux.md` what it does and what has been seen; `tests/disc.rs` the end-to-end tests (helpers copied from rnse's).

## Things that are easy to miss

- **From flash the only way out is the guard** (the kit's `sdk/src/flash.c`). Nothing in Linux writes the flash; keep it that way, and do not weaken that path.
- The controls driver keeps the display key and eject working under Linux, and RETURN held goes back to the loader (`loader_return`, through `linuxboot`): see the rules above.

## Where things are that are not here

- `sdk/` is the kit, rnse-sdk, as a git submodule pinned by tag (`git submodule update --init`); `RNSE_SDK=/path` uses another checkout. Change the kit in rnse-sdk, tag it, then move the submodule.
- A `docs/....md` or `src/....rs` named in a comment that is not in this repository is in `rnse` (https://github.com/jroark/rnse): the emulator and its reverse-engineering notes (`docs/hardware.md`, `docs/hardware-193.md`, `docs/boot-rom.md`, `docs/firmware-format.md`).
- `rnse-ports/...`, `rnse-linux/...`, `rnse-sdbridge/...`, `rnse-hardware/...` are paths in those repositories (https://github.com/jroark/NAME).
- The tests run the programs on the emulator: the crate `rnse` from git at the commit this repository was split from (`Cargo.toml`, `Cargo.lock`).
