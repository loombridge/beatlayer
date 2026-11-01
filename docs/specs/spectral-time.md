# Spectral Time: a spectral freezer and a spectral delay

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-8.47 (all of it: R1 to R20). The short-time Fourier transform and the spectrogram display kind are Beatlayer's own, `bl_dsp::stft` (WP-8.28, [the architecture](../02-architecture.md)) and [the device framework](device-framework.md)'s spectrogram rule; the framework is WP-2.5's, its seeded random source WP-5.42's
- **Parity-matrix row:** Audio effects: Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.37 "Spectral Time" with 29.37.1 to 29.37.4, read 30 October 2026; the Live 12.4.3 release notes

## The behaviour

Spectral Time works on the sound's spectrum, frame by frame. Its freezer captures a moment and holds it, by hand or retriggered by onsets in the input or on a synced interval, fading frozen spectra in and out or crossfading between them. Its delay delays each frequency band separately: Tilt delays the highs more than the lows or the reverse, Spray scatters the delay times at random, Mask limits the delay to the highs or the lows, Shift moves each repeat's pitch, and Feedback repeats them. The two sections run in either order. Resolution trades latency for accuracy, and a spectrogram shows the result.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.spectral_time`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.37.1 "Freezer Section" | Freezer On; the Freeze button; Manual and Retrigger modes; in Manual, Fade In and Fade Out in ms; in Retrigger, Onsets with Sensitivity (0 to 100 %) or Sync with an Interval in ms or beat time; the fade shape Crossfade (X-Fade, a percentage of the Interval) or Envelope (Fade In and Fade Out, up to eight freezes stacked) |
| S2 | Manual | Section 29.37.2 "Delay Section" | Delay On; Mode Time (ms), Notes (beat divisions), 16th, 16th Triplet and 16th Dotted; Time; Feedback; Shift; Stereo; the delay's own Dry/Wet; Tilt (positive delaying the highs more, negative the lows); Spray; Mask (positive the highs, negative the lows) |
| S3 | Manual | Sections 29.37.3 "Resolution" and 29.37.4 "Global Controls" | Resolution: lower, less latency and less accuracy; Input Send; the order Frz > Dly or Dly > Frz; Dry/Wet; Zero Dry Signal Latency in the context menu; the spectrogram |
| S4 | Release notes | Live 12.4.3 release notes | Control-surface mappings of the spectral devices improved |
| S5 | Beatlayer | `bl_dsp::stft`; [the device framework](device-framework.md) R4, R29, R35 to R38 and its spectrogram rule; [beat repeat](beat-repeat.md) (steps on the song's grid); [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | The STFT and its latency; seeds, displays, spectrograms; synced triggering; banks; gains and automation; a delay's input reaching silence |

## Rules

- **R1.** Spectral Time (`beatlayer.spectral_time`) is an audio effect with Live's name, Beatlayer's own layout and one display cell, the spectrogram (R16). Its body reads the freezer section, the delay section, the resolution section and the global controls. (S1 to S3)
- **R2.** The input, raised by Input Send (a gain from −∞ to +12 dB, 0 dB, rule G1 and IMP-08), is analysed by `bl_dsp::stft` with a Hann window at four times overlap and a frame size Resolution chooses: 256, 512, 1,024, 2,048 or 4,096 frames at 48 kHz (2,048 by default), scaled with the sample rate to the nearest power of two. The wet path's latency is the STFT's, one sample less than the frame size, reported for delay compensation. (S3, S5; the sizes and window are Beatlayer's own)
- **R3.** Every Resolution's analysis and resynthesis are made with the node; a change of Resolution starts the new size afresh (its delay and freezes empty), runs both sizes from then until the new one's first output, one latency later, and crossfades over 50 ms from the running size to the new one, and changes the reported latency, which makes a new program. Nothing is made on the audio thread. (S3, S5; Beatlayer's own)
- **R4.** With Zero Dry Signal Latency on (a switch in the context menu, saved with the device), the dry signal passes undelayed and the device reports no latency, so the wet signal arrives late by the analysis latency (R2); with it off, the dry signal is delayed to line up. (S3)

### The freezer

- **R5.** With Freezer On, pressing Freeze in Manual mode captures the spectrum of the next whole frame and holds its magnitudes, each bin's phase advancing each hop by the frequency of the spectral peak it belongs to, so the frozen sound sustains, within 0.5 dB of the captured magnitudes for as long as it is held, and the live sound passes in the measure the freeze is not yet in; Fade In (0 to 5,000 ms, 50 ms) and Fade Out (0 to 5,000 ms, 500 ms) fade it in on capture and out on release. Freeze is a switch parameter, automatable. (S1; ranges and defaults are Beatlayer's own)
- **R6.** In Retrigger mode, with Freeze on, Onsets captures a new frozen spectrum at each onset an online spectral-flux detector finds in the input, Sensitivity (0 to 100 %, 50 %) lowering its threshold; Sync captures one every Interval, a time (10 ms to 5 s) or a division (1/64 note to 4 bars) counted on the song's grid from the bar, landing on its frame through the tempo map at any block size. A capture takes the first whole frame that begins at or after its trigger; a retriggered freezer plays only what it has frozen, and fades it all out when Freeze goes off. (S1, S5; the detector, ranges and defaults are Beatlayer's own)
- **R7.** With fade shape Crossfade, each new frozen spectrum crossfades from the last over X-Fade (0 to 100 % of the Interval, 20 %), equal-power; with Envelope, each frozen spectrum fades in and out by Fade In and Fade Out and up to eight overlap, the oldest dropped when a ninth begins. (S1)

### The delay

- **R8.** With Delay On, each bin's magnitude and phase pass through a delay of whole hops: Mode Time takes Time (10 ms to 5 s, 250 ms), Notes a division (1/64 note to 4 bars), 16th, 16th Triplet and 16th Dotted 1 to 16 such sixteenths; synced times follow the tempo map. Each mode's time is a parameter of its own. A bin's echo of a generated tone lands within one hop of its time. (S2; ranges are Beatlayer's own)
- **R9.** Tilt, −100 to +100 % (0), scales each bin's delay by a factor rising linearly in log frequency from (1 − Tilt) at 20 Hz to (1 + Tilt) at 20 kHz, so positive Tilt delays the highs more. Spray, 0 to 100 % (0), adds to each bin's delay a random offset of up to ±Spray × the delay, a new draw per bin when the delay time or Spray changes, from the device's seed (device framework R29). (S2, S5; the laws are Beatlayer's own)
- **R10.** Mask, −100 to +100 % (0), limits the delay to part of the spectrum: positive Mask passes only the bins above a frequency moving from 20 Hz to 20 kHz in log frequency as Mask rises, negative only those below a frequency moving down; the bins left out are not delayed. (S2; the law is Beatlayer's own)
- **R11.** Feedback, 0 to 100 % (50 %), returns each bin's delayed output to its input, so each repeat's magnitude is Feedback times the last within 0.5 dB; Shift, −24 to +24 semitones (0), moves the fed-back spectrum's bins by that interval before each repeat, so repeats climb or fall. The loop's gain stays below 1. Stereo, 0 to 100 % (0), offsets the right channel's delays by up to half the delay time against the left's. The delay's Dry/Wet (0 to 100 %, 100 %) balances the section's input against its echoes. (S2; laws and ranges are Beatlayer's own)
- **R12.** The per-bin delay lines are made with the node for the longest time at the slowest tempo the transport allows at the finest Resolution, capped at 8 seconds of spectral frames; a time beyond the cap is held at it, and the parameter that sets the time now reads "(held at 8 s)" in `device.parameters` while the memory can't hold it. (S5; the cap is Beatlayer's own)

### The order and global controls

- **R13.** The order Frz > Dly runs the freezer into the delay; Dly > Frz the delay into the freezer. With either section off it is skipped. (S3)
- **R14.** Dry/Wet, 0 to 100 % (100 %), balances the dry signal against the wet. (S3)
- **R15.** A frozen spectrum, the delay's state and every random draw depend only on the input, the parameters, the song position and the seed, so a render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S5)

### The spectrogram

- **R16.** The spectrogram draws the wet output's magnitudes over the last four seconds against log frequency, in Beatlayer's own colour ramp, as [the device framework](device-framework.md)'s spectrogram kind lays it out; `device.display` reads its grid as numbers headless. (S3, S5)

### Throughout

- **R17.** Nothing on the audio thread allocates, locks or blocks. (S5)
- **R18.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S5)
- **R19.** Spectral Time declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Freezer On, Freeze, Delay On, Time, Feedback, Tilt, Spray, Dry/Wet), Freezer (Mode, Trigger, Sensitivity, Interval Mode, Interval Time, Interval Division, Fade Shape, X-Fade), Fades, Delay (Delay Mode, Notes, 16th, 16th Triplet, 16th Dotted, Mask, Shift, Stereo), Delay Mix and Global (Order, Resolution, Input Send, Dry/Wet), so every automatable parameter is in a bank. (S4, S5; Beatlayer's own)
- **R20.** A set with Spectral Time saves and reopens with every parameter and Zero Dry Signal Latency. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/devices/tests/spectral_time.rs`, `spectral_time_is_an_audio_effect_with_lives_name_and_a_spectrogram_cell` | unit |
| R2 | `crates/devices/tests/spectral_time.rs`, `the_reported_latency_is_the_analysis_frame_less_one_at_each_resolution`, `input_send_at_minus_infinity_leaves_the_wet_path_silent` | unit, real-time trap |
| R3 | `crates/devices/tests/spectral_time.rs`, `a_change_of_resolution_crossfades_makes_nothing_and_reports_the_new_latency` | unit, real-time trap |
| R4 | `crates/devices/tests/spectral_time.rs`, `zero_dry_signal_latency_passes_the_dry_signal_undelayed_and_reports_none` | unit, real-time trap |
| R5 | `crates/devices/tests/spectral_time.rs`, `a_frozen_spectrum_holds_its_level_within_half_a_decibel_for_thirty_seconds`, `freeze_fades_in_over_fade_in_and_out_over_fade_out` | unit, real-time trap |
| R6 | `crates/devices/tests/spectral_time.rs`, `retrigger_refreezes_on_each_onset_in_the_input`, `retrigger_sync_captures_on_the_songs_grid_at_any_block_size`, `retrigger_sync_in_milliseconds_follows_the_song_not_the_blocks`, `sensitivity_lowers_the_threshold_an_onset_must_cross` | unit, real-time trap |
| R7 | `crates/devices/tests/spectral_time.rs`, `crossfade_trades_one_frozen_spectrum_for_the_next_at_equal_power`, `envelope_stacks_up_to_eight_freezes_and_a_ninth_drops_the_oldest` | unit, real-time trap |
| R8 | `crates/devices/tests/spectral_time.rs`, `an_echo_lands_within_one_hop_of_its_time_in_every_mode`, `a_synced_time_follows_the_tempo` | unit, real-time trap |
| R9 | `crates/devices/tests/spectral_time.rs`, `tilt_delays_the_highs_more_or_the_lows_more_as_its_sign_says`, `spray_scatters_each_bins_delay_the_same_way_every_run` | unit, real-time trap |
| R10 | `crates/devices/tests/spectral_time.rs`, `mask_limits_the_delay_to_the_highs_or_the_lows` | unit, real-time trap |
| R11 | `crates/devices/tests/spectral_time.rs`, `each_repeat_is_feedback_times_the_last_to_within_half_a_decibel`, `shift_moves_each_repeat_by_its_interval_but_not_the_first`, `stereo_delays_the_right_channel_up_to_half_the_time_longer`, `the_delays_own_dry_wet_balances_its_input_against_its_echoes` | unit, real-time trap |
| R12 | `crates/devices/tests/spectral_time.rs`, `a_time_beyond_the_delay_memory_is_held_at_it`; `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_spectral_time_by_id_and_reopens_it` | unit, agent API |
| R13 | `crates/devices/tests/spectral_time.rs`, `the_order_switch_runs_the_freezer_into_the_delay_or_the_delay_into_the_freezer`, `with_a_section_off_it_is_skipped_in_either_order` | unit, real-time trap |
| R14 | `crates/devices/tests/spectral_time.rs`, `dry_wet_balances_the_dry_signal_against_the_wet` | unit, real-time trap |
| R15 | `crates/devices/tests/spectral_time.rs`, `a_render_is_the_same_to_the_bit_at_blocks_of_1_64_512_and_4096_and_run_after_run` | unit (golden renders at four block sizes) |
| R16 | `crates/devices/tests/spectral_time.rs`, `the_spectrogram_shows_the_wet_output_against_log_frequency_over_four_seconds`; `crates/cli/tests/agent.rs`, `an_agent_reads_spectral_times_spectrogram_through_device_display`; `crates/ui/tests/device_panel.rs`, `spectral_times_cell_draws_its_spectrogram_as_one_image` | unit, agent API (`device.display`), UI script |
| R17 | every test above under the allocation trap; `rtsan/tests/devices.rs`, `spectral_time_passes_under_rtsan` | real-time trap, RealtimeSanitizer |
| R18 | `crates/devices/tests/rules.rs`; `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_spectral_time_by_id_and_reopens_it` | unit, agent API |
| R19 | `crates/devices/tests/banks.rs` | unit |
| R20 | `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_spectral_time_by_id_and_reopens_it` | agent API, save and reopen |
