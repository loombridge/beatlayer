//! What the map modes can map across the views (WP-7.3b;
//! `docs/specs/map-modes.md`, R11 to R24): the relative Session mapping
//! strip, New, the crossfader's fields, the step buttons, the Control Bar,
//! the Clip View's controls and a rack's radio-button macros, each mapped
//! and driven by the remote input, and `remote.targets` listing them.

use std::path::{Path, PathBuf};

use bl_audio_io::NullControl;
use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Editor, MidiData, Note, SceneId,
    TrackId, TrackKind,
};
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError, testing};
use serde_json::{Value, json};

struct Rig {
    shell: Shell,
    control: NullControl,
}

impl Rig {
    fn new() -> Self {
        let (mut shell, control) = testing::rig();
        shell.start_audio();
        let mut rig = Self { shell, control };
        rig.run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        );
        rig
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn fails(&mut self, id: &str, args: Value) -> ShellError {
        self.shell.run(id, args).expect_err(id)
    }

    fn scenes(&self) -> Vec<SceneId> {
        self.shell
            .editor()
            .document()
            .scenes()
            .map(|scene| scene.id)
            .collect()
    }

    fn track(&self, kind: TrackKind) -> TrackId {
        self.shell
            .editor()
            .document()
            .tracks()
            .find(|track| track.kind() == kind)
            .expect("the default set has the track")
            .id()
    }

    fn map(&mut self, source: Value, target: Value) -> Value {
        self.run("remote.map", json!({ "source": source, "target": target }))
    }

    fn key(&mut self, key: &str) {
        self.run("remote.input", json!({ "key": key }));
    }

    fn press(&mut self, key: &str) {
        self.run("remote.input", json!({ "key": key, "pressed": true }));
    }

    fn release(&mut self, key: &str) {
        self.run("remote.input", json!({ "key": key, "pressed": false }));
    }

    fn cc(&mut self, controller: u8, value: u8) {
        self.run(
            "remote.input",
            json!({ "bytes": [0xB0, controller, value] }),
        );
    }

    fn highlighted(&self) -> Option<SceneId> {
        self.shell.highlighted_scene()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().count()
    }

    fn refused(&self) -> u64 {
        self.shell.remote_counts().1
    }
}

fn key(name: &str) -> Value {
    json!({ "type": "key", "key": name })
}

fn cc(controller: u8) -> Value {
    json!({ "type": "cc", "channel": 1, "controller": controller })
}

fn strip(control: &str) -> Value {
    json!({ "type": "strip", "control": control })
}

fn bar(control: &str) -> Value {
    json!({ "type": "bar", "control": control })
}

// Spec: map-modes R14, R15, R16. Scene Down and Scene Up highlight the
// scene after or before, stopping at the ends, the first with none
// highlighted; the scene number box highlights the scene of its number; each
// asks the grid to centre the scene.
#[test]
fn the_strip_highlights_scenes_stops_at_the_ends_and_asks_the_grid_to_centre() {
    let mut rig = Rig::new();
    let scenes = rig.scenes();
    assert!(scenes.len() >= 5);
    rig.map(key("d"), strip("scene_down"));
    rig.map(key("u"), strip("scene_up"));
    rig.map(cc(20), strip("scene_number"));

    assert_eq!(rig.highlighted(), None);
    let (asked, _) = rig.shell.session().centre_request();
    rig.key("d");
    assert_eq!(rig.highlighted(), Some(scenes[0]), "none highlighted: the first");
    rig.key("d");
    assert_eq!(rig.highlighted(), Some(scenes[1]));
    rig.key("u");
    rig.key("u");
    assert_eq!(rig.highlighted(), Some(scenes[0]), "Scene Up stops at the first");
    let (now, centred) = rig.shell.session().centre_request();
    assert_eq!((now, centred), (asked + 4, Some(scenes[0])));

    rig.cc(20, 3);
    assert_eq!(rig.highlighted(), Some(scenes[3]), "the controller value plus one");
    rig.cc(20, 127);
    assert_eq!(
        rig.highlighted(),
        scenes.last().copied(),
        "a number past the last stops at the last"
    );
    assert_eq!(rig.refused(), 0);

    // The commands answer the same, and a click selecting a scene asks for
    // no centring.
    let result = rig.run("session.highlight_scene", json!({ "number": 2 }));
    assert_eq!(result["number"], 2);
    assert!(matches!(
        rig.fails("session.highlight_scene", json!({ "number": 0 })),
        ShellError::InvalidArgs { .. }
    ));
    assert!(matches!(
        rig.fails("session.highlight_scene", json!({})),
        ShellError::InvalidArgs { .. }
    ));
    let (asked, _) = rig.shell.session().centre_request();
    rig.run("session.select", json!({ "scene": scenes[4] }));
    assert_eq!(rig.shell.session().centre_request().0, asked);
}

