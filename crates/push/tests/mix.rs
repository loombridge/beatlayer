//! Mix Mode (`docs/specs/push-2-tracks.md`, R28 to R42) on the simulated
//! Push 2: Track Mix, Global Mix, Input & Output, unfolding groups and
//! racks, and Mute, Solo, Delete and Duplicate on tracks.

use bl_control::DrumView;
use bl_control::mix::{Chooser, Entry, InOut};
use bl_model::fader::{fader_gain, fader_position, send_level, send_position};
use bl_model::{
    Chain, ChainId, Command, Device, DeviceId, PanMode, RackKind, Receive, TrackId, TrackKind,
};
use bl_push::content::{DisplayMode, MixPage};
use bl_push::mix::{Global, View};
use serde_json::{Value, json};

use crate::display::golden;
use crate::rig::Rig;

/// A rig in Mix Mode with the first track selected: ten tracks, the first
/// eight in the ring.
fn mixing() -> Rig {
    let mut rig = Rig::mixing();
    rig.model.columns = rig.tracks.clone();
    rig.model.selection.tracks = vec![rig.tracks[0]];
    rig.start();
    rig.tick();
    rig.tap("mix");
    rig
}

fn add_returns(rig: &mut Rig, count: usize) -> Vec<TrackId> {
    (0..count)
        .map(|index| {
            let id: TrackId = rig.editor.new_id();
            rig.apply(Command::CreateTrack {
                id,
                kind: TrackKind::Return,
                name: format!("Return {index}"),
                parent: None,
                index,
            });
            id
        })
        .collect()
}

/// What the display says, line by line.
fn text(rig: &Rig) -> Vec<String> {
    rig.snapshot().screen.text()
}

fn has(rig: &Rig, line: &str) -> bool {
    text(rig).iter().any(|l| l == line)
}

/// The commands asked for since the last call, as an ID and its arguments.
fn commands(rig: &mut Rig) -> Vec<(String, Value)> {
    rig.commands()
        .into_iter()
        .map(|c| match c.split_once(' ') {
            Some((id, args)) if !id.starts_with("end") => (
                id.to_owned(),
                serde_json::from_str(args).unwrap_or(Value::Null),
            ),
            _ => (c, Value::Null),
        })
        .collect()
}

/// One touch of encoder `column`: the steps in `turns`, then the release.
fn turn(rig: &mut Rig, column: u8, turns: &[i32]) -> Vec<(String, Value)> {
    assert!(rig.sim.touch(column));
    for steps in turns {
        assert!(rig.sim.turn(column, *steps));
    }
    assert!(rig.sim.release(column));
    rig.advance(1);
    commands(rig)
}

fn close(a: &Value, b: f64) -> bool {
    a.as_f64().is_some_and(|a| (a - b).abs() < 1e-9)
}

fn state(rig: &Rig) -> bl_push::mix::State {
    rig.snapshot().mix.expect("Mix Mode is shown")
}

// Spec: push-2-tracks R28. Tapping Mix puts Push 2 in Mix Mode, Track Mix the
// first time, and tapping it again switches between Track Mix and Global Mix;
// entered from another mode, it opens in the view it was last in.
#[test]
fn mix_toggles_track_mix_and_global_mix_and_opens_in_the_last_one() {
    let mut rig = mixing();
    let snapshot = rig.snapshot();
    assert_eq!(snapshot.display_mode, DisplayMode::Mix);
    assert_eq!(snapshot.mix_page, MixPage::Track);
    assert!(has(&rig, "upper 1: Mix (selected)"));
    assert!(has(&rig, "upper 2: Input & Output"));

    rig.tap("mix");
    assert_eq!(rig.snapshot().mix_page, MixPage::Global);
    assert!(has(&rig, "upper 1: Volumes (selected)"));
    assert!(has(&rig, "upper 2: Pans"));

    rig.tap("device");
    assert_eq!(rig.snapshot().display_mode, DisplayMode::Device);
    assert!(rig.snapshot().mix.is_none(), "only while Mix is shown");
    rig.tap("mix");
    assert_eq!(rig.snapshot().mix_page, MixPage::Global, "the last view");
}

