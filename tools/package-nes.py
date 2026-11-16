#!/usr/bin/env python3
"""Package the built NES payloads, adapter project, and corresponding source."""
from pathlib import Path
import hashlib
import shutil
import tarfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'out/nes-release'
OUT.mkdir(parents=True, exist_ok=True)
for name in ('RNSE.BIN', 'NESCORE.BIN'):
    shutil.copy2(ROOT / 'sdk/out/nes' / name, OUT / name)
shutil.copy2(ROOT / 'hardware/nes-can/firmware/build/nes-can.hex', OUT / 'nes-can.hex')
shutil.copy2(ROOT / 'docs/nes.md', OUT / 'README.md')
for name in ('COPYING', 'COPYING.LIB', 'UPSTREAM.md'):
    shutil.copy2(ROOT / 'sdk/third_party/nofrendo' / name, OUT / name)


def include(path):
    return (path.is_file() and not set(path.parts) & {'__pycache__', 'build'}
            and path.name != 'placement.kicad_pcb'
            and path.suffix not in ('.kicad_prl', '.lck'))


with zipfile.ZipFile(OUT / 'nes-can-hardware.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
    for path in sorted((ROOT / 'hardware/nes-can').rglob('*')):
        if include(path):
            archive.write(path, path.relative_to(ROOT / 'hardware'))

sources = ('sdk/src', 'sdk/include', 'sdk/third_party', 'sdk/examples/nes',
           'sdk/examples/nesboot', 'sdk/link.ld', 'sdk/build.sh', 'sdk/nes-build.sh',
           'tools/sh4-docker', 'tools/package-nes.py', 'tests/nes', 'docs/nes.md')
with tarfile.open(OUT / 'nes-source.tar.gz', 'w:gz') as archive:
    for name in sources:
        path = ROOT / name
        for item in sorted(path.rglob('*')) if path.is_dir() else [path]:
            if include(item):
                archive.add(item, arcname=str(item.relative_to(ROOT)), recursive=False)

files = sorted(path for path in OUT.iterdir() if path.is_file() and path.name != 'SHA256SUMS')
(OUT / 'SHA256SUMS').write_text(''.join(
    hashlib.sha256(path.read_bytes()).hexdigest() + '  ' + path.name + '\n'
    for path in files))
print(OUT)
