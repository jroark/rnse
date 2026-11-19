#!/usr/bin/env python3
"""Turn an extracted tree into one of the repositories split from rnse (split/PLAN.md).

    finish.py NAME DEST --source RNSE_CHECKOUT (--rnse-rev REV | --rnse-path DIR)

NAME is the repository (rnse-sdk, rnse-ports, rnse-linux, rnse-sdbridge,
rnse-hardware); split/NAME/root the directory of rnse that DEST's root was.
DEST is what extract.sh's git filter-repo passes leave.  This

  1. turns relative markdown links that now lead out of the repository
     into links to the repository that has the file on GitHub (rnse or
     another one split from it), and fails on a link that leads nowhere;
  2. for rnse-sdk, rewrites the paths written from rnse's root (`sdk/build.sh`
     is now `./build.sh`, `$PWD/sdk/tools/...` is `$PWD/tools/...`); for the
     others, the kit beside the directory (`../sdk`, rnse's) becomes the
     checkout of rnse-sdk beside the repository (`../rnse-sdk`);
  3. points Cargo.toml, if there is one, at the emulator crate from git
     (--rnse-rev) or a local checkout (--rnse-path, for testing), with
     rnse's release profile;
  4. writes .gitignore: the directory's own, rnse's rules that apply
     anywhere (firmware, keys, data) and those for this directory, plus
     split/NAME/gitignore;
  5. copies split/NAME/overlay/ in, with the mandatory hardware rules and
     the firmware rule pasted verbatim from rnse's CLAUDE.md where the
     overlay has their markers.

Standard library only, like everything else in rnse.
"""
import argparse
import os
import re
import sys
import textwrap
from pathlib import Path

HERE = Path(__file__).resolve().parent
GITHUB = "https://github.com/jroark"
# Not ours to rewrite: vendored code keeps its upstream text.
SKIP_DIRS = {".git", "target", "out", "work", "third_party"}
TEXT_SUFFIXES = {".c", ".h", ".S", ".ld", ".sh", ".py", ".rs", ".md", ".toml", ".txt", ".yml", ""}


def listed(path):
    """Non-comment, non-blank lines of a list file, or nothing."""
    if not path.is_file():
        return []
    return [l.strip() for l in path.read_text().splitlines() if l.strip() and not l.lstrip().startswith("#")]


def repositories():
    """Each repository split from rnse: (name, its root in rnse, the extra paths it takes as they are)."""
    out = []
    for root in sorted(HERE.glob("rnse-*/root")):
        name = root.parent.name
        extras = [l for l in listed(root.parent / "include.txt") if "==>" not in l]
        out.append((name, root.read_text().strip(), extras))
    return out


def owner(path):
    """The repository that has rnse's `path` after the split, and its path there."""
    for name, root, extras in repositories():
        if path.startswith(root):
            return name, path[len(root):]
        if path in extras:
            return name, path
    return "rnse", path


def text_files(dest):
    for p in sorted(dest.rglob("*")):
        rel = p.relative_to(dest)
        if any(part in SKIP_DIRS for part in rel.parts) or p.is_symlink() or not p.is_file():
            continue
        if p.suffix in TEXT_SUFFIXES or p.name in {"Dockerfile", "run"}:
            try:
                yield p, p.read_text()
            except UnicodeDecodeError:
                continue


LINK = re.compile(r"\]\(([^)\s#]+)(#[^)\s]*)?((?:\s+\"[^\"]*\")?)\)")


def links(name, dest, source, root, extras):
    """Markdown links that lead out of the repository, to GitHub."""
    changed = []
    for p, text in text_files(dest):
        if p.suffix != ".md":
            continue
        rel = p.relative_to(dest).as_posix()
        here = rel if rel in extras else root + rel           # where it was in rnse

        def link(m):
            target, anchor, title = m.group(1), m.group(2) or "", m.group(3)
            if re.match(r"[a-z]+:", target) or target.startswith("/"):
                return m.group(0)
            was = os.path.normpath(os.path.join(os.path.dirname(here), target))
            if not (source / was).exists():
                sys.exit(f"{rel}: the link to {target} leads nowhere in rnse either")
            repo, there = owner(was)
            if repo == name:
                new = os.path.relpath(there, os.path.dirname(rel) or ".")
                return f"]({new}{anchor}{title})"
            kind = "tree" if (source / was).is_dir() else "blob"
            return f"]({GITHUB}/{repo}/{kind}/main/{there}{anchor}{title})"

        new = LINK.sub(link, text)
        if new != text:
            p.write_text(new)
            changed.append(rel)
    return changed


# `sdk/` as the start of a path relative to rnse's root: not after a word
# character, a dot, a slash, a dollar or a dash (so `$PWD/sdk/` is handled
# apart, and `$sdk/`, the scripts' variable, and `rnse-sdk/` are left
# alone), and followed by more path.
SDK_PREFIX = re.compile(r"(?<![\w./$-])sdk/(?=[\w.$*{-])")
# The scripts at the top, which as commands need a ./ once they are there.
SDK_SCRIPT = re.compile(r"(?<![\w./$-])sdk/(build|modern)\.sh\b")
# rnse's kit beside the directory: ../sdk, ../../sdk from one level down.
KIT_BESIDE = re.compile(r"(?<![\w.-])((?:\.\./)+)sdk(?![\w.-])")


