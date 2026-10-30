# Chorus-Ensemble

- **Status:** implemented
- **Work packages:** WP-5.20b; Poli's built-in chorus reuses this chorus in its two-voice mode (WP-8.16, [Bass and Poli](bass-and-poli.md) R14)
- **Parity-matrix row:** Audio effects: Chorus-Ensemble, to Live's current version (the 12.4 Chorus-Ensemble, with its ensemble mode)
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.8 "Chorus-Ensemble" and 29.8.1, read on 18 October 2026; the Live 12.4 release notes (5 May 2026); the ranges, defaults and algorithms the manual does not give are Beatlayer's own

## The behaviour

Chorus-Ensemble is Beatlayer's chorus, ensemble, and vibrato effect. It modulates delayed signals to thicken audio or produce pitch variation.

The device provides three modes: Chorus (two modulated delay lines with selectable fixed delay times or Auto scaling, and 1 or 2 taps), Ensemble (three modulated delay lines with modulation phases evenly split 120° apart for a rich 1970s string-ensemble chorus texture), and Vibrato (direct pitch modulation with no dry layer).

In Chorus and Ensemble modes, Width adjusts the stereo image balance from mono (0%) through equal (100%) to sides-boosted (200%, with side channel gain 6 dB higher than mid). All modes offer an adjustable high-pass filter that excludes bass frequencies from modulation. Global parameters include Output gain and Warmth.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.8 "Chorus-Ensemble" and 29.8.1 | Three modes (Chorus, Ensemble, Vibrato). High-pass filter (20 Hz to 2000 Hz). Width (0% to 200%). Rate, Amount, Feedback with invert toggle, Warmth, Output, Dry/Wet. Vibrato: wet only, Offset (0° to 180°), Shape (sine to triangle), disables Feedback and Dry/Wet |
| S2 | Manual | Live 12.4 release notes | Classic mode renamed Chorus; added Time (Auto, 7, 10, 20, 35, 50 ms) and Taps (1 or 2 taps) |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain reaches −∞ dB; every parameter is automatable and modulatable |
| S4 | Beatlayer | [Device framework](device-framework.md) and [device chains](device-chains.md) | Headless control, parameters, real-time safety |
| S5 | Beatlayer | Beatlayer's own | The manual gives no ranges, defaults, depth, delay times or algorithms for this device; every rule that gives one gives Beatlayer's own |

## Rules

- **R1.** Beatlayer ships Chorus-Ensemble, a native audio effect device type (`beatlayer.chorus_ensemble`, display name "Chorus-Ensemble", `DeviceRole::AudioEffect`). (S1, S2)
- **R2.** In all modes, a high-pass filter is available with cutoff from 20.0 Hz to 2000.0 Hz (default 20.0 Hz), attenuating modulation of content below the cutoff. Output is a gain reaching −∞ dB (default 0 dB / gain 1.0). Warmth (0% to 100%, default 0%) adds soft saturation and gentle filtering. (S1, S3, S5)
- **R3.** In Chorus and Ensemble modes, controls include Rate (0.01 Hz to 20.0 Hz, default 1.0 Hz), Amount (0% to 100%, default 50%), Feedback (0% to 100%, default 0%), Feedback Invert (toggle Ø, default off), Dry/Wet (0% to 100%, default 50%), and Width (0% to 200%, default 100%). Width sets the balance between mid and side channels: at 0% wet is mono, at 100% equal, and at 200% side channel gain is 6 dB higher than mid gain within 0.1 dB. (S1, S5)
- **R4.** In Chorus mode, two delay lines per channel are modulated. The Time parameter selects Auto (delay time scales dynamically with modulation amount) or a fixed delay time (7 ms, 10 ms, 20 ms, 35 ms, or 50 ms), maintaining a constant base delay independent of Amount. The Taps parameter selects 1 Tap (single delay line per channel) or 2 Taps (two delay lines per channel). (S1, S2, S5)
- **R5.** In Ensemble mode, three delay lines per channel are modulated with their LFO phases split evenly a third of a cycle apart (120° apart within 1°). The lines' base delay is 10 ms and Amount scales the modulation's depth to 3.5 ms at 100 % (Beatlayer's own: the manual gives neither). (S1, S5)
- **R6.** In Vibrato mode, pitch modulation is applied with no dry signal passing (wet only). Feedback, Feedback Invert, and Dry/Wet are disabled and ignored. Offset (0° to 180°, default 180°) controls the stereo channel phase offset. Shape (0% to 100%, default 0%) morphs the modulation waveform seamlessly from Sine (at 0%) to Triangle (at 100%). (S1, S5)
- **R7.** The device reports 0 latency through `Node::latency`. (S1)
- **R8.** Renders are bit-identical at block sizes 1, 64, 512, and 4096. No memory allocation, locking, or blocking calls occur on the audio thread. (S3, S4)
- **R9.** An agent can add, configure parameters by ID, read parameters and display headlessly, and save/reopen sets containing the device. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/chorus_ensemble.rs`, `the_chorus_ensemble_is_described_and_laid_out` | unit |
| R2 | `crates/devices/tests/chorus_ensemble.rs`, `high_pass_output_and_warmth_act_on_audio` | real-time trap |
| R3 | `crates/devices/tests/chorus_ensemble.rs`, `width_and_feedback_operate_in_chorus_mode` | real-time trap |
| R4 | `crates/devices/tests/chorus_ensemble.rs`, `chorus_time_modes_and_tap_counts_behave_as_specified` | real-time trap |
| R5 | `crates/devices/tests/chorus_ensemble.rs`, `ensemble_modulations_are_one_third_cycle_apart` | real-time trap |
| R6 | `crates/devices/tests/chorus_ensemble.rs`, `vibrato_mode_passes_no_dry_and_ignores_feedback_and_dry_wet` | real-time trap |
| R7 | `crates/devices/tests/chorus_ensemble.rs`, `latency_is_zero` | unit |
| R8 | `crates/devices/tests/chorus_ensemble.rs`, `renders_are_bit_identical_at_any_block_size`; `rtsan/tests/devices.rs`, `the_modulation_devices_pass_under_rtsan` | real-time trap, RTSan |
| R9 | `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_the_modulation_devices_by_id_and_reopens_them` | MCP script |
