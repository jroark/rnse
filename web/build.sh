#!/bin/sh
# Build the static site of the browser front end into target/web/site.
# Needs a Rust toolchain with the wasm32-unknown-unknown target; set CARGO
# to its cargo if that is not the one on the PATH.
set -e
cd "$(dirname "$0")/.."
: "${CARGO:=cargo}"
"$CARGO" build --release --lib --target wasm32-unknown-unknown --target-dir target/web \
    --config 'profile.release.debug=0' --config 'profile.release.strip=true'
site=target/web/site
rm -rf "$site"
mkdir -p "$site"
cp web/index.html web/worker.js assets/faceplate.png "$site/"
cp target/web/wasm32-unknown-unknown/release/rnse.wasm "$site/"
ls -l "$site"
