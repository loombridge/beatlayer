# Tuner

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.28 (measurement). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); device displays are WP-5.43 ([the device framework spec](device-framework.md), R35 to R38)
- **Parity-matrix row:** Audio effects: Tuner
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.39 "Tuner", read on 20 October 2026; chapter 15 "Using Tuning Systems"; Live 12.1 release notes

## The behaviour

Tuner analyses incoming monophonic audio to detect its fundamental pitch and distance in cents from the nearest semitone. It is a measurement tool and passes audio through unaltered with zero latency.

The device provides two main views: Classic View (showing a tuner needle curve with Target Mode and Strobe Mode) and Histogram View (displaying pitch history over time with zoom and Auto centring). The readout toggles between frequency in Hertz and deviation in cents. The reference concert pitch defaults to 440 Hz and adjusts from 410 to 480 Hz. Note spelling preferences (Sharps, Flats, or Sharps and Flats) are provided as context menu options in the device's Show Options menu. With a tuning in the set, the Tuner names the nearest note of the tuning and its distance in cents, and its Reference is set aside (R10); without one it uses equal temperament relative to the Reference pitch.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.39 "Tuner" | Monophonic pitch detection; Classic View with Target and Strobe modes; Histogram View with zoom, scroll and Auto centring; Hertz/Cents switch; Note Spellings context menu (Sharps, Flats, Sharps and Flats); Reference slider 410–480 Hz, default 440 Hz |
| S2 | Beatlayer | [The native device framework](device-framework.md), R9, R11, R35–R38; Show Options items (WP-5.42 / T189) | Display data published lock-free; headless readback via `device.display`; menu options in Show Options menu |

## Rules

- **R1.** Audio passes through untouched with zero latency: output is bit-for-bit identical to input at all sample rates and block sizes. (S1, S2)
- **R2.** Monophonic sines between 41 Hz and 2000 Hz are detected within 1 cent of their true frequency. The pitch is measured once per 2,048 samples of new audio (about 43 ms at 48 kHz), so the readout follows a player quickly without the analysis costing more than a block of audio time; the analysis buffer holds a whole period of 41 Hz at the node's sample rate. (S1)
- **R3.** The distance in cents from the nearest equal-temperament semitone is calculated as $100 \times (p - \text{round}(p))$, where $p = 69 + 12 \log_2(f / \text{Ref})$. Setting the Reference to 432 Hz shifts the cents readout according to this relationship. (S1)
- **R4.** The note spellings in the Show Options menu (Sharps `C#`, Flats `D♭`, or Sharps and Flats `C#/D♭`) govern note name formatting. (S1)
- **R5.** The device offers Classic and Histogram views, switchable via view buttons or `device.set_display_view`. (S1, S2)
  - Classic View supports Target mode (default, showing a needle along the curve within [-50, +50] cents) and Strobe mode (simulated rotating strobe band whose speed and direction indicate pitch deviation).
  - Histogram View displays pitch history over time (the last 32 measurements), with Auto centring enabled by the Auto parameter (centring the pitch history within a ±50 cents window around the detected note).
- **R6.** Readout chooser toggles between displaying deviation in cents and frequency in Hertz in the needle marker and primary readings. (S1)
- **R7.** Tuner publishes its detected pitch, nearest note, cents deviation, strobe phase, and pitch history lock-free after every block, readable headless by `device.display`. (S2)
- **R8.** The display cell expands in a floating window according to IMP-14. (S2)
- **R9.** Every parameter can be automated and modulated; no memory allocation, lock, or blocking operation occurs on the audio thread. (S2)
- **R10.** While the set has a tuning, the Tuner, on any track but a MIDI track that bypasses it, names the note of the tuning nearest the pitch it hears, as the note editor names it, and shows its distance in cents; its Reference is set aside. It reads the tuning as node data, as the instruments do. The rule is [the tunings spec](tunings.md) R33. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/tuner.rs`, `the_output_is_the_input_bit_for_bit`, `renders_are_bit_identical_at_block_sizes_1_64_512_and_4096` | unit, real-time trap |
| R2 | `crates/devices/tests/tuner.rs`, `sines_from_41_hz_to_2_khz_read_their_note_within_one_cent`, `a_96_khz_stream_reads_its_pitch_within_one_cent` | unit, real-time trap |
| R3 | `crates/devices/tests/tuner.rs`, `a_432_hz_reference_moves_the_cents_reading` | unit, real-time trap |
| R4 | `crates/devices/tests/tuner.rs`, `each_spelling_names_the_note_as_stated` | unit, real-time trap |
| R5 | `crates/devices/tests/tuner.rs`, `views_and_modes_switch_cleanly`, `histogram_view_shows_pitch_history_and_auto_centring` | unit |
| R6 | `crates/devices/tests/tuner.rs`, `classic_strobe_mode_and_readout_chooser` | unit |
| R7 | `crates/shell/tests/device_displays.rs`, `the_tuners_display_publishes_and_reads_headless` | unit |
| R8 | `crates/ui/tests/device_panel.rs`, `the_tuners_cell_draws_in_docked_and_floating_windows` | UI script |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; `rtsan/tests/devices.rs`, `tuner_passes_under_rtsan` | unit, RTSan |
| R10 | the tunings spec's R33 tests: `crates/devices/tests/tuner_tuning.rs` | unit, real-time |

## Defaults, ranges and details

- Parameters:
  - `Reference`: Continuous, range 410.0 Hz to 480.0 Hz, default 440.0 Hz.
  - `Readout`: Chooser, choices `["Hz", "Cents"]`, default Cents (index 1).
  - `Classic Mode`: Chooser, choices `["Target", "Strobe"]`, default Target (index 0).
  - `Auto`: Toggle, default true.
  - `Sharps (C#)`: Toggle (menu option), default true.
  - `Flats (D♭)`: Toggle (menu option), default false.
  - `Sharps and Flats (C#/D♭)`: Toggle (menu option), default false.
- Views: `Classic` and `Histogram`.
- Beatlayer's own choices: YIN-based monophonic time-domain pitch detector with parabolic interpolation over a 2048-sample analysis window, measured once per 2,048 samples (Beatlayer's own response time); tuning systems follow the set's tuning (R10).