// Spec: map-modes R15; launch-quantisation R16. Launch Scene launches the
// highlighted scene and Select Next Scene on Launch moves the highlight on;
// a track's Launch button presses the slot at the highlight.
#[test]
fn launch_scene_and_a_tracks_launch_button_act_at_the_highlight() {
    let mut rig = Rig::new();
    let track = rig.track(TrackKind::Midi);
    let scenes = rig.scenes();
    for scene in &scenes[..2] {
        rig.run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
    }
    rig.map(key("l"), strip("launch_scene"));
    rig.map(key("t"), json!({ "type": "track_launch", "track": track }));
    let playing = |shell: &Shell, scene: SceneId| {
        shell.session_states().iter().any(|slot| {
            slot.track == track && slot.scene == scene && slot.state == bl_engine::SlotState::Playing
        })
    };

    assert!(matches!(
        rig.fails("session.launch_highlighted_scene", json!({})),
        ShellError::Unavailable { .. }
    ));
    rig.run("session.highlight_scene", json!({ "number": 1 }));
    rig.key("l");
    testing::wait_for(&rig.control, &mut rig.shell, "the first scene", |shell| {
        playing(shell, scenes[0])
    });
    assert_eq!(rig.highlighted(), Some(scenes[1]), "the highlight moved on");

    rig.key("t");
    testing::wait_for(&rig.control, &mut rig.shell, "the second slot", |shell| {
        playing(shell, scenes[1])
    });
    assert_eq!(rig.refused(), 0);
}

// Spec: map-modes R15; session-context-menus R10. Cancel Launch drops the
// scene launch that is queued and leaves the clips that play.
#[test]
fn cancel_launch_drops_a_queued_scene_launch_and_leaves_what_plays() {
    let mut rig = Rig::new();
    let track = rig.track(TrackKind::Midi);
    let scenes = rig.scenes();
    for scene in &scenes[..2] {
        rig.run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
    }
    rig.map(key("c"), strip("cancel_launch"));
    rig.run("session.launch_scene", json!({ "scene": scenes[0] }));
    testing::wait_for(&rig.control, &mut rig.shell, "the first scene", |shell| {
        shell
            .session_states()
            .iter()
            .any(|slot| slot.state == bl_engine::SlotState::Playing)
    });
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "8_bars" }),
    );
    rig.run("session.launch_scene", json!({ "scene": scenes[1] }));
    testing::wait_for(&rig.control, &mut rig.shell, "the queued launch", |shell| {
        shell
            .session_states()
            .iter()
            .any(|slot| slot.state == bl_engine::SlotState::Queued)
    });
    rig.key("c");
    testing::wait_for(&rig.control, &mut rig.shell, "the launch dropped", |shell| {
        shell
            .session_states()
            .iter()
            .all(|slot| slot.state != bl_engine::SlotState::Queued)
    });
    assert!(
        rig.shell
            .session_states()
            .iter()
            .any(|slot| slot.scene == scenes[0] && slot.state == bl_engine::SlotState::Playing),
        "the playing clip plays on"
    );
    assert_eq!(rig.refused(), 0);
    // With nothing queued the button is refused and counted.
    rig.key("c");
    assert_eq!(rig.refused(), 1);
}

// Spec: map-modes R20. New stops the armed tracks' clips and highlights the
// first scene after the highlighted one whose armed slots are empty,
// inserting a scene after the last when there is none, as one undo step.
#[test]
fn new_stops_the_armed_tracks_and_highlights_or_inserts_a_scene() {
    let mut rig = Rig::new();
    let track = rig.track(TrackKind::Midi);
    let scenes = rig.scenes();
    for scene in &scenes[..2] {
        rig.run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
    }
    rig.run("mixer.set_arm", json!({ "track": track, "arm": true }));
    rig.map(key("n"), bar("new"));
    rig.run("session.launch", json!({ "track": track, "scene": scenes[0] }));
    testing::wait_for(&rig.control, &mut rig.shell, "the clip", |shell| {
        shell
            .session_states()
            .iter()
            .any(|slot| slot.state == bl_engine::SlotState::Playing)
    });
    rig.run("session.highlight_scene", json!({ "number": 1 }));

    let steps = rig.steps();
    rig.key("n");
    assert_eq!(rig.highlighted(), Some(scenes[2]), "the first scene with free slots");
    assert_eq!(rig.steps(), steps, "highlighting is no undo step");
    testing::wait_for(&rig.control, &mut rig.shell, "the clip stopped", |shell| {
        shell
            .session_states()
            .iter()
            .all(|slot| slot.state != bl_engine::SlotState::Playing)
    });

    // Fill every scene's slot: the next New inserts a scene after the last.
    for scene in &scenes[2..] {
        rig.run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
    }
    let steps = rig.steps();
    rig.key("n");
    let now = rig.scenes();
    assert_eq!(now.len(), scenes.len() + 1);
    assert_eq!(rig.highlighted(), now.last().copied());
    assert_eq!(rig.steps(), steps + 1, "inserting a scene is one undo step");
    assert_eq!(rig.refused(), 0);
}

