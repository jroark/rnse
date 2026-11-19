#!/usr/bin/env python3
"""The names a directory's files had before they moved there (split/PLAN.md, phase D).

    history.py ROOT        in an rnse checkout, e.g.  history.py ports/

For each file under ROOT at HEAD, `git log --follow` lists the names it had;
this prints an `old==>new` line (git filter-repo --paths-from-file) for each
old name outside ROOT.  Only renames count: the first copy met going back
ends the file's history, since what came before it is the history of the
file it was copied from (a LICENSE copied from nofrendo's COPYING, a
launcher started as a copy of another), which stays with that file.
An old name that two files claim is an error.  Standard library only.
"""
import subprocess
import sys


def history(root):
    files = subprocess.check_output(["git", "ls-files", "--", root], text=True).split()
    renames = {}
    for f in files:
        out = subprocess.check_output(
            ["git", "log", "--follow", "-M", "--name-status", "--format=", "--", f], text=True)
        for line in out.splitlines():
            parts = line.split("\t")
            if parts[0].startswith("C"):
                break
            if not parts[0].startswith("R"):
                continue
            old = parts[1]
            if old.startswith(root):
                continue
            if renames.get(old, f) != f:
                sys.exit(f"{old} was both {renames[old]} and {f}")
            renames[old] = f
    return renames


if __name__ == "__main__":
    for old, new in sorted(history(sys.argv[1]).items()):
        print(f"{old}==>{new}")
