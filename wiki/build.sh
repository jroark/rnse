#!/bin/sh
# Build rns-e.com into target/wiki/site: the wiki pages (tools/wiki.py)
# with the browser emulator (web/build.sh) under emulator/.
# Needs a Rust toolchain with the wasm32-unknown-unknown target; set CARGO
# to its cargo if that is not the one on the PATH.  WIKI_ONLY=1 skips the
# WebAssembly build and reuses target/web/site as it is.
set -e
cd "$(dirname "$0")/.."
[ -n "$WIKI_ONLY" ] || web/build.sh
python3 tools/wiki.py target/wiki/site
mkdir -p target/wiki/site/emulator
cp target/web/site/index.html target/web/site/worker.js target/web/site/rnse.wasm \
   target/web/site/faceplate.png target/wiki/site/emulator/
find target/wiki/site -type f | wc -l | xargs echo "files:"
du -sh target/wiki/site
