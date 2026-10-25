//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{Equivalent, Gap, LaterView, Live, Query, Unobserved};
    vec![
        (
            "clip.set_colour",
            Exception(Equivalent(
                "the clip's context menu, opened with the Context Menu key or Shift-F10 \
                 (S-CTX-002) on the selected clip: its last row is the colour palette, whose \
                 swatches the arrow keys move between and Enter chooses (themes R22)",
            )),
        ),
        (
            "clip.set_scale",
            main(a_clip_selected, control_bar, "Scale Mode", Key::Enter),
        ),
        // Grooves (WP-6.19): the Groove Pool, the Clip View's groove controls
        // and the browser's Grooves label come with WP-6.20; until then
        // agents reach these.
        ("clip.set_groove", Exception(GROOVE_POOL)),
        (
            "midi.set_port",
            settings("tempo-midi", a_midi_port, "In: Keys Track", Key::Enter),
        ),
        ("midi.ports", Exception(Query)),
        (
            "clip.stretch",
            Exception(Equivalent(
                "a Shift-drag on a warped clip's edge (S-ARR-006); its length \
                 is reached from the keyboard by `arrangement.resize_clips` \
                 after the Clip View's BPM field has set the clip's tempo \
                 (`clip.set_segment_bpm`)",
            )),
        ),
        (
            "clip.slide_contents",
            Exception(Equivalent(
                "a Shift+Option-drag on a clip's contents (S-ARR-005); the \
                 same contents are reached from the keyboard by the Clip \
                 View's Start and loop fields (`clip.set_region`, \
                 `clip.set_loop`)",
            )),
        ),
        ("clip.create", Exception(EDITING)),
        ("clip.move_to_lane", Exception(EDITING)),
        ("clip.delete", Exception(EDITING)),
        ("clip.move", Exception(EDITING)),
        ("clip.move_by", Exception(EDITING)),
        ("clip.set_length", Exception(EDITING)),
        ("clip.trim_start", Exception(EDITING)),
        // Views of later milestones.
        ("clip.set_gain", Exception(CLIP_VIEW)),
        ("clip.set_launch_mode", Exception(CLIP_VIEW)),
        ("clip.set_loop", Exception(CLIP_VIEW)),
        ("clip.set_loop_enabled", Exception(CLIP_VIEW)),
        ("clip.set_midi_loop", Exception(CLIP_VIEW)),
        ("clip.set_signature", Exception(CLIP_VIEW)),
        (
            "clip_view.foreground",
            Exception(Equivalent(
                "clicking a note, or a loop bar, which assistive technology reads by name and \
                 activates (multi-clip editing R3, R10)",
            )),
        ),
        (
            "clip_view.select_loop_bars",
            Exception(Equivalent(
                "the loop bars above the note editor, which assistive technology reads by \
                 name and activates (multi-clip editing R10)",
            )),
        ),
        ("clip.set_midi_region", Exception(CLIP_VIEW)),
        ("clip.set_data", Exception(CLIP_VIEW)),
        // The crop's document-level commands (WP-4.13): `clip.crop` is the
        // one a person and an agent run, from the menus and Cmd-Shift-J, and
        // it applies these.
        (
            "clip.crop_region",
            Exception(Equivalent(
                "clip.crop, from the clip's and time selection's menus and Cmd-Shift-J (audio-clip-editing R14)",
            )),
        ),
        (
            "clip.crop_span",
            Exception(Equivalent(
                "clip.crop, from the Sample Editor's menu (audio-clip-editing R16)",
            )),
        ),
        // The reversal's commands (WP-4.7): `clip.reverse` is the one a
        // person and an agent run, from the Audio Utilities panel's Reverse
        // button, and it applies `clip.reverse_file`.
        ("clip.reverse", Exception(AUDIO_PANEL)),
        (
            "clip.reverse_file",
            Exception(Equivalent(
                "clip.reverse, from the Audio Utilities panel's Reverse button (audio-clip-editing R7)",
            )),
        ),
        // The clip's pitch (WP-4.7): the Audio Utilities panel's Transpose
        // and Detune fields.
        ("clip.set_pitch", Exception(AUDIO_PANEL)),
        ("clip.set_envelope_breakpoint", Exception(CLIP_ENVELOPES)),
        ("clip.set_envelope_curve", Exception(CURVING)),
        ("clip.move_envelope_breakpoint", Exception(CLIP_ENVELOPES)),
        ("clip.delete_envelope_breakpoint", Exception(CLIP_ENVELOPES)),
        ("clip.delete_envelope", Exception(CLIP_ENVELOPES)),
        ("clip.set_envelope", Exception(CLIP_ENVELOPE_EDIT)),
        ("clip.delete_envelope_range", Exception(CLIP_ENVELOPE_EDIT)),
        (
            "clip.move_envelope_breakpoints",
            Exception(CLIP_ENVELOPE_EDIT),
        ),
        ("clip.select_envelope_time", Exception(CLIP_SELECTING)),
        ("clip.select_envelope_segment", Exception(CLIP_SELECTING)),
        (
            "clip.insert_envelope_shape",
            Exception(CLIP_ENVELOPE_SHAPES),
        ),
        ("clip.simplify_envelope", Exception(CLIP_ENVELOPE_SHAPES)),
        ("clip.stretch_envelope_values", Exception(CLIP_HANDLES)),
        ("clip.stretch_envelope_time", Exception(CLIP_HANDLES)),
        ("clip.skew_envelope", Exception(CLIP_HANDLES)),
        (
            "clip.set_envelope_linked",
            main(
                envelopes_tab_with_an_envelope,
                device_view,
                "Linked",
                Key::Enter,
            ),
        ),
        (
            "clip.set_follow_actions",
            Exception(Unobserved(
                "the follow controls in the Launch Properties panel, which Tab reaches; their place in Live's Tab order",
            )),
        ),
        ("clip.edit_sample", Exception(AUDIO_PANEL)),
        (
            "clip.replace_sample",
            Exception(Unobserved(
                "a file dropped from the browser on the Clip View replaces the clip's sample (the \
             sample editor spec, R18)",
            )),
        ),
        ("clip.set_warp", Exception(AUDIO_PANEL)),
        ("clip.set_warp_mode", Exception(AUDIO_PANEL)),
        ("clip.set_warp_settings", Exception(AUDIO_PANEL)),
        ("clip.set_ram_mode", Exception(AUDIO_PANEL)),
        ("clip.set_tempo_leader", Exception(AUDIO_PANEL)),
        ("clip.set_high_quality", Exception(AUDIO_PANEL)),
        (
            "clip.insert_warp_markers",
            Exception(Equivalent(
                "Insert Warp Marker (Cmd+I, S-SAMPLE-009) with a time selection in the Sample \
                 Editor, which puts a marker at each transient in it (the sample editor spec, \
                 R10)",
            )),
        ),
        (
            "clip.reset_transients",
            Exception(Equivalent(
                "the Sample Editor's context menu, opened from the keyboard with Shift+F10 \
                 (Ctrl+Option+Shift+M on macOS): its Reset Transients item",
            )),
        ),
        (
            "clip.move_transient",
            Exception(Live(
                "the manual's Sample Editor keys (42.11) have none for moving a transient; \
                 Shift-dragging a pseudo-warp marker is the mouse's way",
            )),
        ),
        ("clip.transients", Exception(Query)),
        (
            "clip.shift_warp_marker",
            Exception(Live(
                "the manual's Sample Editor keys (42.11) have none for moving the waveform \
                 under a marker; Shift-dragging the marker is the mouse's way",
            )),
        ),
        ("clip.set_segment_bpm", Exception(AUDIO_PANEL)),
        ("clip.scale_tempo", Exception(AUDIO_PANEL)),
        ("clip_view.sample_editor", Exception(Query)),
        (
            "clip_view.select_transients",
            Exception(Equivalent(
                "an agent's form of clicking a transient's pseudo-warp marker; Shift+Left and \
                 Shift+Right select a span, and Delete Transient (Cmd+Shift+Delete) acts on the \
                 transients in it",
            )),
        ),
        ("clip.set_1_1_1_here", Exception(AUTO_WARP)),
        ("clip.warp_from_here", Exception(AUTO_WARP)),
        ("clip.warp_as_loop", Exception(AUTO_WARP)),
        ("clip.warp_selection_as_loop", Exception(AUTO_WARP)),
        ("clip.save_default", Exception(CLIP_TITLE_BAR)),
        ("clip.defaults", Exception(CLIP_TITLE_BAR)),
        ("clip.set_legato", Exception(CLIP_LAUNCH)),
        ("clip.set_launch_quantization", Exception(CLIP_LAUNCH)),
        ("clip.set_velocity_amount", Exception(CLIP_LAUNCH)),
        ("note.add", Exception(NOTE_EDITOR)),
        // The MIDI Tools (WP-6.11): the panels' controls are in the Tab
        // order; `midi_tool.apply` has Cmd+Enter (S-NOTE-033).
        ("midi_tool.list", Exception(Query)),
        ("midi_tool.state", Exception(Query)),
        (
            "clip_view.show_panel",
            main(
                midi_tools_shown,
                detail_area,
                "MIDI Generative Tools",
                Key::Enter,
            ),
        ),
        (
            "midi_tool.set",
            main(midi_tools_shown, detail_area, "Degrees", Key::ArrowUp),
        ),
        (
            "midi_tool.set_auto",
            main(
                midi_tools_shown,
                detail_area,
                "Test Transpose Auto",
                Key::Enter,
            ),
        ),
        (
            "midi_tool.reset",
            main(midi_tools_changed, detail_area, "Reset", Key::Enter),
        ),
        (
            "midi_tool.choose",
            main(
                midi_tools_shown,
                detail_area,
                "Transformation Selector",
                Key::ArrowUp,
            ),
        ),
        (
            "midi_tool.reseed",
            main(midi_tools_lfo_random, detail_area, "Reseed", Key::Enter),
        ),
        (
            "midi_tool.randomize",
            main(
                euclidean_shown,
                detail_area,
                "Randomize Rotations",
                Key::Enter,
            ),
        ),
        (
            "midi_tool.add_chord",
            main(stacks_shown, detail_area, "Add Chord", Key::Enter),
        ),
        (
            "midi_tool.delete_chord",
            main(stacks_shown, detail_area, "Delete Chord", Key::Enter),
        ),
        (
            "midi_tool.load_bank",
            Exception(Equivalent(
                "Enter on a chord bank selected in the browser, which runs browser.load_selected",
            )),
        ),
        (
            "midi_tool.close",
            Exception(Equivalent(
                "changing the selected notes or the time selection, which closes the open                  application (the MIDI Tools spec, R21)",
            )),
        ),
        // `note.move`, `note.resize`, `note.delete` and
        // `clip_view.select_notes` have keys of their own now: the
        // note-selection keys (S-A11Y-010, S-A11Y-011), Delete
        // (S-EDIT-005) and Cmd-A (S-NOTE-001).
        ("note.ramp_value", Exception(NOTE_EDITOR)),
        // The Clip View's MPE tab (WP-6.18): its breakpoint edits.
        ("note.set_expression", Exception(MPE_BREAKPOINTS)),
        ("note.add_expression_breakpoint", Exception(MPE_BREAKPOINTS)),
        (
            "note.move_expression_breakpoints",
            Exception(MPE_BREAKPOINTS),
        ),
        (
            "note.delete_expression_breakpoints",
            Exception(MPE_BREAKPOINTS),
        ),
        ("note.set_expression_curve", Exception(MPE_BREAKPOINTS)),
        ("note.scale_expression", Exception(MPE_BREAKPOINTS)),
        ("note.draw_expression", Exception(MPE_BREAKPOINTS)),
        ("note.clear_expressions", Exception(MPE_BREAKPOINTS)),
        // Spec: step-recording R2. The pointer's press plays a preview; from
        // the keyboard, selecting a note with the note-jump keys sounds it.
        (
            "note.preview",
            Exception(Equivalent(
                "the pointer's press on a piano ruler key or a note; a note selected from the \
                 keys sounds as a preview too (`clip_view.select_notes`)",
            )),
        ),
        // The stretch markers are dragged (I-DRAG-040); Live has no key
        // for stretching notes, so a keyboard path needs the maintainer's
        // choice of keys.
        (
            "note.stretch",
            Exception(LaterView(
                "keys for the note stretch markers (Live has none; the choice is the maintainer's)",
            )),
        ),
        // The expression lanes and the footer (WP-6.1): the pointer drags
        // the markers and uses the footer's controls and the Lane
        // Selector, which assistive technology reads by name and value
        // (note-editing R20); Tab doesn't reach them yet.
        ("note.randomize", Exception(NOTE_FOOTER)),
        // The Articulation Map's list (WP-6.47; the articulation map spec,
        // R15): its rows are reached by the pointer, and the buttons that
        // would add, delete and move an articulation are not built; the
        // spec leaves the panel's own buttons open, so agents reach them.
        (
            "articulation.add",
            Exception(LaterView(
                "the Articulation Map's list's own button for adding an articulation",
            )),
        ),
        (
            "articulation.delete",
            Exception(LaterView(
                "the Articulation Map's list's own button for deleting an articulation",
            )),
        ),
        (
            "articulation.move",
            Exception(LaterView(
                "the Articulation Map's list's own button for moving an articulation",
            )),
        ),
        (
            "articulation.set",
            Exception(Equivalent(
                "a double-click on a row's name in the Articulation Map's list",
            )),
        ),
        ("articulation.list", Exception(Query)),
        (
            "articulation.learn",
            Exception(Equivalent(
                "the Learn button in the Articulation Map's list",
            )),
        ),
        (
            "clip.save_file",
            Exception(Gap(
                "a drag of a clip into the browser has no keyboard form yet (clip files R3)",
            )),
        ),
        // WP-5.6: a drum rack's pad view and the chain list's columns.
        (
            "pad.select",
            main(a_drum_rack, device_view, "Pad C1: Gain", Key::Enter),
        ),
        (
            "pad.set_mute",
            main(a_drum_rack, device_view, "Mute of Pad C1: Gain", Key::Enter),
        ),
        (
            "pad.set_solo",
            main(a_drum_rack, device_view, "Solo of Pad C1: Gain", Key::Enter),
        ),
        (
            "pad.preview",
            main(
                a_drum_rack,
                device_view,
                "Preview of Pad C1: Gain",
                Key::Enter,
            ),
        ),
        (
            "pad.hot_swap",
            main(
                a_drum_rack,
                device_view,
                "Hot-Swap of Pad C1: Gain",
                Key::Enter,
            ),
        ),
        (
            "pad.hot_swap_sample",
            Exception(Equivalent(
                "the pad's Hot-Swap button (`pad.hot_swap`) and the browser: the Hot-Swap \
                 Sample button shows only while the pointer is over the pad (drum racks R19)",
            )),
        ),
        (
            "pad.drop",
            Exception(Equivalent(
                "a drag from the browser or the desktop onto a pad; the keyboard hot-swaps the \
                 pad (`pad.hot_swap`) and loads from the browser (`browser.load_selected`)",
            )),
        ),
        (
            "pad.swap",
            Exception(Equivalent(
                "a drag from one pad to another; changing a chain's Receive moves it to \
                 another pad (`chain.set_receive`)",
            )),
        ),
        (
            "pad.rename",
            Exception(Equivalent(
                "the pad's context menu, from the pad with Shift-F10 (S-CTX-002)",
            )),
        ),
        (
            "pad.delete",
            Exception(Equivalent(
                "the pad's context menu, from the pad with Shift-F10 (S-CTX-002), or Delete in \
                 the chain list (`chain.delete`)",
            )),
        ),
        (
            "pad.save_default",
            Exception(Equivalent(
                "the pad's context menu, from the pad with Shift-F10 (S-CTX-002)",
            )),
        ),
        // Live's own exception.
        (
            "clip.set_fades",
            Exception(Live(
                "Live doesn't support editing audio clip fades with a screen reader (manual \
                 41.4.1.3); its fade keys, Cmd-Option-F and Cmd-Option-Delete (S-ARR-008, \
                 S-ARR-009), make and delete fades from the keyboard, and the handles don't \
                 reach a screen reader",
            )),
        ),
        // MIDI file import and export (WP-2.21): the menus show them, and a
        // person reaches them once the file dialogs are built.
        (
            "midi.import_file",
            Exception(Equivalent(
                "the Create menu's Import MIDI File... dialog, which waits on the file dialogs \
                 (midi-clips R16)",
            )),
        ),
        (
            "midi.export_clip",
            Exception(Equivalent(
                "the File menu's Export MIDI Clip... dialog, which waits on the file dialogs \
                 (midi-clips R19)",
            )),
        ),
        (
            "clip.create_in_slot",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        (
            "clip.move_to_slot",
            Exception(LaterView("the Session grid and its menus (WP-3.7)")),
        ),
        // Slice to New MIDI Track's dialog (WP-4.16).
        (
            "clip.slice_to_midi_track",
            Exception(Equivalent(
                "the Slice to New MIDI Track dialog's OK, which Tab reaches",
            )),
        ),
        (
            "clip.close_slice_dialog",
            Exception(Equivalent(
                "the Slice to New MIDI Track dialog's Cancel, or Esc",
            )),
        ),
    ]
}
