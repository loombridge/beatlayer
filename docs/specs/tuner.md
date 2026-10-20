# Tuner

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.28 (measurement). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); device displays are WP-5.43 ([device displays](device-framework.md#r35-r38))
- **Parity-matrix row:** Audio effects: Tuner
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.39 "Tuner", read on 20 October 2026; chapter 15 "Using Tuning Systems"; Live 12.1 release notes

## The behaviour

Tuner analyses incoming monophonic audio to detect its fundamental pitch and distance in cents from the nearest semitone. It is a measurement tool and passes audio through unaltered with zero latency.

The device provides two main views: Classic View (showing a tuner needle curve with Target Mode and Strobe Mode) and Histogram View (displaying pitch history over time with zoom and Auto centring). The readout toggles between frequency in Hertz and deviation in cents. The reference concert pitch defaults to 440 Hz and adjusts from 410 to 480 Hz. Note spelling preferences (Sharps, Flats, or Sharps and Flats) are provided as context menu options in the device's Show Options menu. Tuning systems support waits for Phase 6; until then equal temperament relative to the Reference pitch is used.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.39 "Tuner" | Monophonic pitch detection; Classic View with Target and Strobe modes; Histogram View with zoom, scroll and Auto centring; Hertz/Cents switch; Note Spellings context menu (Sharps, Flats, Sharps and Flats); Reference slider 410–480 Hz, default 440 Hz |
| S2 | Beatlayer | [The native device framework](device-framework.md), R9, R11, R35–R38; Show Options items (WP-5.42 / T189) | Display data published lock-free; headless readback via `device.display`; menu options in Show Options menu |

## Rules

- **R1.** Audio passes through untouched with zero latency: output is bit-for-bit identical to input at all sample rates and block sizes. (S1, S2)
- **R2.** Monophonic sines between 41 Hz and 2000 Hz are detected within 1 cent of their true frequency. (S1)
- **R3.** The distance in cents from the nearest equal-temperament semitone is calculated as $100 \times (p - \text{round}(p))$, where $p = 69 + 12 \log_2(f / \text{Ref})$. Setting the Reference to 432 Hz shifts the cents readout according to this relationship. (S1)
- **R4.** The note spellings in the Show Options menu (Sharps `C#`, Flats `D♭`, or Sharps and Flats `C#/D♭`) govern note name formatting. (S1)
- **R5.** The device offers Classic and Histogram views, switchable via view buttons or `device.set_display_view`. (S1, S2)
- **R6.** Tuner publishes its detected pitch, nearest note, and cents deviation lock-free after every block, readable headless by `device.display`. (S2)
- **R7.** The display cell expands in a floating window according to IMP-14. (S2)
- **R8.** Every parameter can be automated and modulated; no memory allocation, lock, or blocking operation occurs on the audio thread. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/tuner.rs`, `the_output_is_the_input_bit_for_bit` | unit, real-time trap |
| R2 | `crates/devices/tests/tuner.rs`, `sines_from_41_hz_to_2_khz_read_their_note_within_one_cent` | unit, real-time trap |
| R3 | `crates/devices/tests/tuner.rs`, `a_432_hz_reference_moves_the_cents_reading` | unit, real-time trap |
| R4 | `crates/devices/tests/tuner.rs`, `each_spelling_names_the_note_as_stated` | unit, real-time trap |
| R5 | `crates/devices/tests/tuner.rs`, `views_and_modes_switch_cleanly` | unit |
| R6 | `crates/shell/tests/device_displays.rs`, `the_tuners_display_publishes_and_reads_headless` | unit |
| R7 | `crates/ui/tests/device_panel.rs`, `the_tuners_cell_draws_in_docked_and_floating_windows` | UI script |
| R8 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; `rtsan/tests/devices.rs`, `tuner_passes_under_rtsan` | unit, RTSan |

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
- Beatlayer's own choices: YIN-based monophonic time-domain pitch detector with parabolic interpolation over a 2048-sample analysis window; Phase 6 tuning systems integration.
