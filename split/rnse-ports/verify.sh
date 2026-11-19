#!/bin/sh
# rnse-ports' own checks, after split/verify.sh's common ones:
#
#     RNSE_SDK=/path/to/rnse-sdk sh split/verify.sh rnse-ports DEST
#
# The programs built with the kit alone, the emulator tests.  The games
# (doom/build.sh and the others: downloads) are built only with
# VERIFY_GAMES=1.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "${1:?usage: verify.sh DEST}"
fail() { echo "FAIL: rnse-ports: $*" >&2; exit 1; }
for f in Cargo.toml tests/disc.rs tests/dvd.rs docs/doom.md dvd/dvd/scramble.c nes/nofrendo/COPYING tools/build-launchers.sh; do
    [ -e "$f" ] || fail "$f missing"
done
[ -f Cargo.lock ] || [ -n "${RNSE_LOCAL:-}" ] || fail "Cargo.lock missing"
# No CSS descrambler: scramble.c has only the empty weak hooks.
grep -q '__attribute__((weak))' dvd/dvd/scramble.c || fail "dvd/dvd/scramble.c lost its weak hooks"
sh tools/build-launchers.sh
if [ -n "${VERIFY_GAMES:-}" ]; then
    for g in doom quake umac win311; do sh "$g/build.sh"; done
fi
sh "$here/../tools/cargo-test.sh"
