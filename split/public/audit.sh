#!/bin/sh
# Checks a repository's whole history before it is made public: no
# personal emails or private links, no local paths, no firmware or
# keys by file type, no private key or certificate blocks, no blob
# over 5 MB.  Prints what it finds; exits non-zero on any finding.
#
#   sh split/public/audit.sh REPO
set -e
cd "${1:-.}"
fail=0
bad() { echo "audit: $*"; fail=1; }
ids=$(git log --all --format='%ae%n%ce' | sort -u | grep -vE '^(noreply@anthropic\.com|noreply@github\.com|[0-9]+\+[A-Za-z0-9-]+@users\.noreply\.github\.com)$' || true)
[ -z "$ids" ] || bad "identities other than noreply ones: $ids"
# Commits already public whose messages carry a session link, kept by the
# user's decision of 2026-11-19 (split/PLAN.md, phase B): theirs are not
# counted.  Only in rnse; the split repositories strip them.
known=$(for k in 2df80d72c022f2f54b1f219b7aa2230a92a66e92 897ce980bcf1da000cc045063b2b4a8e8c48335b; do git rev-parse --verify -q "$k^{commit}" 2>/dev/null || true; done | tr '\n' ' ')
n=$(for c in $(git rev-list --all); do case " $known " in *" $c "*) continue ;; esac; git log -1 --format=%B "$c"; done |
    grep -ciE 'gmail\.com|claude\.ai/code/session|session_0' || true)
[ "$n" = 0 ] || bad "$n private strings in commit messages"
# The strings are put together here so that this file does not contain
# them, or the audit would find itself.
g=gmail; u=Users; h=home; c=claude.ai
for s in "$g.com" "/$u/" "/$h/" "$c/code/session"; do
    # (Not in this file: its first version spelled the patterns out.)
    c=$(git log --all --oneline -S"$s" -- . ':!split/public/audit.sh' | wc -l | tr -d ' ')
    [ "$c" = 0 ] || bad "'$s' in the files of $c commits: $(git log --all --format=%h -S"$s" -- . ':!split/public/audit.sh' | tr '\n' ' ')"
done
p=$(git log --all --format= --name-only | sort -u | grep -iE '\.(iso|bin|kwi|mot|img|sal|pem|key|crt|der|p12|pfx|wad|pak|dsk|rom|zip|xlsx)$' || true)
[ -z "$p" ] || bad "firmware, keys, archives or data by name: $p"
k=$(git log --all -p -G'-----BEGIN [A-Z ]*(PRIVATE KEY|CERTIFICATE)-----$' --format=%h | grep -E '^[0-9a-f]{7,}$' || true)
[ -z "$k" ] || bad "key or certificate blocks added in: $k"
b=$(git rev-list --objects --all | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' | awk '$1=="blob" && $2>5000000 {print $3" ("$2")"}')
[ -z "$b" ] || bad "blobs over 5 MB: $b"
[ $fail = 0 ] && echo "audit: clean ($(git rev-list --all --count) commits)"
exit $fail
