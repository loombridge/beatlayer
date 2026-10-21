# Linked-track editing: linking tracks so one edit reaches them all

- **Status:** draft
- **Work packages:** WP-6.23 (R1 to R16). It builds on the Arrangement's edits ([arrangement editing](arrangement-editing.md), WP-1.8b), Consolidate ([audio clip editing](audio-clip-editing.md), WP-4.13), the Sample Editor's edits to several clips ([sample editor](sample-editor.md) R19, WP-4.10), and take lanes and comping ([comping](comping.md), WP-6.21 and WP-6.22); fade handles and crossfades are WP-6.51's ([audio clips](audio-clips.md) R23 to R31)
- **Parity-matrix row:** Recording: Linked-track editing
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026 (chapter 6, section 6.14 "Linked-Track Editing" with 6.14.1 and 6.14.2; chapter 9, section 9.2.4.4; chapter 21), and the UI parity reference's row C-TRACK-018

## The behaviour

Linking tracks makes the Arrangement edit them as one, for the takes of a performance recorded on several tracks at once, such as a drum kit's microphones. Select two or more tracks and choose Link Tracks in a track header's menu: the tracks show a linked-track indicator in their headers, and from then on selecting clips or time on one selects the same on the others, so moving, resizing, splitting and consolidating clips, creating and editing their fades, arming the track, and inserting, renaming, deleting and auditioning take lanes reach every linked track at once, and a comp made on one is made on all. A set can hold several separate groups of linked tracks; a track is in one at most. Unlink Track(s) takes tracks out of their group.

This spec covers linking and unlinking, and which edits reach the linked tracks. The edits themselves are their own specs'. Linked-track editing ties tracks together; it isn't IMP-21's linked clips, where copies of a clip share their contents. Undo step names in code type are Beatlayer's own.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 6 "Arrangement View", section 6.14 "Linked-Track Editing" and 6.14.1 "Linking and Unlinking Tracks" | Linked-track editing brings the comping workflow and other phase-locked edits to several tracks at once. Link Tracks in a track header's menu links the selected tracks, and in a group track's header menu links the tracks inside it. Linked tracks show a linked-track indicator button in their headers. A set can hold several separate instances of linked tracks, a track belonging to one at most. Hovering over an indicator highlights the tracks of its instance; clicking it selects them. Holding Cmd (Ctrl) while choosing Link Tracks in the menu of a track already linked adds the selected tracks to its instance. Any subset of linked tracks, or a mix of linked and unlinked ones, can be linked anew by selecting them and choosing Link Tracks. Unlink Track(s), in the track's or the group's header menu, takes the selected tracks out of their instance |
| S2 | Manual | Same chapter, section 6.14.2 "Editing Linked Tracks" | Once tracks are linked, these act on all of them at once: moving and resizing clips; selecting clips and time; the …Time commands; splitting and consolidating clips; creating and editing audio clip fades, only fades that start at the same position being adjusted together; arming and disarming; renaming, inserting and deleting take lanes, and switching a take lane's Audition Mode, even where some of the tracks hide their take lanes |
| S3 | Manual | Chapter 9, section 9.2.4.4, through [warping](warping.md) S5 and [sample editor](sample-editor.md) R19 | With several clips of the same length selected, a warp-marker edit to one is made to all of them |
| S4 | Reference | [menus.md](../ui-parity/menus.md) C-TRACK-018 | Link Tracks and Unlink Track(s) are in the track header's menu with several tracks selected |
| S5 | Spec | [arrangement editing](arrangement-editing.md) R1, R2, R4, R7 to R11, R13, R14; [audio clip editing](audio-clip-editing.md) R19, R25; [audio clips](audio-clips.md) R6; [comping](comping.md) R4, R6, R7, R14, R17, R19, R21; [track context menu](track-context-menu.md) R2, R11; [recording](recording.md) R1 | The Arrangement's selection and edits, Consolidate, fades, take lanes and comps, the menu that acts on the selected title bars, and arming |
| S6 | Beatlayer | [The architecture](../02-architecture.md), "The document", "Agent control", decision D17 and the readback rule; [the improvements](../05-improvements.md), IMP-21 | Objects have stable IDs; every action is a registered command with a schema, a description, a permission class and a readback, run the same way by a person and an agent; linked clips are a different, proposed feature |

## Rules

**Linking (WP-6.23)**

