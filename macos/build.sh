#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
APP="$ROOT/out/macos/RNS-E.app"
cargo build --release --manifest-path macos/bridge/Cargo.toml
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp assets/faceplate.png "$APP/Contents/Resources/faceplate.png"
xcrun swiftc -O -swift-version 5 -target "$(uname -m)-apple-macosx15.0" -import-objc-header macos/bridge/rnse.h macos/Sources/main.swift macos/Sources/Faceplate.swift macos/bridge/target/release/librnse_macos.a -framework AppKit -framework AVFoundation -o "$APP/Contents/MacOS/RNS-E"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>RNS-E</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleIdentifier</key><string>org.rnse.emulator</string>
<key>CFBundleName</key><string>RNS-E</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
mkdir -p "$ROOT/out/macos/AppIcon.iconset"
xcrun swiftc macos/Icon.swift -o "$ROOT/out/macos/make-icon"
"$ROOT/out/macos/make-icon" "$ROOT/out/macos/AppIcon.iconset"
iconutil -c icns "$ROOT/out/macos/AppIcon.iconset" -o "$APP/Contents/Resources/AppIcon.icns"
codesign --force --sign - "$APP"
printf 'Built %s\n' "$APP"
