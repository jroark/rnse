#!/bin/sh
# Phase B of split/PLAN.md: the public history of rnse.
#
#   sh split/public/rewrite.sh DEST [URL]
#
# Clones main alone (no other branches, no tags) from URL (default
# rnse's GitHub repository) into DEST, a new directory, and rewrites
# it with git filter-repo:
#   - the two personal email addresses become the GitHub noreply one
#     (mailmap; names unchanged);
#   - Claude-Session: lines leave every commit message;
#   - generated fabrication ZIPs under hardware/ leave every commit;
#   - local paths in the boards' files leave every commit (replace.txt).
# Then runs audit.sh on the result.  Nothing is pushed: pushing is a
# separate step that needs the user's approval (PLAN.md, phase B).
set -e
here=$(cd "$(dirname "$0")" && pwd)
dest=$1
url=${2:-git@github.com:jroark/rnse.git}
[ -n "$dest" ] || { echo "usage: rewrite.sh DEST [URL]" >&2; exit 2; }
[ ! -e "$dest" ] || { echo "rewrite.sh: $dest exists" >&2; exit 2; }
git clone -q --single-branch --branch main --no-tags "$url" "$dest"
cd "$dest"
before=$(git rev-parse HEAD^{tree})
git filter-repo --force --quiet \
    --mailmap "$here/mailmap" \
    --replace-text "$here/replace.txt" \
    --path-glob 'hardware/**/*.zip' --invert-paths \
    --message-callback '
import re
return re.sub(rb"(?m)^Claude-Session: [^\n]*\n?", b"", message)
'
echo "rewrite.sh: $(git rev-list --count HEAD) commits; main's tree $before, now $(git rev-parse HEAD^{tree})"
sh "$here/audit.sh" .
