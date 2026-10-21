# Linked clips

- **Status:** draft
- **Work packages:** WP-6.44 (R1 to R16)
- **Parity-matrix row:** none (Beatlayer's own); [the improvements](../05-improvements.md), IMP-21 (linked clips)
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026, chapter 6 "Arrangement View" (sections 6.7, 6.12, 6.13 and 6.14) and chapter 7 "Session View"; the Live 12 release notes, 12.0 to 12.4.6; the UI parity reference's [menus.md](../ui-parity/menus.md) M-EDIT-006, C-CLIP-SES and C-CLIP-ARR

## The behaviour

A person writes a drum pattern, then wants it in twenty places and wants to change a note in all twenty later. Duplicate as Linked Clip, next to Duplicate, makes a copy that shares its contents with the original: its notes, or its audio file with its warping and gain. Edit the notes of any copy and every copy changes. Each copy keeps its own place, length, markers, loop and name, so one can play the first two bars and another all four. A small link mark on each copy says it's linked; Unlink Clip makes a copy independent again with the contents it has.

Live has no linked clips. Its nearest feature is linked-track editing (manual 6.14), which links tracks so that moving, resizing, splitting and selecting clips on one does the same on the others; it doesn't share a clip's contents. Beatlayer builds linked-track editing as Live has it (WP-6.23), and linked clips beside it. Plain Copy, Paste and Duplicate keep making independent copies, as in Live.

This spec covers which settings a link shares, which edits reach every linked clip, what unlinks a clip, the mark, saving, and the commands. What a clip holds is [midi-clips.md](midi-clips.md)'s, [audio-clips.md](audio-clips.md)'s and [warping.md](warping.md)'s; where Duplicate puts a copy is [arrangement-editing.md](arrangement-editing.md)'s.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The improvements](../05-improvements.md), IMP-21, and rules 1 to 4 | "Duplicate as Linked Clip" next to Duplicate: every copy shares its notes or audio edits, editing one edits all, each copy keeps its own position and length; a mark shows it's linked; "Unlink Clip" makes it independent |
| S2 | Manual | Live 12 reference manual, chapter 6 "Arrangement View", section 6.14 "Linked-Track Editing" (6.14.1 and 6.14.2) | Tracks are linked from the track header's menu and show a linked-track button; moving, resizing, selecting, the …Time commands, splitting, consolidating, fades, arming and take-lane edits apply to every linked track; the section says nothing of sharing a clip's contents |
| S3 | Manual | Same chapter, sections 6.7 "Moving and Resizing Clips", 6.12 "Splitting Clips" and 6.13 "Consolidating Clips" | A clip's place, length and region are set in the Arrangement; splitting makes two clips that play what the one played; consolidating makes a new clip of a selection |
| S4 | Reference | [menus.md](../ui-parity/menus.md) M-EDIT-006 (Duplicate, ⌘D, with Duplicate Track Without Clips after it, IMP-12), C-CLIP-SES-003 and C-CLIP-ARR-003 (Duplicate in the clip context menus) | Where Duplicate sits in the Edit menu and the clips' context menus |
| S5 | Release notes | Live 12 release notes, 12.0 to 12.4.6, read 20 October 2026 | No release adds clips that share contents |
| S6 | Beatlayer | [arrangement-editing.md](arrangement-editing.md) R10 and R14; [session-view.md](session-view.md) R7, R10 and R11; [midi-clips.md](midi-clips.md); [audio-clips.md](audio-clips.md); [warping.md](warping.md); [clip-envelopes.md](clip-envelopes.md); [midi-controller-data.md](midi-controller-data.md) | Where Duplicate places a copy; what a slot clip is; what a MIDI clip and an audio clip hold, their warp markers, a clip's envelopes and its MIDI controller envelopes |
| S7 | Beatlayer | [The architecture](../02-architecture.md), "The document" and "Agent control"; decisions D17 | The document is an immutable tree whose unchanged nodes are shared between versions; every action is a registered command with a permission class and a readback; every edit is one undo step |

## Rules

