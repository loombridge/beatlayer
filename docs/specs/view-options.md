# View options: panning with the middle button, note colours, a mark on empty tracks, and the mouse wheel on controls

- **Status:** implemented (R1 to R16 each have a passing test)
- **Work packages:** WP-6.49b (R1, panning with the middle button, Live's; with Live's note colours by velocity in [piano-roll.md](piano-roll.md)), WP-6.49 (R2 to R16, the three options of Beatlayer's own)
- **Parity-matrix row:** Interface placement and key bindings: Placement of every view, panel, and control (R1, panning with the middle button, which Live 12.0 added); [the improvements](../05-improvements.md), IMP-34 (a mark on empty tracks), IMP-35 (note colours by pitch class or scale degree) and IMP-36 (the mouse wheel adjusting controls)
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026, chapters 6, 10 and 42; the Live 12 release notes, 12.0 to 12.4.6; the UI parity reference's [menus.md](../ui-parity/menus.md) (View), [layout.md](../ui-parity/layout.md) L-SET-010, [interactions.md](../ui-parity/interactions.md) I-DRAG-009 and I-SCROLL-001 to 005, and [shortcuts.toml](../ui-parity/shortcuts.toml) S-VAL-001 to 008

## The behaviour

Four small things about how Beatlayer's views look and answer the mouse. One is Live's: since Live 12.0, holding the middle mouse button (pressing the wheel) and dragging pans a view, the Arrangement, the Session View or any area that scrolls, and Beatlayer does the same everywhere. The other three are Beatlayer's own options, each off or at Live's behaviour until a person changes it:

- **Note colours.** Live's MIDI Note Editor colours notes by velocity, softer notes paler. A View option keeps that or colours each note by its pitch class, the same colour for every C, or by its degree in the clip's scale, the root in one colour and the fifth in another, still paler for softer notes.
- **A mark on empty tracks.** A View option marks tracks that hold no clips and no devices, in the Arrangement, the Session View and the mixers, so a person sees what to clean up.
- **The mouse wheel on controls.** A Display & Input setting lets the wheel turn knobs and move sliders, faders and number boxes under the pointer, a step at a time as the arrow keys do. Off, the wheel only scrolls, as in Live.

This spec covers these four. The views they change are their own specs'; where a view's own rules repeat a rule here for that view, they cite it.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Release notes | Live 12 release notes, 12.0, "Feature Improvements", "Interface", read 20 October 2026, recorded in [interactions.md](../ui-parity/interactions.md) I-DRAG-058 | Holding the mouse wheel down pans views, such as the Arrangement View, the Session View, or any area that scrolls |
| S2 | Manual | Live 12 reference manual, chapter 6 "Arrangement View", section 6.2 "Navigation and Zooming", through [interactions.md](../ui-parity/interactions.md) I-DRAG-009 and I-SCROLL-001 to 005 | Cmd-Option (Ctrl-Alt) dragging pans the Arrangement; the wheel scrolls, Shift with it scrolls sideways, Cmd (Ctrl) with it zooms, Option (Alt) with it changes a track's height |
| S3 | Manual | Same manual, chapter 10 "Editing MIDI", sections 10.5.7 "Deactivating Notes", 10.5.12 "Editing Velocities", 10.6 "Folding and Scales" and 10.8.1 "Focus Mode" | A note's colour is paler the softer its velocity; deactivated notes are grey; Scale Mode highlights the clip's scale on the key track; in Focus Mode other clips' notes are grey. The manual offers no other colouring |
| S4 | Manual | Same manual, chapter 42 "Live Keyboard Shortcuts", section 42.6 "Adjusting Values", through [shortcuts.toml](../ui-parity/shortcuts.toml) S-VAL-001 to 008 | Up and Down step a focused value, Shift with them in fine steps, Shift while dragging finer; the section lists no wheel gesture for a value |
| S5 | Release notes | Live 12 release notes, 12.0 to 12.4.6, read 20 October 2026 | No release lets the wheel change a value, colours notes other than by velocity, or marks empty tracks |
| S6 | Beatlayer | [The improvements](../05-improvements.md), IMP-34, IMP-35 and IMP-36, and rules 1 to 4; [interactions.md](../ui-parity/interactions.md) I-SCROLL-007 (the wheel option) | A mark on tracks with no clips or devices; note colours by velocity (Live's), pitch class, or scale degree, as a View option; the mouse wheel adjusting knobs, sliders and number boxes, as an option if Live doesn't already |
| S7 | Reference | [menus.md](../ui-parity/menus.md) M-VIEW-036 to M-VIEW-040 (Automation Mode, Show Grouped Tracks, Show Selected Grouped Track; Fold to Notes, Fold to Scale) and [layout.md](../ui-parity/layout.md) L-SET-010 (Display & Input, its Navigation and Keyboard section) | Where the View menu keeps its track and note-editor options, and where Settings keeps its input settings |
| S8 | Beatlayer | [device-framework.md](device-framework.md) R16 to R18; [mixer-view.md](mixer-view.md) R6; [automation.md](automation.md) R5 and R17; [piano-roll.md](piano-roll.md); [arrangement-view.md](arrangement-view.md) R2, R3 and R7; [session-view.md](session-view.md) R15; [accessibility.md](accessibility.md); [interface-zoom.md](interface-zoom.md) R1 | How a control steps and drags and names its undo step; volume's and pan's keys; a hand move on an automated control and while Automation Arm records; where tracks' title bars sit; every control reaches assistive technology |

## Rules

**Panning with the middle button (Live's)**

- **R1.** Pressing the middle mouse button over any area that scrolls and dragging pans it: the area's content moves with the pointer, along both axes where the area scrolls both ways and along its one axis otherwise, within the limits its wheel scrolling keeps. It works in the Arrangement's lanes as Cmd-Option dragging does (I-DRAG-009; I-DRAG-058), in the Session View's grid, in both mixers, in the MIDI Note Editor and the Sample Editor, in the Device View's chain, in the browser's lists, and in every other area that scrolls. It selects nothing, moves no clip or note, changes nothing in the set, and isn't an undo step. Panning lasts until the button is released. (S1, S2)

**Note colours (IMP-35, Beatlayer's own options)**

- **R2.** View ▸ Note Colors, a submenu straight after Fold to Scale (M-VIEW-040), offers By Velocity, By Pitch Class and By Scale Degree, the one in use ticked; By Velocity, Live's colouring, is ticked on a new install. The choice is an application setting, kept between launches, not saved with the set, and not an undo step. The submenu's name and place are Beatlayer's own. (S3, S6, S7)
- **R3.** By Pitch Class colours each note in the MIDI Note Editor by its key's pitch class, one hue of the theme's twelve for each of C to B, the same in every octave. By Scale Degree colours each note in the clip's scale (its Scale Mode, root and scale) by its degree, the root in the first of the theme's hues and each degree up the scale in the next, and every note outside the scale in the theme's out-of-scale grey; for a clip whose Scale Mode is off it colours as By Velocity. (S6)
- **R4.** Whatever the choice, a note is paler the softer its velocity, as By Velocity is (S3); a deactivated note is grey; a selected note keeps its selection outline; and other clips' notes in Focus Mode stay grey. Only the hue changes. (S3, S6)
- **R5.** Colour is never the only sign: a note's name for assistive technology gives its pitch, velocity and, with By Scale Degree, its degree, whichever colouring is chosen. (S8)

**A mark on empty tracks (IMP-34, Beatlayer's own option)**

- **R6.** View ▸ Mark Empty Tracks, straight after Show Selected Grouped Track (M-VIEW-038), is a switch, off on a new install and ticked while on; it's an application setting, kept between launches, not saved with the set, and not an undo step. Its name and place are Beatlayer's own. (S6, S7)
- **R7.** A track is empty when it holds no clip anywhere (its lanes in the Arrangement, its take lanes and its Session slots) and no device. A group is empty when it has no device and every track in it is empty; a return track when it has no device. Main is never marked. Automation and a track's settings don't make it non-empty. (S6)
- **R8.** While the switch is on, every empty track's title bar shows a hollow ring, a glyph of Beatlayer's own, at its right end, in the title bar's text colour, wherever its title bar shows: the Arrangement's headers, the Session View's title bars, and both mixers' title bars. The mark comes and goes on the frame a clip or a device is added or removed. (S6, S8)
- **R9.** While the switch is on, assistive technology reads an empty track's title bar with "empty". (S8)

**The mouse wheel on controls (IMP-36, Beatlayer's own option)**

- **R10.** Settings ▸ Display & Input has a switch, Adjust Values with Mouse Wheel, as the last row of its Navigation and Keyboard section (L-SET-010); it's off on a new install, kept between launches, and its command is `settings.set_wheel_adjusts_values`. Its name and place are Beatlayer's own. Off, the wheel scrolls and zooms exactly as it does in Live (S2). (S4, S6, S7)
- **R11.** While it's on, turning the wheel over a knob, a slider, a fader or a number box (a device's, the mixer's, the Control Bar's, the Clip View's) moves it as the Up and Down keys move it when it has the focus (S-VAL-001, [mixer-view.md](mixer-view.md) R6, [device-framework.md](device-framework.md) R17): a step a notch, up or away from the person raising it; with Shift, in Shift's fine steps (S-VAL-002; I-SCROLL-007). A trackpad's scroll moves it a step for every 20 points of scroll, Beatlayer's own rate. The wheel over anything else scrolls and zooms as before. (S4, S6, S8)
- **R12.** While the wheel moves a control the Status Bar shows its name and value as a drag does (device-framework R18), and a control that's automated or armed for automation takes the move as a hand move does ([automation.md](automation.md) R5, R17). (S8)
- **R13.** A run of wheel moves on one control is one undo step, named as a drag on it is (Change "<name>"): the run ends when the pointer leaves the control, another control is changed, a key is pressed, or half a second passes with no wheel movement, counted on the app's clock. (S8)

**Commands and zoom**

- **R14.** `view.set_note_colors` (`velocity`, `pitch_class` or `scale_degree`) and `view.set_mark_empty_tracks` set R2's and R6's options, and `settings.set_wheel_adjusts_values` R10's; each is a registered command with a schema and a description, read back through `view.state` (`note_colors`, `mark_empty_tracks`) and `settings.state` (`display_input.wheel_adjusts_values`). (S8)
- **R15.** `track.list_empty` is a read command that lists the set's empty tracks by R7, whether or not the mark shows, so an agent finds them. (S6)
- **R16.** At every interface zoom from 50% to 200% the empty-track mark scales with its title bar (interface-zoom R1). (S8)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/ui/tests/arrangement.rs`, `the_middle_button_pans_the_lanes`; `crates/ui/tests/session_view.rs`, `the_middle_button_pans_the_grid`; `crates/ui/tests/mixer.rs`, `the_middle_button_pans_the_mixer`; `crates/ui/tests/clip_view.rs`, `the_middle_button_pans_the_note_area`; `crates/ui/tests/sample_editor.rs`, `the_middle_button_pans_the_waveform`; `crates/ui/tests/device_view.rs`, `the_middle_button_pans_the_device_chain`; `crates/ui/tests/browser.rs`, `the_middle_button_pans_the_browsers_lists` | UI script |
| R2 | `crates/ui/tests/view_options.rs`, `the_view_menu_offers_both_options_after_the_items_they_follow`; `crates/shell/tests/view_options.rs`, `the_options_are_off_at_first_read_back_and_survive_a_relaunch_outside_the_set` | UI script, headless |
| R3 | `crates/ui/tests/clip_view.rs`, `by_pitch_class_every_c_has_one_hue_and_a_softer_note_is_paler`, `by_scale_degree_colours_the_clips_degrees_and_greys_the_rest` | UI script |
| R4 | `crates/ui/tests/clip_view.rs`, `by_pitch_class_every_c_has_one_hue_and_a_softer_note_is_paler` (paler, deactivated grey), `by_scale_degree_colours_the_clips_degrees_and_greys_the_rest` (Scale Mode off) | UI script |
| R5 | `crates/ui/tests/clip_view.rs`, `a_note_is_named_for_assistive_technology_with_its_pitch_velocity_and_degree` | UI script |
| R6 | `crates/ui/tests/view_options.rs`, `the_view_menu_offers_both_options_after_the_items_they_follow`; `crates/shell/tests/view_options.rs`, `the_options_are_off_at_first_read_back_and_survive_a_relaunch_outside_the_set` | UI script, headless |
| R7 | `crates/shell/tests/view_options.rs`, `track_list_empty_lists_tracks_with_no_clip_and_no_device_whatever_the_switch` | Headless |
| R8 | `crates/ui/tests/view_options.rs`, `an_empty_track_wears_the_ring_in_the_arrangement_until_a_clip_or_a_device_is_added`, `the_session_view_and_both_mixers_mark_empty_tracks_too` | UI script |
| R9 | `crates/ui/tests/view_options.rs`, `an_empty_track_wears_the_ring_in_the_arrangement_until_a_clip_or_a_device_is_added`, `the_session_view_and_both_mixers_mark_empty_tracks_too` | UI script |
| R10 | `crates/ui/tests/view_options.rs`, `the_wheel_switch_is_the_last_row_of_navigation_and_keyboard`; `crates/ui/tests/mixer.rs`, `the_wheel_scrolls_the_mixer_over_a_fader_unless_the_switch_is_on`; `crates/ui/tests/device_panel.rs`, `with_the_switch_off_the_wheel_leaves_the_knob_alone` | UI script |
| R11 | `crates/ui/tests/device_panel.rs`, `the_wheel_moves_a_knob_a_percent_a_notch_and_a_run_is_one_undo_step`, `shift_makes_the_wheel_step_finely`, `a_trackpad_moves_a_knob_a_step_for_every_twenty_points`; `crates/ui/tests/mixer.rs`, `the_wheel_moves_a_fader_a_decibel_a_notch_and_shift_a_tenth`; `crates/ui/tests/transport.rs`, `the_wheel_steps_the_tempo_field_as_the_keys_do` | UI script |
| R12 | `crates/ui/tests/device_panel.rs`, `the_status_bar_reads_the_knob_while_the_wheel_moves_it`, `the_wheel_on_an_automated_knob_overrides_its_envelope_as_a_hand_move_does` | UI script |
| R13 | `crates/ui/tests/device_panel.rs`, `the_wheel_moves_a_knob_a_percent_a_notch_and_a_run_is_one_undo_step`, `a_run_ends_when_the_pointer_leaves_the_control_or_a_key_is_pressed`; `crates/ui/tests/mixer.rs`, `the_wheel_moves_a_fader_a_decibel_a_notch_and_shift_a_tenth` | UI script |
| R14 | `crates/shell/tests/view_options.rs`, `the_options_are_off_at_first_read_back_and_survive_a_relaunch_outside_the_set`; `cargo xtask commands` (schemas, descriptions, readback rows) | Headless |
| R15 | `crates/shell/tests/view_options.rs`, `track_list_empty_lists_tracks_with_no_clip_and_no_device_whatever_the_switch` | Headless |
| R16 | `crates/ui/tests/view_options.rs`, `the_ring_keeps_its_place_in_its_title_bar_at_every_zoom` | UI script |
