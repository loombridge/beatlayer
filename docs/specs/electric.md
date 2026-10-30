# Electric: an electric piano of a hammer, a fork, a damper and a pickup

- **Status:** draft
- **Work packages:** WP-8.23 (this spec, R1 to R14). The tine and tone bar are modal resonators of [the physical models](physical-models.md); voices and per-note bends are [the instruments' voice section](instrument-voices.md); the oversampler is [the device framework](device-framework.md) R8
- **Parity-matrix row:** Instruments: Collision, Tension, Electric (Electric)
- **Checked against:** the Live 12 reference manual (online edition), read 30 October 2026 (chapter 31, section 31.5: 31.5.1 to 31.5.5); the Live 12 release notes, 12.0 (every instrument plays MPE)

## The behaviour

Electric models an electric piano. A key throws a hammer at a fork: a thin tine that the hammer strikes and a heavier tone bar coupled to it, which rings on. A pickup beside the tine turns its motion into the sound, adding the harmonics of its own nonlinearity as the tine swings closer and further; a damper stops the fork when the key is released and adds its own small noises. The global section holds the volume, the voices, the tuning and the bend ranges.

The device's sections, Hammer, Fork and Damper/Pickup, are chosen by icons, with the global section beside them. Every law here is Beatlayer's own reading of the manual's controls on a modal model of the fork.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 31 "Live Instrument Reference", section 31.5.1 "Architecture and Interface" | Hammer, Fork, Damper/Pickup and Global sections, the first three chosen by their icons |
| S2 | Manual | Section 31.5.2 "Hammer Section" | Stiffness (harder: brighter), moved by velocity and key; Noise with its Pitch (the noise's centre), Decay and Key (its level following the key); Force's Amount (soft to hard), moved by velocity and key |
| S3 | Manual | Section 31.5.3 "Fork Section" | The tine, the struck part, with Color (the balance of high and low partials), Decay (while the key is held) and Key (its level following the key); the tone bar, a second resonance, with its own Decay; Release, the fork's decay after the key is let go, for both |
| S4 | Manual | Section 31.5.4 "Damper/Pickup Section", 31.5.4.1 "Pickup Parameters" and 31.5.4.2 "Damper Parameters" | The pickup's Symmetry (its height against the tine; 50 % straight in front, brighter), Distance (closer, more overdrive), Type R (electrodynamic) or W (electrostatic), Input (the fork's level into it, driving its distortion) and Output (moved by the key); the damper's Tone (soft to bright), Level (its noise) and Att/Rel (−100 % its noise at the attack only, 100 % at the release only, 0 both) |
| S5 | Manual | Section 31.5.5 "Global Section" | Volume; Voices; Semi; Detune (±50 cents); Stretch (0 % equal temperament; positive stretching the upper notes sharp and the lower flat); Pitch Bend and Note PB ranges in semitones |
| S6 | Beatlayer | [The physical models](physical-models.md) R1 to R7 and R12 to R17; [the instruments' voice section](instrument-voices.md); [the device framework](device-framework.md) R8, R29 and R41 to R45 | The Cantilever and Beam resonators; the shared voices; the oversampler and its latency; seeded randomness; pages |

## Rules

- **R1.** Beatlayer ships Electric (`beatlayer.electric`), an instrument listed under the browser's Instruments label, with factory presets of Beatlayer's own. Its body is a shell and display (device framework R41): sections Hammer, Fork and Damper/Pickup chosen by their buttons, the selected one's controls filling the display, and the global section always shown. (S1, S6)
- **R2.** The hammer strikes the tine once per note with a force Force's Amount (0 to 100 %) sets, moved by velocity and key (Vel and Key amounts, −100 % to 100 %): a raised-cosine pulse whose width Stiffness sets (soft, 5 ms; hard, 0.2 ms), moved by velocity and key, so a harder hammer is shorter and brighter. Noise adds a burst of band-pass noise from the device's seed at Pitch (100 Hz to 10 kHz) decaying over Decay (1 ms to 1 s), its level following the key by Key. (S2; the pulse and ranges are Beatlayer's own)
- **R3.** The tine is the physical models' Cantilever resonator at the note's pitch (physical models R1); Color (−100 % to 100 %) tilts its partials' gains from the fundamental to the upper partials (physical models R6's Brightness); Decay (100 ms to 60 s) is its decay while the key is held; Key (−100 % to 100 %) moves its level with the key by up to 12 dB over four octaves from C3. (S3, S6)
- **R4.** The tone bar is a Beam resonator (physical models R1) tuned to the note's fundamental and driven by the tine, so the fundamental rings on after the tine's high partials die; its Decay (100 ms to 60 s) is its own. (S3, S6; the coupling is Beatlayer's own)
- **R5.** Release (10 ms to 10 s) is the fork's decay, tine and tone bar together, after the key is let go: from the note's end both fall 60 dB within Release. (S3)
- **R6.** The tine and tone bar ring at their pitch across the keyboard within 1 cent, and their decay follows Decay and Release within 5 %. (S3, S6)
- **R7.** The pickup turns the tine's displacement x into a signal through a nonlinearity set by its geometry: Type R (electrodynamic) responds to the change of a magnetic field, as 1 / (d − x)² differentiated; Type W (electrostatic) to capacitance, as 1 / (d − x); d is Distance (closer, more distortion) and Symmetry (0 to 100 %, 50 % in front) offsets the tine's rest position against the pickup's centre, so away from 50 % even harmonics rise. Input (−∞ to +12 dB) drives the fork's signal into it and Output (−∞ to +12 dB, moved by the key) sets its level. (S4; the laws are Beatlayer's own readings of the two pickup types)
- **R8.** The pickup's nonlinearity runs at four times the sample rate through the device framework's oversampler (device framework R8), and the device reports that latency. (S6)
- **R9.** The damper lands when a note ends, stopping the fork as R5 says, and adds a short noise: Tone (dark to bright) its colour, Level (−∞ to 0 dB) its level, and Att/Rel (−100 % to 100 %) where it sounds, at the note's start (the damper lifting), its end (landing) or both. (S4)
- **R10.** The global section: Volume (−∞ to 0 dB), Voices (1 to 32, 16 by default; instrument voices R3), Semi (−48 to +48), Detune (−50 to +50 cents), Stretch (instrument voices R17), Pitch Bend (0 to 24 semitones, 2 by default) and Note PB (0 to 48, 48 by default; instrument voices R14). (S5)
- **R11.** Every control is a parameter, automatable and modulatable (rule G2), and Electric's banks hold every one. (S6)
- **R12.** Sixteen voices render at 48 kHz inside the device budget. (S6)
- **R13.** The output is the same at any block size and run after run; nothing on the audio thread allocates, locks or blocks. (S6)
- **R14.** A note's sound depends on its velocity through the hammer's force and stiffness alone, as their Vel amounts give, so at every amount 0 two velocities sound alike but for level. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R11 | `crates/devices/tests/banks.rs`; `crates/devices/tests/rules.rs`; to be written in WP-8.23 (layout test) | unit |
| R2, R14 | to be written in WP-8.23 | unit under the real-time trap (the attack's width and spectrum against force and stiffness) |
| R3, R4, R5, R6 | to be written in WP-8.23 | render (pitch within 1 cent; decays within 5 %) |
| R7, R8 | to be written in WP-8.23 | unit (harmonics against Symmetry and Distance; latency reported) |
| R9, R10 | to be written in WP-8.23 | render |
| R12 | to be written in WP-8.23 | benchmark against the device budget |
| R13 | to be written in WP-8.23 | render at block sizes 1, 64, 512 and 4,096; repeated renders identical |
