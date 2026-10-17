# Drum racks

- **Status:** draft
- **Work packages:** WP-5.6. What every rack has (the rack in the document and the engine, chains and their mixers, the chain list, presets and hot-swap) is [racks](racks.md)' (WP-5.1, WP-5.2, WP-5.7, WP-5.8); a drum rack's Macro Controls are [macros](macros.md)'; Slice to New MIDI Track's drum rack of samplers is WP-4.16's; pad controllers that follow the visible pads come with control surfaces (Phase 7); similar-sample swapping (S-SWAP-001 to S-SWAP-005) is Phase 9's
- **Parity-matrix row:** Racks and device features: Drum Rack pads, choke groups, return chains, extract chains
- **Checked against:** the Live 12 reference manual (online edition), read 17 October 2026 (chapters 17, 23, 25 and 42), and the UI parity reference's rows [L-DEV-008](../ui-parity/layout.md), [C-DEVICE-013](../ui-parity/menus.md) and [S-VIEW-007](../ui-parity/shortcuts.toml)

## The behaviour

A drum rack is Beatlayer's equivalent of Live's Drum Rack: an instrument rack ([racks](racks.md)) whose chains are chosen by note rather than by zones. Each chain receives one note and passes it on to its devices as a note of its own choosing, so every drum sound can sit on its own key and play its instrument at whatever pitch it needs; a chain can belong to one of sixteen choke groups, where each hit silences the others, as a closed hi-hat cuts off an open one. Each chain has sends into as many as six return chains of audio effects inside the rack, whose outputs go to the rack's own output or straight to the set's return tracks.

The pad view shows the 128 notes as pads, sixteen at a time. Dropping a sample on a pad makes a chain on that note with Beatlayer's sampler playing it; dropping effects adds them to the pad's chain, and another sample replaces only the sound. Dragging one pad onto another swaps their notes. Each pad has mute, solo, preview and hot-swap, and D switches hot-swap between the rack and its last selected pad. A drum chain can be pulled out of the rack onto a track of its own, with its notes when it's dragged from the mixer.

