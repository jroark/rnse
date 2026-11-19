#!/bin/sh
# Make one of the repositories split from rnse, with history
# (split/PLAN.md, phase D):
#
#     sh split/extract.sh NAME SOURCE DEST      NAME: rnse-sdk, rnse-ports, ...
#
# SOURCE is a clean clone of rnse at the commit to split from (what the
# new Cargo.toml pins the emulator crate to, so it must be on GitHub);
# DEST, which must not exist, gets the new repository: a fresh clone of
# SOURCE's branch, filtered by git filter-repo, then finish.py, in one
# commit.  Nothing is pushed.
#
# split/NAME/root names the directory that becomes the root.  Taken with
# history: that directory, the names its files had before they moved into
# it (split/tools/history.py: renames only), and split/NAME/include.txt.
# Left out: split/NAME/exclude.txt (paths in the new repository) and,
# for every other repository, the old names of its files under this root
# (what moved out of sdk/ into the ports, say).  Commit messages lose
# their Claude-Session lines and session links, as rnse's did (phase B).
# Needs git-filter-repo (pip install git-filter-repo).
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
name=${1:?usage: extract.sh NAME SOURCE DEST}
source=$(CDPATH= cd -- "${2:?usage: extract.sh NAME SOURCE DEST}" && pwd)
dest=${3:?usage: extract.sh NAME SOURCE DEST}
[ -f "$here/$name/root" ] || { echo "no split/$name/root" >&2; exit 1; }
[ ! -e "$dest" ] || { echo "$dest exists" >&2; exit 1; }
command -v git-filter-repo >/dev/null || { echo "needs git-filter-repo" >&2; exit 1; }
[ -z "$(git -C "$source" status --porcelain)" ] || { echo "$source has uncommitted changes" >&2; exit 1; }
root=$(cat "$here/$name/root")
rev=$(git -C "$source" rev-parse HEAD)
branch=$(git -C "$source" rev-parse --abbrev-ref HEAD)

lists=$(mktemp -d)
trap 'rm -rf "$lists"' EXIT
strip() { [ -f "$1" ] && grep -v '^#' "$1" | grep . || true; }
{
    echo "$root"
    # A rename line only renames: the old name needs a line of its own
    # to be kept at all.
    (cd "$source" && python3 "$here/tools/history.py" "$root") | sed 's/^\(.*\)==>.*/\1\n&/'
    strip "$here/$name/include.txt"
} > "$lists/include"
{
    strip "$here/$name/exclude.txt"
    for other in "$here"/rnse-*/root; do
        [ "$other" = "$here/$name/root" ] && continue
        (cd "$source" && python3 "$here/tools/history.py" "$(cat "$other")") |
            sed 's/==>.*//' | grep "^$root" | sed "s#^$root##" || true
    done
} | sort -u > "$lists/exclude"

git clone -q --no-local --single-branch --branch "$branch" "$source" "$dest"
cd "$dest"
# 1. Keep this repository's paths, under their present names.
git filter-repo --force --quiet --paths-from-file "$lists/include" \
    --message-callback '
import re
message = re.sub(rb"(?m)^Claude-Session:.*\n?", b"", message)
return re.sub(rb"https://claude\.ai/code/session_\w+\n?", b"", message)'
# 2. Its directory becomes the root.
git filter-repo --force --quiet --path-rename "$root":
# 3. Leave what the other repositories take.
[ -s "$lists/exclude" ] && git filter-repo --force --quiet --invert-paths --paths-from-file "$lists/exclude"
git remote remove origin 2>/dev/null || true
cp "$lists/include" "$dest.include.txt"
cp "$lists/exclude" "$dest.exclude.txt"

python3 "$here/finish.py" "$name" "$dest" --source "$source" --rnse-rev "$rev"
# The lock file pins the emulator crate as well; making it fetches rnse
# at that commit from GitHub, so the commit has to be there.
if [ -f Cargo.toml ] && ! CARGO_NET_GIT_FETCH_WITH_CLI=true cargo generate-lockfile -q; then
    echo "WARNING: no Cargo.lock: rnse $rev is not on GitHub (fine for a trial run with RNSE_LOCAL)" >&2
fi
git add -A
git commit -q -F - <<MSG
Split from rnse at $rev

The history is that of rnse's $root (and of the files before they moved
into it); this commit makes the directory a repository of its own: the
emulator crate from git at that commit, links to the rnse repositories,
CLAUDE.md, README.md, .gitignore and CI (split/ in rnse).
MSG
echo "$name: $(git rev-list --count HEAD) commits; lists in $dest.include.txt, $dest.exclude.txt"
echo "next: sh $here/verify.sh $name $dest"