// Spec: push-2-tracks R29, R35, R1. In Track Mix the encoders set the selected
// track's volume, pan and sends A to F, left to right; one touch is one undo
// step; Shift makes it finer.
#[test]
fn track_mix_encoders_set_the_selected_tracks_volume_pan_and_sends() {
    let mut rig = mixing();
    let returns = add_returns(&mut rig, 2);
    rig.tick();
    let track = rig.tracks[0];
    assert!(has(&rig, "column 1: Volume = 0.0 dB"));
    assert!(has(&rig, "column 2: Pan = C"));
    assert!(has(&rig, "column 3: Send A = -inf dB"));
    assert!(has(&rig, "column 4: Send B = -inf dB"));
    assert!(!text(&rig).iter().any(|l| l.starts_with("column 5")));

    let done = turn(&mut rig, 0, &[1, 4]);
    assert_eq!(done.len(), 3, "{done:?}");
    for ((id, args), steps) in done.iter().zip([1.0, 5.0]) {
        assert_eq!(id, "mixer.set_volume");
        assert_eq!(args["track"], json!(track));
        assert!(close(
            &args["volume"],
            fader_gain(fader_position(1.0) + steps / 200.0)
        ));
    }
    assert!(done[2].0.starts_with("end "), "one touch, one undo step");

    let done = turn(&mut rig, 1, &[10]);
    assert_eq!(done[0].0, "mixer.set_pan");
    assert!(close(&done[0].1["pan"], (0.5 + 10.0 / 200.0) * 2.0 - 1.0));

    let done = turn(&mut rig, 3, &[20]);
    assert_eq!(done[0].0, "mixer.set_send");
    assert_eq!(done[0].1["to"], json!(returns[1]));
    assert!(close(
        &done[0].1["level"],
        send_level(send_position(0.0) + 20.0 / 200.0)
    ));

    // Shift: a tenth of the step.
    assert!(rig.sim.press("shift"));
    rig.advance(1);
    let done = turn(&mut rig, 0, &[10]);
    assert!(close(
        &done[0].1["volume"],
        fader_gain(fader_position(1.0) + 10.0 / 2000.0)
    ));
    assert!(rig.sim.let_go("shift"));
    rig.advance(1);

    // What the window changes shows on the next frame (R3).
    rig.apply(Command::SetVolume { track, volume: 0.5 });
    rig.tick();
    assert!(has(&rig, "column 1: Volume = -6.0 dB"), "{:?}", text(&rig));
}

// Spec: push-2-tracks R29. With more than six returns the two rightmost upper
// buttons are arrows that shift the sends shown by one: with eight returns
// they reach sends G and H.
#[test]
fn with_eight_returns_the_arrows_show_sends_g_and_h() {
    let mut rig = mixing();
    let returns = add_returns(&mut rig, 8);
    rig.tick();
    assert!(has(&rig, "upper 7: <"));
    assert!(has(&rig, "upper 8: >"));
    assert!(has(&rig, "column 8: Send F = -inf dB"));
    rig.tap("upper_8");
    rig.tap("upper_8");
    assert!(has(&rig, "column 3: Send C = -inf dB"));
    assert!(has(&rig, "column 8: Send H = -inf dB"));
    assert_eq!(state(&rig).shift, 2);
    rig.tap("upper_8");
    assert_eq!(state(&rig).shift, 2, "no further than the last send");
    let done = turn(&mut rig, 7, &[10]);
    assert_eq!(done[0].0, "mixer.set_send");
    assert_eq!(done[0].1["to"], json!(returns[7]));
    rig.tap("upper_7");
    assert!(has(&rig, "column 3: Send B = -inf dB"));

    // Six returns fit: no arrows.
    let mut rig = mixing();
    add_returns(&mut rig, 6);
    rig.tick();
    assert!(!has(&rig, "upper 7: <"));
}

