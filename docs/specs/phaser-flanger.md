# Phaser-Flanger

- **Status:** implemented
- **Work packages:** WP-5.20b
- **Parity-matrix row:** Audio effects: Phaser-Flanger
- **Checked against:** Live 12's reference manual, chapter 29 "Live Audio Effect Reference", section 29.29 "Phaser-Flanger", read on 18 October 2026; section 23.2.1

## The behaviour

Phaser-Flanger combines phaser, flanger and doubler modulation effects in a single device with three selectable modes: Phaser, Flanger, and Doubler. Each mode modulates the signal phase or delay time to produce characteristic comb filtering, sweeping notches, or double-tracking textures.

In Phaser mode, cascaded all-pass filter sections produce sweeping notches whose count, center frequency, spread, and modulation routing (Blend) are adjustable. In Flanger mode, a short modulated delay line with feedback generates comb filtering. In Doubler mode, delay lines generate thicker double-tracking effects with bipolar modulation.

Global controls include modulation Amount, Feedback with an Invert toggle, Output gain, Warmth for gentle saturation and tone, and Dry/Wet mix balance. The expanded view reveals advanced modulation controls: the main LFO with ten waveforms, tempo sync, Phase and Spin stereo modes, and Duty Cycle; a secondary triangle LFO (LFO2) with rate and Mix; an envelope follower with attack and release; and a Safe Bass high-pass filter that excludes low frequencies from the wet path.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.29 "Phaser-Flanger" | Three modes (Phaser, Flanger, Doubler). Phaser: Notches, Center, Spread, Blend. Flanger: Time, comb filtering. Doubler: Time, bipolar modulation. Global: Amount, Feedback with invert toggle (Ø), Output, Warmth, Dry/Wet. Expanded view: main LFO (waveforms, Hz/sync, Phase/Spin, Duty Cycle), LFO2 (triangle, Mix, Hz/sync), envelope follower (Env Fol, Amount, Attack, Release), Safe Bass (5 Hz to 3000 Hz) |
| S2 | Manual | Live 12 reference manual, section 23.2.1 "Device Expanded View" | Device expanded view toggled from the title bar |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain reaches −∞ dB; every parameter is automatable and modulatable |
| S4 | Beatlayer | [Device framework](device-framework.md) and [device chains](device-chains.md) | Shared LFO, envelope follower, expand toggle, headless control |

## Rules

