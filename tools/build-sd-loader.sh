#!/bin/sh
# Usage: PATH=tools/sh4-docker:$PATH sh tools/build-sd-loader.sh stock.iso [index]
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
disc=${1:?usage: build-sd-loader.sh stock.iso [index]}
index=${2:-0380}
sdk/build.sh 192 sdboot
sdk/build.sh 192 returntest
mkdir -p out/sd-loader
cargo run --release --bin rnse -- custom "$disc" V222 sdk/out/sdboot-192.bin out/sd-loader/sdboot.iso --index "$index"
cp sdk/out/returntest-192.bin out/sd-loader/RNSE.BIN
cargo run --release --bin rnse -- info out/sd-loader/sdboot.iso > out/sd-loader/disc-info.txt
(cd out/sd-loader && shasum -a 256 sdboot.iso RNSE.BIN > SHA256SUMS)
