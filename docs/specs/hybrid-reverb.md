# Hybrid Reverb: a convolution engine and an algorithmic one, in series or parallel

- **Status:** draft
- **Work packages:** WP-8.34 (part 1: the signal flow and routing, the input section, the convolution engine with its chooser and the person's own responses, Attack, Decay and Size, the EQ, the output section, the display, the algorithmic engine's shared controls and Dark Hall: R1 to R22); WP-8.35 (part 2: Quartz, Shimmer, Tides and Prism, and Freeze across the algorithms: R23 to R30). Impulse-response slots, the chooser and the bundled responses are [impulse responses](impulse-responses.md)'; the convolution engine is Beatlayer's own (`bl_dsp::convolution`, [the architecture](../02-architecture.md)); the framework is WP-2.5's ([the native device framework](device-framework.md)), its display path WP-5.43's
- **Parity-matrix row:** Audio effects: Echo, Hybrid Reverb, Roar, Spectral Resonator, Spectral Time
- **Checked against:** the Live 12 reference manual (online edition), chapter 29 "Live Audio Effect Reference", section 29.23 "Hybrid Reverb" with 29.23.1 to 29.23.6, read 30 October 2026; the Live 12.3 release notes

## The behaviour

Hybrid Reverb joins two reverbs. Its convolution engine plays an impulse response, bundled or the person's own, shaped by Attack, Decay and Size; its algorithmic engine is one of five designs (Dark Hall, Quartz, Shimmer, Tides and Prism), each with its own controls. The two run in series (the convolution feeding the algorithm) or in parallel, Blend setting how much of each is heard, or one alone. Before them an input section sends the signal in with a predelay that can feed back; after them an EQ shapes the reverb, before or after the algorithm, and an output section sets its width, gives it a vintage character, keeps its bass in mono and mixes it with the dry signal. Freeze holds the algorithm's tail indefinitely.

The device takes Live's name with Beatlayer's own type ID, `beatlayer.hybrid_reverb`. The five algorithms are Beatlayer's own designs under Live's names, written here as their structures; so is Vintage's character.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 29, section 29.23 "Hybrid Reverb" and 29.23.1 "Signal Flow" | Input, then the two engines, their routing, the EQ and the output; the routing chooser Serial, Parallel, Algorithm and Convolution; Blend in Serial and Parallel, all convolution at one end and all algorithm at the other; the display shows the impulse response |
| S2 | Manual | Section 29.23.2 "Input Section" | Send sets how much signal enters the reverb; Predelay, in milliseconds or synced (Predelay Sync), typically 1 to 25 ms; Feedback returns the predelay's output to its input, set apart for the time and the synced predelay |
| S3 | Manual | Section 29.23.3 "Convolution Reverb Engine" | The impulse-response choosers (category and response, with arrows); Attack, Decay and Size shape the response's envelope and length; dropped files become responses |
| S4 | Manual | Section 29.23.4 "Algorithmic Reverb Engine" with 29.23.4.1 to 29.23.4.5 | The Algorithm chooser; Decay (time to −60 dB), Size, Delay, Freeze and Freeze In shared by the algorithms; Dark Hall's Damping, Mod, Shape, Bass X and Bass Mult; Quartz's Damping, Lo Damp, Mod, Diffusion and Distance; Shimmer's Damping, Mod, Pitch, Diffusion and Shimmer; Tides' Damping, Wave (noise at 0 %, sine at 50 %, square at 100 %), Tide, Phase (0 to 180°) and Rate; Prism's Low Mult, High Mult and X over |
| S5 | Manual | Section 29.23.5 "EQ Section" | The EQ switch and Pre Algo; a low and a high band, each a pass filter (6 to 96 dB per octave) or a shelf; two peak bands between them |
| S6 | Manual | Section 29.23.6 "Output Section" | Stereo (mono at 0 %, wider above 100 %); Vintage (Subtle, Old, Older, Extreme); Bass Mono (the reverb mono below 180 Hz); Dry/Wet |
| S7 | Release notes | Live 12.3 release notes, Hybrid Reverb | A tail no longer stays audible after the device is switched off and on again |
| S8 | Beatlayer | [Impulse responses](impulse-responses.md); [the device framework](device-framework.md) R4, R26 to R29, R35 to R38; [control surfaces](control-surfaces.md) R19; [the improvements](../05-improvements.md) G1 and G2; [the reverb](reverb.md); [the pitch and frequency shifter](pitch-and-frequency-shifter.md) R3 | Slots and the chooser; smoothing, the LFO, seeds, display data; banks; gains to −∞ dB and automatable parameters; Beatlayer's algorithmic reverb's parts; delay-line pitch shifting |
| S9 | Paper | J.-M. Jot and A. Chaigne, "Digital delay networks for designing artificial reverberators", AES Convention 90, 1991 | Feedback delay networks with a unitary feedback matrix and per-line absorption filters set from a reverberation time per band |

