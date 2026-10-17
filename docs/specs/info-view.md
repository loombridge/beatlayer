# Info View

- **Status:** implemented
- **Work packages:** WP-2.24
- **Parity-matrix row:** Info View (M2, "Built in WP-2.24")
- **Checked against:** Live 12.4.3, on 10 October 2026

## The behaviour

The Info View is the panel at the bottom left of the detail area, at the same height as the detail area and 231 points wide at the reference window. It reads whatever the pointer is over: the control's name in Live's words, then the help text of the command the control runs, and the control's key on a separate line where the default keymap has one. While the pointer is over empty space it shows nothing, and it follows the pointer, not the keyboard focus. It shows and hides from the Info toggle at the left of the Status Bar, from View ▸ Info in the View menu, and from its key; while it hides, the Device View takes its place at the detail area's left. A new, empty set opens with it shown.

The help text is written from each command's and each parameter's registered description (D17), so a piece of help exists once — registered for agents — and the Info View, the menus, and the agent API all read it. A registered command or a device parameter without a description fails a check.

This spec covers the Info View's place, its show and hide, and what it shows while the pointer is over a control. It leaves to a later package the user's own info text (L-INFO-003), which Live edits through Edit Info Text and which arrives with the edit commands.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Observation | Live 12.4.3 on macOS, 10 October 2026: the panel's place and size from the accessibility tree and measuring captures; the contents' order from watching the pointer move; a new set's default state | The Info View at the detail area's left (L-INFO-001); the name, description, and key on a separate line (L-INFO-002); the pointer-driven updates (L-INFO-002, I-FOCUS-012); a new set opens with it shown (layout.md, "Default state") |
| S2 | Manual | Live 12 reference manual, section 2.2.2 | The Info View's place, its show/hide controls, the Status Bar's Info toggle, and the user's own info text (L-INFO-001 to 003, L-STAT-001, I-FOCUS-012) |
| S3 | Manual | Live 12 reference manual, section 42.1; observed in Live 12.4.3 on macOS, 10 October 2026, from the View menu | The shortcut is Shift+Slash in the global context, and View ▸ Info is ticked while the view shows (S-VIEW-008, M-VIEW-004) |

## Rules

- **R1.** The Info View sits at the bottom left of the detail area: the detail area's left edge to 240 points at 1600 × 1000 (231 points wide), the detail area's whole height. (S1, S2)
- **R2.** The Info View shows and hides from the Info toggle at the left of the Status Bar, from View ▸ Info in the View menu, and from Shift+Slash, which applies in the global context; the menu item is ticked while it shows. While it hides, the Device View takes its place at the detail area's left, running to the detail area's whole width. (S1, S2, S3)
- **R3.** A new, empty set opens with the Info View shown. (S1)
- **R4.** While the pointer is over a control, the Info View shows the control's name, in Live's words, then the registered description of the command the control runs, and the command's key on a separate line where the default keymap has one. While the pointer is over empty space, it shows nothing. (S1, S2)
- **R5.** The Info View follows the pointer, not the keyboard focus: moving the pointer changes it; moving the keyboard focus does not. (S1, S2)
- **R6.** The text the Info View shows is the registered description of the command or parameter, not a second copy written for the UI: a registered command, or a device parameter of a device type Beatlayer registers, without a description fails a check. (architecture, D17; build plan, WP-2.24)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/ui/tests/info_view.rs`, `a_new_set_shows_the_info_view_where_live_puts_it`; `crates/ui/src/layout.rs` (tests), `the_info_view_shows_at_the_detail_areas_left` | UI script, unit |
| R2 | `crates/ui/tests/info_view.rs`, `the_status_bar_toggle_hides_it_and_the_device_view_widens`, `the_key_hides_and_shows_it`; `crates/shell/tests/shell_commands.rs`, `the_info_view_toggle_works_like_the_browsers`; `crates/ui/src/layout.rs` (tests), `hiding_the_info_view_widens_the_device_view` | UI script, unit |
| R3 | `crates/ui/tests/info_view.rs`, `a_new_set_shows_the_info_view_where_live_puts_it`; `crates/shell/tests/shell_commands.rs`, `the_info_view_toggle_works_like_the_browsers` | UI script, unit |
| R4 | `crates/ui/tests/info_view.rs`, `hovering_a_control_reads_the_command_registered_for_it`, `the_control_under_the_pointer_is_the_toggle_itself`, `hovering_empty_space_clears_it` | UI script |
| R5 | `crates/ui/tests/info_view.rs`, `hovering_a_control_reads_the_command_registered_for_it`, `hovering_empty_space_clears_it` (the content changes only when the pointer moves) | UI script |
| R6 | `crates/devices/tests/rules.rs`, `every_parameter_has_a_description` (parameters); the command registry's own completeness check, which the `cargo xtask commands` and `cargo xtask check` steps run (commands) | unit |

## Not built yet

- The user's own info text (L-INFO-003), edited through Edit Info Text (M-EDIT-025), is not built; it arrives with the edit commands.