This spec covers the drum rack's notes and chains, choke groups, previews, sends and return chains, the pad view, dropping on pads, hot-swapping pads and their samples, Group to Drum Rack, and extracting drum and return chains. Chains, their mixers and the chain list follow [racks](racks.md), and each chain's devices follow [device chains](device-chains.md).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 25 "Instrument, Drum and Effect Racks", sections 25.1.1, 25.2, 25.4 and 25.4.1 | A drum rack's chain takes only the note assigned to it; a drum rack's chains keep an instrument rack's order and it can hold up to six return chains of audio effects fed by sends; Group to Drum Rack sits beside Group in a device title bar's menu; drum chains have sends and MIDI assignment besides volume and pan; with Auto Select on, a drum chain is selected when it receives its note |
| S2 | Manual | Same chapter, section 25.6 "Drum Racks" | Four extra switches in the view column, top to bottom: Input/Output, Send, Return, Auto Select. Receive picks the incoming note a chain answers, listed by note name, MIDI note number and General MIDI drum sound; Play sets the note the chain's devices get; Choke puts the chain in one of sixteen choke groups, where triggering one chain silences the others in its group; All Notes in Receive passes every note as it comes and disables Play and Choke; a Preview button left of the choosers fires a note into the chain. Each drum chain has, besides the usual mixer and Hot-Swap, a post-fader send to each return chain, unavailable until return chains exist. Up to six return chains of audio effects sit at the bottom of the chain list; each one's Audio To sends it to the rack's main output or straight to the set's return tracks |
| S3 | Manual | Same chapter, section 25.6.1 "Pad View" | One pad for each of the 128 MIDI notes. The overview at the left moves the visible pads in groups of 16, by dragging its selector or with Up and Down; with Cmd (Alt on Windows) held it moves by single rows. A sample, effect, instrument or preset dropped on a pad goes to that pad's note, making or changing chains as needed: a sample on an empty pad makes a chain with a sampler playing it; an effect dropped after goes after the sampler; a new sample replaces only the sampler and sample, keeping the effects before and after it. Several samples dropped on a pad go to new chains rising chromatically from it, replacing samples there and keeping effects; with Cmd (Alt) held they layer on that one pad in a nested instrument rack. Dragging a pad onto another swaps their notes, so clips play the other sound; with Cmd (Alt) it layers both pads' chains in a nested instrument rack. Changing a Receive note moves the chain on the pads, and a pad with several chains on its note plays them all. An empty pad shows its note, and hovering it shows the note and its General MIDI sound in the Status Bar; a pad of one chain shows the chain's name and has mute, solo, preview and hot-swap, and can be renamed and deleted; a pad of several reads Multi, its mute, solo and preview act on all its chains with mixed states shown, hot-swap and renaming are off, and deleting deletes them all. In hot-swap, D switches the target between the drum rack and the last selected pad |
| S4 | Manual | Same chapter, section 25.5 "Zones" | Drum racks have no zones: their chains are filtered by the Receive choosers |
| S5 | Manual | Same chapter, section 25.8.1 "Extracting Chains" | Chains drag out of a rack, from the chain list or the Session mixer. A drum chain dragged from the mixer to a new track takes its MIDI notes with its devices; dragged from the chain list, only its devices. Return chains drag out too, and dragged to the mixer they make new return tracks |
| S6 | Manual | Chapter 23 "Working with Instruments and Effects", sections 23.2.4.1 "Hot-Swapping Samples" and 23.2.6.4 "Sample Dropping Defaults" | Each drum pad shows a Hot-Swap Sample button at its bottom right while the pointer is over it; clicking it opens the sample's folder in the browser to swap another in, and the button again, the hot-swap bar's close button, Esc, Q, or going to another view ends it. Save as Default Pad, in a pad's context menu, saves a sample-dropping default; the one saved last is used |
| S7 | Manual | Chapter 17 "Routing and I/O", section 17.5.1.1 "Routing Points in Racks"; chapter 42 "Live Keyboard Shortcuts", section 42.1 | A drum rack offers its return chains' Pre FX, Post FX and Post Mixer as routing points; D switches between a drum rack and its last selected pad |
| S8 | Reference | [layout.md](../ui-parity/layout.md) L-DEV-008; [menus.md](../ui-parity/menus.md) C-DEVICE-013; [shortcuts.toml](../ui-parity/shortcuts.toml) S-VIEW-007 | The drum rack's pad view among a rack's parts; Group to Drum Rack in a device title bar's menu with several devices selected; D's binding |
| S9 | Beatlayer | [Racks](racks.md) R1, R5, R9, R10, R16, R18 to R24, R38 and R42 to R45; [sampler](sampler.md) R2 and R11; [mixer](mixer.md) R7, R11 and R18; [routing](routing.md) R7, R14 and R16; [MIDI I/O](midi-io.md) R19 and R20; [delay compensation](delay-compensation.md) R2 | What every rack has; the sampler plays its sample at its own pitch at C3 and loads samples by content hash; sends start at −∞ dB and glide; returns under Solo in Place; routing order and labels; Live's octave numbering (C3 is note 60) and the computer keyboard's velocity of 100 |
| S10 | Beatlayer | [The improvements](../05-improvements.md), rules G1 and G2; [the architecture](../02-architecture.md), D17 and "Real-time rules" | Every gain reaches −∞ dB; every parameter is automatable and modulatable; every action is a registered command whose effect reads back; nothing on the audio thread allocates, locks or blocks |

## Rules

**Notes, chains and choke groups**

- **R1.** A drum rack is an instrument rack whose chains are chosen by note: each chain has a Receive note (a MIDI note from 0 to 127, or All Notes), a Play note (0 to 127) and a Choke group (None, or 1 to 16), besides the chain mixer every rack's chain has ([racks](racks.md) R5) and its sends (R6). A drum rack has no zones. (S1, S2, S4)
- **R2.** A note-on reaches every chain whose Receive is its pitch, and every chain on All Notes. A chain receiving one note gets it at its Play note's pitch, with its velocity; a chain on All Notes gets it as it came, and its Play and Choke are shown disabled. A note no chain receives sounds nothing. A note-off reaches the chains its note-on reached, at the pitch each got, whatever Receive and Play say by then, and messages other than notes reach every chain (both Beatlayer's own). (S1, S2, S3)
- **R3.** The Receive chooser lists All Notes first (Beatlayer's own place), then each note by its name in Live's octave numbering (C3 is note 60: [MIDI I/O](midi-io.md) R19), its number, and its General MIDI drum sound where it has one. A chain the rack makes for a pad receives that pad's note and plays C3, so a sampler in it plays its sample at its own pitch ([sampler](sampler.md) R2), and is in no choke group (Beatlayer's own defaults). (S2, S9)
- **R4.** A note-on that reaches a chain in a choke group silences every other chain in that group: their output fades to silence over 5 ms (the sampler's steal fade, Beatlayer's own) and stays silent until a note reaches them again. A chain doesn't choke itself, and a chain in no group chokes nothing and isn't choked. (S2)
- **R5.** A chain's Preview button, left of its choosers in the chain list, and a pad's (R12) play the chain's Receive note into that chain alone, or into each chain of a Multi pad, at velocity 100 (the computer keyboard's, [MIDI I/O](midi-io.md) R20; Beatlayer's own), for as long as the button is held; a chain on All Notes previews C3 (Beatlayer's own). The note goes on through the rest of the track's chain and its mixer, whether or not the song plays. Previewing changes nothing in the set and isn't an undo step. (S2, S3, S9)

