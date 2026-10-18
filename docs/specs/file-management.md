# Templates and file management

- **Status:** implemented
- **Work packages:** WP-2.25
- **Parity-matrix row:** Files and export: Sets, templates, Collect All and Save, File Manager (missing files)
- **Checked against:** the Live 12 reference manual (online edition), read on 14 October 2026

## The behaviour

You make a set the one a new set starts from, and save templates to reuse a setup. You collect a set's external files into its project folder so the set no longer depends on files elsewhere. When a set is opened and a file it uses can't be found where it was, Beatlayer looks for it again by its content and relinks it. This spec covers the default set, templates, Collect All and Save, and finding and relinking missing files. Where the commands sit in the File menu is in menus.md; how a file becomes a clip and where a set's media table lives are in audio-import.md and app-shell.md.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 5 "Managing Files and Sets", section 5.4.5 "Template Sets" | Save Live Set As Default Set makes the current set the initialized default state for new sets; Save Live Set As Template saves a template that appears in the browser's Templates category and the User Library's Templates folder; that folder is made the first time a template is saved; a template loads with the saved configuration. |
| S2 | Manual | Same chapter, section 5.7 "Collecting External Files" | Collect All and Save copies files used outside the project into the project's Samples folder (or Presets folder for devices); a dialog chooses which locations to collect from. |
| S3 | Manual | Same chapter, section 5.6 "Locating Missing Files", sections 5.6.1 and 5.6.2 | The Status Bar shows a warning for missing files; the File Manager offers manual repair and an automatic search whose options are Search Folder (a chosen folder, with subfolders), Search Project, and Search Library; one candidate is accepted, several need the person to choose. |
| S4 | Beatlayer | audio-import.md R9 and R10; the architecture's "Project files and interop" | Media is identified by the BLAKE3 hash of the file's bytes and relinked by content, so a moved file is found whatever its name; the search hashes only files of the recorded length. |
| S5 | Decision | D20 in the architecture | A new install's factory default set opens in Arrangement View; a saved default set keeps the user's choice. |

## Rules

- **R1.** On start-up and after New Set, the set is the saved default set when one has been saved and can be read, otherwise the factory default set. (S1, S5)
- **R2.** `set.save_as_default` saves the open set as the default set, which R1 then uses. It is saved as a copy, so the open set keeps its own file, its name, and its undo history, and stays open. `path` names the default set's file (`.blset` is added if it's missing); left out, it goes in the User Library's `Templates` folder, or beside the application's preferences when there is no User Library. The choice is kept for the next start. (S1)
- **R3.** `set.save_as_template` saves the open set as a template in the User Library's `Templates` folder, making that folder the first time. `name` names the template, without the `.blset` extension; left out it takes the open set's name. As in R2 the open set is unchanged. A template is an ordinary set file: it can be opened, or set as the default set with R2. (S1)
- **R4.** `set.collect_all_and_save` copies every media file the set uses that lives outside its project folder into the project's `Samples/Imported` folder, re-points the set's media table at the copies, and saves the set, so the set no longer depends on files elsewhere. A name already taken in `Samples/Imported` gets ` 2`, ` 3`, and so on before its extension, so nothing is overwritten. It fails if the set has never been saved, so it has no project folder to collect into. The result's `files` is how many files were copied. (S2)
- **R5.** The browser's Templates label lists the set files in the User Library's `Templates` folder (browser R2); with no User Library it says so. (S1)
- **R6.** Files the set uses that are missing or changed are found again by their content (audio-import R10): Beatlayer searches the set's project folder, the User Library, the folders the files were last in, and any folders a call gives, each with its subfolders; it hashes only files of the recorded length and relinks each missing file to the first match, whatever its name. This runs by itself when a set is opened, and on demand as `set.relink_missing`. The set's media table is re-pointed at what was found, so the next save keeps the new paths; the Status Bar says how many were relinked. The result's `files` is how many were relinked. (S3, S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/shell/tests/file_management.rs`, `a_new_set_opens_from_the_saved_default` | unit, through `Shell::run` on real folders |
| R2 | `crates/shell/tests/file_management.rs`, `a_new_set_opens_from_the_saved_default`, `the_file_commands_are_registered_and_reach_the_agent_api` | unit |
| R3 | `crates/shell/tests/file_management.rs`, `templates_are_saved_into_the_user_library_and_listed` | unit, through `Shell::run` on real folders |
| R4 | `crates/shell/tests/file_management.rs`, `collect_all_and_save_copies_external_files_into_the_project` | unit, through `Shell::run` on real folders |
| R5 | `crates/shell/tests/file_management.rs`, `templates_are_saved_into_the_user_library_and_listed` | unit |
| R6 | `crates/shell/tests/file_management.rs`, `a_moved_file_is_found_by_the_automatic_search` | unit, through `Shell::run` on real folders |

## Not built yet

- A default set that uses samples: R1 opens the saved default set's document, but its media table isn't loaded into the media store yet, so such a set's clips play silence until relinked.
- Collect All's choice of locations to collect from: it isn't offered, and R4 collects every external file.
- Choosing between several candidates for one missing file: it isn't offered, and R6 relinks the first match, taking the folders in the order R6 lists them.
