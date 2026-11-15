#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [ "$#" -ne 1 ]; then printf 'Usage: %s /absolute/path/to/firmware.iso\n' "$0"; exit 2; fi
./macos/build.sh
cargo test --release --manifest-path macos/bridge/Cargo.toml
xcrun swiftc -O -swift-version 5 -target "$(uname -m)-apple-macosx15.0" -import-objc-header macos/bridge/rnse.h macos/Smoke.swift macos/bridge/target/release/librnse_macos.a -framework AppKit -o out/macos/smoke
# The runner prints the available variants and skips hardware absent from the disc.
out/macos/smoke "$1" auto 22 out/macos/smoke
