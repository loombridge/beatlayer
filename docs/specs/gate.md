# The gate

- **Status:** implemented (every rule has a passing test)
- **Work packages:** WP-5.21
- **Parity-matrix row:** Audio effects: Gate
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.20 "Gate", and the release notes of 12.2, read 18 October 2026; the controls' ranges and defaults, the detector and the envelope's shape are Beatlayer's own

## The behaviour

Gate is Beatlayer's gate: it lets a signal through while the level of its detector is above a threshold and turns it down to a floor while it is not. Threshold sets the level that opens it, and Return the gap below the threshold at which it starts to close, so a level near the threshold doesn't make it chatter. Attack, Hold and Release shape how it opens, how long it stays open after the level falls, and how it closes; Floor sets how far a closed gate turns the signal down, down to silence; Flip turns it round so it passes only what is below the threshold; Lookahead delays the signal by 0, 1 or 10 ms so the gate has opened when a peak arrives. Its sidechain section lets another track's signal drive the detector, through an EQ that hears only a band of it, or lets the user listen to that signal.

The detector hears what the engine hands the device on its sidechain (the [routing spec](routing.md), R22 to R31: another track through the sidechain's gain and mix, or the device's own input). The framework is the [device framework spec](device-framework.md), and the shared parts of Compressor's SC filter are the [compressor spec](compressor.md)'s (R13).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.20 "Gate" | Threshold sets the sensitivity. Return (hysteresis) is the gap between the level that opens the gate and the one that closes it; a larger gap stops it opening and closing rapidly near the threshold. Attack is the time from closed to open; Hold is how long the gate stays open after the signal falls below the threshold; Release is how long it takes to close after the hold. Floor is the attenuation of a closed gate: −∞ dB mutes it, 0 dB leaves a closed gate with no effect, values between attenuate partly. Flip makes the gate pass the signal only while it is below the threshold. Lookahead has three settings, 0, 1 and 10 ms, and delays the input so the gate can act before the sound arrives. The display shows the input level and the output level |
| S2 | Manual | Same, section 29.20, and the release notes of Live 12.2 | The sidechain section is shown or hidden with a toggle in the title bar (at the left from 12.2). With the sidechain on, another track's signal triggers the gate instead of the gated signal. Gain changes the trigger level only, and the source is never heard. Dry/Wet at 100 % means the sidechain alone triggers the gate, at 0 % it is bypassed. An EQ in the sidechain makes only a chosen band of the signal trigger the gate. Listen plays only the sidechain input in place of the gate's output |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter can be automated and modulated |

## Rules

- **R1.** Beatlayer ships Gate, an audio-effect device type with a sidechain input, of its own layout: every parameter has a descriptor and a place in the layout, and the parameters keep their values through a saved set and through the agent API. Threshold runs from −80 dB to 0 dB (−30 dB at first), Return from 0 to 24 dB (6 dB), Attack from 0.01 to 50 ms (1 ms), Hold from 1 ms to 2 s (20 ms), Release from 1 ms to 5 s (100 ms), all Beatlayer's own; Floor is a gain from −∞ dB to 0 dB, −∞ at first; Look offers 0 ms, 1 ms and 10 ms; Flip and SC EQ are switches, off at first. (S1, S3)
- **R2.** The detector's level is the larger of the two channels' sample magnitudes, after the SC EQ when it is on. A level at or above Threshold opens the gate; a level below Threshold less Return for the Hold time closes it; a level between the two leaves it as it is, and a level in that band while the gate is open starts the Hold time again. A stepped tone opens the gate within 0.1 dB of Threshold and closes it within 0.1 dB of Threshold less Return. (S1)
- **R3.** The gate's opening moves from 0 to 1 in a straight line over Attack and from 1 to 0 over Release, and the gain is Floor plus the opening's share of the rest, so Attack and Release are the times the gate takes to open and to close, each within 5 %. The Hold time, within 5 %, passes between the level falling below the closing threshold and the opening beginning to fall. (S1)
- **R4.** Floor is the gain of a closed gate: a closed gate turns the signal down by Floor within 0.1 dB, at 0 dB it has no effect, and at −∞ dB it is silent. (S1, S3)
- **R5.** With Flip on, the gate is shut while its detector says open and open while it says closed, so it passes only the signal below the threshold; Attack, Hold and Release act as before. (S1)
- **R6.** Look delays the output by 0, 1 or 10 ms (0, 48 or 480 frames at 48 kHz) while the detector hears the input undelayed, so the gate has begun to open when a delayed peak comes out. The device reports the delay as its latency from the moment it is built, for delay compensation. (S1)
- **R7.** With the sidechain on, the detector hears the key the engine hands the device in place of the track's own signal, so a key from another track opens the gate on an input that is below the threshold; with the SC EQ on, the detector hears the key through a filter (Beatlayer's types Low Pass, Band Pass and High Pass, Freq and Res), so only a band of it opens the gate. The key itself is never heard unless Listen is on, when the device's output is the key (routing R24). (S2)
- **R8.** The output is the same, bit for bit, at any block size: the continuous parameters the detector works from are read every 16 frames counted from the device's start, and the stepped ones and Floor on every frame. (S1)
- **R9.** The display (device framework R36) holds the readings Threshold and Return from the parameters and, with an engine publishing, Gain Reduction (the gain now in dB, 0 for an open gate), Input and Output (the peaks since the last block in dB). The gain reduction is readable headlessly, with `device.display`, for the meters that follow it. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/gate.rs`, `the_gate_is_described_and_declares_a_sidechain`; `crates/shell/tests/eqs_and_dynamics.rs`, `a_set_with_the_four_devices_reopens_with_every_parameter`; `crates/cli/tests/agent.rs`, `an_agent_adds_the_eqs_and_dynamics_sets_them_by_id_and_keys_them_from_another_track`; `rtsan/tests/devices.rs`, `the_gate_passes_under_rtsan` | unit, MCP script, RTSan |
| R2 | `crates/devices/tests/gate.rs`, `a_stepped_tone_opens_it_at_the_threshold_and_closes_it_return_below` | unit, real-time trap |
| R3 | `crates/devices/tests/gate.rs`, `attack_hold_and_release_are_the_times_they_say` | unit, real-time trap |
| R4 | `crates/devices/tests/gate.rs`, `floor_sets_a_closed_gates_gain_and_minus_infinity_is_silent` | unit, real-time trap |
| R5 | `crates/devices/tests/gate.rs`, `flip_passes_only_what_is_below_the_threshold` | unit, real-time trap |
| R6 | `crates/devices/tests/gate.rs`, `lookahead_delays_the_output_and_reports_the_latency` | unit, real-time trap |
| R7 | `crates/devices/tests/gate.rs`, `a_key_opens_it_on_a_quiet_input_and_the_sc_eq_hears_only_its_band`; `crates/cli/tests/agent.rs`, `an_agent_adds_the_eqs_and_dynamics_sets_them_by_id_and_keys_them_from_another_track` (a key from another track opens it, and Listen plays the key) | unit, MCP script, real-time trap |
| R8 | `crates/devices/tests/gate.rs`, `renders_are_bit_identical_at_any_block_size` | unit, real-time trap |
| R9 | `crates/devices/tests/gate.rs`, `the_display_reads_the_threshold_and_return_and_the_published_gain_reduction` | unit, real-time trap |
| G1, G2 | `crates/devices/tests/rules.rs` over the shipped types | real-time trap |