// Spec: push-2-tracks R30. A track in Split Stereo Pan Mode has its two pan
// sliders on the second and third encoders and its sends start on the fourth.
#[test]
fn a_split_pan_track_takes_encoders_2_and_3() {
    let mut rig = mixing();
    let returns = add_returns(&mut rig, 1);
    let track = rig.tracks[0];
    rig.apply(Command::SetPanMode {
        track,
        mode: PanMode::Split,
    });
    rig.tick();
    assert!(has(&rig, "column 2: Pan L = C"));
    assert!(has(&rig, "column 3: Pan R = C"));
    assert!(has(&rig, "column 4: Send A = -inf dB"));
    let done = turn(&mut rig, 1, &[20]);
    assert_eq!(done[0].0, "mixer.set_split_pan_left");
    assert!(close(&done[0].1["pan"], (0.5 + 20.0 / 200.0) * 2.0 - 1.0));
    let done = turn(&mut rig, 2, &[-20]);
    assert_eq!(done[0].0, "mixer.set_split_pan_right");
    assert!(close(&done[0].1["pan"], (0.5 - 20.0 / 200.0) * 2.0 - 1.0));
    let done = turn(&mut rig, 3, &[20]);
    assert_eq!(done[0].0, "mixer.set_send");
    assert_eq!(done[0].1["to"], json!(returns[0]));
}

fn entry(label: &str, chosen: bool, command: &str, args: Value) -> Entry {
    Entry {
        label: label.to_owned(),
        chosen,
        command: command.to_owned(),
        args,
    }
}

fn chooser(name: &str, entries: Vec<Entry>) -> Option<Chooser> {
    let text = entries
        .iter()
        .find(|e| e.chosen)
        .map_or(String::new(), |e| e.label.clone());
    Some(Chooser {
        name: name.to_owned(),
        text,
        entries,
    })
}

// Spec: push-2-tracks R31, R4. With Input & Output chosen the encoders step the
// selected track's Input Type, Input Channel, Monitor, Output Type and Output
// Channel through their choices, one for every ten steps; a track without one
// leaves its encoder empty.
#[test]
fn input_and_output_steps_input_type_and_monitor_through_their_choices() {
    let mut rig = mixing();
    let track = rig.tracks[0];
    let set_input = |input: Value| json!({ "track": track, "input": input });
    let monitor = |state: &str| json!({ "track": track, "monitor": state });
    rig.model.mix.in_out = Some(InOut {
        track,
        choosers: [
            chooser(
                "Input Type",
                vec![
                    entry(
                        "Ext. In",
                        true,
                        "routing.set_input",
                        set_input(json!({"type": "external"})),
                    ),
                    entry(
                        "Resampling",
                        false,
                        "routing.set_input",
                        set_input(json!({"type": "resampling"})),
                    ),
                    entry(
                        "No Input",
                        false,
                        "routing.set_input",
                        set_input(json!({"type": "none"})),
                    ),
                ],
            ),
            None,
            chooser(
                "Monitor",
                vec![
                    entry("In", false, "routing.set_monitor", monitor("in")),
                    entry("Auto", true, "routing.set_monitor", monitor("auto")),
                    entry("Off", false, "routing.set_monitor", monitor("off")),
                ],
            ),
            None,
            None,
        ],
    });
    rig.tap("upper_2");
    assert_eq!(state(&rig).view, View::InOut);
    assert!(has(&rig, "upper 2: Input & Output (selected)"));
    assert!(has(&rig, "column 1: Input Type = Ext. In"));
    assert!(has(&rig, "column 3: Monitor = Auto"));
    assert!(!text(&rig).iter().any(|l| l.starts_with("column 2")));
    assert!(!text(&rig).iter().any(|l| l.starts_with("column 4")));

    // Nine steps are not a choice; ten are, and a turn that goes on before
    // the document shows the first choice goes on from it.
    assert!(turn(&mut rig, 0, &[9]).is_empty());
    let done = turn(&mut rig, 0, &[10, 10]);
    assert_eq!(done[0].0, "routing.set_input");
    assert_eq!(done[0].1["input"], json!({"type": "resampling"}));
    assert_eq!(done[1].1["input"], json!({"type": "none"}));
    let done = turn(&mut rig, 2, &[-10]);
    assert_eq!(done[0], ("routing.set_monitor".to_owned(), monitor("in")));
    // An encoder with nothing does nothing.
    assert!(turn(&mut rig, 1, &[30]).is_empty());
    assert!(turn(&mut rig, 4, &[30]).is_empty());
}

