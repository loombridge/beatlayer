# The chord lane: a scale and a transposition for each section of the Arrangement

- **Status:** draft
- **Work packages:** WP-6.48 (R1 to R12)
- **Parity-matrix row:** none: Beatlayer's own; [the improvements](../05-improvements.md), IMP-33 (a chord and transpose lane)
- **Checked against:** [scales.md](scales.md) and the improvements' rules, read 20 October 2026; the lane is Beatlayer's own, so every rule here is too

## The behaviour

The chord lane is an optional lane of Beatlayer's own in the Arrangement, hidden until a person shows it. It holds regions, each over a stretch of the song, and each region names a scale and a transposition: "D Dorian, +2", say, for a bridge. A MIDI track switched to follow the lane plays its Arrangement clips' notes moved by the region's transposition while the song plays through the region, counted in degrees of each clip's scale where the clip has one, and its scale-aware devices take the region's scale as their current scale. Nothing in the clips changes: hiding the lane, deleting a region or switching a track off plays the notes as written. A new track doesn't follow the lane, so a set that never uses it plays as it would in Live.

This spec covers the lane, its regions, the tracks' switch and what playback does with them. The scales and their arithmetic are [scales.md](scales.md)'s.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The improvements](../05-improvements.md), IMP-33 (a chord and transpose lane) and rules 1 to 4 (add, don't move; a changed default is an option; new things where Live users would look; agents reach it too) | The improvement and the limits on it |
| S2 | Beatlayer | [scales.md](scales.md) R1, R2, R15 and R17 | The scale list, moving a key by degrees, a device's current scale, held notes |
| S3 | Beatlayer | [The architecture](../02-architecture.md), "The document", "Transport and clip launching", "Real-time rules" and decision D17 | Edits are commands and undo steps; playback lands on exact frames; nothing on the audio thread allocates, locks or blocks; every action is a registered command an agent can run and read back |

## Rules

- **R1.** The chord lane is a lane of the Arrangement, hidden in a new set, shown and hidden by an item of its own in the View menu, `Chord Lane`, ticked while it shows (`view.chord_lane`, `read`). Whether it shows is saved with the set and isn't an undo step. (S1)
- **R2.** The lane holds regions that don't overlap. Each has a start and an end in the song, a scale (a root and a scale name from the list, or none) and a transposition, a whole number from −24 to +24 (0 by default), and it reads as its scale and its transposition, such as "D Dorian +2". A stretch with no region sets nothing. (S1, S2)
- **R3.** Drawing in the lane with the pointer makes a region over the dragged stretch, snapped to the Arrangement's grid; dragging a region moves it, dragging an edge resizes it, and a region can't be made to overlap another. Selecting a region and pressing Delete deletes it. Each is one undo step (`chord_lane.add_region`, `chord_lane.move_region`, `chord_lane.delete_region`). (S1, S3)
- **R4.** A region's scale and transposition are set from its context menu and by `chord_lane.set_region`, one undo step. (S1, S3)
- **R5.** Each MIDI track has a Follow Chord Lane switch, off in a new track, which shows in the track's context menu and is set by `track.set_follow_chord_lane`, one undo step, saved with the set. (S1)
- **R6.** While the song plays through a region, each note of a following track's Arrangement clips moves by the region's transposition as it leaves the clip, before the track's devices: by that many degrees of the clip's scale where the clip's Scale Mode is on ([scales.md](scales.md) R2), by that many semitones where it is off. A note moved off the keyboard doesn't play. The clip's notes stay as they are in the set. (S1, S2)
- **R7.** A note keeps the transposition it started with: its note-off and its expressions go to the key its note-on went to, whatever region it ends in ([scales.md](scales.md) R17). (S2)
- **R8.** While the song plays through a region that names a scale, that scale is the current scale of the scale-aware devices on each following track ([scales.md](scales.md) R15), in place of its playing clip's; where no region names one, R15 of the scales spec holds as it is. (S2)
- **R9.** Session clips, audio clips and monitored input aren't moved and don't take a region's scale: the lane acts only on the Arrangement clips of MIDI tracks. (S1)
- **R10.** The set saves the lane's regions and each track's switch; a set saved before the lane opens with no regions and no track following it. (S1)
- **R11.** A region's start and end are on the song's beats, so a tempo change keeps them in place on the bars; the moves land on the notes' own frames, the same at any block size, with nothing allocated, locked or blocked on the audio thread, and an exported song moves its notes as playback does. (S3)
- **R12.** `view.chord_lane`, `chord_lane.add_region`, `chord_lane.move_region`, `chord_lane.set_region`, `chord_lane.delete_region` (`destructive`) and `track.set_follow_chord_lane` are registered commands with schemas, descriptions and permission classes, the others `edit`; the query tool's `song` reads the lane's regions in its `chord_lane` field and its `track` reads `follow_chord_lane`, their readbacks. (S3)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-6.48 | |
| R2 | to be written in WP-6.48 | |
| R3 | to be written in WP-6.48 | |
| R4 | to be written in WP-6.48 | |
| R5 | to be written in WP-6.48 | |
| R6 | to be written in WP-6.48 | |
| R7 | to be written in WP-6.48 | |
| R8 | to be written in WP-6.48 | |
| R9 | to be written in WP-6.48 | |
| R10 | to be written in WP-6.48 | |
| R11 | to be written in WP-6.48 | |
| R12 | to be written in WP-6.48 | |
