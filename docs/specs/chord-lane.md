# The chord lane: a scale and a transposition for each section of the Arrangement

- **Status:** built (R1 to R12 each have a passing test)
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
| R1 | `crates/ui/tests/chord_lane.rs`, `the_lane_is_hidden_in_a_new_set_and_shows_from_the_view_menu_above_the_first_track`; `crates/shell/tests/chord_lane.rs`, `the_lane_is_hidden_in_a_new_set_and_view_chord_lane_shows_and_hides_it`, `a_saved_set_keeps_the_lane_its_regions_and_each_track_s_switch`; `crates/cli/tests/older_sets.rs`, `a_set_saved_before_the_chord_lane_loads_with_no_regions_and_no_track_following` | unit |
| R2 | `crates/model/tests/chord_lane.rs`, `regions_never_overlap_and_a_refused_edit_changes_nothing`, `a_region_that_breaks_a_rule_is_refused`; `crates/ui/tests/chord_lane.rs`, `drawing_in_the_lane_makes_a_region_on_the_grid_in_one_undo_step` | unit |
| R3 | `crates/ui/tests/chord_lane.rs`, `drawing_in_the_lane_makes_a_region_on_the_grid_in_one_undo_step`, `a_region_is_moved_and_resized_with_the_pointer_one_undo_step_each`, `a_region_cannot_be_dragged_over_another`, `clicking_selects_a_region_and_delete_deletes_it`; `crates/shell/tests/chord_lane.rs`, `drawing_is_one_undo_step_however_far_the_drag_goes_and_selects_the_region`, `moving_and_resizing_are_one_undo_step_each`, `selecting_a_region_is_no_undo_step_and_a_missing_region_is_refused`; `crates/model/tests/chord_lane.rs`, `a_region_is_added_moved_set_and_deleted_one_undo_step_each` | unit |
| R4 | `crates/ui/tests/chord_lane.rs`, `the_region_s_menu_sets_its_scale_and_its_transposition`; `crates/model/tests/chord_lane.rs`, `a_region_is_added_moved_set_and_deleted_one_undo_step_each` | unit |
| R5 | `crates/model/tests/chord_lane.rs`, `only_a_midi_track_follows_the_lane`; `crates/ui/tests/chord_lane.rs`, `a_midi_track_s_menu_has_follow_chord_lane_ticked_while_it_follows`; `crates/shell/tests/chord_lane.rs`, `a_duplicated_track_follows_the_lane_as_its_source_did`; `crates/shell-spec/src/context_menus.rs`, `a_midi_tracks_menu_adds_follow_chord_lane_after_save_as_default_midi_track` | unit |
| R6 | `crates/engine/tests/chord_lane.rs`, `a_following_track_s_notes_move_by_the_region_they_start_in`, `a_note_moved_off_the_keyboard_does_not_play`, `a_lane_edit_sends_data_and_no_program` | unit |
| R7 | `crates/engine/tests/chord_lane.rs`, `a_note_held_across_a_region_s_edge_ends_on_its_own_key`, `a_moved_note_s_expressions_go_to_the_key_its_note_on_went_to` | unit |
| R8 | `crates/engine/tests/chord_lane.rs`, `the_region_s_scale_is_the_following_track_s_current_scale` | unit |
| R9 | `crates/engine/tests/chord_lane.rs`, `a_track_that_does_not_follow_and_a_session_clip_play_as_written` | unit |
| R10 | `crates/model/tests/chord_lane.rs`, `the_lane_and_the_switch_are_saved_and_a_set_from_before_loads_with_neither`, `a_saved_lane_with_overlapping_regions_or_a_shared_id_does_not_load`; `crates/cli/tests/older_sets.rs`, `a_set_saved_before_the_chord_lane_loads_with_no_regions_and_no_track_following` | unit |
| R11 | `crates/engine/tests/chord_lane.rs`, `a_following_track_s_notes_move_by_the_region_they_start_in` (block sizes 1, 64 and 512, under the allocation trap), `an_export_moves_its_notes_as_playback_does` | unit |
| R12 | `crates/model/tests/command_registry.rs`, `every_command_runs_by_id`, `every_document_command_is_registered`; `crates/agent/tests/chord_lane.rs`, `every_chord_lane_command_runs_by_id_and_what_it_sets_reads_back`, `the_chord_lane_commands_have_the_classes_the_spec_gives`; `crates/agent/tests/readback.rs`, `every_query_readback_the_command_table_names_is_in_a_real_full_result` | unit |
