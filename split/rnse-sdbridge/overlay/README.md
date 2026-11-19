# rnse-sdbridge

A Raspberry Pi RP2040 in an Audi RNS-E 192's SD slot, as a card on one
side and a phone's USB host on the other, and the 192's programs that
show Android Auto on the unit's own display with the phone's sound,
driven by its own knob.  Built on [rnse-sdk](https://github.com/jroark/rnse-sdk)
(the submodule `sdk/`) and tested on the emulator
[rnse](https://github.com/jroark/rnse), out of which this was split (at
rnse commit @RNSE_REV@, with its history).  The card board itself is in
[rnse-hardware](https://github.com/jroark/rnse-hardware).

No head unit certificate is included, and none ever will be: Android
Auto's handshake needs your own.  See [docs/sdbridge.md](docs/sdbridge.md)
for wiring and [docs/android-auto.md](docs/android-auto.md) for what was
measured.  Programs for a real unit must keep the display opening and the
disc ejecting at all times: see CLAUDE.md, "Mandatory hardware behavior".
Licence: MIT; h264bsd and downloaded code keep their own.
