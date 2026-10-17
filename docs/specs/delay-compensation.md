# Plug-in delay compensation

- **Status:** partial
- **Work packages:** WP-2.11
- **Parity-matrix row:** Mixing: Delay compensation
- **Checked against:** Live 12.4.3, on 14 October 2026

## The behaviour

A device or plug-in can delay its input by a number of frames (look-ahead,
linear-phase filtering, oversampling, a hosted plug-in's own reported latency).
Without compensation, a signal through a high-latency device reaches the Main
output later than a signal that does not, so tracks that should line up drift
apart. Beatlayer compensates for this the way Live does: it holds every path
back to the longest one so everything reaches Main aligned, offsets a track's
clips and a device's sidechain source to match, and offsets a parameter change
meant for a song position by the same latency so automation acts on the audio it
is meant for.

This spec covers the latency model and the compensation the engine applies. It
leaves the latency a particular device reports to that device's own spec
(`device-chains.md` for the native framework, `clap-hosting.md` for plug-ins),
and automation editing to `automation.md`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, "Mixing", delay compensation | Live delays every track so that signals through high-latency devices stay aligned at the Main output; automation and sidechains are compensated with the audio; compensation is recomputed when the device set changes. |
| S2 | Observation | Live 12.4.3 on macOS, 14 October 2026 | A device disabled from its title bar reports no latency and the chain realigns; re-enabling restores it. |

## Rules

- **R1.** Each device and plug-in reports the frames it delays its input by; a switched-off device reports none, but its latency is remembered so it returns when the device is switched on again. (S1, S2)
- **R2.** Every path into a summing point — a track's inputs, a track's output into Main, into a group or into another track's input, and a send into a return — is held back to the latency of the longest path into that point, so all paths reach Main aligned. (S1)
- **R3.** A track's clips are held back to line up with that track's inputs. (S1)
- **R4.** A device's sidechain source is held back to line up with the device's own input at the point in the chain where the device sits. (S1)
- **R5.** A parameter change sent for a song frame is held back by its node's latency, so automation acts on the audio it is meant for; a change sent to take effect now, such as a person's control move, is not delayed. (S1)
- **R6.** Compensation is recomputed when the graph or a device's latency changes. The delay lines are preallocated and processed in place, so the audio thread allocates nothing. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R2 | `crates/engine/tests/pdc.rs`, `two_tracks_one_through_a_latency_device_null_at_main`, `the_same_through_a_group_lines_up`, `the_same_through_a_send_lines_up` | golden render |
| R1 | `crates/engine/tests/pdc.rs`, `switching_the_latency_device_off_and_on_keeps_its_compensation` | golden render |
| R3 | `crates/engine/tests/pdc.rs` (clips held back in every aligned case) | golden render |
| R4 | `crates/engine/tests/pdc.rs`, `a_sidechain_keyed_from_another_track_lines_up` | golden render |
| R5 | `crates/engine/tests/pdc.rs`, `automation_lands_on_the_compensated_sample` | golden render |
| R6 | `crates/engine/tests/pdc.rs`, `a_latency_change_recompiles_and_the_compensation_follows`; every block in `crates/engine/tests/pdc.rs` runs under the allocation trap | golden render |

## Not built yet

- A plug-in's latency counts from the compile that follows its activation, so it can read as 0 on the first compile; native devices report at creation and compensate fully. A VST3 plug-in that changes its latency asks for a restart, the host reports the new latency to the shell, which tells the engine, and the compiler compiles again with the device's node kept and the new latency in its slot. CLAP latency changes are not reported to the engine yet.
- A latency change that forces a recompile starts the delay lines empty, so it can click; the old and new delays are not crossfaded.
- R5's offset applies to parameter changes sent for a song frame; a scheduled command's `Action::Param` fires at the song position directly and is not shifted yet.