## Rules

### The device, the routing and the input (WP-8.34)

- **R1.** Hybrid Reverb (`beatlayer.hybrid_reverb`) is an audio effect with Live's name, Beatlayer's own layout, one impulse-response slot named IR ([impulse responses](impulse-responses.md) R1) holding a bundled hall in a new device, and one display cell. Its body reads, left to right, the input section, the convolution engine's controls, the algorithmic engine's controls, and the output section; an EQ switch in the title row shows the EQ section in place of the engines' controls, as view state saved with the set and not an undo step. (S1, S5; the starting response and saving the view are Beatlayer's own)
- **R2.** Routing is Serial, Parallel, Algorithm or Convolution. Serial feeds the convolution engine's output into the algorithmic engine and mixes the two outputs by Blend; Parallel feeds both from the input section and mixes their outputs by Blend; Algorithm and Convolution run one engine alone and ignore Blend. Blend, 0 to 100 %, mixes with equal-power laws: 0 % is the convolution engine alone and 100 % the algorithmic engine alone. (S1; equal power and which end is which are Beatlayer's own)
- **R3.** Send is a gain from −∞ to 0 dB (0 dB by default) on the signal entering the reverb (rule G1). Predelay is, with Predelay Sync off, a time from 0.5 to 250 ms (logarithmic, 10 ms by default) or, with it on, a division from 1/64 note to 1 bar, including triplet and dotted divisions, landing on its frame through the tempo map. Feedback returns the predelay's output to its input with a gain from 0 to 95 %, a parameter of its own for the time and for the synced predelay, so echoes of the predelay enter the engines at multiples of its time. (S2; ranges and defaults are Beatlayer's own)

### The convolution engine (WP-8.34)

