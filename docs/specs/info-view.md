# Info View

- **Status:** partial (R1 to R8 implemented; R9 to R12 come with WP-6.58)
- **Work packages:** WP-2.24 (R1 to R6), WP-6.27 (R7, the Info View beside stacked and hidden detail views), WP-6.29 (R8, the Info View with the second window open), WP-6.58 (R9 to R12, the user's own info text and Edit Info Text)
- **Parity-matrix row:** Info View (M2, "Built in WP-2.24")
- **Checked against:** Live 12.4.3, on 10 October 2026; for R9 to R12, the Live 12 reference manual (online edition), read 20 October 2026, and the UI parity reference's menu rows (M-EDIT-025, C-TRACK-006, C-SCENE-006, C-CLIP-SES-006, C-CLIP-ARR-008, C-DEVICE-005, C-TIME-025)

## The behaviour

The Info View is the panel at the bottom left of the detail area, at the same height as the detail area and 231 points wide at the reference window. It reads whatever the pointer is over: the control's name in Live's words, then the help text of the command the control runs, and the control's key on a separate line where the default keymap has one. While the pointer is over empty space it shows nothing, and it follows the pointer, not the keyboard focus. It shows and hides from the Info toggle at the left of the Status Bar, from View ▸ Info in the View menu, and from its key; while it hides, the Device View takes its place at the detail area's left. A new, empty set opens with it shown.

The help text is written from each command's and each parameter's registered description (D17), so a piece of help exists once — registered for agents — and the Info View, the menus, and the agent API all read it. A registered command or a device parameter without a description fails a check.

This spec covers the Info View's place, its show and hide, and what it shows while the pointer is over a control. The user's own info text (L-INFO-003), which a person writes with Edit Info Text into the Info View and which the set saves, is R9 to R12.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Observation | Live 12.4.3 on macOS, 10 October 2026: the panel's place and size from the accessibility tree and measuring captures; the contents' order from watching the pointer move; a new set's default state | The Info View at the detail area's left (L-INFO-001); the name, description, and key on a separate line (L-INFO-002); the pointer-driven updates (L-INFO-002, I-FOCUS-012); a new set opens with it shown (layout.md, "Default state") |
| S2 | Manual | Live 12 reference manual, section 2.2.2 | The Info View's place, its show/hide controls, the Status Bar's Info toggle, and the user's own info text (L-INFO-001 to 003, L-STAT-001, I-FOCUS-012) |
| S3 | Manual | Live 12 reference manual, section 42.1; observed in Live 12.4.3 on macOS, 10 October 2026, from the View menu | The shortcut is Shift+Slash in the global context, and View ▸ Info is ticked while the view shows (S-VIEW-008, M-VIEW-004) |
| S4 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in layout.md L-INFO-001 and L-WIN2-002; [detail-views.md](detail-views.md) R7 to R11; [second-window.md](second-window.md) R10 | The Info View is as tall as the detail area; the second window's contents are a Control Bar, a main view, a detail area with the Clip or Device View, and a Status Bar, with no Info View listed; the detail area's views and heights; the window with the focus |
| S5 | Manual | Live 12 reference manual (online edition, read 20 October 2026), section 2.2.2 | Tracks, clips, devices and more take info text of the user's own: Edit Info Text in an item's context menu, then typing into the Info View; the set saves it |
| S6 | Beatlayer | [menus.md](../ui-parity/menus.md) M-EDIT-025 (Edit ▸ Edit Info Text, which writes the Info View text for the selection), C-TRACK-006, C-SCENE-006, C-CLIP-SES-006, C-CLIP-ARR-008, C-DEVICE-005 (Edit Info Text in the track, scene, Session clip, Arrangement clip and device menus), C-TIME-025 (a locator's); [layout.md](../ui-parity/layout.md) L-INFO-003; [racks.md](racks.md) R26, [macros.md](macros.md) R14, [time-editing.md](time-editing.md) R8 (chains', macros' and locators' info text) | Where Edit Info Text is reached, and the items already holding info text |

## Rules

- **R1.** The Info View sits at the bottom left of the detail area: the detail area's left edge to 240 points at 1600 × 1000 (231 points wide), the detail area's whole height. (S1, S2)
- **R2.** The Info View shows and hides from the Info toggle at the left of the Status Bar, from View ▸ Info in the View menu, and from Shift+Slash, which applies in the global context; the menu item is ticked while it shows. While it hides, the Device View takes its place at the detail area's left, running to the detail area's whole width. (S1, S2, S3)
- **R3.** A new, empty set opens with the Info View shown. (S1)
- **R4.** While the pointer is over a control, the Info View shows the control's name, in Live's words, then the registered description of the command the control runs, and the command's key on a separate line where the default keymap has one. While the pointer is over empty space, it shows nothing. (S1, S2)
- **R5.** The Info View follows the pointer, not the keyboard focus: moving the pointer changes it; moving the keyboard focus does not. (S1, S2)
- **R6.** The text the Info View shows is the registered description of the command or parameter, not a second copy written for the UI: a registered command, or a device parameter of a device type Beatlayer registers, without a description fails a check. (architecture, D17; build plan, WP-2.24)
- **R7.** The Info View runs the detail area's whole height whatever the detail area holds: one view, the Clip View stacked over the Device View, or the expanded Clip View ([detail-views.md](detail-views.md) R7, R8, R11). With neither detail view showing, it keeps the bottom left at the Device View's height ([detail-views.md](detail-views.md) R9; Beatlayer's own). (S1, S4)
- **R8.** The Info View belongs to the main window: the second window's detail area has none (L-WIN2-002), and the main window's Info View describes what the pointer is over in either window, as R4 and R5 say (Beatlayer's own for the second window). (S4)
- **R9.** Tracks, clips (in the Arrangement and in Session slots), scenes, devices, rack chains, macros and locators each carry info text of the user's own, which the set saves; a set saved before it loads with none, and an empty text is none. Scenes, chains, macros and locators among them rest on their menus and specs (S6). (S5, S6)
- **R10.** Edit Info Text, in the Edit menu (M-EDIT-025) for the selected item and in the context menus that hold it (C-TRACK-006, C-SCENE-006, C-CLIP-SES-006, C-CLIP-ARR-008, C-DEVICE-005, a chain's, a macro's and a locator's, C-TIME-025), turns the Info View into a text field holding that item's info text, with the keyboard focus, showing the Info View first if it's hidden. Typing changes the text; Return keeps it, Option-Return (Alt-Return) starts a new line, and Esc drops the change; the focus leaving keeps it. Keeping a change is one undo step, `Change Info Text`, as a chain's is. The keys, and showing a hidden Info View, are Beatlayer's own. The Edit menu's item is disabled while nothing that carries info text is selected. (S2, S5, S6)
- **R11.** While the pointer is over an item that has info text, the Info View shows the item's name and its info text in place of the registered description, and the key line as R4 says (Beatlayer's own); with no info text, it shows what R4 says. The keyboard focus doesn't change what it shows (R5). Assistive technology reads an item's info text as its description. (S2, S5)
- **R12.** The commands are registered with schemas, descriptions and permission classes, `edit`, run headless with the item named: `track.set_info_text`, `clip.set_info_text`, `scene.set_info_text` and `device.set_info_text`, beside `chain.set_info`, `rack.set_macro_info_text` and `locator.set_info_text`; the query tool's track, clip, scene and device read it back (`info_text`), and the macros', chains' and locators' readbacks as their specs say. With them, the commands for chains', macros' and locators' info text gain the keyboard path [accessibility.md](accessibility.md) R8 asks for. (S6)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/ui/tests/info_view.rs`, `a_new_set_shows_the_info_view_where_live_puts_it`; `crates/ui/src/layout.rs` (tests), `the_info_view_shows_at_the_detail_areas_left` | UI script, unit |
| R2 | `crates/ui/tests/info_view.rs`, `the_status_bar_toggle_hides_it_and_the_device_view_widens`, `the_key_hides_and_shows_it`; `crates/shell/tests/shell_commands.rs`, `the_info_view_toggle_works_like_the_browsers`; `crates/ui/src/layout.rs` (tests), `hiding_the_info_view_widens_the_device_view` | UI script, unit |
| R3 | `crates/ui/tests/info_view.rs`, `a_new_set_shows_the_info_view_where_live_puts_it`; `crates/shell/tests/shell_commands.rs`, `the_info_view_toggle_works_like_the_browsers` | UI script, unit |
| R4 | `crates/ui/tests/info_view.rs`, `hovering_a_control_reads_the_command_registered_for_it`, `the_control_under_the_pointer_is_the_toggle_itself`, `hovering_empty_space_clears_it` | UI script |
| R5 | `crates/ui/tests/info_view.rs`, `hovering_a_control_reads_the_command_registered_for_it`, `hovering_empty_space_clears_it` (the content changes only when the pointer moves) | UI script |
| R6 | `crates/devices/tests/rules.rs`, `every_parameter_has_a_description` (parameters); the command registry's own completeness check, which the `cargo xtask commands` and `cargo xtask check` steps run (commands) | unit |
| R7 | `crates/ui/src/layout.rs` (tests), `stacked_views_keep_the_device_view_at_the_bottom`, `with_neither_view_the_main_view_runs_to_the_detail_areas_bottom` | unit |
| R8 | `crates/ui/tests/second_window.rs`, `the_main_windows_info_view_describes_what_the_pointer_is_over_in_the_second_window`, `it_has_no_assistant_toggle_and_no_info_view`; `crates/shell/tests/second_window.rs`, `the_info_view_and_the_undo_history_stay_in_the_main_window` | UI script |
| R9 | to be written in WP-6.58 | |
| R10 | to be written in WP-6.58 | |
| R11 | to be written in WP-6.58 | |
| R12 | to be written in WP-6.58 | |

## Not built yet

- The user's own info text (L-INFO-003), edited through Edit Info Text (M-EDIT-025), is not built; it arrives with the edit commands.
