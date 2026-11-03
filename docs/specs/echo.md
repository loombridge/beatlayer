# Echo: two delay lines with modulation, character and a reverb

- **Status:** implemented
- **Work packages:** WP-8.37 (all of it: R1 to R24). The framework it sits on is WP-2.5 ([the native device framework](device-framework.md)); its LFO, envelope follower and seeded random source are WP-5.42's (device framework R26 to R29); its display path is WP-5.43's (device framework R35 to R38); synced delay times are [the delay](delay.md)'s and [Filter Delay](filter-delay.md)'s way
- **Parity-matrix row:** Audio effects: Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.14 "Echo" with 29.14.1 to 29.14.4, read 30 October 2026

## The behaviour

Echo is a modulated delay with the character of tape and analogue units. Two delay lines, left and right (or mid and side), each take a beat division or a time in milliseconds; their echoes are drawn as a tunnel on its display, where dragging sets the times. The input can be driven into a clip, the feedback inverted, and the repeats filtered by a high-pass and a low-pass with resonance. Three tabs hold the rest: Echo (the filters and the tunnel), Modulation (an LFO and an envelope moving the delay time and the filter) and Character (a gate and ducking that follow the input, noise and wobble that age the sound, and Repitch, which glides the pitch when the time changes). A small reverb sits before the delay, after it, or inside its feedback loop.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.echo`, its own look and its own tunnel. Its input reaches silence (rule G1, IMP-08).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.14 "Echo" | Channel Mode (Stereo, Ping Pong, Mid/Side); left and right delay lines (mid and side in Mid/Side), each in beat divisions or milliseconds with its Sync switch and Sync Mode (Notes, Triplet, Dotted, 16th); Stereo Link; a Delay Offset slider per channel that swings the time; Input with a D button that drives the dry signal; Feedback with Ø, which inverts it |
| S2 | Manual | Section 29.14.1 "Echo Tab" | The Echo Tunnel shows the echoes and its handles drag the delay times; Filter switches the HP and LP filters, each with its cutoff and resonance (HP Res, LP Res); a button shows the filter's display in place of the tunnel |
| S3 | Manual | Section 29.14.2 "Modulation Tab" | Waveform (Sine, Triangle, Sawtooth Up, Sawtooth Down, Square, Noise); Sync with Rate in beat divisions, or Freq in Hz; Phase; Mod Delay, with Modulation ×4 multiplying its depth by four; Mod Filter; Env Mix, from the LFO alone at 0 % to the envelope alone at 100 % |
| S4 | Manual | Section 29.14.3 "Character Tab" | Gate (Threshold, Release); Ducking (Threshold, Release); Noise (Amount, Morph) and Wobble (Amount, Morph), each with an on switch; Repitch, a pitch glide when the delay time changes |
| S5 | Manual | Section 29.14.4 "Global Controls" | Reverb, with its location before the delay, after it, or in the feedback loop, and Decay; Stereo, mono at 0 % and wider above 100 %; Output; Dry/Wet, with Equal-Loudness in its context menu |
| S6 | Beatlayer | [The device framework](device-framework.md) R4, R8, R26 to R29, R35 to R38; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | Smoothing, the oversampler, the effects LFO, follower and seeded random source, display data, banks of eight; every gain reaches −∞ dB, every parameter is automatable, a delay's input reaches silence |
| S7 | Beatlayer | [The delay](delay.md) R1 and R6; [Filter Delay](filter-delay.md) R2 and R3; [the musical time spec](musical-time.md) | Synced times through the tempo map; Repitch as a glide of the read position |

## Rules

### The delay lines

- **R1.** Echo (`beatlayer.echo`) is an audio effect with Live's name, Beatlayer's own layout and one display cell. Its body is the left section (the two delay lines' controls, Input and Feedback), the tabs Echo, Modulation and Character with the display, and the global controls (Reverb, Decay, Stereo, Output, Dry/Wet). The tabs are one pages node, `Tabs`, in the middle of the body ([the device framework](device-framework.md) R41 to R45): choosing a tab is view state, saved with the set and not an undo step, and `device.set_page` chooses it. The display offers two views, `Tunnel` and `Filter`, which `device.set_display_view` chooses; Equal-Loudness is in the device's menu of options. (S1 to S5; saving the tab with the set is Beatlayer's own)
- **R2.** Each line's time is, with its Sync on, a division counted as Sync Mode says: Notes (1/64 note to 16 bars: 1/64, 1/32, 1/16, 1/8, 1/4, 1/2, 1, 2, 4, 8 and 16 bars), Triplet and Dotted (the same divisions, two thirds and one and a half times as long), or 16th (1 to 16 sixteenth notes); a bar is four quarter notes; and with Sync off, a time from 1 ms to 5 s, logarithmic. Each mode's amount is a parameter of its own. A synced echo lands on the exact frame its division names through the tempo map, at any tempo and across a tempo change. (S1, S7; the ranges are Beatlayer's own)
- **R3.** Stereo Link gives the right line the left line's Sync, Sync Mode and time; each line's Delay Offset, −33 to +33 % of its division, stays its own and moves its echoes later or earlier, as a swing. Offset applies only while the line is synced. (S1; the range is Beatlayer's own)
- **R4.** Channel Mode Stereo runs the left input through the left line and the right through the right. Ping Pong sums the input into the left line, whose output feeds the right line, whose output returns to the left through Feedback, so the echoes alternate sides as [the delay](delay.md) R4's do (the first echo on each side is at full level, and Feedback returns the right line's output to the left). Mid/Side encodes the input to mid and side, runs mid through the left line and side through the right, and decodes after; the lines' controls then read Mid and Side. (S1)
- **R5.** Input is a gain from −∞ to +12 dB, 0 dB by default, on the signal entering the lines (rule G1, IMP-08): at −∞ nothing new enters and the echoes already in the lines play out. With D on, the signal after Input goes through a soft clip before the lines and to the dry path too, the curve tanh(x) normalised to unit slope at zero. (S1, S6; the range and the clip's curve are Beatlayer's own)
- **R6.** Feedback, 0 to 100 %, returns each line's output to its input through the filters, the reverb when it is in the loop (R16) and the character stages; Ø inverts the returned signal. Whatever the settings, the returned signal is held below 1 by a soft limit at 0 dBFS, linear up to −6 dBFS and bending toward 0 dBFS above it, so the echoes never grow without bound. (S1; the limit is Beatlayer's own)

### The Echo tab

- **R7.** With Filter on, each loop has a 12 dB per octave high-pass (HP, 20 Hz to 20 kHz, 20 Hz by default) and low-pass (LP, 20 Hz to 20 kHz, 20 kHz by default) in series, each with its resonance (HP Res and LP Res, 0 to 100 %, a Q from 0.5 at 0 % to 10 at 100 %, evenly on a log scale). Each filter's response, measured on the first echo of an impulse, is within 0.5 dB of its design from 20 Hz to 20 kHz. With Filter off they pass everything. (S2; the slopes, ranges and Q law are Beatlayer's own)
- **R8.** The display draws the tunnel: one ring per echo, its size falling with the echo's level and its place across the cell by its channel, for the first 16 echoes of each line as the times and Feedback give them, with a handle for each line's time, low in the cell, the right line's a little above the left's, whose place across it is the travel of the parameter that sets the time (the division of the line's Sync Mode, or the free time); dragging a handle sideways sets that line's time as one undo step, and with Stereo Link the right line's handle sets the left line's time too. The tunnel draws an echo only while its level is above −60 dB. The filter-display button draws the two filters' response instead, with a handle on each cutoff, dragging which sets the cutoff and, vertically, the resonance, in one undo step; the arrow keys and a screen reader's steps move a handle by a step of its parameter. The display data (each line's echo times and levels, the filter curve) is published as device framework R35 says and read by `device.display`, whose two views are `tunnel` and `filter`. (S2, S6; the look is Beatlayer's own)

### The Modulation tab

- **R9.** The LFO's Waveform is Sine, Triangle, Sawtooth Up, Sawtooth Down, Square or Noise (the effects LFO's Sine, Triangle, Saw Up, Saw Down, Rectangle and Random, device framework R26). With Sync on, its Rate is a division from 1/64 note to 8 bars, its cycles on the beat grid as device framework R27 says; with Sync off, Freq runs from 0.01 to 40 Hz. Phase, 0 to 360°, puts the right line's LFO ahead of the left's. (S3, S6; the ranges are Beatlayer's own)
- **R10.** The modulation source is the LFO and an envelope follower of the input (attack 10 ms, release 200 ms, device framework R28) mixed by Env Mix: the LFO alone at 0 %, the envelope alone at 100 %. Mod Delay, 0 to 100 %, moves each line's delay time by up to ±2 % of the time times the source (the envelope's level read from −60 dBFS at 0 to 0 dBFS at 1), or ±8 % with Modulation ×4 on; Mod Filter, 0 to 100 %, moves both filters' cutoffs by up to ±2 octaves times the source. (S3, S6; the depths and the follower's times are Beatlayer's own)

### The Character tab

- **R11.** With Gate on, the signal entering the lines passes only while the input's level is above Gate Threshold (−60 to 0 dBFS, −30 dBFS by default), and closes over Gate Release (1 to 1,000 ms, 100 ms) once it falls below, the level read by an envelope follower of the input's peak with a 1 ms attack and a 10 ms release, and the gate opens over 1 ms and closes in a straight line over Gate Release. (S4; ranges, defaults and the follower are Beatlayer's own)
- **R12.** With Ducking on, the wet signal is lowered while the input's level is above Ducking Threshold (−60 to 0 dBFS, −30 dBFS), by up to 30 dB, one dB for each dB the input is above it, reached over 1 ms, and comes back over Ducking Release (1 to 1,000 ms, 200 ms), the time the lowering takes to fall to 1/e of what it was. The detector is the gate's, and reads the input, never the echoes. (S4; the depth law, ranges and defaults are Beatlayer's own)
- **R13.** With Noise on, a hiss is added inside the loop at Amount (0 to 100 %, up to −40 dBFS at its peak), its colour moved by Morph (0 % white, 100 % a 6 dB per octave fall above 1 kHz, a one-pole low-pass, the two blended between). With Wobble on, the delay time is moved by a slow irregular motion, a sum of a 0.5 Hz and a 3.3 Hz sine and a random line ([the device framework](device-framework.md) R26's Wander at 2 Hz), by up to ±0.5 % of the time at Amount 100 %, Morph moving the balance from the slow motion to the fast. Both follow the device's seed (device framework R29), so a render repeats run after run. (S4, S6; the laws are Beatlayer's own)
- **R14.** With Repitch on, a change of delay time moves the read position to the new time over 200 ms at a constant rate, so the echoes glide in pitch as tape does, interpolating between frames so no alias passes −80 dB below the signal; with it off, a change crossfades from the old time to the new over 50 ms with no change of pitch. (S4, S7; the times are Beatlayer's own)

### The global controls

- **R15.** Reverb, 0 to 100 %, mixes a small reverb into the wet signal, crossfading from the signal to the reverb, whose output has the energy of its input; Decay, 0.2 to 10 s, is its reverberation time, within 10 % measured as RT30 on its own output. The reverb is Beatlayer's own, built from the parts of [the reverb](reverb.md). (S5; the design and ranges are Beatlayer's own)
- **R16.** The reverb's location is Pre (before the delay lines, so the echoes repeat a reverberated signal), Post (after them, on their output) or Feedback (inside the loop, so each echo is more diffuse than the last); in Feedback the loop's gain stays below 1 as R6 says. (S5)
- **R17.** Stereo, 0 to 200 %, sets the wet signal's width by its mid and side: mono at 0 %, unchanged at 100 %, the side doubled at 200 %. Output is a gain from −∞ to +12 dB on the wet signal (rule G1). Dry/Wet, 0 to 100 %, balances the dry signal against the wet; with Equal-Loudness on (Dry/Wet's context menu, a switch saved with the device) the two follow sine and cosine laws, so the sum of a steady noise stays within 0.5 dB at every setting. (S5, S6; the laws are Beatlayer's own)

### Throughout

- **R18.** The delay lines are made with the node, long enough for the longest time at the slowest tempo the transport allows with the largest offset and modulation; nothing Echo does on the audio thread allocates, locks or blocks. (S6)
- **R19.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, with every modulation, Noise and Wobble on. (S6)
- **R20.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API (`device.insert`, `device.set_param`, `device.parameters`); every gain parameter reaches −∞ dB. (S6)
- **R21.** Echo declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (the two times, Sync, Feedback, Input, Channel Mode, Reverb, Dry/Wet), Filter, Modulation, Character and Global, so every automatable parameter is in a bank. (S6; the banks are Beatlayer's own)
- **R22.** Echo reports no latency. (Beatlayer's own)
- **R23.** A set with Echo saves and reopens with every parameter, its tab and Equal-Loudness. (S6)
- **R24.** Every control takes the keyboard focus in the Device View, the tabs among them, and assistive technology reads each with its name and value. (S6)

## Tests

Unless it names another file, a test is in `crates/devices/tests/echo.rs`.

| Rule | Test | Kind |
|---|---|---|
| R1 | `echo_is_described_with_its_tabs_its_views_and_its_banks`; `crates/ui/tests/echo.rs`, `the_tabs_switch_the_controls_shown_and_are_no_undo_step` | unit (description, layout); UI script (the tabs) |
| R2 | `a_synced_echo_lands_on_its_frame_at_every_block_size`, `a_free_echo_lands_on_its_frame_at_every_block_size`, `a_synced_echo_follows_a_tempo_change_at_every_block_size`, `the_sync_modes_count_their_divisions` | real-time trap (blocks of 1, 64, 512 and 4,096) |
| R3 | `stereo_link_gives_the_right_line_the_left_time_and_offsets_stay_apart` | real-time trap |
| R4 | `ping_pong_alternates_the_echoes_between_the_sides`, `mid_side_runs_the_mid_and_the_side_through_the_lines_and_round_trips` | real-time trap |
| R5 | `input_at_minus_infinity_lets_the_echoes_already_in_the_lines_play_out`, `d_drives_the_signal_after_input_into_a_clip_on_both_paths`; `crates/devices/tests/rules.rs`, `every_gain_parameter_reaches_minus_infinity` | real-time trap |
| R6 | `feedback_returns_the_echo_and_invert_flips_it`, `the_loop_stays_bounded_at_a_hundred_per_cent_with_every_stage_on`; `crates/devices/src/echo.rs`, `the_limit_is_linear_below_the_knee_and_never_passes_one` | real-time trap, unit |
| R7 | `the_filters_measured_responses_are_within_half_a_decibel_of_their_design`, `with_filter_off_the_loop_passes_everything` | real-time trap |
| R8 | `the_tunnel_shows_each_lines_echoes_as_the_times_and_feedback_give_them`, `a_tunnel_handle_drags_the_parameter_that_sets_its_lines_time`, `the_filter_view_draws_the_two_filters_response_with_a_handle_on_each_cutoff`; `crates/shell/tests/echo.rs`, `the_lines_times_are_published_after_every_block_at_the_tempo`, `an_agent_reads_the_tunnel_and_the_filter_view_by_device_id`; `crates/ui/tests/echo.rs`, `dragging_a_tunnel_handle_sideways_sets_that_lines_time_as_one_undo_step`, `with_both_lines_at_the_same_time_each_handle_can_still_be_taken`, `the_filter_views_handles_set_the_cutoff_sideways_and_the_resonance_up` | unit; agent API; UI script (a handle drag is one undo step) |
| R9, R10 | `a_synced_lfo_moves_the_time_on_the_beat_and_x4_multiplies_the_depth`, `env_mix_runs_from_the_lfo_alone_to_the_envelope_alone`, `mod_filter_moves_both_cutoffs_with_the_source` | real-time trap |
| R11 | `the_gate_follows_a_burst_with_its_threshold_and_release` | real-time trap |
| R12 | `ducking_lowers_the_wet_signal_in_proportion_and_comes_back_over_its_release`, `ducking_reads_the_input_and_never_the_echoes` | real-time trap |
| R13 | `noise_and_wobble_repeat_run_after_run_and_follow_the_device`, `noise_morph_tilts_the_hiss_down_above_a_kilohertz`, `wobble_moves_the_time_by_a_fraction_of_a_per_cent` | real-time trap |
| R14 | `repitch_glides_the_pitch_with_no_alias_and_off_it_crossfades_at_the_same_pitch` | real-time trap |
| R15 | `crates/devices/src/echo.rs`, `the_reverbs_rt30_is_within_ten_per_cent_of_decay` | unit (RT30 on the reverb's own output) |
| R16 | `the_reverb_before_and_after_the_lines_is_the_same_and_in_the_loop_it_diffuses_each_echo` | unit |
| R17 | `stereo_sets_the_width_of_the_wet_signal`, `output_scales_the_wet_signal_and_equal_loudness_keeps_the_sum_steady` | unit |
| R18, R19 | `a_render_is_the_same_at_four_block_sizes_with_every_stage_on_and_the_tempo_moving`; `crates/devices/src/echo.rs`, `the_lines_hold_the_longest_time_at_the_slowest_tempo_with_everything_at_its_deepest`; `crates/cli/tests/golden.rs`, the `echo` fixture; `rtsan/tests/devices.rs`, `echo_passes_under_rtsan` | real-time trap, RealtimeSanitizer, golden render |
| R20, R21 | `crates/devices/tests/rules.rs` (every parameter automatable and modulatable, every gain to −∞ dB, every parameter described); `crates/devices/tests/banks.rs`; `crates/cli/tests/agent.rs`, `an_agent_adds_sets_and_reads_echo_by_id_reads_its_displays_and_reopens_it` | rules over every device type; agent API |
| R22 | `echo_reports_no_latency` | unit |
| R23 | `crates/shell/tests/echo.rs`, `a_set_with_echo_saves_and_reopens_with_every_parameter_and_its_tab`; the agent test above | save and reopen |
| R24 | `crates/ui/tests/echo.rs`, `a_handle_takes_the_keyboard_and_steps_with_the_arrows`; `crates/ui/tests/device_pages.rs` (the tabs' tab list and arrow keys) | UI script with the accessibility tree |
