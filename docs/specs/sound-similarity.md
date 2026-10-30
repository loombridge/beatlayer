# Sound similarity: analysis, Show Similar Files, and similar sample swapping

- **Status:** draft
- **Work packages:** WP-9.8 (sound analysis: what is analysed, the descriptor, the database, the Status Bar's progress and Pause: R1 to R10); WP-9.9 (Show Similar Files and its results: R11 to R19); WP-9.10 (similar sample swapping in Simpler, Drum Sampler and Drum Rack pads, and the similarity reference: R20 to R30). Automatic tags from the same analysis are [browser tags](browser-tags.md) R49 to R58; the browser's places, search and item menu are [browser](browser.md)'s; the samplers' own rules are [sampler](sampler.md)'s, [one-shot drum sampler](one-shot-drum-sampler.md)'s and [drum racks](drum-racks.md)'
- **Parity-matrix row:** Views and workflow: Browser: similarity search and similar-sample swapping
- **Checked against:** the Live 12 reference manual (online edition), chapter 3 section 3.4 and chapter 4 section 4.4.2, read 30 October 2026; chapter 42, sections 42.22 and 42.23; the UI parity reference rows C-BROWSER-003, M-VIEW-026, S-BRW-010, S-SWAP-001 to 005 and I-HOLD-004

## The behaviour

Beatlayer listens to the person's samples and presets in the background and keeps a description of how each sounds. From any compatible file selected in the browser, Show Similar Files lists the sounds most like it, from every place, most similar first, with the file as the reference in the search field. A saved label made from such a list keeps the reference at its top.

In Simpler, Drum Sampler and a Drum Rack's pads, the same search swaps the loaded sample for the next or previous similar one, from the device's display or with Cmd-Right and Cmd-Left; Cmd-Up keeps the sample playing now as the reference, and Cmd-Down goes back to it. Holding Option over a Drum Rack shows the swap controls on its pads.

Analysis runs at low priority and never makes the browser or the interface wait. While it runs, the Status Bar shows how far it has got, with a Pause button. The description is Beatlayer's own set of measurements of the sound, computed on this computer with no download.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 3 "Live Concepts", section 3.4 "Sound Similarity" | Similarity search works on samples up to 60 seconds long, instrument presets and drum presets, comparing a reference against the libraries. New user audio files are analysed in the background. While scanning and analysis run, the Status Bar shows their state, with a Pause button beside it. A Show Similar Files icon appears beside a compatible file selected in the browser; the icon, the item's menu and Cmd-Shift-F (Ctrl-Shift-F) show similar files. The reference shows in the search field, and the similar sounds below it, most similar first. A saved label made from the results puts the reference at its top. Similar sample swapping replaces the samples in Drum Rack, Simpler and Drum Sampler with similar sounds |
| S2 | Manual | Chapter 4 "Working with the Browser", section 4.4.2 "Tags" | The sound analysis also tags samples up to 60 seconds long |
| S3 | Manual | Chapter 42, section 42.22 (S-BRW-010) and section 42.23 (S-SWAP-001 to 005), through [shortcuts.md](../ui-parity/shortcuts.md) | Cmd-Shift-F shows similar files; Cmd-Right and Cmd-Left swap to the next and previous similar sample, Cmd-Up keeps the current sample as the reference, Cmd-Down returns to it, and Option held shows the swap controls on drum pads |
| S4 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in [menus.md](../ui-parity/menus.md) C-BROWSER-003 and M-VIEW-026 | Show Similar Files in a browser item's menu after Show in Finder, and in the View menu, both with ⇧⌘F |
| S5 | Reference | [interactions.md](../ui-parity/interactions.md) I-HOLD-004 | Option held over a Drum Rack shows the similar-sample swap controls |
| S6 | Spec | [browser tags](browser-tags.md) R1 to R4 (the database on disk), R2 (search within a frame at 100,000 names) and R22 to R27 (saved labels); [browser](browser.md) R25 (the item menu) and R27 (instrument presets' previews, WP-6.67); [one-shot drum sampler](one-shot-drum-sampler.md); [sampler](sampler.md) (the region in frames); [drum racks](drum-racks.md) | Where the descriptions are kept, how search must stay fast, how labels are saved, how presets are previewed |
| S7 | Beatlayer | The architecture's "Processes and threads" (the background pool), "Media and warping" (the media store's content hashes) and "Agent control" (D17) | Background work at low priority; files named by the hash of their bytes; every action a registered command with a readback |

## Rules

### Sound analysis (WP-9.8)

- **R1.** Beatlayer analyses every audio file of 60 seconds or less in the browser's places and the User Library, and every instrument and drum preset there and among Beatlayer's own devices' presets. Files longer than 60 seconds, MIDI files, sets, clips and effect presets aren't analysed and can't be a reference. (S1, S2)
- **R2.** An audio file's description is computed from its decoded audio summed to mono at 22,050 Hz; a preset's from the render the browser previews it with ([browser](browser.md) R27): middle C held for one second, then released for one. (S1, S6; the preset's render is Beatlayer's own)
- **R3.** The description is Beatlayer's own vector of measurements, each normalised over the analysed material: the means and deviations of 40 log-mel band energies and of 20 mel-frequency cepstral coefficients, the spectral centroid's, flatness's and flux's means and their envelopes over eight equal parts of the sound, the onset density, the length in seconds (on a logarithmic scale), and how pitched the sound is. Two sounds' distance is the cosine distance between their vectors. (S7; Beatlayer's own)
- **R4.** Descriptions are kept in the browser's database by the BLAKE3 hash of the file's bytes, so a file is analysed once, a moved or renamed file keeps its description, and two copies share one. A changed file is analysed again. A preset's description is kept by the hash of its file and of the devices' versions it names. (S6, S7)
- **R5.** Analysis runs on the background pool at the lowest priority, one file at a time, after indexing has finished with a place, and takes no more than one core. It reuses decoded audio and Beatlayer's analysis cache where a file is already decoded or analysed. Search ([browser tags](browser-tags.md) R2), previews, the interface and the audio thread never wait for it. (S1, S6, S7)
- **R6.** While files wait to be analysed, the Status Bar's right end shows "Analyzing Sounds" with the share done and a Pause button; Pause stops the analysis once the file under way is finished, the button then reading Resume, and Resume goes on from the next file. The pause lasts until Resume or until Beatlayer quits. With nothing waiting, neither shows. (S1; the wording and the place are Beatlayer's own)
- **R7.** `browser.analysis_state` (`read`) gives the number of files and presets analysed, waiting and skipped, whether analysis is paused, and the item under way; `browser.set_analysis_paused` (`edit`) pauses and resumes it, and names `browser.analysis_state` as its readback. (S7)
- **R8.** A file that can't be decoded is skipped and counted as skipped, and isn't tried again until it changes. (Beatlayer's own)
- **R9.** On Beatlayer's generated test families (kicks, snares, hi-hats, basses and pads, each made from one recipe with random variation), the five nearest neighbours of each sound are of its own family in at least 80 % of cases. (S7; the measure and target are Beatlayer's own)
- **R10.** Analysis needs no download and no network. (S7)