def paths(name, dest):
    changed = []
    for p, text in text_files(dest):
        if name == "rnse-sdk":
            new = text.replace("$PWD/sdk/", "$PWD/")
            new = SDK_SCRIPT.sub(r"./\1.sh", new)
            new = SDK_PREFIX.sub("", new)
        else:
            new = KIT_BESIDE.sub(r"\1rnse-sdk", text)
        if new != text:
            p.write_text(new)
            changed.append(str(p.relative_to(dest)))
    return changed


def cargo(dest, source, rev, path):
    toml = dest / "Cargo.toml"
    if not toml.is_file():
        return
    text = toml.read_text()
    # The header comment loses what it says about living inside rnse's
    # workspace, and says where the emulator comes from instead.
    head, body = re.match(r"((?:#[^\n]*\n)*)(.*)", text, re.S).groups()
    words = " ".join(l[1:].strip() for l in head.splitlines())
    words = re.sub(r"\s*Inside the rnse repository.*$", "", words)
    if words:
        words += (" The emulator, the `rnse` crate, comes from git, pinned to a commit"
                  " of https://github.com/jroark/rnse (Cargo.lock pins it too).")
        head = "".join("# " + l + "\n" for l in textwrap.wrap(words, 70))
    if rev:
        dep = f'rnse = {{ git = "{GITHUB}/rnse", rev = "{rev}" }}'
    else:
        dep = f'rnse = {{ path = "{Path(path).resolve()}" }}'
    body, n = re.subn(r'(?m)^rnse = \{ path = "\.\." \}$', dep, body)
    if n != 1:
        sys.exit("Cargo.toml: no `rnse = { path = \"..\" }` to replace")
    # The emulator runs billions of cycles: the tests need rnse's profile.
    root = (source / "Cargo.toml").read_text()
    profiles = root[root.index("[profile.release]"):]
    toml.write_text(head + body.rstrip("\n") + "\n\n" + profiles)


def gitignore(name, dest, source, root):
    lines, seen = [], set()

    def add(l):
        if l and not l.startswith("#") and l in seen:
            return
        seen.add(l)
        lines.append(l)

    existing = dest / ".gitignore"          # the directory's own, now at the root
    for l in existing.read_text().splitlines() if existing.is_file() else []:
        add(l)
    if lines:
        add("")
    add("/target")
    for l in (source / ".gitignore").read_text().splitlines():
        if l.startswith("/" + root):
            add("/" + l[len(root) + 1:])
        elif l == "/firmware/":
            add(l)    # where firmware is kept, in any repository
        elif l.startswith("/"):
            continue  # another directory of rnse's
        else:
            add(l)
    for l in (HERE / name / "gitignore").read_text().splitlines() if (HERE / name / "gitignore").is_file() else []:
        add(l)
    text = re.sub(r"\n{3,}", "\n\n", "\n".join(lines).strip("\n") + "\n")
    (dest / ".gitignore").write_text(text)


def section(claude, start, end):
    i = claude.index(start)
    j = claude.index(end, i + len(start))
    return claude[i:j].rstrip("\n") + "\n"


def overlay(name, dest, source, rev):
    claude = (source / "CLAUDE.md").read_text()
    rules = section(claude, "## Mandatory hardware behavior", "## Commands")
    firmware = section(claude, "Firmware is copyrighted", "## Mandatory hardware behavior")
    src = HERE / name / "overlay"
    for p in sorted(src.rglob("*")) if src.is_dir() else []:
        if p.is_dir():
            continue
        out = dest / p.relative_to(src)
        out.parent.mkdir(parents=True, exist_ok=True)
        text = p.read_text()
        text = text.replace("<!-- MANDATORY HARDWARE RULES -->\n", rules)
        text = text.replace("<!-- FIRMWARE RULE -->\n", firmware)
        text = text.replace("@RNSE_REV@", rev or "(a local checkout)")
        out.write_text(text)
        if p.suffix == ".sh":
            out.chmod(0o755)
    for marker in ("<!-- MANDATORY", "<!-- FIRMWARE", "@RNSE_REV@"):
        for p, text in text_files(dest):
            if marker in text:
                sys.exit(f"{p}: {marker} left in")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("name")
    ap.add_argument("dest", type=Path)
    ap.add_argument("--source", type=Path, required=True, help="the rnse checkout the tree came from")
    how = ap.add_mutually_exclusive_group(required=True)
    how.add_argument("--rnse-rev", help="commit of rnse for the emulator crate (git dependency)")
    how.add_argument("--rnse-path", help="a local rnse checkout for the emulator crate (testing)")
    a = ap.parse_args()
    dest, source = a.dest.resolve(), a.source.resolve()
    if not (HERE / a.name / "root").is_file():
        sys.exit(f"no split/{a.name}/root")
    root = (HERE / a.name / "root").read_text().strip()
    extras = [l for l in listed(HERE / a.name / "include.txt") if "==>" not in l]
    if not (dest / ".git").exists() or (dest / root).is_dir() and not (source / root / root).is_dir():
        sys.exit(f"{dest} is not an extracted tree (no .git, or {root} still in it)")
    linked = links(a.name, dest, source, root, extras)
    rewritten = paths(a.name, dest)
    cargo(dest, source, a.rnse_rev, a.rnse_path)
    gitignore(a.name, dest, source, root)
    overlay(a.name, dest, source, a.rnse_rev)
    print(f"{a.name}: links in {len(linked)} files, paths in {len(rewritten)}; Cargo.toml, .gitignore, overlay done")


if __name__ == "__main__":
    main()
