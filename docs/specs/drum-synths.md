# The Drum Synths: DS Kick, DS Snare, DS Clap, DS HH, DS Cymbal, DS Tom, DS FM and DS Clang

- **Status:** draft
- **Work packages:** WP-8.24 (the shared rules and DS Kick, DS Snare, DS Tom and DS Clap: R1 to R14), WP-8.25 (DS HH, DS Cymbal, DS FM and DS Clang: R15 to R20). DS FM's operators are [Operator](operator.md)'s core (R1 to R3); a Drum Rack's pads and choke groups are [drum racks](drum-racks.md)'
- **Parity-matrix row:** Instruments: Drum Synths (Kick, Snare, Clap, Hi-hat, Cymbal, Tom, FM, Clang)
- **Checked against:** the Live 12 reference manual (online edition), read 30 October 2026 (chapter 33 "Max for Live Devices", section 33.1 "Max for Live Instruments": 33.1.1 to 33.1.8); the Live 12 release notes, 12.0 to 12.3.5 (the Drum Synths' fixes)

## The behaviour

The Drum Synths are eight small instruments, each a synthesised drum sound with a few controls: a kick, a snare, a hand clap, a hi-hat, a cymbal, a tom, an FM percussion voice and a cowbell-and-clave. Each is usually played from a Drum Rack pad: a note starts a hit at its velocity, the hit runs its own length, set by its Decay, and a pad's choke group cuts it. Each has a display along its top with some controls inside it; clicking the display's upper half plays a hit.

Live's Drum Synths are Max for Live devices; Beatlayer's are native, with Live's names. The manual gives each device's controls and what each does, but no ranges or curves, so every synthesis law below is Beatlayer's own, from common drum-synthesis practice.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 33 "Max for Live Devices", section 33.1.6 "DS Kick" | Pitch (the kick's tuning in hertz), Drive (distortion), OT (added harmonics), Attack (smoothing the sine), Click (a switch adding a click for a sharper start), Decay, Env (the pitch modulation's amount) and Volume |
| S2 | Manual | Section 33.1.7 "DS Snare" | Color (the pitched part's tone), Tone (the noise's presence), the noise filter's type (low-pass, high-pass or band-pass), Decay, Tune and Volume |
| S3 | Manual | Section 33.1.8 "DS Tom" | Pitch (in hertz), Color (the filter's gain and cutoff), Tone (the level of resonant band-pass filters imitating a membrane's tuning), Bend (the pitch envelope), Decay and Volume |
| S4 | Manual | Section 33.1.2 "DS Clap" | Sloppy (the time between two delay lines, so the panned claps line up more or less tightly), Tail (filtered noise added to the clap's impulse), Spread (0 % mono to 100 % wide), Tone, Tune, Decay and Volume |
| S5 | Manual | Section 33.1.5 "DS HH" | Noise (white or pink), Tone (a high-pass's cutoff, in the display), the resonant high-pass's slope (12 or 24 dB), Attack (the pitched part's attack), Pitch, Decay and Volume |
| S6 | Manual | Section 33.1.3 "DS Cymbal" | Tone (a high-pass's cutoff, in the display), Pitch, Decay and Volume |
| S7 | Manual | Section 33.1.4 "DS FM" | Tone (a low-pass's cutoff, in the display), Feedb. (feedback in the FM algorithm, noisier as it rises), Amnt (the FM amount), Mod (blending between modulation types), Pitch, Decay and Volume |
| S8 | Manual | Section 33.1.1 "DS Clang" | Tone A and Tone B (the two cowbell tones' levels), Filter (the high-pass and band-pass cutoff), Noise (white noise), Clave (a switch repeating a clave sound), Repeat (how many repeats, with Clave on), Pitch, Decay and Volume; every Drum Synth previews a hit when the upper half of its display is clicked |
| S9 | Beatlayer | [Drum racks](drum-racks.md); [tunings](tunings.md) R14; [Operator](operator.md) R1 to R3; [the physical models](physical-models.md) R1; [the device framework](device-framework.md) R29, R35 to R38; [the improvements](../05-improvements.md), rules G1 and G2 | Pads, choke groups and a Drum Rack's twelve steps; the FM core; the circular membrane's ratios; seeded randomness and display data; gains to −∞ dB and every parameter automatable |

## Rules

**Every Drum Synth (WP-8.24)**

- **R1.** Beatlayer ships the eight Drum Synths with Live's names and type IDs of its own: DS Kick (`beatlayer.ds_kick`), DS Snare (`beatlayer.ds_snare`), DS Clap (`beatlayer.ds_clap`), DS HH (`beatlayer.ds_hh`), DS Cymbal (`beatlayer.ds_cymbal`), DS Tom (`beatlayer.ds_tom`), DS FM (`beatlayer.ds_fm`) and DS Clang (`beatlayer.ds_clang`), instruments listed under the browser's Instruments label, each with factory presets of Beatlayer's own. (S1 to S8)
- **R2.** A note on starts a hit on its frame at its velocity; the hit runs for its own length whatever the note's length, so a note's end changes nothing. A new hit while one sounds cuts the old one with a 2 ms fade from its frame, so a device sounds one hit at a time. (S1 to S8; Beatlayer's own)
- **R3.** A hit's level is Volume (−∞ to 0 dB) times its velocity's amplitude share, v / 127, so velocity 127 plays at Volume and a hit never plays above it (rule G1). (S1 to S8, S9; Beatlayer's own law)
- **R4.** Decay (10 ms to 4 s, each device its own default) is the hit's time to fall 60 dB from its peak, within 5 %; the hit ends when it falls below −90 dB. (S1 to S8; Beatlayer's own range)
- **R5.** A device's pitch control (Pitch in hertz for DS Kick and DS Tom; Tune or Pitch in semitones, −24 to +24, for the others) sets its sound's pitch whatever key plays it: the key chooses a pad, not a pitch, as a Drum Rack plays twelve steps whatever the set's tuning ([tunings](tunings.md) R14). (S1 to S8, S9; Beatlayer's own reading)
- **R6.** Each device draws a display across the top of its body: the hit's amplitude envelope drawn from its parameters, with the controls the manual puts in the display along its lower edge. Clicking the display's upper half previews a hit at velocity 100 through the track, without recording and without an undo step (`device.preview_hit`, `read`). (S5 to S8; the drawing and the command's name are Beatlayer's own)
- **R7.** Every control is a parameter, automatable and modulatable (rule G2), and each device's banks hold every one. (S9)
- **R8.** Every random part (noise, the clap's scatter) comes from the device's seeded generator and starts again with each hit, so a hit sounds the same every time at the same settings and two renders are identical. (S9)
- **R9.** A Drum Synth on a Drum Rack pad plays the notes the pad receives and is cut by its choke group as any instrument there is ([drum racks](drum-racks.md)). (S9)
- **R10.** Each device's output is the same at any block size, and nothing on the audio thread allocates, locks or blocks. (S9)

**DS Kick, DS Snare, DS Tom and DS Clap (WP-8.24)**

- **R11.** DS Kick: a sine at Pitch (30 to 120 Hz, 50 Hz by default) whose pitch starts Env (0 to 100 %) × 4 octaves higher and falls exponentially to Pitch with a 30 ms time constant; Attack (0 to 100 %) fades the sine in over up to 10 ms, smoothing its start; OT (0 to 100 %) mixes in its second and third harmonics, up to −6 dB and −12 dB; Drive (0 to 100 %) drives it through a soft clipper by up to 24 dB with the output scaled back by half; Click (off by default) adds a 1 ms band-passed click at 4 kHz; Decay (default 400 ms). (S1; Beatlayer's own)
- **R12.** DS Snare: a body of two sines at 185 Hz and 330 Hz times 2^(Tune / 12), each falling a semitone over 20 ms, whose Color (0 to 100 %) morphs them towards triangles; and noise from the device's seed through the chosen filter (low-pass, high-pass or band-pass, at 2 kHz to 8 kHz with Color), mixed in by Tone (0 to 100 %); the body decays four times faster than the noise, the noise over Decay (default 250 ms). (S2; Beatlayer's own)
- **R13.** DS Tom: a sine at Pitch (40 to 400 Hz, 110 Hz by default) whose pitch starts Bend (0 to 100 %) × an octave higher and falls to Pitch over half of Decay; Tone (0 to 100 %) mixes in four resonant band-passes at the circular membrane's next ratios (physical models R1: 1.594, 2.136, 2.296, 2.653 times Pitch) excited by the hit; Color (0 to 100 %) raises a low-pass's cutoff from 1 kHz to 10 kHz and its gain by up to 6 dB; Decay (default 500 ms). (S3, S9; Beatlayer's own)
- **R14.** DS Clap: four bursts of noise from the device's seed, 8 ms apart, each decaying in 6 ms, through a band-pass at 1.2 kHz times 2^(Tune / 12), then Tail (0 to 100 %) adds band-passed noise decaying over Decay (default 300 ms); the claps go through two delay lines panned left and right whose times differ by Sloppy (0 to 30 ms), so they line up tightly at 0; Spread (0 to 100 %) sets the pan from mono to hard left and right; Tone (0 to 100 %) tilts the band-pass's brightness. (S4; Beatlayer's own)

**DS HH, DS Cymbal, DS FM and DS Clang (WP-8.25)**

- **R15.** DS HH: six square waves at inharmonic ratios (1, 1.342, 1.2312, 1.6532, 1.9523, 2.1523) times a base of 400 Hz × 2^(Pitch / 12), band-limited, summed with noise (white or pink, white by default) through a resonant high-pass at Tone (2 to 16 kHz) of 12 or 24 dB per octave (12 by default); Attack (0 to 100 %) fades the squares in over up to 20 ms, the noise starting at once; Decay (default 120 ms). (S5; Beatlayer's own)
- **R16.** DS Cymbal: the same six inharmonic squares at a base of 500 Hz × 2^(Pitch / 12), band-limited at every pitch, with noise, through a high-pass at Tone (1 to 12 kHz), decaying over Decay (default 1.5 s). (S6; Beatlayer's own)
- **R17.** DS FM: two operators of the FM core ([Operator](operator.md) R1, R3), a modulator into a carrier at 200 Hz × 2^(Pitch / 12); Amnt (0 to 100 %) sets the index up to 8, the modulator's envelope decaying twice as fast as the carrier's; Feedb. (0 to 100 %) is the modulator's feedback; Mod (0 to 100 %) blends the modulator's ratio through 1, 1.41, 2.76 and 3.5 in turn; Tone (200 Hz to 20 kHz) is a low-pass's cutoff; Decay (default 300 ms). (S7, S9; Beatlayer's own)
- **R18.** DS Clang: two square waves at 540 Hz and 800 Hz times 2^(Pitch / 12), band-limited, their levels Tone A and Tone B, through a high-pass and a band-pass at Filter, with white noise mixed in by Noise; with Clave on (off by default) the hit is a clave instead, a 2.5 kHz sine burst decaying in 20 ms, sounded Repeat times (1 to 8) 25 ms apart, each repeat on the frame that follows the hit's start by a whole number of 25 ms at any block size; Decay (default 250 ms). (S8; Beatlayer's own)
- **R19.** The metallic devices (DS HH, DS Cymbal, DS Clang) fold back nothing above −80 dB at the highest Pitch. (S5, S6, S8)
- **R20.** Every device of this part plays from a Drum Rack pad and keeps R1 to R10. (S9)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R7 | `crates/devices/tests/banks.rs`; `crates/devices/tests/rules.rs` | unit |
| R2, R3, R4, R5 | to be written in WP-8.24 | render under the allocation trap |
| R6 | to be written in WP-8.24 | UI script; agent API (`device.preview_hit`, `device.display`) |
| R8, R10 | to be written in WP-8.24 | render at block sizes 1, 64, 512 and 4,096; repeated renders identical |
| R9 | to be written in WP-8.24 | engine test (a Drum Rack with a choke group) |
| R11, R12, R13, R14 | to be written in WP-8.24 | unit and golden render of each device (pitch sweep, click, noise filter types, bend, spread and tail) |
| R15, R16, R17, R18, R19 | to be written in WP-8.25 | unit and golden render (noise colours and slopes; FM feedback and amount; the clave's repeats on their frames; fold-back) |
| R20 | to be written in WP-8.25 | engine test |
