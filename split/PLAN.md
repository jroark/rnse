# Splitting rnse into several repositories, and making them public

This plan is for an agent that can create GitHub repositories and push
to them. Work through it one phase at a time. Each phase ends with
something verified and pushed, and the repositories work at every step
in between. Record progress in the status table below and commit it to
rnse.

## Decisions (the user, 2026-11-18)

- **Everything goes public**: rnse and every repository split from it.
- **History**: rewritten once before the split (phase B): the two
  personal Gmail addresses become `45201+jroark@users.noreply.github.com`,
  and the `Claude-Session:` lines are stripped from commit messages. The
  result is pushed as a **new** public `jroark/rnse`; the current private
  repository is renamed to an archive first. (A force-push in place would
  leave the old PRs' `refs/pull/*` with the original emails, visible once
  public.)
- **Licences**: MIT for rnse, rnse-sdk and rnse-sdbridge; GPL-2.0 for
  rnse-ports and rnse-linux (they carry GPLv2 code); CERN-OHL-P for
  rnse-hardware. Write the standard texts, with the user as copyright
  holder; third-party code keeps its own licence files.
- **Hardware**: one `rnse-hardware` repository for every KiCad board
  (`nes-can`, `ps2-can`, `sdbridge-card`), replacing `rnse-nes-can`.
  Generated fabrication ZIPs are not committed; a script makes them.
- **Open PRs**: merge #31 (the SD bridge card) without its generated
  ZIPs; close #5 (the CarPlay-style UI draft, blocked on hardware) and
  port it to rnse-sdk after the split.
- **The wiki stays in rnse** (phase 6 is answered: no rnse-wiki). Once
  the repositories are public, wiki pages may link to them.

## What the audit of 2026-11-18 found

Run on every branch of the private repository (706 paths ever committed):
no firmware, boot ROM, map data, keys or certificates
(`sdbridge/tests/tzcert.py` rewrites a test certificate's dates; it holds
none), no AWS account, bucket or distribution IDs, no unit serial, no
local home paths, no firmware disassembly listings in docs. Largest blob
1.4 MB (`assets/faceplate.png`); pack 19 MB. Repeat the audit on the
rewritten history (phase B) and on each extracted repository (its
`verify.sh`). Saleae captures (`*.sal`) can hold the boot ROM (the
`report` example sends it over CAN), so they are ignored and never
committed.

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
  repository (name, owner `jroark`; visibility is decided: public, but
  only after its verification passed), pushing to rnse's `main`,
  force-pushing or renaming anything, deleting branches, adding CI
  secrets, or changing the AWS role used by the wiki deployment.
  Approval for one repository does not extend to the next.
- **Every public repository** gets secret scanning and push protection,
  protection on `main`, and a README that states the hardware rules
  (above) and that the user's unit, discs and data are their own risk
  and their own property.
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
| `rnse` | the emulator crate and CLI (`src/`, `assets/`), `web/`, `macos/`, CPU fuzzing tools, emulator-only end-to-end tests, the reverse-engineering docs (`hardware*.md`, `boot-rom.md`, `firmware-format.md`, `telephone.md`), the wiki (`wiki/`, `tools/wiki.py`, its workflow) | nothing |
| `rnse-sdk` | `sdk/` as its root: kit, C library, driver/measurement examples, SD loader, `base`, `player`, `modern` + `apps/`, `tools/sh4-docker`, `docs/custom-firmware.md`, `sd-loader.md`, `modern-192.md`; a test crate running the programs on `rnse` | `rnse` (git dev-dependency, pinned rev) |
| `rnse-ports` | Doom, Quake, umac, Windows 3.11 (`doom/`, `quake/`, `umac/`, `win311/` + their `*boot` launchers), NES (nofrendo, `nes`, `nesboot`), the DVD player; their docs and tests | `rnse-sdk` (tag, via `RNSE_SDK`), `rnse` (tests) |
| `rnse-linux` | `linux/`, `linuxboot`, `docs/linux.md`, the `linux_*` tests | `rnse-sdk`, `rnse` |
| `rnse-sdbridge` | `sdbridge/` (RP2040 firmwares, `patches/`, `link.py`; never `work/` or `out/`), `linkbench`, `h264bench` + `third_party/h264bsd`, the Android Auto viewer (`aaview`, `aaviewer`), `tools/linktest.py`, `aa-streams.sh`, `telephone_probe.py`, `tests/linkbench.rs`, `tests/aaviewer.rs`, `tests/data/margin.*`, `docs/sdbridge*.md`, `android-auto.md` | `rnse-sdk`, `rnse` |
| `rnse-hardware` | `hardware/nes-can`, `hardware/ps2-can`, `hardware/sdbridge-card` (KiCad sources, BOMs, scripts; fabrication ZIPs generated, not committed), `tests/nes/protocol.c`, the hardware half of `tools/package-nes.py` | none (the NES protocol header is owned by rnse-ports and pinned here) |

