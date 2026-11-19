# Splitting rnse into several repositories

This plan is for an agent that can create GitHub repositories and push
to them. Work through it one phase at a time. Each phase ends with
something verified and pushed, and the repositories work at every step
in between. Record progress in the status table below and commit it to
rnse.

## Ground rules (they apply to every phase)

- **Firmware is never committed**, in any repository: update disc images,
  boot ROM dumps, flash images, extracted modules, card images, WADs,
  PAKs, Mac ROMs and disks, Windows disk images, keys and certificates.
  Every new repository gets rnse's `.gitignore` rules for these, and
  `verify.sh`-style checks that no such file is tracked. History
  rewriting only removes files. It must never bring in a file that
  rnse's history did not already have.
- **The hardware safety rules go everywhere a payload is built.**
  "Mandatory hardware behavior" in rnse's `CLAUDE.md` is copied
  *verbatim* into the `CLAUDE.md` of every repository that builds code
  for the unit: rnse-sdk, rnse-ports, rnse-linux and rnse-sdbridge
  (linkbench runs on the unit). Copy it by script from rnse's file, as
  `rnse-sdk/finish.py` does, not by retyping it. Nothing in any phase
  adds, enables or exposes a firmware-flash path. The flash guard
  (`sdk/src/flash.c`) and the way back (`sdk/src/wayback.c`, "The way
  back" in `docs/custom-firmware.md`) move unchanged.
- **Ask the user before anything outward-facing**: creating each
  repository (name, owner `jroark`, **visibility**: private unless the
  user says otherwise), pushing to rnse's `main`, making a repository
  public, adding CI secrets, or changing the AWS role used by the wiki
  deployment. Approval for one repository does not extend to the next.
- **Keep history** with `git filter-repo` (`pip install
  git-filter-repo`). Run it on fresh clones, never on the user's
  working copy.
- **Standard library only** stays the rule for the emulator crate.
  The other repositories keep their existing build tools: shell, the
  SH-4 GCC, pinned downloads that are never committed.
- **No green without evidence.** The end-to-end tests return early and
  report success without `RNSE_DISC` (and `RNSE_BOOT_ROM`, `RNSE_DOOM`,
  ...). A run without them proves only that things compile and build.
  Say so whenever you report a result. The user owns the disc and boot
  ROM images. Ask them to run the full commands locally if you cannot.

## Target layout

| Repository | Contents | Depends on |
|---|---|---|
| `rnse` | the emulator crate and CLI (`src/`, `assets/`), `web/`, `macos/`, CPU fuzzing tools, emulator-only end-to-end tests, the reverse-engineering docs (`hardware*.md`, `boot-rom.md`, `firmware-format.md`, `telephone.md`) | nothing |
| `rnse-sdk` | `sdk/` as its root: kit, C library, driver/measurement examples, SD loader, `base`, `player`, `modern` + `apps/`, `tools/sh4-docker`, `docs/custom-firmware.md`, `sd-loader.md`, `modern-192.md`; a test crate running the programs on `rnse` | `rnse` (git dev-dependency, pinned rev) |
| `rnse-ports` | Doom, Quake, umac, Windows 3.11 (`doom/`, `quake/`, `umac/`, `win311/` + their `*boot` launchers), NES (nofrendo, `nes`, `nesboot`), the DVD player; their docs and tests | `rnse-sdk` (tag, via `RNSE_SDK`), `rnse` (tests) |
| `rnse-linux` | `linux/`, `linuxboot`, `docs/linux.md`, the `linux_*` tests | `rnse-sdk`, `rnse` |
| `rnse-sdbridge` | `sdbridge/` (RP2040), `linkbench`, `h264bench` + `third_party/h264bsd`, `tools/linktest.py`, `aa-streams.sh`, `telephone_probe.py`, `tests/linkbench.rs`, `tests/data/margin.*`, `docs/sdbridge*.md`, `android-auto.md` | `rnse-sdk`, `rnse` |
| `rnse-nes-can` | `hardware/nes-can` (KiCad), `tests/nes/protocol.c`, the hardware half of `tools/package-nes.py` | none (protocol shared with rnse-ports' NES code) |
| `rnse-wiki` (optional; ask) | `wiki/`, `tools/wiki.py`, `.github/workflows/wiki.yml` | builds `rnse`'s `web/`; its AWS role trusts rnse's `main` only and would have to be changed by the user |

Licensing, for the user to decide (do not write licence texts on your
own): rnse's `Cargo.toml` says MIT but there is no `LICENSE` file.
rnse-sdk inherits MIT. rnse-ports carries GPL code (nofrendo is
vendored under GPLv2; Doom, Quake and umac's GPLv2 parts are downloaded)
next to BSD-3 (Tiny386) and MIT, so it needs a GPL-compatible licence.
Separating it is one of the reasons for the split.

## Status

| Phase | State |
|---|---|
| 0. Prepare `sdk/` in rnse | done (branch `ccr-249b75e2-puy2ij`) |
| 1. Create rnse-sdk | ready: tooling in `split/rnse-sdk/`, tree step tested |
| 1b. Move non-SDK programs out of `sdk/`; `sdk/` becomes a submodule | to do |
| 2. rnse-ports | to do |
| 3. rnse-linux | to do |
| 4. rnse-sdbridge | to do |
| 5. rnse-nes-can | to do |
| 6. rnse-wiki | ask the user whether to do it |
| 7. Clean up rnse | to do |

## Phase 0: prepare `sdk/` in rnse (done)

So that phase 1 is a plain subdirectory extraction:

- The root `Cargo.toml` is a workspace with member `sdk`. `sdk/Cargo.toml`
  is crate `rnse-sdk`, which has tests only and a dev-dependency
  `rnse = { path = ".." }`.
- The 28 tests that run SDK examples which stay in the SDK moved verbatim
  from `tests/disc.rs` to `sdk/tests/emulated.rs`, with copies of the
  helpers they use (`disc`, `sdk_example`, `started_by_stock_software*`,
  `boot_rom`, `rom_192`, `until`, `stock_is_in_flash`) and of
  `tests/data/tones.mp3`. `tests/disc.rs` keeps the emulator-only tests
  and those of Linux, the ports and the SD link (28 of the 56 there were;
  `link_benchmark_from_the_sd_loader`, added since, makes 29).
- `sdk/build.sh 192|193 DIR` builds an example from any directory (a
  path with a slash). The output is byte-identical to the in-tree build.
- `tools/sh4-docker` and `tools/build-sd-loader.sh` moved to `sdk/tools/`.
  `sdk/modern.sh`, `sdk/tools/build-sd-loader.sh` and
  `sdk/tests/check-modern.sh` work from the SDK's directory and find the
  emulator binary through `sdk/tools/rnse.sh` (`$RNSE`, rnse's own
  build when inside rnse, or `rnse` on the PATH). Their outputs moved
  from `out/...` to `sdk/out/...`.
- References in docs, `CLAUDE.md` and the ports' build scripts were
  updated.

Checked: `cargo test --release --workspace` (unit tests pass; the
end-to-end tests were *not* run for real because no disc image was
available). Every remaining example builds for both units
(`modern` for the 192 only, by design).

## Phase 1: create rnse-sdk

Tooling: `split/rnse-sdk/`

- `include.txt`: the paths taken, with history. `sdk/` becomes the root.
  `tools/sh4-docker/` and `tools/build-sd-loader.sh` keep their history
  from before the move. Three docs come along.
- `exclude.txt`: what stays in rnse for later phases (the ports'
  launchers, NES, DVD, `linuxboot`, `linkbench`, `h264bench`,
  `third_party/nofrendo`, `third_party/h264bsd`).
- `extract.sh SOURCE DEST`: fresh clone, three `git filter-repo` passes
  (include, rename `sdk/:`, invert-exclude), then `finish.py`, one
  commit. Nothing is pushed.
- `finish.py`: rewrites root-relative paths (`sdk/build.sh` becomes
  `./build.sh`, `sdk/src/...` becomes `src/...`, `$PWD/sdk/tools/...`
  becomes `$PWD/tools/...`). Markdown links to docs that stay in rnse
  become links to `https://github.com/jroark/rnse/blob/main/docs/...`.
  It pins `rnse` in `Cargo.toml` to the source commit (`git`, `rev`),
  adds rnse's release profile, and copies `overlay/` in (`CLAUDE.md`
  with the hardware rules and the firmware rule pasted verbatim,
  `README.md`, `.gitignore`, CI, `tools/build-all.sh`,
  `tools/examples.txt`).
- `verify.sh DEST`: structure and rule checks, no `sdk/` prefixes left,
  excluded paths absent, no firmware-like files tracked, every example
  built for every unit in `examples.txt`, host tests, `cargo test
  --release`.
- `examples.txt`: the examples and units.

What has been tested (phase 0): `finish.py --prune` and `verify.sh` on a
plain copy of the tree (`sdk/` plus the three docs) with
`--rnse-path`. The filter-repo passes in `extract.sh` have **not** been
run (git-filter-repo could not be installed in the session that wrote
this). Check their result carefully the first time.

Steps:

1. Ask the user to merge the phase 0 branch into rnse's `main`, or to
   approve pushing it. Then clone rnse at `main` (the commit that
   `Cargo.toml` will pin must be on GitHub).
2. Ask the user to approve creating `jroark/rnse-sdk` and its
   visibility.
3. `sh split/rnse-sdk/extract.sh /path/to/rnse-clone /path/to/rnse-sdk`
4. Inspect the result:
   - `git log --stat | head -200`: the history is the SDK's.
   - `git log --all --format= --name-only | sort -u`: only SDK paths,
     no `doom/`, `linux/`, `src/*.rs`, ports.
   - The first commits of `tools/sh4-docker/` predate the move.
   - Paths in `exclude.txt` are absent from every commit
     (`git log --all -- examples/doomboot` prints nothing).
5. `sh split/rnse-sdk/verify.sh /path/to/rnse-sdk`, with
   `PATH=$PWD/tools/sh4-docker:$PATH` or Debian's
   `gcc-sh4-linux-gnu` installed. `cargo test` fetches rnse from GitHub:
   if rnse is private, git needs credentials
   (`CARGO_NET_GIT_FETCH_WITH_CLI=true` with a token for the
   machine/CI). Then commit `Cargo.lock`.
6. Ask the user to run the real end-to-end run, which needs their disc
   and boot ROM:
   `PATH=$PWD/tools/sh4-docker:$PATH RNSE_DISC=disc.iso RNSE_BOOT_ROM=rom.bin cargo test --release`
   in rnse-sdk. Compare with the same tests in rnse at the source
   commit: `cargo test --release -p rnse-sdk`. They must agree.
7. Push `main`, tag `v0.1.0`. Check the CI run. If rnse is private, the
   CI's `cargo test` needs a read token as a secret: ask the user
   before adding one, or drop that CI step until rnse is public.
8. Update the status table above.

## Phase 1b: make rnse use rnse-sdk

Until the later phases are done, rnse still builds the ports and Linux
on the SDK. From phase 1 on, rnse-sdk is the only place SDK changes are
made, so rnse's `sdk/` must stop being a second copy.

1. In rnse, move the programs listed in `exclude.txt` out of `sdk/` to
   the directories of the repositories they will go to, with `git mv`:
   - `doom/boot/` (from `sdk/examples/doomboot`)
   - `quake/boot/`, `umac/boot/`, `win311/boot/`
   - `nes/` (`examples/nes`, `examples/nesboot`, `third_party/nofrendo`,
     `nes-build.sh`)
   - `dvd/` (`examples/dvd`, `tests/check-dvd.sh`, `tests/dvd_host.c`)
   - `linux/boot/`
   - `sdbridge/linkbench/`, `sdbridge/h264bench/` (with
     `third_party/h264bsd`; `build.sh` looks for `libs` in
     `third_party/` of the SDK, so either keep h264bsd in rnse-sdk's
     `third_party/` or let `build.sh` accept a `libs` entry that is a
     path. Decide, and test that the output is byte-identical as phase 0
     did.)

   Then update their build scripts, docs and tests:
   - `sdk/build.sh 192 ../doom/boot`
   - `-I$sdk/examples/doomboot` becomes `-I$root/doom/boot`
   - `sdk_example(..., "doomboot")` in `tests/disc.rs` needs a directory
     argument
   - `tools/package-nes.py` and the docs
   
   Build each one before and after and compare the binaries.
2. Take `sdk` out of the workspace, delete `sdk/`, and add rnse-sdk as a
   git submodule at `sdk/`, pinned to `v0.1.0`. The ports and Linux keep
   working through `$root/sdk`. The SDK's tests now run in rnse-sdk.
3. In rnse's `CLAUDE.md` and docs, point SDK matters at rnse-sdk, and
   replace the three moved docs with one-line pointers. Keep
   `src/bin/rnse.rs`'s usage text (it names `docs/custom-firmware.md`)
   accurate by naming the repository.
4. Add a downstream check to rnse's CI, since 28 tests that exercise the
   emulator's loader paths now live in rnse-sdk. It checks out rnse-sdk
   at its latest tag and runs
   `cargo test --release --config 'patch."https://github.com/jroark/rnse".rnse.path="'$PWD'"'`
   there.

## Phases 2–5: the other repositories

Each follows the pattern of phases 0 and 1. First make the directory
self-contained inside rnse: tests in its own crate, scripts that find
the SDK through `RNSE_SDK` (default `$root/sdk` while inside rnse) and
the emulator through `$RNSE`, docs referenced by repository. Then write
`split/<repo>/{include,exclude}.txt`, `extract.sh`, `finish.py` (reuse
rnse-sdk's, parameterised) and `verify.sh`. Extract, verify, push, tag.
Finally delete the paths from rnse.

- **rnse-ports** (phase 2): `doom/`, `quake/`, `umac/`, `win311/`,
  `nes/`, `dvd/`, their docs (`doom.md`, `quake.md`, `umac.md`,
  `win311.md`, `nes.md`, `dvd.md`) and the tests
  `win311_runs_from_an_sd_card`, `doom_runs_from_an_sd_card`,
  `mac_runs_from_an_sd_card`, `dvd_player_from_the_sd_loader`,
  `quake_runs_from_an_sd_card` and `nes_*` from `tests/disc.rs`, with
  `tests/dvd.rs`. Keep with history the files' old paths under
  `sdk/examples/...` and `sdk/third_party/nofrendo` (`old==>new` lines
  in `include.txt`). DVD: keep the rule that no CSS descrambler is
  shipped, only the empty weak hooks of `scramble.c`. The licence must
  be settled first (above). The user's own ROMs, disks and WADs are
  never committed; the build scripts download only what they pin today.
- **rnse-linux** (phase 3): `linux/`, `linux/boot`, `docs/linux.md`, the
  `linux_*` tests and `linux_firmware_disc`. Keep the rule that nothing
  in Linux writes the flash; the guard is the only way out from flash.
- **rnse-sdbridge** (phase 4): `sdbridge/` (never `sdbridge/work/` or
  `sdbridge/out/`; the pico-sdk is cloned by `build.sh`; certificates and
  keys stay outside), `linkbench`, `h264bench`, `tests/linkbench.rs`,
  `tests/data/margin.*`, `sdk_decodes_h264_from_a_card`,
  `link_benchmark_from_the_sd_loader`, `sdbridge/link.py`, the tools and
  docs named in the table.
- **rnse-nes-can** (phase 5): `hardware/nes-can`, `tests/nes/protocol.c`.
  Split `tools/package-nes.py` into the hardware half (here) and the
  software half (rnse-ports). The protocol header shared with
  `nes/protocol.h` needs one owner; make it rnse-ports and pin it here.

## Phase 6: rnse-wiki (optional)

Ask the user first. The site is built by `wiki/build.sh`, which also
builds rnse's `web/` (the emulator in WebAssembly). Its deployment signs
in to AWS through an IAM role that trusts rnse's `main` branch only.
Moving it means the user changes that trust policy and the repository
variables `AWS_ROLE_ARN`, `AWS_REGION`, `WIKI_BUCKET` and
`WIKI_DISTRIBUTION`. Leaving it in rnse is a reasonable answer.

## Phase 7: clean up rnse

When phases 2–5 are done:

- Remove the `sdk/` submodule and the moved directories.
- Leave a short "Related repositories" section in `README.md` and
  `CLAUDE.md`, and update the architecture section of `CLAUDE.md` to
  what remains.
- Keep the downstream CI checks of phase 1b for every repository whose
  tests exercise the emulator.
- Delete `split/` once the status table says everything is done, or keep
  it as a record if the user prefers.