- **R1.** Duplicate as Linked Clip sits in the Edit menu straight after Duplicate Track Without Clips, and in both clips' context menus (C-CLIP-SES, C-CLIP-ARR) straight after Duplicate, enabled while clips are selected; it has no key. It runs `clip.duplicate_linked`, which places a copy of each selected clip where Duplicate places one (arrangement-editing R10 in the Arrangement; in the Session View, in the first empty slot below it on its track, a scene being added when there's none), selects the copies, and links each copy to its clip, as one undo step named `Duplicate as Linked Clip`. The places in the menus, the Session View's placement and the undo name are Beatlayer's own. (S1, S4, S6)
- **R2.** A link is two or more clips of one kind, MIDI or audio, that share their contents (R3). A clip belongs to at most one link. Duplicating a linked clip as linked adds the copy to the same link. A link's clips can sit on any tracks of their kind, in the Arrangement's lanes and in Session slots alike. (S1)
- **R3.** A link shares its clips' contents. A MIDI clip's contents are its notes, with every value each note holds (key, start, length, velocity, release velocity, probability, velocity deviation, mute, and its per-note expression), and its MIDI controller envelopes ([midi-controller-data.md](midi-controller-data.md)). An audio clip's are its file, its Warp switch, its warp markers and tempo, its warp mode and that mode's settings, its transient edits, its gain, and its HiQ and RAM switches. Everything else is each clip's own: where it is, its length, its offset into its contents, its name, colour and mute, its Start and End markers, its loop switch and brace, its time signature, its fades, its launch settings, its Lead/Follow switch, and its envelopes on its track's parameters. (S1, S6)
- **R4.** An edit that changes a linked clip's contents (R3) changes every clip in its link the same way, as one undo step named as the edit is, whichever of the clips the edit names and whatever view it's made in: drawing, moving or deleting notes, a velocity change, quantising, a warp marker moved, Reverse. An edit to a clip's own settings changes that clip only. (S1, S7)
- **R5.** Crop Clip and Consolidate on a linked clip take it out of its link first, in the same undo step, so the other clips keep their contents. (S1, S3)
- **R6.** Splitting a linked clip gives two clips that are both in its link, each playing the part of the shared contents the one played before it was split. (S1, S3)
- **R7.** Copy and Paste, Duplicate, Option-drag (Ctrl-drag) copies, `scene.duplicate`, and saving a clip to the browser make independent clips: the copy holds the contents as they are and belongs to no link. (S1, S6)
- **R8.** Unlink Clip, straight after Duplicate as Linked Clip in both clips' context menus and enabled only while a linked clip is selected, runs `clip.unlink`: the selected clips leave their links, keeping the contents they have, as one undo step named `Unlink Clip`. A link left with one clip ends, and that clip is independent. (S1)
- **R9.** Moving a linked clip (along the timeline, to another track of its kind, between the Arrangement and a slot), resizing it, and moving its markers or loop brace keep it in its link. Deleting one takes it out of its link; a link left with one clip ends. (S1, S6)
- **R10.** A linked clip shows a link mark, a glyph of Beatlayer's own, at the right end of its title bar in the Arrangement and at the right of its slot in the Session View, in the clip's text colour, and beside its name in the Clip View. While the pointer is over the mark the Status Bar says how many other clips the clip is linked to. Clicking the mark selects every clip of its link in that view. (S1)
- **R11.** Assistive technology reads a linked clip's name with "linked" and the number of clips in its link, and the mark as a button that selects them. (S7)
- **R12.** The engine plays each linked clip as it would play an independent clip with the same contents and settings: a render of a set with linked clips is the same to the bit as a render of the set with every link undone. (S1, S7)
- **R13.** The set saves each link's shared contents once and each linked clip names its link, with its own settings beside it; loading gives back the links and every clip as it was. A set saved before linked clips loads unchanged, with no links. A clip that names a link the set doesn't hold, or a link whose clips aren't all of one kind, doesn't load. (S7)
- **R14.** `clip.duplicate_linked` takes the clips to copy and, for each, its copy's ID and, optionally, where the copy goes (a track and a start, or a track and a scene); left out, R1's place. `clip.unlink` takes the clips. Both are `edit` and read back through the query tool's `clip` object, whose `link` field gives the clip's link and the IDs of the other clips in it (`null` for an independent clip). (S7)
- **R15.** Through the agent API, an edit to a linked clip's contents reaches every clip in its link as R4 says, and its result names every clip it changed. A command whose class depends on what it removes (a `clip.set_data` that leaves fewer notes) counts what it removes from every clip in the link. (S7)
- **R16.** Linked clips and linked tracks are separate. A clip on a linked track (S2, WP-6.23) can belong to a link of clips; moving it moves the clips on the linked tracks as linked-track editing says, and changing its contents changes its link's clips as R4 says. (S2, S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-6.44 | |
| R2 | to be written in WP-6.44 | |
| R3 | to be written in WP-6.44 | |
| R4 | to be written in WP-6.44 | |
| R5 | to be written in WP-6.44 | |
| R6 | to be written in WP-6.44 | |
| R7 | to be written in WP-6.44 | |
| R8 | to be written in WP-6.44 | |
| R9 | to be written in WP-6.44 | |
| R10 | to be written in WP-6.44 | |
| R11 | to be written in WP-6.44 | |
| R12 | to be written in WP-6.44 | |
| R13 | to be written in WP-6.44 | |
| R14 | to be written in WP-6.44 | |
| R15 | to be written in WP-6.44 | |
| R16 | to be written in WP-6.44 | |
