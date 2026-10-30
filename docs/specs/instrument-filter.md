# The instruments' filter: types, slopes, the Clean, OSR, MS2, SMP and PRD circuits, Drive and Morph

- **Status:** draft
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
- **R3.** Clean is the plug-in kit's state-variable filter, as Simpler and Sampler played before this spec: the same coefficients and state for the same Frequency, Resonance, Type and Slope, so a render through Clean equals one made before this spec sample for sample. Its Morph type is the kit's morphing state-variable filter. Clean has no Drive and no distortion of its own. (S2, S5)
- **R4.** At small signal (an input at −40 dBFS and below) every circuit is a linear filter and follows its own linear prototype on the warped frequency axis within 0.5 dB from 20 Hz to 20 kHz, at cutoffs of 100 Hz, 1 kHz and 10 kHz and Resonance 0 %, 50 % and 90 %, at 44.1 kHz and at 48 kHz: Clean and OSR a state-variable section (two cascaded for 24 dB), MS2 and SMP a Sallen-Key section (two cascaded for 24 dB), PRD a four-pole ladder (its second pole's output for 12 dB). (S2, S3, S5; the prototypes are Beatlayer's own readings)
- **R5.** OSR's state-variable section clips the state in its resonance path hard at ±1: as the level or the resonance rises, its resonant peak stops growing at that ceiling and its tone hardens, and its output never exceeds +6 dBFS whatever the input, Drive and Resonance. (S3; the ceiling is Beatlayer's own)
- **R6.** MS2's Sallen-Key section passes its feedback through a soft clipper (tanh): as the level or the resonance rises, the peak rounds off gradually, and at 100 % Resonance with no input it rings down rather than oscillating on, its peak held below +6 dBFS. (S3, S4; the clipper and the bound are Beatlayer's own)
- **R7.** PRD's ladder saturates only its input stage and puts no limit on its feedback: at high Resonance it self-oscillates at the cutoff within 50 cents of it, at a level its stages' own saturation sets, and loud input pulls the resonance down less than MS2's does. Its output never exceeds +12 dBFS. (S3, S4; Beatlayer's own bound)
- **R8.** SMP is a Sallen-Key section with PRD's input saturation in front and MS2's soft clipper in its feedback set to clip later, so its distortion at a given level and Resonance lies between MS2's and PRD's: measured with a 100 Hz sine at −6 dBFS through a 1 kHz low-pass at 70 % Resonance and 12 dB Drive, its total harmonic distortion is above MS2's and below PRD's. (S3; Beatlayer's own reading)
- **R9.** Drive, from 0 to 24 dB (0 dB by default), is a gain on the signal into the circuit for OSR, MS2, SMP and PRD on low-pass, high-pass and band-pass; it does nothing on Clean, on notch and on Morph, and the device shows it greyed there. The circuit's output isn't scaled back, so at small signal Drive raises the level by its gain. (S1, S2, S3; Beatlayer's own range, Sampler's, and law)
- **R10.** Morph, from 0 to 100 % (0 % by default), sweeps the Morph type from low-pass at 0 % through band-pass at 25 %, high-pass at 50 % and notch at 75 % back to low-pass at 100 %, each stop the pure type and each place between a crossfade of its two neighbours; its context menu snaps it to any of the four. It plays with Clean and OSR. (S1, S2, S5)

**Per voice, per frame**

- **R11.** The filter runs per voice, every voice in the plug-in kit's lanes, and reads Frequency, Resonance, Morph and Drive per frame from each voice's modulated values, so a modulation of the cutoff at audio rate is heard at audio rate. A change of Type, Slope or Circuit lands on its frame without a click: the state carries over where the structure is the same and the new structure starts from silence, crossfaded over 5 ms, where it isn't. (S5, S6; the crossfade is Beatlayer's own)
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
| R1, R2 | to be written in WP-8.3 | unit (descriptors; the circuit chooser's offered list) |
| R3, R15 | to be written in WP-8.3 | golden render (Simpler and Sampler sets from before the change, sample for sample) |
| R4 | to be written in WP-8.3 | unit under the real-time trap (swept sines at −40 dBFS, measured by FFT against each prototype) |
| R5, R6, R7, R8 | to be written in WP-8.3 | unit under the real-time trap (levels, self-oscillation and distortion at high level and resonance) |
| R9, R10 | to be written in WP-8.3 | unit under the real-time trap |
| R11 | to be written in WP-8.3 | render (audio-rate cutoff modulation; a click detector on type changes); block sizes 1, 64, 512 and 4,096 |
| R12 | to be written in WP-8.3 | property test (random settings at 44.1 to 192 kHz stay bounded); a denormal timing test |
| R13 | to be written in WP-8.3 | benchmark against the device budget; real-time trap |
| R14, R16 | to be written in WP-8.3 | agent API (set and read every new parameter) |
| R17 | review | |
