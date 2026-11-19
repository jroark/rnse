#!/usr/bin/env python3
"""Make one of the repositories split out of rnse, with history (split/PLAN.md, phase D).

    python3 split/extract.py NAME SOURCE DEST (--rnse-rev REV | --rnse-path DIR)
                             [--sdk-url URL --sdk-tag TAG]

NAME is rnse-sdk, rnse-ports, rnse-linux, rnse-sdbridge or rnse-hardware;
its configuration is split/NAME/.  SOURCE is a clean clone of rnse at the
commit to split from (with --rnse-rev, the commit Cargo.toml pins, which
must be on GitHub; --rnse-path points the crate at a local checkout, for
trying it out).  DEST, which must not exist, gets the new repository.
Nothing is pushed.

What it does:

 1. History.  Every file under the repository's directory in SOURCE
    (sdk/, ports/, linux/, sdbridge/, hardware/), plus the paths of
    split/NAME/extra.txt, is followed back through its renames
    (`git log --follow`); git filter-repo keeps exactly those paths, each
    renamed to its place in the new repository, on a fresh clone of the
    branch.  A file's history from before it moved into the directory
    comes along; nothing else of rnse does.
 2. Paths.  Text that named paths from rnse's root is rewritten: the
    repository's own prefix goes (`linux/build.sh` is `build.sh`, but
    `<linux/module.h>` stays: only where the rest of the path exists);
    paths into another split repository read `rnse-ports/...`; markdown
    links are resolved from where the file was in rnse and point into this
    repository, another one, or rnse, on GitHub.  Vendored code
    (third_party/, nofrendo/) is left as it came.
 3. The kit.  In the repositories that build on it, the kit is the git
    submodule sdk/ (--sdk-url, --sdk-tag; without them it is left out and
    RNSE_SDK has to name one): the defaults that found it beside the
    directory in rnse now find it there.
 4. Cargo.toml takes the emulator crate from git at --rnse-rev (or a
    path), with rnse's release profile.
 5. split/NAME/overlay/ is copied in, with the mandatory hardware rules
    and the firmware rule pasted verbatim from SOURCE's CLAUDE.md.
 6. One commit, "Split from rnse at REV".

Standard library only, like everything else in rnse.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OWNER = "https://github.com/jroark"
ROOTS = {"rnse-sdk": "sdk", "rnse-ports": "ports", "rnse-linux": "linux",
         "rnse-sdbridge": "sdbridge", "rnse-hardware": "hardware"}
REPO_OF = {v: k for k, v in ROOTS.items()}
USES_KIT = {"rnse-ports", "rnse-linux", "rnse-sdbridge"}
# Vendored code keeps its upstream text.
SKIP_DIRS = {".git", "target", "out", "work", "third_party", "nofrendo"}
TEXT_SUFFIXES = {".c", ".h", ".S", ".s", ".ld", ".sh", ".py", ".rs", ".md", ".toml", ".txt",
                 ".patch", ".config", ".list", ".json", ".csv", ".yml", ""}


def git(cwd, *args, check=True):
    r = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and r.returncode:
        sys.exit(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.stdout


def listed(path):
    if not path.exists():
        return []
    return [l.strip() for l in path.read_text().splitlines() if l.strip() and not l.lstrip().startswith("#")]


# ---- 1. history ----------------------------------------------------------

def suffix(a, b):
    """How many trailing path components a and b have in common."""
    n = 0
    for x, y in zip(reversed(a.split("/")), reversed(b.split("/"))):
        if x != y:
            break
        n += 1
    return n

def renames(source, root, extra):
    """old path ==> new path, for every name each kept file ever had."""
    current = {}
    for f in git(source, "ls-files", "--", f"{root}/").splitlines():
        current[f] = f[len(root) + 1:]
    for line in extra:
        old, _, new = line.partition("==>")
        for f in git(source, "ls-files", "--", old).splitlines():
            current[f] = new + f[len(old):] if old.endswith("/") else new
    head = set(git(source, "ls-files").splitlines())
    mapping = {}
    for f, new in sorted(current.items()):
        # Newest first.  --follow takes a copy for a rename (the same test
        # tone in three places): a name that a file staying in rnse still
        # has is that file's, and so is everything older down the chain;
        # the copy's history starts where it was made.
        chain, name = [f], f
        log = git(source, "log", "--follow", "--format=", "--name-status", "--", f)
        for line in log.splitlines():
            fields = line.split("\t")
            if len(fields) == 3 and fields[2] == name:
                if not fields[0].startswith("R"):
                    break  # C: a copy, whose source is another file still
                name = fields[1]
                if name in head and name not in current:
                    break
                chain.append(name)
            elif len(fields) == 2 and fields[1] == name and fields[0] == "A":
                break
        for old in set(chain) | {f}:
            if old in mapping and mapping[old] != new:
                # --follow pairs similar files by guesswork (the same
                # licence text in two places, a launcher started as a copy
                # of another): the file whose path ends more like the old
                # one keeps it.
                mine, theirs = suffix(old, new), suffix(old, mapping[old])
                if mine == theirs:
                    sys.exit(f"{old} would go to both {mapping[old]} and {new}")
                if theirs > mine:
                    continue
            mapping[old] = new
    # An old name that is still in use at the source's head by a file that
    # stays in rnse would bring that file's history along: refuse.
    clash = sorted(o for o in mapping if o in head and o not in current)
    if clash:
        sys.exit("old names still in use in rnse: " + ", ".join(clash))
    return mapping, current


# ---- 2. paths ------------------------------------------------------------

def text_files(dest):
    for p in sorted(dest.rglob("*")):
        rel = p.relative_to(dest)
        if any(part in SKIP_DIRS for part in rel.parts) or p.is_symlink() or not p.is_file():
            continue
        if p.suffix in TEXT_SUFFIXES or p.name in {"Dockerfile", "run", "inittab"}:
            try:
                yield p, p.read_text()
            except UnicodeDecodeError:
                continue


def exists_under(base, rest):
    first = rest.split("/")[0].rstrip(".,;:)`'\"")
    return bool(first) and (base / first).exists()


def names_under(base, rest):
    """A path into another repository: an existing file or its build output."""
    first = rest.split("/")[0]
    return exists_under(base, rest) or first in ("out", "work")


LINK = re.compile(r"\]\(([^)\s#]+)(#[^)\s]*)?\)")


def rewrite(dest, source, name, root, origin):
    """origin: new relative path -> its path in rnse (for markdown links)."""
    own = re.compile(r"(?<![\w./$<\"'-])" + re.escape(root) + r"/([\w.*-][\w./*-]*)")
    others = [r for r in ROOTS.values() if r != root and not (r == "sdk" and name in USES_KIT)]
    other = re.compile(r"(?<![\w./$<\"'-])(" + "|".join(others) + r")/([\w.*-][\w./*-]*)")
    changed = []
    for p, text in text_files(dest):
        new = text
        if root == "sdk":
            new = new.replace("$PWD/sdk/", "$PWD/")
            new = re.sub(r"(?<![\w./$-])sdk/(build|modern)\.sh\b", r"./\1.sh", new)
        new = own.sub(lambda m: m.group(1) if exists_under(dest, m.group(1)) else m.group(0), new)
        new = other.sub(lambda m: f"{REPO_OF[m.group(1)]}/{m.group(2)}"
                        if names_under(source / m.group(1), m.group(2)) else m.group(0), new)
        if p.suffix == ".md":
            rel = str(p.relative_to(dest))
            was = Path(origin.get(rel, f"{root}/{rel}"))

            def link(m):
                target, anchor = m.group(1), m.group(2) or ""
                if re.match(r"^[a-z]+:", target) or target.startswith("/"):
                    return m.group(0)
                resolved = os.path.normpath(str(was.parent / target))
                if resolved.startswith("../"):
                    return m.group(0)
                top = resolved.split("/")[0]
                if top in ROOTS.values() and not (top == root):
                    if top == "sdk" and name in USES_KIT:
                        return m.group(0) if (dest / os.path.relpath(resolved, str(was.parent))).exists() else \
                            f"]({OWNER}/rnse-sdk/blob/main/{resolved[4:]}{anchor})"
                    return f"]({OWNER}/{REPO_OF[top]}/blob/main/{resolved[len(top) + 1:]}{anchor})"
                inside = resolved[len(root) + 1:] if top == root else None
                if inside is None:
                    # Mapped in by extra.txt (rnse-sdk's docs)?
                    inside = next((n for n, o in origin.items() if o == resolved), None)
                if inside is not None:
                    there = os.path.relpath(inside, os.path.dirname(rel) or ".")
                    return f"]({there}{anchor})"
                return f"]({OWNER}/rnse/blob/main/{resolved}{anchor})"

            new = LINK.sub(link, new)
        if new != text:
            p.write_text(new)
            changed.append(str(p.relative_to(dest)))
    return changed


# ---- 3. the kit ----------------------------------------------------------

KIT_DEFAULTS = [
    ('join("../sdk")', 'join("sdk")'),
    ("$here/../../sdk", "$here/../sdk"),
    ("${RNSE_SDK:-../sdk}", "${RNSE_SDK:-sdk}"),
    ("PORTS.parent / 'sdk'", "PORTS / 'sdk'"),
]
KIT_WORDS = re.compile(r"rnse's (`?)sdk/\1 beside (?:this directory|ports/)")


def kit(dest, name, url, tag):
    if name not in USES_KIT:
        return
    used = set()
    for p, text in text_files(dest):
        new = text
        for a, b in KIT_DEFAULTS:
            if a in new:
                used.add(a)
                new = new.replace(a, b)
        new = KIT_WORDS.sub(r"the submodule \1sdk/\1", new)
        if new != text:
            p.write_text(new)
    if not used:
        sys.exit("no default path to the kit found to rewrite")
    if url:
        git(dest, "-c", "protocol.file.allow=always", "submodule", "add", url, "sdk")
        git(dest / "sdk", "checkout", "-q", tag)
        git(dest, "add", "sdk", ".gitmodules")


# ---- 4. Cargo.toml -------------------------------------------------------

def cargo(dest, source, name, rev, path):
    toml = dest / "Cargo.toml"
    if not toml.exists():
        return
    text = toml.read_text()
    note = ("# The emulator crate comes from rnse on GitHub at the commit this\n"
            "# repository was split from" + (", and the kit is the submodule\n# sdk/ (RNSE_SDK to use another).\n"
                                              if name in USES_KIT else ".\n"))
    # The leading comment says how the crate lives inside rnse: from that
    # sentence to the end of the comment, it gives way to the note.
    head, sep, rest = text.partition("[package]")
    m = re.search(r"Inside[\s#]+the[\s#]+rnse[\s#]+repository", head)
    i = m.start() if m else -1
    if not sep or i < 0:
        sys.exit("Cargo.toml: no `Inside the rnse repository` comment to replace")
    kept = head[:i].rstrip()
    kept = "" if kept == "#" else (kept + "\n" if kept.endswith(".") else kept.rstrip("#").rstrip() + "\n")
    text = kept + note + sep + rest
    dep = f'rnse = {{ git = "{OWNER}/rnse", rev = "{rev}" }}' if rev else f'rnse = {{ path = "{Path(path).resolve()}" }}'
    text, n = re.subn(r'(?m)^rnse = \{ path = "\.\." \}$', dep, text)
    if n != 1:
        sys.exit('Cargo.toml: no `rnse = { path = ".." }` to replace')
    root = (source / "Cargo.toml").read_text()
    toml.write_text(text.rstrip("\n") + "\n\n" + root[root.index("[profile.release]"):])


# ---- 5. overlay ----------------------------------------------------------

def section(claude, start, end):
    i = claude.index(start)
    return claude[i:claude.index(end, i + len(start))].rstrip("\n") + "\n"


def overlay(dest, source, name, rev):
    claude = (source / "CLAUDE.md").read_text()
    rules = section(claude, "## Mandatory hardware behavior", "## Commands")
    firmware = section(claude, "Firmware is copyrighted", "## Mandatory hardware behavior")
    src = HERE / name / "overlay"
    for p in sorted(src.rglob("*")):
        if p.is_dir():
            continue
        out = dest / p.relative_to(src)
        out.parent.mkdir(parents=True, exist_ok=True)
        text = p.read_text()
        text = text.replace("<!-- MANDATORY HARDWARE RULES -->\n", rules)
        text = text.replace("<!-- FIRMWARE RULE -->\n", firmware)
        text = text.replace("@RNSE_REV@", rev or "(a local checkout)")
        out.write_text(text)
        out.chmod(p.stat().st_mode)
    for p, text in text_files(dest):
        for marker in ("<!-- MANDATORY", "<!-- FIRMWARE", "@RNSE_REV@"):
            if marker in text:
                sys.exit(f"{p}: {marker} left in")


# ---- main ----------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("name", choices=sorted(ROOTS))
    ap.add_argument("source", type=Path)
    ap.add_argument("dest", type=Path)
    how = ap.add_mutually_exclusive_group(required=True)
    how.add_argument("--rnse-rev", help="commit of rnse for the emulator crate (git dependency)")
    how.add_argument("--rnse-path", help="a local rnse checkout for the emulator crate (trying it out)")
    ap.add_argument("--sdk-url", help="rnse-sdk to add as the submodule sdk/")
    ap.add_argument("--sdk-tag", default="v0.1.0")
    ap.add_argument("--private", type=Path,
                    help="the directory of split/public/rewrite.sh's mailmap and replace.txt, applied here too")
    a = ap.parse_args()
    source, dest, name = a.source.resolve(), a.dest.resolve(), a.name
    root = ROOTS[name]
    if dest.exists():
        sys.exit(f"{dest} exists")
    if shutil.which("git-filter-repo") is None:
        sys.exit("needs git-filter-repo")
    if git(source, "status", "--porcelain"):
        sys.exit(f"{source} has uncommitted changes")
    rev = git(source, "rev-parse", "HEAD").strip()
    if a.rnse_rev and a.rnse_rev != rev:
        sys.exit(f"--rnse-rev {a.rnse_rev} is not {source}'s HEAD {rev}")
    branch = git(source, "rev-parse", "--abbrev-ref", "HEAD").strip()

    mapping, current = renames(source, root, listed(HERE / name / "extra.txt"))
    origin = {new: old for old, new in current.items()}
    # Two passes: filter-repo keeps a path that matches a rename's new name
    # too (rnse's own LICENSE beside the renamed doom/LICENSE), so first
    # keep the old names alone, then rename.
    keep = dest.parent / f".{dest.name}.keep"
    paths = dest.parent / f".{dest.name}.paths"
    keep.write_text("".join(f"{o}\n" for o in sorted(mapping)))
    paths.write_text("".join(f"{o}==>{n}\n" for o, n in sorted(mapping.items())))
    try:
        subprocess.run(["git", "clone", "-q", "--no-local", "--single-branch", "--branch", branch,
                        str(source), str(dest)], check=True)
        private = []
        if a.private:
            private = ["--mailmap", str(a.private.resolve() / "mailmap"),
                       "--replace-text", str(a.private.resolve() / "replace.txt")]
        subprocess.run(["git", "filter-repo", "--force", "--quiet", "--paths-from-file", str(keep)],
                       cwd=dest, check=True)
        subprocess.run(["git", "filter-repo", "--force", "--quiet", "--paths-from-file", str(paths), *private],
                       cwd=dest, check=True)
    finally:
        keep.unlink(missing_ok=True)
        paths.unlink(missing_ok=True)
    git(dest, "remote", "remove", "origin", check=False)
    head = set(git(dest, "ls-files").splitlines())
    want = set(current.values())
    if head != want:
        sys.exit(f"after filtering, the tree differs: missing {sorted(want - head)[:5]}, extra {sorted(head - want)[:5]}")

    changed = rewrite(dest, source, name, root, origin)
    kit(dest, name, a.sdk_url, a.sdk_tag)
    cargo(dest, source, name, a.rnse_rev, a.rnse_path)
    overlay(dest, source, name, a.rnse_rev)
    git(dest, "add", "-A")
    # The commit is public: only a GitHub noreply address may be on it.
    email = os.environ.get("SPLIT_EMAIL") or git(source, "config", "user.email", check=False).strip()
    who = os.environ.get("SPLIT_NAME") or git(source, "config", "user.name", check=False).strip()
    if not email.endswith("@users.noreply.github.com") or not who:
        sys.exit(f"the commit would carry {email!r}: set SPLIT_EMAIL (a GitHub noreply address) and SPLIT_NAME")
    git(dest, "-c", f"user.email={email}", "-c", f"user.name={who}", "commit", "-q", "-m", f"Split from rnse at {rev}\n\n"
        f"History from rnse's {root}/ and, before they moved there, the paths\n"
        f"its files had (split/extract.py in rnse).  This commit rewrites paths\n"
        f"that were relative to rnse's root, takes the emulator crate from git at\n"
        f"that commit and adds this repository's CLAUDE.md, README.md and CI\n"
        f"(split/{name}/overlay in rnse).")
    commits = git(dest, "rev-list", "--count", "HEAD").strip()
    print(f"{name}: {commits} commits, {len(want)} files, {len(mapping) - len(want)} older names followed, "
          f"{len(changed)} files with paths rewritten")


if __name__ == "__main__":
    main()
