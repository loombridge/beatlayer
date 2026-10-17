# Plug-in scanning

- **Status:** partial (CLAP and VST3 scanning, the cache, the blocklist, Rescan, the browser's Plug-Ins label, and the set's plug-in list are built and tested; Audio Unit and LV2 wait for their hosting packages)
- **Work packages:** WP-2.9. The hosting itself is WP-2.6 ([CLAP hosting](clap-hosting.md)), WP-2.7 ([VST3 hosting](vst3-hosting.md)) and WP-2.8 (Audio Unit); the browser is WP-1.10; the set's file management is WP-2.25
- **Parity-matrix row:** Plug-in formats: (Beatlayer only) CLAP; the browser's Plug-Ins category
- **Checked against:** [the architecture](../02-architecture.md), "Plug-in hosting" and "Scanning"; the CLAP specification 1.2 (`clap/entry.h`, `clap/plugin.h`, and the `CLAP_PATH` convention); Live 12.4.6 on macOS for the browser's Plug-Ins label (observed 13 October 2026)

## The behaviour

The browser's Plug-Ins label lists the plug-ins installed on this machine, each by its name, in the order the scanner found it (by name). Rescan looks for them again. A plug-in that crashed or hung while it was being scanned isn't listed but is remembered on a blocklist, with the reason, so nothing tries to load it again by accident; the set's list of plug-ins (`plugin.list`) names any the set uses that this machine doesn't have, and why. Hidden plug-ins and a preferred format change only what the browser and search show, never what a set may load.

A scan describes each plug-in by running it, so a plug-in's own code can crash or hang the process it runs in. Beatlayer therefore scans in `beatlayer-scan`, a child process with a timeout: when the child stops answering or dies, the scanner kills it (if it still runs), records the plug-in on the blocklist, and carries on with the next file. The main process never loads a plug-in's code to scan it.

The scanner caches what it found by the plug-in file's path, size, and modification time, so a scan of files that haven't changed reads the cache and only loads the ones that did. The cache and the blocklist are kept in the config directory with the rest of the app's settings. Beatlayer scans only when `plugin.scan` or `plugin.rescan` runs, and does so synchronously.

CLAP and VST3 are scanned, each in its format's own folders; Audio Unit and LV2 reuse the same scanning, cache, blocklist, and commands when their hosting lands (WP-2.8). The browser's "Show hidden" toggle and its list of inactive plug-ins are drawn in Phase 6; the data and commands are built.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The architecture](../02-architecture.md), "Plug-in hosting" and "Scanning" | Scanning runs in `beatlayer-scan`, a child process with a timeout; a crash or hang goes on a blocklist and the main app never loads it; results are cached by path, modification time, and format. |
| S2 | Specification | The CLAP specification 1.2 | A plug-in is a shared library or bundle with a factory that lists descriptors (ID, name, vendor, version, features); the host honours `CLAP_PATH`; a descriptor's features say whether it's an instrument, a note effect, or an audio effect. |
| S3 | Manual | Live 12 reference manual, chapter 23 "Working with Instruments and Effects", section 23.2 "Using Plug-Ins" | Plug-ins sit in device chains as Live's own devices do; Live finds them in the plug-in folders the user's formats use, and the browser lists them. |
| S4 | Observation | Live 12.4.6 on macOS, 13 October 2026 | The browser's sidebar has a Plug-Ins label beside the other Library labels; a newly installed plug-in needs Rescan before it appears. |

## Rules

