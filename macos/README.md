# RNS-E for macOS

A native AppKit frontend for the existing Rust emulator. Requires macOS 15 or newer,
Xcode command line tools, and Rust. It uses Apple frameworks and the emulator's
standard-library-only Rust core; no package downloads are needed.

From the repository root:

```sh
./macos/build.sh
open out/macos/RNS-E.app
```

The script builds for the current Mac, creates an icon and app bundle, and signs it
ad hoc for local use. The current Rust toolchain's standard library requires macOS
15. Universal builds and distribution signing are not configured.

Open or drag in your own firmware update image, select its variant, then choose
Boot / Reset. Firmware is never bundled. Previous file choices and configuration
are remembered, but every boot starts with fresh device storage.

The main view uses `assets/faceplate.png`. Click the photographed buttons. Drag a
knob around its center or scroll over it to turn; click its left/right rim for one
detent. The knobs visibly rotate. Click the large knob's center to select. The
volume knob's power switch is not emulated by the core; use Ignition in the toolbar.
Open Display and CD Eject expose keys hidden behind the pictured display.

Click the faceplate background for keyboard control: Up/Down turn the selector,
Enter selects, Escape returns, Left/Right select previous/next, 1–4 press the soft
keys. R/M/N/T/I/C/S select Radio/Media/Nav/Telephone/Info/Car/Setup. Command-O opens
firmware, Command-R boots/resets, Command-P pauses, Command-S saves a screenshot.
Short clicks last at least 200 ms of emulated time so they work on slower hosts.

Media controls mount data/navigation images, an audio CD assembled from one or
more WAV tracks (16-bit PCM, 44.1 kHz stereo), and read-only SD images. SD 2 is
available only on the 192. Ejecting a disc keeps the drive empty on reset. Optional
serial and GPS settings apply at boot; GPS is enabled only for firmware whose
getter the core recognizes. A supported GPS fix defaults to San Francisco.

Sound uses AVAudioEngine and follows the firmware's source and volume settings.
The screen is placed in the faceplate's original cutout, matching the browser
frontend's geometry. Screenshot export saves the firmware display alone. The log
panel shows EUC-JP firmware output and retains a bounded tail. Slow emulation is
reported in the status line; controls stay responsive.

## Verification

```sh
cargo test --release
cargo test --release --manifest-path macos/bridge/Cargo.toml
./macos/check.sh /absolute/path/to/firmware.iso
```

The check script builds a native C-bridge runner and boots the first supported
variant of each hardware generation present on the image. It exercises faceplate
input, frame/audio/log transfer, ignition, eject, stop, and fresh reset, saving
screenshots under `out/macos/`. Firmware-backed tests in the root crate require
`RNSE_DISC`; without it they return early. Audio playback can be checked with:

```sh
RNSE_DISC=/absolute/path/to/firmware.iso cargo test --release --test disc cd_audio_plays
```

All C bridge calls for a session must be serialized. The app owns sessions on one
background queue and copies borrowed output buffers before returning to AppKit.
Rust errors and panics become user-visible errors; reset creates a fresh machine.