// Spec: push-2-tracks R32, R33, R34. Global Mix's upper buttons choose what the
// encoders set on the eight tracks: Pans sets eight pans, a split-pan track's
// encoder does nothing, and the arrows move the ring one track past eight.
#[test]
fn global_mix_sets_pans_on_eight_tracks_and_shifts_them_past_eight() {
    let mut rig = mixing();
    let split = rig.tracks[2];
    rig.apply(Command::SetPanMode {
        track: split,
        mode: PanMode::Split,
    });
    rig.tap("mix");
    assert!(has(&rig, "column 1: Volume = 0.0 dB"));
    rig.tap("upper_2");
    assert_eq!(state(&rig).global, Global::Pans);
    for column in 0..8 {
        assert!(has(&rig, &format!("column {}: Pan = C", column + 1)));
    }
    for (column, track) in rig.tracks.clone().iter().take(8).enumerate() {
        let done = turn(&mut rig, u8::try_from(column).unwrap(), &[10]);
        if *track == split {
            assert!(done.is_empty(), "a split-pan track's encoder is inert");
            assert!(state(&rig).encoders[column].as_ref().unwrap().inert);
        } else {
            assert_eq!(done[0].0, "mixer.set_pan");
            assert_eq!(done[0].1["track"], json!(track));
        }
    }

    // Ten tracks: the right arrow moves the ring a track.
    rig.tap("right");
    assert_eq!(
        commands(&mut rig),
        [(
            "control_surface.move_ring".to_owned(),
            json!({ "row": 1, "step": "right" })
        )]
    );
    rig.tap("left");
    assert_eq!(commands(&mut rig)[0].1["step"], json!("left"));
    // Eight tracks do not need them.
    rig.model.columns.truncate(8);
    rig.tick();
    rig.tap("right");
    assert!(commands(&mut rig).is_empty());
}

// Spec: push-2-tracks R32. Past six returns the rightmost upper button is an
// arrow that shifts the sends' buttons by one, round to Send A after the last.
#[test]
fn global_mix_sends_buttons_shift_and_wrap() {
    let mut rig = mixing();
    let returns = add_returns(&mut rig, 8);
    rig.tap("mix");
    assert!(has(&rig, "upper 3: Send A"));
    assert!(has(&rig, "upper 7: Send E"));
    assert!(has(&rig, "upper 8: >"));
    rig.tap("upper_8");
    assert!(has(&rig, "upper 3: Send B"));
    rig.tap("upper_8");
    rig.tap("upper_8");
    assert!(has(&rig, "upper 3: Send D"));
    assert!(has(&rig, "upper 7: Send H"));
    rig.tap("upper_8");
    assert!(has(&rig, "upper 3: Send A"), "round to Send A");
    rig.tap("upper_8");
    rig.tap("upper_7");
    assert_eq!(
        state(&rig).global,
        Global::Send(5),
        "Send F after one shift"
    );
    assert!(has(&rig, "column 1: Send F = -inf dB"));
    let done = turn(&mut rig, 1, &[10]);
    assert_eq!(done[0].1["to"], json!(returns[5]));
    assert_eq!(done[0].1["track"], json!(rig.tracks[1]));
}

// Spec: push-2-tracks R36. Master selects the Main track, so Track Mix sets
// its volume and pan; pressing it again selects the track selected before.
// It is lit while the Main track is selected.
#[test]
fn master_selects_main_and_back() {
    let mut rig = mixing();
    let main = rig.model.document.main_id();
    assert_eq!(
        rig.snapshot().led("master").unwrap().colour,
        bl_push::leds::white::DIM
    );
    rig.tap("master");
    assert_eq!(
        commands(&mut rig),
        [(
            "arrangement.select_tracks".to_owned(),
            json!({ "tracks": [main] })
        )]
    );
    rig.model.selection.tracks = vec![main];
    rig.tick();
    assert_eq!(
        rig.snapshot().led("master").unwrap().colour,
        bl_push::leds::white::FULL
    );
    assert!(has(&rig, "column 1: Volume = 0.0 dB"));
    let done = turn(&mut rig, 0, &[10]);
    assert_eq!(done[0].1["track"], json!(main));
    assert!(
        !text(&rig).iter().any(|l| l.starts_with("column 3")),
        "no sends"
    );

    rig.tap("master");
    assert_eq!(
        commands(&mut rig),
        [(
            "arrangement.select_tracks".to_owned(),
            json!({ "tracks": [rig.tracks[0]] })
        )]
    );
}