// Spec: map-modes R19. With one field mapped each press toggles the
// crossfader between its ends; with two, a press moves it to the field's
// position and a release while the other is held snaps back to it.
#[test]
fn the_crossfaders_one_field_toggles_and_two_fields_snap_back() {
    let mut rig = Rig::new();
    let position = |rig: &Rig| rig.shell.editor().document().settings().crossfader;
    let field = |field: &str| json!({ "type": "crossfader_field", "field": field });
    rig.map(key("a"), field("left"));
    assert!((position(&rig) - 0.5).abs() < 1e-9);
    rig.key("a");
    assert!(position(&rig).abs() < 1e-9, "from the centre to the left end");
    rig.key("a");
    assert!((position(&rig) - 1.0).abs() < 1e-9, "from the left end to the right");
    rig.key("a");
    assert!(position(&rig).abs() < 1e-9);

    rig.map(key("b"), field("right"));
    rig.press("a");
    assert!(position(&rig).abs() < 1e-9);
    rig.press("b");
    assert!((position(&rig) - 1.0).abs() < 1e-9);
    rig.release("b");
    assert!(position(&rig).abs() < 1e-9, "back to the field still held");
    rig.release("a");
    assert!(position(&rig).abs() < 1e-9, "nothing held: it stays");
    assert_eq!(rig.refused(), 0);
}

// Spec: map-modes R21; step-recording R12. The step buttons run
// `note.step_record`, as Left and Right do: forward adds the held note at the
// insert marker, back shortens it again.
#[test]
fn the_step_buttons_step_record_with_a_held_note() {
    let (mut shell, clip) = shell_with_a_midi_clip();
    let run = |shell: &mut Shell, id: &str, args: Value| {
        shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    };
    for (source, target) in [("f", "step_forward"), ("b", "step_back")] {
        run(
            &mut shell,
            "remote.map",
            json!({ "source": key(source), "target": { "type": target } }),
        );
    }
    let notes = |shell: &Shell| match &shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => bl_shell::clip_view::notes_in_order(midi)
            .iter()
            .map(|n| (n.key, n.length.get()))
            .collect::<Vec<_>>(),
        ClipData::Audio(_) => unreachable!(),
    };
    let step = shell
        .arrangement()
        .grid_ticks(shell.editor().document())
        .get();

    // A on the computer MIDI keyboard is C3.
    run(
        &mut shell,
        "midi.computer_keyboard_key",
        json!({ "key": "A", "pressed": true }),
    );
    run(&mut shell, "remote.input", json!({ "key": "f" }));
    assert_eq!(notes(&shell), [(60, step)], "forward adds the held note");
    run(&mut shell, "remote.input", json!({ "key": "f" }));
    assert_eq!(notes(&shell), [(60, 2 * step)], "and lengthens it again");
    run(&mut shell, "remote.input", json!({ "key": "b" }));
    assert_eq!(notes(&shell), [(60, step)], "back shortens it");
    assert_eq!(shell.remote_counts().1, 0);
}

