# The track header's context menu

- **Status:** implemented
- **Work packages:** WP-4.22
- **Parity-matrix row:** Menu bar and context menus: structure, order, and wording of generic commands
- **Checked against:** Live 12.4.3 on macOS, observed on 10 October 2026 (the menu as opened on a MIDI track's title bar), and the Live 12 reference manual, chapters 6, 8, 18, 20, 21, and 22

## The behaviour

Right-clicking a track's title bar opens a menu of what can be done to the track: the clipboard items, Duplicate and Delete, Rename and Edit Info Text, Freeze and Bounce, the three Insert items, Group Tracks, then Select Track Content, Save as Default MIDI Track, the colour palette, and Assign Track Color to Clips. The title bars are the Arrangement's track headers and the title bars over the Session View and over both mixers. The menu acts on the selected title bars, so with several tracks selected an item acts on all of them as its command does.

Items whose commands exist today run them. The rest are drawn in Live's place and disabled, each saying which package makes it work. The menu differs a little when the track is a group, when several tracks are selected, when it opens in the Arrangement, and when the Arrangement has a time selection on the track.

This spec covers the track title bar's menu, Delete and Rename on the selected title bars, and how the menu opens from the keyboard. The commands it runs are specified where they live: [tracks and undo](tracks-and-undo.md), [routing](routing.md) for Duplicate Track Without Clips, and [arrangement editing](arrangement-editing.md) for Cut, Copy and Duplicate.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Observation | Live 12.4.3 on macOS, 10 October 2026: the context menu opened on a MIDI track's title bar (menus.md, C-TRACK-001 to C-TRACK-016) | The items in order, their separators, and the keys Live shows beside them |
| S2 | Manual | Live 12 reference manual, sections 6.14.1, 8.1.1.3, 18.3, chapters 20 and 21, section 22.2 (menus.md, C-TRACK-017 to C-TRACK-020) | The items the menu gains for a group, several selected tracks, the Arrangement's take lanes, and a time selection |
| S3 | Observation | Live 12.4.3 on macOS, 10 October 2026 (shortcuts.toml, S-EDIT-001, S-EDIT-002, S-EDIT-004, S-TRACK-001 to S-TRACK-004, S-TRACK-006, S-TRACK-007, S-TRACK-017, S-TRACK-018, S-CTX-002) | The keys that run the items |

## Rules

- **R1.** A right-click on a track's title bar opens the track header's menu, in the Arrangement's headers (the pinned returns and Main too), over the Session View, and over the Arrangement's mixer. Its rows are C-TRACK-001 to C-TRACK-016 of menus.md in Live's order with Live's separators and labels, except that Duplicate Track Without Clips (IMP-12) sits straight after Duplicate. (S1)
- **R2.** The right-click first selects the title bar unless it's selected already; with several selected title bars, the one clicked keeps the selection, so the menu acts on all of them. (S1)
- **R3.** Cut, Copy and Duplicate run what their keys run in the Arrangement (`arrangement.cut`, `arrangement.copy`, `arrangement.duplicate`) and are enabled as those commands allow. Duplicate Track Without Clips runs `track.duplicate_without_clips`. Insert Audio Track, Insert MIDI Track and Insert Return Track run `track.create` as their keys do. Group Tracks runs `track.group`. Each item shows the key Live shows. (S1, S3)
- **R4.** Delete runs `track.delete` on every selected track except Main, as one undo step named Delete Track(s); a track deleted with its group isn't deleted twice. With no track selectable, or only Main selected, Delete is disabled. The Delete key does the same on selected title bars. (S1, S3)
- **R5.** Rename types the name in the first selected track's title bar, which starts from the track's name; Return runs `track.rename` with it, Esc leaves the name as it was, and while it's typed the keys go to it and not to shortcuts. Main can't be renamed, and Rename is disabled when only Main is selected. (S1, S3)
- **R6.** The items whose commands aren't built are drawn disabled in their places, and each names its package in its AccessKit description and its hover text, as "Not built yet: arrives with …": Edit Info Text (WP-2.24, the Info View, which leaves the user's own info text to the edit commands), Freeze Track (WP-4.14), Bounce Track in Place (WP-4.15), Select Track Content and Save as Default MIDI Track (no package plans them yet), and the colour palette (drawn as 5 rows of 14 dimmed swatches, one control for a screen reader) and Assign Track Color to Clips (they wait for tracks to have colours, which no package plans yet). (S1)
- **R7.** The menu gains rows for the other states, drawn beside the items they go with because menus.md doesn't record where Live puts them: for a group track, Ungroup Tracks (`track.ungroup`, enabled), Bounce Group to New Track (WP-4.15) and Assign Track Color to Grouped Tracks and Clips (disabled); with several tracks selected, Link Tracks and Unlink Track(s) (disabled, Phase 6); in the Arrangement's headers, Show Take Lanes, Insert Take Lane and Delete All Take Lanes (disabled, Phase 6); in the Arrangement with a time selection on the track, Crop Clip (disabled, WP-4.13). Deactivate All Devices (IMP-25) isn't in the menu until IMP-25's placement is decided (WP-5.29). (S2)
- **R8.** The menu's items run in the track-headers context whichever view it opened in, so Delete, Rename and the keys shown work from the Session View and the mixers as from the Arrangement's headers. (S3)
- **R9.** S-CTX-002 opens the menu for the selected title bar: in the Arrangement, with the view or the title bar focused and the title bars selected; over the Session View and the mixers, with the track's column focused. The menu's first item that can be chosen is focused, Up and Down move through the items that can be chosen, and Enter runs one. Every item reaches AccessKit with its label, its keys and its enabled state. (S3; accessibility R8)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/shell-spec/src/context_menus.rs`, `the_menus_follow_lives_rows_and_separators_in_order`; `crates/ui/tests/track_menu.rs`, `the_menu_lists_lives_items_and_names_later_packages`, `the_session_view_s_title_bars_open_the_same_menu`, `the_arrangement_mixer_s_title_bars_open_it_too` | unit, UI script |
| R2 | `crates/ui/tests/track_menu.rs`, `with_several_tracks_selected_each_item_acts_on_all_of_them` | UI script |
| R3 | `crates/ui/tests/track_menu.rs`, `working_items_show_their_keys_and_run_their_commands` | UI script |
| R4 | `crates/shell/tests/shell_commands.rs`, `delete_and_rename_from_a_key_or_menu_act_on_the_selected_tracks`; `crates/ui/tests/track_menu.rs`, `delete_and_rename_act_on_the_selected_track`, `the_delete_key_deletes_the_selected_title_bars_tracks` | headless, UI script |
| R5 | `crates/ui/tests/track_menu.rs`, `delete_and_rename_act_on_the_selected_track`; `crates/shell/tests/shell_commands.rs`, `delete_and_rename_from_a_key_or_menu_act_on_the_selected_tracks` | UI script, headless |
| R6 | `crates/shell-spec/src/context_menus.rs`, `the_device_and_track_menus_pending_items_name_their_package`; `crates/ui/tests/track_menu.rs`, `the_menu_lists_lives_items_and_names_later_packages` | unit, UI script |
| R7 | `crates/shell-spec/src/context_menus.rs`, `the_track_menus_other_states_add_the_items_menus_md_lists`; `crates/ui/tests/track_menu.rs`, `a_group_and_a_time_selection_add_their_items` | unit, UI script |
| R8 | `crates/shell/tests/shell_commands.rs`, `delete_and_rename_from_a_key_or_menu_act_on_the_selected_tracks`; `crates/ui/tests/track_menu.rs`, `the_session_view_s_title_bars_open_the_same_menu` | headless, UI script |
| R9 | `crates/ui/tests/track_menu.rs`, `the_keys_open_the_selected_title_bar_s_menu`, `the_keys_open_a_session_column_s_title_menu` | UI script |
