# Spectrum

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-5.28 (measurement). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); device displays are WP-5.43 ([device displays](device-framework.md#r35-r38))
- **Parity-matrix row:** Audio effects: Spectrum
- **Checked against:** the Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.38 "Spectrum", read on 20 October 2026; [layout.md](../ui-parity/layout.md) L-DEV-004, L-DEV-007, L-DEV-013

## The behaviour

Spectrum performs real-time frequency analysis of incoming audio signals and displays the magnitude spectrum across frequency. It is a measurement tool and not an audio effect: incoming audio passes through untouched with zero latency.

The user can configure the analysis block size, channel routing (Left, Right, or Both), refresh interval, averaging depth across successive blocks, drawing style (interpolated line or discrete bins), and peak hold (Max). The frequency axis can be displayed in linear, logarithmic, or semitone scaling. The vertical dynamic range can be adjusted manually with range sliders or automatically to incoming signal levels. The device layout includes an expanded view that gives the analyser display more space.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29 "Live Audio Effect Reference", section 29.38 "Spectrum" | Block chooser selects sample count for analysis; Channel selects left, right or both; Refresh sets update frequency; Avg sets block averaging count; Graph toggles between line and bins; Max holds accumulated peak amplitude, reset by click; Scale X toggles linear, log and semitones; Range/Auto toggles dynamic range mode; range sliders set min and max amplitude; expanded view button moves display to large window |
| S2 | Beatlayer | [The native device framework](device-framework.md), R9, R11, R35–R38 | Display data published lock-free after every block; display cell grows with floating window; headless readback via `device.display`; no allocation, lock or blocking on audio thread |
| S3 | Observation | Live 12.4.3 / 12.4.6 layout and display cells | Display cell framed in black; expanded view toggled from title bar |

## Rules

- **R1.** Audio passes through untouched with zero latency: output is bit-for-bit identical to input at all sample rates and block sizes. (S1, S2)
- **R2.** The analyser computes Hann-windowed FFT measurements of size set by the Block parameter (256, 512, 1024, 2048, 4096, 8192, 16384). Calibrated such that a full-scale sine wave at a bin frequency reads 0.0 dB within 0.1 dB. (S1, S2)
- **R3.** Discrete frequency bins are spaced exactly `rate / Block` Hz apart. (S1)
- **R4.** Channel chooser selects analysis of Left channel, Right channel, or Both (mono sum `(L + R) * 0.5`). (S1)
- **R5.** Avg averages the power of N successive analysis blocks (from 1 to 32 blocks, default 1). (S1)
- **R6.** Max enables peak-hold: the highest measured level at each bin is retained until cleared or reset. (S1)
- **R7.** The latest spectrum is published lock-free after every block through triple-buffer display data, and is readable headless by `device.display` with `live = true`. (S2)
- **R8.** The display cell expands in a floating window according to IMP-14. (S2)
- **R9.** Every parameter can be automated and modulated; no memory allocation, lock, or blocking operation occurs on the audio thread. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/spectrum_analyser.rs`, `the_output_is_the_input_bit_for_bit` | unit, real-time trap |
| R2 | `crates/devices/tests/spectrum_analyser.rs`, `a_full_scale_sine_reads_0_db_within_point_one_db_at_every_block_size` | unit, real-time trap |
| R3 | `crates/devices/tests/spectrum_analyser.rs`, `the_bins_are_the_rate_over_block_apart` | unit, real-time trap |
| R4 | `crates/devices/tests/spectrum_analyser.rs`, `each_channel_choice_reads_its_channel` | unit, real-time trap |
| R5 | `crates/devices/tests/spectrum_analyser.rs`, `avg_n_averages_n_blocks` | unit, real-time trap |
| R6 | `crates/devices/tests/spectrum_analyser.rs`, `max_holds_the_highest_reading_until_cleared` | unit, real-time trap |
| R7 | `crates/shell/tests/device_displays.rs`, `the_spectrums_display_publishes_and_reads_headless` | unit |
| R8 | `crates/ui/tests/device_panel.rs`, `the_spectrums_cell_draws_in_docked_and_floating_windows` | UI script |
| R9 | `crates/devices/tests/rules.rs`, `every_parameter_is_automatable_to_the_frame`; `rtsan/tests/devices.rs`, `spectrum_passes_under_rtsan` | unit, RTSan |

## Defaults, ranges and details

- Parameters:
  - `Block`: Chooser, choices `["256", "512", "1024", "2048", "4096", "8192", "16384"]`, default 2048 (index 3).
  - `Channel`: Chooser, choices `["Left", "Right", "Both"]`, default Both (index 2).
  - `Refresh`: Continuous, range 10.0 ms to 1000.0 ms, default 50.0 ms.
  - `Avg`: Integer, range 1 to 32, default 1.
  - `Graph`: Chooser, choices `["Line", "Bins"]`, default Line (index 0).
  - `Max`: Toggle, default false.
  - `Scale X`: Chooser, choices `["Linear", "Logarithmic", "Semitone"]`, default Logarithmic (index 1).
  - `Range Mode`: Chooser, choices `["Range", "Auto"]`, default Range (index 0).
  - `Range Min`: Continuous (dB), range -150.0 dB to 0.0 dB, default -120.0 dB.
  - `Range Max`: Continuous (dB), range -60.0 dB to 12.0 dB, default 0.0 dB.
  - `Clear Max`: Toggle (action trigger to reset peak hold), default false.
- Beatlayer's own choices where the manual states no values: exact refresh range (10–1000 ms), average range (1–32), range min/max bounds (-150 to +12 dB).
