#!/usr/bin/env python3
"""Turn an extracted tree into the rnse-sdk repository (split/PLAN.md).

    finish.py DEST --source RNSE_CHECKOUT (--rnse-rev REV | --rnse-path DIR) [--prune]

DEST is either the result of extract.sh's git filter-repo passes (sdk/
already at the root, history kept) or, with --prune, a plain copy of the
same tree (sdk/'s files at the root plus docs/), which is how the step is
tested without rewriting any history.  It

  1. with --prune, deletes what exclude.txt keeps behind in rnse;
  2. rewrites paths that were relative to rnse's root (`sdk/build.sh` is
     now `build.sh`; `$PWD/sdk/tools/...` is `$PWD/tools/...`;
     `sdk/tests/emulated.rs` is `tests/emulated.rs`), and turns markdown
     links to rnse's own docs into links to the rnse repository;
  3. points Cargo.toml at the emulator crate from git (--rnse-rev) or a
     local checkout (--rnse-path, for testing), and gives it rnse's
     release profile;
  4. copies overlay/ in, with the mandatory hardware rules and the
     firmware rule pasted verbatim from rnse's CLAUDE.md.

Standard library only, like everything else in rnse.
"""
import argparse
import re
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RNSE_URL = "https://github.com/jroark/rnse"
# Docs that move into rnse-sdk; every other docs/*.md stays in rnse.
OWN_DOCS = {"custom-firmware.md", "sd-loader.md", "modern-192.md"}
# Not ours to rewrite: vendored code keeps its upstream text.
SKIP_DIRS = {".git", "target", "out", "third_party"}
TEXT_SUFFIXES = {".c", ".h", ".S", ".ld", ".sh", ".py", ".rs", ".md", ".toml", ".txt", ""}


def listed(path):
    """Non-comment, non-blank lines of one of the lists beside this file."""
    return [l.strip() for l in path.read_text().splitlines() if l.strip() and not l.lstrip().startswith("#")]


def prune(dest):
    for rel in listed(HERE / "exclude.txt"):
        p = dest / rel.rstrip("/")
        if p.is_dir():
            shutil.rmtree(p)
        elif p.exists():
            p.unlink()
        else:
            sys.exit(f"exclude.txt names {rel}, which is not in {dest}")


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


# `sdk/` as the start of a path relative to rnse's root: not after a word
# character, a dot, a slash, a dollar or a dash (so `$PWD/sdk/` is handled
# apart, and `$sdk/`, the scripts' variable, and `rnse-sdk/` are left
# alone), and followed by more path.
SDK_PREFIX = re.compile(r"(?<![\w./$-])sdk/(?=[\w.$*{-])")
# The scripts at the top, which as commands need a ./ once they are there.
SDK_SCRIPT = re.compile(r"(?<![\w./$-])sdk/(build|modern)\.sh\b")
DOC_LINK = re.compile(r"\]\(((?:\.\./docs/|docs/)?)([\w-]+\.md)(#[\w-]*)?\)")


def rewrite(dest):
    changed = []
    for p, text in text_files(dest):
        new = text.replace("$PWD/sdk/", "$PWD/")
        new = SDK_SCRIPT.sub(r"./\1.sh", new)
        new = SDK_PREFIX.sub("", new)
        if p.suffix == ".md":
            in_docs = p.parent.name == "docs"

            def link(m):
                prefix, name, anchor = m.group(1), m.group(2), m.group(3) or ""
                if not (in_docs or prefix):
                    return m.group(0)  # a link to a file beside it, not a doc
                if name in OWN_DOCS:
                    return m.group(0)
                return f"]({RNSE_URL}/blob/main/docs/{name}{anchor})"

            new = DOC_LINK.sub(link, new)
        if new != text:
            p.write_text(new)
            changed.append(str(p.relative_to(dest)))
    return changed


def cargo(dest, source, rev, path):
    toml = dest / "Cargo.toml"
    text = toml.read_text()
    # The comment about living inside rnse's workspace goes.
    text = re.sub(r"(?m)^# Inside the rnse repository.*\n(?:^#.*\n)*?(?=^\[package\])", "", text)
    if rev:
        dep = f'rnse = {{ git = "{RNSE_URL}", rev = "{rev}" }}'
    else:
        dep = f'rnse = {{ path = "{Path(path).resolve()}" }}'
    text, n = re.subn(r'(?m)^rnse = \{ path = "\.\." \}$', dep, text)
    if n != 1:
        sys.exit("Cargo.toml: no `rnse = { path = \"..\" }` to replace")
    # The emulator runs billions of cycles: the tests need rnse's profile.
    root = (source / "Cargo.toml").read_text()
    profiles = root[root.index("[profile.release]"):]
    toml.write_text(text.rstrip("\n") + "\n\n" + profiles)


def section(claude, start, end):
    i = claude.index(start)
    j = claude.index(end, i + len(start))
    return claude[i:j].rstrip("\n") + "\n"


def overlay(dest, source, rev):
    claude = (source / "CLAUDE.md").read_text()
    rules = section(claude, "## Mandatory hardware behavior", "## Commands")
    firmware = section(claude, "Firmware is copyrighted", "## Mandatory hardware behavior")
    src = HERE / "overlay"
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
    shutil.copy(HERE / "examples.txt", dest / "tools" / "examples.txt")
    for marker in ("<!-- MANDATORY", "<!-- FIRMWARE", "@RNSE_REV@"):
        for p, text in text_files(dest):
            if marker in text:
                sys.exit(f"{p}: {marker} left in")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dest", type=Path)
    ap.add_argument("--source", type=Path, required=True, help="the rnse checkout the tree came from")
    how = ap.add_mutually_exclusive_group(required=True)
    how.add_argument("--rnse-rev", help="commit of rnse for the emulator crate (git dependency)")
    how.add_argument("--rnse-path", help="a local rnse checkout for the emulator crate (testing)")
    ap.add_argument("--prune", action="store_true", help="delete exclude.txt's paths (plain copies)")
    a = ap.parse_args()
    dest, source = a.dest.resolve(), a.source.resolve()
    if not (dest / "build.sh").is_file() or (dest / "sdk").exists():
        sys.exit(f"{dest} does not look like an extracted SDK (build.sh at its root, no sdk/)")
    if a.prune:
        prune(dest)
    changed = rewrite(dest)
    cargo(dest, source, a.rnse_rev, a.rnse_path)
    overlay(dest, source, a.rnse_rev)
    print(f"rewrote {len(changed)} files; Cargo.toml and overlay done")


if __name__ == "__main__":
    main()
