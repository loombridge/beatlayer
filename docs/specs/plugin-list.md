# The list of a set's plug-ins

- **Status:** implemented (every rule below has a passing test; the four choices the spec notes leave to the maintainer — the item's name and place, the preset name's source, the export's format, and Show Only Missing — are the drafts the rules describe, and the decisions on them are still open; R5's program name comes from an Audio Unit, which reports the preset it holds, as CLAP's preset extension and the VST3 bindings report no program name)
- **Work packages:** WP-6.41 (R1 to R11). The command that lists them, `plugin.list`, is WP-2.9's ([plugin-scanning.md](plugin-scanning.md) R8)
- **Parity-matrix row:** none (Beatlayer's own); [the improvements](../05-improvements.md), IMP-29 (list of a set's plug-ins)
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026, chapter 5 "Managing Files and Sets" (sections 5.4.6 and 5.6 to 5.8) and chapter 24 "Using Plug-Ins"; the Live 12 release notes, 12.0 to 12.4.6; the UI parity reference's [menus.md](../ui-parity/menus.md) M-FILE-007 and [layout.md](../ui-parity/layout.md) L-OTH-002 and L-BRW-016

## The behaviour

Before sending a set to someone, or opening it on another computer, a person wants to know which plug-ins it needs. File ▸ Manage Plug-Ins, next to Manage Files, shows a list of every plug-in the set uses: its name and format, its vendor and version, the preset each device holds, the tracks it's on, and whether this computer has it. Plug-ins this computer is missing are marked, so a person sees at once what won't load. Export writes the list to a text file to send with the set or keep with a session's notes.

Live has no such list. Its File Manager (Manage Files) lists a set's samples and other files, missing ones included, but not its plug-ins. Beatlayer's list opens where the File Manager opens and leaves the File Manager as it is.

This spec covers the view, the preset names it shows, its export, and its commands. What `plugin.list` reports, and how the scanner knows what this computer has, are [plugin-scanning.md](plugin-scanning.md)'s.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The improvements](../05-improvements.md), IMP-29, and rules 1 to 4 | Every plug-in in a set, with its format, version, vendor, preset name, the tracks that use it, and which are missing on this machine; a `query` command first, then a view beside Manage Files, with export to text |
| S2 | Manual | Live 12 reference manual, chapter 5 "Managing Files and Sets", sections 5.4.6, 5.6, 5.7 and 5.8 | Manage Files (File menu) and File Manager (View menu) open the File Manager, whose Manage Set section lists the set's files with their locations, missing ones included; it lists samples, MIDI files, clips, presets and sets, not plug-ins |
| S3 | Reference | [menus.md](../ui-parity/menus.md) M-FILE-007 (Manage Files) and M-FILE-008 (Open Live Set Backups) after it; [layout.md](../ui-parity/layout.md) L-OTH-002 (the File Manager opens in the detail area's place) and L-BRW-016 (a plug-in's format badge) | Where Manage Files sits and where its view opens; how the browser marks a plug-in's format |
| S4 | Release notes | Live 12 release notes, 12.0 to 12.4.6, read 20 October 2026 | No release adds a list of a set's plug-ins |
| S5 | Beatlayer | [plugin-scanning.md](plugin-scanning.md) R8 and R13; [device-chains.md](device-chains.md) R22; [app-shell.md](app-shell.md) R15 and R17; [interface-zoom.md](interface-zoom.md) R1 | `plugin.list` groups the set's plug-ins by format and identifier with their devices, tracks and whether they're missing, and reads the scanner's cache during a scan; a device's title bar shows its preset's name; the File menu's words; where a set's project folder is; every row scales with the zoom |
| S6 | Beatlayer | [The architecture](../02-architecture.md), "Plug-in hosting" and "Agent control"; decisions D17 | A plug-in device's state in the set names the file it came from; every action is a registered command with a permission class and a readback |

## Rules

- **R1.** File ▸ Manage Plug-Ins sits straight after Manage Files (M-FILE-007). It runs `view.toggle_plugin_list`, which shows the list in the place the File Manager takes (L-OTH-002), in front of whatever showed there, and hides it again; the list's close button hides it too. Whether it shows is view state: not saved with the set and no undo step. The item's name and place are Beatlayer's own. (S1, S3)
- **R2.** The list has a row for each plug-in `plugin.list` names: its name, its format's badge (as the browser draws it, L-BRW-016), its vendor, its version, its devices' preset names (R5), and the tracks its devices are on, by name, in display order. A plug-in this computer doesn't have shows a Missing mark, and, when the scanner blocklisted its file, the reason. Rows are in `plugin.list`'s order, by name. A line above the rows says how many plug-ins the set uses and how many are missing. A set with no plug-ins shows only that line. (S1, S5)
- **R3.** A Show Only Missing switch above the rows, off each time the list opens, hides every row that isn't missing. (S1)
- **R4.** The list follows the set: a plug-in added, removed or replaced, a track renamed, and a scan finishing change its rows on the next frame, without a click. (S1, S5)
- **R5.** A plug-in device keeps the name of the preset it was last loaded from or saved as, a Beatlayer device preset's name, or else the name of the program the plug-in reports through its format where its format reports one; the set saves it with the device, and a device loaded before this package has none. `plugin.list` reports each device with its preset name, and a row shows its devices' preset names, each once. (S1, S5)
- **R6.** Clicking a track's name in a row selects that track and the first of the plug-in's devices on it, and shows it in the Device View, as `device.select` does. (S1, S5)
- **R7.** Export…, at the list's foot, asks where to save, suggesting `<set name> Plug-Ins.txt` in the set's project folder (or where the system suggests, for a set never saved), and writes the list there as UTF-8 text: a first line with the set's name and the date, then a header line, then one line per plug-in with its name, format, vendor, version, preset names, tracks, and `Missing` or nothing, separated by tabs, so a text editor and a spreadsheet both read it. It writes nothing in the set. The file's layout is Beatlayer's own. (S1)
- **R8.** Every part of the list reaches assistive technology: the rows are a table whose cells read in order, a missing plug-in's row says Missing, and the switch, the track names and Export are buttons. Every part works from the keyboard. (S6)
- **R9.** At every interface zoom from 50% to 200% the list scales by the zoom and keeps its place (interface-zoom R1). (S5)
- **R10.** `view.toggle_plugin_list` and `view.set_plugin_list_only_missing` are in the read class, and `view.state` reports whether the list shows and whether Show Only Missing is on. `plugin.export_list { "path": … }` writes R7's file and is in the `external` class, since it writes outside the set; it returns the path written, and has nothing in the set to read back. (S6)
- **R11.** `plugin.list`'s result is the list's content: an agent reads every row R2 shows, preset names included, without the view. (S1, S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/ui/tests/plugin_list.rs`, `the_list_opens_in_the_detail_area_and_its_rows_reach_assistive_technology`; `crates/shell/tests/menus.rs`, `every_item_reaches_a_command_or_is_pending` | UI script, unit |
| R2 | `crates/ui/tests/plugin_list.rs`, `the_list_opens_in_the_detail_area_and_its_rows_reach_assistive_technology`; `crates/shell/tests/plugin_list.rs`, `the_set_list_names_the_plug_ins_with_their_tracks_and_preset_names` | UI script, unit |
| R3 | `crates/ui/tests/plugin_list.rs`, `show_only_missing_hides_the_plug_ins_this_computer_has`; `crates/shell/tests/plugin_list.rs`, `the_view_state_says_whether_the_list_shows_and_shows_only_the_missing` | UI script, unit |
| R4 | `crates/ui/tests/plugin_list.rs`, `the_list_follows_the_set` | UI script |
| R5 | `crates/shell/tests/plugin_list.rs`, `the_set_list_names_the_plug_ins_with_their_tracks_and_preset_names`, `a_preset_name_survives_the_set_being_saved_and_opened_again`, `an_audio_unit_keeps_the_name_of_the_preset_it_reports` (macOS) | unit |
| R6 | `crates/ui/tests/plugin_list.rs`, `a_click_on_a_track_name_selects_the_device` | UI script |
| R7 | `crates/shell/tests/plugin_list.rs`, `the_export_writes_the_list_as_tab_separated_text`; `crates/cli/tests/agent.rs`, `an_agent_writes_the_plug_in_list_only_when_it_may` | unit, through the agent API |
| R8 | `crates/ui/tests/plugin_list.rs`, `the_list_opens_in_the_detail_area_and_its_rows_reach_assistive_technology`; `crates/ui/tests/accessibility/reachability/mod.rs`, `every_m1_command_is_reachable_from_the_keyboard` | UI script |
| R9 | `crates/ui/tests/plugin_list.rs`, `the_list_scales_with_the_interface_zoom` | UI script |
| R10 | `crates/shell/tests/plugin_list.rs`, `the_view_state_says_whether_the_list_shows_and_shows_only_the_missing`; `cargo xtask commands` | unit |
| R11 | `crates/shell/tests/plugin_list.rs`, `the_set_list_names_the_plug_ins_with_their_tracks_and_preset_names` | unit |
