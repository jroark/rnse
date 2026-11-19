#!/usr/bin/env python3
"""Check a repository made by split/extract.py (split/PLAN.md, phase D).

    python3 split/verify.py NAME DEST [SOURCE]

SOURCE is the rnse checkout it was made from (default: this one).

Checks its shape and rules (CLAUDE.md with the hardware rules verbatim
where programs for the unit are built, LICENSE, README, the emulator crate
from git), that no path relative to rnse's root is left, the privacy and
firmware audit (split/public/audit.sh), then builds what it builds and
runs its tests as its CI does.  The kit is the submodule sdk/ or RNSE_SDK.
With RNSE_DISC (and the data the tests name) set, the emulator tests run
for real; without, they report success and prove nothing, and this says
so.
"""
import importlib.util
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("extract", HERE / "extract.py")
ex = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ex)

RULES = ("## Mandatory hardware behavior", "Always preserve physical ejection",
         "Hold RETURN for two seconds", "No firmware-flash option")
PAYLOADS = {"rnse-sdk", "rnse-ports", "rnse-linux", "rnse-sdbridge"}
CHECKS = {
    "rnse-sdk": ["sh tools/build-all.sh", "sh tests/check-modern.sh"],
    "rnse-ports": [
        "for p in doom/doomboot quake/quakeboot umac/macboot win311/pcboot dvd/dvd; do "
        "for u in 192 193; do sh $SDK/build.sh $u $PWD/$p >/dev/null || exit 1; done; done",
        "sh nes/build.sh >/dev/null",
        "cc -Wall -Wextra -Werror -I$SDK/include -Ines/nes nes/tests/protocol.c nes/nes/protocol.c -o $TMP/p && $TMP/p",
        "python3 -I -m py_compile nes/package.py",
    ],
    "rnse-linux": ["for u in 192 193; do sh $SDK/build.sh $u $PWD/linuxboot >/dev/null || exit 1; done"],
    "rnse-sdbridge": [
        "sh tests/run.sh >/dev/null",
        "for p in linkbench h264bench aaview; do for u in 192 193; do sh $SDK/build.sh $u $PWD/$p >/dev/null || exit 1; done; done",
        "sh $SDK/build.sh 192 $PWD/aaviewer >/dev/null",
    ],
    "rnse-hardware": [
        "for f in $(git ls-files '*.py'); do python3 -I -m py_compile $f || exit 1; done",
        "if command -v kicad-cli >/dev/null || [ -x /Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli ]; "
        "then sh sdbridge-card/fab.sh >/dev/null && rm sdbridge-card/manufacturing/sdbridge-card-revB.zip; "
        "else echo 'no kicad-cli: fab.sh not tried'; fi",
    ],
}


def main():
    name, dest = sys.argv[1], Path(sys.argv[2]).resolve()
    global SOURCE
    SOURCE = Path(sys.argv[3]).resolve() if len(sys.argv) > 3 else HERE.parent
    root = ex.ROOTS[name]
    bad = []

    def need(cond, what):
        if not cond:
            bad.append(what)

    need((dest / "LICENSE").is_file(), "LICENSE missing")
    need((dest / "README.md").is_file(), "README.md missing")
    claude = (dest / "CLAUDE.md").read_text() if (dest / "CLAUDE.md").exists() else ""
    need(bool(claude), "CLAUDE.md missing")
    if name in PAYLOADS:
        for r in RULES:
            need(r in claude, f"CLAUDE.md lacks {r!r}")
    toml = dest / "Cargo.toml"
    if toml.exists():
        t = toml.read_text()
        need('path = ".."' not in t, "Cargo.toml still points at ../")
        need("rnse = {" in t and "[profile.release]" in t, "Cargo.toml: no rnse dependency or release profile")
    if name in ex.USES_KIT:
        sdk = Path(os.environ.get("RNSE_SDK", dest / "sdk"))
        need((sdk / "build.sh").is_file(), f"no kit at {sdk} (git submodule update --init, or RNSE_SDK)")
    else:
        sdk = dest
    need(not (dest / root).exists(), f"a {root}/ directory is left")

    # Paths relative to rnse's root that the rewrite missed: the repository's
    # own prefix where the rest exists here, or another one's directory.
    own = re.compile(r"(?<![\w./$<\"'-])" + re.escape(root) + r"/([\w.*-][\w./*-]*)")
    others = [r for r in ex.ROOTS.values() if r != root and not (r == "sdk" and name in ex.USES_KIT)]
    other = re.compile(r"(?<![\w./$<\"'-])(" + "|".join(others) + r")/([\w.*-][\w./*-]*)")
    left = []
    for p, text in ex.text_files(dest):
        if p.name in ("CLAUDE.md", "README.md") and p.parent == dest:
            continue
        for m in own.finditer(text):
            if ex.exists_under(dest, m.group(1)):
                left.append(f"{p.relative_to(dest)}: {m.group(0)}")
        for m in other.finditer(text):
            if m.group(1) in ("hardware",) and name == "rnse-sdbridge" and m.group(2).endswith(".h"):
                continue  # the pico-sdk's headers
            if not ex.names_under(SOURCE / m.group(1), m.group(2)):
                continue  # not a path of that directory ("reports/...", prose)
            left.append(f"{p.relative_to(dest)}: {m.group(0)}")
    need(not left, "paths relative to rnse's root are left:\n  " + "\n  ".join(left[:20]))

    audit = subprocess.run(["sh", str(HERE / "public/audit.sh"), str(dest)], capture_output=True, text=True)
    print(audit.stdout.strip())
    need(audit.returncode == 0, "the audit found something (above)")
    if bad:
        sys.exit("FAIL:\n" + "\n".join(bad))

    env = dict(os.environ, SDK=str(sdk), TMP=str(Path(os.environ.get("TMPDIR", "/tmp")).resolve()))
    for cmd in CHECKS[name]:
        r = subprocess.run(["sh", "-ec", cmd], cwd=dest, env=env, capture_output=True, text=True)
        print(("ok    " if r.returncode == 0 else "FAIL  ") + cmd[:100] + (f"\n{r.stdout[-600:]}{r.stderr[-1500:]}" if r.returncode or r.stdout.strip() else ""))
        if r.returncode:
            sys.exit("FAIL: " + cmd)
    if toml.exists():
        r = subprocess.run(["cargo", "test", "--release"], cwd=dest, env=env, capture_output=True, text=True)
        results = [l for l in r.stdout.splitlines() if l.startswith("test result")]
        print("\n".join(results) or r.stderr[-2000:])
        if r.returncode:
            sys.exit("FAIL: cargo test\n" + "\n".join(l for l in r.stdout.splitlines() if "FAILED" in l or "panicked" in l))
        if not os.environ.get("RNSE_DISC"):
            print("NOTE: RNSE_DISC unset: the end-to-end tests returned early and proved nothing")
    print(f"verify {name}: ok")


if __name__ == "__main__":
    main()