Licences (decided above): MIT for rnse, rnse-sdk and rnse-sdbridge
(h264bsd keeps its own `LICENSE.md`; the TinyUSB patch is MIT; the
pico-sdk and mbedTLS are downloaded, not committed); GPL-2.0 for
rnse-ports (nofrendo vendored under GPLv2; Doom, Quake and umac's GPLv2
parts downloaded; Tiny386's BSD-3 is compatible) and rnse-linux (kernel
patches and drivers); CERN-OHL-P for rnse-hardware.

## Status

The order changed on 2026-11-18, when the user decided to make
everything public. Instead of extracting one repository at a time (with
rnse's `sdk/` a submodule in between), every future repository is first
made one self-contained top-level directory inside rnse (phase C), then
all are extracted from the same commit (phase D) and published in
dependency order (phase E). Phases 0, 1 and 2–5 below still describe how
each extraction is done; phase 1b's submodule is no longer needed.

| Phase | State |
|---|---|
| 0. Prepare `sdk/` in rnse | done (branch `ccr-249b75e2-puy2ij`) |
| A. Land or park outstanding work | mostly done (2026-11-18, PR #35): sound work committed; PR #31 merged with its ZIP replaced by `hardware/sdbridge-card/fab.sh` (kicad-cli; output checked identical); `hardware/ps2-can`, `sdk/examples/cardstart` committed; #5 closed (branch `carplay-ui-192` kept for porting). Left: LICENSE files (copyright holder's name to be chosen by the user); deleting the 22 merged remote branches and the clean, merged worktrees (the user does it; the agent was not permitted) |
| B. Rewrite history; new public-ready rnse | to do: on a fresh clone, `git filter-repo --mailmap` (both Gmail addresses to the noreply one) and `--message-callback` dropping `Claude-Session:` lines, plus `--path-glob 'hardware/**/*.zip' --invert-paths` (PR #31's committed fabrication ZIPs); repeat the audit; then, with the user's approval, rename the private repository (e.g. `rnse-archive`), create a new `jroark/rnse`, push `main`, re-add the Actions variables `AWS_ROLE_ARN`, `AWS_REGION`, `WIKI_BUCKET`, `WIKI_DISTRIBUTION`, and check the wiki deployment (the role trusts the repository by name). **Protect `main` at once** (no force-push, no deletion): GitHub refused it on the private repository on the free plan (403), and the new repository is public. Every worktree and local clone must be re-cloned afterwards |
| C. Every future repository a self-contained directory in rnse | to do (the moves of phase 1b step 1, for all repositories; byte-identical builds before and after) |
| D. Extract all from one commit | to do: generalise `split/rnse-sdk/` (`extract.sh`, `finish.py`, `verify.sh`) to take a repository name; `git filter-repo` is now installed, so run and inspect the passes for real; the user runs each repository's end-to-end tests with their disc, boot ROM and data |
| E. Publish in dependency order | to do: rnse public first (so the others' `git` dependency needs no token), then rnse-sdk `v0.1.0`, then rnse-ports, rnse-linux, rnse-sdbridge, rnse-hardware; CI, scanning and branch protection on each |
| F. Clean up rnse (phase 7) | to do; wiki pages may then link to the public repositories |
| 1. Create rnse-sdk | ready: tooling in `split/rnse-sdk/`, tree step tested; part of D |
| 1b. `sdk/` as a submodule | dropped (see above) |
| 2–5. ports, linux, sdbridge, hardware | part of C and D |
| 6. rnse-wiki | decided: the wiki stays in rnse |

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
  `link_benchmark_from_the_sd_loader`, `android_auto_viewer_from_the_sd_loader`,
  `tests/aaviewer.rs`, `sdbridge/link.py`, the tools and docs named in the
  table. The Android Auto head unit certificate and key stay outside every
  repository (`~/rnse-hu-cert`), as do the bridge's logs (`sdbridge/work/`).
- **rnse-hardware** (phase 5): `hardware/nes-can`, `hardware/ps2-can`,
  `hardware/sdbridge-card` (from PR #31), `tests/nes/protocol.c`. Split
  `tools/package-nes.py` into the hardware half (here) and the software
  half (rnse-ports). The protocol header shared with `nes/protocol.h`
  needs one owner; make it rnse-ports and pin it here. Fabrication ZIPs
  are made by each board's script, not committed (`ps2-can`'s
  `.gitignore` already does this).

## Phase 6: rnse-wiki (decided: not done)

The user chose to keep the wiki in rnse. For the record: the site is built by `wiki/build.sh`, which also
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
