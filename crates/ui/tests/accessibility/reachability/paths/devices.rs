//! Commands in this area that have no key and no menu item, and how each is reached.

use crate::accessibility::reachability::setups::*;
use crate::accessibility::reachability::*;
use egui::Key;

pub(crate) fn entries() -> Vec<(&'static str, Path)> {
    use Path::Exception;
    use Reason::{AgentOnly, Equivalent, Gap, LaterView, Query};
    vec![
        ("modulation.state", Exception(Query)),
        // The Device View (WP-2.4): Enter on a title bar's controls.
        (
            "device.set_enabled",
            main(a_device, device_view, "Device Activator", Key::Enter),
        ),
        (
            "device.save_preset",
            main(a_device, device_view, "Save Preset", Key::Enter),
        ),
        (
            "device.expand",
            main(an_expandable_device, device_view, "Expand", Key::Enter),
        ),
        ("device.set_show_preset_name", Exception(DEVICE_MENU)),
        ("device.set_role", Exception(DEVICE_MENU)),
        ("device.save_default_preset", Exception(DEVICE_MENU)),
        ("device.swap_to_snapshot", Exception(DEVICE_MENU)),
        (
            "device.load_preset",
            Exception(Equivalent(
                "Enter on a preset in the browser while hot-swapping (`browser.load_selected`)",
            )),
        ),
        (
            "device.set_state",
            Exception(Equivalent("loading a preset (`device.load_preset`)")),
        ),
        (
            "device.move",
            Exception(Equivalent(
                "Cmd-X on the selected device, then Cmd-V before another (`device.cut`, \
                 `device.paste`)",
            )),
        ),
        (
            "device.add",
            Exception(LaterView(
                "the browser's device labels, once Beatlayer has devices (WP-2.5)",
            )),
        ),
        (
            "device.insert",
            Exception(Equivalent(
                "adding a device as a person does (`device.add`)",
            )),
        ),
        ("device.set_param", Exception(DEVICE_PARAMS)),
        // Racks (WP-5.1): Group and Ungroup are on Cmd-G and Cmd-Shift-G;
        // the rack's chain list and its controls arrive with the Device
        // View's rack (WP-5.2), until then agents reach them.
        (
            "device.wrap_in_rack",
            Exception(Equivalent("Group (`device.group`, Cmd-G)")),
        ),
        (
            "device.unwrap_rack",
            Exception(Equivalent("Ungroup (`device.ungroup`, Cmd-Shift-G)")),
        ),
        (
            "device.insert_in_chain",
            Exception(Equivalent(
                "adding a device as a person does (`device.add`)",
            )),
        ),
        (
            "device.move_to_chain",
            Exception(Equivalent(
                "Cmd-X on the selected device, then Cmd-V before another (`device.cut`, \
                 `device.paste`)",
            )),
        ),
        (
            "device.set_enabled_all",
            Exception(Equivalent(
                "Option-Enter (Alt-Enter) on the rack's Device Activator, as Option-click does \
                 (I-CLICK-024)",
            )),
        ),
        // The rack's frame (WP-5.2): the chain list's rows and their
        // controls are in the Device View's Tab order, and its keys are
        // the rack-chains context's.
        (
            "chain.add",
            Exception(Equivalent(
                "the chain list's drop area (`chain.create`), or Paste there (`chain.paste`, \
                 Cmd-V)",
            )),
        ),
        (
            "chain.create",
            Exception(Equivalent(
                "a drag onto the chain list's drop area; the keyboard pastes a chain \
                 (`chain.paste`, Cmd-V)",
            )),
        ),
        (
            "chain.save_preset",
            Exception(Gap(
                "a drag of the chain's row into the browser has no keyboard form yet (racks R46)",
            )),
        ),
        (
            "chain.load_preset",
            Exception(Equivalent(
                "Enter on a chain preset in the browser (`browser.load_selected`)",
            )),
        ),
        (
            "device.add_preset",
            Exception(Equivalent(
                "Enter on a preset in the browser (`browser.load_selected`)",
            )),
        ),
        (
            "device.replace",
            Exception(Equivalent(
                "Enter on a preset or device in the browser while hot-swapping \
                 (`browser.load_selected`)",
            )),
        ),
        (
            "chain.drop_on_track",
            Exception(Equivalent(
                "a drag onto the track's own chain; the keyboard cuts the chain's devices and \
                 pastes them (`device.cut`, `device.paste`)",
            )),
        ),
        (
            "chain.remove",
            Exception(Equivalent("Delete in the chain list (`chain.delete`)")),
        ),
        (
            "chain.move",
            Exception(Equivalent(
                "a drag in the chain list; the keyboard cuts the chain and pastes it \
                 (`chain.cut`, `chain.paste`)",
            )),
        ),
        (
            "chain.set_colour",
            Exception(Equivalent(
                "the chain's context menu, from its row with Shift-F10 (S-CTX-002)",
            )),
        ),
        (
            "chain.set_info",
            Exception(Equivalent(
                "Edit ▸ Edit Info Text (M-EDIT-025) while a chain is selected: it opens the Info \
                 View's field for it (the Info View spec, R10)",
            )),
        ),
        (
            "chain.set_gain",
            main(a_rack, device_view, "Chain Volume of Pad", Key::ArrowUp),
        ),
        (
            "chain.set_pan",
            main(a_rack, device_view, "Chain Pan of Pad", Key::ArrowUp),
        ),
        (
            "chain.set_enabled",
            main(a_rack, device_view, "Chain Activator of Pad", Key::Enter),
        ),
        (
            "chain.set_solo",
            Exception(Equivalent(
                "the chain's Solo button, or S in the chain list (`chain.solo`)",
            )),
        ),
        (
            "chain.set_zone",
            main(
                a_rack_with_the_zone_editor,
                device_view,
                "Pad 0 to 0, body",
                Key::ArrowRight,
            ),
        ),
        (
            "rack.set_zone_editor",
            main(a_rack, device_view, "Chain", Key::Enter),
        ),
        (
            "rack.set_auto_select",
            main(a_rack, device_view, "Auto Select", Key::Enter),
        ),
        // Spec: macros R14, R15 and R19. The Macro Controls' buttons, the
        // macros' knobs, and their context menus reach these with the
        // pointer; their keys wait for a later package.
        (
            "rack.set_macro_count",
            Exception(Equivalent("Show More and Show Fewer")),
        ),
        (
            "rack.map_mode",
            Exception(Equivalent("the macro view's Map switch")),
        ),
        (
            "rack.select_macro",
            Exception(Equivalent("a click on a macro's knob")),
        ),
        (
            "rack.map_macro",
            Exception(Equivalent("Map mode's Map button")),
        ),
        (
            "rack.set_mapping_range",
            Exception(Equivalent("the mappings list's Min and Max boxes")),
        ),
        (
            "rack.invert_mapping",
            Exception(Equivalent("the mappings list's Invert Range")),
        ),
        (
            "rack.unmap_macro",
            Exception(Equivalent("the mappings list's Delete")),
        ),
        (
            "rack.randomize_macros",
            Exception(Equivalent("the Rand button")),
        ),
        (
            "rack.rename_macro",
            Exception(Equivalent("Rename on a macro's context menu")),
        ),
        (
            "rack.set_macro_colour",
            Exception(Equivalent("Colour on a macro's context menu")),
        ),
        (
            "rack.set_macro_info_text",
            Exception(Equivalent(
                "Edit ▸ Edit Info Text (M-EDIT-025) while a macro is selected: it opens the Info \
                 View's field for it (the Info View spec, R10)",
            )),
        ),
        (
            "device.set_info_text",
            Exception(Equivalent(
                "Edit ▸ Edit Info Text (M-EDIT-025) while a device is selected: it opens the Info \
                 View's field for it (the Info View spec, R10)",
            )),
        ),
        (
            "rack.set_macro_randomize_excluded",
            Exception(Equivalent(
                "Exclude Macro from Randomization on a macro's context menu",
            )),
        ),
        (
            "rack.set_macro_style",
            Exception(Equivalent("Macro Style on a macro's context menu")),
        ),
        (
            "rack.set_mapped_value",
            Exception(Equivalent("the macro commands' own step")),
        ),
        (
            "rack.macros",
            Exception(Equivalent(
                "what the Macro Controls and the mappings list show",
            )),
        ),
        // Spec: macros R25 to R30 and R32. The Macro Variations view's
        // buttons, its rows and the macro's context menu reach these with the
        // pointer; Rename, Duplicate and Delete also take Cmd+R, Cmd+D and the
        // Delete key through the device's own commands.
        (
            "rack.store_variation",
            Exception(Equivalent("the variations view's New button")),
        ),
        (
            "rack.launch_variation",
            Exception(Equivalent("a variation row's Launch button")),
        ),
        (
            "rack.overwrite_variation",
            Exception(Equivalent("a variation row's Overwrite button")),
        ),
        (
            "rack.rename_variation",
            Exception(Equivalent("Rename on a variation row")),
        ),
        (
            "rack.duplicate_variation",
            Exception(Equivalent("Duplicate on a variation row")),
        ),
        (
            "rack.delete_variation",
            Exception(Equivalent("Delete on a variation row")),
        ),
        (
            "rack.set_macro_variation_excluded",
            Exception(Equivalent(
                "Exclude Macro From Variations on a macro's context menu",
            )),
        ),
        (
            "rack.show_variations",
            Exception(Equivalent("the view column's Show/Hide Macro Variations")),
        ),
        (
            "rack.select_variation",
            Exception(Equivalent("a click on a variation's row")),
        ),
        (
            "chain.select",
            main(a_rack, device_view, "Chain Pad", Key::Enter),
        ),
        (
            "chain.hot_swap",
            main(a_rack, device_view, "Hot-Swap of Pad", Key::Enter),
        ),
        (
            "rack.move_pads",
            main(a_drum_rack, device_view, "Pad C1: Gain", Key::ArrowDown),
        ),
        (
            "chain.preview",
            main(a_drum_rack, device_view, "Preview of Gain", Key::Enter),
        ),
        (
            "chain.set_receive",
            main(a_drum_rack, device_view, "Receive of Gain", Key::ArrowDown),
        ),
        (
            "chain.set_play",
            main(a_drum_rack, device_view, "Play of Gain", Key::ArrowDown),
        ),
        (
            "chain.set_choke",
            main(a_drum_rack, device_view, "Choke of Gain", Key::ArrowDown),
        ),
        (
            "chain.set_send",
            main(
                a_drum_rack,
                device_view,
                "Send of Gain to Reverb",
                Key::ArrowUp,
            ),
        ),
        (
            "chain.set_audio_to",
            main(
                a_drum_rack,
                device_view,
                "Audio To of Reverb",
                Key::ArrowDown,
            ),
        ),
        (
            "device.group_to_drum_rack",
            Exception(Equivalent(
                "the device's context menu (Group to Drum Rack, C-DEVICE-013), from its Show \
                 Options button (Enter, then the arrows), as in Live",
            )),
        ),
        (
            "chain.extract",
            Exception(LaterView(
                "the Session mixer's chain strips, from which a chain is dragged out (WP-5.19)",
            )),
        ),
        (
            "chain.extract_return",
            Exception(LaterView(
                "the Session mixer's chain strips, from which a return chain is dragged onto \
                 the returns (WP-5.19)",
            )),
        ),
        (
            "device.load_sample",
            Exception(Gap(
                "loading a sample has no key or menu item yet: dropping the file on the \
                 sampler's Device View frame is the only way, and where Live's own path (its \
                 Hot-Swap control) goes is to observe (sampler R11)",
            )),
        ),
        (
            "device.set_sample",
            Exception(Equivalent("loading a sample (`device.load_sample`)")),
        ),
        // WP-5.10: a modulator's Map and Unmap buttons, the controls it
        // maps while Map is lit (Enter on one), and the Shaper's preset
        // buttons, which draw its shape.
        (
            "modulator.arm_map",
            main(a_wave_mod, device_view, "Map 1", Key::Enter),
        ),
        (
            "modulator.unmap",
            main(a_mapped_wave_mod, device_view, "Unmap 1", Key::Enter),
        ),
        (
            "modulator.map",
            main(a_lit_map_button, device_view, "Gain", Key::Enter),
        ),
        (
            "modulator.set_shape",
            main(a_curve_mod, device_view, "Shape preset 1", Key::Enter),
        ),
        // The drum sampler's slot cells (WP-5.17): Tab to one, then Enter
        // selects the slot.
        (
            "device.select_slot",
            main(a_drum_sampler, device_view, "Slot 2", Key::Enter),
        ),
        // A device's sidechain strip and section (routing R29): Enter on
        // its buttons, Up and Down on its chooser and number boxes.
        (
            "device.show_sidechain",
            main(
                a_sidechained_device,
                device_view,
                "Show/Hide Sidechain",
                Key::Enter,
            ),
        ),
        (
            "device.set_sidechain_listen",
            main(
                a_sidechained_device,
                device_view,
                "Sidechain Listen",
                Key::Enter,
            ),
        ),
        (
            "device.set_sidechain_enabled",
            main(a_sidechained_device, device_view, "External", Key::Enter),
        ),
        (
            "device.set_sidechain_input",
            main(
                a_sidechained_device,
                device_view,
                "Sidechain Source",
                Key::ArrowUp,
            ),
        ),
        (
            "device.set_midi_sidechain_source",
            main(
                a_midi_sidechained_device,
                device_view,
                "MIDI Sidechain Source",
                Key::ArrowUp,
            ),
        ),
        // Down on the external instrument's channel chooser.
        (
            "device.set_external",
            main(
                a_hardware_device,
                device_view,
                "External MIDI Channel",
                Key::ArrowDown,
            ),
        ),
        (
            "device.set_sidechain_mix",
            main(a_sidechained_device, device_view, "Mix", Key::ArrowDown),
        ),
        (
            "device.set_sidechain_gain",
            main(a_sidechained_device, device_view, "SC Gain", Key::ArrowDown),
        ),
        // The multisampler (WP-5.13): the zone editor's header buttons and
        // layer rows, and the Sample tab's fields, are in the Device View's
        // Tab order.
        (
            "device.set_multisampler_view",
            main(a_multisampler, device_view, "Auto", Key::Enter),
        ),
        (
            "device.hot_swap_layer",
            main(a_multisampler, device_view, "Hot-Swap Sample", Key::Enter),
        ),
        (
            "device.select_layers",
            main(a_multisampler, device_view, "Tone", Key::Enter),
        ),
        (
            "device.delete_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet, and the \
                 Delete key, which deletes the selected device (multisampler R11)",
            )),
        ),
        (
            "device.set_layer",
            main(a_multisampler, device_view, "Root", Key::ArrowUp),
        ),
        (
            "device.set_layer_options",
            main(a_multisampler, device_view, "Snap", Key::Enter),
        ),
        (
            "device.insert_layers",
            Exception(Equivalent(
                "`device.add_layers`, which reads the files; this form takes layers whose \
                 samples are already in the set's media",
            )),
        ),
        (
            "device.update_layers",
            Exception(Equivalent(
                "`device.set_layer`, which sets the same settings and snaps the markers",
            )),
        ),
        (
            "device.set_layer_media",
            Exception(Equivalent(
                "`device.set_layer_sample`, which reads the file",
            )),
        ),
        (
            "device.set_layer_sample",
            Exception(Equivalent(
                "Hot-Swap Sample (`device.hot_swap_layer`), then Enter in the browser \
                 (`browser.load_selected`) on the file",
            )),
        ),
        (
            "device.import_sfz",
            Exception(Equivalent(
                "`browser.load_selected`, which Enter runs on a selected SFZ instrument in the \
                 browser and loads as a multisampler (multisampler R47); this form names the \
                 file, the track or the device",
            )),
        ),
        (
            "device.add_layers",
            Exception(Gap(
                "adding samples to the layer list is a drop of files on the zone editor; the \
                 keyboard has no way to name files yet (multisampler R3)",
            )),
        ),
        (
            "device.set_layer_zone",
            Exception(Gap(
                "dragging a zone's edge, body or fade in the zone editor has no keyboard \
                 form yet (multisampler R10)",
            )),
        ),
        // Sample editing in the sampler devices (WP-5.15): the display's
        // context menu, which the keyboard doesn't open yet, and gestures
        // on the waveform.
        (
            "device.edit_sample",
            Exception(Gap(
                "the sample display's context menu, which the keyboard doesn't open yet \
                 (multisampler R55)",
            )),
        ),
        (
            "device.crop_sample",
            Exception(Gap(
                "the sample display's context menu, which the keyboard doesn't open yet \
                 (multisampler R59)",
            )),
        ),
        (
            "device.reverse_sample",
            Exception(Gap(
                "the sample display's context menu, which the keyboard doesn't open yet \
                 (multisampler R59)",
            )),
        ),
        (
            "device.audition_sample",
            Exception(Gap(
                "a click on the sample display, which has no keyboard form yet (multisampler R54)",
            )),
        ),
        (
            "device.convert",
            Exception(Gap(
                "the device's title bar menu, which the keyboard doesn't open yet \
                 (multisampler R62)",
            )),
        ),
        (
            "device.add_sample_edit",
            Exception(Equivalent(
                "`device.edit_sample`, which checks the selection against the sample",
            )),
        ),
        (
            "device.clear_sample_edits",
            Exception(Equivalent(
                "`device.edit_sample` with `revert`, which reads the unedited length itself",
            )),
        ),
        (
            "device.set_processed_layer_sample",
            Exception(Equivalent(
                "`device.crop_sample` and `device.reverse_sample`, which write the copy",
            )),
        ),
        // Simpler's Start and End flags (WP-7.33): the model's command writes
        // them as given, and the shell's checks them against the sample.
        (
            "device.write_sample_region",
            Exception(Equivalent(
                "`device.set_sample_region`, which clamps the flags to the sample",
            )),
        ),
        (
            "device.set_sample_region",
            Exception(Gap(
                "dragging a flag in the sample display, which has no keyboard form yet \
                 (sampler R22)",
            )),
        ),
        // Manual slicing in the sample display, and the Warp controls
        // (WP-5.15b): the slicing gestures have no keyboard form yet, and
        // the Warp controls are in the Device View's Tab order.
        (
            "device.add_slice",
            Exception(Gap(
                "a double-click on the sample display, which has no keyboard form yet \
                 (sampler R20)",
            )),
        ),
        (
            "device.move_slice",
            Exception(Gap(
                "dragging a slice in the sample display, which has no keyboard form yet \
                 (sampler R20)",
            )),
        ),
        (
            "device.delete_slice",
            Exception(Gap(
                "a double-click on a slice in the sample display, which has no keyboard form yet \
                 (sampler R20)",
            )),
        ),
        (
            "device.set_warp",
            main(a_snapshot, device_view, "Warp", Key::Enter),
        ),
        (
            "device.set_warp_mode",
            main(a_snapshot, device_view, "Warp Mode", Key::Enter),
        ),
        (
            "device.set_warp_settings",
            main(a_snapshot, device_view, "Preserve", Key::Enter),
        ),
        (
            "device.set_sample_bpm",
            main(a_snapshot, device_view, "Sample BPM", Key::ArrowUp),
        ),
        (
            "device.scale_tempo",
            main(a_snapshot, device_view, "Double Tempo", Key::Enter),
        ),
        (
            "device.snap_layer_markers",
            Exception(Gap(
                "the loop marker's context menu, which the keyboard doesn't open yet \
                 (multisampler R20)",
            )),
        ),
        (
            "device.duplicate_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet \
                 (multisampler R11)",
            )),
        ),
        (
            "device.rename_layer",
            Exception(Gap(
                "the layer list's context menu and a double-click, which the keyboard doesn't \
                 reach yet (multisampler R11)",
            )),
        ),
        (
            "device.move_layers",
            Exception(Gap(
                "no control moves layers by hand yet: the list's sorts reorder it (multisampler \
                 R11)",
            )),
        ),
        (
            "device.sort_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet \
                 (multisampler R11)",
            )),
        ),
        (
            "device.distribute_layers",
            Exception(Gap(
                "the layer list's context menu, which the keyboard doesn't open yet \
                 (multisampler R11)",
            )),
        ),
        (
            "device.normalize_layers",
            Exception(Gap(
                "the layer list's and the waveform's context menus, which the keyboard \
                 doesn't open yet (multisampler R11)",
            )),
        ),
        ("device_view.state", Exception(Query)),
        (
            "device_view.scroll",
            Exception(Equivalent(
                "the wheel and a middle-button drag; Left and Right select the next device \
                 and bring it into view (racks R25)",
            )),
        ),
        ("device.parameters", Exception(Query)),
        ("device.display", Exception(Query)),
        // A display's view buttons (device framework R38): Enter on one.
        (
            "device.set_display_view",
            main(a_squeeze, device_view, "Activity view", Key::Enter),
        ),
        // Floating device windows (WP-2.5b): Float and Dock are
        // `device.set_floating`, on a key and in the device's menu.
        ("device.windows", Exception(Query)),
        (
            "device.set_window",
            Exception(Equivalent(
                "moving or resizing a floating device's window, which the system's window \
                 keys and commands do",
            )),
        ),
        // Plug-ins (WP-2.6): loaded by path until the browser lists them
        // (WP-2.9); the window's button is in the plug-in device's title
        // bar (WP-2.10, L-DEV-010).
        ("plugin.info", Exception(Query)),
        // ARA plug-ins (IMP-20): what their documents hold, for agents.
        ("ara.state", Exception(Query)),
        (
            "plugin.load",
            Exception(LaterView("the browser's plug-ins (WP-2.9)")),
        ),
        // Audio Units (WP-2.8) load by component description until the
        // browser lists them (WP-2.8's second part) and the scanner finds
        // them.
        (
            "plugin.load_au",
            Exception(LaterView("the browser's Audio Units (WP-2.8)")),
        ),
        (
            "plugin.show_window",
            main(
                a_plugin,
                device_view,
                "Show/Hide Plug-In Window",
                Key::Enter,
            ),
        ),
        // A plug-in's MPE Mode and settings (WP-6.17b): in its menu, and the
        // dialog's controls, which a keyboard reaches by Tab.
        ("plugin.set_mpe", Exception(DEVICE_MENU)),
        ("plugin.show_mpe_settings", Exception(DEVICE_MENU)),
        // Apply Tuning to Per-Note Pitch Bend (WP-6.9b): the menu's last item.
        ("plugin.set_tuning", Exception(DEVICE_MENU)),
        (
            "plugin.set_mpe_settings",
            Exception(Equivalent(
                "the MPE/Multi-channel Settings dialog's controls (Tab to one, then Enter), \
                 opened from the plug-in's menu",
            )),
        ),
        (
            "plugin.set_window",
            Exception(Equivalent(
                "moving or resizing a plug-in window, which the system's window keys and \
                 commands do, as for a floating device's window",
            )),
        ),
        (
            "plugin.configure",
            main(a_plugin, device_view, "Configure", Key::Enter),
        ),
        (
            "device.set_configured_params",
            Exception(Equivalent(
                "plugin.configure_param, plugin.unconfigure_param and \
                 plugin.move_configured_param, which edit the configured list one entry at a \
                 time",
            )),
        ),
        (
            "plugin.configure_param",
            Exception(Equivalent(
                "moving a parameter in the plug-in's own window while Configure is on, which \
                 adds it to the device's panel",
            )),
        ),
        (
            "plugin.unconfigure_param",
            Exception(AgentOnly(
                "removing a configured parameter; the device panel draws knobs only, so a \
                 person reaches it through the agent API",
            )),
        ),
        (
            "plugin.move_configured_param",
            Exception(AgentOnly(
                "reordering a device's configured parameters; the device panel draws knobs \
                 only, so a person reaches it through the agent API",
            )),
        ),
        (
            "plugin.set_panel_param",
            Exception(Equivalent(
                "right-clicking a panel knob and picking a parameter, which remaps that knob",
            )),
        ),
        // WP-2.9b: Settings ▸ Plug-Ins' Rescan and Plug-In Sources
        // switches. The app scans by itself at start; a scan of named paths
        // is an agent's.
        (
            "plugin.scan",
            Exception(AgentOnly(
                "scanning named files or folders; the app scans the system folders itself",
            )),
        ),
        (
            "plugin.rescan",
            settings("plug-ins", nothing, "Rescan Plug-Ins", Key::Enter),
        ),
        ("plugin.list", Exception(Query)),
        (
            "plugin.export_list",
            Exception(Equivalent(
                "the Export… button in the list of the set's plug-ins, which asks where to \
                 save the list",
            )),
        ),
        (
            "plugin.settings",
            settings(
                "plug-ins",
                nothing,
                "Use VST3 Plug-In System Folders",
                Key::Enter,
            ),
        ),
        // The autosampler's dialog (WP-5.16, IMP-27).
        (
            "device.autosample_open",
            Exception(Gap(
                "the device's title bar menu, which the keyboard doesn't open yet \
                 (multisampler R64)",
            )),
        ),
        (
            "device.autosample",
            Exception(Equivalent(
                "the autosampler's dialog's Autosample, which Tab reaches",
            )),
        ),
        (
            "device.autosample_cancel",
            Exception(Equivalent(
                "the autosampler's dialog's Cancel, which Tab reaches",
            )),
        ),
    ]
}
