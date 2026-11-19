#!/bin/sh
# Make the rnse-sdk repository from rnse, with history (split/PLAN.md,
# phase 1):
#
#     sh split/rnse-sdk/extract.sh SOURCE DEST
#
# SOURCE is a clone of rnse at the commit to split from (its HEAD is
# what Cargo.toml pins the emulator crate to, so that commit must be
# pushed); DEST, which must not exist, gets the new repository: a fresh
# clone, filtered by git filter-repo, then finish.py, committed.
# Nothing is pushed.  Needs git-filter-repo (pip install git-filter-repo).
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source=$(CDPATH= cd -- "${1:?usage: extract.sh SOURCE DEST}" && pwd)
dest=${2:?usage: extract.sh SOURCE DEST}
[ ! -e "$dest" ] || { echo "$dest exists" >&2; exit 1; }
command -v git-filter-repo >/dev/null || { echo "needs git-filter-repo" >&2; exit 1; }
[ -z "$(git -C "$source" status --porcelain)" ] || { echo "$source has uncommitted changes" >&2; exit 1; }
rev=$(git -C "$source" rev-parse HEAD)
branch=$(git -C "$source" rev-parse --abbrev-ref HEAD)

lists=$(mktemp -d)
trap 'rm -rf "$lists"' EXIT
grep -v '^#' "$here/include.txt" | grep . > "$lists/include"
grep -v '^#' "$here/exclude.txt" | grep . > "$lists/exclude"

# filter-repo wants a fresh clone; only the branch being split.
git clone --no-local --single-branch --branch "$branch" "$source" "$dest"
cd "$dest"
# 1. Keep the SDK, the files that moved into it and its docs.
git filter-repo --force --paths-from-file "$lists/include"
# 2. sdk/ becomes the root (tools/sh4-docker's older history meets its
#    newer at tools/sh4-docker).
git filter-repo --force --path-rename sdk/:
# 3. Leave what later phases take.
git filter-repo --force --invert-paths --paths-from-file "$lists/exclude"
git remote remove origin 2>/dev/null || true

python3 "$here/finish.py" "$dest" --source "$source" --rnse-rev "$rev"
chmod +x tools/build-all.sh
git add -A
git commit -q -m "Split from rnse at $rev

The SDK's history comes from rnse's sdk/ (and tools/sh4-docker,
tools/build-sd-loader.sh and three docs before they moved); this commit
rewrites paths relative to rnse's root, takes the emulator crate from
git at that commit and adds CLAUDE.md, README.md, the CI and
tools/build-all.sh (split/rnse-sdk in rnse)."
git log --oneline | wc -l | xargs echo "commits:"
echo "next: sh $here/verify.sh $dest"
