# RNS-E hardware requirements

## Always preserve physical disc ejection

Every custom firmware, loader, diagnostic and test payload must support the
physical controls needed to open/fold down the display and eject the disc.
Keep faceplate polling and eject handling responsive on normal screens, error
screens, and during long-running reads, initialization and tests. Do not reserve
the display/open key solely for UI navigation without an accessible physical
way to open the display and reach the disc-eject control. The SDK's
KEY_DISPLAY is named EJECT by the emulator; KEY_CD_EJECT is a separate key.

Verify the actual eject command path before providing a hardware test build.
A visible button, loader-return shortcut or reset is not evidence that physical
ejection works. Review updater handoffs too: never assume the firmware reached
through a handoff provides working eject controls.

User-reported hardware incident (2026-11-15): pressing SETUP twice on the
original loader disc invoked the stock updater and flashed firmware. The
resulting firmware did not support the eject button, trapping the disc and
requiring disassembly of the RNS-E. Always include eject functionality.
