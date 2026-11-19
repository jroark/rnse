#!/bin/sh
# rnse-sdk's own checks, after split/verify.sh's common ones:
#
#     sh split/verify.sh rnse-sdk DEST
#
# Every example built for every unit in tools/examples.txt, the host
# tests, and the emulator tests.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "${1:?usage: verify.sh DEST}"
fail() { echo "FAIL: rnse-sdk: $*" >&2; exit 1; }
for f in build.sh link.ld include/rnse.h src/flash.c src/wayback.c tests/emulated.rs \
         tools/sh4-docker/run tools/examples.txt docs/custom-firmware.md Cargo.toml; do
    [ -e "$f" ] || fail "$f missing"
done
[ -f Cargo.lock ] || [ -n "${RNSE_LOCAL:-}" ] || fail "Cargo.lock missing"
# Paths relative to rnse's root that the rewrite missed.  ($sdk is the
# scripts' own variable, rnse-sdk the repository's name.)
if grep -rnE '(^|[^A-Za-z0-9_./$-])sdk/[A-Za-z]' --exclude-dir=.git --exclude-dir=target --exclude-dir=third_party --exclude-dir=out . ; then
    fail "paths with an sdk/ prefix are left (above)"
fi
# What the other repositories took is in none of the commits.
for rel in examples/doomboot examples/linuxboot examples/linkbench examples/nes third_party/nofrendo third_party/h264bsd; do
    [ -z "$(git log --format=%h -- "$rel")" ] || fail "$rel is in the history"
done
sh tools/build-all.sh
sh tests/check-modern.sh
sh "$here/../tools/cargo-test.sh"
