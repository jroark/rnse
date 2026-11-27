# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

An emulator for the Audi RNS-E navigation unit (Aisin AW, SH-4 CPU), written from scratch in Rust with **no dependencies outside the standard library** (keep it that way: PNG writing, the HTTP server and ISO 9660 parsing are all hand-rolled for this reason). It runs the unit's real firmware from an update CD image.

Firmware is copyrighted and must never be committed. `.gitignore` blocks `*.iso`, `*.bin`, `*.KWI`, `*.MOT`, `/firmware/`, `/out/`; do not add firmware-derived files (flash dumps, extracted modules, SD card images) to the repository under other names either.

## Mandatory hardware behavior

- **Always preserve physical ejection.** Every custom firmware, loader,
  diagnostic and test payload must support opening/folding down the display
  and ejecting the disc. Keep faceplate polling and eject handling responsive
  on normal screens, error screens and during long-running operations. Do not
  repurpose the display/open key without another accessible physical way to
  open the display and reach the disc-eject control. SDK `KEY_DISPLAY` is
  emulator `EJECT`; `KEY_CD_EJECT` is a separate key. Verify the actual eject
  command path before providing hardware test builds, including across handoffs.
- **Hold RETURN for two seconds to return to the loader.** Every payload
  launched by the SD loader must implement this shortcut from every screen,
  close active apps and stop playback before calling `loader_return(status)`.
  A short RETURN retains its normal back/cancel behavior; a long hold must not
  also trigger that short action or immediately relaunch the payload after
  returning. Return directly to the resident loader without resetting or
  invoking an updater. Outside a loader session, report that return is
  unavailable and remain operable, with eject still working.
- **No firmware-flash option unless the user explicitly asks for it.** Default
  custom loaders, test payloads and UIs must neither flash firmware nor expose
  any updater/install/restore/erase option or hidden button sequence that can
  do so. This includes double-SETUP, `stock_update()`, firmware removal and
  automatic updater handoffs. Permission to build, test, recover, return to
  the loader or eject a disc is not permission to flash. Existing updater
  mechanisms described elsewhere in this file are technical background, not
  authorization to expose or invoke them. Include a flash option only when
  explicitly requested by the user.

Hardware incident reported 2026-11-15: double-SETUP on the original loader
invoked an updater and flashed firmware whose eject button did not work. The
disc was trapped and the user had to disassemble the RNS-E. These requirements
apply to all future hardware builds.

## Commands

```sh
cargo build --release                 # always use --release; the emulator runs billions of cycles
cargo test --release                  # unit tests only (disc tests silently pass without a disc)
RNSE_DISC=disc.iso cargo test --release -- --nocapture   # plus end-to-end tests in tests/disc.rs
RNSE_DISC=disc.iso cargo test --release --test disc os_draws_193   # a single end-to-end test
cargo test --release sh4::tests       # CPU instruction unit tests only
```

The software for the unit that used to live here (the SDK, the ports, Linux, the SD bridge, the boards) is in repositories of its own, split from this one with their history: rnse-sdk (https://github.com/jroark/rnse-sdk), rnse-ports, rnse-linux, rnse-sdbridge and rnse-hardware. Each pins a commit of this crate in its `Cargo.toml`; `.github/workflows/downstream.yml` runs the tests of the four that use it (most of the emulator's loader, card and flash paths are exercised there, by real programs) against every change here. A change to the emulator's behaviour can break them: run that workflow, or their tests locally against this checkout (below), before merging one. A path such as `sdk/examples/report` or `linux/linuxboot` in a comment or doc here names a file of the repository of that name (`sdk/` is rnse-sdk's root), as it was here before the split.

The tests in `tests/disc.rs` return early (and report success) when `RNSE_DISC` is unset, so a green run without it proves nothing about boot behaviour. The same goes for the comparisons with a real 192 boot ROM without `RNSE_BOOT_ROM=file` (the file `rnse report --rom` writes; firmware, never committed). A run that tests everything here for the 192:

```sh
RNSE_DISC=$PWD/disc.iso RNSE_BOOT_ROM=$PWD/rom.bin cargo test --release --test disc
```

The split repositories' tests against this checkout, with them cloned beside it (they find the kit as `../rnse-sdk`) and an SH-4 GCC on the PATH (rnse-sdk's `tools/sh4-docker` has one in a container):

