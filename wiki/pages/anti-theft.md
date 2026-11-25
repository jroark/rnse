title: Anti-theft PIN
order: 45
summary: How the 192 and 193 store and check their four-digit anti-theft PIN, why swapping a 193's DVD drive locks it out, and how it was verified on real hardware.
---
After a loss of power, an RNS-E asks for a four-digit anti-theft PIN before it will
play or navigate. On the 192 that PIN sits in NOR flash in plain text. On the 193 it
is encrypted with AES-128 using a key read at boot from the DVD drive — which is why
swapping the drive on a 193 breaks even the unit's own factory PIN.

Everything below was worked out from the stock firmware (`V2220317` for the 192;
`V5751050` and `V57510AA` for the 193) and confirmed on real bench units.

## How the lock works

The anti-theft state machine lives in the `THF` module. After power is restored, it
walks through three checks before asking the user for a code:

1. **Factory grace counter.** On a fresh or blank unit, `THF` initializes a 50-boot
   grace counter and authorizes the unit immediately, decrementing the counter at
   each start so the factory can test the radio without a PIN.
2. **Instrument cluster handshake.** Once the grace counter reaches zero, the unit
   sends a challenge to the car's instrument cluster (Kombi) on CAN ID `0x361`. If
   the cluster answers with a matching response for the stored car, the unit unlocks
   automatically without showing the PIN screen.
3. **Manual PIN entry.** If no cluster answers (for example on a bench, or after
   moving the unit to another car), two 3-second timeouts expire and the unit opens
   the PIN screen: a dial of digits `0`–`9` turned with the main knob, "Remaining
   attempts: 2", and the unit's 14-character serial number (`AUZ...`) along the
   bottom. Two wrong entries put the unit into a one-hour lockout.

## The 192: plaintext in flash

On the 192, the factory parameter block at flash `0x00ffe000` holds both the serial
number and the PIN in plain ASCII:

| Flash address | Length | Contents |
|---|---|---|
| `0x00ffe01f` | 14 bytes | Unit serial number (`AUZ...`) |
| `0x00ffe031` | 4 bytes | Four-digit anti-theft PIN in ASCII (`'0'`–`'9'`) |

In `V2220317`, the check at `0x8019c180` compares the four entered digits directly
against `0x00ffe031` with `memcmp` (`0x801e0320`). If any of the four bytes at
`0x00ffe031..0x00ffe035` is not an ASCII digit (such as `0xff` on blank flash), the
check at `0x8019c192` accepts any entry.

