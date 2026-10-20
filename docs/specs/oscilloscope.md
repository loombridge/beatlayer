# Oscilloscope

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.28 (measurement). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); device displays are WP-5.43 ([device displays](device-framework.md#r35-r38))
- **Parity-matrix row:** Audio effects: Oscilloscope (IMP-31)
- **Checked against:** `docs/05-improvements.md` IMP-31 (accepted 20 October 2026); `crates/devices/src/testing.rs` SCOPE

## The behaviour

Oscilloscope is a native audio effect device with no Live counterpart, positioned beside Spectrum and Tuner in the browser's Audio Effects. It captures and draws the waveform of incoming audio in real time, with headless display data readable by agents.

Its controls include a time window (1 ms to 1 s), display gain, freeze, grid divisions (1 to 16), and a rising-edge trigger with an adjustable threshold level. It passes audio through unaltered with zero latency.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The improvements](../05-improvements.md), IMP-31 (accepted 20 October 2026) | A native oscilloscope that draws its input's waveform, its display data readable headless; controls and trigger proposed with WP-5.28 |
| S2 | Beatlayer | [The native device framework](device-framework.md), R9, R11, R35–R38; `crates/devices/src/testing.rs` | Test scope layout: Time, Gain, Freeze, Divisions, Trigger; display data published lock-free; no allocation on audio thread |

## Rules

- **R1.** Audio passes through untouched with zero latency: output is bit-for-bit identical to input at all sample rates and block sizes. (S1, S2)
- **R2.** When triggered, the captured waveform window begins at the rising-edge crossing through the set trigger level. (S1, S2)
- **R3.** With Freeze enabled, the current captured window is held stationary and does not update from incoming audio. (S1, S2)
- **R4.** The Time parameter sets the span of audio captured and displayed, from 1 ms to 1000 ms. (S2)
- **R5.** Gain adjusts the visual scaling of the displayed waveform between -36 dB and +36 dB without altering the audio output. (S2)
- **R6.** Waveform display data is published lock-free after every block through triple-buffer display data, and is readable headless by `device.display`. (S2)
- **R7.** The display cell expands in a floating window according to IMP-14. (S2)
- **R8.** Every parameter can be automated and modulated; no memory allocation, lock, or blocking operation occurs on the audio thread. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/oscilloscope.rs`, `the_output_is_the_input_bit_for_bit` | unit, real-time trap |
| R2 | `crates/devices/tests/oscilloscope.rs`, `a_triggered_sine_starts_on_its_rising_crossing` | unit, real-time trap |
| R3 | `crates/devices/tests/oscilloscope.rs`, `freeze_holds_the_captured_window` | unit, real-time trap |
| R4 | `crates/devices/tests/oscilloscope.rs`, `time_parameter_adjusts_window_span` | unit, real-time trap |
| R5 | `crates/devices/tests/oscilloscope.rs`, `gain_scales_display_without_altering_audio` | unit, real-time trap |
| R6 | `crates/shell/tests/device_displays.rs`, `the_oscilloscopes_display_publishes_and_reads_headless` | unit |
| R7 | `crates/ui/tests/device_panel.rs`, `the_oscilloscopes_cell_draws_in_docked_and_floating_windows` | UI script |
| R8 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; `rtsan/tests/devices.rs`, `oscilloscope_passes_under_rtsan` | unit, RTSan |

## Defaults, ranges and details

- Parameters:
  - `Time`: Continuous (ms), range 1.0 ms to 1000.0 ms, logarithmic, default 50.0 ms.
  - `Gain`: Continuous (gain), range -36.0 dB to 36.0 dB, default 0.0 dB.
  - `Freeze`: Toggle, default false.
  - `Divisions`: Integer, range 1 to 16, default 4.
  - `Trigger`: Continuous (%), range -100.0% to 100.0%, default 0.0%.
- Layout: Control bands on either side of a centered stretchable display cell ("Waveform", min width 180 pt, `BODY_HEIGHT` 158 pt).