fn group_with_children(rig: &mut Rig) -> (TrackId, Vec<TrackId>) {
    let group: TrackId = rig.editor.new_id();
    rig.apply(Command::CreateTrack {
        id: group,
        kind: TrackKind::Group,
        name: "Strings".to_owned(),
        parent: None,
        index: 10,
    });
    let children: Vec<TrackId> = (0..2)
        .map(|index| {
            let id: TrackId = rig.editor.new_id();
            rig.apply(Command::CreateTrack {
                id,
                kind: TrackKind::Audio,
                name: format!("Violin {index}"),
                parent: Some(group),
                index,
            });
            id
        })
        .collect();
    (group, children)
}

// Spec: push-2-tracks R37. A group track is drawn with an icon; a selected
// one's second press folds or unfolds it, as its fold in the window does, and
// the tracks inside show after it with a line under the group to the last of
// them.
#[test]
fn unfolding_a_group_shows_its_tracks_in_the_window_too() {
    let mut rig = mixing();
    let (group, children) = group_with_children(&mut rig);
    // The window has the group folded: its tracks aren't in the columns, and
    // the ring starts on the group.
    rig.model.columns = rig.tracks.iter().copied().chain([group]).collect();
    rig.model.mix.folded_groups = vec![group];
    rig.model.rows[0].ring.as_mut().unwrap().first_track = Some(rig.tracks[5]);
    rig.model.selection.tracks = vec![rig.tracks[0]];
    rig.tick();
    let screen = rig.snapshot().screen;
    let at = screen
        .lower
        .iter()
        .position(|l| l.text == "Strings")
        .expect("the group is among the eight");
    assert_eq!(screen.lower[at].icon, bl_push::screen::Icon::Group);
    assert!(screen.underlines.is_empty(), "folded");

    // The first press selects it; the second unfolds it.
    rig.tap(&format!("lower_{}", at + 1));
    assert_eq!(
        commands(&mut rig),
        [
            ("session.select".to_owned(), json!({ "track": group })),
            (
                "arrangement.select_tracks".to_owned(),
                json!({ "tracks": [group] })
            ),
        ]
    );
    rig.model.selection.tracks = vec![group];
    rig.tick();
    rig.tap(&format!("lower_{}", at + 1));
    assert_eq!(
        commands(&mut rig),
        [(
            "arrangement.fold_groups".to_owned(),
            json!({ "fold": false })
        )]
    );
    rig.model.mix.folded_groups.clear();
    rig.model.columns = rig
        .tracks
        .iter()
        .copied()
        .chain([group])
        .chain(children.iter().copied())
        .collect();
    rig.model.rows[0].ring.as_mut().unwrap().first_track = Some(rig.tracks[5]);
    rig.tick();
    let screen = rig.snapshot().screen;
    // The ring can't start so far on that the eight aren't full: the group
    // is further left now.
    let at = screen
        .lower
        .iter()
        .position(|l| l.text == "Strings")
        .unwrap();
    assert_eq!(screen.lower[at + 1].text, "Violin 0");
    assert_eq!(screen.lower[at + 2].text, "Violin 1");
    let at = u8::try_from(at).unwrap();
    assert_eq!(
        screen
            .underlines
            .iter()
            .map(|u| (u.0, u.1))
            .collect::<Vec<_>>(),
        [(at, at + 2)]
    );
    assert!(has(&rig, &format!("underline: {} to {}", at + 1, at + 3)));

    // Again folds it.
    rig.tap(&format!("lower_{}", at + 1));
    assert_eq!(
        commands(&mut rig),
        [(
            "arrangement.fold_groups".to_owned(),
            json!({ "fold": true })
        )]
    );
}

struct Drums {
    track: TrackId,
    rack: DeviceId,
    chains: Vec<ChainId>,
    returns: Vec<ChainId>,
}