- **R1.** `plugin.scan` finds the plug-in files in the paths it's given (a file, a folder searched below it, or each format's own system plug-in folders when no path is given; for VST3, the format's user, system and network folders) and describes each one: its format, file, identifier, name, vendor and version when it gives them, and its role in a chain from its features. (S2, S3)
- **R2.** The scanner caches each plug-in file by its path, size, and modification time; a scan of a file whose size and time are unchanged answers from the cache without loading it. The cache is kept in the config directory and read again at the next start. (S1)
- **R3.** The scanner loads each plug-in in the `beatlayer-scan` child process, never in the main process, and waits at most a timeout for it. (S1)
- **R4.** A plug-in whose scan crashes, hangs past the timeout, or refuses to load doesn't stop the scan: the child is stopped and the plug-in is put on the blocklist with the reason. The blocklist is kept with the cache. (S1)
- **R5.** The test plug-ins (`bl-test-clap` and `bl-test-vst3`) can be told to crash or to hang when they're scanned, so R3 and R4 are tested with a real plug-in of each format. Only the tests use this. (Beatlayer's own testing rule.)
- **R6.** `plugin.rescan` scans the same paths again, ignoring the cache and clearing the blocklist for them. (S1, S4)
- **R7.** The Plug-Ins label lists what the scanner found, and search over it matches by name. Hidden plug-ins are left out unless "Show hidden" is on; when a plug-in exists in more than one format, the preferred format is the one shown. Hiding a plug-in never stops a set from loading it, and it's remembered by its format and identifier, so moving its file keeps it hidden. `plugin.settings` has no way to clear the format preference once set; the Settings UI (Phase 6) will give one. (S4)
- **R8.** `plugin.list` names every plug-in the set uses, grouped by format and identifier, with the devices and tracks that use it and whether this machine has it (`missing`, with `blocked` and the reason when the scanner blocklisted its file). The list has no preset name yet: a set holds a device's plug-in state, not the name of a preset it came from. (S3)
- **R9.** `plugin.scan`, `plugin.rescan`, `plugin.list`, and `plugin.settings` are registered commands with schemas and descriptions, so an agent reaches them. Scanning reads files outside the project, so it's in the external permission class. (Beatlayer's command rule.)
- **R10.** CLAP and VST3 are scanned; VST3's files are found in the format's own user, system and network folders on each platform, and its loader is the VST3 host's (WP-2.7). Audio Unit and LV2 scanning are left as hooks: the scanner's format type, the extension it searches for, and the command's format all take any of them, and each format's own hosting (WP-2.8) adds the loader. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/plugin-host/tests/scan.rs`, `a_scan_describes_the_test_plugin_and_caches_it`, `a_scan_describes_the_test_vst3_plugin_and_caches_it`, `a_folder_is_searched_for_vst3_plug_ins`; `crates/shell/tests/plugin_scanning.rs`, `scanning_rescanning_and_the_set_list_work_through_commands`; `crates/agent/tests/plugin_vst3.rs`, `a_scan_lists_vst3_plug_ins_through_the_agent_api` | unit, agent command |
| R2 | `crates/plugin-host/tests/scan.rs`, `a_scan_describes_the_test_plugin_and_caches_it`, `a_scan_describes_the_test_vst3_plugin_and_caches_it`, `the_cache_round_trips_through_the_store_file` | unit |
| R3 | `crates/plugin-host/tests/scan.rs`, `a_plugin_that_hangs_on_scan_times_out_and_is_blocklisted`, `a_vst3_module_that_hangs_on_scan_times_out_and_is_blocklisted` | unit |
| R4 | `crates/plugin-host/tests/scan.rs`, `a_plugin_that_crashes_on_scan_is_blocklisted`, `a_plugin_that_hangs_on_scan_times_out_and_is_blocklisted`, `a_vst3_module_that_crashes_on_scan_is_blocklisted`, `a_vst3_module_that_hangs_on_scan_times_out_and_is_blocklisted`, `a_file_that_isnt_a_plugin_is_blocklisted_with_why`; `crates/shell/tests/plugin_scanning.rs`, `a_plugin_that_fails_a_scan_is_blocklisted_and_reported` | unit, agent command |
| R5 | `crates/test-clap/src/lib.rs` (the `BL_TEST_CLAP_SCAN` modes) and `crates/test-vst3/src/lib.rs` (the `BL_TEST_VST3_SCAN` modes), used by the crash and hang tests above | unit |
| R6 | `crates/plugin-host/tests/scan.rs`, `a_scan_describes_the_test_plugin_and_caches_it`, `a_scan_describes_the_test_vst3_plugin_and_caches_it`; `crates/shell/tests/plugin_scanning.rs`, `scanning_rescanning_and_the_set_list_work_through_commands` | unit, agent command |
| R7 | `crates/plugin-host/tests/scan.rs`, `hidden_plugins_are_left_out_unless_asked_for`, `the_format_preference_keeps_the_preferred_copy`; `crates/shell/tests/plugin_scanning.rs`, `hidden_plug_ins_leave_the_browser_and_come_back_with_show_hidden`, `a_scan_finds_vst3_plug_ins_and_the_format_preference_chooses` | unit, agent command |
| R8 | `crates/shell/tests/plugin_scanning.rs`, `scanning_rescanning_and_the_set_list_work_through_commands` | agent command |
| R9 | `crates/shell-spec/src/plugins.rs`, `every_listed_id_is_registered_and_complete` | unit |
| R10 | `crates/plugin-host/src/scan.rs`, `files_are_taken_by_extension_and_bundles_by_name`; `crates/plugin-host/tests/scan.rs`, `the_vst3_folders_follow_the_formats_locations`, `vst3_bundles_are_taken_by_extension_and_name` | unit |