### Show Similar Files (WP-9.9)

- **R11.** With one compatible item (R1) selected in the browser's content pane, a Show Similar Files icon shows at the right end of its row; clicking it shows similar files. Show Similar Files in the item's menu (C-BROWSER-003) and in the View menu (M-VIEW-026), and Cmd-Shift-F, Ctrl-Shift-F on Windows and Linux (S-BRW-010), do the same for the selected item. For an item not yet analysed, or not compatible, the icon doesn't show and the menu items are disabled, with the reason in their description. (S1, S3, S4; the icon's place in the row is Beatlayer's own)
- **R12.** Showing similar files runs `browser.similar` (`read`) with the item: the browser chooses the All label, puts the reference in the search field as a token naming the file, and lists in the content pane the analysed items most like it from every place, most similar first, the reference itself first. The list holds items of the reference's own kind: samples for a sample, presets for an instrument or drum preset. (S1; keeping kinds apart is Beatlayer's own)
- **R13.** The list holds the 100 nearest items whose distance from the reference is under 0.35, fewer if fewer are that near. (Beatlayer's own)
- **R14.** Typing in the search field after the reference narrows the list by name as any search does; deleting the reference token, or Esc, ends the similarity search and goes back to the browser as it was. The filters ([browser tags](browser-tags.md) R18 to R21) narrow the list too. (S1; Beatlayer's own)
- **R15.** Saving a label from the results ([browser tags](browser-tags.md) R22) saves the reference and its similar items, with the reference first; the label lists them in that order, not by name. (S1)
- **R16.** `browser.similar`'s result gives the reference and each item's path, kind and distance. It needs no window and runs headless. (S7)
- **R17.** Previewing, dragging and loading items from the list work as for any list in the browser. (S1)
- **R18.** The icon reaches assistive technology as a button named Show Similar Files on the selected row; the reference token in the search field is read as "Similar to" and the file's name. (S7; the names are Beatlayer's own)
- **R19.** The results come from the database alone, in the time a search takes ([browser tags](browser-tags.md) R2), with no new analysis. (S6)

### Similar sample swapping (WP-9.10)

- **R20.** Swapping works on Simpler, on Drum Sampler, and on a Drum Rack's pad whose chain plays one Simpler or Drum Sampler. Each has a similarity reference: the sample it held when it was loaded, until changed (R23). (S1)
- **R21.** Swapping to the next similar sample (`device.swap_similar` with `direction` `next`, `edit`) loads into the device the next item in the reference's similar list (R12, R13) after the sample it holds; the previous (`previous`) loads the one before it. The list doesn't wrap: past either end the command is refused. A swap is one undo step named Swap Sample. (S1, S3; not wrapping is Beatlayer's own)
- **R22.** A swap replaces only the sample. Every parameter keeps its value; the region (Start and End flags, [sampler](sampler.md)) resets to the new sample's whole length, and slices and warp markers detected in the old sample are worked out again for the new one. On a pad, the pad's note, name colour (unless the name was the old sample's, in which case it takes the new sample's), choke group and chain settings stay. (S1; Beatlayer's own)
- **R23.** Save as Similarity Reference (`device.set_similarity_reference`, `edit`) makes the sample the device holds its reference; Return to Reference (`device.return_to_reference`, `edit`) loads the reference back, one undo step named Return to Reference, disabled while the device holds it. The reference is saved with the set and with the device's preset. (S1, S3)
- **R24.** With the Device View or a Drum Rack's pads focused and a swappable device or pad selected, Cmd-Right (S-SWAP-001) swaps to the next, Cmd-Left (S-SWAP-002) to the previous, Cmd-Up (S-SWAP-003) saves the reference and Cmd-Down (S-SWAP-004) returns to it; Ctrl on Windows and Linux. (S3)
- **R25.** Simpler's and Drum Sampler's sample displays show, at the top right of the waveform, Previous Similar and Next Similar buttons and a Show Similar Files button, and their menus hold Show Similar Files, Save as Similarity Reference and Return to Reference. Show Similar Files there runs R12 with the device's reference. (S1; the buttons' places are Beatlayer's own)
- **R26.** While Option (Alt on Windows and Linux) is held over a Drum Rack's pads (S-SWAP-005, I-HOLD-004), each swappable pad shows Previous Similar and Next Similar arrows over its left and right edges, which swap that pad's sample. Releasing Option hides them. (S3, S5; the arrows' places are Beatlayer's own)
- **R27.** A device or pad whose reference hasn't been analysed, or whose reference has no similar items, shows the swap buttons disabled and refuses the commands with the reason. (Beatlayer's own)
- **R28.** The query tool's `device` reads back a swappable device's sample, its reference and the position of its sample in the reference's list; each swap command names it as its readback. (S7)
- **R29.** A swap's new sample is loaded off the audio thread and reaches the engine as a sample change does, at a block boundary, with the voices playing the old sample fading out over 5 ms; nothing on the audio thread allocates, locks or blocks. (S7; the fade is Beatlayer's own)
- **R30.** The swap buttons and pad arrows reach assistive technology as buttons named Previous Similar Sample, Next Similar Sample and Show Similar Files. (S7; the names are Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R10 | to be written in WP-9.8: `crates/analysis/src/similarity.rs`; `crates/shell/tests/sound_analysis.rs` over a generated place, a restart with real database files, a move, Pause on the test clock; R9's generated families | unit, shell test |
| R11 to R19 | to be written in WP-9.9: `crates/ui/tests/similar_files.rs` for each entry point; `crates/shell/tests/similar_files.rs` for the order; `crates/ui/tests/keymap/resolve.rs` for S-BRW-010 | UI script, shell test |
| R20 to R30 | to be written in WP-9.10: `crates/shell/tests/sample_swap.rs` per device and pad, undo, a save and reload of the reference; `crates/ui/tests/sample_swap.rs` pressing every S-SWAP key | shell test, UI script |