- **R4.** The convolution engine convolves the input section's output with the slot's response on the shared engine, with no latency, the response chosen and loaded as [impulse responses](impulse-responses.md) R2 to R14 say, the choosers placed at the top of the convolution engine's controls. (S3, S8)
- **R5.** Attack, 0 to 1,000 ms (0 by default), fades the response in over its first Attack milliseconds along a raised-cosine curve. Decay, 50 ms to 30 s (30 s by default, which leaves the response as it is), shapes its tail with an exponential envelope that reaches −60 dB at Decay after its start. Size, 20 to 100 % (100 % by default), plays the response shortened in time by that factor, resampled so a reflection at t seconds arrives at Size × t, its pitch rising with it as a smaller room's does. (S3; the ranges, curves and Size's law are Beatlayer's own)
- **R6.** A change of Attack, Decay or Size makes a new prepared response on a loader thread from the decoded one, which the engine crossfades to over 50 ms; a drag makes at most one new response every 50 ms, and the last value always arrives. The audio thread does none of this work. (S8; Beatlayer's own)

### The algorithmic engine's shared controls and Dark Hall (WP-8.34)

- **R7.** The algorithmic engine takes the Algorithm chooser (Dark Hall, Quartz, Shimmer, Tides, Prism; Dark Hall in a new device) and the shared controls Decay (0.2 to 60 s, the time for the tail to fall 60 dB at 1 kHz, 2 s by default), Size (0 to 100 %, scaling the network's delay lengths between a small room's and a large hall's, 50 % by default) and Delay (0 to 250 ms, the algorithm's own predelay after the input section's, 0 by default). Each algorithm's own controls show beside them only while it is chosen; the others keep their values. (S4; ranges and defaults are Beatlayer's own)
- **R8.** Dark Hall is an eight-line feedback delay network with a Householder feedback matrix, whose lines' lengths are mutually prime and spread over 25 to 120 ms at Size 50 %, each line with an absorption filter setting its decay so a tone at 1 kHz falls 60 dB in Decay within 10 %. Damping (0 to 100 %) lowers a first-order low-pass in each line's absorption, so the highs decay faster; Mod (0 to 100 %) moves each line's length by up to ±1 ms with its own sine between 0.1 and 1 Hz, seeded per device; Shape (0 to 100 %) sets the input diffusion's density, from four allpasses passing early echoes at 0 % to a smooth onset at 100 %; Bass X (50 to 800 Hz) is the crossover below which Bass Mult (0.5 to 3, ×) multiplies the decay time. (S4, S9; the design and ranges are Beatlayer's own)

### The EQ (WP-8.34)

- **R9.** With EQ on, four bands act on the reverb: Low and High, each a high-pass and low-pass of 6, 12, 24, 48 or 96 dB per octave or a shelf with its gain (−18 to +18 dB), and two peak bands with frequency (20 Hz to 20 kHz), gain (−18 to +18 dB) and Q (0.3 to 10). Each band's response, measured on the wet output with Dry/Wet 100 % and a unit impulse as the response, is within 0.5 dB of its design. Pre Algo on puts the EQ before the algorithmic engine (after the convolution engine in Serial); off, after both engines. (S5; ranges are Beatlayer's own)

### The output (WP-8.34)

- **R10.** Stereo, 0 to 200 % (100 % by default), scales the reverb's side against its mid: mono at 0 %, unchanged at 100 %, the side doubled at 200 %. (S6; the law above 100 % is Beatlayer's own)
- **R11.** Vintage is Off, Subtle, Old, Older or Extreme. Off passes the reverb unchanged; each other step runs the reverb through a sample-rate reduction with linear interpolation (to 32, 24, 16 and 10 kHz), a bit-depth reduction (to 16, 12, 10 and 8 bits, with triangular dither from the device's seed) and a low-pass at a third of the reduced rate. (S6; the character is Beatlayer's own)
- **R12.** With Bass Mono on, the reverb below 180 Hz is mono: split by a fourth-order Linkwitz-Riley crossover that sums flat within 0.1 dB, its low band's left and right replaced by their mean. (S6; the crossover is Beatlayer's own)
- **R13.** Dry/Wet, 0 to 100 % (50 % by default), balances the dry signal against the reverb. (S6; the default is Beatlayer's own)

### The display (WP-8.34)

- **R14.** The display draws the slot's response as a waveform over time, the response's amplitude envelope with Attack, Decay and Size applied drawn over it, and the response's category and name; dragging on it moves Attack (from the left), Decay (from the right) and Size (vertically), each drag one undo step. A file dropped on it loads as [impulse responses](impulse-responses.md) R14 says. Its data (the response's peak envelope in 256 points, the applied envelope, the length in seconds) is published as device framework R35 says and read by `device.display`. (S1, S3, S8; the look and the drags are Beatlayer's own)

### Throughout (WP-8.34)

- **R15.** Switching the device off and on clears every tail: the engines' lines and the convolution's history are silent when it comes back on. (S7)
- **R16.** The device reports no latency: the convolution engine adds none and the algorithms none. (S3, S8)
- **R17.** A render is bit-identical at block sizes 1, 64, 512 and 4,096 and run after run, with Mod and every seeded part on. (S8)
- **R18.** Nothing on the audio thread allocates, locks or blocks: the networks' lines are made with the node for the largest Size; responses arrive prepared ([impulse responses](impulse-responses.md) R4). (S8)
- **R19.** Every parameter is automatable and modulatable (rule G2) and set and read back through the agent API; every gain reaches −∞ dB. Changing Algorithm crossfades over 50 ms from the old algorithm's output to the new one's, with no click, as one undo step. (S8)
- **R20.** Hybrid Reverb declares its banks of eight ([control surfaces](control-surfaces.md) R19): Main (Send, Predelay, Routing, Blend, Decay, Size, Stereo, Dry/Wet), Input, Convolution, Algorithm (the chosen algorithm's own controls take this bank's places, as the device shows them), EQ and Output, so every automatable parameter is in a bank. (S8; the banks are Beatlayer's own)
- **R21.** A set with Hybrid Reverb saves and reopens with every parameter, its response and its view. (S8)
- **R22.** Every control takes the keyboard focus in the Device View and assistive technology reads each with its name and value. (S8)

### The other algorithms and Freeze (WP-8.35)

- **R23.** Quartz is an eight-line feedback delay network with an input diffusion chain whose density Diffusion (0 to 100 %) sets, so low settings leave distinct early echoes; Damping (0 to 100 %) lowers a low-pass and Lo Damp (0 to 100 %) raises a high-pass in each line's absorption; Mod moves the lines' lengths as Dark Hall's does; Distance (0 to 100 %) moves the balance from the early echoes to the late tail and adds up to 40 ms before the late part. Its decay at 1 kHz is within 10 % of Decay. (S4, S9; the design is Beatlayer's own)
- **R24.** Shimmer is a loop of eight allpasses and delays with a pitch shifter in its feedback path: each pass through the loop sends Shimmer's share (0 to 100 %) of the signal through a delay-line pitch shifter moving it by Pitch (−24 to +24 semitones, +12 by default), as [the pitch and frequency shifter](pitch-and-frequency-shifter.md) R3 shifts, so a tone gains partials at Pitch's interval above it within 5 cents, then at twice it, each fainter. Diffusion (0 to 100 %) sets the allpasses' gain; below 10 % the loop sounds as discrete echoes. Damping and Mod act as Dark Hall's. The loop's gain is capped below 1 at every frequency whatever Decay, Shimmer and Pitch say. (S4, S8; the design and the cap are Beatlayer's own)
- **R25.** Tides is an eight-line feedback delay network whose lines' lengths are moved by an LFO: Wave sets its shape, from noise (a random line, seeded) at 0 % through a sine at 50 % to a square at 100 %, blending between; Tide (0 to 100 %) sets its depth, up to ±5 ms; Phase (0 to 180°) puts the right channel's motion ahead of the left's; Rate is a synced division (1/32 note to 8 bars, with triplet, sixteenth and dotted counts as the manual's modes give), its cycles on the beat grid as device framework R27 says. Damping acts as Dark Hall's. The lines' motion is at its rate within 0.1 %. (S4, S8; the design is Beatlayer's own)
- **R26.** Prism is two eight-line feedback delay networks split by a fourth-order Linkwitz-Riley crossover at X over (100 Hz to 8 kHz): the low network's decay time is Decay × Low Mult and the high network's Decay × High Mult (each 0.25 to 4, ×), each within 10 % in its band, and the two outputs sum flat within 0.1 dB with both Mults at 1. (S4; the design is Beatlayer's own)
- **R27.** Freeze holds the algorithmic engine's tail: with it on the network's feedback is made lossless (the absorption filters and Damping set aside, the feedback matrix unitary), so the tail's level stays within 0.5 dB over 30 s and never grows; Shimmer's pitch path and Tides' motion are held still while frozen. With Freeze In on, the input still enters the frozen network; with it off, nothing enters. Turning Freeze off returns the network to its decay from where it is. (S4; lossless and the held motion are Beatlayer's own)
- **R28.** Each algorithm's decay at 1 kHz, measured as RT30 on the wet output, is within 10 % of Decay from 0.5 to 20 s. Each algorithm costs within a factor of two of Dark Hall's processing time per block. (S4; Beatlayer's own)
- **R29.** Switching algorithm crossfades over 50 ms as R19 says and is one undo step; the new algorithm starts with empty lines. (S8)
- **R30.** Every part-2 algorithm renders bit-identically at block sizes 1, 64, 512 and 4,096, and allocates, locks and blocks nothing on the audio thread. (S8)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R3 | to be written in WP-8.34 | unit, real-time trap (routing paths and Blend's laws; Predelay on its frame, synced and free; predelay Feedback's echoes) |
| R4 to R6 | to be written in WP-8.34 | unit on generated responses (a unit impulse, a decaying noise): Attack's and Decay's envelopes, Size's arrival times; a drag's rate of new responses |
| R7, R8 | to be written in WP-8.34 | unit (Dark Hall's RT30 per band, Bass Mult below Bass X, seeded Mod) |
| R9 to R13 | to be written in WP-8.34 | unit (each EQ band's response; Stereo; each Vintage step; Bass Mono's crossover sum) |
| R14 | to be written in WP-8.34 | agent API (`device.display`); UI script (a drop, a drag) |
| R15 to R18 | to be written in WP-8.34 | unit (off and on clears tails); real-time trap, RealtimeSanitizer; golden renders at four block sizes |
| R19 to R22 | to be written in WP-8.34 | `crates/devices/tests/rules.rs`; agent API; `crates/devices/tests/banks.rs`; save and reopen; UI script with the accessibility tree |
| R23 to R26, R28 | to be written in WP-8.35 | unit (each algorithm's RT30; Shimmer's partials within 5 cents; Tides' rate; Prism's bands); timing comparison |
| R27 | to be written in WP-8.35 | unit (frozen level within 0.5 dB over 30 s; Freeze In) |
| R29, R30 | to be written in WP-8.35 | unit (click-free switch, one undo step); real-time trap; golden renders at four block sizes |
