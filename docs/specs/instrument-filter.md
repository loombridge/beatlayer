# The instruments' filter: types, slopes, the Clean, OSR, MS2, SMP and PRD circuits, Drive and Morph

- **Status:** partial (Simpler and Sampler run on it, WP-8.3; the circuit chooser's greying, Drive's greying and Simpler's Morph snapping menu are not built)
- **Work packages:** WP-8.3 (this spec, R1 to R17, with Simpler and Sampler moved onto it). Operator ([operator](operator.md)), Wavetable ([wavetable](wavetable.md)), Bass and Poli ([bass and poli](bass-and-poli.md)) and Meld's MS2 and OSR types ([meld](meld.md)) use it from their own packages. Auto Filter's circuits (SVF, DFM, MS2 and PRD) are a different set, its own ([Auto Filter](auto-filter.md) R3 to R5); Analog's and Tension's multimode filters are their own ([analog](analog.md), [tension](tension.md)); Drift's is [polysynth](polysynth.md) R8
- **Parity-matrix row:** Instruments: Simpler; Sampler (multisample, zones, modulation); Operator; Wavetable (their filters)
- **Checked against:** the Live 12 reference manual (online edition), read 30 October 2026 (chapter 31, sections 31.9.5 and 31.9.10.4, 31.10.8, 31.11.3 and 31.13.4); the Live 12 release notes, 12.2 (Auto Filter's circuits described)

## The behaviour

Simpler, Sampler, Operator and Wavetable share one filter in Live, and Beatlayer gives them one filter too. It runs per voice and offers five types, low-pass, high-pass, band-pass, notch and Morph, each at 12 or 24 dB per octave, and a choice of circuit that decides how it behaves when it is pushed: Clean, a filter with no distortion of its own, offered on every type; OSR, a state-variable filter whose resonance is held in by a hard clip, on every type; and MS2, SMP and PRD, three circuits for low-pass and high-pass only. MS2 is a Sallen-Key design that clips softly; PRD is a ladder with no limit of its own on its resonance; SMP is a design of its own that sits between the two. Drive pushes the signal into every circuit but Clean, on low-pass, high-pass and band-pass. Morph sweeps the Morph type from low-pass through band-pass, high-pass and notch and back to low-pass.

At low levels every circuit is its own linear filter and follows that filter's response; the circuits part only as the signal and the resonance rise, each by its own nonlinearity. Every law here is Beatlayer's own reading of the manual's one-line descriptions, built and measured as Auto Filter's circuits are.

This spec covers the filter. What modulates its cutoff in each device (key tracking, the filter envelope, velocity, an LFO, the matrix) is each device's spec's.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 31 "Live Instrument Reference", section 31.11.3 "Filter" (Simpler) and section 31.10.8 "The Filter/Global Tab" (Sampler) | Low-pass, high-pass, band-pass, notch and Morph, each at 12 or 24 dB, with the circuit behaviours Clean, OSR, MS2, SMP and PRD; Frequency and Resonance; Drive for the non-Clean low-pass, high-pass and band-pass; Morph sweeping low-pass, band-pass, high-pass, notch and back |
| S2 | Manual | Section 31.9.5 "Filter Section" and 31.9.10.4 "Filter Shell and Display" (Operator) | The same types, slopes and circuits; Clean is available for every type and matches EQ Eight; OSR for every type; MS2, SMP and PRD for low-pass and high-pass only; Drive adds gain before the filter for the non-Clean circuits on low-pass, high-pass and band-pass; the Morph control's context menu snaps it to a fixed type; Resonance is a bandwidth for band-pass and notch |
| S3 | Manual | Section 31.13.4 "Filters" (Wavetable) | OSR is a state-variable filter with a hard-clipping limit on its resonance; MS2 a Sallen-Key design with soft clipping; SMP a custom design with traits of MS2 and PRD; PRD a ladder design with no explicit resonance limit; Drive adds gain or distortion before the filter |
| S4 | Release notes | Live 12 release notes, 12.2 (Auto Filter) | MS2 is a Sallen-Key design that soft-clips; PRD is a ladder with no explicit resonance limiting |
| S5 | Beatlayer | [Auto Filter](auto-filter.md) R3 to R5; [the sampler](sampler.md) R15 and R16; [the multisampler](multisampler.md) R37 to R39 | How Beatlayer builds circuits by softening a section's integrators and measures them; Simpler's and Sampler's filters as built before this spec, the kit's state-variable filter |
| S6 | Beatlayer | [The device framework](device-framework.md) R4, R6 and R9; [the architecture](../02-architecture.md), "Real-time rules" and "Capacity" | Per-frame parameter changes, smoothing, no allocation on the audio thread, the device budget |

## Rules

**Types, slopes and circuits**

- **R1.** The filter's Type is low-pass, high-pass, band-pass, notch or Morph, and its Slope 12 or 24 dB per octave, for every type. Frequency is logarithmic from 20 Hz to 20 kHz and Resonance runs from 0 to 100 %; for band-pass and notch, Resonance narrows the band. A device states its own defaults. (S1, S2)
- **R2.** Its Circuit is Clean, OSR, MS2, SMP or PRD. Clean and OSR play on every type; MS2, SMP and PRD on low-pass and high-pass only. The circuit chooser offers only the circuits the chosen type plays; a type change that leaves the circuit unoffered keeps the Circuit parameter's value and plays Clean until a type that offers it is chosen again, and the chooser shows the unoffered circuit greyed (Beatlayer's own). (S1, S2)
- **R3.** Clean is the plug-in kit's state-variable filter, as Simpler and Sampler played before this spec: the same coefficients and state for the same Frequency, Resonance, Type and Slope, so a render through Clean equals one made before this spec sample for sample. Its Morph type runs the four kit types it sweeps through, low-pass, band-pass, high-pass and notch, and crossfades the two neighbours of its place, as Sampler's Morph did before this spec. Clean has no Drive and no distortion of its own. (S2, S5)
- **R4.** At small signal (an input at −40 dBFS and below) every circuit is a linear filter and follows its own linear prototype on the warped frequency axis within 0.5 dB from 20 Hz to 20 kHz, at cutoffs of 100 Hz, 1 kHz and 10 kHz and Resonance 0 %, 50 % and 90 %, at 44.1 kHz and at 48 kHz: Clean and OSR a state-variable section (two cascaded for 24 dB), MS2 and SMP a Sallen-Key section (two cascaded for 24 dB), PRD a four-pole ladder (its second pole's output for 12 dB). (S2, S3, S5; the prototypes are Beatlayer's own readings)
- **R5.** OSR's state-variable section clips the state in its resonance path hard at ±1: as the level or the resonance rises, its resonant peak stops growing at that ceiling and its tone hardens, and its output never exceeds +6 dBFS whatever the input, Drive and Resonance. (S3; the ceiling is Beatlayer's own)
- **R6.** MS2's Sallen-Key section passes its feedback through a soft clipper (tanh): as the level or the resonance rises, the peak rounds off gradually, and at 100 % Resonance with no input it rings down rather than oscillating on, its peak held below +6 dBFS. (S3, S4; the clipper and the bound are Beatlayer's own)
- **R7.** PRD's ladder saturates only its input stage and puts no limit on its feedback: at high Resonance it self-oscillates at the cutoff within 50 cents of it, at a level its stages' own saturation sets, and loud input pulls the resonance down less than MS2's does. Its output never exceeds +12 dBFS. (S3, S4; Beatlayer's own bound)
- **R8.** SMP is a Sallen-Key section with PRD's input saturation in front and MS2's soft clipper in its feedback set to clip later, so its distortion at a given level and Resonance lies between MS2's and PRD's: measured with a 100 Hz sine at −6 dBFS through a 1 kHz low-pass at 70 % Resonance and 12 dB Drive, its total harmonic distortion is above MS2's and below PRD's. (S3; Beatlayer's own reading)
- **R9.** Drive, from 0 to 24 dB (0 dB by default), is a gain on the signal into the circuit for OSR, MS2, SMP and PRD on low-pass, high-pass and band-pass; it does nothing on Clean, on notch and on Morph, and the device shows it greyed there. The circuit's output isn't scaled back, so at small signal Drive raises the level by its gain. (S1, S2, S3; Beatlayer's own range, Sampler's, and law)
- **R10.** Morph, from 0 to 100 % (0 % by default), sweeps the Morph type from low-pass at 0 % through band-pass at 25 %, high-pass at 50 % and notch at 75 % back to low-pass at 100 %, each stop the pure type and each place between a crossfade of its two neighbours; its context menu snaps it to any of the four. It plays with Clean and OSR: Clean crossfades the finished types, OSR blends the four taps of each of its sections. (S1, S2, S5)

**Per voice, per frame**

- **R11.** The filter runs per voice, every voice in the plug-in kit's lanes, and reads Frequency, Resonance, Morph and Drive per frame from each voice's modulated values, so a modulation of the cutoff at audio rate is heard at audio rate. A change of Type, Slope or Circuit lands on its frame without a click: the new structure starts from silence and the old one fades out under it over 5 ms, each running its own state. (S5, S6; the crossfade is Beatlayer's own)
- **R12.** The filter is stable at every setting from 44.1 to 192 kHz: the cutoff is prewarped and clamped to 0.45 of the sample rate, so a 20 kHz cutoff at 44.1 kHz stays stable at full Resonance; denormals are flushed in every state, so a long silence after loud input costs no more than silence. (S6)
- **R13.** Sixteen voices through the filter with any circuit render inside the architecture's device budget, and nothing the filter does on the audio thread allocates, locks or blocks; a voice's filter output is the same at any block size. (S6)

**The devices**

- **R14.** Simpler, Sampler, Operator and Wavetable offer the whole filter (R1 to R13). Simpler's filter gains Circuit, Drive and the Morph type with its Morph control; Sampler's gains Circuit. Bass, Poli and Meld use the parts their specs name. (S1, S2, S3)
- **R15.** A set saved before this spec opens with every Simpler and Sampler filter on Clean, Drive at 0 dB and Morph at 0 %, and renders as it did, sample for sample; nothing in an older set changes when it is opened or saved again. (S5)
- **R16.** Type, Slope, Circuit, Frequency, Resonance, Drive and Morph are each device's parameters: automatable, modulatable (rule G2), set by `device.set_param` and read by `device.parameters`. (S6)
- **R17.** Every circuit is built and measured by Beatlayer's own method, its own code from published filter structures (the state-variable filter, the Sallen-Key section and the transistor ladder in their zero-delay-feedback forms), never from Live's sound. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | `crates/devices/tests/instrument_filter.rs`, `the_chooser_offers_only_the_circuits_a_type_plays`; `crates/devices/tests/sampler.rs`, `simplers_circuits_drive_and_morph_play`; `crates/agent/tests/instrument_filter.rs`, `an_agent_sets_and_reads_simplers_circuit_drive_morph_and_types` | unit (the offered list; a circuit a type doesn't offer plays Clean); engine test; agent API |
| R3, R15 | `crates/devices/tests/instrument_filter.rs`, `clean_is_the_kits_filter_sample_for_sample`; `crates/devices/tests/sampler.rs`, `an_older_sets_filter_renders_as_it_did`; `crates/devices/tests/multisampler.rs`, `an_older_sets_filters_render_as_they_did` | unit; golden renders (hashes of Simpler and Sampler renders made before the change, sample for sample) |
| R4 | `crates/devices/tests/instrument_filter.rs`, `every_circuit_follows_its_prototype_within_half_a_db_at_small_signal` | unit under the real-time trap (impulse responses at −40 dBFS, read by DFT against each prototype, where the response is above −50 dB) |
| R5 | `crates/devices/tests/instrument_filter.rs`, `osr_holds_its_peak_and_stays_under_plus_six_dbfs` | unit under the real-time trap |
| R6 | `crates/devices/tests/instrument_filter.rs`, `ms2_rounds_its_peak_off_rings_down_and_stays_under_plus_six_dbfs` | unit under the real-time trap |
| R7 | `crates/devices/tests/instrument_filter.rs`, `prd_self_oscillates_at_its_cutoff_and_stays_under_plus_twelve_dbfs` | unit under the real-time trap |
| R8 | `crates/devices/tests/instrument_filter.rs`, `smps_distortion_lies_between_ms2s_and_prds` | unit under the real-time trap |
| R9 | `crates/devices/tests/instrument_filter.rs`, `drive_is_a_gain_for_the_circuits_on_low_pass_high_pass_and_band_pass`; `crates/devices/tests/multisampler.rs`, `the_circuits_play_and_res_morph_and_drive_act_per_frame_at_any_block_size` | unit under the real-time trap; engine test |
| R10 | `crates/devices/tests/instrument_filter.rs`, `morph_passes_through_the_four_types_in_order`; `crates/devices/tests/multisampler.rs`, `the_filters_types_and_morph_match_their_prototypes` | unit under the real-time trap; engine test |
| R11 | `crates/devices/tests/instrument_filter.rs`, `a_voices_output_is_the_same_at_any_block_size`, `a_cutoff_modulated_at_audio_rate_is_heard_at_audio_rate`, `a_change_of_type_slope_or_circuit_makes_no_click`; `crates/devices/tests/multisampler.rs`, `the_circuits_play_and_res_morph_and_drive_act_per_frame_at_any_block_size` | render (block sizes 1, 64, 512 and 4,096; the Sampler's at 1, 100, 512 and 4,096); a click detector on type changes |
| R12 | `crates/devices/tests/instrument_filter.rs`, `nothing_is_unbounded_at_any_setting_or_rate`, `a_long_silence_after_loud_input_is_exactly_silent` | property test (random settings at 44.1 to 192 kHz stay bounded; a 20 kHz cutoff at 44.1 kHz and full Resonance); a long silence leaves exactly zero, so no denormal stays |
| R13 | `crates/devices/tests/instrument_filter.rs`, `sixteen_voices_render_faster_than_the_device_budget_with_no_allocation`, `a_voices_output_is_the_same_at_any_block_size` | benchmark against the device budget (judged at full depth); real-time trap |
| R14, R16 | `crates/agent/tests/instrument_filter.rs`, `an_agent_sets_and_reads_simplers_circuit_drive_morph_and_types`, `an_agent_sets_and_reads_samplers_circuit_and_its_other_filter_values` | agent API (set and read every new parameter) |
| R17 | review | |

## Not built yet

- The circuit chooser doesn't grey a circuit the type doesn't offer (R2), and Drive isn't greyed on Clean, notch and Morph (R9): the generic device panel has no greyed choice or control yet. [`offered`](../../crates/devices/src/instrument_filter.rs) gives the list the chooser would show.
- Simpler's Morph control has no context menu that snaps it to a type (R10); Sampler's has.
- A change of Type, Slope or Circuit doesn't carry the filter's state over where the structure is the same: it always crossfades to a structure that starts from silence.
- Operator, Bass, Poli and Meld don't use the filter yet; each does from its own work package. Wavetable does, twice, from its first part.