// Spec: map-modes R11. The Control Bar's controls take mappings: a switch
// follows its press and a controller, the Tempo field moves, the
// Quantization Menu is a radio button.
#[test]
fn the_control_bar_controls_take_mappings() {
    let mut rig = Rig::new();
    let settings = |rig: &Rig| rig.shell.editor().document().settings().clone();
    rig.map(key("m"), bar("metronome"));
    let was = settings(&rig).metronome.enabled;
    rig.key("m");
    assert_eq!(settings(&rig).metronome.enabled, !was);
    rig.key("m");
    assert_eq!(settings(&rig).metronome.enabled, was);

    rig.map(cc(21), bar("tempo"));
    rig.cc(21, 0);
    assert!((settings(&rig).tempo.bpm_at(Ticks::ZERO).get() - 20.0).abs() < 1e-6);
    rig.cc(21, 127);
    assert!((settings(&rig).tempo.bpm_at(Ticks::ZERO).get() - 999.0).abs() < 1e-6);

    rig.map(cc(22), bar("quantization"));
    rig.cc(22, 0);
    assert_eq!(
        rig.shell.launch_settings().quantization,
        bl_model::LaunchQuantization::ALL[0]
    );
    rig.cc(22, 127);
    assert_eq!(
        rig.shell.launch_settings().quantization,
        *bl_model::LaunchQuantization::ALL.last().unwrap()
    );

    rig.map(key("o"), bar("punch_in"));
    rig.key("o");
    assert!(settings(&rig).punch_in);
    assert_eq!(rig.refused(), 0);
}

// Spec: map-modes R11. Monitor and Cue Volume are mappable mixer controls.
#[test]
fn monitor_and_the_cue_volume_take_mappings() {
    let mut rig = Rig::new();
    let track = rig.track(TrackKind::Midi);
    rig.map(key("i"), json!({ "type": "monitor", "track": track }));
    let monitor = |rig: &Rig| {
        rig.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .routing()
            .monitor
    };
    let first = monitor(&rig);
    rig.key("i");
    assert_ne!(monitor(&rig), first, "a key steps to the next option");

    rig.map(cc(23), json!({ "type": "cue_volume" }));
    rig.cc(23, 0);
    assert!(rig.shell.editor().document().settings().cue_volume.abs() < 1e-9);
    rig.cc(23, 127);
    assert!(rig.shell.editor().document().settings().cue_volume > 1.9);
    assert_eq!(rig.refused(), 0);
}

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

/// A shell showing the factory default set with two audio clips on its third
/// track, both selected in the Arrangement.
fn shell_with_two_audio_clips() -> (Shell, [ClipId; 2]) {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().nth(2).unwrap();
    let mut ids = Vec::new();
    for at in 0..2_i64 {
        let id: ClipId = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    Ticks(at * Ticks::WHOLE.get() * 4),
                    Ticks(Ticks::WHOLE.get() * 4),
                    ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
                ),
            })
            .unwrap();
        ids.push(id);
    }
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Clips.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": ids }))
        .unwrap();
    (shell, [ids[0], ids[1]])
}

/// A shell showing the factory default set with an empty MIDI clip on its
/// first track, selected, the track armed, Preview on and the computer MIDI
/// keyboard on: what step recording needs.
fn shell_with_a_midi_clip() -> (Shell, ClipId) {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip: ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(Ticks::WHOLE.get() * 4),
                ClipData::Midi(MidiData::new(Vec::<Note>::new())),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell.use_test_clock(bl_core::clock::TestClock::new());
    for (id, args) in [
        ("set.open", json!({ "path": "Steps.blset" })),
        ("arrangement.select_clips", json!({ "clips": [clip] })),
        ("mixer.set_arm", json!({ "track": track, "arm": true })),
        ("clip_view.set_preview", json!({ "on": true })),
        ("midi.toggle_computer_keyboard", json!({ "on": true })),
        (
            "clip_view.set_insert_marker",
            json!({ "clip": clip, "beat": 0 }),
        ),
    ] {
        shell.run(id, args).unwrap_or_else(|error| panic!("{id}: {error}"));
    }
    // The frame that turns the input's copy on.
    shell.poll();
    (shell, clip)
}

// Spec: map-modes R17. A mapped Clip View control acts on the clips the
// Clip View shows when the message arrives: Transpose moves both of two
// selected clips, in one undo step.
#[test]
fn a_mapped_clip_view_transpose_moves_both_selected_clips() {
    let (mut shell, clips) = shell_with_two_audio_clips();
    shell
        .run(
            "remote.map",
            json!({
                "source": cc(24),
                "target": { "type": "clip", "control": "transpose" },
            }),
        )
        .unwrap();
    let transpose = |shell: &Shell, clip: ClipId| match &shell
        .editor()
        .document()
        .clip(clip)
        .unwrap()
        .data
    {
        ClipData::Audio(audio) => audio.transpose,
        ClipData::Midi(_) => unreachable!(),
    };
    let steps = shell.editor().undo_history().count();
    shell
        .run("remote.input", json!({ "bytes": [0xB0, 24, 127] }))
        .unwrap();
    assert_eq!(transpose(&shell, clips[0]), 48);
    assert_eq!(transpose(&shell, clips[1]), 48);
    assert_eq!(
        shell.editor().undo_history().count(),
        steps + 1,
        "both clips change in one step"
    );
    // With no clip shown it does nothing and is counted.
    shell.run("arrangement.select_clips", json!({ "clips": [] })).unwrap();
    let refused = shell.remote_counts().1;
    shell
        .run("remote.input", json!({ "bytes": [0xB0, 24, 0] }))
        .unwrap();
    assert_eq!(transpose(&shell, clips[0]), 48);
    assert!(shell.remote_counts().1 > refused);
}

