# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

An emulator for the Audi RNS-E navigation unit (Aisin AW, SH-4 CPU), written from scratch in Rust with **no dependencies outside the standard library** (keep it that way: PNG writing, the HTTP server and ISO 9660 parsing are all hand-rolled for this reason). It runs the unit's real firmware from an update CD image.

Firmware is copyrighted and must never be committed. `.gitignore` blocks `*.iso`, `*.bin`, `*.KWI`, `*.MOT`, `/firmware/`, `/out/`; do not add firmware-derived files (flash dumps, extracted modules, SD card images) to the repository under other names either.

## Commands

```sh
cargo build --release                 # always use --release; the emulator runs billions of cycles
cargo test --release                  # unit tests only (disc tests silently pass without a disc)
RNSE_DISC=disc.iso cargo test --release -- --nocapture   # plus end-to-end tests in tests/disc.rs
RNSE_DISC=disc.iso cargo test --release --test disc os_draws_193   # a single end-to-end test
cargo test --release sh4::tests       # CPU instruction unit tests only
```

The tests in `tests/disc.rs` return early (and report success) when `RNSE_DISC` is unset, so a green run without it proves nothing about boot behaviour. A local update disc image may be sitting in the repository root as an ignored `*.iso`; use it for `RNSE_DISC` if present. The end-to-end tests run whole firmware updates and boots and take minutes.

CPU core differential testing against QEMU (needs `qemu-sh4eb-static` from `qemu-user-static`, i.e. Linux; uses the `sh4run` binary from the release build):

```sh
tools/cpufuzz.py --count 1000 [--fpu] [--seed S] [--keep DIR]
tools/cpudiff.py DIR/test.elf DIR/test.bin    # first diverging instruction of a failing program
```

ADDV, SUBV, ROTL, ROTR, MAC.W, MAC.L and DIV1-by-zero are excluded from the fuzzer because QEMU 8.2 gets them wrong; their reference is the SH-4 manual, via `src/sh4/tests.rs`. Don't "fix" the core to match QEMU on those.

Running the emulator (`rnse` with no arguments prints every option):

```sh
rnse info disc.iso                                   # variants and modules on the disc
rnse loader disc.iso --variant V574 --save-flash flash.img   # run the update loader
rnse boot disc.iso --ignition --cycles 4000000000 \
    --key SETUP@27000 --knob 1:2@29000 --screenshot menu.png  # headless, scripted
rnse ui disc.iso --variant V574                      # browser front end on 127.0.0.1:8080
```

`boot` with `--screenshot`/`--shot MS:FILE` plus scripted `--key`/`--knob` is the way to verify UI-visible changes. The firmware's log arrives as `[fw] ...` lines on stdout, largely Japanese in EUC-JP (`| iconv -f EUC-JP -t UTF-8 -c`). The investigation switches (`--io-log`, `--trace-exc`, `--trace-bios`, `--trace-can`, `--trace-serial`, `--trace-coral`, `--trace-ge`, `--trace-atapi`, `--trace-calls`, `--tp`, `--break`, `--peek`) are the main debugging tools.

## Architecture

Two different computers are emulated, selected by the firmware *variant* name (`board::Model::for_variant`):

- **192** (`Model::Rnse192`; variants `V220` Europe, `V222` North America): SH7750R with external devices on the bus — Fujitsu Coral graphics chip, HCAN2 CAN controller, companion-chip UARTs, ATAPI drive.
- **193** (`Model::Rnse193`; variants `V574`, `V575`, `VB48`): SH7770-family system-on-chip, where most of those functions are on-chip peripherals (undocumented by Renesas) and graphics go through a 2D engine and a display unit with blended planes at 800x480.

Anything touching memory maps, peripherals, boot or the display must be considered for both models; `Machine::new()` is a 192, `Machine::for_model` picks explicitly.

Layers, bottom up:

