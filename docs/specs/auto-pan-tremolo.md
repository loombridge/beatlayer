# Auto Pan-Tremolo

- **Status:** partial (every rule below has a passing test; the display cell's waveform drawing and visual handle are not built yet)
- **Work packages:** WP-5.20c. Sits on WP-2.5 ([the native device framework](device-framework.md)); LFO and seeded randomness are WP-5.42's (device framework R26, R27, R29); pan law is [mixer](mixer.md) R3
- **Parity-matrix row:** Audio effects: Auto Pan-Tremolo (12.3)
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.3 "Auto Pan-Tremolo", read on 19 October 2026; Live 12 release notes for 12.3 (25 November 2025); ranges and defaults the manual does not give are Beatlayer's own

## The behaviour

Auto Pan-Tremolo modulates stereo placement or amplitude with a low-frequency oscillator to create automatic panning, tremolo, and gating. It operates in two modes: Panning, where two LFO channels modulate stereo panorama by alternating amplitude between the left and right channels, and Tremolo, where an LFO modulates signal amplitude directly to vary volume.

The LFO offers nine waveforms: Sine, Triangle, Shark Tooth, Saw Up, Saw Down, Square, Random, Wander, and Sample & Hold. Six time modes determine the modulation rate: Rate (in Hz), Time (in seconds, 100 ms to 200 s), Synced (straight beat divisions), Dotted (dotted beat divisions), Triplet (triplet beat divisions), and 16th (sixteenth notes per cycle). Stereo offset provides Phase (degrees between channels) and Spin (percentage rate difference), with random shapes restricted to Phase mode. Phase Offset shifts the LFO start position relative to the beat grid in synced modes, and acts as stereo width for random shapes. Invert flips waveform polarity.

Tremolo mode adds Harmonic (modulating high and low frequencies in antiphase across a fixed 600 Hz crossover) and Vintage (applying a warm non-linear curve to amplitude modulation). Shared controls adjust modulation Amount (reaching silence at troughs or extremes at 100 %), Shape (which sharpens transitions in Panning mode and rounds or compresses peaks into ramps in Tremolo mode), Attack Time (delaying maximum modulation depth following a transient), and Dynamic Frequency Modulation (scaling LFO rate with incoming signal level). The device reports no latency.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.3 "Auto Pan-Tremolo" | Panning and Tremolo modes; the nine waveforms; six time modes (Rate, Time 100 ms to 200 s, Synced, Dotted, Triplet, 16th); stereo offset as Phase and Spin (random waves Phase only); Phase Offset against the beat grid in synced modes, and as stereo width for random waves; Invert polarity; Tremolo's Harmonic (600 Hz split in antiphase) and Vintage (non-linear shaping); Amount; Shape behavior per mode; Attack Time transient response; Dynamic Frequency Modulation |
| S2 | Release notes | Live 12.3 (25 November 2025) | Auto Pan renamed Auto Pan-Tremolo; Tremolo mode, Harmonic, Vintage, Time mode, Attack Time, and Dynamic Frequency Modulation introduced; previous device kept as Auto Pan Legacy |
| S3 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2 | Every gain and level reaches −∞ dB; every parameter can be automated and modulated |
| S4 | Beatlayer | [The device framework spec](device-framework.md), R26, R27, R29; [the mixer spec](mixer.md), R3 | The effects LFO, synced timing, and seeded random source; constant-power pan law |
| S5 | Beatlayer | [The architecture](../02-architecture.md), "Offline rendering" and "Real-time rules" | A render repeats run after run and is bit-identical at any block size; no allocation, lock or blocking on audio thread |
| S6 | Beatlayer | Beatlayer's own | Ranges and defaults not given in the manual: Rate (0.01 to 40 Hz, default 1 Hz), Time default 1 s, divisions defaults, Phase default (180° in Panning, 0° in Tremolo), Attack Time range (0 to 1000 ms), Dynamic Frequency Modulation (-100 % to 100 %), Shape (-100 % to 100 %), 600 Hz crossover filter design, and Vintage non-linear curve |

## Rules

- **R1.** Beatlayer ships Auto Pan-Tremolo, an audio-effect device type with its own layout: every parameter has a descriptor and a place in the layout. The layout consists of mode choosers, waveform and rate controls, stereo offset and phase offset, harmonic and vintage toggles, amount, shape, attack time, and dynamic frequency modulation. (S1, S2, S6)
- **R2.** Mode selects Panning or Tremolo. In Panning mode, modulation alters the stereo pan position according to Beatlayer's constant-power pan law (mixer R3: left √2·sin((1 − pan)·π/4), right √2·sin((1 + pan)·π/4)); at Amount 100 %, a mono input moves fully between sides, the far side reaching −∞ dB. In Tremolo mode, modulation scales signal amplitude directly; at Amount 100 %, troughs reach −∞ dB. Both modes share Amount, rate, Shape, Attack Time, and Dynamic Frequency Modulation. (S1, S3, S4, S6)
- **R3.** Waveform is one of Sine, Triangle, Shark Tooth, Saw Up, Saw Down, Square, Random, Wander, and Sample & Hold. Random, Wander, and Sample & Hold take reproducible random values seeded from the device's ID (device framework R29). (S1, S4)
- **R4.** Time mode is Rate (Hz), Time (seconds), Synced, Dotted, Triplet, or 16th. Rate runs from 0.01 Hz to 40 Hz; Time runs from 100 ms (0.1 s) to 200 s; Synced offers straight beat divisions 1/32 to 8 bars; Dotted offers dotted divisions 1/16. to 4 bars.; Triplet offers triplet divisions 1/16T to 2 bars T; 16th offers 1 to 64 sixteenth notes per cycle. Free rates (Hz and Time) produce periods accurate to within 0.1 %; synced modes align cycles to the song's beat grid across tempos and tempo changes. (S1, S4, S6)
- **R5.** Stereo Offset is Phase (0° to 360°) or Spin (−50 % to 50 %). At Phase 180°, channels move in antiphase. Spin detunes the right channel's rate by its percentage. Waveforms Random, Wander, and Sample & Hold allow Phase mode only. (S1, S4, S6)
- **R6.** Phase Offset (−180° to 180°) shifts the LFO start position relative to the beat grid in synced modes. For Random, Wander, and Sample & Hold waveforms, Phase Offset scales the stereo width between the two channels' random paths (0° mono, 180° full stereo variation). (S1, S6)
- **R7.** Invert inverts the polarity of the LFO waveform: positive and negative values swap, flipping peaks into troughs and Saw Up into Saw Down. (S1)
- **R8.** Harmonic mode is an option in Tremolo mode that splits the audio into low and high bands at a fixed 600 Hz crossover. The two bands are modulated in antiphase. At Amount 0 %, the two bands sum flat within 0.1 dB (0 dBFS). (S1, S6)
- **R9.** Vintage mode is an option in Tremolo mode that applies a warm non-linear shaping curve (`tanh(1.2 * x) / 1.2`) to the amplitude modulation. (S1, S6)
- **R10.** Shape (−100 % to 100 %) reshapes the modulation wave. In Panning mode, positive Shape sharpens transitions between channels. In Tremolo mode, positive values round the peaks while negative values compress peaks into angular shapes, producing gating ramps with Invert off and pumping ramps with Invert on. (S1, S6)
- **R11.** Attack Time (0 to 1000 ms) delays the onset of modulation depth after a transient/onset is detected in the input signal. Following an onset, depth ramps from 0 to full Amount in Attack Time within 5 %. (S1, S6)
- **R12.** Dynamic Frequency Modulation (−100 % to 100 %) dynamically scales the LFO rate with the incoming signal level. A positive value increases the LFO rate as signal level rises; a negative value decreases it. (S1, S6)
- **R13.** Auto Pan-Tremolo introduces no latency (reports 0 frames). (S1, S4)
- **R14.** The display cell publishes the current pan position and tremolo level after each block, readable headlessly via `device.display`. (S1, S4)
- **R15.** Processing allocates, locks, and blocks nothing on the audio thread. Renders are bit-identical at block sizes 1, 64, 512, and 4,096, and repeat run after run. (S4, S5)
- **R16.** Every parameter can be inserted, set by ID, and read back through the agent API, is automatable and modulatable, and a set saves and reopens with every parameter. (S3, S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/auto_pan_tremolo.rs`, `auto_pan_tremolo_is_described` | unit |
| R2 | `crates/devices/tests/auto_pan_tremolo.rs`, `panning_at_amount_100_moves_mono_input_by_pan_law_reaching_silence`, `tremolo_at_amount_100_reaches_silence_at_troughs` | unit, real-time trap |
| R3 | `crates/devices/tests/auto_pan_tremolo.rs`, `every_waveform_plays_and_random_shapes_are_repeatable` | unit, real-time trap |
| R4 | `crates/devices/tests/auto_pan_tremolo.rs`, `hz_and_time_periods_are_accurate_within_0_1_percent`, `each_synced_mode_starts_cycle_on_beat_across_tempos` | unit, real-time trap |
| R5 | `crates/devices/tests/auto_pan_tremolo.rs`, `phase_offsets_channels_and_spin_detunes` | unit, real-time trap |
| R6 | `crates/devices/tests/auto_pan_tremolo.rs`, `each_synced_mode_starts_cycle_on_beat_across_tempos`, `phase_offset_acts_as_width_on_random_shapes` | unit, real-time trap |
| R7 | `crates/devices/tests/auto_pan_tremolo.rs`, `invert_flips_the_shape` | unit, real-time trap |
| R8 | `crates/devices/tests/auto_pan_tremolo.rs`, `harmonic_bands_sum_flat_at_amount_zero_and_move_in_antiphase` | unit, real-time trap |
| R9 | `crates/devices/tests/auto_pan_tremolo.rs`, `vintage_adds_warmth_with_nonlinear_shaping` | unit, real-time trap |
| R10 | `crates/devices/tests/auto_pan_tremolo.rs`, `shape_modifies_panning_and_tremolo_waveforms` | unit, real-time trap |
| R11 | `crates/devices/tests/auto_pan_tremolo.rs`, `attack_time_delays_modulation_depth_after_onset_within_5_percent` | unit, real-time trap |
| R12 | `crates/devices/tests/auto_pan_tremolo.rs`, `dynamic_frequency_modulation_scales_rate_with_input_level` | unit, real-time trap |
| R13 | `crates/devices/tests/auto_pan_tremolo.rs`, `auto_pan_tremolo_reports_zero_latency` | unit |
| R14 | `crates/devices/tests/auto_pan_tremolo.rs`, `pan_position_is_readable_headlessly_through_display` | unit |
| R15 | `crates/devices/tests/auto_pan_tremolo.rs`, `renders_are_bit_identical_at_various_block_sizes_and_runs`; `rtsan/tests/devices.rs`, `auto_pan_tremolo_passes_under_rtsan` | unit, RTSan |
| R16 | `crates/devices/tests/rules.rs` over shipped types; `crates/cli/tests/agent.rs`, `an_agent_inserts_auto_pan_tremolo_by_type_and_sets_and_reads_parameters`; `crates/devices/tests/auto_pan_tremolo.rs`, `a_set_with_auto_pan_tremolo_saves_and_reopens` | unit, MCP |

## Not built yet

- The display cell's real-time waveform drawing and visual handles: display cell remains a black frame.
- Auto Pan Legacy, which isn't built: a Live set or preset holding it maps it to Auto Pan-Tremolo ([Live device mapping](live-device-mapping.md) R15).
