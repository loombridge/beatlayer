# Tracks and undo: the shape of a song and how edits are undone

This spec covers which kinds of track a song has, how they nest, which of them hold clips and which mixer switches each has, and how Live's undo history behaves. Beatlayer's implementation is `bl-model` (WP-0.6). The design it sits in is "The document" in [the architecture](../02-architecture.md#the-document).

## Sources

Every source is either public documentation, paraphrased rather than quoted, or an observation of Live running, as [the clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code) allows. The documentation was read on 10 October 2026.

| Ref | Source |
|---|---|
| M5.4.2 | Live 12 Reference Manual, chapter 5 "Managing Files and Sets", section 5.4.2 "Accessing a Set's Undo History" |
| M18.1 | Live 12 Reference Manual, chapter 18 "Mixing", section 18.1 "The Live Mixer" |
| M18.2 | Live 12 Reference Manual, section 18.2 "Audio and MIDI Tracks" |
| M18.3 | Live 12 Reference Manual, section 18.3 "Group Tracks" |
| M18.4 | Live 12 Reference Manual, section 18.4 "Return Tracks and the Main track" |
| O | Observed in Live 12.4.3 on macOS, 10 October 2026, between 20:28 and 20:38, in a new set with nothing signed in or saved, watching View > Undo History. The steps and readings are in "What Live does", items 9 to 13 |

The manual is at `ableton.com/en/live-manual/12/`.

## What Live does

1. **Track kinds.** Audio and MIDI tracks hold and play clips (M18.2). Group tracks sum the tracks inside them (M18.3). Return tracks and the Main track can't play clips (M18.4).
2. **Groups.** A group track holds audio and MIDI tracks, and a group can be put inside another group (M18.3). A group can't hold clips, but it has mixer controls and can host audio effects (M18.3).
3. **Deleting and ungrouping.** Deleting a group deletes everything inside it. Ungrouping turns its contents back into separate tracks (M18.3).
4. **Return tracks.** A song can have several return tracks, added with an Insert Return Track command. They sit after the other tracks: at the bottom of the Arrangement and on the right of the Session mixer (M18.4).
5. **The Main track.** A song has exactly one Main track (M18.4).
6. **Mixer switches.** A track's mixer has a volume, a pan, a Track Activator that mutes the track when off, a Solo switch, and an Arm Recording switch for tracks that can record (M18.1). Group tracks have a Solo switch (M18.3).
7. **Undo history.** Live keeps a list of every action since the set was opened, newest first. Choosing an entry returns the set to that point; the actions after it are shown as undone, and you can reapply them (M5.4.2).
8. **Where history starts and ends.** Opening or creating a set is the first entry in the history and can't be undone. The history isn't saved with the set; it starts again each time the set is opened (M5.4.2).
9. **Main and return track switches.** The Main track has no Track Activator and no Solo switch of its own; its mixer strip carries the global Solo/Cue mode button and the cue volume knob instead. Return tracks have a Track Activator, showing the return's letter, and a Solo switch, but no Arm switch (O).
10. **The history panel.** View > Undo History opens the history as a panel in the window's right-hand area, where the Learn View sits. The newest entry is at the top and each is named for its action: a new set's list read, from the top, two mixer changes, two clip inserts, and New Live Set at the bottom (O).
11. **One gesture, one step.** Dragging a track's volume field is one entry, and a pan drag is one entry: mixer changes are in the history. Each edit of the time signature's numerator or denominator field is one entry, and tempo breakpoint work shows as adding and moving breakpoints (O).
12. **No change, no step.** Clicking an empty lane, which moves the insert marker, selecting a clip, and making a time selection add no entry (O).
13. **Redo after a new edit.** After one undo, the undone entry stays in the list above the current one, greyed. Making a new edit replaces it, and the Edit menu then reads Nothing to Redo: a new edit discards the undone actions (O).

## What Beatlayer does

Unit tests are in `crates/model/src/`, and property tests in `crates/model/tests/`.

| Rule | From | Test |
|---|---|---|
| A track is one of five kinds: audio, MIDI, group, return, or Main. | 1 | `document::tests::new_document_has_only_main` |
| Only audio and MIDI tracks hold clips. An audio clip goes only on an audio track and a MIDI clip only on a MIDI track. | 1, 2 | `command::tests::clips_only_on_matching_tracks` |
| A group holds audio, MIDI, and group tracks, nested to any depth. A group can't go inside itself or inside a group within it. | 2 | `command::tests::groups_nest`, `command::tests::group_cannot_contain_itself` |
| Return tracks and the Main track are never inside a group. | 2, 4 | `command::tests::returns_stay_out_of_groups` |
| Deleting a group deletes every track inside it, with their clips and devices. | 3 | `command::tests::deleting_a_group_deletes_its_contents` |
| Return tracks form their own ordered list, which comes after the other tracks. | 4 | `document::tests::display_order` |
| There is exactly one Main track. It can't be created, deleted, or moved. | 5 | `command::tests::main_is_fixed` |
| Every kind of track has a device chain. | 2, 4 | `command::tests::devices_on_every_track` |
| Every track has a volume and a pan. Every track except Main has mute and solo. Only audio and MIDI tracks have arm. | 6, 9 | `command::tests::mixer_switches_by_kind` |
| Undo history is a list of steps. Undo returns the song to the state before the newest step, and redo reapplies it. Each step is one transaction: a UI gesture such as a drag is one step, however many commands it applies. Mixer changes are undo steps. | 7, 11 | `editor::tests::*`, `tests/undo_properties.rs` |
| A new or loaded document starts with an empty history, so creating or opening it can't be undone. The history isn't part of the document, so it isn't saved. | 8 | `editor::tests::history_starts_empty` |
| A transaction that leaves the song as it was records no undo step, so a click that doesn't change the song doesn't fill the history. | 12 | `editor::tests::empty_transactions_record_nothing` |
| A new edit after an undo discards the steps that could have been redone. | 13 | `editor::tests::a_new_step_discards_redo` |

The Undo History panel (Phase 6, layout row L-OTH-001 in the UI parity reference) takes its look from items 10 and 13: newest first, a fixed bottom entry for creating or opening the set, and undone steps kept greyed above the current one until a new edit replaces them.

## Beatlayer's own choices

These don't come from Live; they follow from [the architecture](../02-architecture.md#the-document).

| Choice | Test |
|---|---|
| Every track, clip, and device has a 64-bit ID that never changes and is never reused within the project, even after the object is deleted and the deletion undone. | `editor::tests::ids_are_never_reused`, `undo_all_then_redo_all` |
| Selection, scroll position, zoom, and other view state aren't in the document, so they're never undo steps. Live agrees for the insert marker, clip selection, and time selection (item 12). | (none: there is no such state in `bl-model`) |

The document itself lets clips on a track overlap. The Arrangement's edits (WP-1.8b) keep them from doing so with Live's rule: whatever is placed over part of a clip replaces that part ([arrangement-editing](arrangement-editing.md) R6).

## The Undo History panel

The panel that lists the history and jumps through it is [undo-history.md](undo-history.md) (WP-6.28). It adds one choice of Beatlayer's own to the history itself:

| Choice | Test |
|---|---|
| Each step records who made it: the person, an agent by the name the agent API knows it by, or the built-in assistant ([undo-history.md](undo-history.md) R9). | to be written in WP-6.28 |
