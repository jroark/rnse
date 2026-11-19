#!/bin/sh
# Check a made rnse-sdk (extract.sh) or a test tree (finish.py --prune):
#
#     sh split/rnse-sdk/verify.sh DEST
#
# Builds every example for every unit, runs the host tests, builds and
# runs the emulator tests, and checks that nothing outside the tree is
# reached for.  With RNSE_DISC (and RNSE_BOOT_ROM) set the emulator
# tests run for real; without, they report success and prove nothing.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
dest=$(CDPATH= cd -- "${1:?usage: verify.sh DEST}" && pwd)
cd "$dest"
fail() { echo "FAIL: $*" >&2; exit 1; }
[ ! -e sdk ] || fail "an sdk/ directory is left"
for f in build.sh link.ld include/rnse.h src/flash.c src/wayback.c tests/emulated.rs \
         tools/sh4-docker/run tools/examples.txt docs/custom-firmware.md CLAUDE.md Cargo.toml; do
    [ -e "$f" ] || fail "$f missing"
done
grep -q "^## Mandatory hardware behavior" CLAUDE.md || fail "CLAUDE.md lacks the hardware rules"
grep -q "Always preserve physical ejection" CLAUDE.md || fail "CLAUDE.md lacks the eject rule"
grep -q "No firmware-flash option" CLAUDE.md || fail "CLAUDE.md lacks the no-flash rule"
grep -q 'rnse = {' Cargo.toml || fail "Cargo.toml has no rnse dependency"
! grep -q 'path = "\.\."' Cargo.toml || fail "Cargo.toml still points at ../"
# Paths relative to rnse's root that the rewrite missed.  ($sdk is the
# scripts' own variable, rnse-sdk the repository's name.)
if grep -rnE '(^|[^A-Za-z0-9_./$-])sdk/[A-Za-z]' --exclude-dir=.git --exclude-dir=target --exclude-dir=third_party --exclude-dir=out . ; then
    fail "paths with an sdk/ prefix are left (above)"
fi
for rel in $(grep -v '^#' "$here/exclude.txt"); do
    [ ! -e "$rel" ] || fail "$rel should have stayed in rnse"
done
if git rev-parse --git-dir >/dev/null 2>&1; then
    git ls-files | grep -iE '\.(iso|bin|kwi|mot|img|rom|wad|pak|pem|key|crt|der)$' && fail "firmware-like files are tracked"
fi
sh tools/build-all.sh
sh tests/check-modern.sh
log=$(mktemp)
cargo test --release >"$log" 2>&1 || { tail -40 "$log"; rm -f "$log"; fail "cargo test"; }
grep "^test result" "$log"
rm -f "$log"
[ -n "${RNSE_DISC:-}" ] || echo "NOTE: RNSE_DISC unset: the emulator tests returned early and proved nothing"
echo "verify: ok"
