# rnse-ports

Doom, Quake, a Mac Plus (umac), a PC running Windows 3.11 (Tiny386), an
NES emulator and a DVD player for the Audi RNS-E 192, as programs started
from an SD card.  Built on [rnse-sdk](https://github.com/jroark/rnse-sdk)
(the submodule `sdk/`) and tested on the emulator
[rnse](https://github.com/jroark/rnse), out of which this was split (at
rnse commit @RNSE_REV@, with its history).

No game data, ROMs or disk images are included: the build scripts
download the free shareware data they pin, and the rest is your own.
The DVD player plays unencrypted discs only and ships no descrambler.

```sh
git clone --recurse-submodules https://github.com/jroark/rnse-ports
PATH=$PWD/sdk/tools/sh4-docker:$PATH sh doom/build.sh
```

Programs for a real unit must keep the display opening and the disc
ejecting at all times: see CLAUDE.md, "Mandatory hardware behavior".
Licence: GPL-2.0-only (`LICENSE`); vendored and downloaded code keeps its
own.
