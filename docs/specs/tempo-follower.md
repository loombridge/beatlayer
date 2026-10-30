# Tempo Follower: the song's tempo following incoming audio

- **Status:** draft
- **Work packages:** WP-9.12 (the real-time tempo tracker in `bl-analysis` and the copy of incoming audio off the audio thread: R11 to R18); WP-9.13 (the settings, the Follow toggle, the tracker as the engine's follower, EXT, Link and the commands: R1 to R10, R19 to R24). The engine's way of playing at a followed tempo is [transport](transport.md) R24 to R26; EXT's side is [MIDI clock](midi-clock.md) R15 and R28; Link's is [Link](link.md) R26; the Control Bar's toggle is [Control Bar transport](control-bar-transport.md) R24 to R26
- **Parity-matrix row:** Recording: Tempo Follower
- **Checked against:** the Live 12 reference manual (online edition), chapter 37 sections 37.2 and 37.2.1 and section 37.1.2, read 30 October 2026; the UI parity reference rows L-CTRL-003 and L-SET-014

## The behaviour

Tempo Follower listens to one of the audio interface's inputs and moves the song's tempo to follow what it hears, such as a drummer's own timing or a DJ's turntables, where Link or MIDI clock can't be used. It's set up on the Tempo & MIDI page of Settings: Show Tempo Follower Toggle puts a Follow toggle in the Control Bar's tempo group, and Input Channel chooses what it listens to. With Follow on, Beatlayer analyses that input in real time and the tempo follows; it never runs while the toggle is hidden. If the input can't be reached, Follow is greyed out.

Tempo Follower and External Sync exclude each other: while Follow is on, EXT is switched off and disabled, though Beatlayer still sends MIDI clock. With Link on too, the followed tempo is the tempo Beatlayer gives the Link session.

Beatlayer's tracker is its own: incoming audio is copied off the audio thread, a tracker thread measures onset strength, estimates the tempo and tracks the beat, and the engine plays each block at the tempo it last gave, through the follower interface Link and MIDI clock use.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 37 "Synchronizing with Link, Tempo Follower, and MIDI", section 37.2 | Tempo Follower is for where Link or MIDI clock isn't available or wanted: it analyses an incoming audio signal in real time and changes Live's tempo to match, such as a drummer's timing or a DJ's turntables |
| S2 | Manual | The same chapter, section 37.2.1 | Tempo & MIDI's Input Channel chooses the interface input carrying the source, such as a drum overhead or a DJ mixer's record out; clearly rhythmic material works best. Show Tempo Follower Toggle puts a Follow toggle at the left of the Control Bar beside the other tempo controls; switching Follow on starts Tempo Follower, which listens to the input, and it doesn't run while the toggle is hidden. If Live can't reach the input, the feature is disabled and Follow is greyed out. External Sync is disabled while Tempo Follower is active; Live can still send MIDI clock but not receive it. With Link on, the tempo from the input is used for the Link session |
| S3 | Manual | The same chapter, section 37.1.2 | A tempo change from a Link participant overrides the set's tempo automation |
| S4 | Reference | [layout.md](../ui-parity/layout.md) L-CTRL-003 (the Tempo Follower toggle in the tempo group, hidden by default, shown by Show Tempo Follower Toggle) and L-SET-014 (the Tempo & MIDI page's Tempo Follower section: Show Tempo Follower Toggle, Input Channel) | Where the controls sit |
| S5 | Paper | Ellis, "Beat Tracking by Dynamic Programming", Journal of New Music Research, 2007; Davies and Plumbley, "Context-Dependent Beat Tracking of Musical Audio", IEEE Transactions on Audio, Speech and Language Processing, 2007; Stark, Davies and Plumbley, "Real-Time Beat-Synchronous Analysis of Musical Audio", DAFx 2009; Böck and Widmer, "Maximum Filter Vibrato Suppression for Onset Detection", DAFx 2013 | Onset strength from spectral flux, tempo induction by autocorrelation or comb filters over it, and beat tracking that runs causally in real time |
| S6 | Spec | [transport](transport.md) R24 to R29 (a follower asked once per block, a per-block song speed, starts a follower decides, the tempo map coming back, Phase Nudge refused while a follower leads); [MIDI clock](midi-clock.md) R9, R15 and R28; [Link](link.md) R3, R4 and R10; [automation](automation.md) R5 and R8; [Control Bar transport](control-bar-transport.md) R6; the architecture's "Real-time rules" | How following works in the engine; EXT and clock out; Link's tempo; a hand move overriding an envelope |

## Rules

### Settings and the toggle (WP-9.13)

- **R1.** The Tempo & MIDI page of Settings (L-SET-014) starts with a Tempo Follower section: Show Tempo Follower Toggle (a chooser of Show and Hide, Hide on a new install) and Input Channel (a chooser of the audio input's channels as mono channels and stereo pairs, by the names the Input Config gives them, the first mono channel on a new install). Both are kept in the preferences, not in a set. (S2, S4; the default channel and keeping them in the preferences are Beatlayer's own)
- **R2.** With Show Tempo Follower Toggle on Show, the Follow toggle (L-CTRL-003) shows in the Control Bar's tempo group at its left ([Control Bar transport](control-bar-transport.md) R24). Hiding it switches Follow off; Tempo Follower never runs while the toggle is hidden. (S2, S4)
- **R3.** Follow (`tempo_follower.set_enabled`) is off on a new install and when Beatlayer starts, and isn't kept in a set. Switched on, Beatlayer listens to the input channel and the song's tempo follows it (R6). (S2; off at start is Beatlayer's own)
- **R4.** While the input channel can't be reached (no audio device, the channel not on the device, or the device's inputs off), Follow is greyed out, `tempo_follower.set_enabled` is refused with the reason, and Follow already on goes off, giving the song back its tempo (R9). (S2)
- **R5.** While Follow is on, EXT and Options ▸ External Sync are switched off and disabled ([MIDI clock](midi-clock.md) R15, R28), and an incoming clock isn't followed; MIDI clock out goes on at the followed tempo ([MIDI clock](midi-clock.md) R9). Switching Follow off leaves EXT off. (S2)

### Following (WP-9.13)

- **R6.** With Follow on and the tracker confident (R14), Beatlayer's follower gives the engine the tracker's tempo once per block ([transport](transport.md) R24): the song plays each block at that tempo, and everything in song time follows it. The followed tempo becomes the set's tempo, shown in the Tempo field and saved with the set, with no undo step of its own. (S1, S6; no undo step is Beatlayer's own, as Link's is)
- **R7.** The tempo follows only, not the beat's phase: Beatlayer's beat isn't moved to land on the incoming beat, and a start isn't held for one; Play starts at once at the followed tempo. (S1; Beatlayer's own)
- **R8.** While the song has tempo automation, a followed tempo change overrides it as a hand move does ([automation](automation.md) R5): the envelope stops playing and Re-Enable Automation lights ([automation](automation.md) R8). While a clip leads the tempo ([warping](warping.md) R44), the followed tempo wins and the leader plays as a follower, as under EXT ([warping](warping.md) R55). (S3, S6; following Link's rule is Beatlayer's own)
- **R9.** When Follow goes off, the next block plays at the tempo the set now has, with the song's beat where it was ([transport](transport.md) R26). (S6)
- **R10.** The tracker listens while the transport is stopped, so the tempo is already followed when Play is pressed; the Tempo field follows while stopped too. (S2; Beatlayer's own)

### The tracker (WP-9.12)

- **R11.** The audio callback copies the chosen input channel, summed to mono for a pair, into a lock-free single-producer ring for the tracker; it allocates nothing, takes no lock and doesn't block, and when the ring is full it drops the block and counts it. Nothing else of the tracker runs on the audio thread. (S6)
- **R12.** The tracker runs on a thread of its own, on frames of about 10 ms (the device rate downsampled to 11,025 Hz, frames of 128 samples with a hop of 110): an onset strength from half-wave rectified spectral flux with a maximum filter across frequency, a tempo estimate from a comb-filter bank over the last 6 seconds of onset strength from 60 to 200 BPM, and a causal beat tracker that predicts each next beat from the estimate. (S5; the numbers are Beatlayer's own)
- **R13.** Between tempos in a ratio of 2 or 1/2, the tracker keeps the one nearest the tempo it is following, and at the start the one nearest the set's tempo; a change to a tempo in such a ratio needs 4 seconds of agreeing evidence. (S5; Beatlayer's own)
- **R14.** The tracker gives a tempo, a beat phase and a confidence from 0 to 1. Below a confidence of 0.3 (silence, a held chord, noise) the follower keeps the last confident tempo. The tempo it gives moves smoothly, by at most 2 % a second, except when it first becomes confident. (S5; the numbers are Beatlayer's own)
- **R15.** On generated drum loops from 80 to 180 BPM, straight and swung, the given tempo settles within ±0.5 BPM of the loop's within 8 seconds of the loop starting, and stays there. (S5; Beatlayer's own tolerance)
- **R16.** Across a ramp of 8 % over 16 bars the given tempo lags the true tempo by no more than 1.5 BPM. (Beatlayer's own tolerance)
- **R17.** On the generated loops at 80 to 180 BPM, fewer than 5 % of runs settle on half or double the loop's tempo. (Beatlayer's own tolerance)
- **R18.** Fed the same audio from the same start, the tracker gives the same tempos. Tests feed it audio directly on the test clock. (S6)

### Commands, readback and assistive technology (WP-9.13)

- **R19.** `tempo_follower.set_enabled` (`edit`, as it changes the set's tempo), `settings.set_show_tempo_follower_toggle` and `settings.set_tempo_follower_input` (`edit`, kept preferences) each name their readback; `tempo_follower.state` (`read`) gives whether Follow is on, whether it can run and why not, the input channel, the tempo it follows, the tracker's confidence and beat phase, and the blocks dropped. (S6)
- **R20.** With Link on and Follow on, the followed tempo is Beatlayer's tempo for the Link session ([Link](link.md) R26). (S2)
- **R21.** Phase Nudge is refused while Follow leads ([transport](transport.md) R29). Tap Tempo is refused while Follow is on, with the reason. (S6; Beatlayer's own)
- **R22.** An offline render doesn't follow: it plays the set's own tempo map. (Beatlayer's own, as [Link](link.md) R14)
- **R23.** The Follow toggle is a switch named Tempo Follower in the accessibility tree, its value on or off, its description the followed tempo or the reason it's greyed out. (S6; the name is Beatlayer's own)
- **R24.** The settings rows, the toggle and the Tempo field redraw from what the engine and the tracker last published, without waiting on either. (S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R5 | to be written in WP-9.13: `crates/shell/tests/tempo_follower.rs` (real preference files, an unavailable input); `crates/ui/tests/tempo_follower.rs` | shell test, UI script |
| R6 to R10 | to be written in WP-9.13: `crates/engine/tests/tempo_follower.rs`, a recorded tracker output at block sizes 1, 61 and 512 under the allocation trap | engine test |
| R11 to R18 | to be written in WP-9.12: `crates/analysis/src/tempo_tracker.rs` on generated loops, ramps, silence, noise and a held chord; `rtsan/tests/tempo_follower.rs` for the copy | unit, RealtimeSanitizer |
| R19 to R24 | to be written in WP-9.13: `crates/agent/tests/tempo_follower.rs`; `cargo xtask commands` | agent API, UI script |