```sh
cd ../rnse-sdk && RNSE_DISC=$PWD/../rnse/disc.iso cargo test --release --config 'patch."https://github.com/jroark/rnse".rnse.path="../rnse"'
```
 A local update disc image may be sitting in the repository root as an ignored `*.iso`; use it for `RNSE_DISC` if present. The end-to-end tests run whole firmware updates and boots and take minutes.

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
rnse boot disc.iso --ignition --cycles 2800000000 \
    --key SETUP@7000 --knob 1:2@8500 --screenshot menu.png  # headless, scripted
rnse ui disc.iso --variant V574                      # browser front end on 127.0.0.1:8080
```

`boot` with `--screenshot`/`--shot MS:FILE` plus scripted `--key`/`--knob` is the way to verify UI-visible changes. The firmware's log arrives as `[fw] ...` lines on stdout, largely Japanese in EUC-JP (`| iconv -f EUC-JP -t UTF-8 -c`). `--gdb PORT` serves GDB's remote protocol (`src/gdb.rs`; rnse-sdk builds ELF files with debug information for it). The investigation switches (`--io-log`, `--trace-exc`, `--trace-bios`, `--trace-can`, `--trace-serial`, `--trace-coral`, `--trace-ge`, `--trace-atapi`, `--trace-flash`, `--trace-sd`, `--trace-dsp`, `--trace-sound`, `--trace-calls`, `--tp`, `--break`, `--peek`, `--watch-store`) are the main debugging tools.

## Architecture

Two different computers are emulated, selected by the firmware *variant* name (`board::Model::for_variant`):

- **192** (`Model::Rnse192`; variants `V220` Europe, `V222` North America): SH7750R with external devices on the bus — Fujitsu Coral graphics chip, HCAN2 CAN controller, companion-chip UARTs, ATAPI drive, and two NEC signal processors for the SD cards and MP3.
- **193** (`Model::Rnse193`; variants `V574`, `V575`, `VB48`): a Renesas R8A77730 system-on-chip (SH-Navi family; long taken for an SH7770), where most of those functions are on-chip peripherals (undocumented by Renesas) and graphics go through a 2D engine and a display unit with blended planes at 800x480.

Anything touching memory maps, peripherals, boot or the display must be considered for both models; `Machine::new()` is a 192, `Machine::for_model` picks explicitly.

Layers, bottom up:

- `src/sh4/` — CPU core, a big-endian interpreter (`interp.rs` integer/system, `fpu.rs`, `mmu.rs` UTLB only). External memory goes through the `Bus` trait with 29-bit physical addresses. The P4 region (0xE000_0000 up) is handled inside the core: `onchip.rs` models the SH7750R peripherals (INTC, TMU, SCIF, DMAC, ...), and for the 193 `navi.rs` takes every P4 address that isn't core registers, timer unit 0 or the interrupt controller.
- `src/board.rs` — `Board` implements `Bus`: memory map, chip selects, port/watchdog registers, interrupt routing (`update_irl`), display stepping. The module doc comment is the authoritative address map for the 192; `navi.rs`'s doc comment is the one for the 193.
- `src/dev/` — device models. Some are real chips (`coral.rs` graphics for the 192, `q2.rs` 2D engine for the 193, `atapi.rs`, `flash.rs`, `hcan.rs`, `scif.rs`, `uart.rs`, `adc.rs`, `sdhi.rs` the 193's SD host interface, `upd77115.rs` the 192's two card processors with `sdcard.rs` and `media.rs`); `radio.rs` and `panel.rs` are *stand-ins* for other processors in the unit (radio processor, faceplate controller) that answer just enough of the serial protocol for the main software to come up.
- `src/bootrom.rs` — the first 48 KiB of flash are a boot ROM that is not on any update disc (a real 192's can be put in its place with `rnse boot --boot-rom FILE`; such an image is firmware and lives outside the repository). The stub places reserved opcode `0xFFFD` at each entry point; executing it stops the CPU with `Stop::Hypercall`, and `machine.rs` performs the service in Rust (`bootrom::Hook`, `TRAPA #0x3d..0x3f` services) and resumes. Everything about it is inferred from how the disc's modules use it — evidence in `docs/boot-rom.md`.
- `src/machine.rs` — `Machine` owns `Cpu` + `Board`: start-up sequences (`install`, `install_boot_stub`, `start_loader`, `start_os`), boot ROM services, the run loop (`run(cycles)`, which also advances emulated time, CAN cyclic frames, scripted input), and `render()` for the screen.
- `src/fw.rs` — disc side: ISO 9660 reader, the `LOADING.KWI` container, variants and modules with their 0x80-byte headers and checksums. Deliberately version-independent.
- The software for the unit is in the split repositories (above). What the emulator offers it: `rnse custom` puts a program in the update loader's place on a copy of a disc (it then runs from RAM; the image carries the stock loader as the way back), `rnse firmware` puts one in the operating system's place, to be installed in flash and started by the boot ROM (192; rnse-sdk's guard, `src/flash.c` there, is the only way it is taken out again: do not weaken that path), `rnse firmware ... --kernel` adds a Linux kernel in the application's place (`fw::firmware_disc_with_kernel`, for rnse-linux's `linuxboot`), `rnse card` writes FAT card images (`src/fat.rs`), and `rnse report` (`src/report.rs`) reassembles the CAN log of rnse-sdk's `report` example, which reads a real unit's registers, boot ROM and card processors. rnse-sdk's `docs/custom-firmware.md` says what the emulator does and does not establish about real hardware.
- Front ends: `src/bin/rnse.rs` (CLI; hand-written argument parsing and the usage text — update the usage string when adding an option), `src/ui.rs` (std-only HTTP server; the machine thread publishes frames into `Shared` and picks up input, the page in `assets/ui.html` polls), `src/bin/sh4run.rs` (bare CPU runner for the fuzzer only).

### Things that are easy to miss

- **Firmware debug output is obtained by hooking the firmware's `printf`** as a breakpoint (`Machine::hook_printf`), since the main software formats messages and sends them nowhere. The address differs per build and is listed by module version string in `Machine::known_printf`; supporting a new firmware build means adding its address there (or passing `--printf ADDR`).
- The navigation application (`APM`) has its own copy of that `printf` (`Machine::known_app_printf`, hooked with `hook_printf_also`); its log is the main source on how navigation discs are handled.
- **Navigation needs a map DVD**, given with `--nav-disc FILE` (ISO image, or an SD card image with the DVD's volume in a type-0x96 partition; `Disc::open` finds it and reads the file on demand, it is several GB). End-to-end: `RNSE_DISC=fw.iso RNSE_NAV_DISC=card.img cargo test --release --test disc navigation` (plus `RNSE_SERIAL` if the disc is tied to a unit). Scripted check of the map: `--key NAV@22500 --key SOFT_BR@26000 --screenshot map.png` with `--cycles 15000000000`.
- **Unmodelled regions behave like RAM** so register read-back works, and every access can be logged (`iolog.rs`, `--io-log`). The usual workflow for bringing up a new peripheral is: log what the firmware does to the registers, then write the model.
- Behaviour here is reverse-engineered, not specified. When something is learned about the hardware or formats, record it where the existing knowledge lives: the module doc comments (address tables) and `docs/hardware.md` (192), `docs/hardware-193.md`, `docs/firmware-format.md`, `docs/boot-rom.md`. Read the relevant doc before changing a device model.
- **The update loader is started by the installed application, not the boot ROM**, when the disc in the drive has a higher software index than the installed one (`docs/boot-rom.md`, "Starting the loader"). `rnse boot disc.iso --installed-index NNNN` (an index lower than the disc's in its 2nd and 3rd digit) runs that whole path with the stock software; rnse-sdk's `tests/emulated.rs` `stock_software_starts_a_custom_program` does it with a program of the kit on the disc. `rnse loader` is the shortcut that starts the loader directly. A disc put in while the unit runs (`--disc-in MS`) starts an update too, on every software seen, but only because the emulated drive says "not ready" (2/04/00) rather than "becoming ready" (2/04/01) while it loads the disc: the OS waits on the second by itself and the application then never sees the disc arrive. That choice is inferred from the software (same section of the doc; `Atapi::loading_sense`, `--loading-sense`), and rnse-sdk's `examples/drive` is how to check it on a unit.
- **`--strict` is the hardware-realism mode**: arbitrary RAM contents, the watchdog armed from the start, a loader started with timers and interrupts in use, and the CPU's caches modelled (`src/sh4/cache.rs`; off otherwise, the core then takes its fast paths). The SDK's tests run with all of it. Emulator code that reads or writes RAM on the CPU's behalf has to go through `Machine::peek8`/`poke8` or happen inside a boot ROM service (the caches are synchronised around those), or it will disagree with the cache model.
- Time is counted in CPU cycles; convert with `Machine::cycles_per_ms()`. CLI times (`--key KEY@MS`, `--shot MS:FILE`) are emulated milliseconds, and a boot to the UI takes on the order of billions of cycles (the user interface is up after about 6 s of emulated time on the 192 and 15 s on the 193). The operating system's tick is 1 ms on both units (peripheral clocks 58.8 and 50 MHz); the 193's is pinned by its sound output, see "Clocks and time" in `docs/hardware-193.md`. One instruction counts as one CPU cycle, which is an estimate: `Machine::set_cpu_ratio`/`--cpu-ratio` change the CPU's pace against the timers, and anything timing-sensitive deserves a look at a neighbouring ratio.
- **The 193 reads SD cards and plays MP3 itself**: `src/dev/sdhi.rs` (host interface), the DMA channels and the serial sound interface in `navi.rs`; `src/fat.rs` and `rnse card` write card images; `tests/disc.rs` `mp3_from_a_card_plays` goes from the card to the samples (`tests/data/tones.mp3` is a synthetic tone, not firmware).
- **The 192's cards and MP3 belong to two signal processors of their own** (NEC uPD77115A), which are emulated: `src/dev/upd77115.rs` is the processor (instruction set, host port, timer, audio output, SD card interface, the boot ROM's services), `src/dev/sdcard.rs` the card, `src/dev/media.rs` the wiring to the main processor. They run the program the main software downloads, so the protocol between the two is nobody's guess; read `docs/hardware.md`, "SD card processors", before touching any of it. Several codes of the instruction set are not in NEC's manual and were found from that program (listed in the module's comments). The part's undocumented ROM at 0x7000 holds the SD copy protection and is deliberately not modelled. `--trace-sd`, `--trace-dsp`, `--dsp-tp ADDR[+N]` and `--dsp-watch A-B` are the tools; `tests/disc.rs` `mp3_from_a_card_plays_on_the_192` is the end-to-end check.
- **What is on the 192's boards is known from photographs** (`docs/hardware.md`, "The boards"): two 16 MiB flash chips, 256 KiB of battery-backed SRAM, an MB86293 graphics chip, two NEC uPD77115A signal processors for the cards, and so on. Check a device model against that section before changing it. The 193's are in `docs/hardware-193.md`, "The boards": four Spansion PL127J flash chips as two 32 MiB banks of 16-bit pairs (`Flash::paired`), 256 KiB of battery-backed SRAM, an R8A77730 SoC.
- **A CAN frame the unit sends takes its time on the emulated bus** (`Machine::can_tick`, `Hcan::tx_next`/`tx_complete`); a bare `Hcan` in a unit test needs `tx_flush`.
- **The European 192 software's own start of a loader can collide with one of its timer handlers** (`docs/boot-rom.md`, "Starting the loader"): `--installed-index` runs on V220 end in an address error at the default CPU ratio and work at 3 or 5; the tests retry at a neighbouring ratio for exactly that fault.
- A loader run ends "abnormally" by design: the 192's loader jumps to the reset entry (`Stop::Fault` with `reset_requested`), the 193's lets the watchdog reset the unit. The end-to-end tests assert this.
