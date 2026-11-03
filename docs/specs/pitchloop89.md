# PitchLoop89: a two-voice pitch-shifting looping delay

- **Status:** draft
- **Work packages:** WP-8.52, with [Vector Delay](vector-delay.md) (all of it: R1 to R14). The framework is WP-2.5's ([the native device framework](device-framework.md)); its LFOs and seeded random source are WP-5.42's
- **Parity-matrix row:** Audio effects: Convolution Reverb, Color Limiter, Gated Delay, Pitch Hack, PitchLoop89, Re-Enveloper, Spectral Blur, Vector Delay
- **Checked against:** Ableton's PitchLoop89 pack page, read 30 October 2026; the Live 12 reference manual, which doesn't describe the device

## The behaviour

PitchLoop89 has two pitch-shifting delays. Each records the input into a short circular buffer and plays it back from a moving position at a speed and direction that set its pitch, so short snippets loop, transposed and reversed. The two voices run on the left and right, summed, or crossed, each with its own filters and feedback; Hold freezes a voice's buffer so it loops what it has; a vibrato LFO, a position LFO and a Random button move the playback.

Live's PitchLoop89 comes in its own pack as a Max device; Beatlayer's is native, with Live's name, Beatlayer's own type ID (`beatlayer.pitchloop89`) and look, and its controls drafted from the pack page.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Product page | Ableton, "PitchLoop89" pack page (ableton.com/en/packs/pitchloop89/), read 30 October 2026 | Two independent pitch-shifting delays, each holding short circular snippets, used apart or as one stereo effect; a bandwidth choice setting the snippet length, whose changes jump by octaves, low settings adding grit; playback speed and direction for pitch and reversal; a low-pass and high-pass per channel; routing L/R, Sum and Cross; feedback; Hold, which freezes and loops one or both channels; a vibrato LFO (sine or square); a position LFO with seven waveforms, a smooth random one among them; a Random button moving the position; its parameters in six banks |
| S2 | Beatlayer | [The device framework](device-framework.md) R4, R26 to R29; [the pitch and frequency shifter](pitch-and-frequency-shifter.md); [Grain Delay](grain-delay.md); [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1, G2 and IMP-08 | Smoothing, the effects LFO, seeds; delay-line pitch shifting; grains; banks; gains and automation; a delay's input reaching silence |

## Rules

- **R1.** PitchLoop89 (`beatlayer.pitchloop89`) is an audio effect with Live's name, Beatlayer's own layout and no display cell: two voice columns (A and B) and a shared column (Routing, Hold, the LFOs, Random, Input, Dry/Wet). (S1; the layout is Beatlayer's own)
- **R2.** Each voice writes the input (raised by Input, a gain from −∞ to +12 dB, 0 dB, rule G1 and IMP-08) into a circular buffer whose length is its Bandwidth (the read place stays at least 10 frames behind the write place, so a loop is Delay's share of the length plus that margin): 25, 50, 100, 200, 400 or 800 ms (100 ms by default); halving or doubling it moves a snippet's pitch by an octave against the same speed, as the page describes. (S1; the lengths are Beatlayer's own)
- **R3.** Each voice reads its buffer from Delay (0 to 100 % of the buffer behind the write position, 50 %) at Speed (−200 to +200 %, +100 %): the read position advances by Speed per frame relative to the write position, so +100 % is no transposition, +200 % an octave up, −100 % reversed at the original pitch; the voice's output is within 5 cents of 12 log2(|Speed|) semitones on a steady tone. When the read position reaches the write position it is crossfaded over 5 ms to Delay's position again, so the snippet loops. (S1; the law is Beatlayer's own)
- **R4.** Each voice has a 12 dB per octave low-pass (200 Hz to 20 kHz, 20 kHz) and high-pass (20 Hz to 5 kHz, 20 Hz), a Feedback (0 to 100 %, 0) returning its output to its buffer's input, and a Level (a gain from −∞ to 0 dB, rule G1). (S1; ranges are Beatlayer's own)
- **R5.** Routing L/R gives voice A the left input and output and B the right; Sum feeds both from the input's sum and mixes both to both outputs; Cross feeds each voice's feedback into the other voice's buffer. (S1; Cross's reading is Beatlayer's own)
- **R6.** Hold A and Hold B stop their voice's writing, so it loops what its buffer holds, for as long as Hold stays on. (S1)
- **R7.** The vibrato LFO (Sine or Square, 0.1 to 20 Hz, depth 0 to 100 cents) moves both voices' speed; the position LFO (Sine, Triangle, Saw Up, Saw Down, Rectangle, Sample & Hold and Wander, the last the smooth random "move" the page names; 0.01 to 20 Hz, depth 0 to 100 % of the buffer) moves both voices' Delay. Random, a button, draws a new Delay offset for each voice within the position LFO's depth, from the device's seed; it is a parameter, automatable, whose rising edge draws. (S1, S2; ranges and the Random law are Beatlayer's own)
- **R8.** Dry/Wet, 0 to 100 % (50 %), balances the dry signal against the voices. The device reports no latency. (S1; Beatlayer's own)
- **R9.** The buffers are made with the node for 800 ms at 192 kHz; nothing on the audio thread allocates, locks or blocks. (S2)
- **R10.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S2)
- **R11.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S2)
- **R12.** PitchLoop89 declares six banks of eight ([control surfaces](control-surfaces.md) R19), as the page's six banks: Main (both voices' Speed and Delay, Routing, Feedback A and B, Dry/Wet), Voice A, Voice B, Filters, Modulation and Global, so every automatable parameter is in a bank. (S1, S2; the banks' contents are Beatlayer's own)
- **R13.** A set with the device saves and reopens with every parameter. (S2)
- **R14.** No name of any hardware appears in the device, its parameters or its documentation beyond its own name, PitchLoop89. (Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | `crates/devices/tests/pitchloop89.rs`: `it_is_an_audio_effect_named_for_live_with_no_display`, `each_halving_of_bandwidth_loops_a_snippet_an_octave_higher` | unit (description; Bandwidth's octave steps) |
| R3 | `pitchloop89.rs`: `each_voice_transposes_within_five_cents_and_reads_backwards_below_zero` | unit (each voice's transposition within 5 cents at four speeds, reversed below 0) |
| R4 | `pitchloop89.rs`: `the_filters_cut_12_db_an_octave_and_level_scales_the_voice`, `feedback_returns_the_voice_to_its_buffer` | unit |
| R5 | `pitchloop89.rs`: `the_routings_send_the_voices_where_they_say` | unit (each routing's paths) |
| R6 | `pitchloop89.rs`: `hold_loops_what_the_buffer_holds_and_ignores_the_input_after` | unit |
| R7 | `pitchloop89.rs`: `the_vibrato_lfo_moves_the_pitch_at_its_rate_and_depth`, `the_position_lfo_moves_delay_at_its_rate_and_depth`, `random_draws_each_voices_delay_offset_from_the_devices_seed` | unit (the LFOs' rates; Random follows the seed) |
| R8 | `pitchloop89.rs`: `dry_wet_and_input_balance_the_signal_and_reach_silence` | unit |
| R9, R10 | `pitchloop89.rs`: `a_render_is_bit_identical_at_four_block_sizes_and_run_after_run` (every block under the allocation trap); `rtsan/tests/devices.rs`: `pitchloop89_passes_under_rtsan` | real-time trap; renders at four block sizes |
| R11, R13 | `crates/devices/tests/rules.rs` (G1, G2); `crates/cli/tests/agent.rs`: `an_agent_sets_pitchloop89_and_vector_delay_by_id_and_finds_them_after_a_reopen` | `rules.rs`; agent API; save and reopen |
| R12 | `crates/devices/tests/banks.rs` | banks |
| R14 | `cargo xtask private-terms`, which `cargo xtask check` runs | text check |