/// A track with a drum rack of three drum chains on the notes 36 to 38 and
/// two return chains, in track 2's place.
fn drum_rack(rig: &mut Rig) -> Drums {
    let track = rig.tracks[1];
    let rack: DeviceId = rig.editor.new_id();
    let mut device = Device::rack(rack, RackKind::Drum);
    let mut chains = Vec::new();
    let mut returns = Vec::new();
    for n in 0..3u8 {
        let id: ChainId = rig.editor.new_id();
        let mut chain = Chain::new(id, format!("Drum {n}"));
        chain.receive = Receive::Note(36 + n);
        device = device.with_chain(chain);
        chains.push(id);
    }
    for n in 0..2 {
        let id: ChainId = rig.editor.new_id();
        device = device.with_chain(Chain::new_return(id, format!("Return {n}")));
        returns.push(id);
    }
    rig.apply(Command::InsertDevice {
        track,
        index: 0,
        device,
    });
    Drums {
        track,
        rack,
        chains,
        returns,
    }
}

// Spec: push-2-tracks R37, R38. A rack track is drawn with an icon; a selected
// one's second press unfolds its chains, which show after it with a line under
// the track; a chain is mixed like a track, and in a drum rack its sends go to
// the rack's return chains; Select with a pad reaches the pad's chain.
#[test]
fn unfolding_a_drum_rack_mixes_its_chains_and_sends_and_select_with_a_pad_reaches_one() {
    let mut rig = mixing();
    let drums = drum_rack(&mut rig);
    rig.model.selection.tracks = vec![drums.track];
    rig.tick();
    let screen = rig.snapshot().screen;
    assert_eq!(screen.lower[1].icon, bl_push::screen::Icon::Rack);

    // The second press, the track being selected, unfolds the chains.
    rig.tap("lower_2");
    assert_eq!(
        commands(&mut rig),
        [(
            "mixer_view.fold_chains".to_owned(),
            json!({ "rack": drums.rack, "shown": true })
        )]
    );
    rig.model.mix.open_racks = vec![drums.rack];
    rig.tick();
    let names: Vec<String> = rig
        .snapshot()
        .screen
        .lower
        .iter()
        .map(|l| l.text.clone())
        .collect();
    assert_eq!(
        names,
        [
            "Track 1", "Track 2", "Drum 0", "Drum 1", "Drum 2", "Return 0", "Return 1", "Track 3"
        ]
    );
    assert!(has(&rig, "underline: 2 to 7"));
    assert_eq!(state(&rig).unfolded_racks, [drums.rack]);

    // Global Mix: the chains' volumes, then their sends to the return chains.
    rig.tap("mix");
    let done = turn(&mut rig, 2, &[10]);
    assert_eq!(done[0].0, "chain.set_gain");
    assert_eq!(done[0].1["chain"], json!(drums.chains[0]));
    assert!(close(
        &done[0].1["volume"],
        fader_gain(fader_position(1.0) + 10.0 / 200.0)
    ));
    // A return chain of a drum rack has volume and pan, but no sends.
    // The sends' buttons run along the rack's return chains too.
    rig.tap("upper_3");
    assert_eq!(state(&rig).global, Global::Send(0));
    let done = turn(&mut rig, 3, &[20]);
    assert_eq!(done[0].0, "chain.set_send");
    assert_eq!(done[0].1["chain"], json!(drums.chains[1]));
    assert_eq!(done[0].1["to"], json!(drums.returns[0]));
    assert!(
        turn(&mut rig, 6, &[20]).is_empty(),
        "a return chain has none"
    );
    // The tracks send to the set's returns, of which there are none here.
    assert!(turn(&mut rig, 0, &[20]).is_empty());
    rig.tap("mix");

    // Track Mix mixes the chain pressed.
    rig.tap("lower_4");
    assert_eq!(
        commands(&mut rig),
        [(
            "chain.select".to_owned(),
            json!({ "chain": drums.chains[1] })
        )]
    );
    assert!(has(&rig, "column 1: Volume = 0.0 dB"));
    assert!(has(&rig, "column 2: Pan = C"));
    assert!(has(&rig, "column 3: Send A = -inf dB"));
    assert!(has(&rig, "column 4: Send B = -inf dB"));
    let done = turn(&mut rig, 1, &[10]);
    assert_eq!(done[0].0, "chain.set_pan");
    assert_eq!(done[0].1["chain"], json!(drums.chains[1]));
    let done = turn(&mut rig, 3, &[20]);
    assert_eq!(done[0].0, "chain.set_send");
    assert_eq!(done[0].1["to"], json!(drums.returns[1]));

    // Select with a pad: the pad's chain, and the eight move to it.
    rig.model.drum = Some(DrumView {
        rack: drums.rack,
        first: 36,
        selected: None,
        filled: 0b111 << 36,
    });
    rig.tap("lower_2");
    rig.model.selection.tracks = vec![drums.track];
    rig.tick();
    commands(&mut rig);
    assert!(rig.sim.press("select"));
    rig.advance(1);
    assert!(rig.sim.press_pad(0, 2, 100));
    rig.advance(1);
    assert!(rig.sim.release_pad(0, 2));
    rig.advance(1);
    assert!(rig.sim.let_go("select"));
    rig.advance(1);
    let done = commands(&mut rig);
    assert_eq!(
        done,
        [(
            "pad.select".to_owned(),
            json!({ "rack": drums.rack, "note": 38 })
        )]
    );
    assert!(rig.reached_tracks().is_empty(), "the pad played nothing");
    let selected: Vec<_> = state(&rig)
        .places
        .iter()
        .filter(|p| p.selected)
        .map(|p| p.chain)
        .collect();
    assert_eq!(selected, [Some(drums.chains[2])]);

    // Folded back in, the chain is no place to mix: the track is the subject.
    rig.model.mix.open_racks.clear();
    rig.tick();
    assert_eq!(
        state(&rig)
            .places
            .iter()
            .filter(|p| p.selected)
            .map(|p| (p.track, p.chain))
            .collect::<Vec<_>>(),
        [(drums.track, None)]
    );
}

