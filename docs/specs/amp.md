# Amp: seven amplifier models

- **Status:** draft
- **Work packages:** WP-8.40 (all of it: R1 to R14). [Cabinet](cabinet.md), meant to follow it, and [Pedal](pedal.md), meant to go before it, are their own devices; the oversampler and the framework are WP-2.5's ([the native device framework](device-framework.md) R8)
- **Parity-matrix row:** Audio effects: Amp, Cabinet, Pedal
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.1 "Amp" with 29.1.1, read 30 October 2026

## The behaviour

Amp models a guitar amplifier: a preamp that drives the signal into distortion, a tone stack with Bass, Middle and Treble, and a power stage whose Presence control brightens the top end. Seven models range from clean to heavy, with one for bass. Gain sets how hard the preamp is driven, Volume how hard the power stage is, and the result can be mono or processed in stereo (Dual), and mixed with the dry signal. It is meant to be followed by Cabinet.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.amp`. Every model is Beatlayer's own design, a chain of stages written below, and none is named after or modelled on any manufacturer's product.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.1 "Amp" | Seven models: Clean, Boost, Blues, Rock, Lead, Heavy and Bass; Gain (the preamp's drive), Volume (the power stage's), Bass, Middle, Treble, Presence (the high end, in the power stage); an Output switch, mono or stereo (Dual, about twice the processing); Dry/Wet |
| S2 | Manual | Section 29.1.1 "Amp Tips" | Amp and Cabinet are meant to be used together; the power stage's controls interact with Gain; Amp suits sources other than guitar |
| S3 | Paper | D. T. Yeh and J. O. Smith, "Discretization of the '59 Fender Bassman tone stack", DAFx 2006 | A passive three-knob tone stack's transfer function as a third-order filter whose coefficients follow its component values and knob positions |
| S4 | Paper | N. Koren, "Improved VT models for SPICE simulations", Glass Audio 5, 1996 | A triode's plate current as a smooth function of grid and plate voltages, giving its asymmetric soft clipping |
| S5 | Beatlayer | [The device framework](device-framework.md) R4, R8; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2; [tube distortion](tube-distortion.md), [saturator](saturator.md), [overdrive](overdrive.md) | Smoothing, the oversampler, banks of eight, gains to −∞ dB, automatable parameters; Beatlayer's shapers already built |

## Rules

- **R1.** Amp (`beatlayer.amp`) is an audio effect with Live's name, Beatlayer's own layout and no display cell: the model chooser, then Gain, Bass, Middle, Treble, Presence and Volume, the Output switch (Mono, Dual) and Dry/Wet. (S1; the layout is Beatlayer's own)
- **R2.** Each model is three parts in series, run at four times the sample rate through the oversampler (device framework R8): a preamp of n triode stages, each u ↦ T(k·u + β) − T(β), where T is the triode curve (R3) and k the stage's gain, with a 20 Hz high-pass between stages; the tone stack (R4); and a power stage P(v·x) with sag (R5). Gain (0 to 10, 5 by default) scales every preamp stage's k between the model's lowest and highest gain, logarithmically. (S1, S4; the structure is Beatlayer's own)
- **R3.** The triode curve T(x) is 1.5 x / (1 + |x|^1.5)^(1/1.5) for x ≥ 0 and 1.2 tanh(x / 1.2) − 0.08 x² / (1 + x²) for x < 0, a soft, asymmetric curve after Koren's triode law, normalised to unit slope at zero. Its static curve, measured with a slow sine sweep, lies within 0.01 of the formula. (S4; the curve is Beatlayer's own)
- **R4.** The tone stack is, for every model but Bass, a passive three-band stack after Yeh and Smith whose Bass, Middle and Treble (each 0 to 10, 5 by default) are its potentiometers, with component values from the model's row in R6; for Bass, an active Baxandall stack with ±15 dB shelves at 100 Hz and 4 kHz and a ±12 dB peak at 600 Hz. Each model's measured response at small signal (Gain at its lowest), at Bass, Middle and Treble 0, 5 and 10, is within 0.5 dB of its design from 20 Hz to 20 kHz. (S3; the component values and the active stack are Beatlayer's own)
- **R5.** The power stage is P(x) = tanh(x) with sag: its drive v, set by Volume, falls by up to the model's sag share while the stage's envelope (a 30 ms attack, 300 ms release follower of its output) is high, compressing as a loaded supply does. Presence (0 to 10, 5 by default) is a high shelf of −6 to +9 dB at 3 kHz in the power stage's feedback, acting on the stage's input. (S1; the law is Beatlayer's own)
- **R6.** The models, as rows of preamp stages n, gain range (k at Gain 0 and 10), triode bias β, tone-stack values (R1, R2, R3 in kΩ, C1, C2, C3 in nF, the stack's resistors and capacitors in Yeh and Smith's naming) and sag share: Clean, 1 stage, k 1 to 4, β 0.05, 250 / 1000 / 25 kΩ, 0.25 / 20 / 20 nF, sag 0.05; Boost, 2 stages, k 2 to 12, β 0.1, 220 / 1000 / 22 kΩ, 0.47 / 22 / 22 nF, sag 0.1; Blues, 2 stages, k 2 to 20, β 0.15, 250 / 250 / 56 kΩ, 0.25 / 20 / 20 nF, sag 0.2; Rock, 3 stages, k 3 to 30, β 0.1, 220 / 1000 / 33 kΩ, 0.47 / 22 / 22 nF, sag 0.15; Lead, 3 stages, k 5 to 60, β 0.2, 250 / 1000 / 33 kΩ, 0.47 / 47 / 22 nF, sag 0.15; Heavy, 4 stages, k 6 to 80, β 0.2, 220 / 1000 / 47 kΩ, 0.68 / 47 / 47 nF, sag 0.1; Bass, 2 stages, k 1 to 10, β 0.05, the active stack of R4, sag 0.2. (S1; every value is Beatlayer's own)
- **R7.** Each model's harmonic balance on a 220 Hz sine at −12 dBFS with Gain 5 (the second against the third harmonic) is within 1 dB of its design's computed figure, and each model's DC after the power stage is below −80 dBFS: a 5 Hz high-pass follows the preamp and the power stage. (Beatlayer's own)
- **R8.** Volume is a gain from −∞ to +12 dB (0 dB by default) into the power stage and, scaled back by its own amount after it, sets how hard the stage is driven without changing the level of a clean signal by more than 1 dB; at −∞ the device is silent (rule G1). (S1, S5; the law is Beatlayer's own)
- **R9.** With Output Mono, the input's channels are summed and one amp runs, its output on both channels; with Dual, each channel runs its own amp, so a signal on the left alone leaves nothing on the right. (S1)
- **R10.** Dry/Wet, 0 to 100 % (100 % by default), balances the dry signal, delayed by the oversampler's latency, against the amp's output. The device reports the oversampler's latency, the same for every model and setting. (S1, S5)
- **R11.** A render is bit-identical at block sizes 1, 64, 512 and 4,096; aliasing from a 5 kHz sine at Gain 10 stays 70 dB below the fundamental in the band below 20 kHz; nothing on the audio thread allocates, locks or blocks. (S5; the level is Beatlayer's own)
- **R12.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; changing model crossfades over 20 ms with no click. (S5)
- **R13.** Amp declares one bank of eight ([control surfaces](control-surfaces.md) R19): Model, Gain, Bass, Middle, Treble, Presence, Volume, Dry/Wet, with Output in a second bank, so every automatable parameter is in a bank. (S5; Beatlayer's own)
- **R14.** A set with Amp saves and reopens with every parameter. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-8.40 | unit (description and layout) |
| R2, R3 | to be written in WP-8.40 | unit (the triode curve on a sweep; the chain's order) |
| R4 | to be written in WP-8.40 | unit (each model's tone stack within 0.5 dB at nine knob settings) |
| R5 to R8 | to be written in WP-8.40 | unit (Presence's shelf; sag; harmonic balance per model; DC; Volume at −∞) |
| R9, R10 | to be written in WP-8.40 | unit (Dual keeps channels apart; reported latency; Dry/Wet aligned) |
| R11 | to be written in WP-8.40 | unit (aliasing); real-time trap; golden renders at four block sizes |
| R12 to R14 | to be written in WP-8.40 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |
