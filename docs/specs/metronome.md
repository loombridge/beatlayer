# The metronome: switch, settings, click, and count-in

- **Status:** partial (R1 to R9 each have a passing test; R10 is WP-7.14's)
- **Work packages:** WP-2.15 (R1 to R9); WP-7.14 (the count-in under Link, R10)
- **Parity-matrix row:** Views and workflow: Control Bar; Recording
- **Checked against:** the Live 12 reference manual, read 15 October 2026 (chapter 19), and Live 12.4.3 on macOS, 13 October 2026 (the Control Bar's metronome).

## The behaviour

The Control Bar has a Metronome switch, and a small pop-up arrow beside it opens the Metronome Settings menu (the metronome's context menu opens the same menu). The menu sets the Count-In length, the tick's Sound, the Rhythm at which it ticks, and whether the metronome is heard only while recording. Turning the switch on makes the metronome click on the beats of the song as it plays; the switch is saved with the set.

This spec covers the metronome switch and its settings, the clicks the engine plays and their timing, and the count-in before the transport moves. It leaves to other packages: the metronome's volume (the mixer's Preview Volume, WP-1.6), the Arrangement Position fields showing the count-in in blue as negative bars-beats-sixteenths (WP-1.18's fields, a display change), recording itself (WP-2.13), and clip launches starting the tick (the Session View, Phase 3).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Observation | Live 12.4.3 on macOS, 13 October 2026 | The Metronome Settings menu has three groups and one switch, each a single choice with a tick: Count-In (None default, then 1 Bar, 2 Bars, 4 Bars); Sound (Classic default, Click, Wood); Rhythm (Auto default, then 1 Bar, 1/2, 1/2T, 1/4, 1/4T, 1/8, 1/8T, 1/16, 1/16T); and Enable Only While Recording, a switch off by default. What Auto does, and how each sound and rhythm sounds, wasn't checked (listening). |
| S2 | Manual | Live 12 reference manual, chapter 19 "Recording New Clips", section 19.4 "Recording in Sync" and 19.4.1 "Metronome Settings" | The metronome is switched on in the Control Bar and ticks once the transport runs; its volume is the mixer's Preview Volume, and its pull-down menu (or its context menu) sets the count-in and the tick's sound. Rhythm sets the beat division the tick follows: with Auto it follows the time signature's denominator; divisions that don't fit one bar of the current signature show disabled; a division that stops fitting after a time-signature change reverts to Auto, and returns to the chosen division if a later change makes it fit again. With Enable Only While Recording checked and the metronome on, the metronome is highlighted while the transport runs but audible only while recording. |
| S3 | Spec | [transport.md](transport.md) R15 | A musical position falls on a frame through the tempo map, so playback and its events follow the map, including inside a tempo change. |
| S4 | Manual | The same manual, section 19.6 "Recording with Count-in" | The count-in is set in the pull-down menu next to the Metronome switch. With any value other than None, Live doesn't begin recording until the count-in is complete; the Control Bar's Arrangement Position fields show it in blue as bars-beats-sixteenths, running from negative bars-beats-sixteenths (for example −2.1.1 with 2 bars) up to 1.1.1, where recording starts. |
| S5 | Decision | D17 in the architecture | Every user-facing action is a registered command with a schema, a description, and a permission class, reachable through the agent API. |
| S6 | Spec | [transport.md](transport.md) R20 and the architecture's real-time rules | The engine publishes transport state without a lock, and its audio callback allocates nothing, takes no lock, and blocks on nothing. |
| S7 | Manual | The Live 12 reference manual (online edition, read 23 October 2026), chapter 37, section 37.1.2 "Using Link"; [Link](link.md) R1 and R7 | The metronome's recording count-in can't be used while Link is on; with peers, a start waits for the session's quantum |

## Rules

- **R1.** The Control Bar's Metronome switch runs `transport.set_metronome_enabled` and shows whether the metronome is on; the Playback menu's Metronome item (key O) runs it too. The switch is part of the set, so switching it is one undo step and survives saving and reopening. (S1, S2, S5)
- **R2.** With the metronome on, a click starts on the exact frame of each beat the Rhythm setting gives, following the tempo map and the time signatures: the click's frame is the frame the map gives its musical position, at any block size, at any tempo, and across a tempo change in the map. (S2, S3)
- **R3.** A bar's first beat is accented: its click is louder and higher than the beats after it. (S1, S2)
- **R4.** Rhythm Auto clicks once per unit of the time signature's denominator — a quarter note in 4/4, an eighth note in 6/8. Every other choice clicks at its own division (1 Bar, 1/2, 1/2T, 1/4, 1/4T, 1/8, 1/8T, 1/16, 1/16T). A division that doesn't fit one bar of the current time signature is shown disabled in the menu and its clicks fall back to Auto's; if a later time-signature change makes it fit again, its own clicks return. (S1, S2)
- **R5.** Count-In (None default, 1 Bar, 2 Bars, or 4 Bars) plays that many bars of beat clicks before the transport moves. One click sounds on each beat of the chosen bars, the first beat of each bar accented; a count-in plays when the metronome is on or the transport is recording; the transport stays at the insert marker for the count-in's frames and then plays from it. (S1, S2, S4)
- **R6.** Sound (Classic default, Click, or Wood) chooses the click's character. The three are Beatlayer's own, synthesized in code, with no sample content. (S1, S2)
- **R7.** Enable Only While Recording (off by default): with the switch on and the metronome on, clicks sound only while the transport is recording, and stop when the recording does, however the take ended (WP-2.13). With Punch-In on, they start at the punch-in point, not at play start ([recording](recording.md) R9; the Live 12 manual, 19.4.1). (S1, S2)
- **R8.** Every metronome setting is a registered command an agent can run — `transport.set_metronome_enabled`, `transport.set_metronome_count_in`, `transport.set_metronome_sound`, `transport.set_metronome_rhythm`, `transport.set_metronome_only_while_recording` — and each is saved with the set and reversed by undo. (S1, S5)
- **R9.** The metronome renders on the audio thread without allocating, taking a lock, or blocking. (S6)
- **R10.** While Link is on ([Link](link.md) R1), no count-in plays: playback and recording start as they would with Count-In set to None, a start with peers waiting for the session's quantum instead ([Link](link.md) R7). The Count-In setting is kept in the set, its menu items stay enabled with the chosen one ticked, and it plays again once Link is off. `transport.state` reports `counting_in` false throughout. (S7; keeping the setting and the items enabled are Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/ui/tests/transport.rs`, `the_section_reaches_assistive_technology`; `crates/model/tests/command_registry.rs`, `every_command_runs_by_id` | UI script, unit |
| R2 | `crates/engine/tests/metronome.rs`, `clicks_land_on_each_beats_exact_sample_with_the_bar_accented`, `clicks_follow_a_tempo_change_in_the_map` | unit (frame by frame, under the allocation trap) |
| R3 | `crates/engine/tests/metronome.rs`, `clicks_land_on_each_beats_exact_sample_with_the_bar_accented` | unit |
| R4 | `crates/engine/tests/metronome.rs`, `each_rhythm_clicks_its_division_of_the_bar`, `auto_follows_the_denominator_and_a_division_that_doesnt_fit_reverts`; `crates/model/tests/metronome.rs`, `auto_follows_the_denominator_and_divisions_fit_the_bar` | unit |
| R5 | `crates/engine/tests/metronome.rs`, `count_in_plays_its_bars_before_the_transport_moves` | unit |
| R6 | `crates/engine/tests/metronome.rs`, `synthesized_sound_modes_have_distinct_characteristics` | unit |
| R7 | `crates/engine/tests/metronome.rs`, `only_while_recording_silences_playback_unless_recording`, `a_locate_that_ends_the_take_ends_the_recording_only_clicks`, `with_punch_in_the_recording_only_clicks_start_at_the_punch_in_point` | unit |
| R8 | `crates/model/tests/metronome.rs`, `metronome_commands_and_undo_redo`, `metronome_set_roundtrip`, `metronome_registered_commands`; `crates/agent/tests/tools.rs`, `the_metronome_settings_run_through_the_agent_api`; `crates/shell/tests/transport.rs`, `the_shell_registers_the_transport_and_runs_it_on_the_engine` | unit, agent API |
| R9 | `crates/engine/tests/metronome.rs` (every block runs under `bl_rt::install_trap!()`) | unit, real-time (allocation trap) |
| R10 | `crates/engine/tests/link.rs`, `no_count_in_plays_while_link_is_on` | engine test |
