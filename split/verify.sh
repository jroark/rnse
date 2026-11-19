#!/bin/sh
# Check a repository made by extract.sh (split/PLAN.md, phase D):
#
#     sh split/verify.sh NAME DEST
#
# The checks every repository gets (its history audited as rnse's was
# for going public, nothing firmware-like tracked, no relative link out of
# the tree, the hardware rules where code for the unit is built), then
# split/NAME/verify.sh DEST with that repository's builds and tests.  With
# RNSE_DISC (and RNSE_BOOT_ROM and each repository's data) set the
# emulator tests run for real; without, they report success and prove
# nothing.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
name=${1:?usage: verify.sh NAME DEST}
dest=$(CDPATH= cd -- "${2:?usage: verify.sh NAME DEST}" && pwd)
root=$(cat "$here/$name/root")
cd "$dest"
fail() { echo "FAIL: $name: $*" >&2; exit 1; }

sh "$here/public/audit.sh" . || fail "the history audit (above)"
[ -z "$(git status --porcelain)" ] || fail "uncommitted changes"
git ls-files | grep -iE '\.(iso|bin|kwi|mot|img|rom|wad|pak|dsk|sal|zip|uf2|pem|key|crt|der|p12|pfx)$' &&
    fail "firmware-like files are tracked (above)"
for f in LICENSE README.md .gitignore; do [ -f "$f" ] || fail "$f missing"; done
[ ! -e "$root" ] || fail "a $root directory is left"
# Relative markdown links that lead nowhere.
python3 - <<'PY' || fail "broken links (above)"
import os, re, subprocess, sys
bad = 0
for f in subprocess.check_output(["git", "ls-files", "*.md"], text=True).split():
    for m in re.finditer(r"\]\(([^)\s#]+)(#[^)\s]*)?(?:\s+\"[^\"]*\")?\)", open(f).read()):
        t = m.group(1)
        if re.match(r"[a-z]+:", t) or t.startswith("/"):
            continue
        if not os.path.exists(os.path.normpath(os.path.join(os.path.dirname(f), t))):
            print(f"{f}: {t}"); bad = 1
sys.exit(bad)
PY
if [ -f Cargo.toml ]; then
    grep -q 'rnse = { git = "https://github.com/jroark/rnse", rev = "' Cargo.toml || fail "Cargo.toml does not take rnse from git"
    grep -q '^\[profile.release\]' Cargo.toml || fail "Cargo.toml lacks rnse's release profile"
fi
if [ "$name" != rnse-hardware ]; then
    # Code for the unit is built here: the rules, verbatim from rnse.
    [ -f CLAUDE.md ] || fail "CLAUDE.md missing"
    python3 - "$here/../CLAUDE.md" CLAUDE.md <<'PY' || fail "CLAUDE.md lacks rnse's hardware rules, verbatim"
import sys
src, dst = (open(p).read() for p in sys.argv[1:])
i = src.index("## Mandatory hardware behavior"); j = src.index("## Commands", i)
sys.exit(src[i:j].rstrip() not in dst)
PY
    if grep -rnE '(^|[^A-Za-z0-9_.-])(\.\./)+sdk([^A-Za-z0-9_.-]|$)' --exclude-dir=.git --exclude-dir=target --exclude-dir=third_party . ; then
        fail "rnse's kit beside the directory (../sdk) is still named (above)"
    fi
fi
sh "$here/$name/verify.sh" "$dest"
echo "verify: $name: ok"