// Spec: push-2-tracks R39. Mute switches the selected track's Track Activator
// and Solo its Solo; holding either with a lower button does it to that track;
// while either is locked each lower button switches its track; both are lit
// while the selected track is muted or soloed; a lower-button solo takes the
// others off, as a click does.
#[test]
fn mute_and_solo_act_on_the_selected_track_held_with_a_lower_button_and_locked() {
    let mut rig = mixing();
    let t = rig.tracks.clone();
    let mute = |track: TrackId, on: bool| {
        (
            "mixer.set_mute".to_owned(),
            json!({ "track": track, "mute": on }),
        )
    };
    rig.tap("mute");
    assert_eq!(commands(&mut rig), [mute(t[0], true)]);
    assert_eq!(rig.snapshot().led("mute").unwrap().colour, 0);
    rig.apply(Command::SetMute {
        track: t[0],
        mute: true,
    });
    rig.tick();
    assert_ne!(
        rig.snapshot().led("mute").unwrap().colour,
        0,
        "lit while muted"
    );
    rig.tap("mute");
    assert_eq!(commands(&mut rig), [mute(t[0], false)]);

    // Held with a lower button: that track, and the Mute button's own action
    // doesn't follow when it is let go.
    assert!(rig.sim.press("mute"));
    rig.advance(10);
    rig.tap("lower_3");
    assert!(rig.sim.let_go("mute"));
    rig.advance(10);
    assert_eq!(commands(&mut rig), [mute(t[2], true)]);

    // Locked: every lower button switches its track, and a tap selects none.
    rig.apply(Command::SetMute {
        track: t[2],
        mute: true,
    });
    assert!(rig.sim.press("mute"));
    rig.advance(1100);
    assert!(rig.sim.let_go("mute"));
    rig.advance(10);
    assert!(rig.snapshot().locked.contains(&"mute"));
    rig.tap("lower_3");
    rig.tap("lower_5");
    assert_eq!(commands(&mut rig), [mute(t[2], false), mute(t[4], true)]);
    rig.tap("mute");
    assert!(!rig.snapshot().locked.contains(&"mute"));
    commands(&mut rig);

    // Solo: exclusive, as a click; the only one soloed goes off.
    rig.tap("solo");
    assert_eq!(
        commands(&mut rig),
        [("mixer.solo_exclusive".to_owned(), json!({ "track": t[0] }))]
    );
    assert!(rig.sim.press("solo"));
    rig.advance(10);
    rig.tap("lower_4");
    assert!(rig.sim.let_go("solo"));
    rig.advance(10);
    assert_eq!(
        commands(&mut rig),
        [("mixer.solo_exclusive".to_owned(), json!({ "track": t[3] }))]
    );
    rig.apply(Command::SetSolo {
        track: t[3],
        solo: true,
    });
    rig.tick();
    assert!(rig.sim.press("solo"));
    rig.advance(10);
    rig.tap("lower_4");
    assert!(rig.sim.let_go("solo"));
    rig.advance(10);
    assert_eq!(
        commands(&mut rig),
        [(
            "mixer.set_solo".to_owned(),
            json!({ "track": t[3], "solo": false })
        )]
    );
}

