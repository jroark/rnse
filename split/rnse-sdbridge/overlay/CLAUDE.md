# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

The SD bridge: firmware for an RP2040-Zero (pico-sdk, cloned pinned by `build.sh`, never committed) that answers the Audi RNS-E 192 (SH7750R, variants `V220`/`V222`) as a read-only SD card made up as it is read, and the 192's programs at the other end of that link: `linkbench` (times the link, its words checked against `LINKTEST.BIN`), `h264bench`, and the Android Auto viewer (`aaview` loads `aaviewer`, which decodes the phone's video from `AALINK.BIN`). The 192's programs are built on the kit, rnse-sdk (https://github.com/jroark/rnse-sdk; `RNSE_SDK`, default `../rnse-sdk`), and tested on the emulator, rnse (https://github.com/jroark/rnse). It was split out of rnse with its history. `docs/sdbridge.md` (wiring, what is tested), `docs/sdbridge-laptop.md` (tools, builds, flashing, where the work stands), `docs/android-auto.md`.

<!-- FIRMWARE RULE -->
Android Auto's head unit certificate and key, and any other key or certificate, stay outside every repository (`SDHANDSHAKE.UF2` is built only with `AA_CERT`/`AA_KEY` pointing outside it); so do the bridge's logs (`work/`) and builds (`out/`, `*.UF2`).

<!-- MANDATORY HARDWARE RULES -->
## Commands

```sh
./build.sh                                  # out/SDBRIDGE.UF2, SDPHONE.UF2, SDLINK.UF2 (pico-sdk cloned, pinned)
sh tests/run.sh                             # card logic on the host, PIO programs in a simulator (tests/piosim.py)
sh ../rnse-sdk/build.sh 192 ./linkbench     # the 192's programs: linkbench, h264bench, aaview, aaviewer
cargo test --release                        # tests/linkbench.rs, tests/aaviewer.rs (no firmware); tests/disc.rs skips without RNSE_DISC
RNSE_DISC=$PWD/disc.iso cargo test --release --test disc -- --nocapture
```

`tests/disc.rs` needs `RNSE_DISC` (an update disc image, firmware: never committed) and an SH-4 GCC on the PATH (or the kit's `tools/sh4-docker`); without them it returns early and reports success, which proves nothing. A crate's tests run in its own directory, so give these variables absolute paths.

## Layout

- `src/`: the RP2040 firmwares (`main.c` the card, `card.c`, `volume.c`; `sdphone.c`, `aap.c`; `sdhandshake.c`, `aa_tls.c`; `sdlink.c`, `aastream.c`), `CMakeLists.txt`, `build.sh`, `patches/` (TinyUSB), `link.py`, `uf2.py`, `pem2c.py`.
- `linkbench/`, `h264bench/`, `aaview/`, `aaviewer/` (`aalink.h` is the format of `AALINK.BIN`; its file `place` links it outside the loader's place): the 192's programs; `third_party/h264bsd` (a `libs` entry of h264bench and aaviewer).
- `tests/`: `run.sh`, `host.c`, `tls_host.c`, `piosim.py`; the emulator tests (crate `rnse-sdbridge`; `rnse` pinned in `Cargo.toml` and `Cargo.lock`); `data/margin.*` (synthetic H.264 frames made by `tools/aa-streams.sh`, and ffmpeg's CRCs of them). `tools/linktest.py` checks the link test's words.

A `docs/....md` named in a comment that is not in this repository is in rnse (`docs/hardware.md`, "SD card processors", is the one that matters here) or, for the kit's own (`docs/custom-firmware.md`, `docs/sd-loader.md`), in rnse-sdk; `sdk/src/...` is the kit's `src/...`.

## Things that are easy to miss

- **The card is read by the 192's card processors**, running the installed software's program for them, not by the SH-4: what the bridge must answer, and how fast, is set by that program (`docs/hardware.md` in rnse, "SD card processors"). Timings measured on a unit are in `docs/sdbridge.md`; keep what was measured apart from what is assumed.
- Programs on the 192 must keep EJECT and the display working while they wait on the link: a stalled bridge must not trap the disc.
