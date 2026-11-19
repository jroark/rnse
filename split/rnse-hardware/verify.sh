#!/bin/sh
# rnse-hardware's own checks, after split/verify.sh's common ones:
#
#     sh split/verify.sh rnse-hardware DEST
#
# The three boards are there with their sources; no fabrication archive is
# tracked; the scripts compile.  KiCad's checks are not run (kicad-cli).
set -eu
cd "${1:?usage: verify.sh DEST}"
fail() { echo "FAIL: rnse-hardware: $*" >&2; exit 1; }
for b in nes-can ps2-can sdbridge-card; do
    for f in "$b/README.md" "$b/$b.kicad_pro" "$b/$b.kicad_sch" "$b/$b.kicad_pcb"; do
        [ -f "$f" ] || fail "$f missing"
    done
done
[ -z "$(git log --all --format= --name-only | grep -i '\.zip$')" ] || fail "a ZIP is in the history"
for p in $(git ls-files '*.py'); do python3 -m py_compile "$p" || fail "$p"; done
for s in $(git ls-files '*.sh'); do sh -n "$s" || fail "$s"; done
grep -q "CERN Open Hardware Licence Version 2 - Permissive" LICENSE || fail "LICENSE is not CERN-OHL-P"
