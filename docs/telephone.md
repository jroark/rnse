# Telephone emulation investigation

Telephone emulation is not working in the firmware yet. The host-side
state machine in `src/dev/telephone.rs` supports one simulated phone,
three fictional contacts, incoming and outgoing silent calls, and timed
actions. It is not connected to `Machine`, the CLI, or the browser.
It emits no speculative CAN messages and changes no firmware memory.

## Reproducing the investigation

With locally extracted OSM modules:

```
python3 tools/telephone_probe.py out/V222/OSM.BIN out/V575/OSM.BIN
target/release/rnse boot RNSE_NA_FW_0320_0350.iso --variant V222 \
  --ignition --cycles 4200000000 --key TEL@30000 --trace-can \
  --screenshot out/tel-before-222.png > out/tel-before-222.log 2>&1
```

The second command was run against V2220371. Its TEL screen says
“Telephone is not installed.” The run continued to its cycle budget.
The same check on V57510AA (`--variant V575 --cycles 6000000000
--key TEL@45000`) also reaches the telephone screen with the same
message, then runs to its cycle budget. Extracted modules, disassemblies, traces and
screenshots belong under ignored `out/`, never in version control.

## Confirmed static evidence

Addresses below are cached SH-4 addresses, specific to these builds.

| Evidence | V2220371 | V57510AA |
|---|---|---|
| `TEL_TSK` name | `80034858` | `80439238` |
| Task-table pointer to that name | `80033d6c` | `80438758` |
| `TELCAN_TSK` name | `800348bc` | `8043929c` |
| Task-table pointer to that name | `80033f6c` | `80438958` |
| TELCAN entry in table | zero | `804dac70` |
| `^CTEL=1` literal pointer | `8010af94` | `805663c4` |

The nonzero V575 TELCAN entry jumps to `806278e4`. Both builds contain
telephone settings/phonebook storage paths and `^CTEL` command strings.
These strings alone do not establish whether AT commands travel on a
serial link or through a higher-level CAN transport.

In V222, `801163ec` builds receive descriptors for `6c5`, `6c4` and
`665`, then calls a transmit setup routine with `461` and argument 30.
`801161d4` fills those receive descriptors. The `665` descriptor points
to callback `80115a3c`, which copies received data into a 50-entry ring
with ten bytes per entry, then signals event bit `40` for task `21`.
The string `[TEL]: Ring Over` belongs to this buffer-overflow path; it
is not evidence of an incoming-call notification.

`80116338` reads a network-state byte via `801c84d8` and tests bit
`10`. That getter copies RAM byte `8c176712`. The two branches select
different telephone initialization paths. This byte belongs to CAN
network management, rather than factory telephone coding; its full
meaning is not established. Do not force it to make TEL appear online.

## CAN prerequisites now implemented

The 192's three shared receive filters were missing. In particular,
`665` never reached the telephone ring buffer because mailbox 3's
acceptance mask was zero. With the boot receive filters installed,
breakpoints confirm `80115a3c`, `80115b1c` and `80116630` execute.

The 192's service 9 and 193's service `0b` now hand over transmit enable
and initial network state through the firmware-provided block lists.
Both also inherit a running CAN timer. The 192 uses TMU2; the 193 uses
the separate channel at `ffd81000`, interrupt `480`. See
[boot-rom.md](boot-rom.md) for layouts, evidence and the timer-period
assumption. Both builds now send cyclic `661`/`604` frames and attempt
transport channel opens on `6c0`.

On V2220371, periodic `665` frames with seven zero bytes cause requests
on `6c4`: `a0 0f 8a ff 4a ff`. Supplying `6c5:a10f8aff4aff` stops
those channel-open retries. This confirms a transport response, not a
working telephone application handshake. With the tested payloads,
both TEL screens still report that the telephone is not installed.
The V575 telephone path has not yet been made to open `6c4`.

## Remaining protocol prerequisite

Before exposing `--phone` or browser controls, establish how the real
firmware discovers the telephone module, the source of its configuration,
and the transport/application handshake for commands and phonebook data.
The receive identifiers above are insufficient to construct a valid
module response. V575 has a separate active TELCAN task, so compatibility
cannot be assumed from the V222 routines.

Next, trace V222's queued receive consumer at `80115b1c` and transport
setup at `80116630` (calls `801c882c` and `801c89d0`), and V575's
TELCAN dispatcher at `80627984`. Validate exchanges on both generations
before connecting the host state model. Do not call the overall feature
complete until contacts and call actions work on the firmware screens.

## Validation of this foundation

The initial foundation's `cargo test --release` passed all 48 unit tests, including five telephone
state/scheduling tests. With `RNSE_DISC=RNSE_NA_FW_0320_0350.iso`, the
disc suite also passes: eight firmware checks run, covering both hardware
generations; the navigation check returns early because `RNSE_NAV_DISC`
was not supplied. These results establish existing boot/UI behavior and
host-side state transitions, not telephone protocol compatibility.

The CAN continuation adds a unit test for independent unit-1 timer
registers, periodic interrupt `480`, acknowledgement and masking. The
disc-backed `can_timer_drives_status_and_transport` check runs 30 emulated
seconds on both generations, requiring cyclic status frames and transport
requests; on the 192 it also requires a telephone channel-open request.
It passes on both local discs (V2220371/V57510AA and
V2220317/V5751020). All 49 unit tests pass, as do the main disc's nine
firmware checks; the tenth disc test, navigation, returns early without
`RNSE_NAV_DISC`.
