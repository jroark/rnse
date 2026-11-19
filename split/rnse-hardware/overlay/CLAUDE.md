# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Boards for the Audi RNS-E 192 in KiCad 10 (`README.md` lists them), split out of the emulator's repository, `rnse` (https://github.com/jroark/rnse), with their history. Each board's directory has its project, its generator and checks (`generate.py`, `audit.py`, `finish.py` where it has them), its BOM and its README, which says what has and has not been tested: say so too when changing a board.

<!-- FIRMWARE RULE -->
Generated fabrication archives (`*.zip`: `sdbridge-card/fab.sh`, `nes-can/package.py`) are made, never committed.

## Where things are that are not here

- The SD bridge card's firmware and the 192's programs at the other end are rnse-sdbridge's; the NES emulator that reads `nes-can` is rnse-ports'. Payloads for the unit follow rnse's "Mandatory hardware behavior" (eject and the display always available, RETURN held back to the loader, no firmware flashing unless asked): a board must not take those away either (the bridge card's wiring stays behind the cage and clear of the screen's opening, so the display still folds and the disc still ejects).
- `rnse-ports/...`, `rnse-sdbridge/...` are paths in those repositories (https://github.com/jroark/NAME).