**Sends and return chains**

- **R6.** While the rack has return chains, each drum chain has one send to each, taken after the chain's Volume, Pan and activator (post-fader). A send's level reaches −∞ dB (rule G1), starts there, glides as a track's send does ([mixer](mixer.md) R7, R18), and is an automation target that can be modulated (rule G2). With no return chains, the Send section shows no sends. (S2, S9, S10)
- **R7.** A drum rack holds up to six return chains, in a section of their own at the bottom of its chain list; a seventh is refused. Each holds audio effects only, takes the sum of every drum chain's send to it, and has the chain mixer of [racks](racks.md) R5 (Volume, Pan, activator and Solo) but no sends. A return chain is made by dropping an audio effect or its preset on the return section's drop area (Beatlayer's own). (S1, S2)
- **R8.** A return chain's Audio To sends its output into the rack's own output (in a new return chain; Beatlayer's own default) or into one of the set's return tracks, listed by letter and name as [routing](routing.md) R16 labels them. Sent to a return track, its output adds to that return's input as a send does, and the return runs after the drum rack's track ([routing](routing.md) R14). Deleting the return track sends the return chain back to the rack's output, and undo restores both (Beatlayer's own, after [routing](routing.md) R7). (S2, S9)
- **R9.** While any drum chain is soloed, the return chains stay audible carrying only the soloed chains' sends, as returns do under Solo in Place ([mixer](mixer.md) R11; Beatlayer's own for return chains). (S9)
- **R10.** Each return chain offers Pre FX, Post FX and Post Mixer to other tracks' inputs, as an instrument rack's chains do ([racks](racks.md) R38 to R41). (S7, S9)

**The pad view**

