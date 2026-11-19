# rnse-hardware

Boards for the Audi RNS-E 192, in KiCad 10:

- `sdbridge-card/` an RP2040 on a full-size SD card (Rev B, two layers,
  0.40 mm), the slot-powered form of the SD bridge whose firmware is
  [rnse-sdbridge](https://github.com/jroark/rnse-sdbridge).
  `sh sdbridge-card/fab.sh` makes the PCB order (Gerbers, drill, job
  file) with `kicad-cli`.
- `nes-can/` NES controllers to the unit's CAN bus (an ATtiny1616;
  firmware in `nes-can/firmware/`), for the NES emulator of
  [rnse-ports](https://github.com/jroark/rnse-ports).
  `python3 nes-can/package.py` packages its firmware and design.
- `ps2-can/` a PS/2 keyboard and mouse to CAN adapter, revision A: a
  prototype, not fabricated or electrically tested.

Split out of [rnse](https://github.com/jroark/rnse) (at commit
@RNSE_REV@, with its history).  Each board's README says what has and has
not been tested.  Generated fabrication archives are made by the boards'
scripts and are never committed.  Licence: CERN-OHL-P-2.0 (`LICENSE`).
