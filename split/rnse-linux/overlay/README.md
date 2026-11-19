# rnse-linux

Linux on the Audi RNS-E 192: board support and drivers for the unit's
screen, controls, CAN, CD drive, SD cards and sound, a GUI run with the
faceplate and a BusyBox shell typed on with the knob.  Started from an SD
card, or installed in flash with a guard that takes it out again.  Built
on [rnse-sdk](https://github.com/jroark/rnse-sdk) (the submodule `sdk/`)
and tested on the emulator [rnse](https://github.com/jroark/rnse), out of
which this was split (at rnse commit @RNSE_REV@, with its history).
See [docs/linux.md](docs/linux.md).

Programs for a real unit must keep the display opening and the disc
ejecting at all times: see CLAUDE.md, "Mandatory hardware behavior".
Licence: GPL-2.0-only; the kernel, BusyBox and the toolchain are
downloaded and keep their own.