The `THF` working block (`THF_FLASH`, 36 bytes) lives in battery-backed SRAM at
`0xb0002c14` (persisted as block #6 via `0x801320ea`), with `wrongpin_cnt` at
`+5` (`0xb0002c19`) and the live lock state in OS scratch RAM at `0x8c0ca010`
(`2` = PIN required, `3` = authorized). Because the 192 does not tie its PIN to the
DVD drive, a 192's drive can be swapped freely without affecting the PIN.

## The 193: AES-128 paired to the DVD drive

The 193 keeps its factory parameter block at flash `0x03ff4000` (inside the last
128 KiB sector, `0x03fe0000..0x03ffffff`):

| Flash address | Length | Contents |
|---|---|---|
| `0x03ff401f` | 14 bytes | Unit serial number (`AUZ...`) |
| `0x03ff403a` | 16 bytes | AES-128-ECB ciphertext of `[PIN_u16_be][14-char serial]` |

The 16-byte plaintext is the PIN as a big-endian 16-bit integer (`0..9999`) followed
by the unit's 14-character ASCII serial number:

```text
ciphertext = AES128_encrypt([PIN_u16_be][14-char serial], key)
```

The AES-128 **key** is not in flash at all. At boot, `0x8058db8a` sends a standard
SCSI `INQUIRY` (`12 00 00 00 60 00`, 96 bytes) to the DVD drive over ATAPI and
copies the 16-byte product-identification field (bytes 16–31 of the reply) into DRAM
at `0x8e9de900`.

On a real 193 drive, bytes 8–15 of that product field carry a **per-drive four-digit
code**:

| Drive | `INQUIRY` bytes 16–31 (16-byte AES key) |
|---|---|
| 193 drive (example from bench unit) | `"DV33M01B 3154   "` |
| 193 blank / error sentinel (`0x8058c8f2`) | `"DV33M01B EEEE   "` |
| 192 drive | `"DV33M01B/       "` |

At boot (`0x80613db0`) and on PIN entry (`0x80613b32`), `0x8058cc7e` decrypts
`0x03ff403a` with `0x80681196` (standard AES-128) using the 16 bytes at `0x8e9de900`,
verifies that `plaintext[2..16]` matches the flash serial at `0x03ff401f` and that
`PIN < 10000`, and formats the four ASCII digits for comparison at `0x80613bd2`.
(`0x8058ce0e` is the factory counterpart that encrypts the record with `0x80680dbc`.)

Persistent `THF` state (`THF_FLASH`, 40 bytes) is stored as block #6 of the 93-block
ping-pong flash store at `0x033e0000` / `0x03400000` (each bank 128 KiB: 93 × 256-byte
pages, 93 × 8-byte directory entries at `+0x5d00`, magic `01..08` at `+0x5fe8`,
wrapping 32-bit sum at `+0x1fff8`, sequence number at `+0x1fffc`) and mirrored at
boot into battery-backed SRAM at `0xb8002c14`:

| Offset | SRAM address | Contents |
|---|---|---|
| `+0x00` | `0xb8002c14` | Magic `0x12345678` |
| `+0x04` | `0xb8002c18` | Lockout flag (`1` = 1-hour lockout active) |
| `+0x06` | `0xb8002c1a` | `wrongpin_cnt` (`0` = 2 attempts left, `1` = 1 left, `2` = lockout) |
| `+0x07` | `0xb8002c1b` | Car VIN (17 bytes) |
| `+0x18` | `0xb8002c2c` | Factory grace counter (starts at `50`) |
| `+0x1e` | `0xb8002c32` | Persisted state (`1` = lockout, `2` = PIN required, `3` = authorized) |
| `+0x20` | `0xb8002c34` | Kombi CAN challenge PRNG accumulator |
| `+0x24` | `0xb8002c38` | Inverted magic `0xedcba987` |

## Why swapping a 193's DVD drive breaks the PIN

Because `0x03ff403a` is encrypted under the original drive's `"DV33M01B XXXX   "`
string, installing a different 193 drive (with a different four-digit code) or a 192
drive (`"DV33M01B/       "`) causes `0x8058cc7e` to decrypt garbage:

- The 14-byte serial comparison fails and `0x8058cc7e` returns `0xffffffff`.
- The boot check sets `0x8e9f3d78 = 0`, which appends a four-hex-digit diagnostic
  suffix after the serial on the PIN screen (for example `AUZBZ7ND801051 5F1D`).
- The unit rejects **every** PIN — including its real factory PIN — until the
  original drive is reinstalled or the firmware is patched.

This was confirmed directly on a physical 193 (`AUZBZ7ND801051`): with a 192 drive
installed (`"DV33M01B/       "`), the unit showed the diagnostic suffix and refused
its code; once its original `"DV33M01B 3154   "` drive was put back, `0x03ff403a`
decrypted to `[0x079c]["AUZBZ7ND801051"]` (`PIN = 1948`) and unlocked immediately.

## Drive-independent recovery and replacement drives

Because the per-drive code is only four decimal digits (`0000`–`9999`), the entire
keyspace is 10,000 keys (`"DV33M01B 0000   "` through `"DV33M01B 9999   "`), and the
14-byte serial inside the decrypted block is self-validating (a false match has
probability roughly 2⁻¹¹²). Given a unit's serial (`0x03ff401f`) and ciphertext
(`0x03ff403a`), sweeping those 10,000 keys recovers both the original drive ID and
the real factory PIN even if the original drive is dead or missing.

On the 193's 400 MHz SH-4A, a small fallback hook in `OSM`'s `0x8058cc7e` failure
path can perform that sweep automatically when the attached drive's key does not
match, cache the recovered 16-byte key into DRAM at `0x8e9de900` so subsequent calls
during PIN entry take the single-iteration stock path, and let any 192 or 193
replacement drive work permanently with the unit's real factory PIN without ever
touching the `0x03fe0000` factory parameter sector. `tools/patch_osm_drive_swap.py`
applies that patch to a stock `V575` update disc (`V5751050` or `V57510AA`),
repacks it with `rnse repack` so the unmodified stock `LDM` installs it, and bumps
`575INDEX.TXT`'s `SOFIDX`.

## In the emulator

The emulator models the real firmware's PIN path rather than bypassing it:

- `--serial AUZ...` writes the 14-character serial to `0x00ffe01f` (192) or
  `0x03ff401f` (193).
- `--pin NNNN` writes the four plaintext ASCII digits to `0x00ffe031` on the 192, or
  AES-128 encrypts `[PIN_u16_be][serial]` under the emulated drive's `INQUIRY` key
  into `0x03ff403a` on the 193.
- `--pin-lock` (193, verified on `V5751050` and `V57510AA`) seeds block #6 of the
  `0x033e0000` / `0x03400000` flash store with `grace = 0, state = 2` so the
  unmodified stock software runs its own AES-128 decode, shows the PIN screen,
  rejects wrong entries, and unlocks on the right code.

## Related

- [The 192's hardware](/hardware-192/): flash, SRAM and the Alpine DVD drive on the 192
- [The 193's hardware](/hardware-193/): the SH-Navi board, flash layout and bench findings
- [Firmware and update discs](/firmware/): how `OSM` and `LDM` modules are structured and flashed
- [The emulator](/the-emulator/): running the stock firmware with `--pin` and `--pin-lock`
