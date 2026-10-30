# Resonators: five tuned resonators

- **Status:** draft
- **Work packages:** WP-8.43 (all of it: R1 to R15). What every scale-aware device shares is [scales](scales.md) R15 to R18; a device following the set's tuning is [tunings](tunings.md)'; the framework is WP-2.5's ([the native device framework](device-framework.md))
- **Parity-matrix row:** Audio effects: Corpus, Resonators, Vocoder
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.31 "Resonators", read 30 October 2026

## The behaviour

Resonators rings an input at five pitches. The input first passes a filter; then five resonators in parallel, each a tuned comb, ring at their pitches: the first set by a note and a fine tune, the other four by an interval above or below it, each with its own switch and gain. Decay sets how long they ring, Const keeps that time the same at every pitch, Color tilts their brightness, and Mode chooses between two resonator designs. Width spreads the resonators between the channels, and Gain and Dry/Wet set the output. With Use Current Scale on, the pitches are counted in degrees of the track's current scale; under a tuning system, in the tuning's notes.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.resonators`.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.31 "Resonators" | The input filter, low-pass, band-pass, high-pass or notch, with its frequency; Mode A (more realistic) and B (suited to a low first resonator); Decay; Const; Color; Resonator I's switch, Gain, Note (C-1 to C5 in semitones, or in scale degrees, or in the tuning's notes) and Fine; Resonators II to V, each with its switch, Gain, Pitch (±24 semitones, or ±21 scale degrees) relative to I, and Fine; Width, Gain and Dry/Wet |
| S2 | Beatlayer | [Scales](scales.md) R15 to R18; [tunings](tunings.md) R1, R6, R12 and R16 | A scale-aware device's current scale and Use Current Scale switch; the pitch a key has under the set's tuning; scale controls disabled under a tuning that doesn't hold twelve notes |
| S3 | Beatlayer | [The device framework](device-framework.md) R4, R9; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2; [Filter Delay](filter-delay.md) | Smoothing, banks of eight, gains to −∞ dB, automatable parameters; comb and filter parts |

## Rules

- **R1.** Resonators (`beatlayer.resonators`) is an audio effect with Live's name, Beatlayer's own layout and no display cell: the filter section, the global section (Mode, Decay, Const, Color), the five resonators' columns, and Width, Gain and Dry/Wet. It is scale-aware: a Use Current Scale switch in its title bar, as [scales](scales.md) R16 says. (S1, S2)
- **R2.** The input filter is a 12 dB per octave low-pass, band-pass, high-pass or notch (band-pass in a new device) at its frequency (50 Hz to 18 kHz, 1 kHz by default, logarithmic), measured within 0.5 dB of its design. (S1; ranges, the slope and the default are Beatlayer's own)
- **R3.** Resonator I's pitch is Note (C-1 to C5, whole semitones, C3 by default) plus Fine (−50 to +50 cents). Resonators II to V each take Pitch (−24 to +24 semitones; defaults +12, +19, +24 and −12) plus their own Fine, relative to I's pitch. Each has a switch (I to III on, IV and V off in a new device) and Gain, a gain from −∞ to 0 dB (rule G1). (S1; the defaults are Beatlayer's own)
- **R4.** With Use Current Scale on, Note counts degrees of the device's current scale ([scales](scales.md) R15) from its root in the octave of C-1, and Pitch counts degrees (−21 to +21) from Resonator I's note within that scale; the Fines stay in cents. A change of current scale retunes the resonators from the frame it happens, gliding over 20 ms. (S1, S2)
- **R5.** While the set has a tuning ([tunings](tunings.md)), Note counts the tuning's notes and Pitch its steps, each pitch being the frequency the tuning gives that note, plus Fine; under a tuning that doesn't hold twelve notes to its pseudo-octave, Use Current Scale is disabled as [tunings](tunings.md) R16 says. Without a tuning, pitches are twelve equal steps from A3 = 440 Hz. (S1, S2)
- **R6.** Each resonator is a feedback comb whose loop is one period of its pitch long, made exact by a fractional delay (a fourth-order Thiran allpass), with a one-pole low-pass in the loop that Color sets; its strongest resonance, measured on white noise through the device with that resonator alone, is within 1 cent of its pitch from C-1 to C5 at 44.1, 48 and 96 kHz. (S1; the design is Beatlayer's own)
- **R7.** Decay, 0.01 to 30 s (1 s by default, logarithmic), sets each loop's feedback so its ring falls 60 dB in Decay, within 10 %, for Resonator I; with Const off, the others keep Resonator I's feedback, so higher resonators ring shorter, as strings do; with Const on, each resonator's feedback is set for its own pitch, so all ring for Decay. A loop's feedback stays below 1 at any setting. (S1; the law is Beatlayer's own)
- **R8.** Color, −100 to +100 % (0 by default), moves the loops' low-pass, from dark (cutoff at 2 times the pitch at −100 %) to open (no filtering at 0 % and above) and adds a matching high-shelf boost above 0 %, up to +6 dB at 10 times the pitch. (S1; the law is Beatlayer's own)
- **R9.** Mode A runs each resonator as one comb on the filtered input. Mode B runs each as two combs a fifth apart (the second at 1.5 times the pitch, 12 dB lower), which gives a low Resonator I a clearer pitch. (S1; Mode B's design is Beatlayer's own)
- **R10.** Width, 0 to 100 % (100 % by default), places the resonators: at 0 % all in the centre; at 100 % I in the centre, II and IV left, III and V right; between, in proportion. Gain, a gain from −∞ to +12 dB (0 dB), sets the resonators' sum (rule G1), and Dry/Wet, 0 to 100 % (100 % by default), balances it against the dry signal. (S1; the placement and ranges are Beatlayer's own)
- **R11.** The comb lines are made with the node, long enough for C-1 at 192 kHz; nothing on the audio thread allocates, locks or blocks. (S3)
- **R12.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run. (S3)
- **R13.** Every parameter is automatable and modulatable (rule G2), Use Current Scale included, and set and read back through the agent API; every gain reaches −∞ dB. (S2, S3)
- **R14.** Resonators declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Filter Frequency, Note, Decay, Color, Width, Gain, Dry/Wet, Mode), Pitches (II to V's Pitch and Gain) and Detail (the filter type, the switches, the Fines, Const), so every automatable parameter is in a bank. (S3; Beatlayer's own)
- **R15.** A set with Resonators saves and reopens with every parameter. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-8.43 | unit (description; the filter's measured response per type) |
| R3, R6 | to be written in WP-8.43 | unit (each resonator's peak within 1 cent from C-1 to C5 at three rates) |
| R4, R5 | to be written in WP-8.43 | unit (degrees in a minor scale; a scale change on its frame); engine test (a clip's scale); unit under a 19-note tuning |
| R7 to R10 | to be written in WP-8.43 | unit (Decay's ring time within 10 %; Const; Color's tilt; Mode B's fifth; Width's placement) |
| R11, R12 | to be written in WP-8.43 | real-time trap; golden renders at four block sizes |
| R13 to R15 | to be written in WP-8.43 | `crates/devices/tests/rules.rs`; `crates/devices/tests/scales.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |
