#!/bin/sh
# rnse-sdbridge's own checks, after split/verify.sh's common ones:
#
#     RNSE_SDK=/path/to/rnse-sdk sh split/verify.sh rnse-sdbridge DEST
#
# The host tests, the 192's programs built with the kit, the emulator
# tests.  The RP2040 firmwares (build.sh: the pico-sdk) only with
# VERIFY_FIRMWARE=1.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "${1:?usage: verify.sh DEST}"
fail() { echo "FAIL: rnse-sdbridge: $*" >&2; exit 1; }
for f in Cargo.toml build.sh src/main.c tests/run.sh tests/linkbench.rs tests/aaviewer.rs tests/disc.rs \
         tests/data/margin.264 third_party/h264bsd docs/sdbridge.md tools/linktest.py; do
    [ -e "$f" ] || fail "$f missing"
done
[ -f Cargo.lock ] || [ -n "${RNSE_LOCAL:-}" ] || fail "Cargo.lock missing"
[ ! -e work ] || [ -z "$(git ls-files work)" ] || fail "work/ is tracked"
sdk=$(CDPATH= cd -- "${RNSE_SDK:-../rnse-sdk}" && pwd)
sh tests/run.sh
for p in linkbench h264bench aaview aaviewer; do sh "$sdk/build.sh" 192 ./$p >/dev/null && echo "built $p for the 192"; done
[ -z "${VERIFY_FIRMWARE:-}" ] || sh build.sh
sh "$here/../tools/cargo-test.sh"