- **R1.** Beatlayer ships Phaser-Flanger, a native audio effect device type (`beatlayer.phaser_flanger`, display name "Phaser-Flanger", `DeviceRole::AudioEffect`). (S1)
- **R2.** Global parameters are Amount (0% to 100%, default 50%), Feedback (0% to 100%, default 0%), Feedback Invert (toggle Ø, default off), Output (gain reaching −∞ dB, default 0 dB / gain 1.0), Warmth (0% to 100%, default 0%), and Dry/Wet (0% to 100%, default 50%). (S1, S3)
- **R3.** In Phaser mode, the effect filters the audio through $N$ second-order all-pass filter sections, with Notches $N \in [1, 16]$ (default 4), Center frequency (20 Hz to 20 kHz, default 1 kHz), Spread (0% to 100%, default 50%), and Blend (0.0 to 1.0, default 0.0). At Amount 0, with 50% dry/wet and 0% feedback, notches occur at frequencies $f_k = \text{Center} \cdot 2^{(k - (N - 1) / 2) \cdot \text{spacing}}$, where $\text{spacing} = 0.5 \cdot (0.2 + 1.8 \cdot \text{Spread} / 100.0)$, matching within 2%. (S1; mapping is Beatlayer's own)
- **R4.** In Flanger mode, comb filtering is produced via a delay line with delay Time (0.05 ms to 20 ms, default 1 ms). At Amount 0, notches fall at comb frequencies $f_k = (k + 0.5) / \text{Time}$ (for $k = 0, 1, 2, \dots$) within 1%. Feedback intensifies the resonance peaks; Feedback Invert shifts the notches to integer multiples $f_k = k / \text{Time}$. (S1)
- **R5.** In Doubler mode, taps sit at delay times $T$ and $1.5 T$ where $T$ is Time (5 ms to 100 ms, default 20 ms). Modulation in Doubler mode is bipolar about the base delay. (S1; tap ratio is Beatlayer's own)
- **R6.** Warmth applies soft saturation and gentle high-frequency rolloff. (S1; algorithm is Beatlayer's own)
- **R7.** The device features an expanded view toggled via `device.expand`. The main LFO, LFO2, envelope follower, and Safe Bass controls reside in the expanded layout. (S1, S2, S4)
- **R8.** The main LFO offers the manual's ten waveforms, which are device framework R26's shared LFO's waves under the names the manual gives them: Sine, Triangle (the default), Saw Up, Saw Down, Rectangle, Random, Random S&H (the shared LFO's Sample & Hold), Triangle Analog, Triangle 8 and Triangle 16; the shared LFO's Shark Tooth and Wander are not among the device's. The rate is in Hz (0.01 to 40 Hz) or in tempo-synced divisions, the stereo modes are Phase (0° to 360°) and Spin (−100% to 100%), and Duty Cycle (−100% to 100%) changes the time scale of the waveform's period, the random waves ignoring it. (S1, S4)
- **R9.** LFO2 provides a triangle waveform with rate (Hz or tempo-synced divisions). LFO2 Mix (0% to 100%, default 0%) blends between the main LFO (at 0%) and LFO2 (at 100%). (S1)
- **R10.** The envelope follower (Env Fol toggle, default off) provides Amount (−100% to 100%, default 0%), Attack (0.1 ms to 500 ms, default 10 ms), and Release (1 ms to 1000 ms, default 100 ms), modulating effect depth from signal level. (S1, S4)
- **R11.** Safe Bass splits the signal at its cutoff into two bands, a first-order pair with the low band taking the input and the high band the wet signal, so that where the effect leaves the signal alone the two bands put it back together and the low band reaches the dry/wet mix in phase and at its own level (the manual gives Safe Bass no slope; the pair is Beatlayer's own). The cutoff runs from 5 Hz to 3000 Hz (default 100 Hz). Content an octave below the cutoff comes out within 0.5 dB of the dry signal. (S1)
- **R12.** The device reports 0 latency through `Node::latency`. (S1)
- **R13.** Renders are bit-identical at block sizes 1, 64, 512, and 4096, and repeat run after run. No memory allocation, locking, or blocking calls occur on the audio thread. (S3, S4)
- **R14.** An agent can add, configure parameters by ID, expand/collapse, read parameters and display headlessly, and save/reopen sets containing the device. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/phaser_flanger.rs`, `the_phaser_flanger_is_described_and_laid_out` | unit |
| R2 | `crates/devices/tests/phaser_flanger.rs`, `global_parameters_act_on_audio` | real-time trap |
| R3 | `crates/devices/tests/phaser_flanger.rs`, `phaser_notches_fall_at_mapped_frequencies` | real-time trap |
| R4 | `crates/devices/tests/phaser_flanger.rs`, `flanger_notches_fall_at_comb_frequencies_and_invert_moves_them` | real-time trap |
| R5 | `crates/devices/tests/phaser_flanger.rs`, `doubler_taps_sit_at_spec_times` | real-time trap |
| R6 | `crates/devices/tests/phaser_flanger.rs`, `warmth_adds_saturation` | real-time trap |
| R7 | `crates/devices/tests/phaser_flanger.rs`, `expanded_part_toggles_with_expand_command` | unit, MCP script |
| R8 | `crates/devices/tests/phaser_flanger.rs`, `main_lfo_duty_cycle_moves_delay_time_path` | real-time trap |
| R9 | `crates/devices/tests/phaser_flanger.rs`, `lfo2_mix_blends_modulation_path` | real-time trap |
| R10 | `crates/devices/tests/phaser_flanger.rs`, `envelope_follower_modulates_effect` | real-time trap |
| R11 | `crates/devices/tests/phaser_flanger.rs`, `safe_bass_leaves_content_an_octave_below_within_half_db_of_dry` | real-time trap |
| R12 | `crates/devices/tests/phaser_flanger.rs`, `latency_is_zero` | unit |
| R13 | `crates/devices/tests/phaser_flanger.rs`, `renders_are_bit_identical_at_any_block_size`; `rtsan/tests/devices.rs`, `the_modulation_devices_pass_under_rtsan` | real-time trap, RTSan |
| R14 | `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_the_modulation_devices_by_id_and_reopens_them` | MCP script |
