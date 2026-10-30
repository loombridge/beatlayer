# Corpus: physical resonators on any sound

- **Status:** draft
- **Work packages:** WP-8.44 (all of it: R1 to R17). The resonator models (Beam, Marimba, String, Membrane, Plate, Pipe, Tube) are Beatlayer's own, in `bl_dsp::physical`, shared with Collision, Tension, Electric and Meld: Corpus's resonator types cite [the physical models](physical-models.md) R1 to R18 (WP-8.5) for their partials, decay, positions and stability rather than defining resonators of their own, and its work package depends on WP-8.5; MIDI sidechains are [routing](routing.md) R32 to R35 and R39; the framework is WP-2.5's ([the native device framework](device-framework.md)), its LFO WP-5.42's
- **Parity-matrix row:** Audio effects: Corpus, Resonators, Vocoder
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.10 "Corpus" with 29.10.1 to 29.10.5, read 30 October 2026

## The behaviour

Corpus rings its input through a physical model of a resonating body: a beam, a marimba bar, a string, a membrane, a plate, a pipe or a tube. The person chooses the body, its decay, its material or radius, its brightness, inharmonicity and the other properties the type has, where it is struck and where its left and right sides are listened to, and its tuning. An LFO can move the tuning, a band-pass filter shapes what enters, and Bleed mixes some of the unprocessed input back. Its frequency can follow the notes of a MIDI track, so a body is played as an instrument.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.corpus`. Its models are Beatlayer's own, shared with Collision.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.10.1 "Resonator Type" | The types Beam, Marimba, String, Membrane, Plate, Pipe and Tube; Quality (Eco, High; not for Pipe and Tube); Decay; Material (not for Pipe and Tube); Radius (Pipe and Tube); Bright; Inharm; Opening (Pipe: closed at one end at 0 %, open at both at 100 %); Ratio (Membrane and Plate); Hit; Width (mono at 0 %, each resonator on its own channel at 100 %); Pos. L and Pos. R (not for Pipe and Tube); Tune (Hz); Fine (cents); Spread (0 %: both resonators tuned alike); an X–Y control for Decay with Material or Radius |
| S2 | Manual | Section 29.10.2 "LFO Section" | Amount; Rate in Hz or synced; Offset (synced only); the waveforms Sine, Square, Triangle, Sawtooth Up, Sawtooth Down, Noise (stepped) and Noise (smooth); Phase, in degrees, or Spin (Hz mode only), no effect on the noise waves |
| S3 | Manual | Section 29.10.3 "Filter Section" | A band-pass on the input, switched by Filter, with Freq and Bdwidth |
| S4 | Manual | Section 29.10.4 "Global Parameters" | Bleed (not for Pipe and Tube); Gain, with a built-in limiter and its lamp; Dry/Wet |
| S5 | Manual | Section 29.10.5 "Sidechain Parameters" | The sidechain toggle; Frequency, which makes MIDI notes set the tuning (Tune then coarse in semitones and Fine in cents); Off Decay and its amount (0 to 100 %), which damps the body on a note-off; MIDI From with its track and tapping point; Last or Low note priority; PB Range in semitones |
| S6 | Beatlayer | `bl_dsp::physical` ([the architecture](../02-architecture.md)); [routing](routing.md) R32 to R35, R39; [the device framework](device-framework.md) R26, R27, R29, R39; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2 | The shared models and their controls; MIDI sidechains and their tapping point; the effects LFO, seeds, channel messages; banks; gains and automation |

## Rules

- **R1.** Corpus (`beatlayer.corpus`) is an audio effect with a MIDI sidechain (routing R32), Live's name, Beatlayer's own layout and one display cell, the X–Y control for Decay against Material (Radius for Pipe and Tube), whose drags set both as one undo step. Its body reads the LFO section, the filter section, the resonator section and the global section; the sidechain section is the left strip of [routing](routing.md) R34 with Frequency, Off Decay, Last/Low and PB Range below the source and tapping point. (S1 to S5; the layout is Beatlayer's own)
- **R2.** The resonator section drives two resonators of the chosen type (Beam in a new device), one heard on each side, each a `bl_dsp::physical` model set from the controls; the device holds no model maths of its own. Controls a type doesn't use are dimmed and keep their values: Quality, Material, Bright, Inharm, Hit, Pos. L, Pos. R and Bleed for Pipe and Tube; Radius except for Pipe and Tube; Opening except for Pipe; Ratio except for Membrane and Plate. (S1, S6)
- **R3.** The controls and their ranges, in Beatlayer's own units: Quality Eco or High (the model's mode counts, as `bl_dsp::physical` gives them); Decay 0 to 100 % (the fundamental's ring from 50 ms to 30 s, logarithmic); Material −100 to +100 % (the highs' decay against the lows', from damped like wood to ringing like metal); Radius 0 to 100 %; Bright −100 to +100 %; Inharm −100 to +100 %; Opening 0 to 100 %; Ratio 0 to 100 %; Hit 0 to 100 % (where the body is excited, from the centre to its edge); Pos. L and Pos. R 0 to 100 % (where each side is listened to); each reaching its model as `bl_dsp::physical` defines it. (S1, S6; the ranges and units are Beatlayer's own)
- **R4.** Tune, 20 Hz to 5 kHz (logarithmic, 440 Hz by default), and Fine, −50 to +50 cents, set the left resonator's fundamental, within 1 cent from 40 Hz to 4 kHz at 44.1, 48 and 96 kHz; Spread, −100 to +100 % (0 by default), detunes the right resonator up and the left down by up to 50 cents each. (S1, S6; ranges are Beatlayer's own)
- **R5.** Width, 0 to 100 % (100 % by default), mixes the two resonators: at 0 % both in the centre, at 100 % the left resonator on the left and the right on the right. (S1)
- **R6.** The LFO moves the tuning by up to ±2 octaves times Amount (0 to 100 %): its waveforms are the effects LFO's Sine, Rectangle, Triangle, Saw Up, Saw Down, Sample & Hold and Random (device framework R26), named as S2 names them; its Rate is 0.01 to 40 Hz or, synced, a division from 1/64 note to 8 bars on the beat grid with Offset in degrees (device framework R27); its stereo mode is Phase (0 to 360°, the right LFO ahead) or Spin (Hz mode only, the right LFO's rate up to 50 % faster), and neither moves the noise waves apart. The noise waves follow the device's seed. (S2, S6; the depth and ranges are Beatlayer's own)
- **R7.** With Filter on, the input passes a band-pass at Freq (50 Hz to 18 kHz, 1 kHz) with Bdwidth (0.1 to 4 octaves, 2 octaves), measured within 0.5 dB of its design, before the resonators. (S3; ranges are Beatlayer's own)
- **R8.** Bleed, 0 to 100 % (0 by default), mixes the unprocessed input into the resonators' output, brightening the result. Gain, a gain from −∞ to +24 dB (0 dB, rule G1), sets the resonators' level into a limiter at 0 dBFS that keeps them from clipping, its lamp lit while it reduces the gain. Dry/Wet, 0 to 100 % (100 % by default), balances the dry signal against the result. (S4; ranges and the limiter's design are Beatlayer's own)
- **R9.** With Frequency on and a MIDI source chosen, each note from the source sets the tuning on its frame: the fundamental becomes the note's pitch (under the set's tuning, twelve equal steps from A3 = 440 Hz without one) transposed by Tune read as semitones (−48 to +48) and Fine in cents; Last plays the newest held note, Low the lowest. Pitch bend moves it by up to PB Range (0 to 24 semitones, 2 by default) at once (device framework R39). (S5, S6; the reference and default are Beatlayer's own)
- **R10.** With Off Decay on, a note-off with no other note held damps the resonators' decay to Off Decay's share (0 % leaving it, 100 % stopping them within 50 ms), from the note-off's frame; with it off, a note-off leaves the body ringing at the last pitch. Releasing a note while others are held moves to the next by the priority. With Frequency on and no note yet, the body rings at Tune. (S5; the law is Beatlayer's own)
- **R11.** The MIDI source and its tapping point are set with `device.set_midi_sidechain_source` and read back from the device's `midi_sidechain` ([routing](routing.md) R35, R39). (S5, S6)
- **R12.** Parameter changes glide over 20 ms (device framework R4), and the models' coefficients move per sample so nothing clicks or goes unstable while they move. (S6)
- **R13.** Nothing on the audio thread allocates, locks or blocks; the models' state is made with the node for High quality. (S6)
- **R14.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, with the LFO's noise waves and MIDI notes. (S6)
- **R15.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. (S6)
- **R16.** Corpus declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Type, Decay, Material, Bright, Tune, Spread, Gain, Dry/Wet), Body (Radius, Inharm, Opening, Ratio, Hit, Pos. L, Pos. R, Quality), LFO, Filter and Sidechain (Frequency, Off Decay and its amount, Last/Low, PB Range, Width, Bleed, Fine), so every automatable parameter is in a bank. (S6; Beatlayer's own)
- **R17.** A set with Corpus saves and reopens with every parameter and its MIDI source. (S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-8.44 | unit (description; each type drives its shared model with the spec's controls; dimmed controls); UI script (the X–Y control's drag is one undo step) |
| R3 to R5 | to be written in WP-8.44 | unit (the tuning within 1 cent at three rates; Spread; Width) |
| R6 to R8 | to be written in WP-8.44 | unit (the LFO's rate synced and free; the filter's response; Bleed; the limiter holds 0 dBFS) |
| R9 to R11 | to be written in WP-8.44 | engine test (a MIDI track's note retunes it on its frame; Last and Low; pitch bend; Off Decay on a note-off); agent API |
| R12 to R14 | to be written in WP-8.44 | real-time trap, RealtimeSanitizer; golden renders at four block sizes |
| R15 to R17 | to be written in WP-8.44 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen |
