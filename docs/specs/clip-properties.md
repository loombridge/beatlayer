# The Clip View's Main Properties panel: Start and End, the loop, Duplicate and the clip's Signature

- **Status:** draft
- **Work packages:** WP-6.55 (R1 to R12). The panel's Groove controls are [grooves.md](grooves.md) R29's (WP-6.20) and its scale controls [scales.md](scales.md) R19's (WP-6.7b); the Launch Properties panel beside it is [launch-quantisation.md](launch-quantisation.md)'s (WP-3.9); the panels' right group is the MIDI Tools' ([midi-tools.md](midi-tools.md) R1, WP-6.11) on a MIDI clip and the Audio Utilities panel's ([sample-editor.md](sample-editor.md) R14, WP-4.10b) on an audio clip; Duplicate Loop is [multi-clip-editing.md](multi-clip-editing.md) R11's, and editing several clips' properties at once that spec's R9; the markers and loop themselves are [midi-clips.md](midi-clips.md)'s, [audio-clips.md](audio-clips.md)'s and [warping.md](warping.md)'s
- **Parity-matrix row:** Clips and launching: The Clip View's Main Properties panel: Start and End with Show and Set, Duplicate, Loop, Position and Length, Signature
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026; the UI parity reference rows L-CLIP-002 to L-CLIP-005, L-CLIP-008, L-CTRL-010, M-NAV-011 and S-FOCUS-010

## The behaviour

The Clip View's panels sit at its left, in two groups under a row of selector buttons. The left group shows one panel: Main Properties, or, for a Session clip, Launch Properties. Main Properties is the same for MIDI and audio clips, in the Arrangement and in Session slots. Its Start and End fields set where the clip starts and stops playing; its Loop switch makes it loop over the brace that Position and Length set; Duplicate doubles the loop; and its Signature fields give the clip a time signature of its own, which changes how the clip's positions read and nothing it plays. Each field has a Show button, which brings its marker into the editor's view, and a Set button, which moves the marker to where the playing clip has reached, on the global launch quantisation's grid. The panel ends with the clip's groove and scale controls.

Every control runs a registered command that an agent runs too; most of them exist since the clips' own packages, and the panel only draws them. This spec covers the panel's place, its controls and what each runs.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 8 "Clip View", sections 8.1 "Clip View Layout" and 8.1.2 "Clip Panels" | The Clip View has a title bar, the clip panels at its left and the editor at its right; both kinds of clip have the Main Clip Properties panel, with region and loop settings, time signature, groove, scale and, for Session clips, the launch settings; audio clips also have the Audio Utilities and Transform panels, MIDI clips the Pitch and Time Utilities, Transform and Generate panels |
| S2 | Manual | Same chapter, section 8.2.1 "Clip and Loop Region Settings" | Start and End set what plays on launch, typed into their fields; during playback Set Start and Set End take the playhead's position, on the global quantisation's grid; warped audio shows bars, beats and sixteenths and unwarped audio minutes, seconds and milliseconds; the Clip Loop switch loops over the area Loop Position and Loop Length give, Length starting as the clip's length; an audio clip loops only with Warp on; during playback Set Loop Position moves the loop's start to the playhead, rounded to the global quantisation, and switches the loop on, and Set Loop Length then moves its end to the playhead; with the loop off, Set Loop Length ends the loop at the playhead keeping its length, and switches the loop on |
| S3 | Manual | Same chapter, section 8.2.2 "Clip Time Signature" | A MIDI or audio clip's Signature fields set a time signature of its own, apart from the song's, for display only: it doesn't change playback |
| S4 | Manual | Same chapter, section 8.9 | Duplicate Loop doubles the loop's length and contents |
| S5 | Beatlayer | The UI parity reference: [layout.md](../ui-parity/layout.md) L-CLIP-002 (the title bar), L-CLIP-003 (the selector row: Main Properties and Launch Properties in the left group, the clip's other panels in the right group), L-CLIP-004 (the Main Properties panel's controls in order, x 251 to 417 at 1600 × 1000; Lengths count from 0, so a one-bar Length reads 1. 0. 0; Start takes positions before 1.1.1, typing 0.3.1 giving -1. 3. 1), L-CLIP-005 (Launch Properties, Session clips only), L-CLIP-008 (the Audio Utilities panel), L-CTRL-010 (the global launch quantisation); [menus.md](../ui-parity/menus.md) M-NAV-011 and [shortcuts.toml](../ui-parity/shortcuts.toml) S-FOCUS-010 (Navigate ▸ Clip Panels, Option-Shift-P) | Where the panel sits, what it holds and in what order, and how its values read |
| S6 | Beatlayer | [midi-clips.md](midi-clips.md) (a MIDI clip's Start and End markers, loop brace and Signature, in ticks of its contents), [audio-clips.md](audio-clips.md) and [warping.md](warping.md) R14 (an audio clip's markers in seconds while unwarped and in beats while warped; only a warped clip loops), [musical-time.md](musical-time.md) (numerators 1 to 99, denominators 1, 2, 4, 8 and 16; how positions read), [multi-clip-editing.md](multi-clip-editing.md) R9 and R11, [launch-quantisation.md](launch-quantisation.md) | What the controls set, and the rules that already hold for them |
| S7 | Beatlayer | [The architecture](../02-architecture.md), "The document", "Agent control" and decision D17; [accessibility.md](accessibility.md) | Every edit is a command and one undo step; every action is a registered command with a schema, a description and a permission class, and what an agent sets it can read back; every control reaches assistive technology |

## Rules

**The panel's place (WP-6.55)**

- **R1.** Whatever clip the Clip View shows, MIDI or audio, in the Arrangement or a Session slot, the panels sit at its left under their selector row (L-CLIP-003), the left group first. The left group's selector holds Main Properties and, for a Session clip, Launch Properties (L-CLIP-005); choosing one shows that panel in the left group. Which one shows is view state: not saved, not an undo step, Main Properties when the app starts (Beatlayer's own), and an Arrangement clip, which has no Launch Properties, always shows Main Properties. The right group, beside it, holds the clip's other panels. (S1, S5)
- **R2.** The Main Properties panel (L-CLIP-004) holds, top to bottom: Start and End, each with a Show and a Set button; Duplicate; the Loop switch; Position and Length, each with Show and Set; the Signature's numerator and denominator; the Groove controls ([grooves.md](grooves.md) R29); and the scale controls ([scales.md](scales.md) R19). A control whose package hasn't landed is drawn in place, disabled, and named as unavailable to assistive technology. (S1, S5)

**Start and End (WP-6.55)**

- **R3.** Start and End show the clip's Start and End markers: on a MIDI clip and a warped audio clip as positions in bars, beats and sixteenths of the clip's contents, its beginning reading 1. 1. 1 and positions before it reading -1. 3. 1 and the like, in the clip's Signature (R9); on an unwarped audio clip as minutes, seconds and milliseconds into its file. Typing a value (Return to confirm, Esc to cancel) or dragging the field moves the marker by `clip.set_midi_region` or `clip.set_region`, one undo step; a value those commands refuse changes nothing and the field reads the marker again. (S2, S5, S6)
- **R4.** Show beside Start, End, Position or Length scrolls the clip's editor so that marker, or the loop brace's start or end, is in view (`clip_view.show_marker`, `read`); it changes nothing in the set (the scrolling is Beatlayer's own reading of the button). (S5)
- **R5.** Set beside Start or End, while the clip plays, moves that marker to where the clip has reached, on the nearest step of the global launch quantisation (L-CTRL-010) at or before it, or exactly where it has reached when the quantisation is None (Beatlayer's own rounding). While the clip isn't playing, Set is disabled (Beatlayer's own). (S2, S5)

**The loop and Duplicate (WP-6.55)**

- **R6.** The Loop switch shows and sets the clip's Loop (`clip.set_loop_enabled`, one undo step); on an unwarped audio clip it is disabled, as [warping.md](warping.md) R14 says. (S2, S6)
- **R7.** Position and Length show the loop brace: Position as a position, as R3 says, and Length as a duration counted from 0, so a one-bar loop reads 1. 0. 0. Typing or dragging either moves the brace by `clip.set_midi_loop` or `clip.set_loop`, the other kept, one undo step. (S2, S5, S6)
- **R8.** Set beside Position, while the clip plays, moves the brace's start to where the clip has reached, rounded as R5 says, and switches Loop on. Set beside Length moves the brace's end there, rounded the same way (Beatlayer's own for Length); with Loop off it moves the whole brace so that it ends there, keeping its length, and switches Loop on. Each is one undo step, and both are disabled while the clip isn't playing. (S2)
- **R9.** Duplicate runs Duplicate Loop on the clip or clips the Clip View shows (`clip.duplicate_loop`, [multi-clip-editing.md](multi-clip-editing.md) R11). (S4, S5, S6)

**The Signature (WP-6.55)**

- **R10.** The Signature's numerator (1 to 99) and denominator (1, 2, 4, 8 or 16) show the clip's own time signature, or, while it has none, the song's at the clip's start. Typing either sets the clip's own (`clip.set_signature`, `edit`, one undo step); `clip.set_signature` with no signature gives the clip back the song's. A clip's signature changes nothing it plays: its Start, End, Position and Length, and the editor's ruler and grid, count bars and beats in it (the ruler and grid are Beatlayer's own reading). On an unwarped audio clip the fields are disabled, as its positions read in time (Beatlayer's own). (S3, S5, S6)

**Several clips, the keyboard and agents (WP-6.55)**

- **R11.** With several clips selected, the Loop switch, Position, Length and the Signature act on every selected clip as [multi-clip-editing.md](multi-clip-editing.md) R9 says. Start and End, typed, set every selected clip's marker to the value, in one undo step; Show and Set act on the clip the Clip View shows (both Beatlayer's own). Navigate ▸ Clip Panels (M-NAV-011, Option-Shift-P, S-FOCUS-010) gives the panel's first control the keyboard focus, and Tab and Shift-Tab move through its controls in R2's order; assistive technology reads each field with its name and value as it reads, and each button with its name. (S5, S6, S7)
- **R12.** Every control runs a registered command an agent runs by ID: the marker and loop commands of R3, R6 and R7; `clip.set_signature` (`edit`), read back in the query tool's `clip` `data`; `clip.set_marker_at_playhead` (`edit`), which does what a Set button does for the marker it names (`start`, `end`, `loop_position` or `loop_length`) and is refused while the clip isn't playing, read back in the same field; `clip_view.show_marker` (`read`); and `clip.duplicate_loop`. (S7)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-6.55 | |
| R2 | to be written in WP-6.55 | |
| R3 | to be written in WP-6.55 | |
| R4 | to be written in WP-6.55 | |
| R5 | to be written in WP-6.55 | |
| R6 | to be written in WP-6.55 | |
| R7 | to be written in WP-6.55 | |
| R8 | to be written in WP-6.55 | |
| R9 | to be written in WP-6.55 | |
| R10 | to be written in WP-6.55 | |
| R11 | to be written in WP-6.55 | |
| R12 | to be written in WP-6.55 | |
