#!/bin/sh
# rnse-linux's own checks, after split/verify.sh's common ones:
#
#     RNSE_SDK=/path/to/rnse-sdk sh split/verify.sh rnse-linux DEST
#
# linuxboot built with the kit for both units, the emulator tests.  The
# kernel (build.sh: downloads, a container) is built only with
# VERIFY_KERNEL=1.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "${1:?usage: verify.sh DEST}"
fail() { echo "FAIL: rnse-linux: $*" >&2; exit 1; }
for f in Cargo.toml build.sh linuxboot/main.c tests/disc.rs docs/linux.md overlay/arch/sh/boards/rnse-controls.c; do
    [ -e "$f" ] || fail "$f missing"
done
[ -f Cargo.lock ] || [ -n "${RNSE_LOCAL:-}" ] || fail "Cargo.lock missing"
sdk=$(CDPATH= cd -- "${RNSE_SDK:-../rnse-sdk}" && pwd)
for unit in 192 193; do sh "$sdk/build.sh" $unit ./linuxboot >/dev/null && echo "built linuxboot for the $unit"; done
[ -z "${VERIFY_KERNEL:-}" ] || sh build.sh
sh "$here/../tools/cargo-test.sh"