- **R11.** The pad view has one pad per MIDI note, 128 in all, sixteen of them showing at a time in four rows of four, the notes rising left to right and from the bottom row up; a new drum rack shows notes 36 (C1) to 51, where General MIDI's drum sounds begin (both Beatlayer's own). The overview at its left shows all 128 as eight groups of sixteen; dragging its selector, or Up and Down with the pad view focused, moves the view sixteen notes, and with Cmd held (Alt on Windows and Linux) one row. Which notes show is view state, kept with the set and no undo step (Beatlayer's own). The pad view sits between the rack's Macro Controls and its chain list (Beatlayer's own place). (S3)
- **R12.** An empty pad shows only its note's name, and hovering it shows the note and its General MIDI sound in the Status Bar. A pad of one chain shows the chain's name, with mute, solo, preview and hot-swap buttons, and the chain can be renamed or deleted from it. A pad of several chains reads Multi: its mute, solo and preview act on all its chains, each button showing a mixed state while they differ; its hot-swap and rename are disabled, and deleting it deletes them all. A pad's mute is its chains' activators, lit while they're off, and its solo is their Solo (Beatlayer's own mapping). (S3)
- **R13.** Clicking a pad selects it, and selects its chains in the chain list so the devices view shows them (Beatlayer's own). Changing a chain's Receive moves it to its new note's pad at once, and a pad shows every chain receiving its note. (S3)

**Dropping on pads**

- **R14.** A sample dropped on an empty pad makes a new chain on its note holding Beatlayer's sampler with the sample loaded ([sampler](sampler.md) R11), or a chain made from the pad default (R17) with the sample in its first sampler. A sample dropped on a pad with a chain replaces that chain's instrument with a sampler playing it, keeping its MIDI and audio effects. An audio effect dropped on a pad goes at the end of its chain, and a MIDI effect at the end of its MIDI effects; an instrument, an instrument preset or an instrument or drum rack replaces the chain's instrument, keeping its effects; on an empty pad, each of these makes a chain holding it (Beatlayer's own for what isn't a sample or an audio effect). Each drop is one undo step. (S3)
- **R15.** Several samples dropped on a pad go one to a pad, rising chromatically from it, each placed as R14 places one sample; samples that would go past note 127 are left out (Beatlayer's own). With Cmd held (Alt on Windows and Linux), they go onto that one pad instead, layered: one chain holding an instrument rack with a chain per sample, each with a sampler. Each drop is one undo step. (S3)
- **R16.** Dragging a pad onto another swaps their notes: each pad's chains take the other pad's note as their Receive, so the set's clips play the other sound. With Cmd held (Alt on Windows and Linux), both pads' chains are layered on the target pad as one chain holding an instrument rack of them, and the dragged pad is left empty (Beatlayer's own). Each drag is one undo step. (S3)
- **R17.** Save as Default Pad, in a pad's context menu, saves the pad's chain as the sample-dropping default, replacing the one before; a sample dropped on an empty pad then makes its chain from that default (R14). (S6)

**Hot-swapping pads and samples**

- **R18.** While hot-swap is on, D (S-VIEW-007) switches its target between the drum rack and the rack's last selected pad. With a pad the target, an item loaded from the browser lands on that pad as R14 places a drop; a pad's own hot-swap button makes it the target. (S3, S7, S8)
- **R19.** A pad whose chain plays a sample shows a Hot-Swap Sample button at its bottom right while the pointer is over it. Clicking it opens the sample's folder in the browser, and loading a sample replaces the pad's sample, one undo step for each; the button again, the hot-swap bar's close button, Esc, Q, or going from the browser to another view ends it. (S6)

**The drum rack's chain list**

- **R20.** With Auto Select on, the chain list selects each chain as a note reaches it, and the devices view follows; the selection isn't an undo step. (S1)
- **R21.** A drum rack's view column has, besides the views every rack has ([racks](racks.md) R18), the pad view's selector and four switches, from the top: Input/Output, Send, Return and Auto Select. Input/Output shows each chain's Preview button, Receive, Play and Choke in its row; Send shows its send sliders; Return shows the return chains' section. (S2)
- **R22.** A drum rack's sum is a summing point for delay compensation: its chains' outputs, its return chains' outputs, and every send into a return chain are held back to line up ([delay compensation](delay-compensation.md) R2, [racks](racks.md) R16). (S9)

**Extracting chains**

- **R23.** A drum chain dragged from the chain list onto another track or rack takes only its devices there ([racks](racks.md) R24). Dragged from the Session mixer onto the drop area right of the tracks, it leaves the rack and makes a new MIDI track holding its devices, and the notes its chain received leave the track's clips for clips on the new track, at the same places and slots, moved to its Play note so its devices hear what they heard in the rack (Beatlayer's own detail). One undo step. (S5)
- **R24.** A return chain dragged onto the Session mixer's return tracks leaves the rack and makes a new return track holding its devices, after the last return; the drum chains' sends to it go with it from the rack (Beatlayer's own). One undo step. (S5)

**Group to Drum Rack and grouping drum chains**

- **R25.** Group to Drum Rack (C-DEVICE-013), with several devices of one chain on a MIDI track selected, replaces them with a new drum rack holding them as one chain that receives note 36 (C1, the first pad a new drum rack shows: R11) and plays C3 (Beatlayer's own notes). It's offered wherever the selection could sit in one chain of an instrument rack ([racks](racks.md) R9), and is one undo step. (S1, S8)
- **R26.** Grouping selected chains of a drum rack ([racks](racks.md) R10) replaces them with one drum chain holding an instrument rack of them; the new chain takes the first chain's Receive, Play and Choke, and each grouped chain keeps its devices and mixer (Beatlayer's own). (S1, S9)

**Saving, commands and the engine**

- **R27.** A drum rack's chains' Receive, Play, Choke and sends, its return chains with their Audio To, and the pad view's place are saved with the set and in its presets ([racks](racks.md) R42). Every edit is one undo step, and every action is a registered command (D17) that reads back through the query tool's `device` object: each chain's `receive`, `play`, `choke` and `sends`, and the rack's `returns`. (S9, S10)
- **R28.** Notes reach their chains, chokes start and previews sound on the exact frames they fall on, the same at any block size, and nothing on the audio thread allocates, locks or blocks: the note routing, the choke state and the return chains' buffers are made with the program. (S10)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/model/tests/drum_racks.rs`, `drum_chains_have_receive_play_and_choke_and_no_zones` | unit |
| R2 | `crates/engine/tests/drum_racks.rs`, `a_note_reaches_its_chains_at_their_play_note`, `all_notes_passes_every_note_as_it_came`, `a_note_off_follows_its_note_on` | engine test under the allocation trap |
| R3 | `crates/shell/tests/drum_racks.rs`, `the_receive_chooser_lists_names_numbers_and_general_midi`, `a_pads_new_chain_receives_its_note_and_plays_c3` | unit |
| R4 | `crates/engine/tests/drum_racks.rs`, `a_choke_group_fades_the_others_within_five_ms` | engine test under the allocation trap |
| R5 | `crates/shell/tests/drum_racks.rs`, `preview_plays_one_chain_while_held_and_changes_nothing` | unit |
| R6 | `crates/engine/tests/drum_racks.rs`, `sends_are_post_fader_and_start_silent`; `crates/model/tests/drum_racks.rs`, `sends_are_automation_targets` | engine test under the allocation trap; unit |
| R7 | `crates/model/tests/drum_racks.rs`, `six_return_chains_and_no_seventh`; `crates/engine/tests/drum_racks.rs`, `a_return_chain_sums_its_sends` | unit; engine test under the allocation trap |
| R8 | `crates/engine/tests/drum_racks.rs`, `a_return_chain_feeds_the_rack_or_a_return_track`; `crates/shell/tests/drum_racks.rs`, `deleting_the_return_track_sends_the_chain_home` | engine test under the allocation trap; unit |
| R9 | `crates/engine/tests/drum_racks.rs`, `soloed_chains_keep_their_returns` | engine test under the allocation trap |
| R10 | `crates/shell/tests/routing.rs`, `a_drum_rack_offers_its_return_chains_taps` | unit |
| R11 | `crates/ui/tests/drum_racks.rs`, `the_pad_view_shows_sixteen_pads_and_moves_by_group_or_row` | UI script |
| R12 | `crates/ui/tests/drum_racks.rs`, `empty_single_and_multi_pads_show_and_offer_what_they_should` | UI script |
| R13 | `crates/ui/tests/drum_racks.rs`, `a_pad_selects_its_chains_and_follows_receive` | UI script |
| R14 | `crates/shell/tests/drum_racks.rs`, `a_sample_on_a_pad_makes_or_replaces_the_sound_and_keeps_effects` | unit |
| R15 | `crates/shell/tests/drum_racks.rs`, `several_samples_rise_chromatically_or_layer_with_cmd` | unit |
| R16 | `crates/shell/tests/drum_racks.rs`, `dragging_a_pad_swaps_notes_or_layers_with_cmd` | unit |
| R17 | `crates/shell/tests/drum_racks.rs`, `save_as_default_pad_shapes_the_next_dropped_sample` | unit |
| R18 | `crates/ui/tests/keymap.rs`, `d_switches_hot_swap_between_the_rack_and_its_pad`; `crates/shell/tests/drum_racks.rs`, `a_hot_swapped_pad_takes_what_the_browser_loads` | keymap test; unit |
| R19 | `crates/ui/tests/drum_racks.rs`, `hot_swap_sample_shows_on_hover_and_swaps_the_sample` | UI script |
| R20 | `crates/ui/tests/drum_racks.rs`, `auto_select_selects_each_chain_its_note_reaches` | UI script |
| R21 | `crates/ui/tests/drum_racks.rs`, `the_view_column_shows_input_output_send_and_return` | UI script |
| R22 | `crates/engine/tests/pdc.rs`, `a_drum_racks_chains_returns_and_sends_line_up` | golden render |
| R23 | `crates/shell/tests/drum_racks.rs`, `a_chain_from_the_list_takes_its_devices_and_from_the_mixer_its_notes` | unit |
| R24 | `crates/shell/tests/drum_racks.rs`, `a_return_chain_dragged_to_the_mixer_makes_a_return_track` | unit |
| R25 | `crates/shell/tests/drum_racks.rs`, `group_to_drum_rack_puts_the_selection_on_c1` | unit |
| R26 | `crates/shell/tests/drum_racks.rs`, `grouping_drum_chains_layers_them_on_the_first_ones_note` | unit |
| R27 | `crates/model/src/repr.rs`, `drum_racks_save_and_load_with_returns`; `crates/cli/tests/agent.rs`, `an_agent_builds_a_kit_and_reads_it_back` | unit; agent test |
| R28 | `crates/engine/tests/drum_racks.rs`, `a_kit_renders_the_same_at_every_block_size`; `rtsan/tests/drum_racks.rs` | golden render; RealtimeSanitizer |