// Spec: push-2-tracks R40. Delete and a lower display button delete the track;
// Duplicate and one duplicate it; the Main track can't be deleted because it
// isn't one of the eight.
#[test]
fn delete_and_duplicate_with_a_lower_button() {
    let mut rig = mixing();
    let t = rig.tracks.clone();
    assert!(rig.sim.press("delete"));
    rig.advance(10);
    rig.tap("lower_2");
    assert!(rig.sim.let_go("delete"));
    rig.advance(10);
    assert_eq!(
        commands(&mut rig),
        [("track.delete".to_owned(), json!({ "track": t[1] }))]
    );
    assert!(rig.sim.press("duplicate"));
    rig.advance(10);
    rig.tap("lower_3");
    assert!(rig.sim.let_go("duplicate"));
    rig.advance(10);
    assert_eq!(
        commands(&mut rig),
        [
            (
                "arrangement.select_tracks".to_owned(),
                json!({ "tracks": [t[2]] })
            ),
            ("track.duplicate".to_owned(), json!({})),
        ]
    );
}

// Spec: push-2-tracks R41, R42. The display draws each place's name over its
// lower button in its colour and, over its encoder, the control it sets; and
// `push.state` reads the mode back.
#[test]
fn the_display_and_the_state_show_names_values_and_the_underline() {
    let mut rig = mixing();
    add_returns(&mut rig, 3);
    let drums = drum_rack(&mut rig);
    rig.model.selection.tracks = vec![drums.track];
    rig.model.mix.open_racks = vec![drums.rack];
    rig.tick();
    rig.apply(Command::SetVolume {
        track: drums.track,
        volume: 0.5,
    });
    rig.tick();
    let screen = rig.snapshot().screen;
    lines(&text(&rig));
    golden("mix", &screen);

    let state = state(&rig);
    assert_eq!(state.view, View::Mix);
    let volume = state.encoders[0].as_ref().unwrap();
    assert_eq!(volume.name, "Volume");
    assert_eq!(volume.value, "-6.0 dB");
    assert!(volume.position.is_some() && !volume.inert);
    assert_eq!(state.places.len(), 8);
    assert!(state.places[1].selected);
}

fn lines(lines: &[String]) {
    assert!(
        lines
            .iter()
            .any(|l| l == "lower 2: Track 2 [rack] (selected)")
    );
    assert!(lines.iter().any(|l| l == "lower 3: Drum 0"));
    assert!(lines.iter().any(|l| l == "underline: 2 to 7"));
    assert!(lines.iter().any(|l| l == "column 1: Volume = -6.0 dB"));
    assert!(lines.iter().any(|l| l == "column 3: Send A = -inf dB"));
}

// Spec: push-2-tracks R37, push-2 R27. Shift and a lower display button open
// the colour picker for a track, or for a chain of an unfolded rack, and a ring
// pad colours it.
#[test]
fn shift_and_a_lower_button_open_the_colour_picker_for_a_track_or_a_chain() {
    let mut rig = mixing();
    let drums = drum_rack(&mut rig);
    rig.model.mix.open_racks = vec![drums.rack];
    rig.tick();
    for (button, command, args) in [
        (
            "lower_1",
            "track.set_colour",
            json!({ "tracks": [rig.tracks[0]], "colour": 9 }),
        ),
        (
            "lower_3",
            "chain.set_colour",
            json!({ "chain": drums.chains[0], "colour": 9 }),
        ),
    ] {
        assert!(rig.sim.press("shift"));
        rig.advance(10);
        rig.tap(button);
        assert!(rig.sim.let_go("shift"));
        rig.advance(10);
        assert!(rig.snapshot().picker);
        assert!(rig.sim.press_pad(7, 2, 100));
        assert!(rig.sim.release_pad(7, 2));
        rig.advance(10);
        assert_eq!(commands(&mut rig), [(command.to_owned(), args)]);
        assert!(!rig.snapshot().picker);
    }
}