// Spec: macros R34, R35. A radio-button macro is a radio button that takes a
// key, stepping to its next button; a parameter a macro holds can't be
// mapped, and the message names the macro.
#[test]
fn a_radio_button_macro_takes_a_key_and_a_held_parameter_refuses_a_mapping() {
    let mut rig = Rig::new();
    let track = rig.track(TrackKind::Audio);
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    let rack = rig.run(
        "device.add",
        json!({
            "device": {
                "kind": { "rack": { "kind": "audio_effect" } },
                "name": "Rack",
                "role": "audio_effect",
            },
        }),
    )["device"]
        .clone();
    rig.run(
        "rack.set_macro_style",
        json!({ "rack": rack, "macro_number": 1, "style": "radio_buttons" }),
    );
    let macro_one = bl_model::macro_param(1);
    let target = json!({ "type": "device", "device": rack, "param": macro_one });
    let mapped = rig.map(key("r"), target.clone());
    assert_eq!(mapped["kind"]["type"], "radio");
    let value = |rig: &Rig| {
        let rack: bl_model::DeviceId = serde_json::from_value(rack.clone()).unwrap();
        rig.shell
            .editor()
            .document()
            .device(rack)
            .unwrap()
            .params
            .get(&macro_one)
            .copied()
            .unwrap_or(0.0)
    };
    rig.key("r");
    let first = value(&rig);
    rig.key("r");
    assert!(value(&rig) > first, "each key steps to the next button");
    assert_eq!(rig.refused(), 0);

    // A knob macro is a continuous control, mapped to MIDI only.
    let knob = json!({ "type": "device", "device": rack, "param": bl_model::macro_param(2) });
    assert_eq!(
        rig.map(cc(30), knob.clone())["kind"]["type"],
        "continuous"
    );
    assert!(rig
        .shell
        .run("remote.map", json!({ "source": key("k"), "target": knob }))
        .is_err());
}

// Spec: map-modes R24. `remote.targets` lists the controls of each view with
// their IDs, and `remote.map` on one of each kind reads back through
// `remote.mappings`.
#[test]
fn remote_targets_lists_each_kind_and_a_mapping_reads_back() {
    let mut rig = Rig::new();
    let targets = rig.run("remote.targets", json!({ "mode": "midi" }));
    let list = targets["targets"].as_array().unwrap().clone();
    let kinds_of = |kind: &str| -> Vec<&Value> {
        list.iter()
            .filter(|t| t["kind"]["type"] == kind)
            .collect()
    };
    for kind in ["slot", "button", "switch", "ranged_switch", "radio", "continuous"] {
        assert!(!kinds_of(kind).is_empty(), "no {kind}");
    }
    let named = |name: &str| list.iter().any(|t| t["name"] == name);
    for name in [
        "Launch",
        "Scene Down",
        "Crossfader Left",
        "Cue Volume",
        "New",
        "Tempo",
        "Transpose",
        "Step Back",
        "Monitor",
    ] {
        assert!(named(name), "no {name}");
    }
    // Key Map Mode's list has no continuous control.
    let keys = rig.run("remote.targets", json!({ "mode": "key" }));
    assert!(
        keys["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["kind"]["type"] != "continuous")
    );

    // One of each kind maps by the target the list gave and reads back.
    let mut source = 0_u8;
    let mut mapped = 0;
    for kind in ["slot", "button", "switch", "ranged_switch", "radio", "continuous"] {
        let target = kinds_of(kind)[0]["target"].clone();
        source += 1;
        rig.map(cc(source), target.clone());
        let mappings = rig.run("remote.mappings", json!({}));
        assert!(
            mappings["mappings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["target"] == target && m["kind"]["type"] == kind),
            "{kind} didn't read back"
        );
        mapped += 1;
    }
    assert_eq!(mapped, 6);
}