- **R1.** A set holds any number of link groups, each of two or more audio or MIDI tracks; a track is in one at most. Link groups are saved with the set, and a set saved before them opens with none. Audio and MIDI tracks may share a group, Beatlayer's own; group, return and Main tracks are never in one. (S1)
- **R2.** Link Tracks (`track.link`; the track header's menu with two or more tracks selected, C-TRACK-018) puts the selected audio and MIDI tracks into a new link group of their own, taking each out of any group it was in. A group left with one track ends. It's one undo step, `Link Tracks`, and disabled with fewer than two audio or MIDI tracks selected. (S1, S4)
- **R3.** Holding Cmd (Ctrl) while choosing Link Tracks in the menu of a track already in a link group adds the other selected audio and MIDI tracks to that group instead (`track.link` with `into` naming the group's track), taking them out of any group they were in. (S1)
- **R4.** Link Tracks in a group track's header menu links the audio and MIDI tracks inside the group, at any depth, into one link group, and Unlink Track(s) there takes them out of theirs. Beatlayer's own: tracks in nested groups too. (S1)
- **R5.** Unlink Track(s) (`track.unlink`; the menu of a linked track's header) takes the selected tracks out of their link groups, one undo step, `Unlink Tracks`; a group left with one track ends. A deleted track leaves its group the same way, and undoing the deletion puts it back; a duplicated track isn't linked. Beatlayer's own: the duplicate. (S1)
- **R6.** A linked track's header shows the linked-track indicator (L-ARR-022). Hovering over it highlights every track of its group; clicking it (I-CLICK-041) selects every track of its group's title bars, not an undo step. Assistive technology reads it as a button naming the group's tracks. (S1)

**Which edits reach linked tracks (WP-6.23)**

- **R7.** Selecting on a linked track selects on every track of its group: selecting clips also selects, on each of the group's other tracks, the clips that start and end at the same places; selecting time also selects the same time on each of them, on the same lane: the main lane, or the take lane at the same place among the track's take lanes where it has one. Beatlayer's own: which clips match. (S2; arrangement editing R1, R2; comping R26)
- **R8.** These edits, made on a linked track, are made on every track of its group in the same undo step: moving and resizing clips, to the clips R7 matches, by the same amount (arrangement editing R4, R7); splitting at the same places and consolidating the same range (arrangement editing R8; audio clip editing R19); creating and editing an audio clip's fades and crossfades, on the clips whose fade starts at the same place, so a fade-in matches a fade-in and a fade-out a fade-out ([audio clips](audio-clips.md) R6, R23, R24, R27 and R28; `clip.set_fades`); and arming and disarming the track, Arm Exclusive arming the group and disarming the rest. Beatlayer's own: Arm Exclusive. (S2)
- **R9.** An edit that acts on the selection acts on all of it, as R7 extends it: Delete, Duplicate, Cut, Copy and Paste on clips or time selected on a linked track act on the group's tracks as well (arrangement editing R9 to R11). (S2)
- **R10.** With the matching clips of linked tracks selected (R7), a warp-marker edit in the Sample Editor is made to each selected clip whose file is as long as the shown clip's, as the Sample Editor makes it to several selected clips (sample editor R19), and clips of another length are left as they were. (S2, S3)
- **R11.** Inserting, renaming and deleting take lanes and switching a take lane's audition, on a linked track, do the same on every track of its group, whether its take lanes are shown or hidden: Insert Take Lane adds a take lane to each (comping R4); Rename, Delete and audition act on each track's take lane at the same place among its take lanes, where it has one (comping R6, R7, R14). Duplicating and moving take lanes, and Delete All Take Lanes and Delete All Unused Take Lanes, act on the tracks named only. (S2)
- **R12.** The comp commands, on a linked track, act on the same range and lane of every track of its group, each track copying its own lane's material: Copy Selection to Main Lane, replacing with the next or previous take, and moving a comp boundary at the same place (comping R17, R19, R21). So one comp, made with the keys or the mouse, edits the group's tracks together. (S1, S2)
- **R13.** The …Time commands act on every track of the Arrangement and on its take lanes, linked or not ([time editing](time-editing.md) R14). (S2)
- **R14.** Every other edit acts on what it names only: a clip's own settings in the Clip View other than its warp markers (R10), its notes and envelopes, the mixer's controls other than arm, devices, and the track itself (renaming, colour, deleting). (S2)

**Commands and agents (WP-6.23)**

- **R15.** Linking acts in the command layer, so it's the same however an edit is made: a command whose edit R8, R11 or R12 names reaches the group's other tracks whether a person or an agent runs it, by the selection or with its objects named, unless an agent's call gives `"linked": false`, which limits it to what it names; the result lists every object it changed. `arrangement.select_clips` and `arrangement.select_time` extend the selection as R7 says. Beatlayer's own: the `linked` argument. (S6)
- **R16.** `track.link` and `track.unlink` are registered commands in the `edit` class with a schema and a description for a model, each one undo step, reading back through the query tool's `track`, field `linked`, which lists the tracks of its link group, empty for an unlinked track. Linking and unlinking survive save and load, and undo and redo restore them exactly. (S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-6.23 | unit, migration |
| R2 | to be written in WP-6.23 | unit, UI script |
| R3 | to be written in WP-6.23 | UI script |
| R4 | to be written in WP-6.23 | unit, UI script |
| R5 | to be written in WP-6.23 | unit |
| R6 | to be written in WP-6.23 | UI script |
| R7 | to be written in WP-6.23 | shell test |
| R8 | to be written in WP-6.23 | shell test |
| R9 | to be written in WP-6.23 | shell test |
| R10 | to be written in WP-6.23 | shell test |
| R11 | to be written in WP-6.23 | shell test |
| R12 | to be written in WP-6.23 | shell test, golden render |
| R13 | to be written in WP-6.23 | shell test |
| R14 | to be written in WP-6.23 | shell test |
| R15 | to be written in WP-6.23 | agent API |
| R16 | to be written in WP-6.23 | unit, agent API, `cargo xtask commands` |