- `src/sh4/` — CPU core, a big-endian interpreter (`interp.rs` integer/system, `fpu.rs`, `mmu.rs` UTLB only). External memory goes through the `Bus` trait with 29-bit physical addresses. The P4 region (0xE000_0000 up) is handled inside the core: `onchip.rs` models the SH7750R peripherals (INTC, TMU, SCIF, DMAC, ...), and for the 193 `navi.rs` takes every P4 address that isn't core registers, timer unit 0 or the interrupt controller.
- `src/board.rs` — `Board` implements `Bus`: memory map, chip selects, port/watchdog registers, interrupt routing (`update_irl`), display stepping. The module doc comment is the authoritative address map for the 192; `navi.rs`'s doc comment is the one for the 193.
- `src/dev/` — device models. Some are real chips (`coral.rs` graphics for the 192, `q2.rs` 2D engine for the 193, `atapi.rs`, `flash.rs`, `hcan.rs`, `scif.rs`, `uart.rs`, `adc.rs`); `radio.rs` and `panel.rs` are *stand-ins* for other processors in the unit (radio processor, faceplate controller) that answer just enough of the serial protocol for the main software to come up.
- `src/bootrom.rs` — the first 48 KiB of flash are a boot ROM that is not on any update disc. The stub places reserved opcode `0xFFFD` at each entry point; executing it stops the CPU with `Stop::Hypercall`, and `machine.rs` performs the service in Rust (`bootrom::Hook`, `TRAPA #0x3d..0x3f` services) and resumes. Everything about it is inferred from how the disc's modules use it — evidence in `docs/boot-rom.md`.
- `src/machine.rs` — `Machine` owns `Cpu` + `Board`: start-up sequences (`install`, `install_boot_stub`, `start_loader`, `start_os`), boot ROM services, the run loop (`run(cycles)`, which also advances emulated time, CAN cyclic frames, scripted input), and `render()` for the screen.
- `src/fw.rs` — disc side: ISO 9660 reader, the `LOADING.KWI` container, variants and modules with their 0x80-byte headers and checksums. Deliberately version-independent.
- Front ends: `src/bin/rnse.rs` (CLI; hand-written argument parsing and the usage text — update the usage string when adding an option), `src/ui.rs` (std-only HTTP server; the machine thread publishes frames into `Shared` and picks up input, the page in `assets/ui.html` polls), `src/bin/sh4run.rs` (bare CPU runner for the fuzzer only).

### Things that are easy to miss

- **Firmware debug output is obtained by hooking the firmware's `printf`** as a breakpoint (`Machine::hook_printf`), since the main software formats messages and sends them nowhere. The address differs per build and is listed by module version string in `Machine::known_printf`; supporting a new firmware build means adding its address there (or passing `--printf ADDR`).
- The navigation application (`APM`) has its own copy of that `printf` (`Machine::known_app_printf`, hooked with `hook_printf_also`); its log is the main source on how navigation discs are handled.
- **Navigation needs a map DVD**, given with `--nav-disc FILE` (ISO image, or an SD card image with the DVD's volume in a type-0x96 partition; `Disc::open` finds it and reads the file on demand, it is several GB). End-to-end: `RNSE_DISC=fw.iso RNSE_NAV_DISC=card.img cargo test --release --test disc navigation` (plus `RNSE_SERIAL` if the disc is tied to a unit). Scripted check of the map: `--key NAV@45000 --key SOFT_BR@52000 --screenshot map.png` with `--cycles 15000000000`.
- **Unmodelled regions behave like RAM** so register read-back works, and every access can be logged (`iolog.rs`, `--io-log`). The usual workflow for bringing up a new peripheral is: log what the firmware does to the registers, then write the model.
- Behaviour here is reverse-engineered, not specified. When something is learned about the hardware or formats, record it where the existing knowledge lives: the module doc comments (address tables) and `docs/hardware.md` (192), `docs/hardware-193.md`, `docs/firmware-format.md`, `docs/boot-rom.md`. Read the relevant doc before changing a device model.
- Time is counted in CPU cycles; convert with `Machine::cycles_per_ms()`. CLI times (`--key KEY@MS`, `--shot MS:FILE`) are emulated milliseconds, and a boot to the UI takes on the order of billions of cycles.
- A loader run ends "abnormally" by design: the 192's loader jumps to the reset entry (`Stop::Fault` with `reset_requested`), the 193's lets the watchdog reset the unit. The end-to-end tests assert this.
