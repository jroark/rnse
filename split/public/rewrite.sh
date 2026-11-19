#!/bin/sh
# Phase B of split/PLAN.md: the public history of rnse.
#
#   sh split/public/rewrite.sh DEST PRIVATE [URL]
#
# PRIVATE is a directory outside every repository that holds:
#   mailmap      git mailmap lines taking each personal email address to
#                the GitHub noreply one (names unchanged);
#   replace.txt  git filter-repo --replace-text lines removing local
#                paths from the files of every commit.
# They name what must not be published, so they are never committed.
#
# Clones main alone (no other branches, no tags) from URL (default
# rnse's GitHub repository) into DEST, a new directory, and rewrites it
# with git filter-repo: the mailmap and replace.txt applied, the
# Claude-Session: lines taken out of every commit message, and taken out
# of every commit: the boards' generated fabrication ZIPs and the copies
# of mailmap and replace.txt once committed under split/public/.  Then
# runs audit.sh on the result.  Nothing is pushed: pushing is a separate
# step that needs the user's approval (PLAN.md, phase B).
set -e
here=$(cd "$(dirname "$0")" && pwd)
dest=$1
private=$2
url=${3:-git@github.com:jroark/rnse.git}
[ -n "$dest" ] && [ -n "$private" ] || { echo "usage: rewrite.sh DEST PRIVATE [URL]" >&2; exit 2; }
[ ! -e "$dest" ] || { echo "rewrite.sh: $dest exists" >&2; exit 2; }
[ -f "$private/mailmap" ] && [ -f "$private/replace.txt" ] || { echo "rewrite.sh: $private needs mailmap and replace.txt" >&2; exit 2; }
private=$(cd "$private" && pwd)
git clone -q --single-branch --branch main --no-tags "$url" "$dest"
cd "$dest"
before=$(git rev-parse HEAD^{tree})
git filter-repo --force --quiet \
    --mailmap "$private/mailmap" \
    --replace-text "$private/replace.txt" \
    --path-glob 'hardware/**/*.zip' \
    --path split/public/mailmap --path split/public/replace.txt \
    --invert-paths \
    --message-callback '
import re
return re.sub(rb"(?m)^Claude-Session: [^\n]*\n?", b"", message)
'
echo "rewrite.sh: $(git rev-list --count HEAD) commits; main's tree $before, now $(git rev-parse HEAD^{tree})"
sh "$here/audit.sh" .
